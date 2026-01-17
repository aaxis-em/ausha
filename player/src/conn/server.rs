use std::io::Read;
use std::net::{SocketAddr, TcpListener, UdpSocket};
use std::sync::{Arc, Mutex};

pub fn stream_ffmpeg_to_udp<R: Read>(mut ffmpeg_stdout: R, clients: Arc<Mutex<Vec<SocketAddr>>>) {
    let socket = UdpSocket::bind("0.0.0.0:0").expect("Failed to bind UDP socket");

    let mut buf = [0u8; 1400];

    loop {
        match ffmpeg_stdout.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let clients = clients.lock().unwrap();
                for client in clients.iter() {
                    let _ = socket.send_to(&buf[..n], client);
                }
            }
            Err(e) => {
                eprintln!("FFmpeg read error: {}", e);
                break;
            }
        }
    }
}
pub fn create_connection_tcp(clients: Arc<Mutex<Vec<SocketAddr>>>) {
    let listener = TcpListener::bind("0.0.0.0:6996").expect("Failed to bind TCP listener");

    println!("TCP control server on port 6996");

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let addr = stream.peer_addr().unwrap();
                println!("Client joined from {}", addr);

                let udp_addr = SocketAddr::new(addr.ip(), 1234);

                clients.lock().unwrap().push(udp_addr);
            }
            Err(e) => eprintln!("TCP error: {}", e),
        }
    }
}
