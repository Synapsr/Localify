/// Platform-agnostic process control operations.
pub trait ProcessControl: Send {
    /// Check whether a process with the given PID exists.
    fn process_exists(&self, pid: u32) -> bool;
}

/// Whether this process has root's privileges right now. Reads the kernel's answer:
/// the `UID` environment variable can say anything.
#[cfg(target_os = "macos")]
pub fn is_root() -> bool {
    // SAFETY: geteuid takes no arguments, touches no memory and cannot fail.
    unsafe { libc::geteuid() == 0 }
}

#[cfg(unix)]
mod unix;

/// Concrete process control type for the current platform.
#[cfg(unix)]
pub type PlatformProcessControl = unix::UnixProcessControl;

/// Concrete process control type for the current platform.
#[cfg(not(unix))]
pub type PlatformProcessControl = UnsupportedProcessControl;

/// Get the process control provider for the current platform.
#[cfg(unix)]
pub fn get_process_control() -> PlatformProcessControl {
    unix::UnixProcessControl
}

/// Get the process control provider for the current platform.
#[cfg(not(unix))]
pub fn get_process_control() -> PlatformProcessControl {
    UnsupportedProcessControl
}

/// Fallback for unsupported platforms.
#[cfg(not(unix))]
struct UnsupportedProcessControl;

#[cfg(not(unix))]
impl ProcessControl for UnsupportedProcessControl {
    fn process_exists(&self, _pid: u32) -> bool {
        false
    }
}
