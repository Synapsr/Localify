//! Native process identity: who a process is, which session and process group
//! it belongs to, and when it started, read from the kernel without spawning
//! `ps`, `pgrep` or `kill`.
//!
//! Start times are in one boot-scoped monotonic time domain that a change of
//! the wall clock cannot move: `mach_absolute_time` units on macOS, clock ticks
//! since boot on Linux. They are only compared with each other and with
//! [`monotonic_now`], never with the wall clock, and only within one boot
//! ([`boot_id`]).

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "linux")]
use linux as native;
#[cfg(target_os = "macos")]
use macos as native;

use std::collections::HashMap;

/// A living process, as the kernel describes it. Zombies are left out: they
/// have ended and only wait for their parent to read their exit status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessInfo {
    pub pid: u32,
    pub ppid: u32,
    pub pgid: u32,
    /// `None` when the kernel would not say.
    pub sid: Option<u32>,
    /// Boot-monotonic start time, comparable with [`monotonic_now`].
    pub start: u64,
    /// Unix seconds. For display, and for records from before run ids.
    pub started_wall: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    Hangup,
    Terminate,
    Kill,
}

impl Signal {
    fn raw(self) -> libc::c_int {
        match self {
            Self::Hangup => libc::SIGHUP,
            Self::Terminate => libc::SIGTERM,
            Self::Kill => libc::SIGKILL,
        }
    }
}

/// The process with this PID, or `None` once it has ended.
pub fn process_info(pid: u32) -> Option<ProcessInfo> {
    native::process_info(pid)
}

/// Every living process this user may inspect.
pub fn all_processes() -> Vec<ProcessInfo> {
    native::pids()
        .into_iter()
        .filter_map(native::process_info)
        .collect()
}

/// Now, in the time domain of [`ProcessInfo::start`].
pub fn monotonic_now() -> u64 {
    native::monotonic_now()
}

/// This boot's identifier. A process recorded under another boot has ended.
pub fn boot_id() -> Option<String> {
    native::boot_id()
}

/// Whether a process with this PID exists, whoever owns it: a daemon started
/// as root by an older install still counts. Says nothing about which process
/// it is; [`process_info`] does.
pub fn process_exists(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal 0 only checks that the PID exists; kill touches no memory.
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Sends a signal to one process. PID 0 and PIDs past `i32::MAX` are refused,
/// since `kill` reads them as process groups. Returns whether it was delivered.
pub fn send_signal(pid: u32, signal: Signal) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        return false;
    }
    // SAFETY: kill takes two integers and touches no memory; the PID is positive,
    // so it names one process, never a group.
    unsafe { libc::kill(pid, signal.raw()) == 0 }
}

/// Signals a process and every process it started (`pnpm` → `next` → workers).
pub fn signal_process_tree(root_pid: u32, signal: Signal) {
    signal_processes(&process_tree(root_pid), signal);
}

/// A process and all its descendants, parents first.
pub fn process_tree(root_pid: u32) -> Vec<u32> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for process in all_processes() {
        children.entry(process.ppid).or_default().push(process.pid);
    }
    let mut pids = vec![root_pid];
    let mut index = 0;
    while index < pids.len() {
        if let Some(found) = children.get(&pids[index]) {
            pids.extend(found.iter().filter(|pid| **pid != root_pid));
        }
        index += 1;
    }
    pids
}

pub fn signal_processes(pids: &[u32], signal: Signal) {
    for pid in pids {
        send_signal(*pid, signal);
    }
}

/// The processes a run's finisher looks at and signals. The native table is
/// the kernel's; tests give their own to replay PID and session reuse.
pub trait ProcessTable {
    fn processes(&self) -> Vec<ProcessInfo>;
    fn process(&self, pid: u32) -> Option<ProcessInfo>;
    fn signal(&self, pid: u32, signal: Signal);
    fn boot_id(&self) -> Option<String>;
}

/// The kernel's process table.
pub struct NativeProcesses;

impl ProcessTable for NativeProcesses {
    fn processes(&self) -> Vec<ProcessInfo> {
        all_processes()
    }

    fn process(&self, pid: u32) -> Option<ProcessInfo> {
        process_info(pid)
    }

    fn signal(&self, pid: u32, signal: Signal) {
        send_signal(pid, signal);
    }

    fn boot_id(&self) -> Option<String> {
        boot_id()
    }
}

/// Whether this process has root's privileges right now. Reads the kernel's answer:
/// the `UID` environment variable can say anything.
pub fn is_root() -> bool {
    // SAFETY: geteuid takes no arguments, touches no memory and cannot fail.
    unsafe { libc::geteuid() == 0 }
}

#[cfg(test)]
mod tests {
    use std::process::{Command, Stdio};
    use std::thread;
    use std::time::Duration;

    use super::*;

    #[test]
    fn this_process_is_known_natively() {
        let me = process_info(std::process::id()).unwrap();

        assert_eq!(me.pid, std::process::id());
        assert!(me.start > 0);
        assert!(me.start <= monotonic_now());
        assert!(me.sid.is_some());
        assert!(boot_id().is_some_and(|id| !id.is_empty()));
    }

    #[test]
    fn a_later_process_starts_later_and_a_dead_one_is_gone() {
        let before = monotonic_now();
        let mut child = Command::new("sleep")
            .arg("5")
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let info = process_info(child.id()).unwrap();

        assert!(info.start >= process_info(std::process::id()).unwrap().start);
        assert!(info.start + 1 >= before, "{} < {before}", info.start);
        assert_eq!(info.ppid, std::process::id());
        assert!(
            all_processes()
                .iter()
                .any(|process| process.pid == child.id())
        );
        assert!(process_tree(std::process::id()).contains(&child.id()));

        assert!(send_signal(child.id(), Signal::Kill));
        child.wait().unwrap();
        thread::sleep(Duration::from_millis(50));
        assert!(process_info(child.id()).is_none());
    }

    #[test]
    fn process_groups_are_never_signalled() {
        assert!(!send_signal(0, Signal::Terminate));
        assert!(!send_signal(u32::MAX, Signal::Terminate));
    }
}
