mod cli;
mod sink;

use std::io;
use std::time::{Duration, Instant};

use ausha_client::{Client, Config, Stats, loss_ratio};

use sink::Sink;

fn main() {
    let cli = match cli::parse() {
        Ok(cli) => cli,
        Err(e) => {
            eprintln!("error: {e}\n\n{}", cli::USAGE);
            std::process::exit(2);
        }
    };
    if let Err(e) = run(cli) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run(cli: cli::Cli) -> io::Result<()> {
    let mut client = Client::connect(&Config {
        host: cli.host,
        control_port: cli.control_port,
        token: cli.token,
        name: cli.name,
        simulate_loss: cli.simulate_loss,
        latency: cli.latency,
        uplink: cli.uplink_tone,
    })?;

    let params = client.params().clone();
    println!(
        "connected: {} {} Hz, {} ch, {} ms frames, ssrc {:08x}",
        params.codec, params.rate, params.channels, params.ptime_ms, params.ssrc
    );

    let mut sink: Box<dyn Sink> = match cli.sink.as_deref() {
        Some("null") => Box::new(sink::Null::default()),
        Some(program) => Box::new(sink::Process::open(program, cli.sink_latency_ms)?),
        None => Box::new(sink::Process::detect(cli.sink_latency_ms)?),
    };

    let mut tone = match client.uplink() {
        Some(uplink) => {
            println!(
                "uplink: sending a 440 Hz tone, {} samples a frame",
                uplink.frame_len()
            );
            Some(Tone::new(uplink.frame_len(), params.rate))
        }
        None if cli.uplink_tone => {
            eprintln!("uplink: the sender did not offer one; is it running with --uplink?");
            None
        }
        None => None,
    };

    let chunk_frames = (params.rate / 1000 * params.ptime_ms) as usize;
    let mut chunk = vec![0.0f32; chunk_frames * usize::from(params.channels)];

    let started = Instant::now();
    let mut last_report = Instant::now();
    let deadline = cli.run_for_secs.map(Duration::from_secs);

    while client.is_running() {
        client.fill(&mut chunk);
        sink.write(&chunk)?;

        // The sink has just paced this iteration to one frame of real time,
        // and an uplink frame is the same 20 ms, so it paces the tone too.
        if let Some(tone) = &mut tone
            && let Some(uplink) = client.uplink()
        {
            uplink.send(tone.next_frame())?;
        }

        if last_report.elapsed() >= Duration::from_secs(5) {
            report(&client.stats(), started.elapsed());
            last_report = Instant::now();
        }
        if deadline.is_some_and(|limit| started.elapsed() >= limit) {
            break;
        }
    }

    let stats = client.stats();
    drop(client);
    summarise(&stats, started.elapsed());
    Ok(())
}

/// A 440 Hz sine, so that what comes out of the sender's virtual microphone is
/// recognisable by ear and by a spectrum plot.
struct Tone {
    samples: Vec<f32>,
    rate: u32,
    phase: u32,
}

impl Tone {
    const HZ: f32 = 440.0;

    fn new(frame_len: usize, rate: u32) -> Self {
        Self {
            samples: vec![0.0; frame_len],
            rate,
            phase: 0,
        }
    }

    fn next_frame(&mut self) -> &[f32] {
        for sample in self.samples.iter_mut() {
            let t = self.phase as f32 / self.rate as f32;
            *sample = (t * Self::HZ * std::f32::consts::TAU).sin() * 0.5;
            // Wrapping on a whole second keeps the phase continuous.
            self.phase = (self.phase + 1) % self.rate;
        }
        &self.samples
    }
}

fn report(stats: &Stats, elapsed: Duration) {
    let jitter = stats.jitter;
    println!(
        "{:>5}s  depth {:>3}/{:>3} ms  latency {:>3.0} ms  jitter {:>5.1} ms  loss {:>5.2}%  \
         fec {}  plc {}  underruns {}  rate {:.4}",
        elapsed.as_secs(),
        jitter.depth_ms,
        jitter.target_ms,
        stats.buffered_ms,
        jitter.jitter_ms,
        loss_ratio(&jitter) * 100.0,
        jitter.recovered,
        jitter.concealed,
        jitter.underruns,
        stats.ratio,
    );
}

fn summarise(stats: &Stats, elapsed: Duration) {
    let jitter = stats.jitter;
    println!(
        "\nplayed {:.1}s: {} packets, {} lost ({} recovered by FEC, {} concealed), \
         {} reordered, {} late, {} duplicate, {} shed, {} underruns, {} silent frames",
        elapsed.as_secs_f64(),
        jitter.received,
        jitter.lost,
        jitter.recovered,
        jitter.concealed,
        jitter.reordered,
        jitter.late,
        jitter.duplicates,
        jitter.dropped,
        jitter.underruns,
        stats.silence_frames,
    );
}
