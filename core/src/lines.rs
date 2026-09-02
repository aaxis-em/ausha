//! Reads newline-delimited messages from a socket whose read timeout fires
//! regularly, without losing bytes when a timeout lands mid-message.

use std::io::{self, Read};

pub enum Incoming {
    Line(String),
    Idle,
    Closed,
}

pub struct LineReader<R> {
    source: R,
    pending: Vec<u8>,
    chunk: [u8; 1024],
}

impl<R: Read> LineReader<R> {
    pub fn new(source: R) -> Self {
        Self {
            source,
            pending: Vec::new(),
            chunk: [0u8; 1024],
        }
    }

    pub fn read(&mut self) -> io::Result<Incoming> {
        loop {
            if let Some(line) = self.take_line() {
                return Ok(Incoming::Line(line));
            }
            match self.source.read(&mut self.chunk) {
                Ok(0) => return Ok(Incoming::Closed),
                Ok(n) => self.pending.extend_from_slice(&self.chunk[..n]),
                Err(e) if is_timeout(&e) => return Ok(Incoming::Idle),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
    }

    fn take_line(&mut self) -> Option<String> {
        let end = self.pending.iter().position(|b| *b == b'\n')?;
        let line = self.pending.drain(..=end).collect::<Vec<_>>();
        Some(String::from_utf8_lossy(&line[..end]).trim().to_string())
    }
}

fn is_timeout(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}
