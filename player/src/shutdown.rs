//! Turns SIGINT and SIGTERM into a flag the main loop can act on.
//!
//! Without this the sender dies where it stands, which is fine for everything
//! it owns inside the process — ffmpeg is reaped by the kernel — but not for
//! the PulseAudio module the uplink loads, which would outlive it as a
//! microphone nobody can remove.

use std::sync::atomic::{AtomicBool, Ordering};

static REQUESTED: AtomicBool = AtomicBool::new(false);

pub fn listen() {
    // Safety: the handler only stores to an atomic, which is async-signal-safe.
    unsafe {
        libc::signal(libc::SIGINT, handle as libc::sighandler_t);
        libc::signal(libc::SIGTERM, handle as libc::sighandler_t);
    }
}

pub fn requested() -> bool {
    REQUESTED.load(Ordering::Relaxed)
}

extern "C" fn handle(_signal: libc::c_int) {
    REQUESTED.store(true, Ordering::Relaxed);
}
