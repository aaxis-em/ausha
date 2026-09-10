//! The virtual microphone: a PulseAudio source backed by a FIFO.
//!
//! Call applications choose their input from the system's list of capture
//! devices, so the sender has to be one. `module-pipe-source` gives us that for
//! the cost of a named pipe — no root, no kernel module, and nothing for the
//! user to install.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

/// Module ids this process has loaded and not yet unloaded.
///
/// A PulseAudio module outlives the process that asked for it, so `Drop` alone
/// is not enough: the sender is normally stopped with a signal, which unwinds
/// nothing, and a phantom microphone would be left in everyone's input list.
/// [`unload_all`] is the backstop for that path.
static LOADED: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Unloads anything still loaded, for a shutdown that will not run destructors.
pub fn unload_all() {
    let modules = std::mem::take(&mut *LOADED.lock().unwrap_or_else(|e| e.into_inner()));
    for module in modules {
        unload_module(&module);
    }
}

pub struct PipeSource {
    module: String,
    path: PathBuf,
    fifo: File,
    bytes: Vec<u8>,
    dropped: u64,
}

impl PipeSource {
    pub fn open(name: &str, rate: u32, channels: u8) -> io::Result<Self> {
        let path = runtime_dir().join(format!("{name}.mic"));
        // A previous run that died without unloading leaves both the module and
        // the pipe behind, and module-pipe-source will not reuse either.
        unload_stale(&path);
        let _ = std::fs::remove_file(&path);

        let module = load_module(name, &path, rate, channels)?;
        LOADED
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(module.clone());
        match open_fifo(&path) {
            Ok(fifo) => Ok(Self {
                module,
                path,
                fifo,
                bytes: Vec::new(),
                dropped: 0,
            }),
            Err(e) => {
                unload_module(&module);
                Err(e)
            }
        }
    }

    /// Writes one frame as signed 16-bit little-endian.
    ///
    /// The write is non-blocking, and a frame that will not fit is dropped: the
    /// pipe backs up when nothing is recording from the source, and blocking
    /// there would stall the receive loop for as long as nobody was listening.
    /// A frame is well under `PIPE_BUF`, so the write is all-or-nothing and a
    /// drop cannot leave a half-sample behind to shift every sample after it.
    pub fn write(&mut self, samples: &[f32]) -> io::Result<()> {
        self.bytes.clear();
        self.bytes.reserve(samples.len() * 2);
        for sample in samples {
            let scaled = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            self.bytes.extend_from_slice(&scaled.to_le_bytes());
        }
        match self.fifo.write(&self.bytes) {
            Ok(n) if n == self.bytes.len() => Ok(()),
            Ok(_) => Err(io::Error::other("short write to the microphone pipe")),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                self.dropped += 1;
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    /// Frames discarded because nothing was recording from the source.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }
}

impl Drop for PipeSource {
    fn drop(&mut self) {
        LOADED
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|loaded| *loaded != self.module);
        unload_module(&self.module);
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Unloads a pipe source left over by a run that was killed rather than
/// stopped, matching on the pipe it owns so that only ours is touched.
fn unload_stale(path: &Path) {
    let Ok(output) = Command::new("pactl").args(["list", "modules"]).output() else {
        return;
    };
    let listing = String::from_utf8_lossy(&output.stdout);
    let wanted = format!("file={}", path.display());

    let mut module = None;
    for line in listing.lines().map(str::trim) {
        if let Some(index) = line.strip_prefix("Module #") {
            module = Some(index.to_string());
        }
        if line.starts_with("Argument:")
            && line.contains(&wanted)
            && let Some(stale) = module.take()
        {
            eprintln!("uplink: removing a microphone left by an earlier run");
            unload_module(&stale);
        }
    }
}

fn load_module(name: &str, path: &Path, rate: u32, channels: u8) -> io::Result<String> {
    let output = Command::new("pactl")
        .arg("load-module")
        .arg("module-pipe-source")
        .arg(format!("source_name={name}"))
        .arg(format!("file={}", path.display()))
        .arg("format=s16le")
        .arg(format!("rate={rate}"))
        .arg(format!("channels={channels}"))
        .output()
        .map_err(|e| io::Error::other(format!("could not run pactl: {e}")))?;

    if !output.status.success() {
        return Err(io::Error::other(format!(
            "pactl could not create the microphone source: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let module = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if module.is_empty() {
        return Err(io::Error::other("pactl returned no module id"));
    }
    Ok(module)
}

/// Unloads by the id `load-module` returned rather than by name, which would
/// tear down a pipe source somebody else created.
fn unload_module(module: &str) {
    let _ = Command::new("pactl")
        .args(["unload-module", module])
        .status();
}

/// Opened read-write so that `open` does not block waiting for a reader, and
/// non-blocking so that `write` does not block waiting for one either.
fn open_fifo(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
}

fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}
