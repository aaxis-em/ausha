mod cmd;
mod conn;

use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
fn main() {
    let clients = Arc::new(Mutex::new(Vec::new()));

    {
        let clients = clients.clone();
        thread::spawn(|| {
            conn::server::create_connection_tcp(clients);
        });
    }

    let args = cmd::audiocapture::get_audio_capture_command();

    let mut ffmpeg = Command::new("ffmpeg")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("Failed to start ffmpeg");

    let stdout = ffmpeg.stdout.take().unwrap();
    conn::server::stream_ffmpeg_to_udp(stdout, clients);

    let _ = ffmpeg.wait();
}
