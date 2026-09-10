//! The microphone channel from a receiver, decoded into a virtual capture
//! device the rest of the desktop can select.
//!
//! The mirror of the downlink: the same RTP framing, the same jitter buffer,
//! FEC and concealment, running here instead of on the phone.

use std::io;
use std::net::{Ipv4Addr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ausha_core::config;
use ausha_core::crypto::{Direction, Opener, Secret};
use ausha_core::pipeline::{Latency, Pipeline};
use ausha_core::protocol::UplinkParams;

use crate::pipesource::PipeSource;

/// Short enough that the stop flag is noticed promptly, and short enough that a
/// frame is never more than this late.
const POLL_TIMEOUT: Duration = Duration::from_millis(5);

const REPORT_INTERVAL: Duration = Duration::from_secs(5);

/// The capture device and the socket behind it, both alive for the sender's
/// whole run.
///
/// The device deliberately outlives any one session. Tying it to the session
/// looked tidier, but a call application that has already selected it does not
/// cope with it disappearing: when the phone dropped mid-call the application
/// silently fell back to the laptop's own microphone, which is worse than
/// silence because nobody notices. Idle, the device produces silence — a muted
/// microphone, which is what it is.
pub struct Listener {
    port: u16,
    stream: Shared,
    running: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

/// One session's claim on the microphone. Dropping it stops the audio without
/// taking the device away.
pub struct Active {
    stream: Shared,
}

type Shared = Arc<Mutex<Option<Stream>>>;

/// What a claimed uplink decodes with.
struct Stream {
    pipeline: Pipeline,
    opener: Option<Opener>,
    expected_ssrc: u32,
}

impl Listener {
    pub fn bind(port: u16, source_name: &str) -> io::Result<Self> {
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, port))?;
        socket.set_read_timeout(Some(POLL_TIMEOUT))?;
        let port = socket.local_addr()?.port();
        // The pump thread holds the only reference; the socket stays bound for
        // as long as it runs, and `Drop` stops it.
        let socket = Arc::new(socket);

        let pipe = PipeSource::open(source_name, config::SAMPLE_RATE, config::UPLINK_CHANNELS)?;
        let stream: Shared = Arc::new(Mutex::new(None));
        let running = Arc::new(AtomicBool::new(true));
        let thread = thread::spawn({
            let socket = socket.clone();
            let stream = stream.clone();
            let running = running.clone();
            move || pump(socket, pipe, stream, running)
        });

        Ok(Self {
            port,
            stream,
            running,
            thread: Some(thread),
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn claim(&self, params: &UplinkParams, secret: Option<&Secret>) -> io::Result<Active> {
        let pipeline =
            Pipeline::with_latency(&params.stream, Latency::Voice).map_err(io::Error::other)?;
        let stream = Stream {
            pipeline,
            opener: secret.map(|secret| Opener::new(&secret.key(Direction::Uplink))),
            expected_ssrc: params.stream.ssrc,
        };
        *lock(&self.stream) = Some(stream);
        Ok(Active {
            stream: self.stream.clone(),
        })
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Active {
    fn drop(&mut self) {
        *lock(&self.stream) = None;
    }
}

/// Reads datagrams and produces audio on its own clock.
///
/// Nothing else paces this. The desktop receiver is paced by its audio device
/// and the phone by `AudioTrack`, but a pipe accepts whatever it is given as
/// fast as it is given, so the frame the pipe wants has to be metered out here.
fn pump(socket: Arc<UdpSocket>, mut pipe: PipeSource, stream: Shared, running: Arc<AtomicBool>) {
    let frame_len = (config::SAMPLE_RATE / 1000 * config::FRAME_MS) as usize
        * usize::from(config::UPLINK_CHANNELS);
    let mut buf = [0u8; config::MAX_DATAGRAM];
    let mut chunk = vec![0.0f32; frame_len];
    let mut clock = Clock::new(config::SAMPLE_RATE * u32::from(config::UPLINK_CHANNELS));
    let mut discarding = false;
    let mut last_report = Instant::now();

    while running.load(Ordering::Relaxed) {
        if clock.due() {
            let claimed = match lock(&stream).as_mut() {
                Some(stream) => {
                    stream.pipeline.fill(&mut chunk, now_us());
                    true
                }
                // A muted microphone rather than a starved one, so that an
                // application holding it open between calls sees silence.
                None => {
                    chunk.fill(0.0);
                    false
                }
            };
            let before = pipe.dropped();
            if let Err(e) = pipe.write(&chunk) {
                eprintln!("uplink: microphone pipe failed: {e}");
                return;
            }
            // Only worth saying while somebody is actually talking into it: an
            // idle device that nothing is recording is the normal state.
            if claimed {
                discarding = report_discards(discarding, pipe.dropped() > before);
            }
            clock.produced(chunk.len() as u64);
        }

        if last_report.elapsed() >= REPORT_INTERVAL {
            if let Some(stream) = lock(&stream).as_ref() {
                report(&stream.pipeline.stats());
            }
            last_report = Instant::now();
        }

        let Ok((n, _)) = socket.recv_from(&mut buf) else {
            continue;
        };
        let Some(stream) = &mut *lock(&stream) else {
            continue;
        };
        // Any packet reaching this port belongs to the one session that holds
        // the uplink; the SSRC the sender handed out is what identifies it, and
        // it survives a NAT rebinding that the source address would not.
        let datagram = match &mut stream.opener {
            Some(opener) => match opener.open(&buf[..n]) {
                Some(plain) => plain,
                None => continue,
            },
            None => buf[..n].to_vec(),
        };
        if ssrc_of(&datagram) == Some(stream.expected_ssrc) {
            stream.pipeline.on_datagram(&datagram, now_us());
        }
    }
}

/// A poisoned lock here means a panic while decoding one frame, which says
/// nothing about the next one, so the microphone keeps working.
fn lock(stream: &Shared) -> std::sync::MutexGuard<'_, Option<Stream>> {
    stream.lock().unwrap_or_else(|e| e.into_inner())
}

/// The uplink's own reception figures. The downlink's are reported by whoever
/// is playing it; this end is the only place the microphone's are visible.
fn report(stats: &ausha_core::pipeline::Stats) {
    let jitter = stats.jitter;
    let total = jitter.received + jitter.lost;
    if total == 0 {
        println!("uplink: no microphone audio arriving");
        return;
    }
    println!(
        "uplink: {} packets, {:.2}% lost ({} by FEC, {} concealed), depth {}/{} ms, \
         jitter {:.1} ms, {} underruns",
        jitter.received,
        jitter.lost as f64 * 100.0 / total as f64,
        jitter.recovered,
        jitter.concealed,
        jitter.depth_ms,
        jitter.target_ms,
        jitter.jitter_ms,
        jitter.underruns,
    );
}

fn report_discards(was_discarding: bool, discarding: bool) -> bool {
    match (was_discarding, discarding) {
        (false, true) => println!("uplink: nothing is recording from ausha, discarding audio"),
        (true, false) => println!("uplink: something is recording from ausha again"),
        _ => {}
    }
    discarding
}

fn ssrc_of(datagram: &[u8]) -> Option<u32> {
    ausha_core::rtp::parse(datagram)
        .ok()
        .map(|packet| packet.ssrc)
}

/// Counts out real time in samples, so that a slow iteration is made up rather
/// than accumulating into drift.
struct Clock {
    start: Instant,
    produced: u64,
    per_second: u64,
}

impl Clock {
    fn new(per_second: u32) -> Self {
        Self {
            start: Instant::now(),
            produced: 0,
            per_second: u64::from(per_second),
        }
    }

    fn due(&self) -> bool {
        self.start.elapsed() >= self.elapsed_produced()
    }

    fn produced(&mut self, samples: u64) {
        self.produced += samples;
        // Falling far behind — a suspended machine, say — would otherwise be
        // made up as one long burst into the pipe.
        if self.elapsed_produced() + Duration::from_millis(200) < self.start.elapsed() {
            self.produced = self.start.elapsed().as_micros() as u64 * self.per_second / 1_000_000;
        }
    }

    fn elapsed_produced(&self) -> Duration {
        Duration::from_micros(self.produced * 1_000_000 / self.per_second)
    }
}

fn now_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_micros() as u64
}
