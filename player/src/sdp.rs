//! Generates a session description so `ffplay` can act as a receiver while the
//! real mobile client does not exist yet.

use std::net::SocketAddr;

use ausha_core::protocol::StreamParams;

pub fn describe(target: SocketAddr, stream: &StreamParams, bitrate_kbps: u32) -> String {
    let family = if target.is_ipv4() { "IP4" } else { "IP6" };
    let StreamParams {
        rate,
        channels,
        payload_type,
        ..
    } = stream;

    format!(
        "v=0\n\
         o=- 0 0 IN {family} {ip}\n\
         s=Ausha\n\
         c=IN {family} {ip}\n\
         t=0 0\n\
         m=audio {port} RTP/AVP {payload_type}\n\
         b=AS:{bitrate_kbps}\n\
         a=rtpmap:{payload_type} opus/{rate}/{channels}\n\
         a=fmtp:{payload_type} sprop-stereo=1\n",
        ip = target.ip(),
        port = target.port(),
    )
}
