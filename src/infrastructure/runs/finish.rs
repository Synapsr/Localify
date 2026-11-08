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
//! scan is adopted only once a witness is read again, alive and unchanged,
//! after the enumeration.
//!
//! A finisher that signals writes what it proved to `finalize-state.json`
//! first: how far the session is proven unbroken, and the processes it found
//! to be the run's. A finisher that takes over after a crash, and the locked
//! end of a stop, start from there.

use std::collections::HashSet;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use super::output::{self, Acquired, EndReason, FinalizeLock, RunEnd};
use crate::infrastructure::process::{
    Identity, NativeProcesses, ProcessInfo, ProcessTable, Signal, track_tree,
};

/// How long members get between SIGTERM and SIGKILL.
pub const STOP_GRACE: Duration = Duration::from_secs(10);
/// How long a finisher keeps sending SIGKILL before it gives up.
pub const KILL_WAIT: Duration = Duration::from_secs(5);
/// How long a launcher, which holds its service's lock, waits for another
/// finisher of the previous run. A finisher signals for at most STOP_GRACE
/// plus KILL_WAIT under the run's lock; twice that leaves room for its scans.
/// A finisher still holding it after that is stuck or suspended.
pub const CLEANUP_WAIT: Duration =
    Duration::from_secs(2 * (STOP_GRACE.as_secs() + KILL_WAIT.as_secs()));
/// How many scans a finisher tries before it gives up proving a session whose
/// witnesses keep vanishing.
const PROOF_ATTEMPTS: usize = 5;
const POLL: Duration = Duration::from_millis(100);
const STATE_FILE: &str = "finalize-state.json";

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
    /// A process's session decides when both are known; its process group
    /// only when the kernel would not give its session.
    fn contains(&self, process: &ProcessInfo) -> bool {
        match (self.sid, process.sid) {
            (Some(sid), Some(theirs)) => theirs == sid,
            _ => self.pgid.is_some_and(|pgid| process.pgid == pgid),
        }
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
    /// Members that could prove the session kept vanishing before they could
    /// be read again: nothing is decided, and the run stays open.
    Inconclusive,
    /// The session never broke, so every current member belongs to the run.
    Proven(Vec<ProcessInfo>),
}

pub fn membership(table: &dyn ProcessTable, session: &RunSession) -> Membership {
    if !session.same_boot(table) {
        return Membership::OtherBoot;
    }
    match prove(table, session) {
        Proof::Proven(members, _) => Membership::Proven(members),
        Proof::Unproven => Membership::Unproven,
        Proof::Inconclusive => Membership::Inconclusive,
    }
}

enum Proof {
    /// The members, and the whole scan they came from.
    Proven(Vec<ProcessInfo>, Vec<ProcessInfo>),
    Unproven,
    Inconclusive,
}

/// A scan whose membership is proven: it holds the executor itself, or a
/// member that started at or before the last heartbeat, and one such witness
/// is read again, unchanged, after the enumeration. Every candidate of a scan
/// is tried; when all of them vanished, the scan is dropped and the proof
/// starts over, and after a few such scans the proof is inconclusive.
fn prove(table: &dyn ProcessTable, session: &RunSession) -> Proof {
    for _ in 0..PROOF_ATTEMPTS {
        let scan = table.processes();
        let members: Vec<ProcessInfo> = scan
            .iter()
            .filter(|process| session.contains(process))
            .copied()
            .collect();
        // The executor first: when it is the witness, it is proven alive too.
        let mut candidates: Vec<&ProcessInfo> = members
            .iter()
            .filter(|process| session.is_executor(process))
            .collect();
        candidates.extend(members.iter().filter(|process| {
            !session.is_executor(process) && process.start <= session.last_alive_at
        }));
        if candidates.is_empty() {
            return Proof::Unproven;
        }
        if candidates
            .into_iter()
            .any(|witness| session.still_member(table, witness))
        {
            return Proof::Proven(members, scan);
        }
    }
    Proof::Inconclusive
}

/// What a finisher did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finish {
    AlreadyFinalized,
    /// This finisher published `end.json`.
    Finalized(EndReason),
    /// The executor still runs, so the run is not over.
    ExecutorAlive,
    /// Another finisher still held the run's lock when the wait ran out
    /// ([`Finisher::finish_within`] only).
    Busy,
    /// The run's folder is gone: nothing is left to finalize.
    Gone,
}

/// What a finisher proved, kept in the run's folder for the next one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FinalizeState {
    /// The session is proven unbroken until this time, in the processes' time.
    proven_through: u64,
    targets: Vec<Target>,
}

/// A process found to be the run's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct Target {
    pid: u32,
    start: u64,
    /// A member of the run's session; otherwise a descendant of its executor
    /// that left it, signalled while its start still matches.
    member: bool,
}

impl FinalizeState {
    fn load(dir: &Path) -> Result<Self> {
        let path = dir.join(STATE_FILE);
        match fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str(&content)
                .with_context(|| format!("'{}' is not a valid finalize state", path.display())),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => {
                Err(error).with_context(|| format!("Failed to read '{}'", path.display()))
            }
        }
    }

    /// Forgets what was saved: it belongs to another boot.
    fn discard(dir: &Path) -> Result<()> {
        match fs::remove_file(dir.join(STATE_FILE)) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error)
                .with_context(|| format!("Failed to remove '{}'", dir.join(STATE_FILE).display())),
            _ => Ok(()),
        }
    }

    /// Written whole through a synced temporary file renamed over it.
    fn save(&self, dir: &Path) -> Result<()> {
        let path = dir.join(STATE_FILE);
        let temporary = dir.join(format!("{STATE_FILE}.{}.tmp", std::process::id()));
        let mut file = fs::File::create(&temporary)
            .with_context(|| format!("Failed to write '{}'", temporary.display()))?;
        file.write_all(&serde_json::to_vec(self)?)
            .and_then(|()| file.sync_all())
            .with_context(|| format!("Failed to write '{}'", temporary.display()))?;
        fs::rename(&temporary, &path)
            .with_context(|| format!("Failed to write '{}'", path.display()))
    }

    fn identities(&self, member: bool) -> Vec<Identity> {
        self.targets
            .iter()
            .filter(|target| target.member == member)
            .map(|target| Identity {
                pid: target.pid,
                start: target.start,
            })
            .collect()
    }
}

/// Where a round of ending processes keeps what it proved.
struct Journal<'a> {
    dir: &'a Path,
    /// Whether the caller holds `finalize.lock`; otherwise each write takes it.
    locked: bool,
    saved: FinalizeState,
}

impl Journal<'_> {
    fn record(&mut self, horizon: u64, known: &[ProcessInfo], retained: &[Identity]) -> Result<()> {
        let mut targets: Vec<Target> = known
            .iter()
            .map(|process| Target {
                pid: process.pid,
                start: process.start,
                member: true,
            })
            .collect();
        for identity in retained {
            if !targets
                .iter()
                .any(|target| target.pid == identity.pid && target.start == identity.start)
            {
                targets.push(Target {
                    pid: identity.pid,
                    start: identity.start,
                    member: false,
                });
            }
        }
        let state = FinalizeState {
            proven_through: horizon.max(self.saved.proven_through),
            targets,
        };
        if state == self.saved {
            return Ok(());
        }
        let _lock = match self.locked {
            true => None,
            false => match FinalizeLock::acquire(self.dir)? {
                Some(lock) => Some(lock),
                None => return Ok(()),
            },
        };
        state.save(self.dir)?;
        self.saved = state;
        Ok(())
    }
}

/// What a round of ending processes is after.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    /// The run's processes, found by session and by descent, until none is left.
    Run,
    /// The same, but only while the executor lives.
    WhileExecutorLives,
    /// Only the exact processes it is given, alive as themselves.
    SavedOnly,
}

/// How a round of ending processes stopped.
enum Ended {
    /// No process is left.
    Empty,
    /// The executor died while it was being stopped without the lock; what it
    /// proved is in the journal, for the locked end.
    ExecutorGone,
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
    /// lock throughout, from what an earlier finisher proved if one did: its
    /// proven processes are ended, then its stream is finalized. The daemon,
    /// `stop` and the next launcher all finish a run this way.
    /// `captured_by_executor` says the executor read the run's output itself,
    /// so its death lost what was still in the pipe.
    pub fn finish(
        &self,
        dir: &Path,
        session: &RunSession,
        captured_by_executor: bool,
    ) -> Result<Finish> {
        match FinalizeLock::acquire(dir)? {
            Some(lock) => self.finish_locked(&lock, dir, session, captured_by_executor),
            None => Ok(Finish::Gone),
        }
    }

    /// `finish`, for a launcher, which holds its service's lock meanwhile: it
    /// waits at most `limit` for another finisher of the run, so a stuck or
    /// suspended one never holds the service forever.
    pub fn finish_within(
        &self,
        dir: &Path,
        session: &RunSession,
        captured_by_executor: bool,
        limit: Duration,
    ) -> Result<Finish> {
        match FinalizeLock::acquire_within(dir, limit)? {
            Acquired::Locked(lock) => self.finish_locked(&lock, dir, session, captured_by_executor),
            Acquired::Gone => Ok(Finish::Gone),
            Acquired::Busy => Ok(Finish::Busy),
        }
    }

    fn finish_locked(
        &self,
        _lock: &FinalizeLock,
        dir: &Path,
        session: &RunSession,
        captured_by_executor: bool,
    ) -> Result<Finish> {
        let finalized = output::read_end(dir)?.is_some();
        // Another boot ended every process of the run: nothing it saved is
        // trusted, nothing is signalled.
        if !session.same_boot(self.table) {
            FinalizeState::discard(dir)?;
            return match finalized {
                true => Ok(Finish::AlreadyFinalized),
                false => self.publish_end(dir, captured_by_executor),
            };
        }
        let saved = FinalizeState::load(dir)?;
        let escaped = saved.identities(false);
        if finalized {
            // The executor finalized its own run, but what it left outside its
            // session, captured while it lived, is still ended: those exact
            // processes, nothing found by session.
            if !escaped.is_empty() {
                let mut journal = Journal {
                    dir,
                    locked: true,
                    saved,
                };
                self.end_until(&mut journal, session, Vec::new(), escaped, Scope::SavedOnly)?;
            }
            return Ok(Finish::AlreadyFinalized);
        }
        if session.executor_alive(self.table) {
            return Ok(Finish::ExecutorAlive);
        }
        // What an earlier finisher proved extends the executor's heartbeat.
        let session = RunSession {
            last_alive_at: session.last_alive_at.max(saved.proven_through),
            ..session.clone()
        };
        let mut members = match membership(self.table, &session) {
            Membership::Proven(members) => members,
            Membership::OtherBoot | Membership::Unproven => Vec::new(),
            Membership::Inconclusive => bail!(
                "Could not confirm which processes belong to the run: they kept changing while Localify looked. Try again."
            ),
        };
        members.extend(still_saved_members(self.table, &session, &saved, &members));
        let escaped: Vec<Identity> = escaped
            .into_iter()
            .filter(|identity| Identity::current_in(self.table, identity.pid) == Some(*identity))
            .collect();
        if !members.is_empty() || !escaped.is_empty() {
            let mut journal = Journal {
                dir,
                locked: true,
                saved,
            };
            self.end_until(&mut journal, &session, members, escaped, Scope::Run)?;
        }
        self.publish_end(dir, captured_by_executor)
    }

    fn publish_end(&self, dir: &Path, captured_by_executor: bool) -> Result<Finish> {
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
    /// alive, the rest is ended under the lock, from what was proven.
    pub fn stop(
        &self,
        dir: &Path,
        session: &RunSession,
        captured_by_executor: bool,
    ) -> Result<Finish> {
        if session.same_boot(self.table)
            && let Proof::Proven(members, scan) = prove(self.table, session)
            && scan.iter().any(|process| session.is_executor(process))
            && session.executor_alive(self.table)
        {
            let tree = executor_tree(&scan, session);
            let mut journal = Journal {
                dir,
                locked: false,
                saved: FinalizeState::load(dir)?,
            };
            let grace = self.grace + self.grace / 2;
            let finisher = Finisher {
                table: self.table,
                grace,
            };
            finisher.end_until(
                &mut journal,
                session,
                members,
                tree,
                Scope::WhileExecutorLives,
            )?;
        }
        self.finish(dir, session, captured_by_executor)
    }

    /// Signals SIGTERM to `members` and `tree`, SIGKILL to those left after
    /// the grace period, and scans again until none is left. Each process's
    /// start is checked before each signal. `tree` holds the executor's
    /// descendants by identity, so one that left the session is still ended,
    /// and grows from every one of them still alive. What is proven goes to
    /// the journal before each round's signals: a known member is read again,
    /// which moves the proof horizon, the horizon is written, and only then
    /// is any process signalled, each one's identity checked again first.
    fn end_until(
        &self,
        journal: &mut Journal<'_>,
        session: &RunSession,
        members: Vec<ProcessInfo>,
        tree: Vec<Identity>,
        scope: Scope,
    ) -> Result<Ended> {
        let mut known = members;
        let mut retained = tree;
        // The session is proven unbroken until this time, in the processes' time.
        let mut horizon = session.last_alive_at;
        let mut signalled: HashSet<Identity> = HashSet::new();
        let started = Instant::now();
        // Counted from the first SIGKILL, so a finisher suspended past the
        // grace period still sends it before giving up.
        let mut killing_since: Option<Instant> = None;
        loop {
            if scope != Scope::SavedOnly {
                // A member read alive now proves the session until then.
                let before_reading = self.table.now();
                if known
                    .iter()
                    .any(|process| session.still_member(self.table, process))
                {
                    horizon = horizon.max(before_reading);
                }
            }
            journal.record(horizon, &known, &retained)?;
            let killing = started.elapsed() >= self.grace;
            if killing && killing_since.is_none() {
                killing_since = Some(Instant::now());
            }
            for (process, member) in targets(&known, &retained) {
                let signal = match killing {
                    true => Signal::Kill,
                    false if signalled.insert(process) => Signal::Terminate,
                    false => continue,
                };
                self.signal_checked(session, process, member, signal);
            }
            thread::sleep(POLL);
            let scan = self.table.processes();
            if scope == Scope::SavedOnly {
                // Those exact processes, alive as themselves: nothing more.
                let alive = |identity: &Identity| {
                    scan.iter()
                        .any(|process| Identity::of(process) == *identity)
                };
                known.retain(|process| alive(&Identity::of(process)));
                retained.retain(alive);
            } else {
                known = self.still_members(session, &known, &scan, &mut horizon);
                retained = still_retained(session, &retained, &scan);
            }
            let left = targets(&known, &retained);
            if left.is_empty() {
                return Ok(Ended::Empty);
            }
            if scope == Scope::WhileExecutorLives && !session.executor_alive(self.table) {
                journal.record(horizon, &known, &retained)?;
                return Ok(Ended::ExecutorGone);
            }
            if killing_since.is_some_and(|since| since.elapsed() >= KILL_WAIT) {
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

    /// The session's members in `scan` that belong to the run: those already
    /// known, by identity, and those started strictly before `horizon`. When a
    /// known member is read again after the scan, unchanged, the session was
    /// unbroken at least until just before that read, which moves `horizon`:
    /// a process that started before then carried the session while it was
    /// the run's, and carries it still. A start in the same tick proves
    /// nothing. A scan whose members all vanished moves nothing, so a
    /// replacement session is never adopted.
    fn still_members(
        &self,
        session: &RunSession,
        known: &[ProcessInfo],
        scan: &[ProcessInfo],
        horizon: &mut u64,
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
        let before_reading = self.table.now();
        if current
            .iter()
            .any(|process| is_known(process) && session.still_member(self.table, process))
        {
            *horizon = (*horizon).max(before_reading);
        }
        current
            .into_iter()
            .filter(|process| is_known(process) || process.start < *horizon)
            .collect()
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

impl Identity {
    /// Whoever has this PID now, in `table`.
    fn current_in(table: &dyn ProcessTable, pid: u32) -> Option<Self> {
        table.process(pid).map(|process| Self::of(&process))
    }
}

/// The members an earlier finisher proved, still alive as themselves and
/// still in the session, that a fresh proof does not already hold.
fn still_saved_members(
    table: &dyn ProcessTable,
    session: &RunSession,
    saved: &FinalizeState,
    proven: &[ProcessInfo],
) -> Vec<ProcessInfo> {
    saved
        .identities(true)
        .into_iter()
        .filter_map(|identity| table.process(identity.pid))
        .filter(|process| {
            saved.identities(true).contains(&Identity::of(process))
                && session.contains(process)
                && !proven
                    .iter()
                    .any(|known| Identity::of(known) == Identity::of(process))
        })
        .collect()
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
fn targets(known: &[ProcessInfo], retained: &[Identity]) -> Vec<(Identity, bool)> {
    let members: Vec<Identity> = known.iter().map(Identity::of).collect();
    let escaped = retained
        .iter()
        .filter(|identity| !members.contains(identity))
        .map(|identity| (*identity, false));
    members
        .iter()
        .map(|identity| (*identity, true))
        .chain(escaped)
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
