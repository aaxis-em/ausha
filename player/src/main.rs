mod advertise;
mod capture;
mod cli;
mod control;
mod ids;
mod pairing;
mod registry;
mod relay;
mod sdp;

use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, UdpSocket};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use ausha_core::protocol::StreamParams;
use registry::Registry;

fn main() {
    let cli = match cli::parse() {
        Ok(cli) => cli,
        Err(e) => {
            eprintln!("error: {e}\n\n{}", cli::USAGE);
            std::process::exit(2);
        }
    };

    if let Err(e) = run(cli) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run(cli: cli::Cli) -> io::Result<()> {
    let token = cli.token.clone().unwrap_or_else(ids::random_token);
    let ssrc = ids::random_ssrc();
    let stream = StreamParams::new(ssrc);
    let registry = Arc::new(Registry::new());

    let media = Arc::new(UdpSocket::bind((Ipv4Addr::UNSPECIFIED, cli.media_port))?);
    let (ingest, rtcp) = bind_ingest_pair()?;
    ingest.set_read_timeout(Some(Duration::from_secs(1)))?;

    if let Some(target) = cli.static_client {
        registry.add_static_target(target);
        println!("media: static receiver {target}");
        if let Some(path) = &cli.sdp_out {
            std::fs::write(path, sdp::describe(target, &stream, cli.bitrate_kbps))?;
            println!("media: wrote {} for ffplay", path.display());
        }
    }

    let control = TcpListener::bind((Ipv4Addr::UNSPECIFIED, cli.control_port))?;
    let server = Arc::new(control::ControlServer {
        registry: registry.clone(),
        token: token.clone(),
        media_port: cli.media_port,
        stream: stream.clone(),
    });
    thread::spawn(move || control::serve(control, server));
    thread::spawn({
        let media = media.clone();
        let registry = registry.clone();
        move || relay::listen_for_punch(media, registry)
    });

    let compat_ingest = start_compat(&cli.compat_ts)?;

    let mut ffmpeg = capture::spawn(&capture::Settings {
        bitrate_kbps: cli.bitrate_kbps,
        ssrc,
        ingest: ingest.local_addr()?,
        rtcp_port: rtcp.local_addr()?.port(),
        compat_ingest,
    })?;

    let _advertisement = if cli.no_discovery {
        None
    } else {
        advertise::Advertisement::publish(&cli.name, cli.control_port, cli.bitrate_kbps)
            .map_err(|e| eprintln!("discovery: not advertising ({e})"))
            .ok()
    };

    announce(&cli, &token, ssrc);
    pump(&ingest, &media, &registry, &mut ffmpeg)
}

fn pump(
    ingest: &UdpSocket,
    media: &UdpSocket,
    registry: &Registry,
    encoder: &mut capture::Encoder,
) -> io::Result<()> {
    loop {
        relay::forward(ingest, media, registry)?;
        if let Some(status) = encoder.exit_status()? {
            return Err(io::Error::other(format!("ffmpeg exited: {status}")));
        }
    }
}

/// Spawns the MPEG-TS fan-out and returns the loopback address ffmpeg should
/// mux that stream to, or `None` when no compatibility target was requested.
fn start_compat(targets: &[SocketAddr]) -> io::Result<Option<SocketAddr>> {
    if targets.is_empty() {
        return Ok(None);
    }
    let ingest = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
    ingest.set_read_timeout(Some(Duration::from_secs(1)))?;
    let address = ingest.local_addr()?;
    let sender = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;

    for target in targets {
        println!(
            "compat: MPEG-TS to {target} (mpv udp://0.0.0.0:{})",
            target.port()
        );
    }
    let targets = targets.to_vec();
    thread::spawn(move || relay::forward_compat(ingest, sender, targets));
    Ok(Some(address))
}

/// Binds an even RTP port and the odd RTCP port above it, as RTP convention
/// requires. The RTCP socket is never read; it exists so ffmpeg's reports do
/// not draw ICMP port-unreachable replies.
fn bind_ingest_pair() -> io::Result<(UdpSocket, UdpSocket)> {
    for port in (5004..5040).step_by(2) {
        let Ok(rtp) = UdpSocket::bind((Ipv4Addr::LOCALHOST, port)) else {
            continue;
        };
        if let Ok(rtcp) = UdpSocket::bind((Ipv4Addr::LOCALHOST, port + 1)) {
            return Ok((rtp, rtcp));
        }
    }
    Err(io::Error::other("no free loopback RTP port pair"))
}

fn announce(cli: &cli::Cli, token: &str, ssrc: u32) {
    println!("control: tcp/{}", cli.control_port);
    println!("media:   udp/{} ssrc {ssrc:08x}", cli.media_port);
    println!("pairing: {}", ids::format_token(token));

    let Some(address) = advertise::best_local_address() else {
        return;
    };
    let link = pairing::link(&address.to_string(), cli.control_port, token, &cli.name);
    println!("link:    {link}");
    if cli.no_qr {
        return;
    }
    if let Some(code) = pairing::qr(&link) {
        println!("\nScan this with the Ausha app to connect:\n\n{code}");
    }
}
