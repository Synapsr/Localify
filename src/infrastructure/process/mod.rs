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
    Interrupt,
    Terminate,
    Kill,
}

impl Signal {
    fn raw(self) -> libc::c_int {
        match self {
            Self::Hangup => libc::SIGHUP,
            Self::Interrupt => libc::SIGINT,
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

/// When this boot started, in Unix seconds on the wall clock.
pub fn boot_time() -> Option<u64> {
    native::boot_time()
}

/// A start time as `ps -o lstart` printed it in the C locale, in local time:
/// `Thu Nov  5 10:00:00 2026`. Records written before run ids saved that.
pub fn lstart_label(unix_seconds: u64) -> Option<String> {
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let time = libc::time_t::try_from(unix_seconds).ok()?;
    // SAFETY: tm holds integers and a zone pointer, for which zero (null) is valid.
    let mut local: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers are valid for the call, and localtime_r writes only
    // into `local`.
    if unsafe { libc::localtime_r(&time, &mut local) }.is_null() {
        return None;
    }
    Some(format!(
        "{} {} {:>2} {:02}:{:02}:{:02} {}",
        DAYS.get(usize::try_from(local.tm_wday).ok()?)?,
        MONTHS.get(usize::try_from(local.tm_mon).ok()?)?,
        local.tm_mday,
        local.tm_hour,
        local.tm_min,
        local.tm_sec,
        local.tm_year + 1900
    ))
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

/// The foreground process group of this process's controlling terminal, as
/// the terminal itself says, or `None` without a controlling terminal. A
/// Ctrl+C typed there reached every process of that group.
pub fn terminal_foreground_group() -> Option<u32> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    let terminal = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOCTTY)
        .open("/dev/tty")
        .ok()?;
    // SAFETY: tcgetpgrp takes an open descriptor and touches no memory.
    let group = unsafe { libc::tcgetpgrp(terminal.as_raw_fd()) };
    u32::try_from(group).ok().filter(|group| *group > 0)
}

/// A process known by its PID and start time, so a reused PID is never
/// signalled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Identity {
    pub pid: u32,
    pub start: u64,
}

impl Identity {
    pub fn of(process: &ProcessInfo) -> Self {
        Self {
            pid: process.pid,
            start: process.start,
        }
    }

    /// Whoever has this PID now.
    pub fn current(pid: u32) -> Option<Self> {
        process_info(pid).map(|process| Self::of(&process))
    }

    fn is_in(&self, scan: &[ProcessInfo]) -> bool {
        scan.iter().any(|process| Self::of(process) == *self)
    }
}

/// The `tracked` processes still alive as themselves in `scan`, and their
/// descendants. A process is a child only when its parent PID is a tracked
/// process alive in `scan` and it started no earlier than that parent, so a
/// reused PID never brings in the children of another process. A tracked
/// process that is gone is never added back.
pub fn track_tree(tracked: &[Identity], scan: &[ProcessInfo]) -> Vec<Identity> {
    let mut found: Vec<Identity> = tracked
        .iter()
        .filter(|identity| identity.is_in(scan))
        .copied()
        .collect();
    let mut index = 0;
    while index < found.len() {
        let parent = found[index];
        for child in scan.iter().filter(|process| {
            process.ppid == parent.pid && process.pid != parent.pid && process.start >= parent.start
        }) {
            let child = Identity::of(child);
            if !found.contains(&child) {
                found.push(child);
            }
        }
        index += 1;
    }
    found
}

/// Signals each process that is still the one identified, checked right
/// before its signal.
pub fn signal_identities(identities: &[Identity], signal: Signal) {
    for identity in identities {
        if Identity::current(identity.pid) == Some(*identity) {
            send_signal(identity.pid, signal);
        }
    }
}

/// The processes a run's finisher looks at and signals. The native table is
/// the kernel's; tests give their own to replay PID and session reuse.
pub trait ProcessTable {
    fn processes(&self) -> Vec<ProcessInfo>;
    fn process(&self, pid: u32) -> Option<ProcessInfo>;
    fn signal(&self, pid: u32, signal: Signal);
    fn boot_id(&self) -> Option<String>;
    /// Now, in the time domain of [`ProcessInfo::start`].
    fn now(&self) -> u64;
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

    fn now(&self) -> u64 {
        monotonic_now()
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
        let me = Identity::current(std::process::id()).unwrap();
        assert!(track_tree(&[me], &all_processes()).contains(&Identity::of(&info)));

        assert!(send_signal(child.id(), Signal::Kill));
        child.wait().unwrap();
        thread::sleep(Duration::from_millis(50));
        assert!(process_info(child.id()).is_none());
    }

    #[test]
    fn the_start_label_is_what_ps_printed_and_the_boot_came_first() {
        let mut child = Command::new("sleep").arg("5").spawn().unwrap();
        let printed = Command::new("ps")
            .args(["-o", "lstart=", "-p", &child.id().to_string()])
            .env("LC_ALL", "C")
            .output()
            .unwrap();
        let info = process_info(child.id()).unwrap();

        assert_eq!(
            lstart_label(info.started_wall).as_deref(),
            Some(String::from_utf8_lossy(&printed.stdout).trim())
        );
        assert!(boot_time().is_some_and(|boot| boot <= info.started_wall));
        child.kill().unwrap();
        child.wait().unwrap();
    }

    fn process(pid: u32, ppid: u32, start: u64) -> ProcessInfo {
        ProcessInfo {
            pid,
            ppid,
            pgid: pid,
            sid: Some(pid),
            start,
            started_wall: 0,
        }
    }

    #[test]
    fn a_tree_grows_only_from_living_tracked_parents() {
        let root = process(10, 1, 100);
        let child = process(11, 10, 110);
        let grandchild = process(12, 11, 120);
        let tracked = [Identity::of(&root)];

        let tree = track_tree(&tracked, &[root, child, grandchild]);

        assert_eq!(
            tree,
            [root, child, grandchild].map(|process| Identity::of(&process))
        );
    }

    #[test]
    fn a_root_that_is_gone_is_never_added_back_nor_its_pid_s_new_children() {
        let root = process(10, 1, 100);
        // PID 10 now belongs to another process, with a child of its own.
        let reused = process(10, 1, 500);
        let stranger = process(13, 10, 510);

        assert!(track_tree(&[Identity::of(&root)], &[reused, stranger]).is_empty());
    }

    #[test]
    fn a_child_older_than_its_parent_is_not_its_child() {
        let parent = process(10, 1, 500);
        // Started before PID 10 did: its parent was an earlier holder of PID 10.
        let orphan_of_another = process(14, 10, 300);

        let tree = track_tree(&[Identity::of(&parent)], &[parent, orphan_of_another]);

        assert_eq!(tree, [Identity::of(&parent)]);
    }

    #[test]
    fn a_pid_whose_start_changed_is_not_signalled() {
        let mut child = Command::new("sleep").arg("5").spawn().unwrap();
        let wrong = Identity {
            pid: child.id(),
            start: process_info(child.id()).unwrap().start + 1,
        };

        signal_identities(&[wrong], Signal::Kill);

        thread::sleep(Duration::from_millis(100));
        assert!(child.try_wait().unwrap().is_none(), "it was signalled");
        child.kill().unwrap();
        child.wait().unwrap();
    }

    /// What one full scan costs on this machine. Not run by default:
    /// `cargo test --bin localify -- --ignored full_scan_cost --nocapture`.
    #[test]
    #[ignore]
    fn full_scan_cost() {
        let rounds = 50;
        let mut count = 0;
        let started = std::time::Instant::now();
        for _ in 0..rounds {
            count = all_processes().len();
        }
        let each = started.elapsed() / rounds;
        println!("one full scan of {count} processes: {each:?}");
    }

    #[test]
    fn process_groups_are_never_signalled() {
        assert!(!send_signal(0, Signal::Terminate));
        assert!(!send_signal(u32::MAX, Signal::Terminate));
    }
}
