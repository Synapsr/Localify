//! Request entries (`docs/remote-machines.md`, "Request entries"): every
//! control from a peer leaves `services/requests/<peer>/<requestId>.json`,
//! published before the target changes anything, with an immutable
//! fingerprint of what it asked. Each outcome is linked to it as it happens:
//! the launch it dispatched, the run it stopped, how it ended. A control that
//! comes back with the same request id is never run again: it gets the
//! entry's report, and `control.status` asks for that report on its own.
//! Entries are kept for 24 hours, so the answer survives a daemon restart.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use super::wire::{
    Control, ControlAction, ControlOutcome, ControlResult, ControlService, is_request_id,
};
use crate::infrastructure::launch_rev::random_bytes;
use crate::infrastructure::runs::RunId;

/// How long an entry answers for its request.
pub const RETENTION: Duration = Duration::from_secs(24 * 60 * 60);

/// What a control asked, which no later update changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Fingerprint {
    pub action: ControlAction,
    pub service: ControlService,
    pub profile: Option<String>,
    pub expect_run_id: Option<String>,
    pub expect_config_rev: String,
    pub expect_legacy_id: Option<String>,
}

impl Fingerprint {
    pub fn of(control: &Control) -> Self {
        Self {
            action: control.action,
            service: control.service.clone(),
            profile: control.profile.clone(),
            expect_run_id: control.expect.run_id.clone(),
            expect_config_rev: control.expect.config_rev.clone(),
            expect_legacy_id: control.expect.legacy_id.clone(),
        }
    }
}

/// Who a launch was handed to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Via {
    App,
    Background,
}

/// The launch a control dispatched, whose record says what became of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkedLaunch {
    pub id: RunId,
    /// Unix milliseconds.
    pub expires_at: u64,
    pub via: Via,
    /// The port its run listens on, which settling watches.
    pub port: u16,
}

/// The run a stop, or a restart's stop, ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stopped {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_id: Option<String>,
    /// Unix milliseconds.
    pub at: u64,
}

/// What a control's request came to, or where it stands: a `control.result`
/// without the request it answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub outcome: ControlOutcome,
    pub code: Option<String>,
    pub message: String,
    pub run_id: Option<String>,
}

impl Report {
    pub fn new(outcome: ControlOutcome, code: Option<&str>, message: impl Into<String>) -> Self {
        Self {
            outcome,
            code: code.map(str::to_string),
            message: message.into(),
            run_id: None,
        }
    }

    /// A refusal with its code: `conflict` has an outcome of its own.
    pub fn refused(code: &str, message: impl Into<String>) -> Self {
        let outcome = if code == "conflict" {
            ControlOutcome::Conflict
        } else {
            ControlOutcome::Refused
        };
        Self::new(outcome, Some(code), message)
    }

    pub fn with_run(mut self, run_id: Option<String>) -> Self {
        self.run_id = run_id;
        self
    }

    /// Whether nothing can change it any more: `starting` and `unknown`
    /// are worked out again each time.
    pub fn is_final(&self) -> bool {
        !matches!(
            self.outcome,
            ControlOutcome::Starting | ControlOutcome::Unknown
        )
    }

    pub fn answer(&self, re: &str) -> ControlResult {
        ControlResult {
            re: re.to_string(),
            outcome: self.outcome,
            code: self.code.clone(),
            message: self.message.clone(),
            run_id: self.run_id.clone(),
        }
    }
}

/// One control's entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestEntry {
    pub fingerprint: Fingerprint,
    /// Unix milliseconds.
    pub at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch: Option<LinkedLaunch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped: Option<Stopped>,
    /// How it ended, once nothing can change it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Report>,
}

impl RequestEntry {
    pub fn new(fingerprint: Fingerprint, at: u64) -> Self {
        Self {
            fingerprint,
            at,
            launch: None,
            stopped: None,
            result: None,
        }
    }
}

/// What publishing an entry found.
#[derive(Debug)]
pub enum Published {
    New,
    /// The request already has one: this is it.
    Existing(Box<RequestEntry>),
}

/// `services/requests/`.
#[derive(Debug, Clone)]
pub struct RequestEntries {
    dir: PathBuf,
}

impl RequestEntries {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// `<services dir>/requests`.
    pub fn in_services_dir(services: &Path) -> Self {
        Self::new(services.join("requests"))
    }

    fn path(&self, peer: &str, request_id: &str) -> Result<PathBuf> {
        if !is_peer_id(peer) || !is_request_id(request_id) {
            bail!("'{peer}:{request_id}' does not name a request.");
        }
        Ok(self.dir.join(peer).join(format!("{request_id}.json")))
    }

    /// The request's entry, while it is kept: an entry past its 24 hours
    /// is no longer one, swept or not.
    pub fn read(&self, peer: &str, request_id: &str) -> Result<Option<RequestEntry>> {
        let path = self.path(peer, request_id)?;
        let content = match fs::read(&path) {
            Ok(content) => content,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).with_context(|| format!("Failed to read '{}'", path.display()));
            }
        };
        if is_past_retention(&path) {
            return Ok(None);
        }
        serde_json::from_slice(&content)
            .map(Some)
            .with_context(|| format!("'{}' is not a request entry", path.display()))
    }

    /// Publishes the request's first entry, whole, or returns the one
    /// already there: written to a temporary file, synced, then linked to
    /// its name, which fails when an entry exists. Entries past their
    /// retention are swept first, so an old one never stands in the way.
    pub fn publish(&self, peer: &str, request_id: &str, entry: &RequestEntry) -> Result<Published> {
        let _ = self.sweep();
        let path = self.path(peer, request_id)?;
        let dir = path.parent().context("A request entry has a folder")?;
        fs::create_dir_all(dir).with_context(|| format!("Failed to create '{}'", dir.display()))?;
        let temporary = write_temporary(dir, request_id, entry)?;
        let linked = fs::hard_link(&temporary, &path);
        let _ = fs::remove_file(&temporary);
        match linked {
            Ok(()) => {
                sync_dir(dir)?;
                Ok(Published::New)
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                match self.read(peer, request_id)? {
                    Some(existing) => Ok(Published::Existing(Box::new(existing))),
                    // Past its retention: replaced by this one.
                    None => {
                        fs::remove_file(&path)
                            .with_context(|| format!("Failed to remove '{}'", path.display()))?;
                        self.publish(peer, request_id, entry)
                    }
                }
            }
            Err(error) => {
                Err(error).with_context(|| format!("Failed to publish '{}'", path.display()))
            }
        }
    }

    /// Links an outcome to the entry, through a temporary file renamed over
    /// it. The fingerprint never changes; a result already there stands.
    pub fn update(
        &self,
        peer: &str,
        request_id: &str,
        change: impl FnOnce(&mut RequestEntry),
    ) -> Result<RequestEntry> {
        let path = self.path(peer, request_id)?;
        let Some(mut entry) = self.read(peer, request_id)? else {
            bail!("Request {request_id} has no entry to update.");
        };
        let (fingerprint, result) = (entry.fingerprint.clone(), entry.result.clone());
        change(&mut entry);
        entry.fingerprint = fingerprint;
        if result.is_some() {
            entry.result = result;
        }
        let dir = path.parent().context("A request entry has a folder")?;
        let temporary = write_temporary(dir, request_id, &entry)?;
        fs::rename(&temporary, &path)
            .with_context(|| format!("Failed to write '{}'", path.display()))?;
        sync_dir(dir)?;
        Ok(entry)
    }

    /// Removes the entries past their retention. Returns how many went.
    pub fn sweep(&self) -> Result<usize> {
        let peers = match fs::read_dir(&self.dir) {
            Ok(peers) => peers,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Failed to read '{}'", self.dir.display()));
            }
        };
        let mut removed = 0;
        for peer in peers.flatten() {
            let Ok(entries) = fs::read_dir(peer.path()) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let stale = path
                    .extension()
                    .is_some_and(|extension| extension == "json")
                    && is_past_retention(&path)
                    || path.extension().is_some_and(|extension| extension == "tmp")
                        && is_older_than(&path, Duration::from_secs(3600));
                if stale && fs::remove_file(&path).is_ok() {
                    removed += 1;
                }
            }
        }
        Ok(removed)
    }
}

/// A machine id as a folder name: letters, digits, `-` and `_`.
fn is_peer_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

fn is_past_retention(path: &Path) -> bool {
    is_older_than(path, RETENTION)
}

fn is_older_than(path: &Path, age: Duration) -> bool {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .is_ok_and(|modified| {
            SystemTime::now()
                .duration_since(modified)
                .is_ok_and(|elapsed| elapsed >= age)
        })
}

/// The entry, complete and synced, in a file of its own.
fn write_temporary(dir: &Path, request_id: &str, entry: &RequestEntry) -> Result<PathBuf> {
    let content = serde_json::to_vec_pretty(entry)?;
    let mut suffix = [0_u8; 16];
    random_bytes(&mut suffix)?;
    let suffix: String = suffix.iter().map(|byte| format!("{byte:02x}")).collect();
    let temporary = dir.join(format!("{request_id}.json.{suffix}.tmp"));
    let mut file = File::create_new(&temporary)
        .with_context(|| format!("Failed to create '{}'", temporary.display()))?;
    if let Err(error) = file.write_all(&content).and_then(|()| file.sync_all()) {
        let _ = fs::remove_file(&temporary);
        return Err(error).with_context(|| format!("Failed to write '{}'", temporary.display()));
    }
    Ok(temporary)
}

fn sync_dir(dir: &Path) -> Result<()> {
    File::open(dir)
        .and_then(|dir| dir.sync_all())
        .with_context(|| format!("Failed to sync '{}'", dir.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::remote::wire::ControlExpect;

    const REQUEST: &str = "AAAAAAAAAAAAAAAAAAAAAA";

    fn control(action: ControlAction) -> Control {
        Control {
            id: "c1".into(),
            action,
            service: ControlService {
                project_id: "app".into(),
                environment: None,
                service_id: "web".into(),
            },
            profile: None,
            request_id: REQUEST.into(),
            expect: ControlExpect {
                run_id: Some("BBBBBBBBBBBBBBBBBBBBBB".into()),
                config_rev: "0123456789abcdef".into(),
                legacy_id: None,
            },
            expires_at: "2026-11-09T10:00:00Z".into(),
        }
    }

    fn entry(action: ControlAction) -> RequestEntry {
        RequestEntry::new(Fingerprint::of(&control(action)), 1)
    }

    #[test]
    fn the_first_entry_of_a_request_stands() {
        let dir = tempfile::tempdir().unwrap();
        let entries = RequestEntries::new(dir.path().to_path_buf());

        assert!(matches!(
            entries
                .publish("mA", REQUEST, &entry(ControlAction::Restart))
                .unwrap(),
            Published::New
        ));
        let Published::Existing(existing) = entries
            .publish("mA", REQUEST, &entry(ControlAction::Stop))
            .unwrap()
        else {
            panic!("a second entry replaced the first");
        };
        assert_eq!(existing.fingerprint.action, ControlAction::Restart);
        // Another peer's request of the same id is its own.
        assert!(matches!(
            entries
                .publish("mB", REQUEST, &entry(ControlAction::Stop))
                .unwrap(),
            Published::New
        ));
    }

    #[test]
    fn updates_keep_the_fingerprint_and_the_first_result() {
        let dir = tempfile::tempdir().unwrap();
        let entries = RequestEntries::new(dir.path().to_path_buf());
        entries
            .publish("mA", REQUEST, &entry(ControlAction::Stop))
            .unwrap();

        entries
            .update("mA", REQUEST, |entry| {
                entry.fingerprint.action = ControlAction::Start;
                entry.result = Some(Report::new(ControlOutcome::Done, None, "Stopped."));
            })
            .unwrap();
        let updated = entries
            .update("mA", REQUEST, |entry| {
                entry.result = Some(Report::refused("conflict", "Late."));
            })
            .unwrap();

        assert_eq!(updated.fingerprint.action, ControlAction::Stop);
        assert_eq!(updated.result.unwrap().outcome, ControlOutcome::Done);
        // Read back as a restarted daemon reads it.
        let read = RequestEntries::new(dir.path().to_path_buf())
            .read("mA", REQUEST)
            .unwrap()
            .unwrap();
        assert_eq!(read.fingerprint.action, ControlAction::Stop);
    }

    #[test]
    fn names_that_are_not_ids_never_reach_the_file_system() {
        let dir = tempfile::tempdir().unwrap();
        let entries = RequestEntries::new(dir.path().to_path_buf());

        for (peer, request) in [
            ("..", REQUEST),
            ("mA", "../../../etc/passwd"),
            ("a/b", REQUEST),
        ] {
            assert!(entries.read(peer, request).is_err(), "{peer} {request}");
            assert!(
                entries
                    .publish(peer, request, &entry(ControlAction::Stop))
                    .is_err()
            );
        }
    }

    #[test]
    fn an_entry_past_its_retention_is_no_longer_one() {
        let dir = tempfile::tempdir().unwrap();
        let entries = RequestEntries::new(dir.path().to_path_buf());
        entries
            .publish("mA", REQUEST, &entry(ControlAction::Stop))
            .unwrap();
        let path = dir.path().join("mA").join(format!("{REQUEST}.json"));
        let old = SystemTime::now() - RETENTION - Duration::from_secs(1);
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(old)
            .unwrap();

        assert!(entries.read("mA", REQUEST).unwrap().is_none());
        assert_eq!(entries.sweep().unwrap(), 1);
        assert!(!path.exists());
    }
}
