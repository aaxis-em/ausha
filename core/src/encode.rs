//! Opus encoding for the uplink, the mirror of `decode`.
//!
//! The downlink is encoded by ffmpeg on the sender; this exists for the other
//! direction, where the phone has the microphone and no ffmpeg to call.

pub use crate::decode::Error;

use crate::config;

pub struct Encoder {
    inner: opus::Encoder,
    channels: usize,
    frame_samples: usize,
}

impl Encoder {
    /// Configured for speech: Opus's voip mode is markedly more intelligible
    /// than `audio` at the bitrate a microphone deserves, which is a fraction
    /// of what music needs.
    pub fn voice(
        sample_rate: u32,
        channels: u8,
        frame_ms: u32,
        bitrate: u32,
    ) -> Result<Self, Error> {
        let layout = match channels {
            1 => opus::Channels::Mono,
            _ => opus::Channels::Stereo,
        };
        let mut inner = opus::Encoder::new(sample_rate, layout, opus::Application::Voip)
            .map_err(Error::from)?;
        inner
            .set_bitrate(opus::Bitrate::Bits(bitrate as i32))
            .map_err(Error::from)?;
        // Same bargain as the downlink: the redundant copy costs bitrate and
        // buys back an isolated loss, which is the common case on WiFi.
        inner.set_inband_fec(true).map_err(Error::from)?;
        inner
            .set_packet_loss_perc(config::EXPECTED_LOSS_PERCENT as i32)
            .map_err(Error::from)?;

        Ok(Self {
            inner,
            channels: usize::from(channels),
            frame_samples: (sample_rate / 1000 * frame_ms) as usize,
        })
    }

    /// Interleaved samples one frame occupies. The caller must hand `encode`
    /// exactly this many.
    pub fn frame_len(&self) -> usize {
        self.frame_samples * self.channels
    }

    pub fn encode(&mut self, frame: &[f32], out: &mut Vec<u8>) -> Result<(), Error> {
        out.resize(MAX_PACKET, 0);
        let n = self.inner.encode_float(frame, out).map_err(Error::from)?;
        out.truncate(n);
        Ok(())
    }
}

/// Comfortably above what one 20 ms voice frame reaches, and far under the MTU.
const MAX_PACKET: usize = 1024;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::Decoder;
    use std::f32::consts::TAU;

    fn speechlike(frame: usize, samples: usize) -> Vec<f32> {
        (0..samples)
            .map(|i| {
                let t = (frame * samples + i) as f32 / config::SAMPLE_RATE as f32;
                ((TAU * 220.0 * t).sin() * 0.4 + (TAU * 660.0 * t).sin() * 0.2) * 0.8
            })
            .collect()
    }

    fn energy(samples: &[f32]) -> f32 {
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
    }

    fn encoder() -> Encoder {
        Encoder::voice(
            config::SAMPLE_RATE,
            config::UPLINK_CHANNELS,
            config::FRAME_MS,
            config::UPLINK_BITRATE,
        )
        .unwrap()
    }

    #[test]
    fn a_frame_survives_the_round_trip() {
        let mut encoder = encoder();
        let mut decoder = Decoder::new(
            config::SAMPLE_RATE,
            config::UPLINK_CHANNELS,
            config::FRAME_MS,
        )
        .unwrap();
        let mut packet = Vec::new();
        let mut out = vec![0.0; decoder.frame_len()];

        for frame in 0..10 {
            encoder
                .encode(&speechlike(frame, encoder.frame_len()), &mut packet)
                .unwrap();
            decoder.decode(&packet, &mut out).unwrap();
        }
        assert!(energy(&out) > 0.1, "decoded near-silence: {}", energy(&out));
    }

    #[test]
    fn stays_far_under_the_mtu_at_the_uplink_bitrate() {
        let mut encoder = encoder();
        let mut packet = Vec::new();
        for frame in 0..50 {
            encoder
                .encode(&speechlike(frame, encoder.frame_len()), &mut packet)
                .unwrap();
            assert!(
                packet.len() < 200,
                "frame {frame} was {} bytes",
                packet.len()
            );
        }
    }

    #[test]
    fn mono_frames_are_half_the_stereo_length() {
        assert_eq!(encoder().frame_len(), 960);
    }
}
