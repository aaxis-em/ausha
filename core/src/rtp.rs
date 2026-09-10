//! RTP packet parsing (RFC 3550), building, and 16-bit sequence extension.

pub const HEADER_LEN: usize = 12;
const VERSION: u8 = 2;

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    TooShort,
    UnsupportedVersion(u8),
    BadPadding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Packet<'a> {
    pub payload_type: u8,
    pub sequence: u16,
    pub timestamp: u32,
    pub ssrc: u32,
    pub marker: bool,
    pub payload: &'a [u8],
}

pub fn parse(datagram: &[u8]) -> Result<Packet<'_>, Error> {
    if datagram.len() < HEADER_LEN {
        return Err(Error::TooShort);
    }
    let version = datagram[0] >> 6;
    if version != VERSION {
        return Err(Error::UnsupportedVersion(version));
    }

    let csrc_count = usize::from(datagram[0] & 0x0f);
    let has_extension = datagram[0] & 0x10 != 0;
    let has_padding = datagram[0] & 0x20 != 0;

    let mut start = HEADER_LEN + csrc_count * 4;
    if has_extension {
        let header_end = start + 4;
        if datagram.len() < header_end {
            return Err(Error::TooShort);
        }
        let words = u16::from_be_bytes([datagram[start + 2], datagram[start + 3]]);
        start = header_end + usize::from(words) * 4;
    }

    let mut end = datagram.len();
    if has_padding {
        let padding = usize::from(*datagram.last().ok_or(Error::TooShort)?);
        end = end.checked_sub(padding).ok_or(Error::BadPadding)?;
    }
    if end < start {
        return Err(Error::TooShort);
    }

    Ok(Packet {
        payload_type: datagram[1] & 0x7f,
        marker: datagram[1] & 0x80 != 0,
        sequence: u16::from_be_bytes([datagram[2], datagram[3]]),
        timestamp: u32::from_be_bytes([datagram[4], datagram[5], datagram[6], datagram[7]]),
        ssrc: u32::from_be_bytes([datagram[8], datagram[9], datagram[10], datagram[11]]),
        payload: &datagram[start..end],
    })
}

/// Writes the packets the uplink sends. The downlink never needs this — ffmpeg
/// builds its own headers — so this only ever emits the shape we produce: no
/// CSRC, no extension, no padding.
pub struct Builder {
    payload_type: u8,
    ssrc: u32,
    sequence: u16,
    timestamp: u32,
}

impl Builder {
    /// Starts at sequence and timestamp zero rather than at a random offset as
    /// RFC 3550 suggests. The offset exists to make streams hard to correlate
    /// under SRTP; here the SSRC is already fresh per session, and starting at
    /// zero makes a capture far easier to read.
    pub fn new(payload_type: u8, ssrc: u32) -> Self {
        Self {
            payload_type,
            ssrc,
            sequence: 0,
            timestamp: 0,
        }
    }

    /// Appends one frame's packet to `out`, which is cleared first. `samples`
    /// is the frame's length in samples per channel, which is what the RTP
    /// timestamp counts.
    pub fn build(&mut self, payload: &[u8], samples: u32, out: &mut Vec<u8>) {
        out.clear();
        out.reserve(HEADER_LEN + payload.len());
        out.push(VERSION << 6);
        out.push(self.payload_type & 0x7f);
        out.extend_from_slice(&self.sequence.to_be_bytes());
        out.extend_from_slice(&self.timestamp.to_be_bytes());
        out.extend_from_slice(&self.ssrc.to_be_bytes());
        out.extend_from_slice(payload);

        self.sequence = self.sequence.wrapping_add(1);
        self.timestamp = self.timestamp.wrapping_add(samples);
    }
}

/// Lifts 16-bit sequence numbers into a monotonic 64-bit space so the jitter
/// buffer can order packets across wraparound, which happens every 65536
/// packets — about every 22 minutes at 50 packets per second.
#[derive(Debug, Default)]
pub struct SequenceExtender {
    cycles: u64,
    highest: u16,
    started: bool,
}

impl SequenceExtender {
    pub fn extend(&mut self, sequence: u16) -> u64 {
        let extended = self.peek(sequence);
        self.commit(sequence);
        extended
    }

    /// The value [`extend`](Self::extend) would return, without recording the
    /// sequence number. Splitting the two lets a caller that can still reject
    /// the packet — decryption, say — extend it before deciding to trust it.
    pub fn peek(&self, sequence: u16) -> u64 {
        if !self.started {
            return u64::from(sequence);
        }
        if self.moves_forward(sequence) {
            let cycles = self.cycles + u64::from(sequence < self.highest);
            (cycles << 16) | u64::from(sequence)
        } else if sequence > self.highest && self.cycles > 0 {
            ((self.cycles - 1) << 16) | u64::from(sequence)
        } else {
            (self.cycles << 16) | u64::from(sequence)
        }
    }

    pub fn commit(&mut self, sequence: u16) {
        if !self.started {
            self.started = true;
            self.highest = sequence;
        } else if self.moves_forward(sequence) {
            if sequence < self.highest {
                self.cycles += 1;
            }
            self.highest = sequence;
        }
    }

    /// True when `sequence` is closer ahead of the highest seen than behind
    /// it, which is how a wrap is told apart from a late packet.
    fn moves_forward(&self, sequence: u16) -> bool {
        sequence.wrapping_sub(self.highest) <= self.highest.wrapping_sub(sequence)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(sequence: u16, timestamp: u32, payload: &[u8]) -> Vec<u8> {
        let mut out = vec![0x80, 96];
        out.extend_from_slice(&sequence.to_be_bytes());
        out.extend_from_slice(&timestamp.to_be_bytes());
        out.extend_from_slice(&0x1234_5678u32.to_be_bytes());
        out.extend_from_slice(payload);
        out
    }

    #[test]
    fn parses_a_plain_packet() {
        let datagram = header(7, 960, &[1, 2, 3]);
        let packet = parse(&datagram).unwrap();
        assert_eq!(packet.sequence, 7);
        assert_eq!(packet.timestamp, 960);
        assert_eq!(packet.ssrc, 0x1234_5678);
        assert_eq!(packet.payload_type, 96);
        assert_eq!(packet.payload, &[1, 2, 3]);
    }

    #[test]
    fn rejects_short_and_wrong_version() {
        assert_eq!(parse(&[0x80, 96, 0, 1]), Err(Error::TooShort));
        let mut datagram = header(1, 0, &[9]);
        datagram[0] = 0x40;
        assert_eq!(parse(&datagram), Err(Error::UnsupportedVersion(1)));
    }

    #[test]
    fn skips_csrc_and_extension_headers() {
        let mut datagram = vec![0x82 | 0x10, 96];
        datagram.extend_from_slice(&5u16.to_be_bytes());
        datagram.extend_from_slice(&0u32.to_be_bytes());
        datagram.extend_from_slice(&0u32.to_be_bytes());
        datagram.extend_from_slice(&[0; 8]);
        datagram.extend_from_slice(&[0xbe, 0xde, 0, 1]);
        datagram.extend_from_slice(&[0; 4]);
        datagram.extend_from_slice(&[42, 43]);
        assert_eq!(parse(&datagram).unwrap().payload, &[42, 43]);
    }

    #[test]
    fn strips_padding() {
        let mut datagram = header(1, 0, &[7, 0, 0, 3]);
        datagram[0] |= 0x20;
        assert_eq!(parse(&datagram).unwrap().payload, &[7]);
    }

    #[test]
    fn extends_across_wraparound() {
        let mut extender = SequenceExtender::default();
        assert_eq!(extender.extend(65534), 65534);
        assert_eq!(extender.extend(65535), 65535);
        assert_eq!(extender.extend(0), 65536);
        assert_eq!(extender.extend(1), 65537);
    }

    #[test]
    fn orders_reordered_packets_across_wraparound() {
        let mut extender = SequenceExtender::default();
        extender.extend(65535);
        assert_eq!(extender.extend(1), 65537);
        assert_eq!(
            extender.extend(0),
            65536,
            "late packet from before the wrap"
        );
        assert_eq!(
            extender.extend(65534),
            65534,
            "late packet from before the wrap"
        );
    }

    #[test]
    fn keeps_reordered_packets_monotonic_in_the_middle() {
        let mut extender = SequenceExtender::default();
        for sequence in [100, 101, 103, 102, 104] {
            let _ = extender.extend(sequence);
        }
        let mut extender = SequenceExtender::default();
        extender.extend(100);
        assert_eq!(extender.extend(103), 103);
        assert_eq!(extender.extend(102), 102);
    }
}

#[cfg(test)]
mod builder_tests {
    use super::*;

    #[test]
    fn builds_a_packet_that_parses_back() {
        let mut builder = Builder::new(97, 0xdead_beef);
        let mut out = Vec::new();
        builder.build(&[1, 2, 3], 960, &mut out);

        let packet = parse(&out).unwrap();
        assert_eq!(packet.payload_type, 97);
        assert_eq!(packet.ssrc, 0xdead_beef);
        assert_eq!(packet.sequence, 0);
        assert_eq!(packet.timestamp, 0);
        assert_eq!(packet.payload, &[1, 2, 3]);
        assert!(!packet.marker);
    }

    #[test]
    fn advances_the_sequence_by_one_and_the_timestamp_by_a_frame() {
        let mut builder = Builder::new(97, 1);
        let mut out = Vec::new();
        for expected in 0..4u16 {
            builder.build(&[0], 960, &mut out);
            let packet = parse(&out).unwrap();
            assert_eq!(packet.sequence, expected);
            assert_eq!(packet.timestamp, u32::from(expected) * 960);
        }
    }

    #[test]
    fn wraps_the_sequence_the_way_the_extender_expects() {
        let mut builder = Builder::new(97, 1);
        builder.sequence = 65534;
        let mut out = Vec::new();
        let mut extender = SequenceExtender::default();

        let mut extended = Vec::new();
        for _ in 0..4 {
            builder.build(&[0], 960, &mut out);
            extended.push(extender.extend(parse(&out).unwrap().sequence));
        }
        assert_eq!(extended, [65534, 65535, 65536, 65537]);
    }
}
