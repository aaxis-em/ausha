//! Corrects for the sender and receiver running on different crystals.
//!
//! A few tens of ppm of difference moves the buffer by a hundred milliseconds
//! an hour, so a stream left alone eventually either underruns or accumulates
//! delay. This watches the buffer depth and returns a resampling ratio that
//! nudges it back, slowly enough to stay inaudible.

/// Widest correction applied, as a fraction of the nominal rate. 0.5% is far
/// more than crystal drift needs and stays below the threshold of hearing for
/// a slow pitch change.
pub const MAX_CORRECTION: f64 = 0.005;
/// Depth error below this is ignored, so the ratio does not chase jitter.
const DEADBAND_MS: f64 = 5.0;
/// How much of the error to correct per update. Sized for recovery, not for
/// drift: when the target grows, the depth has to follow it within seconds, and
/// steady-state drift is a few milliseconds of error that the deadband absorbs.
/// At this gain a 25 ms shortfall is closed in about five seconds.
const GAIN: f64 = 0.0002;
/// Minimum spacing between updates.
const INTERVAL_US: u64 = 1_000_000;
/// Ratio movement allowed per update, which bounds how fast pitch can change.
const SLEW: f64 = 0.0005;

pub struct DriftController {
    ratio: f64,
    last_update_us: Option<u64>,
}

impl Default for DriftController {
    fn default() -> Self {
        Self {
            ratio: 1.0,
            last_update_us: None,
        }
    }
}

impl DriftController {
    pub fn ratio(&self) -> f64 {
        self.ratio
    }

    pub fn reset(&mut self) {
        self.ratio = 1.0;
        self.last_update_us = None;
    }

    /// Feeds the controller the current buffer depth. Returns the ratio to
    /// resample at: above 1.0 consumes buffered audio faster than real time.
    pub fn update(&mut self, depth_ms: f64, target_ms: f64, now_us: u64) -> f64 {
        let due = match self.last_update_us {
            Some(last) => now_us.saturating_sub(last) >= INTERVAL_US,
            None => true,
        };
        if !due {
            return self.ratio;
        }
        self.last_update_us = Some(now_us);

        let error = depth_ms - target_ms;
        let wanted = if error.abs() < DEADBAND_MS {
            1.0
        } else {
            1.0 + (error * GAIN).clamp(-MAX_CORRECTION, MAX_CORRECTION)
        };

        self.ratio += (wanted - self.ratio).clamp(-SLEW, SLEW);
        self.ratio = self.ratio.clamp(1.0 - MAX_CORRECTION, 1.0 + MAX_CORRECTION);
        self.ratio
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Models a receiver whose clock runs `ppm` faster than the sender's, so
    /// it consumes audio faster than it arrives and the buffer drains.
    fn simulate(ppm: f64, hours: f64, target_ms: f64) -> (f64, f64, f64) {
        let mut controller = DriftController::default();
        let mut depth_ms = target_ms;
        let mut now_us = 0u64;
        let (mut low, mut high) = (depth_ms, depth_ms);

        let seconds = (hours * 3600.0) as u64;
        for _ in 0..seconds {
            let ratio = controller.update(depth_ms, target_ms, now_us);
            // Per second: the sender delivers 1000 ms of audio, the receiver
            // consumes 1000 ms scaled by its own clock and by the correction.
            let consumed = 1000.0 * (1.0 + ppm / 1e6) * ratio;
            depth_ms += 1000.0 - consumed;
            low = low.min(depth_ms);
            high = high.max(depth_ms);
            now_us += INTERVAL_US;
        }
        (depth_ms, low, high)
    }

    #[test]
    fn an_uncorrected_clock_would_drain_the_buffer_within_the_hour() {
        // 30 ppm over an hour is 108 ms, which is more than the whole buffer.
        let uncorrected = 3600.0 * 1000.0 * 30.0 / 1e6;
        assert!(uncorrected > 100.0, "{uncorrected} ms of drift per hour");
    }

    #[test]
    fn holds_the_buffer_across_an_hour_at_thirty_ppm() {
        let (final_depth, low, high) = simulate(30.0, 1.0, 60.0);
        assert!(low > 20.0, "buffer fell to {low} ms, close to underrun");
        assert!(high < 120.0, "buffer grew to {high} ms, adding latency");
        assert!(
            (final_depth - 60.0).abs() < 25.0,
            "settled at {final_depth} ms"
        );
    }

    #[test]
    fn holds_the_buffer_for_a_clock_running_slow() {
        let (final_depth, low, high) = simulate(-30.0, 1.0, 60.0);
        assert!(low > 20.0, "buffer fell to {low} ms");
        assert!(high < 120.0, "buffer grew to {high} ms");
        assert!(
            (final_depth - 60.0).abs() < 25.0,
            "settled at {final_depth} ms"
        );
    }

    #[test]
    fn survives_a_day_at_a_hundred_ppm() {
        let (_, low, high) = simulate(100.0, 24.0, 80.0);
        assert!(low > 20.0, "buffer fell to {low} ms over 24 hours");
        assert!(high < 160.0, "buffer grew to {high} ms over 24 hours");
    }

    #[test]
    fn small_errors_inside_the_deadband_leave_the_ratio_alone() {
        let mut controller = DriftController::default();
        let ratio = controller.update(62.0, 60.0, 0);
        assert_eq!(ratio, 1.0, "2 ms of error is jitter, not drift");
    }

    #[test]
    fn the_correction_stays_within_its_bounds() {
        let mut controller = DriftController::default();
        let mut now_us = 0;
        for _ in 0..1000 {
            let ratio = controller.update(100_000.0, 60.0, now_us);
            assert!(
                ratio <= 1.0 + MAX_CORRECTION,
                "ratio {ratio} exceeded the cap"
            );
            now_us += INTERVAL_US;
        }
        let mut controller = DriftController::default();
        let mut now_us = 0;
        for _ in 0..1000 {
            let ratio = controller.update(0.0, 60.0, now_us);
            assert!(
                ratio >= 1.0 - MAX_CORRECTION,
                "ratio {ratio} exceeded the cap"
            );
            now_us += INTERVAL_US;
        }
    }

    #[test]
    fn updates_are_rate_limited() {
        let mut controller = DriftController::default();
        controller.update(200.0, 60.0, 0);
        let first = controller.ratio();
        controller.update(200.0, 60.0, 500_000);
        assert_eq!(controller.ratio(), first, "half a second is too soon");
        controller.update(200.0, 60.0, 1_000_000);
        assert_ne!(controller.ratio(), first, "a full second is due");
    }
}
