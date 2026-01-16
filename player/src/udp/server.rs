use std::io::Read;
use std::net::UdpSocket;

pub fn stream_ffmpeg_to_udp<R: Read>(mut ffmpeg_stdout: R, udp_dest: &str) {
    let socket = UdpSocket::bind("0.0.0.0:0").expect("Failed to bind UDP socket");

    println!("Streaming FFmpeg output to UDP {}", udp_dest);

    let mut buf = [0u8; 1400];

    loop {
        match ffmpeg_stdout.read(&mut buf) {
            Ok(0) => break, // EOF
            Ok(n) => {
                if let Err(e) = socket.send_to(&buf[..n], udp_dest) {
                    eprintln!("Failed to send UDP packet: {}", e);
                }
            }
            Err(e) => {
                eprintln!("Error reading ffmpeg stdout: {}", e);
                break;
            }
        }
    }

    println!("Finished streaming FFmpeg output");
}
