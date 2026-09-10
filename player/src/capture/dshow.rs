//! Finding a DirectShow loopback device on Windows.
//!
//! Windows has no equivalent of a PulseAudio monitor source, and ffmpeg has no
//! WASAPI loopback demuxer, so capturing what the desktop is playing means
//! finding a DirectShow device that exposes it: either a virtual loopback
//! filter the user installed, or a sound card that still offers Stereo Mix.

/// Names we recognise as "what the desktop is playing", best first. The first
/// is the device installed by virtual-audio-capturer, which works on any card;
/// the rest are the vendor names for a card's own loopback, which is only
/// present when the driver exposes it and the user enabled it.
const LOOPBACK_NAMES: [&str; 5] = [
    "virtual-audio-capturer",
    "stereo mix",
    "what u hear",
    "wave out mix",
    "loopback",
];

/// Picks the first device whose name matches a known loopback, in preference
/// order rather than in the order Windows happened to enumerate them.
pub fn choose(devices: &[String]) -> Option<String> {
    LOOPBACK_NAMES.iter().find_map(|wanted| {
        devices
            .iter()
            .find(|device| device.to_lowercase().contains(wanted))
            .cloned()
    })
}

/// Reads device names out of what `ffmpeg -list_devices true -f dshow` writes
/// to stderr. Two layouts are in the wild: older builds group devices under a
/// "DirectShow audio devices" heading, newer ones tag each line `(audio)`.
pub fn audio_devices(listing: &str) -> Vec<String> {
    let mut devices = Vec::new();
    let mut in_audio_section = false;

    for line in listing.lines() {
        let line = strip_log_prefix(line);
        if let Some(heading) = line.strip_prefix("DirectShow ") {
            in_audio_section = heading.starts_with("audio");
            continue;
        }
        if line.starts_with("Alternative name") {
            continue;
        }
        let Some(name) = quoted(line) else { continue };
        if in_audio_section || line.ends_with("(audio)") {
            devices.push(name);
        }
    }
    devices
}

fn strip_log_prefix(line: &str) -> &str {
    line.split_once("] ").map_or(line, |(_, rest)| rest).trim()
}

fn quoted(line: &str) -> Option<String> {
    let start = line.find('"')? + 1;
    let end = start + line[start..].find('"')?;
    Some(line[start..end].to_string())
}

#[cfg(windows)]
pub fn detect() -> std::io::Result<String> {
    use std::io;
    use std::process::Command;

    const HINT: &str = "install virtual-audio-capturer, or enable Stereo Mix in \
         Sound settings > Recording, then name the device with --capture";

    // ffmpeg always exits non-zero for this query and writes the list to
    // stderr, so neither the status nor stdout says anything useful.
    let output = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-list_devices",
            "true",
            "-f",
            "dshow",
            "-i",
            "dummy",
        ])
        .output()?;
    let listing = String::from_utf8_lossy(&output.stderr);
    let devices = audio_devices(&listing);

    choose(&devices).ok_or_else(|| {
        io::Error::other(match devices.len() {
            0 => format!("no DirectShow audio devices found; {HINT}"),
            _ => format!(
                "no loopback device among the DirectShow audio devices ({}); {HINT}",
                devices.join(", ")
            ),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NEWER: &str = r#"
[dshow @ 000001] "Integrated Camera" (video)
[dshow @ 000001]   Alternative name "@device_pnp_\\?\usb#vid_04f2"
[dshow @ 000001] "Microphone (Realtek(R) Audio)" (audio)
[dshow @ 000001]   Alternative name "@device_cm_{33D9A762}\wave_{0000}"
[dshow @ 000001] "Stereo Mix (Realtek(R) Audio)" (audio)
[dshow @ 000001]   Alternative name "@device_cm_{33D9A762}\wave_{0001}"
dummy: Immediate exit requested
"#;

    const OLDER: &str = r#"
[dshow @ 000001] DirectShow video devices (some may be both video and audio devices)
[dshow @ 000001]  "Integrated Camera"
[dshow @ 000001] DirectShow audio devices
[dshow @ 000001]  "Microphone (Realtek Audio)"
[dshow @ 000001]  "virtual-audio-capturer"
"#;

    #[test]
    fn reads_the_newer_listing_and_ignores_video() {
        assert_eq!(
            audio_devices(NEWER),
            [
                "Microphone (Realtek(R) Audio)",
                "Stereo Mix (Realtek(R) Audio)"
            ]
        );
    }

    #[test]
    fn reads_the_older_sectioned_listing() {
        assert_eq!(
            audio_devices(OLDER),
            ["Microphone (Realtek Audio)", "virtual-audio-capturer"]
        );
    }

    #[test]
    fn prefers_the_virtual_capturer_over_a_cards_stereo_mix() {
        let devices = vec![
            "Stereo Mix (Realtek(R) Audio)".to_string(),
            "virtual-audio-capturer".to_string(),
        ];
        assert_eq!(choose(&devices).unwrap(), "virtual-audio-capturer");
    }

    #[test]
    fn falls_back_to_stereo_mix_and_reports_nothing_when_there_is_no_loopback() {
        assert_eq!(
            choose(&audio_devices(NEWER)).unwrap(),
            "Stereo Mix (Realtek(R) Audio)"
        );
        assert_eq!(choose(&["Microphone (Realtek Audio)".to_string()]), None);
        assert_eq!(choose(&[]), None);
    }
}
