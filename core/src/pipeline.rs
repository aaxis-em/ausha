//! The receive pipeline: datagrams in, playable audio out.
//!
//! Owns the jitter buffer, decoder, drift controller and resampler, and knows
//! nothing about sockets or audio devices. A caller hands it datagrams as they
//! arrive and asks it to fill an output buffer when the device wants samples.

use std::collections::VecDeque;

use crate::config;
use crate::decode::{self, Decoder};
use crate::drift::DriftController;
use crate::jitter::{JitterBuffer, Step};
use crate::protocol::StreamParams;
use crate::resample::Resampler;
use crate::rtp;

#[derive(Debug, Default, Clone, Copy)]
pub struct Report {
    pub filled: usize,
    pub silence: usize,
}

#[derive(Debug, Default, Clone, Copy, serde::Serialize)]
pub struct Stats {
    pub jitter: crate::jitter::Stats,
    pub buffered_ms: f64,
    pub ratio: f64,
    pub silence_frames: u64,
    pub rejected: u64,
}

pub struct Pipeline {
    jitter: JitterBuffer,
    decoder: Decoder,
    drift: DriftController,
    resampler: Resampler,
    frame: Vec<f32>,
    resampled: Vec<f32>,
    ready: VecDeque<f32>,
    channels: usize,
    sample_rate: u32,
    payload_type: u8,
    ssrc: Option<u32>,
    silence_frames: u64,
    rejected: u64,
    started: bool,
}

/// How much depth the buffer aims to hold. Deeper rides out worse networks at
/// the cost of delay, which is a judgement only the listener can make: someone
/// watching video wants Low, someone listening in another room wants Stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Latency {
    Low,
    Balanced,
    Stable,
}

impl Latency {
    pub fn range_ms(self) -> (u32, u32) {
        match self {
            Latency::Low => (40, 120),
            Latency::Balanced => (config::JITTER_MIN_MS, config::JITTER_MAX_MS),
            Latency::Stable => (160, 400),
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "low" => Some(Latency::Low),
            "balanced" => Some(Latency::Balanced),
            "stable" => Some(Latency::Stable),
            _ => None,
        }
    }
}

impl Pipeline {
    pub fn new(params: &StreamParams) -> Result<Self, decode::Error> {
        Self::with_latency(params, Latency::Balanced)
    }

    pub fn with_latency(params: &StreamParams, latency: Latency) -> Result<Self, decode::Error> {
        let decoder = Decoder::new(params.rate, params.channels, params.ptime_ms)?;
        let channels = usize::from(params.channels);
        let frame_len = decoder.frame_len();
        Ok(Self {
            jitter: {
                let (min_ms, max_ms) = latency.range_ms();
                JitterBuffer::new(params.ptime_ms, params.rate, min_ms, max_ms)
            },
            decoder,
            drift: DriftController::default(),
            resampler: Resampler::new(channels),
            frame: vec![0.0; frame_len],
            resampled: Vec::with_capacity(frame_len * 2),
            ready: VecDeque::with_capacity(frame_len * 16),
            channels,
            sample_rate: params.rate,
            payload_type: params.payload_type,
            ssrc: None,
            silence_frames: 0,
            rejected: 0,
            started: false,
        })
    }

    pub fn stats(&self) -> Stats {
        Stats {
            jitter: self.jitter.stats(),
            buffered_ms: self.buffered_ms(),
            ratio: self.drift.ratio(),
            silence_frames: self.silence_frames,
            rejected: self.rejected,
        }
    }

    pub fn on_datagram(&mut self, datagram: &[u8], arrival_us: u64) {
        let Ok(packet) = rtp::parse(datagram) else {
            self.rejected += 1;
            return;
        };
        if packet.payload_type != self.payload_type {
            self.rejected += 1;
            return;
        }
        match self.ssrc {
            Some(ssrc) if ssrc != packet.ssrc => {
                self.rejected += 1;
                return;
            }
            Some(_) => {}
            None => self.ssrc = Some(packet.ssrc),
        }
        self.jitter.insert(&packet, arrival_us);
    }

    /// Fills `output` with interleaved samples, padding with silence if the
    /// buffer cannot keep up. Never blocks and never allocates once the
    /// scratch buffers have grown.
    pub fn fill(&mut self, output: &mut [f32], now_us: u64) -> Report {
        let ratio = self.drift.ratio();

        while self.ready.len() < output.len() {
            let decoded = match self.jitter.pop() {
                Step::Decode(payload) => self.decoder.decode(&payload, &mut self.frame),
                Step::Recover(next) => self.decoder.recover(&next, &mut self.frame),
                Step::Conceal => self.decoder.conceal(&mut self.frame),
                Step::Starve => break,
            };
            let samples = match decoded {
                Ok(samples) => samples * self.channels,
                Err(_) => {
                    self.frame.fill(0.0);
                    self.frame.len()
                }
            };
            self.resampled.clear();
            self.resampler
                .process(&self.frame[..samples], ratio, &mut self.resampled);
            self.ready.extend(self.resampled.iter().copied());
        }

        let filled = self.ready.len().min(output.len());
        for slot in output[..filled].iter_mut() {
            *slot = self.ready.pop_front().unwrap_or(0.0);
        }
        output[filled..].fill(0.0);

        // Regulated after the pops, on undecoded depth. Two deliberate
        // choices: samples already decoded into `ready` are on their way to
        // the device and cannot absorb a loss burst, and the depth left once
        // this fill has taken its frames is the one a burst has to survive.
        self.drift.update(
            f64::from(self.jitter.depth_ms()),
            f64::from(self.jitter.target_ms()),
            now_us,
        );

        let silence = output.len() - filled;
        // Silence before the first frame is the buffer filling, not a dropout.
        if self.started {
            self.silence_frames += (silence / self.channels) as u64;
        }
        self.started |= filled > 0;
        Report { filled, silence }
    }

    fn buffered_ms(&self) -> f64 {
        let ready_frames = (self.ready.len() / self.channels) as f64;
        let ready_ms = ready_frames * 1000.0 / f64::from(self.sample_rate);
        f64::from(self.jitter.depth_ms()) + ready_ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    const FRAME_SAMPLES: usize = (config::SAMPLE_RATE / 1000 * config::FRAME_MS) as usize;
    const TICKS: u32 = FRAME_SAMPLES as u32;

    /// A deterministic pseudo-random source, so a failure always reproduces.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn percent(&mut self, chance: u32) -> bool {
            self.next() % 100 < u64::from(chance)
        }
    }

    fn params() -> StreamParams {
        StreamParams::new(0x1234_5678)
    }

    fn datagram(sequence: u16, timestamp: u32, payload: &[u8]) -> Vec<u8> {
        let mut out = vec![0x80, config::RTP_PAYLOAD_TYPE];
        out.extend_from_slice(&sequence.to_be_bytes());
        out.extend_from_slice(&timestamp.to_be_bytes());
        out.extend_from_slice(&0x1234_5678u32.to_be_bytes());
        out.extend_from_slice(payload);
        out
    }

    /// Encodes a continuous 440 Hz tone the way the sender does.
    fn encode_tone(frames: usize) -> Vec<Vec<u8>> {
        let mut encoder = opus::Encoder::new(
            config::SAMPLE_RATE,
            opus::Channels::Stereo,
            opus::Application::Audio,
        )
        .unwrap();
        encoder.set_bitrate(opus::Bitrate::Bits(128_000)).unwrap();
        encoder.set_inband_fec(true).unwrap();
        encoder
            .set_packet_loss_perc(config::EXPECTED_LOSS_PERCENT as i32)
            .unwrap();

        (0..frames)
            .map(|frame| {
                let pcm: Vec<f32> = (0..FRAME_SAMPLES)
                    .flat_map(|i| {
                        let t = (frame * FRAME_SAMPLES + i) as f32 / config::SAMPLE_RATE as f32;
                        let value = (TAU * 440.0 * t).sin() * 0.5;
                        [value, value]
                    })
                    .collect();
                encoder.encode_vec_float(&pcm, 4000).unwrap()
            })
            .collect()
    }

    struct Outcome {
        output: Vec<f32>,
        stats: Stats,
        midpoint: Stats,
    }

    impl Outcome {
        /// Audio past the prebuffer, where playback is in steady state.
        fn steady(&self) -> &[f32] {
            let skip = FRAME_SAMPLES * 2 * 15;
            &self.output[skip.min(self.output.len())..]
        }
    }

    /// Streams `frames` frames through a link that drops `loss_percent` of
    /// packets and reorders `reorder_percent` of them, pulling audio in
    /// realtime-sized chunks the way an audio callback would.
    fn stream(frames: usize, loss_percent: u32, reorder_percent: u32, seed: u64) -> Outcome {
        let packets = encode_tone(frames);
        let mut pipeline = Pipeline::new(&params()).unwrap();
        let mut rng = Rng(seed);
        let mut output = Vec::new();
        let mut chunk = vec![0.0f32; FRAME_SAMPLES * 2];
        let mut now_us = 0u64;
        let mut held: Option<(u16, u32, Vec<u8>)> = None;
        let mut midpoint = Stats::default();

        for (index, payload) in packets.iter().enumerate() {
            let sequence = index as u16;
            let timestamp = (index as u32).wrapping_mul(TICKS);

            let release = held.take();
            if rng.percent(loss_percent) {
                // dropped in flight
            } else if reorder_percent > 0 && held.is_none() && rng.percent(reorder_percent) {
                held = Some((sequence, timestamp, payload.clone()));
            } else {
                pipeline.on_datagram(&datagram(sequence, timestamp, payload), now_us);
            }
            // Released on the next tick, after the packet behind it, so it
            // genuinely arrives out of order rather than merely late. Only one
            // is ever held, so nothing is lost to the reordering itself.
            if let Some((seq, ts, data)) = release {
                pipeline.on_datagram(&datagram(seq, ts, &data), now_us);
            }

            now_us += u64::from(config::FRAME_MS) * 1000;
            pipeline.fill(&mut chunk, now_us);
            output.extend_from_slice(&chunk);

            if index == frames / 2 {
                midpoint = pipeline.stats();
            }
        }
        Outcome {
            output,
            stats: pipeline.stats(),
            midpoint,
        }
    }

    fn rms(samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
    }

    /// The longest stretch with no audible energy, in milliseconds. Measured
    /// over 1 ms windows because a tone crosses zero every half cycle and
    /// per-sample thresholding would call that silence.
    fn longest_silence_ms(samples: &[f32], channels: usize) -> f64 {
        let window = channels * config::SAMPLE_RATE as usize / 1000;
        let (mut longest, mut current) = (0usize, 0usize);
        for chunk in samples.chunks(window) {
            if rms(chunk) < 1e-3 {
                current += 1;
                longest = longest.max(current);
            } else {
                current = 0;
            }
        }
        longest as f64
    }

    #[test]
    fn a_clean_link_plays_continuous_audio() {
        let outcome = stream(500, 0, 0, 1);
        let audio = outcome.steady();
        assert!(rms(audio) > 0.2, "rms {}", rms(audio));
        assert_eq!(longest_silence_ms(audio, 2), 0.0);
        assert_eq!(outcome.stats.jitter.lost, 0);
        assert_eq!(outcome.stats.jitter.underruns, 0);
    }

    #[test]
    fn survives_ten_percent_loss_without_gaps_or_underruns() {
        let outcome = stream(1500, 10, 0, 0x2545_f491_4f6c_dd1d);
        let audio = outcome.steady();
        let stats = outcome.stats.jitter;

        assert!(stats.lost > 100, "expected real loss, saw {}", stats.lost);

        assert_eq!(stats.underruns, 0, "10% loss must not starve the buffer");
        assert!(
            stats.target_ms > config::JITTER_MIN_MS,
            "loss bursts should have deepened the target from {}, got {}",
            config::JITTER_MIN_MS,
            stats.target_ms
        );
        assert!(
            outcome.midpoint.jitter.target_ms > config::JITTER_MIN_MS,
            "the target must adapt early, not at the end of the run"
        );
        assert!(
            longest_silence_ms(audio, 2) < 1.0,
            "audible dropout of {} ms",
            longest_silence_ms(audio, 2)
        );
        assert!(
            rms(audio) > 0.2,
            "concealment should hold the level, rms {}",
            rms(audio)
        );
        assert!(
            f64::from(stats.recovered as u32) / f64::from(stats.lost as u32) > 0.75,
            "most isolated losses should be recovered by FEC, got {}/{}",
            stats.recovered,
            stats.lost
        );
    }

    #[test]
    fn survives_one_and_five_percent_loss() {
        for loss in [1, 5] {
            let outcome = stream(1000, loss, 0, 7);
            let audio = outcome.steady();
            assert_eq!(outcome.stats.jitter.underruns, 0, "{loss}% loss underran");
            assert_eq!(outcome.stats.silence_frames, 0, "{loss}% loss ran dry");
            assert!(rms(audio) > 0.2, "{loss}% loss: rms {}", rms(audio));
        }
    }

    #[test]
    fn reordering_does_not_disturb_playback() {
        let outcome = stream(800, 0, 20, 99);
        let audio = outcome.steady();
        assert!(outcome.stats.jitter.reordered > 50, "test did not reorder");
        assert_eq!(outcome.stats.jitter.lost, 0, "reordering is not loss");
        assert_eq!(outcome.stats.silence_frames, 0);
        assert!(rms(audio) > 0.2);
    }

    #[test]
    fn loss_and_reordering_together() {
        let outcome = stream(1200, 5, 15, 0xdead_beef);
        let audio = outcome.steady();
        assert_eq!(outcome.stats.silence_frames, 0);
        assert!(longest_silence_ms(audio, 2) < 1.0);
        assert!(rms(audio) > 0.2, "rms {}", rms(audio));
    }

    #[test]
    fn rejects_foreign_and_malformed_packets() {
        let mut pipeline = Pipeline::new(&params()).unwrap();
        pipeline.on_datagram(&[0, 1, 2], 0);
        pipeline.on_datagram(&datagram(0, 0, &[1, 2, 3]), 0);

        let mut foreign = datagram(1, TICKS, &[1, 2, 3]);
        foreign[8..12].copy_from_slice(&0xffff_ffffu32.to_be_bytes());
        pipeline.on_datagram(&foreign, 0);

        let mut wrong_type = datagram(2, TICKS * 2, &[1, 2, 3]);
        wrong_type[1] = 97;
        pipeline.on_datagram(&wrong_type, 0);

        assert_eq!(
            pipeline.stats().rejected,
            3,
            "only the matching packet counts"
        );
        assert_eq!(pipeline.stats().jitter.received, 1);
    }

    #[test]
    fn output_is_silent_before_the_first_packet() {
        let mut pipeline = Pipeline::new(&params()).unwrap();
        let mut chunk = vec![1.0f32; FRAME_SAMPLES * 2];
        let report = pipeline.fill(&mut chunk, 0);
        assert_eq!(report.filled, 0);
        assert_eq!(report.silence, chunk.len());
        assert!(chunk.iter().all(|s| *s == 0.0));
    }
}
