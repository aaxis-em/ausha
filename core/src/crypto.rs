//! ChaCha20-Poly1305 over the RTP payload, keyed by the pairing token.
//!
//! The 12-byte RTP header stays in the clear and is authenticated as
//! associated data, the way SRTP does it: a capture still shows sequence
//! numbers, timestamps and SSRC, so loss and jitter remain diagnosable, while
//! the audio itself does not leave the machine in the clear.

use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{ChaCha20Poly1305, KeyInit, Nonce};
use sha2::{Digest, Sha256};

use crate::protocol::normalize_token;
use crate::rtp::{self, SequenceExtender};

pub const CIPHER: &str = "chacha20-poly1305";
pub const KEY_LEN: usize = 32;
pub const SALT_LEN: usize = 16;
pub const TAG_LEN: usize = 16;

/// A pairing token is 48 bits, which a GPU walks in hours if the derivation is
/// cheap. PBKDF2 at this cost puts a brute force out of reach while adding a
/// couple of hundred milliseconds to a handshake that already blocks.
const KDF_ROUNDS: u32 = 200_000;

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    UnknownCipher(String),
    BadSalt,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::UnknownCipher(name) => write!(f, "unsupported cipher {name:?}"),
            Error::BadSalt => write!(f, "malformed encryption salt"),
        }
    }
}

impl std::error::Error for Error {}

#[derive(Clone)]
pub struct Key([u8; KEY_LEN]);

/// Which way the audio is going. The two directions must never share a key: the
/// nonce is the SSRC and the sequence number, and one key across both would
/// eventually pair the same two with opposite traffic, repeating a nonce — the
/// one failure ChaCha20-Poly1305 does not survive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Downlink,
    Uplink,
}

impl Direction {
    fn label(self) -> &'static [u8] {
        match self {
            Direction::Downlink => b"ausha downlink",
            Direction::Uplink => b"ausha uplink",
        }
    }
}

/// What the pairing token stretches into. Never used as a key itself — each
/// direction takes its own from this.
pub struct Secret([u8; KEY_LEN]);

impl Secret {
    /// Splitting is a single hash rather than a full HKDF because the secret is
    /// already a uniformly random 256 bits; the label is only there so the two
    /// directions cannot land on the same key.
    pub fn key(&self, direction: Direction) -> Key {
        let mut hash = Sha256::new();
        hash.update(self.0);
        hash.update(direction.label());
        Key(hash.finalize().into())
    }
}

/// Stretches the pairing token into a session secret. The salt is fresh per
/// run, so a token reused across runs never produces the same keys twice.
pub fn derive_secret(token: &str, salt_hex: &str) -> Result<Secret, Error> {
    let salt = decode_hex(salt_hex).ok_or(Error::BadSalt)?;
    if salt.len() != SALT_LEN {
        return Err(Error::BadSalt);
    }
    let mut secret = [0u8; KEY_LEN];
    pbkdf2::pbkdf2_hmac::<Sha256>(
        normalize_token(token).as_bytes(),
        &salt,
        KDF_ROUNDS,
        &mut secret,
    );
    Ok(Secret(secret))
}

/// Encrypts outgoing packets. Lives on the sender's fan-out thread.
pub struct Sealer {
    cipher: ChaCha20Poly1305,
    sequence: SequenceExtender,
}

impl Sealer {
    pub fn new(key: &Key) -> Self {
        Self {
            cipher: cipher(key),
            sequence: SequenceExtender::default(),
        }
    }

    /// Returns the packet with its payload encrypted, or `None` if it is not a
    /// packet we produced and so has no header to authenticate.
    pub fn seal(&mut self, datagram: &[u8]) -> Option<Vec<u8>> {
        let (header, body) = split_header(datagram)?;
        let extended = self.sequence.extend(sequence_of(header));
        let sealed = self
            .cipher
            .encrypt(
                &nonce(ssrc_of(header), extended),
                Payload {
                    msg: body,
                    aad: header,
                },
            )
            .ok()?;

        let mut out = Vec::with_capacity(header.len() + sealed.len());
        out.extend_from_slice(header);
        out.extend_from_slice(&sealed);
        Some(out)
    }
}

/// Decrypts incoming packets. Lives wherever datagrams are read.
pub struct Opener {
    cipher: ChaCha20Poly1305,
    sequence: SequenceExtender,
}

impl Opener {
    pub fn new(key: &Key) -> Self {
        Self {
            cipher: cipher(key),
            sequence: SequenceExtender::default(),
        }
    }

    /// Returns the plaintext packet, or `None` if it does not authenticate.
    ///
    /// A packet that fails is dropped rather than reported: to the jitter
    /// buffer a forged or corrupted packet is indistinguishable from one that
    /// never arrived, and it is already equipped to conceal that.
    pub fn open(&mut self, datagram: &[u8]) -> Option<Vec<u8>> {
        let (header, body) = split_header(datagram)?;
        // The rollover counter only advances once a packet authenticates, so a
        // forged sequence number cannot walk the nonce away from the sender's.
        let sequence = sequence_of(header);
        let extended = self.sequence.peek(sequence);
        let opened = self
            .cipher
            .decrypt(
                &nonce(ssrc_of(header), extended),
                Payload {
                    msg: body,
                    aad: header,
                },
            )
            .ok()?;
        self.sequence.commit(sequence);

        let mut out = Vec::with_capacity(header.len() + opened.len());
        out.extend_from_slice(header);
        out.extend_from_slice(&opened);
        Some(out)
    }
}

fn cipher(key: &Key) -> ChaCha20Poly1305 {
    ChaCha20Poly1305::new((&key.0).into())
}

/// Everything past the fixed header is encrypted, CSRC and extension words
/// included, so the receiver reassembles a whole packet before parsing it.
fn split_header(datagram: &[u8]) -> Option<(&[u8], &[u8])> {
    (datagram.len() >= rtp::HEADER_LEN).then(|| datagram.split_at(rtp::HEADER_LEN))
}

fn sequence_of(header: &[u8]) -> u16 {
    u16::from_be_bytes([header[2], header[3]])
}

fn ssrc_of(header: &[u8]) -> u32 {
    u32::from_be_bytes([header[8], header[9], header[10], header[11]])
}

/// SSRC and the extended sequence number, which together never repeat within a
/// session — the property ChaCha20-Poly1305 needs from a nonce.
fn nonce(ssrc: u32, extended_sequence: u64) -> Nonce {
    let mut bytes = [0u8; 12];
    bytes[..4].copy_from_slice(&ssrc.to_be_bytes());
    bytes[4..].copy_from_slice(&extended_sequence.to_be_bytes());
    *Nonce::from_slice(&bytes)
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    text.as_bytes()
        .chunks(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SALT: &str = "000102030405060708090a0b0c0d0e0f";
    const TOKEN: &str = "c838-87a9-3b03";

    fn packet(sequence: u16, payload: &[u8]) -> Vec<u8> {
        let mut out = vec![0x80, 96];
        out.extend_from_slice(&sequence.to_be_bytes());
        out.extend_from_slice(&(u32::from(sequence) * 960).to_be_bytes());
        out.extend_from_slice(&0x1234_5678u32.to_be_bytes());
        out.extend_from_slice(payload);
        out
    }

    fn key(token: &str, salt: &str) -> Key {
        derive_secret(token, salt).unwrap().key(Direction::Downlink)
    }

    fn pair() -> (Sealer, Opener) {
        let key = key(TOKEN, SALT);
        (Sealer::new(&key), Opener::new(&key))
    }

    #[test]
    fn round_trips_a_packet() {
        let (mut sealer, mut opener) = pair();
        let plain = packet(7, b"opus frame");
        let sealed = sealer.seal(&plain).unwrap();

        assert_eq!(&sealed[..rtp::HEADER_LEN], &plain[..rtp::HEADER_LEN]);
        assert_eq!(sealed.len(), plain.len() + TAG_LEN);
        assert!(!sealed.ends_with(b"opus frame"));
        assert_eq!(opener.open(&sealed).unwrap(), plain);
    }

    #[test]
    fn the_token_has_to_match() {
        let (mut sealer, _) = pair();
        let sealed = sealer.seal(&packet(1, b"audio")).unwrap();
        let wrong = key("c838-87a9-3b04", SALT);
        assert!(Opener::new(&wrong).open(&sealed).is_none());
    }

    #[test]
    fn the_salt_makes_every_run_distinct() {
        let sealed = Sealer::new(&key(TOKEN, SALT))
            .seal(&packet(1, b"audio"))
            .unwrap();
        let other = key(TOKEN, "0f0e0d0c0b0a09080706050403020100");
        assert!(Opener::new(&other).open(&sealed).is_none());
    }

    #[test]
    fn rejects_a_tampered_payload_and_a_tampered_header() {
        let (mut sealer, mut opener) = pair();
        let sealed = sealer.seal(&packet(3, b"audio")).unwrap();

        let mut payload = sealed.clone();
        let last = payload.len() - 1;
        payload[last] ^= 1;
        assert!(opener.open(&payload).is_none());

        let mut header = sealed.clone();
        header[3] ^= 1;
        assert!(opener.open(&header).is_none(), "sequence is authenticated");
    }

    #[test]
    fn survives_loss_and_reordering() {
        let (mut sealer, mut opener) = pair();
        let sealed: Vec<_> = (0..8u16)
            .map(|n| sealer.seal(&packet(n, b"audio")).unwrap())
            .collect();

        assert!(opener.open(&sealed[0]).is_some());
        assert!(opener.open(&sealed[3]).is_some(), "after a loss burst");
        assert!(opener.open(&sealed[2]).is_some(), "late packet");
        assert!(opener.open(&sealed[7]).is_some());
    }

    /// Sequence numbers wrap every 22 minutes at 50 packets per second, and a
    /// nonce that wrapped with them would repeat.
    #[test]
    fn keeps_the_nonce_unique_across_wraparound() {
        let (mut sealer, mut opener) = pair();
        for sequence in [65534u16, 65535, 0, 1] {
            let sealed = sealer.seal(&packet(sequence, b"audio")).unwrap();
            assert!(opener.open(&sealed).is_some(), "sequence {sequence}");
        }
        assert_ne!(nonce(1, 65535), nonce(1, 65536));
    }

    /// A forged packet must not move the rollover counter, or every genuine
    /// packet after it would be decrypted against the wrong nonce.
    #[test]
    fn a_rejected_packet_leaves_the_counter_alone() {
        let (mut sealer, mut opener) = pair();
        let first = sealer.seal(&packet(65000, b"audio")).unwrap();
        assert!(opener.open(&first).is_some());

        let mut forged = first.clone();
        forged[2] = 0;
        forged[3] = 5;
        assert!(opener.open(&forged).is_none());

        let next = sealer.seal(&packet(65001, b"audio")).unwrap();
        assert!(opener.open(&next).is_some());
    }

    /// The whole reason the directions are keyed apart: a downlink and an
    /// uplink packet can carry the same SSRC and sequence, and under one key
    /// that is a repeated nonce.
    #[test]
    fn the_two_directions_do_not_share_a_key() {
        let secret = derive_secret(TOKEN, SALT).unwrap();
        let plain = packet(1, b"audio");
        let sealed = Sealer::new(&secret.key(Direction::Downlink))
            .seal(&plain)
            .unwrap();

        assert!(
            Opener::new(&secret.key(Direction::Uplink))
                .open(&sealed)
                .is_none(),
            "the uplink key must not open a downlink packet"
        );
        assert!(
            Opener::new(&secret.key(Direction::Downlink))
                .open(&sealed)
                .is_some()
        );
    }

    #[test]
    fn rejects_a_short_datagram_and_a_bad_salt() {
        let (mut sealer, mut opener) = pair();
        assert!(sealer.seal(&[0x80, 96, 0, 1]).is_none());
        assert!(opener.open(&[0x80, 96, 0, 1]).is_none());
        assert!(matches!(derive_secret(TOKEN, "abc"), Err(Error::BadSalt)));
        assert!(matches!(derive_secret(TOKEN, "zz"), Err(Error::BadSalt)));
    }
}
