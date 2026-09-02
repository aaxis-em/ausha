//! Announces the sender over mDNS so receivers can find it without being told
//! an address.
//!
//! Discovery is a convenience, never the only way in: mDNS is blocked on many
//! consumer routers, on enterprise WiFi with client isolation, and across
//! VLANs, so the pairing link and manual entry always remain.

use std::collections::HashMap;
use std::io;
use std::net::IpAddr;

use mdns_sd::{ServiceDaemon, ServiceInfo};

pub const SERVICE_TYPE: &str = "_ausha._tcp.local.";

/// Holds the registration for as long as the sender runs.
pub struct Advertisement {
    daemon: ServiceDaemon,
    full_name: String,
}

impl Advertisement {
    pub fn publish(instance: &str, control_port: u16, bitrate_kbps: u32) -> io::Result<Self> {
        let daemon = ServiceDaemon::new().map_err(io::Error::other)?;
        let host = hostname();

        let properties: HashMap<String, String> = [
            ("ver".to_string(), "1".to_string()),
            ("codec".to_string(), "opus".to_string()),
            ("rate".to_string(), "48000".to_string()),
            ("channels".to_string(), "2".to_string()),
            ("bitrate".to_string(), bitrate_kbps.to_string()),
        ]
        .into_iter()
        .collect();

        // The address has to be the one a phone can dial. Left to enumerate
        // interfaces itself the daemon advertises loopback, which resolves
        // fine from this machine and is unreachable from anywhere else.
        let address = best_local_address()
            .ok_or_else(|| io::Error::other("no routable local address to advertise"))?;

        let service = ServiceInfo::new(
            SERVICE_TYPE,
            instance,
            &format!("{host}.local."),
            address.to_string(),
            control_port,
            properties,
        )
        .map_err(io::Error::other)?;

        let full_name = service.get_fullname().to_string();
        daemon.register(service).map_err(io::Error::other)?;
        println!("discovery: advertising {instance} at {address} on {SERVICE_TYPE}");
        Ok(Self { daemon, full_name })
    }
}

impl Drop for Advertisement {
    fn drop(&mut self) {
        let _ = self.daemon.unregister(&self.full_name);
        let _ = self.daemon.shutdown();
    }
}

fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .map(|name| name.trim().to_string())
        .ok()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "ausha".to_string())
}

/// The best guess at the address a receiver should dial, used for the pairing
/// link. Loopback and container bridges are skipped.
pub fn best_local_address() -> Option<IpAddr> {
    let output = std::process::Command::new("ip")
        .args(["-4", "route", "get", "1.1.1.1"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let source = text.split_whitespace().skip_while(|w| *w != "src").nth(1)?;
    source.parse().ok()
}
