//! Media path: learns receiver addresses from UDP punch packets and forwards
//! each RTP packet produced by ffmpeg to every connected receiver.

use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;

use crate::registry::Registry;
use ausha_core::config;
use ausha_core::crypto::Sealer;

pub fn listen_for_punch(socket: Arc<UdpSocket>, registry: Arc<Registry>) {
    let mut buf = [0u8; config::MAX_DATAGRAM];
    loop {
        let (n, from) = match socket.recv_from(&mut buf) {
            Ok(result) => result,
            Err(e) => {
                eprintln!("punch: recv failed: {e}");
                continue;
            }
        };
        match parse_punch(&buf[..n]) {
            Some(id) if registry.attach_media(id, from) => {
                println!("punch: session {id:016x} reachable at {from}")
            }
            Some(id) => eprintln!("punch: unknown session {id:016x} from {from}"),
            None => eprintln!("punch: malformed packet from {from}"),
        }
    }
}

fn parse_punch(datagram: &[u8]) -> Option<u64> {
    let text = std::str::from_utf8(datagram).ok()?;
    let id = text.trim().strip_prefix(config::PUNCH_PREFIX)?;
    u64::from_str_radix(id, 16).ok()
}

/// Forwards one RTP packet per datagram. ffmpeg emits exactly one Opus frame
/// per packet, so a datagram lost in flight costs the receiver one frame.
///
/// Sealing happens once per packet rather than once per receiver: every
/// receiver of this run shares the session key.
pub fn forward(
    ingest: &UdpSocket,
    media: &UdpSocket,
    registry: &Registry,
    sealer: Option<&mut Sealer>,
) -> io::Result<()> {
    let mut buf = [0u8; config::MAX_DATAGRAM];
    let Some(packet) = receive(ingest, &mut buf)? else {
        return Ok(());
    };
    let sealed = match sealer {
        Some(sealer) => match sealer.seal(packet) {
            Some(sealed) => Some(sealed),
            // Too short to carry an RTP header, so not something we produced.
            // Passing it on in the clear would leak past the flag.
            None => return Ok(()),
        },
        None => None,
    };
    let outgoing = sealed.as_deref().unwrap_or(packet);
    registry.for_each_target(|target| {
        let _ = media.send_to(outgoing, target);
    });
    Ok(())
}

/// Fan-out for the MPEG-TS compatibility stream, whose targets are fixed at
/// startup and never take part in the handshake.
pub fn forward_compat(ingest: UdpSocket, sender: UdpSocket, targets: Vec<SocketAddr>) {
    let mut buf = [0u8; config::MAX_DATAGRAM];
    loop {
        match receive(&ingest, &mut buf) {
            Ok(Some(packet)) => {
                for target in &targets {
                    let _ = sender.send_to(packet, target);
                }
            }
            Ok(None) => {}
            Err(e) => {
                eprintln!("compat: ingest failed: {e}");
                return;
            }
        }
    }
}

fn receive<'a>(ingest: &UdpSocket, buf: &'a mut [u8]) -> io::Result<Option<&'a [u8]>> {
    let (n, from) = match ingest.recv_from(buf) {
        Ok(result) => result,
        Err(e) if is_timeout(&e) => return Ok(None),
        Err(e) => return Err(e),
    };
    Ok(from.ip().is_loopback().then_some(&buf[..n]))
}

/// `Interrupted` belongs here rather than being an error: it is what a blocked
/// `recv_from` returns when a signal arrives, and the caller's next loop is
/// where the shutdown that signal asked for gets noticed.
fn is_timeout(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted
    )
}
