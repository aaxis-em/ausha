//! Newline-delimited JSON messages exchanged on the TCP control channel.

use serde::{Deserialize, Serialize};

use crate::{config, crypto};

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ClientMessage {
    Hello {
        ver: u16,
        name: String,
        token: String,
        /// Asks for a microphone channel back to the sender. Absent from an
        /// older client, which is then simply a listener.
        #[serde(default)]
        uplink: bool,
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
        /// Present only when the client asked and the sender was started with
        /// `--uplink`. Its absence leaves the client a listener.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        uplink: Option<UplinkParams>,
        /// Covers the session, not one direction: if the downlink is
        /// encrypted so is the uplink.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        encryption: Option<Encryption>,
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

/// Where the client sends its microphone, and how it should encode it. The
/// stream description is a plain [`StreamParams`] so that the sender can run
/// the same [`Pipeline`](crate::pipeline::Pipeline) on it that the client runs
/// on the downlink.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UplinkParams {
    pub port: u16,
    pub bitrate: u32,
    pub stream: StreamParams,
}

impl UplinkParams {
    pub fn new(port: u16, ssrc: u32) -> Self {
        Self {
            port,
            bitrate: config::UPLINK_BITRATE,
            stream: StreamParams {
                channels: config::UPLINK_CHANNELS,
                payload_type: config::UPLINK_PAYLOAD_TYPE,
                ..StreamParams::new(ssrc)
            },
        }
    }
}

/// How the media path is protected, named by the sender in `accept` so the
/// receiver can derive the same key from the pairing token it already holds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Encryption {
    pub cipher: String,
    /// Hex, fresh per run.
    pub salt: String,
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

impl Encryption {
    /// Derives the session secret this `accept` describes. Each direction takes
    /// its own key from it.
    pub fn secret(&self, token: &str) -> Result<crypto::Secret, crypto::Error> {
        if self.cipher != crypto::CIPHER {
            return Err(crypto::Error::UnknownCipher(self.cipher.clone()));
        }
        crypto::derive_secret(token, &self.salt)
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
