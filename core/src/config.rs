//! Stream and protocol constants shared by the control channel and the media path.

use std::time::Duration;

pub const PROTOCOL_VERSION: u16 = 1;

pub const DEFAULT_CONTROL_PORT: u16 = 6996;
pub const DEFAULT_MEDIA_PORT: u16 = 6997;
pub const DEFAULT_BITRATE_KBPS: u32 = 128;

pub const SAMPLE_RATE: u32 = 48000;
pub const CHANNELS: u8 = 2;
pub const FRAME_MS: u32 = 20;
pub const RTP_PAYLOAD_TYPE: u8 = 96;
pub const EXPECTED_LOSS_PERCENT: u32 = 5;

pub const PING_INTERVAL: Duration = Duration::from_secs(2);
pub const SESSION_TIMEOUT: Duration = Duration::from_secs(10);
pub const PUNCH_TIMEOUT: Duration = Duration::from_secs(10);

pub const MAX_DATAGRAM: usize = 2048;
pub const PUNCH_PREFIX: &str = "AUSHA/1 ";

/// Four frames. The buffer sits at this depth while playing, so it rides out
/// three consecutive losses; at 10% independent loss a longer run is rare.
pub const JITTER_MIN_MS: u32 = 80;
pub const JITTER_MAX_MS: u32 = 200;
