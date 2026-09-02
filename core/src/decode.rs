//! Opus decoding, including the two ways a lost frame can be filled in.

use crate::config;

pub struct Decoder {
    inner: opus::Decoder,
    channels: usize,
    frame_samples: usize,
}

#[derive(Debug)]
pub struct Error(opus::Error);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "opus: {}", self.0)
    }
}

impl std::error::Error for Error {}

impl Decoder {
    pub fn new(sample_rate: u32, channels: u8, frame_ms: u32) -> Result<Self, Error> {
        let layout = match channels {
            1 => opus::Channels::Mono,
            _ => opus::Channels::Stereo,
        };
        Ok(Self {
            inner: opus::Decoder::new(sample_rate, layout).map_err(Error)?,
            channels: usize::from(channels),
            frame_samples: (sample_rate / 1000 * frame_ms) as usize,
        })
    }

    /// Interleaved samples one decoded frame occupies.
    pub fn frame_len(&self) -> usize {
        self.frame_samples * self.channels
    }

    pub fn decode(&mut self, payload: &[u8], out: &mut [f32]) -> Result<usize, Error> {
        self.inner.decode_float(payload, out, false).map_err(Error)
    }

    /// Reconstructs the *previous* frame from the redundant copy Opus embeds in
    /// `next_payload`. This is why the jitter buffer holds one frame of
    /// lookahead: the recovery data arrives after the frame it repairs.
    pub fn recover(&mut self, next_payload: &[u8], out: &mut [f32]) -> Result<usize, Error> {
        self.inner
            .decode_float(next_payload, out, true)
            .map_err(Error)
    }

    /// Packet loss concealment: Opus extrapolates from its own decoder state.
    pub fn conceal(&mut self, out: &mut [f32]) -> Result<usize, Error> {
        self.inner.decode_float(&[], out, false).map_err(Error)
    }
}

pub fn default_decoder() -> Result<Decoder, Error> {
    Decoder::new(config::SAMPLE_RATE, config::CHANNELS, config::FRAME_MS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    fn encoder(loss_percent: i32) -> opus::Encoder {
        let mut encoder = opus::Encoder::new(
            config::SAMPLE_RATE,
            opus::Channels::Stereo,
            opus::Application::Audio,
        )
        .unwrap();
        encoder.set_bitrate(opus::Bitrate::Bits(128_000)).unwrap();
        if loss_percent > 0 {
            encoder.set_inband_fec(true).unwrap();
            encoder.set_packet_loss_perc(loss_percent).unwrap();
        }
        encoder
    }

    fn tone(frame: usize) -> Vec<f32> {
        let samples = (config::SAMPLE_RATE / 1000 * config::FRAME_MS) as usize;
        (0..samples)
            .flat_map(|i| {
                let t = (frame * samples + i) as f32 / config::SAMPLE_RATE as f32;
                let value = (TAU * 440.0 * t).sin() * 0.5;
                [value, value]
            })
            .collect()
    }

    fn energy(samples: &[f32]) -> f32 {
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
    }

    #[test]
    fn decodes_a_frame_it_encoded() {
        let mut encoder = encoder(0);
        let mut decoder = default_decoder().unwrap();
        let mut out = vec![0.0; decoder.frame_len()];

        let mut last = 0;
        for frame in 0..10 {
            let packet = encoder.encode_vec_float(&tone(frame), 4000).unwrap();
            last = decoder.decode(&packet, &mut out).unwrap();
        }
        assert_eq!(last * 2, decoder.frame_len());
        assert!(
            energy(&out) > 0.2,
            "decoded a silent frame: {}",
            energy(&out)
        );
    }

    #[test]
    fn fec_reconstructs_a_dropped_frame_from_the_next_packet() {
        let mut encoder = encoder(10);
        let mut decoder = default_decoder().unwrap();
        let packets: Vec<Vec<u8>> = (0..10)
            .map(|f| encoder.encode_vec_float(&tone(f), 4000).unwrap())
            .collect();

        let mut out = vec![0.0; decoder.frame_len()];
        for packet in &packets[..5] {
            decoder.decode(packet, &mut out).unwrap();
        }
        // Frame 5 never arrives; rebuild it from the copy inside packet 6.
        let mut recovered = vec![0.0; decoder.frame_len()];
        decoder.recover(&packets[6], &mut recovered).unwrap();

        assert!(
            energy(&recovered) > 0.1,
            "FEC produced near-silence: {}",
            energy(&recovered)
        );
    }

    #[test]
    fn concealment_produces_audio_rather_than_silence_or_a_click() {
        let mut encoder = encoder(0);
        let mut decoder = default_decoder().unwrap();
        let mut out = vec![0.0; decoder.frame_len()];
        for frame in 0..5 {
            let packet = encoder.encode_vec_float(&tone(frame), 4000).unwrap();
            decoder.decode(&packet, &mut out).unwrap();
        }
        let before = out[out.len() - 2];

        let mut concealed = vec![0.0; decoder.frame_len()];
        decoder.conceal(&mut concealed).unwrap();

        assert!(energy(&concealed) > 0.05, "PLC returned silence");
        assert!(
            (concealed[0] - before).abs() < 0.5,
            "PLC should continue the waveform, not jump ({before} -> {})",
            concealed[0]
        );
    }
}
