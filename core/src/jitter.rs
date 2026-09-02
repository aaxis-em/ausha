//! Reorder buffer with an adaptive target depth, and the per-frame decision of
//! whether to decode, recover from FEC, or conceal.

use std::collections::BTreeMap;

use crate::rtp::{Packet, SequenceExtender};

/// What the decoder should do for the next frame of audio.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    /// The frame arrived; decode it.
    Decode(Vec<u8>),
    /// The frame was lost but the following one carries a redundant copy.
    /// Decode this payload with Opus in-band FEC.
    Recover(Vec<u8>),
    /// The frame was lost with no FEC source; run packet loss concealment.
    Conceal,
    /// Not enough buffered yet to start or continue; play silence.
    Starve,
}

#[derive(Debug, Default, Clone, Copy, serde::Serialize)]
pub struct Stats {
    pub received: u64,
    pub lost: u64,
    pub recovered: u64,
    pub concealed: u64,
    pub reordered: u64,
    pub duplicates: u64,
    pub late: u64,
    pub underruns: u64,
    pub longest_burst: u32,
    pub jitter_ms: f64,
    pub target_ms: u32,
    pub depth_ms: u32,
}

pub struct JitterBuffer {
    frames: BTreeMap<u64, Vec<u8>>,
    extender: SequenceExtender,
    next: Option<u64>,
    playing: bool,
    frame_ms: u32,
    clock_rate: u32,
    min_target_ms: u32,
    max_target_ms: u32,
    target_ms: u32,
    jitter: f64,
    last_transit: Option<i64>,
    highest_seen: Option<u64>,
    current_burst: u32,
    longest_burst: u32,
    clean_since_us: u64,
    stats: Stats,
}

/// How many jitter estimates of headroom the target depth keeps.
const JITTER_MARGIN: f64 = 3.0;
/// A shrink is only considered after this long without growing the target.
const SHRINK_AFTER_US: u64 = 10_000_000;
const SHRINK_STEP_MS: u32 = 5;

impl JitterBuffer {
    pub fn new(frame_ms: u32, clock_rate: u32, min_target_ms: u32, max_target_ms: u32) -> Self {
        Self {
            frames: BTreeMap::new(),
            extender: SequenceExtender::default(),
            next: None,
            playing: false,
            frame_ms,
            clock_rate,
            min_target_ms,
            max_target_ms,
            target_ms: min_target_ms,
            jitter: 0.0,
            last_transit: None,
            highest_seen: None,
            current_burst: 0,
            longest_burst: 0,
            clean_since_us: 0,
            stats: Stats {
                target_ms: min_target_ms,
                ..Stats::default()
            },
        }
    }

    pub fn stats(&self) -> Stats {
        Stats {
            longest_burst: self.longest_burst,
            jitter_ms: self.jitter_ms(),
            target_ms: self.target_ms,
            depth_ms: self.depth_ms(),
            ..self.stats
        }
    }

    pub fn target_ms(&self) -> u32 {
        self.target_ms
    }

    pub fn depth_ms(&self) -> u32 {
        (self.frames.len() as u32) * self.frame_ms
    }

    pub fn insert(&mut self, packet: &Packet, arrival_us: u64) {
        self.observe_jitter(packet.timestamp, arrival_us);
        self.stats.received += 1;

        let extended = self.extender.extend(packet.sequence);
        match self.highest_seen {
            Some(highest) if extended < highest => self.stats.reordered += 1,
            _ => self.highest_seen = Some(extended),
        }

        if let Some(next) = self.next
            && extended < next
        {
            self.stats.late += 1;
            return;
        }
        if self
            .frames
            .insert(extended, packet.payload.to_vec())
            .is_some()
        {
            self.stats.duplicates += 1;
        }
        self.retarget(arrival_us);
    }

    pub fn pop(&mut self) -> Step {
        if !self.playing {
            // One frame past the target, because the pop below takes a frame
            // straight back out; the target is the depth held while playing.
            if self.depth_ms() < self.target_ms + self.frame_ms {
                return Step::Starve;
            }
            self.playing = true;
            self.next = self.frames.keys().next().copied();
        }

        let Some(next) = self.next else {
            return Step::Starve;
        };

        if let Some(payload) = self.frames.remove(&next) {
            self.current_burst = 0;
            self.next = Some(next + 1);
            return Step::Decode(payload);
        }

        if self.frames.is_empty() {
            self.stats.underruns += 1;

            self.grow_after_underrun();
            self.playing = false;
            self.next = None;
            return Step::Starve;
        }

        self.stats.lost += 1;
        self.current_burst += 1;
        self.longest_burst = self.longest_burst.max(self.current_burst);
        self.next = Some(next + 1);
        match self.frames.get(&(next + 1)) {
            Some(fec_source) => {
                self.stats.recovered += 1;
                Step::Recover(fec_source.clone())
            }
            None => {
                self.stats.concealed += 1;
                Step::Conceal
            }
        }
    }

    /// Running dry means the target was too low for this link, whatever the
    /// jitter estimate said. Take the depth immediately and hold it: the
    /// shrink timer restarts from the next insert.
    fn grow_after_underrun(&mut self) {
        self.longest_burst += 1;
        self.target_ms = (self.target_ms + 2 * self.frame_ms).min(self.max_target_ms);
        self.clean_since_us = 0;
    }

    fn jitter_ms(&self) -> f64 {
        self.jitter * 1000.0 / f64::from(self.clock_rate)
    }

    /// RFC 3550 interarrival jitter: a smoothed estimate of how much the gap
    /// between arrivals differs from the gap the timestamps asked for.
    fn observe_jitter(&mut self, timestamp: u32, arrival_us: u64) {
        let arrival_ticks = (arrival_us as i128 * i128::from(self.clock_rate) / 1_000_000) as i64;
        let transit = arrival_ticks - i64::from(timestamp);
        if let Some(previous) = self.last_transit {
            let difference = (transit - previous).abs() as f64;
            self.jitter += (difference - self.jitter) / 16.0;
        }
        self.last_transit = Some(transit);
    }

    /// Grows the target immediately when jitter demands it, and gives back
    /// depth only slowly, so the buffer does not oscillate on a noisy link.
    fn retarget(&mut self, now_us: u64) {
        let for_jitter = (JITTER_MARGIN * self.jitter_ms()).ceil() as u32 + self.frame_ms;
        // A run of N consecutive losses empties N frames of depth without
        // replacing them, so the buffer has to be deeper than the worst run
        // seen recently. Reacting to bursts is what keeps the rare long run
        // from becoming an underrun, rather than waiting for one to happen.
        let for_bursts = (self.longest_burst + 2) * self.frame_ms;
        let wanted = for_jitter
            .max(for_bursts)
            .clamp(self.min_target_ms, self.max_target_ms);

        if wanted > self.target_ms {
            self.target_ms = wanted;
            self.clean_since_us = now_us;
            return;
        }
        if self.clean_since_us == 0 {
            self.clean_since_us = now_us;
            return;
        }
        if now_us.saturating_sub(self.clean_since_us) >= SHRINK_AFTER_US {
            self.longest_burst = self.longest_burst.saturating_sub(1);
            self.target_ms = self
                .target_ms
                .saturating_sub(SHRINK_STEP_MS)
                .max(wanted.max(self.min_target_ms));
            self.clean_since_us = now_us;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rtp::Packet;

    const FRAME_MS: u32 = 20;
    const CLOCK: u32 = 48000;
    const TICKS: u32 = CLOCK / 1000 * FRAME_MS;

    struct Link {
        buffer: JitterBuffer,
        now_us: u64,
        sequence: u16,
        timestamp: u32,
    }

    impl Link {
        fn new() -> Self {
            Self {
                buffer: JitterBuffer::new(FRAME_MS, CLOCK, 40, 200),
                now_us: 0,
                sequence: 0,
                timestamp: 0,
            }
        }

        /// Produces the next frame the sender would emit, advancing its clock.
        fn emit(&mut self) -> (u16, u32, Vec<u8>) {
            let frame = (self.sequence, self.timestamp, vec![self.sequence as u8; 40]);
            self.sequence = self.sequence.wrapping_add(1);
            self.timestamp = self.timestamp.wrapping_add(TICKS);
            self.now_us += u64::from(FRAME_MS) * 1000;
            frame
        }

        fn deliver(&mut self, frame: &(u16, u32, Vec<u8>), extra_delay_us: u64) {
            let packet = Packet {
                payload_type: 96,
                sequence: frame.0,
                timestamp: frame.1,
                ssrc: 1,
                marker: false,
                payload: &frame.2,
            };
            let arrival = self.now_us + extra_delay_us;
            self.buffer.insert(&packet, arrival);
        }
    }

    /// Runs `frames` frames through the buffer, dropping every `drop_every`-th
    /// one, and returns what the decoder was asked to do for each output slot.
    ///
    /// The receiver pulls exactly one frame per frame period, the way an audio
    /// callback does. Draining the buffer on every tick instead would defeat
    /// the point of having one.
    fn run_with_loss(frames: usize, drop_every: usize) -> (Vec<Step>, Stats) {
        let mut link = Link::new();
        let mut steps = Vec::new();
        for index in 0..frames {
            let frame = link.emit();
            if drop_every == 0 || index % drop_every != drop_every - 1 {
                link.deliver(&frame, 0);
            }
            match link.buffer.pop() {
                Step::Starve if steps.is_empty() => {}
                step => steps.push(step),
            }
        }
        (steps, link.buffer.stats())
    }

    #[test]
    fn holds_output_until_the_target_is_buffered() {
        let mut link = Link::new();
        for _ in 0..2 {
            let frame = link.emit();
            link.deliver(&frame, 0);
            assert_eq!(
                link.buffer.pop(),
                Step::Starve,
                "under the 40 ms target plus a frame"
            );
        }
        let frame = link.emit();
        link.deliver(&frame, 0);
        assert!(
            matches!(link.buffer.pop(), Step::Decode(_)),
            "three frames meet it"
        );
    }

    #[test]
    fn delivers_a_clean_stream_in_order_with_no_gaps() {
        let (steps, stats) = run_with_loss(200, 0);
        assert!(steps.iter().all(|s| matches!(s, Step::Decode(_))));
        assert_eq!(stats.lost, 0);
        assert_eq!(stats.underruns, 0);

        let payloads: Vec<u8> = steps
            .iter()
            .map(|s| match s {
                Step::Decode(p) => p[0],
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        let expected: Vec<u8> = (0..payloads.len()).map(|i| i as u8).collect();
        assert_eq!(payloads, expected, "frames must come out in sender order");
    }

    #[test]
    fn reordered_packets_are_put_back_in_order() {
        let mut link = Link::new();
        let frames: Vec<_> = (0..6).map(|_| link.emit()).collect();
        for index in [0, 1, 3, 2, 5, 4] {
            link.deliver(&frames[index], 0);
        }
        let mut order = Vec::new();
        while let Step::Decode(payload) = link.buffer.pop() {
            order.push(payload[0]);
        }
        assert_eq!(order, vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(link.buffer.stats().lost, 0);
        assert_eq!(link.buffer.stats().reordered, 2);
    }

    #[test]
    fn an_isolated_loss_is_recovered_from_the_next_packets_fec() {
        let (steps, stats) = run_with_loss(300, 10);
        let recovered = steps
            .iter()
            .filter(|s| matches!(s, Step::Recover(_)))
            .count();
        let concealed = steps.iter().filter(|s| matches!(s, Step::Conceal)).count();
        assert!(recovered > 0, "isolated losses should use FEC");
        assert_eq!(concealed, 0, "an isolated loss never needs concealment");
        assert_eq!(stats.recovered as usize, recovered);
        assert_eq!(stats.underruns, 0);
    }

    #[test]
    fn consecutive_losses_fall_back_to_concealment() {
        let mut link = Link::new();
        let frames: Vec<_> = (0..12).map(|_| link.emit()).collect();
        for (index, frame) in frames.iter().enumerate() {
            if !(4..=6).contains(&index) {
                link.deliver(frame, 0);
            }
        }
        let mut steps = Vec::new();
        loop {
            match link.buffer.pop() {
                Step::Starve => break,
                step => steps.push(step),
            }
        }
        assert_eq!(steps[4], Step::Conceal, "no FEC source two frames ahead");
        assert_eq!(steps[5], Step::Conceal);
        assert!(
            matches!(steps[6], Step::Recover(_)),
            "frame 7 carries frame 6"
        );
        assert!(matches!(steps[7], Step::Decode(_)));
    }

    #[test]
    fn survives_ten_percent_loss_without_underrunning() {
        let (steps, stats) = run_with_loss(1000, 10);
        assert_eq!(stats.underruns, 0, "10% loss must not starve the buffer");
        // Two ticks are spent prebuffering to the 40 ms target plus a frame.
        assert_eq!(
            steps.len(),
            1000 - 2,
            "one output slot per sender frame after that"
        );
        let handled = steps
            .iter()
            .filter(|s| matches!(s, Step::Recover(_) | Step::Conceal))
            .count();
        assert_eq!(handled, stats.lost as usize);
        assert!(
            stats.recovered as f64 / stats.lost as f64 > 0.99,
            "isolated losses should almost all be recovered, got {}/{}",
            stats.recovered,
            stats.lost
        );
    }

    #[test]
    fn a_jitter_burst_grows_the_target_and_it_shrinks_back_slowly() {
        let mut link = Link::new();
        for _ in 0..20 {
            let frame = link.emit();
            link.deliver(&frame, 0);
            let _ = link.buffer.pop();
        }
        let calm = link.buffer.target_ms();

        for index in 0..40 {
            let frame = link.emit();
            let spike = if index % 2 == 0 { 50_000 } else { 0 };
            link.deliver(&frame, spike);
            let _ = link.buffer.pop();
        }
        let stressed = link.buffer.target_ms();
        assert!(
            stressed > calm,
            "50 ms jitter bursts must grow the target ({calm} -> {stressed})"
        );

        for _ in 0..2000 {
            let frame = link.emit();
            link.deliver(&frame, 0);
            let _ = link.buffer.pop();
        }
        let recovered = link.buffer.target_ms();
        assert!(
            recovered < stressed,
            "target must give depth back once the link is calm ({stressed} -> {recovered})"
        );
        assert!(recovered >= 40, "target must not fall below the floor");
    }

    #[test]
    fn the_target_is_clamped_to_its_bounds() {
        let mut link = Link::new();
        for index in 0..200 {
            let frame = link.emit();
            link.deliver(&frame, if index % 2 == 0 { 900_000 } else { 0 });
            let _ = link.buffer.pop();
        }
        assert_eq!(link.buffer.target_ms(), 200, "clamped to the ceiling");
    }

    #[test]
    fn late_packets_are_dropped_rather_than_played_out_of_order() {
        let mut link = Link::new();
        let frames: Vec<_> = (0..8).map(|_| link.emit()).collect();
        for frame in frames.iter().skip(1) {
            link.deliver(frame, 0);
        }
        let mut steps = Vec::new();
        for _ in 0..4 {
            steps.push(link.buffer.pop());
        }
        link.deliver(&frames[0], 0);
        assert_eq!(link.buffer.stats().late, 1);
        assert!(steps.iter().all(|s| !matches!(s, Step::Starve)));
    }

    #[test]
    fn duplicates_are_counted_and_ignored() {
        let mut link = Link::new();
        let frames: Vec<_> = (0..6).map(|_| link.emit()).collect();
        for frame in &frames {
            link.deliver(frame, 0);
            link.deliver(frame, 0);
        }
        assert_eq!(link.buffer.stats().duplicates, 6);
        let mut count = 0;
        while let Step::Decode(_) = link.buffer.pop() {
            count += 1;
        }
        assert_eq!(count, 6, "a duplicate must not produce an extra frame");
    }
}
