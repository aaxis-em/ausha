//! Newline-delimited JSON messages exchanged on the TCP control channel.

use serde::{Deserialize, Serialize};

use crate::config;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ClientMessage {
    Hello {
        ver: u16,
        name: String,
        token: String,
    },
    Pong {
        ts: u64,
    },
    Stats {
        loss: f32,
        jitter_ms: u32,
        buffer_ms: u32,
    },
    Bye,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ServerMessage {
    Accept {
        session: String,
        media_port: u16,
        stream: StreamParams,
    },
    Ready,
    Ping {
        ts: u64,
    },
    Error {
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamParams {
    pub codec: String,
    pub rate: u32,
    pub channels: u8,
    pub ptime_ms: u32,
    pub payload_type: u8,
    pub ssrc: u32,
    pub fec: bool,
}

impl StreamParams {
    pub fn new(ssrc: u32) -> Self {
        Self {
            codec: "opus".to_string(),
            rate: config::SAMPLE_RATE,
            channels: config::CHANNELS,
            ptime_ms: config::FRAME_MS,
            payload_type: config::RTP_PAYLOAD_TYPE,
            ssrc,
            fec: true,
        }
    }
}

/// Accepts a pairing token in whatever form it was typed. The sender displays
/// it grouped as `xxxx-xxxx-xxxx` for reading aloud, so the grouped form has to
/// authenticate as readily as the bare one.
pub fn normalize_token(token: &str) -> String {
    token
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_authenticate_however_they_were_typed() {
        let expected = normalize_token("c83887a93b03");
        for typed in [
            "c83887a93b03",
            "c838-87a9-3b03",
            "C838-87A9-3B03",
            " c838 87a9 3b03 ",
        ] {
            assert_eq!(normalize_token(typed), expected, "{typed:?} should match");
        }
        assert_ne!(normalize_token("c83887a93b04"), expected);
    }
}
