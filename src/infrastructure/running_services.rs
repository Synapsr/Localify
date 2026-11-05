//! Services that `localify dev` started, so they can be stopped and their output
//! read without the terminal that started them. Agents rely on this: they start
//! services in the background and have no terminal to press Ctrl+C in.
//!
//! Each service has one small JSON file, rewritten on every start. It outlives the
//! process so the output of a service that crashed can still be read. A file says a
//! service runs only while its PID still belongs to the process that wrote it.

use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::branding::BRANDING;
use crate::infrastructure::process::{
    ProcessControl, get_process_control, process_started_at, signal_process_tree,
};

/// Set by `localify dev --detach` on the process it starts in the background:
/// where that process's output goes.
pub const DETACHED_LOG_ENV: &str = "LOCALIFY_DETACHED_LOG";

const STOP_TIMEOUT: Duration = Duration::from_secs(10);

/// Which service in which project environment. `environment` is `None` for a
/// project that has no environments yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceKey {
    pub project_id: String,
    pub environment: Option<String>,
    pub service_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunningService {
    #[serde(flatten)]
    pub key: ServiceKey,
    pub pid: u32,
    /// The start time `ps` gave for `pid`, to tell it from a later process with the same PID.
    pub process_started_at: String,
    pub url: String,
    /// Loopback port the service listens on.
    pub port: u16,
    /// Output file of a service started with `--detach`. A service started in a
    /// terminal writes to that terminal.
    pub log_file: Option<PathBuf>,
}

pub struct RunningServiceStore {
    dir: PathBuf,
}

impl RunningServiceStore {
    pub fn new_default() -> Result<Self> {
        let dir = BRANDING
            .user_state_dir()
            .context("HOME is not set, so Localify cannot track running services.")?;
        Ok(Self::new(dir.join("services")))
    }

    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// Where `dev --detach` writes a new service's output.
    pub fn new_log_file(&self) -> Result<PathBuf> {
        let dir = self.dir.join("logs");
        fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create '{}'", dir.display()))?;
        let stamp = crate::infrastructure::launch_sessions::unix_now();
        Ok(dir.join(format!("{stamp}-{}.log", std::process::id())))
    }

    /// Records that the current process runs this service. The output file of the
    /// service's previous run, if any, is deleted.
    pub fn register(
        &self,
        key: ServiceKey,
        url: String,
        port: u16,
        log_file: Option<PathBuf>,
    ) -> Result<()> {
        let pid = std::process::id();
        let process_started_at =
            process_started_at(pid).context("Failed to read this process's start time from ps")?;
        if let Some(previous) = self.get(&key)?
            && previous.log_file != log_file
        {
            remove_log(&previous);
        }

        let record = RunningService {
            key,
            pid,
            process_started_at,
            url,
            port,
            log_file,
        };
        fs::create_dir_all(&self.dir)
            .with_context(|| format!("Failed to create '{}'", self.dir.display()))?;
        let path = self.record_path(&record.key);
        let content = serde_json::to_string_pretty(&record)?;
        fs::write(&path, content).with_context(|| format!("Failed to write '{}'", path.display()))
    }

    /// The last run of this service, running or not.
    pub fn get(&self, key: &ServiceKey) -> Result<Option<RunningService>> {
        read_record(&self.record_path(key))
    }

    /// Every service with a record, running or not.
    pub fn list(&self) -> Result<Vec<RunningService>> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Failed to read '{}'", self.dir.display()));
            }
        };
        let mut services = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path
                .extension()
                .is_some_and(|extension| extension == "json")
                && let Some(service) = read_record(&path)?
            {
                services.push(service);
            }
        }
        services.sort_by(|left, right| record_name(&left.key).cmp(&record_name(&right.key)));
        Ok(services)
    }

    /// Stops the service if it runs, then forgets it and deletes its output.
    /// Returns whether a process was stopped.
    pub fn stop_and_forget(&self, key: &ServiceKey) -> Result<bool> {
        let Some(service) = self.get(key)? else {
            return Ok(false);
        };
        let stopped = service.is_running() && stop(&service)?;
        remove_log(&service);
        let path = self.record_path(key);
        if path.exists() {
            fs::remove_file(&path)
                .with_context(|| format!("Failed to remove '{}'", path.display()))?;
        }
        Ok(stopped)
    }

    fn record_path(&self, key: &ServiceKey) -> PathBuf {
        self.dir.join(format!("{}.json", record_name(key)))
    }
}

impl RunningService {
    /// Whether the process that wrote this record still runs.
    pub fn is_running(&self) -> bool {
        get_process_control().process_exists(self.pid)
            && process_started_at(self.pid).as_deref() == Some(self.process_started_at.as_str())
    }
}

/// Asks the service and everything it started to stop, then kills what is left
/// after a grace period. Returns false when the service was no longer running.
pub fn stop(service: &RunningService) -> Result<bool> {
    if !service.is_running() {
        return Ok(false);
    }
    signal_process_tree(service.pid, "TERM");
    let deadline = Instant::now() + STOP_TIMEOUT;
    while service.is_running() {
        if Instant::now() >= deadline {
            signal_process_tree(service.pid, "KILL");
            thread::sleep(Duration::from_millis(200));
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    if service.is_running() {
        anyhow::bail!(
            "Service '{}' (PID {}) is still running after SIGKILL. Stop it with: kill -9 {}",
            service.key.service_id,
            service.pid,
            service.pid
        );
    }
    Ok(true)
}

/// The last `lines` lines of a file, for showing why a service stopped.
pub fn tail(path: &Path, lines: usize) -> Result<String> {
    let content = fs::read(path).with_context(|| format!("Failed to read '{}'", path.display()))?;
    let content = String::from_utf8_lossy(&content);
    let all = content.lines().collect::<Vec<_>>();
    let start = all.len().saturating_sub(lines);
    Ok(all[start..].join("\n"))
}

fn read_record(path: &Path) -> Result<Option<RunningService>> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("Failed to read '{}'", path.display()));
        }
    };
    // A record from another version, or cut short, is as good as no record.
    Ok(serde_json::from_str(&content).ok())
}

fn remove_log(service: &RunningService) {
    if let Some(log_file) = &service.log_file {
        let _ = fs::remove_file(log_file);
    }
}

fn record_name(key: &ServiceKey) -> String {
    format!(
        "{}--{}--{}",
        file_safe(&key.project_id),
        file_safe(key.environment.as_deref().unwrap_or("default")),
        file_safe(&key.service_id)
    )
}

fn file_safe(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(service: &str) -> ServiceKey {
        ServiceKey {
            project_id: "my-app".into(),
            environment: Some("feat-auth".into()),
            service_id: service.into(),
        }
    }

    #[test]
    fn a_registered_service_runs_in_this_process() {
        let dir = tempfile::tempdir().unwrap();
        let store = RunningServiceStore::new(dir.path().to_path_buf());

        store
            .register(key("web"), "https://web.localify".into(), 3000, None)
            .unwrap();

        let service = store.get(&key("web")).unwrap().unwrap();
        assert_eq!(service.pid, std::process::id());
        assert!(service.is_running());
        assert_eq!(store.list().unwrap().len(), 1);
    }

    #[test]
    fn a_record_whose_process_changed_is_not_running() {
        let dir = tempfile::tempdir().unwrap();
        let store = RunningServiceStore::new(dir.path().to_path_buf());
        store
            .register(key("web"), "https://web.localify".into(), 3000, None)
            .unwrap();
        let mut service = store.get(&key("web")).unwrap().unwrap();

        service.process_started_at = "Thu Jan  1 00:00:00 1970".into();

        assert!(!service.is_running());
    }

    #[test]
    fn a_new_run_deletes_the_previous_output() {
        let dir = tempfile::tempdir().unwrap();
        let store = RunningServiceStore::new(dir.path().to_path_buf());
        let first_log = dir.path().join("first.log");
        fs::write(&first_log, "old output").unwrap();
        store
            .register(
                key("web"),
                "https://web.localify".into(),
                3000,
                Some(first_log.clone()),
            )
            .unwrap();

        store
            .register(key("web"), "https://web.localify".into(), 3000, None)
            .unwrap();

        assert!(!first_log.exists());
    }

    #[test]
    fn forgetting_a_service_that_ended_removes_its_record_and_output() {
        let dir = tempfile::tempdir().unwrap();
        let store = RunningServiceStore::new(dir.path().to_path_buf());
        let log = dir.path().join("web.log");
        fs::write(&log, "output").unwrap();
        let record = RunningService {
            key: key("web"),
            pid: u32::MAX - 1,
            process_started_at: "never".into(),
            url: "https://web.localify".into(),
            port: 3000,
            log_file: Some(log.clone()),
        };
        fs::write(
            store.record_path(&record.key),
            serde_json::to_string(&record).unwrap(),
        )
        .unwrap();

        assert!(!store.stop_and_forget(&key("web")).unwrap());

        assert!(store.get(&key("web")).unwrap().is_none());
        assert!(!log.exists());
    }

    #[test]
    fn tail_keeps_the_last_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.log");
        fs::write(&path, "one\ntwo\nthree\n").unwrap();

        assert_eq!(tail(&path, 2).unwrap(), "two\nthree");
    }
}
