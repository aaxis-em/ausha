//! Audio output.
//!
//! The sink is a child process reading raw float samples on stdin, which keeps
//! the receiver free of native audio build dependencies. Writes block once the
//! player's buffer is full, which is what paces the pipeline in real time.
//!
//! The mobile apps will replace this with a real audio callback; everything
//! upstream of here is already independent of it.

use std::io::{self, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use ausha_client::config;

pub trait Sink {
    fn write(&mut self, samples: &[f32]) -> io::Result<()>;
}

pub struct Process {
    child: Child,
    stdin: ChildStdin,
    bytes: Vec<u8>,
}

impl Process {
    pub fn open(program: &str, latency_ms: u32) -> io::Result<Self> {
        let mut command = Command::new(program);
        match program {
            "pacat" => command.args([
                "--raw",
                "--format=float32le",
                &format!("--rate={}", config::SAMPLE_RATE),
                &format!("--channels={}", config::CHANNELS),
                &format!("--latency-msec={latency_ms}"),
                "--stream-name=Ausha",
            ]),
            "aplay" => command.args([
                "-q",
                "-f",
                "FLOAT_LE",
                "-r",
                &config::SAMPLE_RATE.to_string(),
                "-c",
                &config::CHANNELS.to_string(),
                "-t",
                "raw",
            ]),
            _ => command.args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-nodisp",
                "-autoexit",
                "-f",
                "f32le",
                "-ar",
                &config::SAMPLE_RATE.to_string(),
                "-ac",
                &config::CHANNELS.to_string(),
                "-i",
                "-",
            ]),
        };

        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()?;
        let stdin = child.stdin.take().expect("stdin was piped");
        Ok(Self {
            child,
            stdin,
            bytes: Vec::new(),
        })
    }

    pub fn detect(latency_ms: u32) -> io::Result<Self> {
        for program in ["pacat", "aplay", "ffplay"] {
            if let Ok(sink) = Self::open(program, latency_ms) {
                println!("sink: {program}");
                return Ok(sink);
            }
        }
        Err(io::Error::other("no pacat, aplay or ffplay on PATH"))
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Sink for Process {
    fn write(&mut self, samples: &[f32]) -> io::Result<()> {
        self.bytes.clear();
        self.bytes.reserve(samples.len() * 4);
        for sample in samples {
            self.bytes.extend_from_slice(&sample.to_le_bytes());
        }
        self.stdin.write_all(&self.bytes)
    }
}

/// Discards audio but consumes it at real time, for soak testing without a
/// sound card.
pub struct Null {
    start: Instant,
    written: u64,
}

impl Default for Null {
    fn default() -> Self {
        Self {
            start: Instant::now(),
            written: 0,
        }
    }
}

impl Sink for Null {
    fn write(&mut self, samples: &[f32]) -> io::Result<()> {
        self.written += (samples.len() / usize::from(config::CHANNELS)) as u64;
        let due = Duration::from_micros(self.written * 1_000_000 / u64::from(config::SAMPLE_RATE));
        if let Some(wait) = due.checked_sub(self.start.elapsed()) {
            thread::sleep(wait);
        }
        Ok(())
    }
}
