//! Fractional resampling for drift correction.
//!
//! Catmull-Rom interpolation over interleaved frames. The ratios involved are
//! tiny — a fraction of a percent — so this only ever has to be transparent
//! very close to 1.0, not to resample across rates.

pub struct Resampler {
    channels: usize,
    history: Vec<f32>,
    position: f64,
}

impl Resampler {
    pub fn new(channels: usize) -> Self {
        Self {
            channels,
            history: vec![0.0; channels * 4],
            position: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.history.iter_mut().for_each(|sample| *sample = 0.0);
        self.position = 0.0;
    }

    /// Consumes `input` at `ratio` input frames per output frame, appending
    /// interpolated frames to `output`. A ratio above 1.0 consumes faster than
    /// it emits, which is how the buffer is drained when it grows too deep.
    pub fn process(&mut self, input: &[f32], ratio: f64, output: &mut Vec<f32>) {
        debug_assert_eq!(input.len() % self.channels, 0);
        let frames = input.len() / self.channels;

        for frame in 0..frames {
            self.push(&input[frame * self.channels..(frame + 1) * self.channels]);
            while self.position < 1.0 {
                for channel in 0..self.channels {
                    output.push(self.interpolate(channel, self.position));
                }
                self.position += ratio;
            }
            self.position -= 1.0;
        }
    }

    fn push(&mut self, frame: &[f32]) {
        self.history.copy_within(self.channels.., 0);
        let tail = self.history.len() - self.channels;
        self.history[tail..].copy_from_slice(frame);
    }

    fn interpolate(&self, channel: usize, t: f64) -> f32 {
        let at = |index: usize| f64::from(self.history[index * self.channels + channel]);
        let (p0, p1, p2, p3) = (at(0), at(1), at(2), at(3));
        let a = -0.5 * p0 + 1.5 * p1 - 1.5 * p2 + 0.5 * p3;
        let b = p0 - 2.5 * p1 + 2.0 * p2 - 0.5 * p3;
        let c = -0.5 * p0 + 0.5 * p2;
        (((a * t + b) * t + c) * t + p1) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::TAU;

    fn sine(frames: usize, hz: f64, rate: f64) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let value = (TAU * hz * i as f64 / rate).sin() as f32;
                [value, value]
            })
            .collect()
    }

    #[test]
    fn a_ratio_of_one_passes_audio_through_unchanged() {
        let mut resampler = Resampler::new(2);
        let input = sine(512, 440.0, 48000.0);
        let mut output = Vec::new();
        resampler.process(&input, 1.0, &mut output);

        assert_eq!(output.len(), input.len());
        // At t=0 Catmull-Rom returns its second control point, so the output
        // is the input delayed by two frames.
        let delay = 2 * 2;
        let error = output[delay..]
            .iter()
            .zip(&input[..input.len() - delay])
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(error < 1e-6, "max error {error}");
    }

    #[test]
    fn output_length_tracks_the_ratio() {
        for ratio in [0.995, 0.999, 1.001, 1.005] {
            let mut resampler = Resampler::new(2);
            let input = sine(48000, 440.0, 48000.0);
            let mut output = Vec::new();
            resampler.process(&input, ratio, &mut output);

            let produced = (output.len() / 2) as f64;
            let expected = 48000.0 / ratio;
            let drift = (produced - expected).abs() / expected;
            assert!(drift < 0.001, "ratio {ratio}: {produced} vs {expected}");
        }
    }

    #[test]
    fn resampling_a_sine_keeps_it_clean() {
        let mut resampler = Resampler::new(2);
        let input = sine(48000, 440.0, 48000.0);
        let mut output = Vec::new();
        resampler.process(&input, 1.005, &mut output);

        let left: Vec<f64> = output
            .iter()
            .step_by(2)
            .skip(64)
            .map(|&s| f64::from(s))
            .collect();
        let peak = left.iter().fold(0.0f64, |m, s| m.max(s.abs()));
        assert!(
            (0.99..=1.01).contains(&peak),
            "peak {peak} should stay near 1.0"
        );

        // A clean resampled sine crosses zero at a steady rate; distortion or
        // dropped samples would add extra crossings.
        let crossings = left
            .windows(2)
            .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
            .count();
        let expected = left.len() as f64 * 440.0 * 1.005 / 48000.0;
        assert!(
            (crossings as f64 - expected).abs() < 2.0,
            "{crossings} crossings, expected about {expected:.1}"
        );
    }

    #[test]
    fn state_carries_across_calls() {
        let input = sine(4800, 440.0, 48000.0);
        let mut whole = Vec::new();
        Resampler::new(2).process(&input, 1.002, &mut whole);

        let mut split = Vec::new();
        let mut resampler = Resampler::new(2);
        for chunk in input.chunks(240) {
            resampler.process(chunk, 1.002, &mut split);
        }
        assert_eq!(whole.len(), split.len());
        assert_eq!(whole, split, "chunking must not change the result");
    }
}
