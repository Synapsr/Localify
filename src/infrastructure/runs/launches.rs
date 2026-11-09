//! Launch records (docs/remote-machines.md, "Launch records"): what happened
//! to one dispatched launch, decided once.
//!
//! Each launch id has one record, `services/launches/<launchId>.json`. Its
//! first decision is published whole: written to a unique temporary file,
//! synced, then linked to the record's name, which fails when a decision is
//! already there. The executor admits or refuses its launch; a reconciler
//! declares it expired once its expiry has passed with no record. Whichever
//! comes first wins, and the other obeys it. An admitted launch then gets its
//! exit, written whole and renamed over the record, which changes nothing
//! else.
//!
//! Readers only read `<launchId>.json`: a temporary file a crash left before
//! its link is never a decision, and the daemon sweeps it after an hour. A
//! decision or an exit counts only once the folder that names it is synced,
//! so a crash never takes back what the other side already obeyed.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use super::RunId;
use super::finish::{Finish, Finisher, RunSession};
use crate::infrastructure::launch_rev::random_bytes;
use crate::infrastructure::process::{self, ProcessTable};
use crate::infrastructure::running_services::{RunningService, RunningServiceStore, ServiceKey};

/// Records younger than this are kept, however many there are.
const RETENTION: Duration = Duration::from_secs(24 * 60 * 60);
/// The newest records kept, however old they are.
const KEEP_AT_LEAST: usize = 100;
/// A temporary file older than this was left by a crash.
pub const TEMPORARY_MAX_AGE: Duration = Duration::from_secs(60 * 60);
/// Held for each read-modify-write of an admitted record's exit, so two
/// finishers never interleave theirs. Not a `.json` file, so never read as a
/// record.
const EXIT_LOCK: &str = "exits.lock";
/// How long a dispatched launch has to reach its executor's admission, in
/// milliseconds.
const DISPATCH_BUDGET_MS: u64 = 15_000;
/// Names one temporary file tries. Each is 128 random bits, so a second one
/// is needed only when another writer picked the same.
const TEMPORARY_ATTEMPTS: usize = 8;
/// In a launch's run folder, before its run record exists: when its executor
/// was last seen alive, right after it started a profile helper.
const ALIVE_FILE: &str = "executor-alive";

/// The one decision about a launch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "camelCase")]
pub enum LaunchRecord {
    Admitted(Box<AdmittedLaunch>),
    #[serde(rename_all = "camelCase")]
    Refused {
        code: RefusalCode,
        message: String,
        /// Unix milliseconds.
        at: u64,
    },
    /// The reconciler's: the expiry passed with no decision, so nothing may
    /// start under this id any more.
    Expired {
        /// Unix milliseconds.
        at: u64,
    },
}

/// A launch its executor admitted, and once its run ended, how.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdmittedLaunch {
    pub run_id: RunId,
    /// The service it runs, so a reconciler finds its run record.
    pub service: ServiceKey,
    pub executor: ExecutorIdentity,
    /// `null` for a launch that no paired machine asked for.
    pub requested_by: Option<RequestedBy>,
    /// Unix milliseconds.
    pub at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<LaunchExit>,
}

/// `localify dev` as it admitted the launch: its start in the boot's
/// monotonic time, and its session when it leads one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutorIdentity {
    pub pid: u32,
    pub start: u64,
    pub sid: Option<u32>,
    pub boot_id: String,
}

/// The paired machine and its request, from `--request`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestedBy {
    pub machine_id: String,
    pub request_id: String,
}

impl RequestedBy {
    /// `<machineId>:<requestId>`, each made of letters, digits, `.`, `_` and
    /// `-`, as a dispatcher passes it.
    pub fn parse(value: &str) -> Result<Self> {
        let valid = |part: &str| {
            (1..=128).contains(&part.len())
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        };
        match value.split_once(':') {
            Some((machine, request)) if valid(machine) && valid(request) => Ok(Self {
                machine_id: machine.to_string(),
                request_id: request.to_string(),
            }),
            _ => bail!(
                "--request '{value}' is not <machineId>:<requestId> (letters, digits, '.', '_' and '-')."
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefusalCode {
    /// The service runs, its port is taken, or its definition is not the
    /// one the dispatcher checked.
    Conflict,
    Expired,
    /// A profile a remote launch may not use: dangerous, unknown or removed.
    RefusedProfile,
}

/// How an admitted launch's run ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchExit {
    pub code: Option<i32>,
    pub signal: Option<i32>,
    pub reason: Option<ExitReason>,
    /// Unix milliseconds.
    pub at: u64,
    /// The launch that stopped it, for a restart.
    pub replaced_by: Option<RunId>,
}

impl LaunchExit {
    pub fn now(code: Option<i32>, signal: Option<i32>, reason: Option<ExitReason>) -> Self {
        Self {
            code,
            signal,
            reason,
            at: unix_ms_now(),
            replaced_by: None,
        }
    }
}

/// Why a run ended when its code does not say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitReason {
    /// The executor died between its admission and its run record.
    InterruptedBeforeRegistration,
    /// The run record could not be written, so nothing started.
    RegistrationFailed,
    /// Something failed after the admission, before the service started,
    /// such as a profile source.
    NotStarted,
    /// Stopped: Ctrl+C, `localify stop`, a restart.
    Stopped,
    /// The launch profile's `max_duration` elapsed.
    MaxDuration,
    /// The executor died before it recorded how its run ended, so another
    /// finisher did, without the code.
    ExecutorLost,
}

/// What publishing a decision found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Publication {
    Published,
    /// Another side decided first: this one obeys it.
    Decided(LaunchRecord),
}

/// What a reconciler reports about one launch: exactly one of these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchReport {
    /// No record yet, and the expiry has not passed.
    Unknown,
    /// Admitted, and its executor runs, but its run is not registered yet.
    Starting,
    Running,
    Exited {
        code: Option<i32>,
        signal: Option<i32>,
        reason: Option<ExitReason>,
    },
    Replaced {
        by: RunId,
    },
    Refused {
        code: RefusalCode,
        message: String,
    },
    Expired,
}

/// `services/launches/`.
pub struct LaunchRecords {
    dir: PathBuf,
}

impl LaunchRecords {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, id: &RunId) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    /// The launch's decision, or `None` while there is none.
    pub fn read(&self, id: &RunId) -> Result<Option<LaunchRecord>> {
        let path = self.path(id);
        match fs::read(&path) {
            Ok(content) => serde_json::from_slice(&content)
                .map(Some)
                .with_context(|| format!("'{}' is not a launch record", path.display())),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => {
                Err(error).with_context(|| format!("Failed to read '{}'", path.display()))
            }
        }
    }

    /// Publishes the launch's first decision, or returns the one already
    /// there, which wins. Either way it returns only once the folder naming
    /// that decision is synced.
    pub fn publish(&self, id: &RunId, record: &LaunchRecord) -> Result<Publication> {
        create_durable_dir(&self.dir)?;
        let path = self.path(id);
        let temporary = self.write_temporary(id, record)?;
        crate::infrastructure::test_pause::pause_at("launch-record-written");
        let linked = fs::hard_link(&temporary, &path);
        let _ = fs::remove_file(&temporary);
        match linked {
            Ok(()) => {
                sync_dir(&self.dir)?;
                Ok(Publication::Published)
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                // The winner may not have synced its name yet: obeyed only
                // once it is durable.
                sync_dir(&self.dir)?;
                let decided = self.read(id)?.with_context(|| {
                    format!("'{}' vanished right after it was found", path.display())
                })?;
                Ok(Publication::Decided(decided))
            }
            Err(error) => {
                Err(error).with_context(|| format!("Failed to publish '{}'", path.display()))
            }
        }
    }

    /// `expired`, once the launch's expiry has passed with no decision.
    /// Returns the decision that stands.
    pub fn expire(&self, id: &RunId) -> Result<LaunchRecord> {
        let expired = LaunchRecord::Expired { at: unix_ms_now() };
        Ok(match self.publish(id, &expired)? {
            Publication::Published => expired,
            Publication::Decided(decided) => decided,
        })
    }

    /// Adds how the run of an admitted launch ended, unless an exit is
    /// already there: the first finisher's stands. Nothing happens to a
    /// launch with no record, or one that was not admitted.
    pub fn add_exit(&self, id: &RunId, exit: LaunchExit) -> Result<()> {
        self.change_exit(id, |current| {
            if current.is_some() {
                return false;
            }
            *current = Some(exit);
            true
        })
    }

    /// Says which launch replaced this one's run. Called under the run lock
    /// by the launch that took the service over, so a replacing launch that
    /// lost never names itself. Its exit is added if no finisher added one
    /// yet.
    pub fn mark_replaced(&self, id: &RunId, by: &RunId) -> Result<()> {
        self.change_exit(id, |current| {
            let exit = current.get_or_insert_with(|| LaunchExit::now(None, None, None));
            if exit.replaced_by.is_some() {
                return false;
            }
            exit.replaced_by = Some(by.clone());
            true
        })
    }

    /// Changes an admitted record's exit, under the exit lock: written whole
    /// to a temporary file, synced, renamed over the record, and the folder
    /// synced.
    fn change_exit(
        &self,
        id: &RunId,
        change: impl FnOnce(&mut Option<LaunchExit>) -> bool,
    ) -> Result<()> {
        if !self.dir.is_dir() {
            return Ok(());
        }
        let _lock = self.lock_exits()?;
        let Some(LaunchRecord::Admitted(mut admitted)) = self.read(id)? else {
            return Ok(());
        };
        if !change(&mut admitted.exit) {
            return Ok(());
        }
        let temporary = self.write_temporary(id, &LaunchRecord::Admitted(admitted))?;
        let path = self.path(id);
        if let Err(error) = fs::rename(&temporary, &path) {
            let _ = fs::remove_file(&temporary);
            return Err(error).with_context(|| format!("Failed to write '{}'", path.display()));
        }
        sync_dir(&self.dir)
    }

    fn lock_exits(&self) -> Result<File> {
        let path = self.dir.join(EXIT_LOCK);
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("Failed to open '{}'", path.display()))?;
        file.lock()
            .with_context(|| format!("Failed to lock '{}'", path.display()))?;
        Ok(file)
    }

    /// The record, complete and synced, in a file of its own that no other
    /// writer picks.
    fn write_temporary(&self, id: &RunId, record: &LaunchRecord) -> Result<PathBuf> {
        self.write_temporary_named(id, record, temporary_suffix)
    }

    /// `write_temporary`, naming each try from `suffix`. A name another
    /// writer already holds is skipped, never written to or removed: this
    /// call removes only a file it created.
    fn write_temporary_named(
        &self,
        id: &RunId,
        record: &LaunchRecord,
        mut suffix: impl FnMut() -> Result<String>,
    ) -> Result<PathBuf> {
        let content = serde_json::to_vec_pretty(record)?;
        for _ in 0..TEMPORARY_ATTEMPTS {
            let temporary = self.dir.join(format!("{id}.json.{}.tmp", suffix()?));
            let mut file = match File::create_new(&temporary) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("Failed to create '{}'", temporary.display()));
                }
            };
            if let Err(error) = file.write_all(&content).and_then(|()| file.sync_all()) {
                let _ = fs::remove_file(&temporary);
                return Err(error)
                    .with_context(|| format!("Failed to write '{}'", temporary.display()));
            }
            return Ok(temporary);
        }
        bail!(
            "Failed to write a launch record in '{}': every temporary name tried was taken. Try again.",
            self.dir.display()
        )
    }

    /// Every record's id and path.
    fn entries(&self) -> Result<Vec<(RunId, PathBuf)>> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Failed to read '{}'", self.dir.display()));
            }
        };
        Ok(entries
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                let id = path
                    .file_name()?
                    .to_str()?
                    .strip_suffix(".json")
                    .and_then(|id| RunId::parse(id).ok())?;
                Some((id, path))
            })
            .collect())
    }

    /// Removes records not changed for 24 hours beyond the newest 100. Never
    /// one of the runs in `current`, which their services' records still
    /// name, nor an admission still without its exit: [`reconcile_unresolved`]
    /// completes those first. Each removal is decided again under the exit
    /// lock, from the record as it is then, so one that just got its exit
    /// stays.
    pub fn prune(&self, current: &HashSet<RunId>) -> Result<usize> {
        let mut removed = 0;
        for (id, path) in self.prune_candidates(current)? {
            if self.remove_if_expendable(&id, &path)? {
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// The records past the newest 100 and past their retention, as listed
    /// now.
    fn prune_candidates(&self, current: &HashSet<RunId>) -> Result<Vec<(RunId, PathBuf)>> {
        let mut records: Vec<(SystemTime, RunId, PathBuf)> = self
            .entries()?
            .into_iter()
            .filter(|(id, _)| !current.contains(id))
            .filter_map(|(id, path)| {
                let modified = fs::metadata(&path).ok()?.modified().ok()?;
                Some((modified, id, path))
            })
            .collect();
        records.sort_by_key(|record| std::cmp::Reverse(record.0));
        Ok(records
            .into_iter()
            .skip(KEEP_AT_LEAST)
            .filter(|(modified, _, _)| past_retention(*modified))
            .map(|(_, id, path)| (id, path))
            .collect())
    }

    /// Removes a record unless, read again under the exit lock, it changed
    /// since it was listed or still waits for its exit.
    fn remove_if_expendable(&self, id: &RunId, path: &Path) -> Result<bool> {
        let _lock = self.lock_exits()?;
        let old = fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .is_ok_and(past_retention);
        if !old {
            return Ok(false);
        }
        if let Ok(Some(LaunchRecord::Admitted(admitted))) = self.read(id)
            && admitted.exit.is_none()
        {
            return Ok(false);
        }
        Ok(fs::remove_file(path).is_ok())
    }
}

fn past_retention(modified: SystemTime) -> bool {
    SystemTime::now()
        .duration_since(modified)
        .is_ok_and(|age| age >= RETENTION)
}

/// 128 random bits, in hex.
fn temporary_suffix() -> Result<String> {
    let mut bytes = [0_u8; 16];
    random_bytes(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Makes the entries of `dir` durable: a link or a rename in it survives a
/// crash once this returns.
fn sync_dir(dir: &Path) -> Result<()> {
    #[cfg(test)]
    tests::record_sync(dir);
    File::open(dir)
        .and_then(|dir| dir.sync_all())
        .with_context(|| format!("Failed to sync '{}'", dir.display()))
}

/// Creates `dir` and its missing parents, each made durable in its parent.
fn create_durable_dir(dir: &Path) -> Result<()> {
    let mut missing = Vec::new();
    let mut ancestor = Some(dir);
    while let Some(path) = ancestor
        && !path.is_dir()
    {
        missing.push(path);
        ancestor = path.parent();
    }
    if missing.is_empty() {
        return Ok(());
    }
    fs::create_dir_all(dir).with_context(|| format!("Failed to create '{}'", dir.display()))?;
    for created in missing.iter().rev() {
        if let Some(parent) = created.parent() {
            sync_dir(parent)?;
        }
    }
    Ok(())
}

/// Removes the temporary files in `dir` older than `max_age`: what a crash
/// between writing and linking or renaming left. Returns how many went.
pub fn sweep_temporaries(dir: &Path, max_age: Duration) -> Result<usize> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => {
            return Err(error).with_context(|| format!("Failed to read '{}'", dir.display()));
        }
    };
    let now = SystemTime::now();
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "tmp") {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .is_ok_and(|modified| now.duration_since(modified).is_ok_and(|age| age >= max_age));
        if old && fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

/// The `--launch-expires` of a launch dispatched now: 15 seconds later. A
/// local dispatcher computes it once a replacement's stop phase is over, so a
/// slow stop never uses up the budget.
pub fn dispatch_expiry() -> u64 {
    unix_ms_now().saturating_add(DISPATCH_BUDGET_MS)
}

/// Unix milliseconds, the unit of `--launch-expires`.
pub fn unix_ms_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Records that this executor is alive now, right after it started a profile
/// helper and before its run record exists. A helper that started by then,
/// and so what it starts, is proven the launch's should the executor die
/// (docs/remote-machines.md, "The end of a run"). Written whole, renamed over
/// the last one; only the executor writes it.
pub fn record_alive(run_dir: &Path) -> Result<()> {
    let temporary = run_dir.join(format!("{ALIVE_FILE}.tmp"));
    fs::write(&temporary, process::monotonic_now().to_string())
        .and_then(|()| fs::rename(&temporary, run_dir.join(ALIVE_FILE)))
        .with_context(|| format!("Failed to write '{}'", temporary.display()))
}

fn read_alive(run_dir: &Path) -> Option<u64> {
    fs::read_to_string(run_dir.join(ALIVE_FILE))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// What happened to launch `id`, from its record, the run record of its
/// service, and its executor: exactly one [`LaunchReport`]. Once `expires_at`
/// (Unix milliseconds) has passed with no record, it publishes `expired`
/// first, so no run with that id can start from then on. An admitted launch
/// whose executor died is completed here: what it left is ended through the
/// identity-checked finisher, and its exit recorded.
pub fn reconcile(
    store: &RunningServiceStore,
    table: &dyn ProcessTable,
    id: &RunId,
    expires_at: Option<u64>,
) -> Result<LaunchReport> {
    let records = store.launches();
    let record = match records.read(id)? {
        Some(record) => record,
        None if expires_at.is_some_and(|expires| unix_ms_now() >= expires) => records.expire(id)?,
        None => return Ok(LaunchReport::Unknown),
    };
    let admitted = match record {
        LaunchRecord::Refused { code, message, .. } => {
            return Ok(LaunchReport::Refused { code, message });
        }
        LaunchRecord::Expired { .. } => return Ok(LaunchReport::Expired),
        LaunchRecord::Admitted(admitted) => admitted,
    };
    if let Some(exit) = &admitted.exit {
        return Ok(exited(exit));
    }
    if let Some(run) = named_run(store, &admitted)? {
        return run_report(store, &run, id);
    }
    let session = unregistered_session(store, &admitted);
    if session.executor_alive(table) {
        return Ok(LaunchReport::Starting);
    }
    // Dead, it can no longer register: a run record found now is final.
    let named = {
        let _record = store.lock_record(&admitted.service)?;
        named_run(store, &admitted)?
    };
    if let Some(run) = named {
        return run_report(store, &run, id);
    }
    end_unregistered(store, &admitted.run_id, &session)?;
    let reason = ExitReason::InterruptedBeforeRegistration;
    records.add_exit(id, LaunchExit::now(None, None, Some(reason)))?;
    recorded_exit(&records, id)
}

/// Reconciles every admission still without its exit, so one whose
/// executor died, before or after registering its run, has what it left
/// ended and its exit recorded: the daemon's part, each pass. One that cannot
/// be decided yet is tried again next time. Returns how many it completed.
pub fn reconcile_unresolved(
    store: &RunningServiceStore,
    table: &dyn ProcessTable,
) -> Result<usize> {
    let records = store.launches();
    let mut completed = 0;
    for (id, _) in records.entries()? {
        let Ok(Some(LaunchRecord::Admitted(admitted))) = records.read(&id) else {
            continue;
        };
        if admitted.exit.is_some() {
            continue;
        }
        if let Ok(LaunchReport::Exited { .. } | LaunchReport::Replaced { .. }) =
            reconcile(store, table, &id, None)
        {
            completed += 1;
        }
    }
    Ok(completed)
}

fn exited(exit: &LaunchExit) -> LaunchReport {
    match &exit.replaced_by {
        Some(by) => LaunchReport::Replaced { by: by.clone() },
        None => LaunchReport::Exited {
            code: exit.code,
            signal: exit.signal,
            reason: exit.reason,
        },
    }
}

/// The exit just recorded in launch `id`'s record.
fn recorded_exit(records: &LaunchRecords, id: &RunId) -> Result<LaunchReport> {
    match records.read(id)? {
        Some(LaunchRecord::Admitted(admitted)) => match &admitted.exit {
            Some(exit) => Ok(exited(exit)),
            None => bail!("The exit of launch {id} was not recorded; try again."),
        },
        _ => bail!("The record of launch {id} changed while it was completed; try again."),
    }
}

/// The service's record, while it names the launch's run.
fn named_run(
    store: &RunningServiceStore,
    admitted: &AdmittedLaunch,
) -> Result<Option<RunningService>> {
    Ok(store
        .get(&admitted.service)?
        .filter(|record| record.run_id() == Some(&admitted.run_id)))
}

/// The report of a launch whose run its service's record names. A run whose
/// executor died without recording how it ended is completed first, by the
/// identity-checked finisher, so it never reads `running` once nothing runs.
/// Its exit then has no code: only the executor knew it.
fn run_report(
    store: &RunningServiceStore,
    run: &RunningService,
    id: &RunId,
) -> Result<LaunchReport> {
    if run.exit.is_none() && !store.complete_dead_run(run)? {
        return Ok(LaunchReport::Running);
    }
    // Also for a finisher that died between the run record's exit and this
    // one's.
    let code = run.exit.as_ref().and_then(|exit| exit.code);
    let reason = code.is_none().then_some(ExitReason::ExecutorLost);
    let records = store.launches();
    records.add_exit(id, LaunchExit::now(code, None, reason))?;
    recorded_exit(&records, id)
}

/// The session of an executor that has no run record: what proves its
/// members is the last sign of life it gave after starting a helper, or else
/// its own start.
fn unregistered_session(store: &RunningServiceStore, admitted: &AdmittedLaunch) -> RunSession {
    let executor = &admitted.executor;
    let alive = read_alive(&store.run_dir(&admitted.run_id)).unwrap_or(executor.start);
    RunSession {
        executor_pid: executor.pid,
        executor_start: executor.start,
        sid: executor.sid,
        pgid: executor.sid,
        boot_id: executor.boot_id.clone(),
        last_alive_at: alive.max(executor.start),
    }
}

/// The session cleanup of "The end of a run", for an executor that died
/// between its admission and its run record: the profile helpers it started
/// there, proven by [`record_alive`], and what they started are ended by the
/// identity-checked finisher, in the run's folder, which is then removed, as
/// no run will use it. Without that folder, nothing ran. Something it cannot
/// confirm, or a run terminal the app still reads, keeps the launch open:
/// the next reconcile tries again.
fn end_unregistered(
    store: &RunningServiceStore,
    run_id: &RunId,
    session: &RunSession,
) -> Result<()> {
    let dir = store.run_dir(run_id);
    match Finisher::native().finish(&dir, session, true)? {
        Finish::Gone => return Ok(()),
        Finish::AlreadyFinalized | Finish::Finalized(_) => {}
        Finish::ExecutorAlive => bail!("The executor of launch {run_id} still runs; try again."),
        Finish::Busy => bail!(
            "The app still reads the terminal of launch {run_id}; try again once it closed it."
        ),
        Finish::Unconfirmed(pids) => bail!("{}", Finish::unconfirmed_message(&pids)),
    }
    fs::remove_dir_all(&dir).with_context(|| format!("Failed to remove '{}'", dir.display()))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::fs::FileTimes;
    use std::sync::{Arc, Barrier};
    use std::thread;

    use super::*;
    use crate::infrastructure::process::{NativeProcesses, process_info};
    use crate::infrastructure::running_services::{NewRun, ServiceOrigin};

    thread_local! {
        static SYNCED: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
    }

    /// Each folder this thread synced, in order.
    pub(super) fn record_sync(dir: &Path) {
        SYNCED.with(|synced| synced.borrow_mut().push(dir.to_path_buf()));
    }

    fn take_syncs() -> Vec<PathBuf> {
        SYNCED.with(|synced| std::mem::take(&mut *synced.borrow_mut()))
    }

    fn backdate(records: &LaunchRecords, id: &RunId, age: Duration) {
        File::options()
            .write(true)
            .open(records.path(id))
            .unwrap()
            .set_times(FileTimes::new().set_modified(SystemTime::now() - age))
            .unwrap();
    }

    const TWO_DAYS: Duration = Duration::from_secs(2 * 24 * 60 * 60);

    fn key() -> ServiceKey {
        ServiceKey {
            project_id: "app".into(),
            environment: Some("main".into()),
            service_id: "web".into(),
        }
    }

    /// An admitted launch whose executor is this test process, or, `alive`
    /// false, a process that started at another time and so is gone.
    fn admitted(id: &RunId, alive: bool) -> LaunchRecord {
        let me = process_info(std::process::id()).unwrap();
        LaunchRecord::Admitted(Box::new(AdmittedLaunch {
            run_id: id.clone(),
            service: key(),
            executor: ExecutorIdentity {
                pid: me.pid,
                start: if alive { me.start } else { me.start + 1 },
                sid: None,
                boot_id: crate::infrastructure::process::boot_id().unwrap(),
            },
            requested_by: Some(RequestedBy {
                machine_id: "machine-a".into(),
                request_id: "req-1".into(),
            }),
            at: unix_ms_now(),
            exit: None,
        }))
    }

    fn refused() -> LaunchRecord {
        LaunchRecord::Refused {
            code: RefusalCode::Conflict,
            message: "already running".into(),
            at: unix_ms_now(),
        }
    }

    fn sandbox() -> (tempfile::TempDir, RunningServiceStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = RunningServiceStore::new(dir.path().join("services"));
        (dir, store)
    }

    fn report(store: &RunningServiceStore, id: &RunId, expires_at: Option<u64>) -> LaunchReport {
        reconcile(store, &NativeProcesses, id, expires_at).unwrap()
    }

    fn register(store: &RunningServiceStore, id: &RunId) {
        let lock = store.lock(&key()).unwrap().admitted();
        store
            .register(
                &lock,
                NewRun {
                    key: key(),
                    run_id: id.clone(),
                    url: "https://web.localify".into(),
                    port: 3000,
                    origin: ServiceOrigin::Background,
                    profile: None,
                    output: None,
                },
            )
            .unwrap();
    }

    #[test]
    fn the_first_decision_wins_and_the_loser_reads_it() {
        let (_dir, store) = sandbox();
        let records = store.launches();
        let id = RunId::generate().unwrap();

        assert_eq!(records.read(&id).unwrap(), None);
        assert_eq!(
            records.expire(&id).unwrap(),
            records.read(&id).unwrap().unwrap()
        );
        let late = records.publish(&id, &admitted(&id, true)).unwrap();

        assert!(matches!(
            late,
            Publication::Decided(LaunchRecord::Expired { .. })
        ));
        assert!(matches!(
            records.read(&id).unwrap(),
            Some(LaunchRecord::Expired { .. })
        ));
    }

    #[test]
    fn of_simultaneous_publishers_exactly_one_wins_and_every_other_obeys_it() {
        let (_dir, store) = sandbox();
        let store = Arc::new(store);
        let id = RunId::generate().unwrap();
        let publishers = 8;
        let barrier = Arc::new(Barrier::new(publishers));
        let outcomes: Vec<(LaunchRecord, Publication)> = (0..publishers)
            .map(|index| {
                let (store, barrier, id) = (Arc::clone(&store), Arc::clone(&barrier), id.clone());
                thread::spawn(move || {
                    let record = LaunchRecord::Refused {
                        code: RefusalCode::Conflict,
                        message: format!("publisher {index}"),
                        at: unix_ms_now(),
                    };
                    barrier.wait();
                    let outcome = store.launches().publish(&id, &record).unwrap();
                    (record, outcome)
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|publisher| publisher.join().unwrap())
            .collect();

        let decided = store.launches().read(&id).unwrap().unwrap();
        let winners: Vec<&LaunchRecord> = outcomes
            .iter()
            .filter(|(_, outcome)| *outcome == Publication::Published)
            .map(|(record, _)| record)
            .collect();
        assert_eq!(winners, [&decided]);
        for (_, outcome) in &outcomes {
            if let Publication::Decided(seen) = outcome {
                assert_eq!(seen, &decided);
            }
        }
        // No temporary file is left behind.
        let left: Vec<_> = fs::read_dir(store.launches().dir())
            .unwrap()
            .flatten()
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "tmp"))
            .collect();
        assert!(left.is_empty(), "{left:?}");
    }

    #[test]
    fn a_decision_and_an_exit_count_only_once_their_folder_is_synced() {
        let (dir, store) = sandbox();
        let records = store.launches();
        let id = RunId::generate().unwrap();
        take_syncs();

        records.publish(&id, &admitted(&id, true)).unwrap();
        // The folders it created, each in its parent, then the record's name.
        assert_eq!(
            take_syncs(),
            [
                dir.path().to_path_buf(),
                dir.path().join("services"),
                records.dir().to_path_buf()
            ]
        );
        records
            .add_exit(&id, LaunchExit::now(Some(0), None, None))
            .unwrap();
        assert_eq!(take_syncs(), [records.dir().to_path_buf()]);
        // The loser makes the winner's name durable before it obeys it.
        assert!(matches!(
            records.expire(&id).unwrap(),
            LaunchRecord::Admitted(_)
        ));
        assert_eq!(take_syncs(), [records.dir().to_path_buf()]);
    }

    #[test]
    fn a_temporary_name_another_writer_holds_is_skipped_and_left_alone() {
        let (_dir, store) = sandbox();
        let records = store.launches();
        create_durable_dir(records.dir()).unwrap();
        let id = RunId::generate().unwrap();
        let taken = records.dir().join(format!("{id}.json.taken.tmp"));
        fs::write(&taken, "another writer's").unwrap();

        let mut names = ["taken", "fresh"].into_iter();
        let written = records
            .write_temporary_named(&id, &refused(), || Ok(names.next().unwrap().into()))
            .unwrap();
        assert_eq!(written, records.dir().join(format!("{id}.json.fresh.tmp")));
        assert_eq!(fs::read_to_string(&taken).unwrap(), "another writer's");

        // When every name tried is taken, it fails and removes nothing.
        let failed = records.write_temporary_named(&id, &refused(), || Ok("taken".into()));
        assert!(failed.is_err());
        assert_eq!(fs::read_to_string(&taken).unwrap(), "another writer's");
        // Real names never collide: 128 random bits each.
        assert_ne!(temporary_suffix().unwrap(), temporary_suffix().unwrap());
    }

    #[test]
    fn the_reconciler_obeys_an_admission_it_finds() {
        let (_dir, store) = sandbox();
        let id = RunId::generate().unwrap();
        let records = store.launches();
        assert_eq!(
            records.publish(&id, &admitted(&id, true)).unwrap(),
            Publication::Published
        );

        assert!(matches!(
            records.expire(&id).unwrap(),
            LaunchRecord::Admitted(_)
        ));
        assert_eq!(report(&store, &id, Some(0)), LaunchReport::Starting);
    }

    #[test]
    fn no_record_is_unknown_until_the_expiry_then_expired() {
        let (_dir, store) = sandbox();
        let id = RunId::generate().unwrap();

        assert_eq!(report(&store, &id, None), LaunchReport::Unknown);
        assert_eq!(
            report(&store, &id, Some(unix_ms_now() + 60_000)),
            LaunchReport::Unknown
        );
        assert_eq!(
            report(&store, &id, Some(unix_ms_now())),
            LaunchReport::Expired
        );
        assert!(matches!(
            store.launches().read(&id).unwrap(),
            Some(LaunchRecord::Expired { .. })
        ));
    }

    #[test]
    fn a_refusal_is_reported_with_its_code() {
        let (_dir, store) = sandbox();
        let id = RunId::generate().unwrap();
        store.launches().publish(&id, &refused()).unwrap();

        assert_eq!(
            report(&store, &id, Some(0)),
            LaunchReport::Refused {
                code: RefusalCode::Conflict,
                message: "already running".into()
            }
        );
    }

    #[test]
    fn a_temporary_file_is_never_a_decision_and_is_swept_once_old() {
        let (_dir, store) = sandbox();
        let records = store.launches();
        let id = RunId::generate().unwrap();
        fs::create_dir_all(records.dir()).unwrap();
        let temporary = records.dir().join(format!("{id}.json.42.7.tmp"));
        fs::write(
            &temporary,
            serde_json::to_vec(&admitted(&id, true)).unwrap(),
        )
        .unwrap();

        assert_eq!(records.read(&id).unwrap(), None);
        assert_eq!(report(&store, &id, Some(0)), LaunchReport::Expired);
        assert_eq!(
            sweep_temporaries(records.dir(), TEMPORARY_MAX_AGE).unwrap(),
            0
        );
        assert!(temporary.exists());
        assert_eq!(sweep_temporaries(records.dir(), Duration::ZERO).unwrap(), 1);
        assert!(!temporary.exists());
    }

    #[test]
    fn the_first_exit_stands_and_a_restart_names_its_launch() {
        let (_dir, store) = sandbox();
        let records = store.launches();
        let id = RunId::generate().unwrap();
        let next = RunId::generate().unwrap();
        records.publish(&id, &admitted(&id, true)).unwrap();

        records
            .add_exit(&id, LaunchExit::now(Some(3), None, None))
            .unwrap();
        records
            .add_exit(&id, LaunchExit::now(None, None, Some(ExitReason::Stopped)))
            .unwrap();
        assert_eq!(
            report(&store, &id, None),
            LaunchReport::Exited {
                code: Some(3),
                signal: None,
                reason: None
            }
        );

        records.mark_replaced(&id, &next).unwrap();
        let Some(LaunchRecord::Admitted(launch)) = records.read(&id).unwrap() else {
            panic!("not admitted");
        };
        let exit = launch.exit.unwrap();
        assert_eq!(
            (exit.code, exit.replaced_by.as_ref()),
            (Some(3), Some(&next))
        );
        assert_eq!(
            report(&store, &id, None),
            LaunchReport::Replaced { by: next }
        );
        assert!(launch.requested_by.is_some());
    }

    #[test]
    fn an_exit_changes_nothing_but_an_admitted_record() {
        let (_dir, store) = sandbox();
        let records = store.launches();
        let id = RunId::generate().unwrap();
        records.publish(&id, &refused()).unwrap();
        let before = records.read(&id).unwrap();

        records
            .add_exit(&id, LaunchExit::now(Some(0), None, None))
            .unwrap();
        records
            .mark_replaced(&id, &RunId::generate().unwrap())
            .unwrap();

        assert_eq!(records.read(&id).unwrap(), before);
        assert!(matches!(before, Some(LaunchRecord::Refused { .. })));
    }

    #[test]
    fn a_registered_run_is_running_until_its_exit() {
        let (_dir, store) = sandbox();
        let id = RunId::generate().unwrap();
        store
            .launches()
            .publish(&id, &admitted(&id, false))
            .unwrap();
        register(&store, &id);

        assert_eq!(report(&store, &id, None), LaunchReport::Running);
        store.mark_exited(&key(), &id, Some(1)).unwrap();
        assert_eq!(
            report(&store, &id, None),
            LaunchReport::Exited {
                code: Some(1),
                signal: None,
                reason: None
            }
        );
    }

    #[test]
    fn a_dead_executor_that_never_registered_is_interrupted_before_registration() {
        let (_dir, store) = sandbox();
        let id = RunId::generate().unwrap();
        store
            .launches()
            .publish(&id, &admitted(&id, false))
            .unwrap();
        // Another run holds the service's record.
        register(&store, &RunId::generate().unwrap());

        let expected = LaunchReport::Exited {
            code: None,
            signal: None,
            reason: Some(ExitReason::InterruptedBeforeRegistration),
        };
        assert_eq!(report(&store, &id, None), expected);
        assert_eq!(report(&store, &id, None), expected);
    }

    #[test]
    fn retention_keeps_a_day_the_newest_hundred_and_current_runs() {
        let (_dir, store) = sandbox();
        let records = store.launches();
        let two_days_ago = SystemTime::now() - Duration::from_secs(2 * 24 * 60 * 60);
        let mut old = Vec::new();
        for index in 0..110 {
            let id = RunId::generate().unwrap();
            records.publish(&id, &refused()).unwrap();
            let file = File::options()
                .write(true)
                .open(records.dir().join(format!("{id}.json")))
                .unwrap();
            let age = two_days_ago - Duration::from_secs(index);
            file.set_times(FileTimes::new().set_modified(age)).unwrap();
            old.push(id);
        }
        let young = RunId::generate().unwrap();
        records.publish(&young, &refused()).unwrap();
        let current: HashSet<RunId> = [old[109].clone()].into();

        // The young one and the 99 newest old ones are the newest hundred.
        assert_eq!(records.prune(&current).unwrap(), 10);
        assert!(records.read(&young).unwrap().is_some());
        assert!(records.read(&old[98]).unwrap().is_some());
        assert!(records.read(&old[99]).unwrap().is_none());
        assert!(records.read(&old[109]).unwrap().is_some());
    }

    #[test]
    fn retention_keeps_an_admission_until_it_has_its_exit() {
        let (_dir, store) = sandbox();
        let records = store.launches();
        for _ in 0..KEEP_AT_LEAST {
            let young = RunId::generate().unwrap();
            records.publish(&young, &refused()).unwrap();
        }
        // Admitted two days ago, never registered: one whose executor, this
        // process, is suspended or slow, and one whose executor died.
        let live = RunId::generate().unwrap();
        records.publish(&live, &admitted(&live, true)).unwrap();
        let dead = RunId::generate().unwrap();
        records.publish(&dead, &admitted(&dead, false)).unwrap();
        let old = RunId::generate().unwrap();
        records.publish(&old, &refused()).unwrap();
        for id in [&live, &dead, &old] {
            backdate(&records, id, TWO_DAYS);
        }

        assert_eq!(records.prune(&HashSet::new()).unwrap(), 1);
        assert!(records.read(&old).unwrap().is_none());
        assert!(records.read(&live).unwrap().is_some());
        assert!(records.read(&dead).unwrap().is_some());

        // The daemon's pass completes the dead one and leaves the live one,
        // which a reconciler still reports as starting, never expired.
        assert_eq!(reconcile_unresolved(&store, &NativeProcesses).unwrap(), 1);
        assert_eq!(report(&store, &live, Some(0)), LaunchReport::Starting);
        assert_eq!(
            report(&store, &dead, None),
            LaunchReport::Exited {
                code: None,
                signal: None,
                reason: Some(ExitReason::InterruptedBeforeRegistration)
            }
        );
        // Complete and old, it goes; the live one stays.
        backdate(&records, &dead, TWO_DAYS);
        assert_eq!(records.prune(&HashSet::new()).unwrap(), 1);
        assert!(records.read(&dead).unwrap().is_none());
        assert!(records.read(&live).unwrap().is_some());
    }

    #[test]
    fn pruning_decides_each_removal_again_under_the_exit_lock() {
        let (_dir, store) = sandbox();
        let records = store.launches();
        for _ in 0..KEEP_AT_LEAST {
            let young = RunId::generate().unwrap();
            records.publish(&young, &refused()).unwrap();
        }
        let changed = RunId::generate().unwrap();
        records
            .publish(&changed, &admitted(&changed, false))
            .unwrap();
        records
            .add_exit(&changed, LaunchExit::now(Some(0), None, None))
            .unwrap();
        let waiting = RunId::generate().unwrap();
        records.publish(&waiting, &refused()).unwrap();
        backdate(&records, &changed, TWO_DAYS);
        backdate(&records, &waiting, TWO_DAYS);
        let candidates = records.prune_candidates(&HashSet::new()).unwrap();
        assert_eq!(candidates.len(), 2);

        // A finisher changed one after it was listed: it stays.
        records
            .mark_replaced(&changed, &RunId::generate().unwrap())
            .unwrap();
        let (id, path) = candidates.iter().find(|(id, _)| *id == changed).unwrap();
        assert!(!records.remove_if_expendable(id, path).unwrap());
        assert!(records.read(&changed).unwrap().is_some());

        // A removal waits while an exit is being written.
        let held = records.lock_exits().unwrap();
        let (id, path) = candidates
            .into_iter()
            .find(|(id, _)| *id == waiting)
            .unwrap();
        let pruner = {
            let records = LaunchRecords::new(records.dir().to_path_buf());
            thread::spawn(move || records.remove_if_expendable(&id, &path).unwrap())
        };
        thread::sleep(Duration::from_millis(300));
        assert!(records.read(&waiting).unwrap().is_some());
        drop(held);
        assert!(pruner.join().unwrap());
        assert!(records.read(&waiting).unwrap().is_none());
    }

    #[test]
    fn a_request_names_a_machine_and_its_request() {
        assert_eq!(
            RequestedBy::parse("machine-A_1:req.2").unwrap(),
            RequestedBy {
                machine_id: "machine-A_1".into(),
                request_id: "req.2".into()
            }
        );
        for invalid in [
            "", "machine", ":req", "machine:", "a/b:c", "a:b:c", "a:../x",
        ] {
            assert!(RequestedBy::parse(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn records_read_as_the_contract_writes_them() {
        let id = RunId::generate().unwrap();
        let json = serde_json::to_value(admitted(&id, true)).unwrap();

        assert_eq!(json["outcome"], "admitted");
        assert_eq!(json["runId"], id.as_str());
        assert!(json["executor"]["bootId"].is_string());
        assert_eq!(json["requestedBy"]["machineId"], "machine-a");
        assert!(json.get("exit").is_none());
        let refusal = serde_json::to_value(LaunchRecord::Refused {
            code: RefusalCode::RefusedProfile,
            message: String::new(),
            at: 1,
        })
        .unwrap();
        assert_eq!(refusal["code"], "refused_profile");
        let exit = serde_json::to_value(LaunchExit::now(
            None,
            Some(9),
            Some(ExitReason::InterruptedBeforeRegistration),
        ))
        .unwrap();
        assert_eq!(exit["reason"], "interrupted_before_registration");
        assert_eq!(exit["replacedBy"], serde_json::Value::Null);
        let lost = serde_json::to_value(ExitReason::ExecutorLost).unwrap();
        assert_eq!(lost, "executor_lost");
    }
}
