//! Guarantees ffmpeg dies with the sender.
//!
//! `Encoder`'s `Drop` covers an orderly exit. This covers the rest: a signal
//! the sender cannot handle would otherwise leave ffmpeg running and holding
//! the capture device, which testing showed happens on a plain `kill`.

use std::io;
use std::process::{Child, Command};

/// Whatever the platform needs held open for the guarantee to hold. Dropping
/// it releases the tie, so it lives as long as the encoder.
#[cfg(not(target_os = "windows"))]
pub struct Guard;

/// Asks the kernel to kill ffmpeg when this process dies.
#[cfg(target_os = "linux")]
pub fn before_spawn(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        command.pre_exec(
            || match libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) {
                -1 => Err(io::Error::last_os_error()),
                _ => Ok(()),
            },
        );
    }
}

#[cfg(not(target_os = "linux"))]
pub fn before_spawn(_command: &mut Command) {}

#[cfg(not(target_os = "windows"))]
pub fn after_spawn(_child: &Child) -> Guard {
    Guard
}

/// Windows has no `PR_SET_PDEATHSIG`. A job object with
/// `KILL_ON_JOB_CLOSE` is the equivalent: the kernel kills everything in the
/// job when the last handle to it closes, which includes the sender dying.
#[cfg(target_os = "windows")]
pub struct Guard(Option<windows_sys::Win32::Foundation::HANDLE>);

#[cfg(target_os = "windows")]
pub fn after_spawn(child: &Child) -> Guard {
    match tie_to_job(child) {
        Ok(job) => Guard(Some(job)),
        Err(e) => {
            eprintln!("capture: warning: ffmpeg may outlive a hard kill ({e})");
            Guard(None)
        }
    }
}

#[cfg(target_os = "windows")]
fn tie_to_job(child: &Child) -> io::Result<windows_sys::Win32::Foundation::HANDLE> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };

    // Safety: every call is a plain Win32 call whose failure is reported
    // through the return value, and the job handle is closed in Drop.
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return Err(io::Error::last_os_error());
        }
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let set = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            std::ptr::addr_of!(limits).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        let assigned = set != 0 && AssignProcessToJobObject(job, child.as_raw_handle()) != 0;
        if !assigned {
            let e = io::Error::last_os_error();
            close(job);
            return Err(e);
        }
        Ok(job)
    }
}

#[cfg(target_os = "windows")]
impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(job) = self.0.take() {
            // Safety: the handle came from CreateJobObjectW and is closed once.
            unsafe { close(job) };
        }
    }
}

#[cfg(target_os = "windows")]
unsafe fn close(job: windows_sys::Win32::Foundation::HANDLE) {
    unsafe { windows_sys::Win32::Foundation::CloseHandle(job) };
}
