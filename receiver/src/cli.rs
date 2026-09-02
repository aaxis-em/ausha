//! Command line arguments.

use ausha_client::{Latency, config};

pub struct Cli {
    pub host: String,
    pub control_port: u16,
    pub token: String,
    pub name: String,
    pub sink: Option<String>,
    pub sink_latency_ms: u32,
    pub run_for_secs: Option<u64>,
    pub simulate_loss: u32,
    pub latency: Latency,
}

pub const USAGE: &str = "\
ausha-recv - play a stream from an Ausha sender

Usage: ausha-recv --host <ip> --token <token> [options]

Options:
  --host <ip>             Sender address (required)
  --token <token>         Pairing token the sender printed (required)
  --control-port <port>   Sender control port (default 6996)
  --name <name>           Name shown on the sender (default this host)
  --sink <program>        pacat, aplay, ffplay, or null (default: first found)
  --sink-latency <ms>     Requested device latency (default 20)
  --run-for <seconds>     Exit after this long, for soak testing
  --latency <preset>      low, balanced or stable (default balanced). Deeper
                          buffering survives worse networks but adds delay
  --simulate-loss <pct>   Drop this percentage of received packets, to exercise
                          concealment against a real sender
  -h, --help              Show this message
";

pub fn parse() -> Result<Cli, String> {
    let mut cli = Cli {
        host: String::new(),
        control_port: config::DEFAULT_CONTROL_PORT,
        token: String::new(),
        name: hostname(),
        sink: None,
        sink_latency_ms: 20,
        run_for_secs: None,
        simulate_loss: 0,
        latency: Latency::Balanced,
    };
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
            "--host" => cli.host = value()?,
            "--token" => cli.token = value()?,
            "--name" => cli.name = value()?,
            "--sink" => cli.sink = Some(value()?),
            "--control-port" => cli.control_port = parse_with(&flag, &value()?)?,
            "--sink-latency" => cli.sink_latency_ms = parse_with(&flag, &value()?)?,
            "--run-for" => cli.run_for_secs = Some(parse_with(&flag, &value()?)?),
            "--simulate-loss" => cli.simulate_loss = parse_with(&flag, &value()?)?,
            "--latency" => {
                let name = value()?;
                cli.latency = Latency::parse(&name).ok_or_else(|| {
                    format!("--latency: expected low, balanced or stable, got {name:?}")
                })?;
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }

    if cli.host.is_empty() {
        return Err("--host is required".to_string());
    }
    if cli.token.is_empty() {
        return Err("--token is required".to_string());
    }
    if cli.simulate_loss > 100 {
        return Err("--simulate-loss must be a percentage".to_string());
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
        .unwrap_or_else(|| "ausha-recv".to_string())
}
