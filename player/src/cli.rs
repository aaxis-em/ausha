//! Command line arguments.

use std::net::SocketAddr;
use std::path::PathBuf;

use ausha_core::config;

pub struct Cli {
    pub control_port: u16,
    pub media_port: u16,
    pub bitrate_kbps: u32,
    pub token: Option<String>,
    pub static_client: Option<SocketAddr>,
    pub sdp_out: Option<PathBuf>,
    pub compat_ts: Vec<SocketAddr>,
    pub name: String,
    pub no_discovery: bool,
    pub no_qr: bool,
}

impl Default for Cli {
    fn default() -> Self {
        Self {
            control_port: config::DEFAULT_CONTROL_PORT,
            media_port: config::DEFAULT_MEDIA_PORT,
            bitrate_kbps: config::DEFAULT_BITRATE_KBPS,
            token: None,
            static_client: None,
            sdp_out: None,
            compat_ts: Vec::new(),
            name: hostname(),
            no_discovery: false,
            no_qr: false,
        }
    }
}

pub const USAGE: &str = "\
ausha - stream desktop audio to receivers on the local network

Usage: ausha [options]

Options:
  --control-port <port>   TCP control channel port (default 6996)
  --media-port <port>     UDP media port receivers punch and listen on (default 6997)
  --bitrate <kbps>        Opus bitrate (default 128)
  --token <token>         Fixed pairing token instead of a freshly generated one
  --static-client <addr>  Always send media to this ip:port, no handshake required
  --sdp-out <path>        Write an SDP for --static-client, playable with ffplay
  --compat-ts <ip:port>   Also push MPEG-TS to this address for mpv or VLC.
                          Repeatable. No pairing needed, so only use it on a
                          network you trust. Play it with:
                            mpv udp://0.0.0.0:<port>
  --name <name>           Name advertised to receivers (default this host)
  --no-discovery          Do not advertise over mDNS
  --no-qr                 Do not print the pairing QR code
  -h, --help              Show this message
";

pub fn parse() -> Result<Cli, String> {
    let mut cli = Cli::default();
    let mut args = std::env::args().skip(1);

    while let Some(flag) = args.next() {
        let mut value = || {
            args.next()
                .ok_or_else(|| format!("{flag} requires a value"))
        };
        match flag.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            "--control-port" => cli.control_port = parse_with(&flag, &value()?)?,
            "--media-port" => cli.media_port = parse_with(&flag, &value()?)?,
            "--bitrate" => cli.bitrate_kbps = parse_with(&flag, &value()?)?,
            "--token" => cli.token = Some(value()?),
            "--static-client" => cli.static_client = Some(parse_with(&flag, &value()?)?),
            "--sdp-out" => cli.sdp_out = Some(PathBuf::from(value()?)),
            "--compat-ts" => cli.compat_ts.push(parse_with(&flag, &value()?)?),
            "--name" => cli.name = value()?,
            "--no-discovery" => cli.no_discovery = true,
            "--no-qr" => cli.no_qr = true,
            other => return Err(format!("unknown argument: {other}")),
        }
    }

    if cli.sdp_out.is_some() && cli.static_client.is_none() {
        return Err("--sdp-out requires --static-client".to_string());
    }
    Ok(cli)
}

fn parse_with<T: std::str::FromStr>(flag: &str, value: &str) -> Result<T, String> {
    value
        .parse()
        .map_err(|_| format!("{flag}: invalid value {value:?}"))
}

fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .map(|name| name.trim().to_string())
        .ok()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "ausha".to_string())
}
