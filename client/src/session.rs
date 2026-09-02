//! Client side of the TCP control channel: handshake, UDP punch, keepalive.

use std::io::{self, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream, UdpSocket};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ausha_core::config;
use ausha_core::lines::{Incoming, LineReader};
use ausha_core::protocol::{ClientMessage, ServerMessage, StreamParams};

/// Short enough that shutdown is prompt, since a blocked read is what the
/// control thread spends its life in.
const POLL_TIMEOUT: Duration = Duration::from_millis(500);

pub struct Session {
    reader: LineReader<TcpStream>,
    stream: TcpStream,
    pub params: StreamParams,
    pub media: UdpSocket,
    pub offset_us: Option<i64>,
}

/// Runs the handshake and returns a session already receiving media.
pub fn connect(host: &str, port: u16, token: &str, name: &str) -> io::Result<Session> {
    let control = TcpStream::connect((host, port))?;
    control.set_nodelay(true)?;
    control.set_read_timeout(Some(POLL_TIMEOUT))?;
    let mut reader = LineReader::new(control.try_clone()?);
    let mut stream = control;

    send(
        &mut stream,
        &ClientMessage::Hello {
            ver: config::PROTOCOL_VERSION,
            name: name.to_string(),
            token: token.to_string(),
        },
    )?;

    let (session, media_port, params) = match expect(&mut reader)? {
        ServerMessage::Accept {
            session,
            media_port,
            stream,
        } => (session, media_port, stream),
        ServerMessage::Error { reason } => {
            return Err(io::Error::other(format!("rejected: {reason}")));
        }
        other => return Err(io::Error::other(format!("expected accept, got {other:?}"))),
    };

    let media = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    media.set_read_timeout(Some(Duration::from_millis(200)))?;
    let server: SocketAddr = format!("{host}:{media_port}")
        .parse()
        .map_err(io::Error::other)?;
    media.send_to(
        format!("{}{session}", config::PUNCH_PREFIX).as_bytes(),
        server,
    )?;

    match expect(&mut reader)? {
        ServerMessage::Ready => {}
        other => return Err(io::Error::other(format!("expected ready, got {other:?}"))),
    }

    Ok(Session {
        reader,
        stream,
        params,
        media,
        offset_us: None,
    })
}

impl Session {
    /// Handles one control message, or returns after the read timeout. Returns
    /// false once the server closes the connection.
    pub fn poll(&mut self) -> io::Result<bool> {
        match self.reader.read()? {
            Incoming::Line(line) if !line.is_empty() => {
                let received_us = now_us();
                if let Ok(ServerMessage::Ping { ts }) = serde_json::from_str(&line) {
                    self.observe_offset(ts, received_us);
                    send(&mut self.stream, &ClientMessage::Pong { ts })?;
                }
                Ok(true)
            }
            Incoming::Line(_) | Incoming::Idle => Ok(true),
            Incoming::Closed => Ok(false),
        }
    }

    pub fn report(&mut self, loss: f32, jitter_ms: u32, buffer_ms: u32) -> io::Result<()> {
        send(
            &mut self.stream,
            &ClientMessage::Stats {
                loss,
                jitter_ms,
                buffer_ms,
            },
        )
    }

    pub fn say_goodbye(&mut self) {
        let _ = send(&mut self.stream, &ClientMessage::Bye);
    }

    /// One-way offset assuming a symmetric path. Not used for playback yet;
    /// it is what A/V sync with the desktop will need.
    fn observe_offset(&mut self, sent_us: u64, received_us: u64) {
        self.offset_us = Some(received_us as i64 - sent_us as i64);
    }
}

fn expect(reader: &mut LineReader<TcpStream>) -> io::Result<ServerMessage> {
    let deadline = Instant::now() + config::PUNCH_TIMEOUT;
    while Instant::now() < deadline {
        match reader.read()? {
            Incoming::Line(line) if line.is_empty() => continue,
            Incoming::Line(line) => {
                return serde_json::from_str(&line)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e));
            }
            Incoming::Idle => continue,
            Incoming::Closed => return Err(io::Error::other("server closed the connection")),
        }
    }
    Err(io::Error::new(io::ErrorKind::TimedOut, "no reply"))
}

fn send(stream: &mut TcpStream, message: &ClientMessage) -> io::Result<()> {
    let mut line = serde_json::to_vec(message)?;
    line.push(b'\n');
    stream.write_all(&line)
}

fn now_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_micros() as u64
}
