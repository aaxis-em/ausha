//! Media path: learns receiver addresses from UDP punch packets and forwards
//! each RTP packet produced by ffmpeg to every connected receiver.

use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;

use crate::registry::Registry;
use ausha_core::config;

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
pub fn forward(ingest: &UdpSocket, media: &UdpSocket, registry: &Registry) -> io::Result<()> {
    let mut buf = [0u8; config::MAX_DATAGRAM];
    if let Some(packet) = receive(ingest, &mut buf)? {
        registry.for_each_target(|target| {
            let _ = media.send_to(packet, target);
        });
    }
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

fn is_timeout(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}
