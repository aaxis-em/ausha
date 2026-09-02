//! Random identifiers: session ids, RTP SSRC, and the pairing token.

pub fn random_u64() -> u64 {
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).expect("system randomness unavailable");
    u64::from_le_bytes(bytes)
}

pub fn random_ssrc() -> u32 {
    (random_u64() as u32 & 0x7fff_ffff) | 1
}

pub fn random_token() -> String {
    let value = random_u64() & 0xffff_ffff_ffff;
    format!("{value:012x}")
}

pub fn format_token(token: &str) -> String {
    token
        .as_bytes()
        .chunks(4)
        .map(|chunk| String::from_utf8_lossy(chunk).to_string())
        .collect::<Vec<_>>()
        .join("-")
}

/// Compares in time independent of how many leading characters match.
pub fn tokens_match(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}
