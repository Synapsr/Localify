//! procfs, as `ps` reads it.

use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use super::ProcessInfo;

pub fn pids() -> Vec<u32> {
    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse().ok())
        .filter(|pid: &u32| *pid > 0)
        .collect()
}

pub fn process_info(pid: u32) -> Option<ProcessInfo> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let fields = Stat::parse(&stat)?;
    if fields.state == 'Z' {
        return None;
    }
    // As `ps` computes it, so a start time `ps` printed matches to the second.
    let started_wall = match boot_time() {
        Some(boot) => boot + fields.start / ticks_per_second(),
        None => {
            let age = monotonic_now().saturating_sub(fields.start) / ticks_per_second();
            wall_now().saturating_sub(age)
        }
    };
    Some(ProcessInfo {
        pid,
        ppid: fields.ppid,
        pgid: fields.pgid,
        sid: Some(fields.sid),
        start: fields.start,
        started_wall,
    })
}

/// `btime` in `/proc/stat`: when this boot started, in Unix seconds.
pub fn boot_time() -> Option<u64> {
    let stat = fs::read_to_string("/proc/stat").ok()?;
    stat.lines()
        .find_map(|line| line.strip_prefix("btime "))?
        .trim()
        .parse()
        .ok()
}

/// The fields of `/proc/<pid>/stat` Localify reads.
#[derive(Debug, PartialEq, Eq)]
struct Stat {
    state: char,
    ppid: u32,
    pgid: u32,
    sid: u32,
    /// Field 22: clock ticks since boot.
    start: u64,
}

impl Stat {
    /// The command name (field 2) is in parentheses and may itself hold spaces
    /// and parentheses, so fields are counted from the last `)`.
    fn parse(stat: &str) -> Option<Self> {
        let rest = &stat[stat.rfind(')')? + 1..];
        let fields: Vec<&str> = rest.split_whitespace().collect();
        // fields[0] is field 3 of the file.
        Some(Self {
            state: fields.first()?.chars().next()?,
            ppid: fields.get(1)?.parse().ok()?,
            pgid: fields.get(2)?.parse().ok()?,
            sid: fields.get(3)?.parse().ok()?,
            start: fields.get(19)?.parse().ok()?,
        })
    }
}

/// `CLOCK_BOOTTIME` in clock ticks, the unit of a process's start time. It
/// counts suspended time too, as the kernel does for start times.
pub fn monotonic_now() -> u64 {
    let mut now = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `now` is a writable timespec, the only memory clock_gettime touches.
    if unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut now) } != 0 {
        return 0;
    }
    let ticks = ticks_per_second();
    let seconds = u64::try_from(now.tv_sec).unwrap_or(0);
    let nanos = u64::try_from(now.tv_nsec).unwrap_or(0);
    seconds * ticks + nanos * ticks / 1_000_000_000
}

pub fn boot_id() -> Option<String> {
    let id = fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
    let id = id.trim().to_string();
    (!id.is_empty()).then_some(id)
}

fn ticks_per_second() -> u64 {
    // SAFETY: sysconf takes an integer and touches no memory.
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    u64::try_from(ticks)
        .ok()
        .filter(|ticks| *ticks > 0)
        .unwrap_or(100)
}

fn wall_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_name_with_spaces_and_parentheses_is_skipped() {
        let stat =
            "4242 (my (odd) cmd) S 1 4242 4240 0 -1 4194560 0 0 0 0 0 0 0 0 20 0 1 0 987654 0 0";

        assert_eq!(
            Stat::parse(stat),
            Some(Stat {
                state: 'S',
                ppid: 1,
                pgid: 4242,
                sid: 4240,
                start: 987654,
            })
        );
    }
}
