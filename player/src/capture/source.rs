//! Locates the platform's desktop-audio loopback device.

use std::io;
use std::process::Command;

pub struct Input {
    pub format: &'static str,
    pub device: String,
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
        format: "pulse",
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
    Err(io::Error::other(
        "Windows capture is not implemented yet: ffmpeg has no WASAPI loopback demuxer, \
         so this needs a DirectShow loopback device (see arch.md)",
    ))
}
