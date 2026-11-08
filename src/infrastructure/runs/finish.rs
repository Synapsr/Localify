//! How a run ends (docs/remote-machines.md, "The end of a run").
//!
//! A run is over when no process of its session is left and its stream is
//! finalized. A session id is never adopted because its number matches: POSIX
//! does not reuse a PID while a process still carries it as its process group
//! or session id, so a member that started at or before the executor's last
//! heartbeat, in the same boot's monotonic time, proves the session never
//! broke. Only then are its members signalled.

use std::collections::HashSet;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};

use super::output::{self, EndReason, FinalizeLock, RunEnd, TryFinalizeLock};
use crate::infrastructure::process::{NativeProcesses, ProcessInfo, ProcessTable, Signal};

/// How long members get between SIGTERM and SIGKILL.
pub const STOP_GRACE: Duration = Duration::from_secs(10);
/// How long a finisher keeps sending SIGKILL before it gives up.
const KILL_WAIT: Duration = Duration::from_secs(5);
/// Extra time a live executor gets to end its own session and finalize its
/// stream before it is killed too.
const EXECUTOR_MARGIN: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(100);

/// What a run's record says about its session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSession {
    pub executor_pid: u32,
    /// The executor's start, in its boot's monotonic time.
    pub executor_start: u64,
    /// Recorded only when the executor led its session, so it equals its PID.
    pub sid: Option<u32>,
    /// Recorded only when the executor led its process group.
    pub pgid: Option<u32>,
    pub boot_id: String,
    /// The executor's last heartbeat, in the same time as `executor_start`.
    pub last_alive_at: u64,
}

impl RunSession {
    fn contains(&self, process: &ProcessInfo) -> bool {
        self.sid.is_some_and(|sid| process.sid == Some(sid))
            || self.pgid.is_some_and(|pgid| process.pgid == pgid)
    }

    fn is_executor(&self, process: &ProcessInfo) -> bool {
        process.pid == self.executor_pid && process.start == self.executor_start
    }

    fn same_boot(&self, table: &dyn ProcessTable) -> bool {
        table.boot_id().as_deref() == Some(self.boot_id.as_str())
    }

    /// Whether the executor itself still runs: same PID, same start, same boot.
    pub fn executor_alive(&self, table: &dyn ProcessTable) -> bool {
        self.same_boot(table)
            && table
                .process(self.executor_pid)
                .is_some_and(|process| process.start == self.executor_start)
    }

    /// Every current process of the session, without any proof.
    pub fn members(&self, table: &dyn ProcessTable) -> Vec<ProcessInfo> {
        table
            .processes()
            .into_iter()
            .filter(|process| self.contains(process))
            .collect()
    }
}

/// What the first scan of a finisher found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Membership {
    /// The record is from another boot: a reboot ended every process of it.
    OtherBoot,
    /// No member started at or before the last heartbeat: whatever carries the
    /// session id now is unrelated.
    Unproven,
    /// The session never broke, so every current member belongs to the run.
    Proven(Vec<ProcessInfo>),
}

pub fn membership(table: &dyn ProcessTable, session: &RunSession) -> Membership {
    if !session.same_boot(table) {
        return Membership::OtherBoot;
    }
    membership_in(&table.processes(), session)
}

/// Proves membership on the very scan whose members are adopted: it holds the
/// executor itself (same PID and start), or a member that started at or
/// before the last heartbeat.
fn membership_in(scan: &[ProcessInfo], session: &RunSession) -> Membership {
    let members: Vec<ProcessInfo> = scan
        .iter()
        .filter(|process| session.contains(process))
        .copied()
        .collect();
    let proven = members
        .iter()
        .any(|process| session.is_executor(process) || process.start <= session.last_alive_at);
    if proven {
        Membership::Proven(members)
    } else {
        Membership::Unproven
    }
}

/// Every descendant of the executor in `scan`, by parent PID, including those
/// that left its session.
fn executor_tree(scan: &[ProcessInfo], session: &RunSession) -> Vec<ProcessInfo> {
    if !scan.iter().any(|process| session.is_executor(process)) {
        return Vec::new();
    }
    let mut tree = Vec::new();
    let mut parents = vec![session.executor_pid];
    while let Some(parent) = parents.pop() {
        for child in scan.iter().filter(|process| process.ppid == parent) {
            if child.pid != session.executor_pid && !tree.contains(child) {
                tree.push(*child);
                parents.push(child.pid);
            }
        }
    }
    tree
}

/// What a finisher did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finish {
    AlreadyFinalized,
    /// This finisher published `end.json`.
    Finalized(EndReason),
    /// The executor still runs, so the run is not over.
    ExecutorAlive,
    /// Processes proven to belong to the run are alive, and this finisher does
    /// not signal them.
    LiveMembers(Vec<u32>),
    /// Another finisher holds the run's lock.
    Busy,
    /// The run's folder is gone: nothing is left to finalize.
    Gone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinishMode {
    /// The next launcher: finalizes only a run with nothing left alive, and
    /// refuses rather than wait for another finisher.
    Admit,
    /// The daemon and `stop`: signals proven members, and waits for the lock.
    End,
}

/// Ends runs. The native one acts on the kernel's processes; tests give a table
/// of their own and a shorter grace period.
pub struct Finisher<'a> {
    pub table: &'a dyn ProcessTable,
    pub grace: Duration,
}

impl Finisher<'static> {
    pub fn native() -> Self {
        Self {
            table: &NativeProcesses,
            grace: STOP_GRACE,
        }
    }
}

impl Finisher<'_> {
    /// Finishes a run whose executor is gone, holding the run's finalization
    /// lock throughout. `captured_by_executor` says the executor read the run's
    /// output itself, so its death lost what was still in the pipe.
    pub fn finish(
        &self,
        dir: &Path,
        session: &RunSession,
        captured_by_executor: bool,
        mode: FinishMode,
    ) -> Result<Finish> {
        let _lock = match mode {
            FinishMode::Admit => match FinalizeLock::try_acquire(dir)? {
                TryFinalizeLock::Acquired(lock) => lock,
                TryFinalizeLock::Busy => return Ok(Finish::Busy),
                TryFinalizeLock::Gone => return Ok(Finish::Gone),
            },
            FinishMode::End => match FinalizeLock::acquire(dir)? {
                Some(lock) => lock,
                None => return Ok(Finish::Gone),
            },
        };
        if output::read_end(dir)?.is_some() {
            return Ok(Finish::AlreadyFinalized);
        }
        if session.executor_alive(self.table) {
            return Ok(Finish::ExecutorAlive);
        }
        if let Membership::Proven(members) = membership(self.table, session) {
            if mode == FinishMode::Admit {
                return Ok(Finish::LiveMembers(
                    members.iter().map(|process| process.pid).collect(),
                ));
            }
            self.end_members(session, members, Vec::new(), self.grace, None)?;
        }
        let reason = if captured_by_executor {
            EndReason::Interrupted
        } else {
            EndReason::Complete
        };
        let (_, written) = output::extent(dir)?;
        output::publish_end(dir, &RunEnd::now(reason, written))?;
        Ok(Finish::Finalized(reason))
    }

    /// Stops a run: every process of its session, the executor included, then
    /// finishes it. The executor is looked for in the one scan whose members
    /// are signalled, never in an earlier look. While it lives, no other
    /// finisher acts, its process tree is ended too (a descendant that left
    /// the session included), and it gets a moment to end its own session and
    /// finalize first. Once it is gone, the run is ended under its lock.
    pub fn stop(
        &self,
        dir: &Path,
        session: &RunSession,
        captured_by_executor: bool,
    ) -> Result<Finish> {
        if session.same_boot(self.table) {
            let scan = self.table.processes();
            if scan.iter().any(|process| session.is_executor(process))
                && let Membership::Proven(members) = membership_in(&scan, session)
            {
                let tree = executor_tree(&scan, session);
                let grace = self.grace + EXECUTOR_MARGIN;
                self.end_members(session, members, tree, grace, None)?;
            }
        }
        self.finish(dir, session, captured_by_executor, FinishMode::End)
    }

    /// Signals SIGTERM to `members` and `tree`, SIGKILL to those left after
    /// `grace`, and scans again until none is left. Each process's start is
    /// checked before each signal. A process that joined the session is
    /// adopted only while a member already known is still alive, which keeps
    /// the proof unbroken. `tree` holds the executor's descendants by
    /// identity, so one that left the session is still ended; the executor's
    /// new descendants join it while the executor lives. `exclude` (the
    /// executor ending its own session) is never signalled, but still counts
    /// as a known member.
    pub fn end_members(
        &self,
        session: &RunSession,
        members: Vec<ProcessInfo>,
        tree: Vec<ProcessInfo>,
        grace: Duration,
        exclude: Option<u32>,
    ) -> Result<()> {
        let mut known = members;
        let mut retained = tree;
        let mut signalled: HashSet<(u32, u64)> = HashSet::new();
        let started = Instant::now();
        loop {
            let killing = started.elapsed() >= grace;
            for (process, escaped) in targets(&known, &retained, exclude) {
                if killing {
                    self.signal_checked(session, &process, escaped, Signal::Kill);
                } else if signalled.insert((process.pid, process.start)) {
                    self.signal_checked(session, &process, escaped, Signal::Terminate);
                }
            }
            thread::sleep(POLL);
            let scan = self.table.processes();
            known = still_members(session, &known, &scan);
            retained = still_retained(session, &retained, &scan);
            let left: Vec<String> = targets(&known, &retained, exclude)
                .iter()
                .map(|(process, _)| process.pid.to_string())
                .collect();
            if left.is_empty() {
                return Ok(());
            }
            if started.elapsed() >= grace + KILL_WAIT {
                let left = left.join(" ");
                bail!(
                    "Processes of the run are still alive after SIGKILL: {left}. Stop them with: kill -9 {left}"
                );
            }
        }
    }

    /// Signals a process only while it is the one seen: same start, and still
    /// in the session unless it was retained from the executor's tree.
    fn signal_checked(
        &self,
        session: &RunSession,
        process: &ProcessInfo,
        escaped: bool,
        signal: Signal,
    ) {
        let unchanged = self
            .table
            .process(process.pid)
            .is_some_and(|now| now.start == process.start && (escaped || session.contains(&now)));
        if unchanged {
            self.table.signal(process.pid, signal);
        }
    }
}

/// The processes to signal, each once: session members, then retained
/// descendants that are not members (`true`).
fn targets(
    known: &[ProcessInfo],
    retained: &[ProcessInfo],
    exclude: Option<u32>,
) -> Vec<(ProcessInfo, bool)> {
    let members = known.iter().map(|process| (*process, false));
    let escaped = retained
        .iter()
        .filter(|process| !known.contains(process))
        .map(|process| (*process, true));
    members
        .chain(escaped)
        .filter(|(process, _)| Some(process.pid) != exclude)
        .collect()
}

fn still_members(
    session: &RunSession,
    known: &[ProcessInfo],
    scan: &[ProcessInfo],
) -> Vec<ProcessInfo> {
    let current: Vec<ProcessInfo> = scan
        .iter()
        .filter(|process| session.contains(process))
        .copied()
        .collect();
    let is_known = |process: &ProcessInfo| {
        known
            .iter()
            .any(|old| old.pid == process.pid && old.start == process.start)
    };
    if current.iter().any(is_known) {
        current
    } else {
        Vec::new()
    }
}

/// Retained descendants still alive as themselves, plus the descendants a
/// live executor started since.
fn still_retained(
    session: &RunSession,
    retained: &[ProcessInfo],
    scan: &[ProcessInfo],
) -> Vec<ProcessInfo> {
    let alive = |old: &ProcessInfo| {
        scan.iter()
            .any(|process| process.pid == old.pid && process.start == old.start)
    };
    let mut still: Vec<ProcessInfo> = retained.iter().filter(|old| alive(old)).copied().collect();
    for process in executor_tree(scan, session) {
        if !still
            .iter()
            .any(|old| old.pid == process.pid && old.start == process.start)
        {
            still.push(process);
        }
    }
    still
}
