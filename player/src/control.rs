//! TCP control server: handshake, UDP address discovery, and keepalive.

use std::io::{self, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::ids;
use crate::registry::{Registry, SessionId};
use ausha_core::config;
use ausha_core::lines::{Incoming, LineReader};
use ausha_core::protocol::{self, ClientMessage, ServerMessage, StreamParams};

pub struct ControlServer {
    pub registry: Arc<Registry>,
    pub token: String,
    pub media_port: u16,
    pub stream: StreamParams,
}

pub fn serve(listener: TcpListener, server: Arc<ControlServer>) {
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let server = server.clone();
                thread::spawn(move || {
                    let peer = stream.peer_addr().ok();
                    match handle(stream, &server) {
                        Err(e) if !is_disconnect(&e) => {
                            eprintln!("control: session with {peer:?} failed: {e}")
                        }
                        _ => {}
                    }
                });
            }
            Err(e) => eprintln!("control: accept failed: {e}"),
        }
    }
}

fn handle(stream: TcpStream, server: &ControlServer) -> io::Result<()> {
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(config::PING_INTERVAL))?;
    let mut reader = LineReader::new(stream.try_clone()?);
    let mut writer = stream;
    let peer = writer.peer_addr()?;

    let hello = match read_hello(&mut reader)? {
        Some(hello) => hello,
        None => return Ok(()),
    };

    let ClientMessage::Hello { ver, name, token } = hello else {
        return reject(&mut writer, "expected hello");
    };
    if ver != config::PROTOCOL_VERSION {
        return reject(&mut writer, "unsupported protocol version");
    }
    if !ids::tokens_match(
        &protocol::normalize_token(&token),
        &protocol::normalize_token(&server.token),
    ) {
        eprintln!(
            "control: rejected {peer} (bad pairing token; this session's is {})",
            ids::format_token(&server.token)
        );
        return reject(&mut writer, "invalid pairing token");
    }

    let (id, punch) = server.registry.open(name.clone());
    let result = run_session(&mut reader, &mut writer, server, id, punch, &name);
    server.registry.close(id);
    println!("control: {name} ({peer}) disconnected");
    result
}

fn read_hello(reader: &mut LineReader<TcpStream>) -> io::Result<Option<ClientMessage>> {
    let deadline = Instant::now() + config::SESSION_TIMEOUT;
    while Instant::now() < deadline {
        match reader.read()? {
            Incoming::Line(line) if line.is_empty() => continue,
            Incoming::Line(line) => return Ok(Some(decode(&line)?)),
            Incoming::Idle => continue,
            Incoming::Closed => return Ok(None),
        }
    }
    Err(io::Error::new(io::ErrorKind::TimedOut, "no hello"))
}

fn run_session(
    reader: &mut LineReader<TcpStream>,
    writer: &mut TcpStream,
    server: &ControlServer,
    id: SessionId,
    punch: Receiver<SocketAddr>,
    name: &str,
) -> io::Result<()> {
    send(
        writer,
        &ServerMessage::Accept {
            session: format!("{id:016x}"),
            media_port: server.media_port,
            stream: server.stream.clone(),
        },
    )?;

    let media = punch
        .recv_timeout(config::PUNCH_TIMEOUT)
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "no UDP punch received"))?;
    send(writer, &ServerMessage::Ready)?;
    println!("control: {name} ready, media to {media}");

    let mut last_seen = Instant::now();
    let mut last_ping = Instant::now();
    loop {
        match reader.read()? {
            Incoming::Line(line) if line.is_empty() => {}
            Incoming::Line(line) => {
                last_seen = Instant::now();
                if matches!(decode(&line)?, ClientMessage::Bye) {
                    return Ok(());
                }
            }
            Incoming::Idle => {}
            Incoming::Closed => return Ok(()),
        }

        if last_seen.elapsed() > config::SESSION_TIMEOUT {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "keepalive lost"));
        }
        if last_ping.elapsed() >= config::PING_INTERVAL {
            send(writer, &ServerMessage::Ping { ts: now_micros() })?;
            last_ping = Instant::now();
        }
    }
}

fn decode(line: &str) -> io::Result<ClientMessage> {
    serde_json::from_str(line).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

fn send(writer: &mut TcpStream, message: &ServerMessage) -> io::Result<()> {
    let mut line = serde_json::to_vec(message)?;
    line.push(b'\n');
    writer.write_all(&line)
}

fn reject(writer: &mut TcpStream, reason: &str) -> io::Result<()> {
    send(
        writer,
        &ServerMessage::Error {
            reason: reason.to_string(),
        },
    )
}

fn is_disconnect(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::ConnectionReset | io::ErrorKind::BrokenPipe | io::ErrorKind::UnexpectedEof
    )
}

fn now_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_micros() as u64
}
