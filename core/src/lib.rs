//! Protocol types and the receive pipeline, shared by the sender, the desktop
//! receiver, and eventually the mobile apps.
//!
//! Everything here is free of sockets, threads, and platform APIs: packets and
//! timestamps come in as arguments, audio goes out into a caller-owned buffer.

pub mod config;
pub mod decode;
pub mod drift;
pub mod jitter;
pub mod lines;
pub mod pipeline;
pub mod protocol;
pub mod resample;
pub mod rtp;
