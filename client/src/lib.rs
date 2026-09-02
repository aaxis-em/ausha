//! Everything a receiver needs except the audio device.
//!
//! Owns the sockets and the threads that feed the pipeline, so the desktop
//! receiver and the Android app differ only in where the samples end up.

mod session;

pub use ausha_core::config;
pub use ausha_core::pipeline::{Latency, Report, Stats};
pub use ausha_core::protocol::StreamParams;

use std::io;
use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ausha_core::pipeline::Pipeline;

pub struct Config {
    pub host: String,
    pub control_port: u16,
    pub token: String,
    pub name: String,
    /// Drops this share of received packets, to exercise concealment against a
    /// real sender.
    pub simulate_loss: u32,
    pub latency: Latency,
}

/// One datagram as it came off the wire, with the moment it arrived.
type Arrival = (Vec<u8>, u64);

/// Readable from any thread, so a UI can show what playback is doing without
/// touching the pipeline.
#[derive(Clone)]
pub struct Monitor {
    stats: Arc<Mutex<Stats>>,
    running: Arc<AtomicBool>,
}

impl Monitor {
    pub fn stats(&self) -> Stats {
        self.stats.lock().map(|s| *s).unwrap_or_default()
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::Relaxed);
    }
}

/// Drives playback. Lives on whichever thread pulls audio, and is the only
/// thing that touches the pipeline.
pub struct Client {
    pipeline: Pipeline,
    incoming: Receiver<Arrival>,
    monitor: Monitor,
    params: StreamParams,
    threads: Vec<JoinHandle<()>>,
}

impl Client {
    /// Completes the handshake and starts receiving before returning, so a
    /// caller that gets a `Client` back already has media arriving.
    pub fn connect(config: &Config) -> io::Result<Self> {
        let session = session::connect(
            &config.host,
            config.control_port,
            &config.token,
            &config.name,
        )?;
        let params = session.params.clone();
        let pipeline = Pipeline::with_latency(&params, config.latency).map_err(io::Error::other)?;

        let monitor = Monitor {
            stats: Arc::new(Mutex::new(pipeline.stats())),
            running: Arc::new(AtomicBool::new(true)),
        };

        let (arrivals, incoming) = mpsc::channel();
        let media = session.media.try_clone()?;
        let mut threads = Vec::new();
        threads.push(thread::spawn({
            let running = monitor.running.clone();
            let loss = config.simulate_loss;
            move || receive_datagrams(media, arrivals, loss, running)
        }));
        threads.push(thread::spawn({
            let monitor = monitor.clone();
            move || keep_session_alive(session, monitor)
        }));

        Ok(Self {
            pipeline,
            incoming,
            monitor,
            params,
            threads,
        })
    }

    pub fn params(&self) -> &StreamParams {
        &self.params
    }

    pub fn monitor(&self) -> Monitor {
        self.monitor.clone()
    }

    pub fn is_running(&self) -> bool {
        self.monitor.is_running()
    }

    /// Fills `output` with interleaved samples. Call from the thread that owns
    /// the audio device; never blocks.
    pub fn fill(&mut self, output: &mut [f32]) -> Report {
        loop {
            match self.incoming.try_recv() {
                Ok((datagram, arrival_us)) => self.pipeline.on_datagram(&datagram, arrival_us),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.monitor.stop();
                    break;
                }
            }
        }
        let report = self.pipeline.fill(output, now_us());
        if let Ok(mut stats) = self.monitor.stats.try_lock() {
            *stats = self.pipeline.stats();
        }
        report
    }

    pub fn stats(&self) -> Stats {
        self.pipeline.stats()
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.monitor.stop();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

/// Hands datagrams to the playback thread without ever blocking it, the shape
/// a real audio callback needs.
fn receive_datagrams(
    media: UdpSocket,
    arrivals: Sender<Arrival>,
    simulate_loss: u32,
    running: Arc<AtomicBool>,
) {
    let mut buf = [0u8; ausha_core::config::MAX_DATAGRAM];
    // Independent random loss, not every Nth packet: periodic dropping never
    // produces the consecutive runs that are the hard case for a jitter buffer.
    let mut rng = 0x2545_f491_4f6c_dd1du64;
    while running.load(Ordering::Relaxed) {
        let Ok((n, _)) = media.recv_from(&mut buf) else {
            continue;
        };
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        if simulate_loss > 0 && rng % 100 < u64::from(simulate_loss) {
            continue;
        }
        if arrivals.send((buf[..n].to_vec(), now_us())).is_err() {
            return;
        }
    }
}

/// Answers pings and forwards reception stats until either side stops.
fn keep_session_alive(mut session: session::Session, monitor: Monitor) {
    let mut last_report = Instant::now();
    while monitor.is_running() {
        match session.poll() {
            Ok(true) => {}
            Ok(false) | Err(_) => break,
        }
        if last_report.elapsed() >= Duration::from_secs(5) {
            let stats = monitor.stats();
            let _ = session.report(
                loss_ratio(&stats.jitter),
                stats.jitter.jitter_ms.round() as u32,
                stats.buffered_ms.round() as u32,
            );
            last_report = Instant::now();
        }
    }
    monitor.stop();
    session.say_goodbye();
}

pub fn loss_ratio(jitter: &ausha_core::jitter::Stats) -> f32 {
    match jitter.received + jitter.lost {
        0 => 0.0,
        total => jitter.lost as f32 / total as f32,
    }
}

fn now_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_micros() as u64
}
