//! Spawns ffmpeg to encode desktop audio as Opus and emit it as an RTP stream.

pub mod source;

use std::io;
use std::net::SocketAddr;
use std::process::ExitStatus;
use std::process::{Child, Command, Stdio};

use ausha_core::config;

pub struct Settings {
    pub bitrate_kbps: u32,
    pub ssrc: u32,
    pub ingest: SocketAddr,
    pub rtcp_port: u16,
    pub compat_ingest: Option<SocketAddr>,
}

/// Owns the ffmpeg child so that it is killed whenever the sender stops,
/// rather than being orphaned and left holding the capture device.
pub struct Encoder(Child);

impl Encoder {
    pub fn exit_status(&mut self) -> io::Result<Option<ExitStatus>> {
        self.0.try_wait()
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub fn spawn(settings: &Settings) -> io::Result<Encoder> {
    let input = source::detect()?;
    println!("capture: {} source {}", input.format, input.device);
    let mut command = Command::new("ffmpeg");
    command
        .args(build_args(&input, settings))
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    tie_lifetime_to_parent(&mut command);
    command.spawn().map(Encoder)
}

/// Asks the kernel to kill ffmpeg when this process dies, so a signal the
/// sender cannot handle still cannot leave ffmpeg holding the capture device.
#[cfg(target_os = "linux")]
fn tie_lifetime_to_parent(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        command.pre_exec(
            || match libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) {
                -1 => Err(io::Error::last_os_error()),
                _ => Ok(()),
            },
        );
    }
}

#[cfg(not(target_os = "linux"))]
fn tie_lifetime_to_parent(_command: &mut Command) {}

#[rustfmt::skip]
fn build_args(input: &source::Input, settings: &Settings) -> Vec<String> {
    let fragment_size =
        (config::SAMPLE_RATE * config::FRAME_MS / 1000 * u32::from(config::CHANNELS) * 2).to_string();
    let bitrate = format!("{}k", settings.bitrate_kbps);
    let rate = config::SAMPLE_RATE.to_string();
    let channels = config::CHANNELS.to_string();
    let frame_duration = config::FRAME_MS.to_string();
    let packet_loss = config::EXPECTED_LOSS_PERCENT.to_string();
    let payload_type = config::RTP_PAYLOAD_TYPE.to_string();
    let ssrc = settings.ssrc.to_string();
    let destination = format!(
        "rtp://{}:{}?rtcpport={}",
        settings.ingest.ip(),
        settings.ingest.port(),
        settings.rtcp_port
    );

    let mut args: Vec<String> = [
        "-hide_banner",
        "-loglevel", "warning",
        "-fflags", "nobuffer",
        "-f", input.format,
        "-fragment_size", &fragment_size,
        "-i", &input.device,
        "-c:a", "libopus",
        "-b:a", &bitrate,
        "-ar", &rate,
        "-ac", &channels,
        "-application", "audio",
        "-frame_duration", &frame_duration,
        "-packet_loss", &packet_loss,
        "-fec:a", "1",
        "-payload_type", &payload_type,
        "-ssrc", &ssrc,
        "-muxdelay", "0",
        "-muxpreload", "0",
        "-map", "0:a",
        "-f", "rtp", &destination,
    ]
    .iter()
    .map(|arg| arg.to_string())
    .collect();

    if let Some(compat) = settings.compat_ingest {
        args.extend(compat_output(settings.bitrate_kbps, compat));
    }
    args
}

/// A second MPEG-TS output for players that cannot be handed an SDP, such as
/// mpv on Android. 1316 bytes is seven 188-byte TS packets, so a lost datagram
/// costs whole packets instead of straddling two.
#[rustfmt::skip]
fn compat_output(bitrate_kbps: u32, target: SocketAddr) -> Vec<String> {
    let bitrate = format!("{bitrate_kbps}k");
    let url = format!("udp://{target}?pkt_size=1316");
    [
        "-map", "0:a",
        "-c:a", "libopus",
        "-b:a", &bitrate,
        "-f", "mpegts",
        &url,
    ]
    .iter()
    .map(|arg| arg.to_string())
    .collect()
}
