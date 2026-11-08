//! How a run ends (docs/remote-machines.md, "The end of a run").
//!
//! A run is over when no process of its session is left and its stream is
//! finalized. A session id is never adopted because its number matches: POSIX
//! does not reuse a PID while a process still carries it as its process group
//! or session id, so a member that started at or before the executor's last
//! heartbeat, in the same boot's monotonic time, proves the session never
//! broke. Only then are its members signalled.
//!
//! Enumerating processes is not atomic: a witness seen early in a scan may be
//! gone by its end, and the rest of the scan then shows a replacement. So a
//! scan is adopted only once its witness is read again, alive and unchanged,
//! after the enumeration.

use std::collections::HashSet;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};

use super::output::{self, EndReason, FinalizeLock, RunEnd, TryFinalizeLock};
use crate::infrastructure::process::{
    Identity, NativeProcesses, ProcessInfo, ProcessTable, Signal, track_tree,
};

/// How long members get between SIGTERM and SIGKILL.
pub const STOP_GRACE: Duration = Duration::from_secs(10);
/// How long a finisher keeps sending SIGKILL before it gives up.
const KILL_WAIT: Duration = Duration::from_secs(5);
/// How many scans a finisher tries before it gives up proving a session whose
/// witnesses keep vanishing, and signals nothing.
const PROOF_ATTEMPTS: usize = 5;
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

    fn executor(&self) -> Identity {
        Identity {
            pid: self.executor_pid,
            start: self.executor_start,
        }
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

    /// Whether `process` is still alive as itself, and still in the session.
    fn still_member(&self, table: &dyn ProcessTable, process: &ProcessInfo) -> bool {
        table
            .process(process.pid)
            .is_some_and(|now| now.start == process.start && self.contains(&now))
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
    match prove(table, session) {
        Some((members, _)) => Membership::Proven(members),
        None => Membership::Unproven,
    }
}

/// A scan whose membership is proven: it holds the executor itself, or a
/// member that started at or before the last heartbeat, and that witness is
/// read again, unchanged, after the enumeration. A witness that vanished
/// discards the scan, and the proof starts over. Returns the members and the
/// whole scan.
fn prove(
    table: &dyn ProcessTable,
    session: &RunSession,
) -> Option<(Vec<ProcessInfo>, Vec<ProcessInfo>)> {
    for _ in 0..PROOF_ATTEMPTS {
        let scan = table.processes();
        let members: Vec<ProcessInfo> = scan
            .iter()
            .filter(|process| session.contains(process))
            .copied()
            .collect();
        // The executor first: when it is the witness, it is proven alive too.
        let witness = members
            .iter()
            .find(|process| session.is_executor(process))
            .or_else(|| {
                members
                    .iter()
                    .find(|process| process.start <= session.last_alive_at)
            })?;
        if session.still_member(table, witness) {
            return Some((members, scan));
        }
    }
    None
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

/// How a round of ending processes stopped.
enum Ended {
    /// No process is left.
    Empty,
    /// The executor died while it was being stopped without the lock: these
    /// processes, proven while it lived, are ended under the lock.
    ExecutorGone(Vec<Identity>),
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
        self.finish_with(dir, session, captured_by_executor, mode, Vec::new())
    }

    /// `finish`, also ending `carried`: processes proven to belong to the run
    /// while its executor lived, signalled only while they are still
    /// themselves.
    fn finish_with(
        &self,
        dir: &Path,
        session: &RunSession,
        captured_by_executor: bool,
        mode: FinishMode,
        carried: Vec<Identity>,
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
            // The executor finalized its own run, but what it left outside its
            // session, captured while it lived, is still ended.
            if !carried.is_empty() {
                self.end_until(session, Vec::new(), carried, self.grace, None, false)?;
            }
            return Ok(Finish::AlreadyFinalized);
        }
        if session.executor_alive(self.table) {
            return Ok(Finish::ExecutorAlive);
        }
        let members = match membership(self.table, session) {
            Membership::Proven(members) => members,
            Membership::OtherBoot | Membership::Unproven => Vec::new(),
        };
        if mode == FinishMode::Admit && !members.is_empty() {
            return Ok(Finish::LiveMembers(
                members.iter().map(|process| process.pid).collect(),
            ));
        }
        if !members.is_empty() || !carried.is_empty() {
            self.end_until(session, members, carried, self.grace, None, false)?;
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
    /// finishes it. While the executor lives, no other finisher acts, so its
    /// session and its process tree (a descendant that left the session
    /// included) are ended without the lock, and it gets a moment to end its
    /// own session and finalize first. As soon as a scan no longer shows it
    /// alive, the rest is ended under the lock.
    pub fn stop(
        &self,
        dir: &Path,
        session: &RunSession,
        captured_by_executor: bool,
    ) -> Result<Finish> {
        let mut carried = Vec::new();
        if session.same_boot(self.table)
            && let Some((members, scan)) = prove(self.table, session)
            && scan.iter().any(|process| session.is_executor(process))
            && session.executor_alive(self.table)
        {
            let tree = executor_tree(&scan, session);
            let grace = self.grace + self.grace / 2;
            if let Ended::ExecutorGone(left) =
                self.end_until(session, members, tree, grace, None, true)?
            {
                carried = left;
            }
        }
        self.finish_with(dir, session, captured_by_executor, FinishMode::End, carried)
    }

    /// Signals SIGTERM to `members` and `tree`, SIGKILL to those left after
    /// `grace`, and scans again until none is left. Each process's start is
    /// checked before each signal. A process that joined the session is
    /// adopted only while a member already known is still alive, which keeps
    /// the proof unbroken. `tree` holds the executor's descendants by
    /// identity, so one that left the session is still ended, and grows from
    /// every one of them still alive. `exclude` (the executor ending its own
    /// session) is never signalled, but still counts as a known member.
    pub fn end_members(
        &self,
        session: &RunSession,
        members: Vec<ProcessInfo>,
        tree: Vec<Identity>,
        grace: Duration,
        exclude: Option<u32>,
    ) -> Result<()> {
        self.end_until(session, members, tree, grace, exclude, false)
            .map(|_| ())
    }

    /// `end_members`, which with `while_executor_lives` returns as soon as a
    /// scan no longer shows the executor alive, with what is left.
    fn end_until(
        &self,
        session: &RunSession,
        members: Vec<ProcessInfo>,
        tree: Vec<Identity>,
        grace: Duration,
        exclude: Option<u32>,
        while_executor_lives: bool,
    ) -> Result<Ended> {
        let mut known = members;
        let mut retained = tree;
        let mut signalled: HashSet<Identity> = HashSet::new();
        let started = Instant::now();
        loop {
            let killing = started.elapsed() >= grace;
            for (process, member) in targets(&known, &retained, exclude) {
                if killing {
                    self.signal_checked(session, process, member, Signal::Kill);
                } else if signalled.insert(process) {
                    self.signal_checked(session, process, member, Signal::Terminate);
                }
            }
            thread::sleep(POLL);
            let scan = self.table.processes();
            known = self.still_members(session, &known, &scan);
            retained = still_retained(session, &retained, &scan);
            let left = targets(&known, &retained, exclude);
            if left.is_empty() {
                return Ok(Ended::Empty);
            }
            if while_executor_lives && !session.executor_alive(self.table) {
                let left = left.into_iter().map(|(process, _)| process).collect();
                return Ok(Ended::ExecutorGone(left));
            }
            if started.elapsed() >= grace + KILL_WAIT {
                let left: Vec<String> = left
                    .iter()
                    .map(|(process, _)| process.pid.to_string())
                    .collect();
                let left = left.join(" ");
                bail!(
                    "Processes of the run are still alive after SIGKILL: {left}. Stop them with: kill -9 {left}"
                );
            }
        }
    }

    /// The session's members in `scan`, when a known member is still one of
    /// them after the scan, read again. Otherwise the session may have broken
    /// during the scan, and no member is adopted.
    fn still_members(
        &self,
        session: &RunSession,
        known: &[ProcessInfo],
        scan: &[ProcessInfo],
    ) -> Vec<ProcessInfo> {
        let current: Vec<ProcessInfo> = scan
            .iter()
            .filter(|process| session.contains(process))
            .copied()
            .collect();
        // By identity: a parent PID changes when a member is reparented.
        let is_known = |process: &ProcessInfo| {
            known
                .iter()
                .any(|old| Identity::of(old) == Identity::of(process))
        };
        let unbroken = current
            .iter()
            .any(|process| is_known(process) && session.still_member(self.table, process));
        if unbroken { current } else { Vec::new() }
    }

    /// Signals a process only while it is the one seen: same start, and for a
    /// session member, still in the session.
    fn signal_checked(
        &self,
        session: &RunSession,
        process: Identity,
        member: bool,
        signal: Signal,
    ) {
        let unchanged = self
            .table
            .process(process.pid)
            .is_some_and(|now| now.start == process.start && (!member || session.contains(&now)));
        if unchanged {
            self.table.signal(process.pid, signal);
        }
    }
}

/// The executor's descendants in `scan`, by identity, including those that
/// left its session.
fn executor_tree(scan: &[ProcessInfo], session: &RunSession) -> Vec<Identity> {
    let executor = session.executor();
    track_tree(&[executor], scan)
        .into_iter()
        .filter(|identity| *identity != executor)
        .collect()
}

/// The processes to signal, each once: session members (`true`), then
/// retained descendants that are not members.
fn targets(
    known: &[ProcessInfo],
    retained: &[Identity],
    exclude: Option<u32>,
) -> Vec<(Identity, bool)> {
    let members: Vec<Identity> = known.iter().map(Identity::of).collect();
    let escaped = retained
        .iter()
        .filter(|identity| !members.contains(identity))
        .map(|identity| (*identity, false));
    members
        .iter()
        .map(|identity| (*identity, true))
        .chain(escaped)
        .filter(|(identity, _)| Some(identity.pid) != exclude)
        .collect()
}

/// Retained descendants still alive as themselves, and every process one of
/// them, or the live executor, started since.
fn still_retained(
    session: &RunSession,
    retained: &[Identity],
    scan: &[ProcessInfo],
) -> Vec<Identity> {
    let executor = session.executor();
    let mut roots = retained.to_vec();
    roots.push(executor);
    track_tree(&roots, scan)
        .into_iter()
        .filter(|identity| *identity != executor)
        .collect()
}
