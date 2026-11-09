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
//! its link is never a decision, and the daemon sweeps it after an hour.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use super::RunId;
use super::finish::{Membership, RunSession, membership};
use crate::infrastructure::process::ProcessTable;
use crate::infrastructure::running_services::{RunningServiceStore, ServiceKey};

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
    /// there, which wins.
    pub fn publish(&self, id: &RunId, record: &LaunchRecord) -> Result<Publication> {
        fs::create_dir_all(&self.dir)
            .with_context(|| format!("Failed to create '{}'", self.dir.display()))?;
        let path = self.path(id);
        let temporary = self.write_temporary(id, record)?;
        crate::infrastructure::test_pause::pause_at("launch-record-written");
        let linked = fs::hard_link(&temporary, &path);
        let _ = fs::remove_file(&temporary);
        match linked {
            Ok(()) => Ok(Publication::Published),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
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

    /// Says which launch stopped this one's run, for a restart. Its exit is
    /// added if no finisher added one yet.
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
    /// to a temporary file, synced, then renamed over the record.
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
        fs::rename(&temporary, &path).with_context(|| {
            let _ = fs::remove_file(&temporary);
            format!("Failed to write '{}'", path.display())
        })
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
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        let temporary = self
            .dir
            .join(format!("{id}.json.{}.{nanos}.tmp", std::process::id()));
        let written = File::create_new(&temporary).and_then(|mut file| {
            file.write_all(&serde_json::to_vec_pretty(record)?)?;
            file.sync_all()
        });
        if let Err(error) = written {
            let _ = fs::remove_file(&temporary);
            return Err(error)
                .with_context(|| format!("Failed to write '{}'", temporary.display()));
        }
        Ok(temporary)
    }

    /// Removes records older than 24 hours beyond the newest 100, except
    /// those of the runs in `current`, which their services' records still
    /// name.
    pub fn prune(&self, current: &HashSet<RunId>) -> Result<usize> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Failed to read '{}'", self.dir.display()));
            }
        };
        let mut records: Vec<(SystemTime, PathBuf)> = entries
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                let id = path
                    .file_name()?
                    .to_str()?
                    .strip_suffix(".json")
                    .and_then(|id| RunId::parse(id).ok())?;
                if current.contains(&id) {
                    return None;
                }
                Some((entry.metadata().ok()?.modified().ok()?, path))
            })
            .collect();
        records.sort_by_key(|record| std::cmp::Reverse(record.0));
        let now = SystemTime::now();
        let mut removed = 0;
        for (modified, path) in records.into_iter().skip(KEEP_AT_LEAST) {
            let old = now
                .duration_since(modified)
                .is_ok_and(|age| age >= RETENTION);
            if old && fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
        Ok(removed)
    }
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

/// Unix milliseconds, the unit of `--launch-expires`.
pub fn unix_ms_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// What happened to launch `id`, from its record, the run record of its
/// service, and its executor: exactly one [`LaunchReport`]. Once `expires_at`
/// (Unix milliseconds) has passed with no record, it publishes `expired`
/// first, so no run with that id can start from then on. An admitted launch
/// whose executor died before registering its run is completed here with
/// `interrupted_before_registration`.
#[allow(dead_code)] // Used by the control executor (PR 4c) and the tests, through the library crate.
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
    if let Some(report) = registered(store, &admitted)? {
        return Ok(report);
    }
    let session = RunSession {
        executor_pid: admitted.executor.pid,
        executor_start: admitted.executor.start,
        sid: admitted.executor.sid,
        pgid: admitted.executor.sid,
        boot_id: admitted.executor.boot_id.clone(),
        // Nothing heartbeats before the run record: only what started with
        // the executor could prove its session.
        last_alive_at: admitted.executor.start,
    };
    if session.executor_alive(table) {
        return Ok(LaunchReport::Starting);
    }
    // Dead, it can no longer register: a run record found now is final.
    let _record = store.lock_record(&admitted.service)?;
    if let Some(report) = registered(store, &admitted)? {
        return Ok(report);
    }
    end_unregistered(table, &session)?;
    let reason = ExitReason::InterruptedBeforeRegistration;
    records.add_exit(id, LaunchExit::now(None, None, Some(reason)))?;
    match records.read(id)? {
        Some(LaunchRecord::Admitted(admitted)) => match &admitted.exit {
            Some(exit) => Ok(exited(exit)),
            None => bail!("The exit of launch {id} was not recorded; try again."),
        },
        _ => bail!("The record of launch {id} changed while it was completed; try again."),
    }
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

/// The report of a launch whose run its service's record names.
fn registered(
    store: &RunningServiceStore,
    admitted: &AdmittedLaunch,
) -> Result<Option<LaunchReport>> {
    let Some(record) = store.get(&admitted.service)? else {
        return Ok(None);
    };
    if record.run_id() != Some(&admitted.run_id) {
        return Ok(None);
    }
    Ok(Some(match &record.exit {
        Some(exit) => LaunchReport::Exited {
            code: exit.code,
            signal: None,
            reason: None,
        },
        None => LaunchReport::Running,
    }))
}

/// The session cleanup of "The end of a run", for an executor that died
/// before its run record. Nothing heartbeats before that record, so only a
/// process that started with the executor could prove its session: the
/// cleanup finds the session unproven and signals nothing, as a finisher never
/// signals what it cannot prove. The profile helpers it was running, the only
/// processes it starts in that window, are left to end on their own. A session
/// it could not decide on keeps the launch open: the next reconcile tries
/// again.
fn end_unregistered(table: &dyn ProcessTable, session: &RunSession) -> Result<()> {
    if session.sid.is_none() {
        return Ok(());
    }
    match membership(table, session) {
        Membership::OtherBoot | Membership::Unproven => Ok(()),
        Membership::Proven(members) if members.is_empty() => Ok(()),
        Membership::Proven(_) | Membership::Inconclusive => bail!(
            "The session of a launch whose executor died before registering is still alive; try again."
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::fs::FileTimes;

    use super::*;
    use crate::infrastructure::process::{NativeProcesses, process_info};
    use crate::infrastructure::running_services::{NewRun, ServiceOrigin};

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
    }
}
