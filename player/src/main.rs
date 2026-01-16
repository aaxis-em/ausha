mod cmd;
mod udp;

use std::process::{Command, Stdio};

fn main() {
    let udp_dest = "192.168.0.59:1234";

    let arguments = cmd::audiocapture::get_audio_capture_command();

    let mut ffmpeg = Command::new("ffmpeg")
        .args(arguments)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("Failed to start ffmpeg");

    if let Some(stdout) = ffmpeg.stdout.take() {
        udp::server::stream_ffmpeg_to_udp(stdout, udp_dest);
    }

    ffmpeg.wait().expect("FFmpeg exited unexpectedly");
}
