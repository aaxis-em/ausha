//! RTP packet parsing (RFC 3550) and 16-bit sequence number extension.

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
        if !self.started {
            self.started = true;
            self.highest = sequence;
            return u64::from(sequence);
        }

        let ahead = sequence.wrapping_sub(self.highest);
        let behind = self.highest.wrapping_sub(sequence);

        if ahead <= behind {
            if sequence < self.highest {
                self.cycles += 1;
            }
            self.highest = sequence;
            (self.cycles << 16) | u64::from(sequence)
        } else if sequence > self.highest && self.cycles > 0 {
            ((self.cycles - 1) << 16) | u64::from(sequence)
        } else {
            (self.cycles << 16) | u64::from(sequence)
        }
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
