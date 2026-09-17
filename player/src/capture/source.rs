//! Locates the platform's desktop-audio loopback device.

use std::io;
#[cfg(target_os = "linux")]
use std::process::Command;

pub struct Input {
    pub format: &'static str,
    pub device: String,
}

/// The ffmpeg input format this platform captures through.
#[cfg(target_os = "linux")]
const FORMAT: &str = "pulse";
#[cfg(target_os = "windows")]
const FORMAT: &str = "dshow";

/// Detects the loopback device unless the user named one.
pub fn resolve(device: Option<String>) -> io::Result<Input> {
    match device {
        Some(device) => Ok(named(device)),
        None => detect(),
    }
}

/// Takes the user at their word when they name a device with `--capture`,
/// since only they know what an unusual sound setup calls it.
fn named(device: String) -> Input {
    Input {
        format: FORMAT,
        #[cfg(target_os = "windows")]
        device: qualify(&device),
        #[cfg(not(target_os = "windows"))]
        device,
    }
}

#[cfg(target_os = "linux")]
pub fn detect() -> io::Result<Input> {
    let device = default_sink()
        .map(|sink| format!("{sink}.monitor"))
        .or_else(first_monitor_source)
        .ok_or_else(|| {
            io::Error::other("no PulseAudio monitor source found; is PulseAudio running?")
        })?;
    Ok(Input {
        format: FORMAT,
        device,
    })
}

#[cfg(target_os = "linux")]
fn default_sink() -> Option<String> {
    let output = Command::new("pactl")
        .args(["get-default-sink"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!name.is_empty()).then_some(name)
}

#[cfg(target_os = "linux")]
fn first_monitor_source() -> Option<String> {
    let output = Command::new("pactl")
        .args(["list", "sources", "short"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split_whitespace().nth(1))
        .find(|name| name.ends_with(".monitor"))
        .map(str::to_string)
}

#[cfg(target_os = "windows")]
pub fn detect() -> io::Result<Input> {
    Ok(Input {
        format: FORMAT,
        device: qualify(&crate::capture::dshow::detect()?),
    })
}

/// dshow takes `audio=<name>`, and a name that already says so is passed
/// through so `--capture "video=..."` stays possible.
#[cfg(target_os = "windows")]
fn qualify(device: &str) -> String {
    match device.contains('=') {
        true => device.to_string(),
        false => format!("audio={device}"),
    }
}
