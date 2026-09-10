//! The microphone channel back to the sender.
//!
//! Encodes, packetises and sends one frame at a time. It is driven by whatever
//! owns the capture device — `AudioRecord` on the phone — so it has no thread
//! and no clock of its own: the device paces it.

use std::io;
use std::net::{SocketAddr, UdpSocket};

use ausha_core::crypto::{Direction, Sealer, Secret};
use ausha_core::encode::Encoder;
use ausha_core::protocol::UplinkParams;
use ausha_core::rtp;

pub struct Uplink {
    socket: UdpSocket,
    target: SocketAddr,
    encoder: Encoder,
    builder: rtp::Builder,
    sealer: Option<Sealer>,
    frame_samples: u32,
    payload: Vec<u8>,
    packet: Vec<u8>,
}

impl Uplink {
    /// Sends from the socket the downlink already punched with, which keeps one
    /// NAT mapping open instead of two and means the sender sees this traffic
    /// arriving from an address it has already seen.
    pub fn new(
        socket: UdpSocket,
        host: &str,
        params: &UplinkParams,
        secret: Option<&Secret>,
    ) -> io::Result<Self> {
        let stream = &params.stream;
        let encoder = Encoder::voice(
            stream.rate,
            stream.channels,
            stream.ptime_ms,
            params.bitrate,
        )
        .map_err(io::Error::other)?;

        Ok(Self {
            socket,
            target: format!("{host}:{}", params.port)
                .parse()
                .map_err(io::Error::other)?,
            encoder,
            builder: rtp::Builder::new(stream.payload_type, stream.ssrc),
            sealer: secret.map(|secret| Sealer::new(&secret.key(Direction::Uplink))),
            frame_samples: stream.rate / 1000 * stream.ptime_ms,
            payload: Vec::new(),
            packet: Vec::new(),
        })
    }

    /// Interleaved samples one frame needs. The caller must hand `send`
    /// exactly this many.
    pub fn frame_len(&self) -> usize {
        self.encoder.frame_len()
    }

    /// Encodes and sends one frame. A datagram that cannot be sent is dropped
    /// rather than retried: by the time a retry landed the frame would be late,
    /// and the sender's jitter buffer conceals a gap better than a stall.
    pub fn send(&mut self, frame: &[f32]) -> io::Result<()> {
        self.encoder
            .encode(frame, &mut self.payload)
            .map_err(io::Error::other)?;
        self.builder
            .build(&self.payload, self.frame_samples, &mut self.packet);

        match &mut self.sealer {
            Some(sealer) => {
                if let Some(sealed) = sealer.seal(&self.packet) {
                    let _ = self.socket.send_to(&sealed, self.target);
                }
            }
            None => {
                let _ = self.socket.send_to(&self.packet, self.target);
            }
        }
        Ok(())
    }
}
