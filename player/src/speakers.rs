//! Silences this machine's own speakers while any receiver is listening.

use std::io;
use std::process::Command;
use std::sync::{Mutex, MutexGuard};

/// The sink this process muted and has not yet unmuted.
///
/// Sessions hold the mute from their own threads, and a signal stops the
/// sender without unwinding them, so [`restore`] is the backstop that keeps a
/// stopped sender from leaving the desktop silent.
static MUTED: Mutex<Option<String>> = Mutex::new(None);

/// Unmutes whatever this process muted, for a shutdown that will not run
/// destructors.
pub fn restore() {
    if let Some(sink) = lock(&MUTED).take() {
        match set_mute(&sink, false) {
            Ok(()) => println!("speakers: {sink} unmuted"),
            Err(e) => eprintln!("speakers: could not unmute {sink}: {e}"),
        }
    }
}

pub struct Speakers {
    sink: String,
    listeners: Mutex<usize>,
}

impl Speakers {
    /// The speakers behind a captured device. Only a sink's monitor has any:
    /// capturing a microphone leaves nothing playing here to silence.
    pub fn behind(capture_device: &str) -> Option<Self> {
        if !cfg!(target_os = "linux") {
            return None;
        }
        let sink = capture_device.strip_suffix(".monitor")?;
        Some(Self {
            sink: sink.to_string(),
            listeners: Mutex::new(0),
        })
    }

    /// Mutes the sink when the first receiver arrives. The mute lasts until the
    /// last returned guard is dropped.
    pub fn listen(&self) -> Listening<'_> {
        let mut listeners = lock(&self.listeners);
        *listeners += 1;
        if *listeners == 1 {
            self.mute();
        }
        Listening(self)
    }

    /// A sink the user had already muted is left for them to unmute, rather
    /// than turned back on when the last receiver leaves.
    fn mute(&self) {
        let result = is_muted(&self.sink).and_then(|muted| match muted {
            true => Ok(()),
            false => {
                set_mute(&self.sink, true)?;
                *lock(&MUTED) = Some(self.sink.clone());
                println!("speakers: {} muted while receivers listen", self.sink);
                Ok(())
            }
        });
        if let Err(e) = result {
            eprintln!("speakers: could not mute {}: {e}", self.sink);
        }
    }

    fn leave(&self) {
        let mut listeners = lock(&self.listeners);
        *listeners -= 1;
        if *listeners == 0 {
            restore();
        }
    }
}

pub struct Listening<'a>(&'a Speakers);

impl Drop for Listening<'_> {
    fn drop(&mut self) {
        self.0.leave();
    }
}

fn is_muted(sink: &str) -> io::Result<bool> {
    let output = pactl(&["get-sink-mute", sink])?;
    Ok(output.trim().ends_with("yes"))
}

fn set_mute(sink: &str, muted: bool) -> io::Result<()> {
    pactl(&["set-sink-mute", sink, if muted { "1" } else { "0" }]).map(drop)
}

fn pactl(args: &[&str]) -> io::Result<String> {
    let output = Command::new("pactl")
        .args(args)
        .output()
        .map_err(|e| io::Error::other(format!("could not run pactl: {e}")))?;
    if !output.status.success() {
        return Err(io::Error::other(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Recovers from poisoning: a session thread that panicked mid-update has
/// still left a count and a sink name worth restoring from.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn a_monitor_has_its_sink_behind_it() {
        let speakers = Speakers::behind("alsa_output.pci.analog-stereo.monitor").unwrap();
        assert_eq!(speakers.sink, "alsa_output.pci.analog-stereo");
    }

    #[test]
    fn a_microphone_has_no_speakers_behind_it() {
        assert!(Speakers::behind("alsa_input.pci.analog-stereo").is_none());
    }
}
