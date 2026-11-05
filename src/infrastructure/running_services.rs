//! Services that `localify dev` started, wherever it runs: a person's terminal,
//! the desktop app's terminal, or the background. `status`, `logs`, `stop` and
//! `restart` act on them from any folder, for a person, the app or an agent.
//!
//! Each service has one small JSON file, rewritten on every start. It outlives
//! the process, so a service that stopped still shows its exit code and output.
//! A file says a service runs only while its PID still belongs to the process
//! that wrote it.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::branding::BRANDING;
use crate::infrastructure::launch_sessions::unix_now;
use crate::infrastructure::process::{
    ProcessControl, get_process_control, process_started_at, process_tree, signal_processes,
};

/// Where the output of a service started by `dev --detach` or the desktop app
/// goes. Set on the `localify dev` process by whoever starts it.
pub const SERVICE_OUTPUT_ENV: &str = "LOCALIFY_SERVICE_OUTPUT";
/// Who started `localify dev`: `app` or `background`. A terminal sets nothing.
pub const SERVICE_ORIGIN_ENV: &str = "LOCALIFY_SERVICE_ORIGIN";

const STOP_TIMEOUT: Duration = Duration::from_secs(10);
/// How much of the end of an output file `tail` reads.
const TAIL_WINDOW: u64 = 1024 * 1024;

/// Which service in which project environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceKey {
    pub project_id: String,
    pub environment: Option<String>,
    pub service_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ServiceOrigin {
    /// The desktop app's terminal panel.
    App,
    /// `localify dev --detach`.
    Background,
    /// A terminal Localify does not own.
    Terminal,
}

impl ServiceOrigin {
    pub fn as_env(self) -> &'static str {
        match self {
            Self::App => "app",
            Self::Background => "background",
            Self::Terminal => "terminal",
        }
    }

    pub fn from_env(value: Option<&str>) -> Self {
        match value {
            Some("app") => Self::App,
            Some("background") => Self::Background,
            _ => Self::Terminal,
        }
    }
}

/// How a run ended, when `localify dev` saw it end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceExit {
    /// Unix seconds.
    pub at: u64,
    /// `None` when the service was ended by a signal.
    pub code: Option<i32>,
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
    pub origin: ServiceOrigin,
    /// Launch profile, to restart the service on the same variables.
    pub profile: Option<String>,
    /// Unix seconds.
    pub started_at: u64,
    /// Output file. A service started in a terminal writes to that terminal only.
    pub log_file: Option<PathBuf>,
    pub exit: Option<ServiceExit>,
}

/// A run about to start, as `register` records it.
pub struct NewRun {
    pub key: ServiceKey,
    pub url: String,
    pub port: u16,
    pub origin: ServiceOrigin,
    pub profile: Option<String>,
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

    /// A new, empty output file for a service about to start, never one that
    /// another service already writes to.
    pub fn new_log_file(&self, owner: &str) -> Result<PathBuf> {
        let dir = self.dir.join("logs");
        fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create '{}'", dir.display()))?;
        let stamp = unix_now();
        for attempt in 0..1000 {
            let path = dir.join(format!("{stamp}-{owner}-{attempt}.log"));
            match File::create_new(&path) {
                Ok(_) => return Ok(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("Failed to create '{}'", path.display()));
                }
            }
        }
        bail!("Failed to create a new output file in '{}'", dir.display())
    }

    /// Records that the current process runs this service. Refuses while another
    /// run of the same service is alive. The previous run's output file is deleted.
    pub fn register(&self, run: NewRun) -> Result<()> {
        let pid = std::process::id();
        if let Some(previous) = self.get(&run.key)? {
            if previous.pid != pid && previous.is_running() {
                bail!(
                    "'{}' is already running (PID {}) at {}. Stop it first with: {} stop {}",
                    run.key.service_id,
                    previous.pid,
                    previous.url,
                    BRANDING.cli_command(),
                    service_selector(&run.key)
                );
            }
            if previous.log_file != run.log_file {
                remove_log(&previous);
            }
        }

        let record = RunningService {
            process_started_at: process_started_at(pid)
                .context("Failed to read this process's start time from ps")?,
            key: run.key,
            pid,
            url: run.url,
            port: run.port,
            origin: run.origin,
            profile: run.profile,
            started_at: unix_now(),
            log_file: run.log_file,
            exit: None,
        };
        self.write(&record)
    }

    /// Records how the current process's run ended. Does nothing when another
    /// run has taken the record over since.
    pub fn mark_exited(&self, key: &ServiceKey, code: Option<i32>) -> Result<()> {
        let Some(mut record) = self.get(key)? else {
            return Ok(());
        };
        if record.pid != std::process::id() {
            return Ok(());
        }
        record.exit = Some(ServiceExit {
            at: unix_now(),
            code,
        });
        self.write(&record)
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
        services.sort_by_key(|service| record_name(&service.key));
        Ok(services)
    }

    /// Whether any record still points at this output file.
    #[allow(dead_code)] // Used by the desktop app through the library crate.
    pub fn log_in_use(&self, log_file: &Path) -> Result<bool> {
        Ok(self
            .list()?
            .iter()
            .any(|service| service.log_file.as_deref() == Some(log_file)))
    }

    /// Stops the service if it runs, then forgets it and deletes its output.
    /// Returns whether a process was stopped.
    pub fn stop_and_forget(&self, key: &ServiceKey) -> Result<bool> {
        let Some(service) = self.get(key)? else {
            return Ok(false);
        };
        let stopped = stop(&service)?;
        remove_log(&service);
        let path = self.record_path(key);
        if path.exists() {
            fs::remove_file(&path)
                .with_context(|| format!("Failed to remove '{}'", path.display()))?;
        }
        Ok(stopped)
    }

    /// Writes through a temporary file, so a reader never sees half a record.
    fn write(&self, record: &RunningService) -> Result<()> {
        fs::create_dir_all(&self.dir)
            .with_context(|| format!("Failed to create '{}'", self.dir.display()))?;
        let path = self.record_path(&record.key);
        let temporary = path.with_extension(format!("json.{}", std::process::id()));
        fs::write(&temporary, serde_json::to_string_pretty(record)?)
            .with_context(|| format!("Failed to write '{}'", temporary.display()))?;
        fs::rename(&temporary, &path)
            .with_context(|| format!("Failed to write '{}'", path.display()))
    }

    fn record_path(&self, key: &ServiceKey) -> PathBuf {
        self.dir.join(format!("{}.json", record_name(key)))
    }
}

impl RunningService {
    /// Whether the process that wrote this record still runs.
    pub fn is_running(&self) -> bool {
        Tracked {
            pid: self.pid,
            started_at: self.process_started_at.clone(),
        }
        .is_alive()
    }
}

/// The arguments that name a service run exactly, from any folder.
pub fn service_selector(key: &ServiceKey) -> String {
    let mut selector = format!("{} --project {}", key.service_id, key.project_id);
    if let Some(environment) = &key.environment {
        selector.push_str(&format!(" --env {environment}"));
    }
    selector
}

/// A process known by its PID and start time, so a reused PID is never signaled.
struct Tracked {
    pid: u32,
    started_at: String,
}

impl Tracked {
    fn of(pid: u32) -> Option<Self> {
        process_started_at(pid).map(|started_at| Self { pid, started_at })
    }

    fn is_alive(&self) -> bool {
        get_process_control().process_exists(self.pid)
            && process_started_at(self.pid).as_deref() == Some(self.started_at.as_str())
    }
}

/// Asks the service and everything it started to stop, and kills what is still
/// alive after a grace period. Every process of the tree is watched, not only
/// `localify dev`: a server that ignores SIGTERM outlives its parent. Returns
/// false when the service was no longer running.
pub fn stop(service: &RunningService) -> Result<bool> {
    if !service.is_running() {
        return Ok(false);
    }
    let mut tracked = track_tree(service.pid, Vec::new());
    signal_processes(&pids(&tracked), "TERM");

    let deadline = Instant::now() + STOP_TIMEOUT;
    loop {
        tracked.retain(Tracked::is_alive);
        if tracked.is_empty() {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            break;
        }
        thread::sleep(Duration::from_millis(100));
        tracked = track_tree(service.pid, tracked);
    }

    signal_processes(&pids(&tracked), "KILL");
    thread::sleep(Duration::from_millis(300));
    tracked.retain(Tracked::is_alive);
    if !tracked.is_empty() {
        let left = pids(&tracked)
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(" ");
        bail!(
            "'{}' still has processes after SIGKILL: {left}. Stop them with: kill -9 {left}",
            service.key.service_id
        );
    }
    Ok(true)
}

/// Adds the descendants of every tracked process still alive, including ones
/// started since the last look.
fn track_tree(root: u32, mut tracked: Vec<Tracked>) -> Vec<Tracked> {
    let mut roots = vec![root];
    roots.extend(tracked.iter().map(|process| process.pid));
    for pid in roots.into_iter().flat_map(process_tree) {
        if !tracked.iter().any(|process| process.pid == pid)
            && let Some(process) = Tracked::of(pid)
        {
            tracked.push(process);
        }
    }
    tracked
}

fn pids(tracked: &[Tracked]) -> Vec<u32> {
    tracked.iter().map(|process| process.pid).collect()
}

/// Lines of a service's output, and where to read on from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputChunk {
    pub lines: Vec<String>,
    /// Byte offset to pass back to read only what comes next.
    pub cursor: u64,
}

/// The last `count` lines of an output file, as a terminal shows them.
pub fn tail(path: &Path, count: usize) -> Result<OutputChunk> {
    let length = file_length(path)?;
    let chunk = read_from(path, length.saturating_sub(TAIL_WINDOW))?;
    let start = chunk.lines.len().saturating_sub(count);
    Ok(OutputChunk {
        lines: chunk.lines[start..].to_vec(),
        cursor: chunk.cursor,
    })
}

/// The complete lines written at or after `cursor`. A line still being written
/// is left for the next read, so the cursor always ends on a line break.
pub fn read_from(path: &Path, cursor: u64) -> Result<OutputChunk> {
    let mut file =
        File::open(path).with_context(|| format!("Failed to read '{}'", path.display()))?;
    let length = file.metadata()?.len();
    // A cursor past the end means the file was replaced by a new run: start over.
    let start = if cursor > length { 0 } else { cursor };
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let complete = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |index| index + 1);
    let text = String::from_utf8_lossy(&bytes[..complete]);
    Ok(OutputChunk {
        lines: plain_lines(&text),
        cursor: start + complete as u64,
    })
}

fn file_length(path: &Path) -> Result<u64> {
    Ok(fs::metadata(path)
        .with_context(|| format!("Failed to read '{}'", path.display()))?
        .len())
}

/// Terminal output as plain lines: colors and cursor moves removed, and a line
/// redrawn with `\r` (progress bars) kept as its last state.
pub fn plain_lines(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\u{1b}' => {
                // Back to the line's start (spinners): what follows replaces it.
                if skip_escape(&mut chars) == Some('G') {
                    line.clear();
                }
            }
            '\n' => lines.push(std::mem::take(&mut line)),
            '\r' if chars.peek() == Some(&'\n') => {}
            '\r' => line.clear(),
            '\u{8}' => {
                line.pop();
            }
            '\t' => line.push(ch),
            ch if ch.is_control() => {}
            ch => line.push(ch),
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// Skips one escape sequence. Returns the final byte of a CSI sequence.
fn skip_escape(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Option<char> {
    match chars.next() {
        // CSI: parameters, then one final byte in @..~.
        Some('[') => chars.by_ref().find(|ch| ('@'..='~').contains(ch)),
        // OSC (titles, links): ends with BEL or ESC \.
        Some(']') => {
            while let Some(ch) = chars.next() {
                if ch == '\u{7}' {
                    break;
                }
                if ch == '\u{1b}' {
                    chars.next();
                    break;
                }
            }
            None
        }
        _ => None,
    }
}

fn read_record(path: &Path) -> Result<Option<RunningService>> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("Failed to read '{}'", path.display()));
        }
    };
    // A record from another version is as good as no record.
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
    use std::process::{Command, Stdio};

    use super::*;

    fn key(service: &str) -> ServiceKey {
        ServiceKey {
            project_id: "my-app".into(),
            environment: Some("feat-auth".into()),
            service_id: service.into(),
        }
    }

    fn run(service: &str, log_file: Option<PathBuf>) -> NewRun {
        NewRun {
            key: key(service),
            url: "https://web.localify".into(),
            port: 3000,
            origin: ServiceOrigin::Background,
            profile: None,
            log_file,
        }
    }

    fn record_for(pid: u32, log_file: Option<PathBuf>) -> RunningService {
        RunningService {
            key: key("web"),
            pid,
            process_started_at: process_started_at(pid).unwrap_or_else(|| "never".into()),
            url: "https://web.localify".into(),
            port: 3000,
            origin: ServiceOrigin::Background,
            profile: None,
            started_at: 0,
            log_file,
            exit: None,
        }
    }

    #[test]
    fn a_registered_service_runs_in_this_process() {
        let dir = tempfile::tempdir().unwrap();
        let store = RunningServiceStore::new(dir.path().to_path_buf());

        store.register(run("web", None)).unwrap();

        let service = store.get(&key("web")).unwrap().unwrap();
        assert_eq!(service.pid, std::process::id());
        assert!(service.is_running());
        assert_eq!(store.list().unwrap().len(), 1);
    }

    #[test]
    fn a_second_live_run_of_the_same_service_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let store = RunningServiceStore::new(dir.path().to_path_buf());
        let mut other = Command::new("sleep").arg("5").spawn().unwrap();
        store.write(&record_for(other.id(), None)).unwrap();

        let error = store.register(run("web", None)).unwrap_err();

        assert!(error.to_string().contains("already running"), "{error}");
        assert_eq!(store.get(&key("web")).unwrap().unwrap().pid, other.id());
        other.kill().unwrap();
        other.wait().unwrap();
    }

    #[test]
    fn a_record_whose_process_changed_is_not_running() {
        let dir = tempfile::tempdir().unwrap();
        let store = RunningServiceStore::new(dir.path().to_path_buf());
        store.register(run("web", None)).unwrap();
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
        store.register(run("web", Some(first_log.clone()))).unwrap();

        store.register(run("web", None)).unwrap();

        assert!(!first_log.exists());
    }

    #[test]
    fn the_exit_code_is_kept_after_the_run() {
        let dir = tempfile::tempdir().unwrap();
        let store = RunningServiceStore::new(dir.path().to_path_buf());
        store.register(run("web", None)).unwrap();

        store.mark_exited(&key("web"), Some(3)).unwrap();

        let exit = store.get(&key("web")).unwrap().unwrap().exit.unwrap();
        assert_eq!(exit.code, Some(3));
    }

    #[test]
    fn forgetting_a_service_that_ended_removes_its_record_and_output() {
        let dir = tempfile::tempdir().unwrap();
        let store = RunningServiceStore::new(dir.path().to_path_buf());
        let log = dir.path().join("web.log");
        fs::write(&log, "output").unwrap();
        store
            .write(&record_for(u32::MAX - 1, Some(log.clone())))
            .unwrap();

        assert!(!store.stop_and_forget(&key("web")).unwrap());

        assert!(store.get(&key("web")).unwrap().is_none());
        assert!(!log.exists());
    }

    #[test]
    fn stop_waits_for_a_child_that_ignores_sigterm() {
        let mut parent = Command::new("/bin/sh")
            .args(["-c", "sh -c 'trap \"\" TERM; sleep 30' & wait"])
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        thread::sleep(Duration::from_millis(300));
        let tree = process_tree(parent.id());
        assert!(tree.len() >= 2, "{tree:?}");

        assert!(stop(&record_for(parent.id(), None)).unwrap());

        let _ = parent.wait();
        for pid in tree {
            assert!(
                !Tracked::of(pid).is_some_and(|process| process.is_alive()),
                "{pid} survived"
            );
        }
    }

    #[test]
    fn output_reads_as_plain_lines() {
        let text = "\u{1b}[32mready\u{1b}[0m in 2s\r\n\u{1b}]0;title\u{7}50%\r100%\n⠙\u{1b}[1G\u{1b}[0Klast";

        assert_eq!(plain_lines(text), ["ready in 2s", "100%", "last"]);
    }

    #[test]
    fn a_cursor_reads_only_complete_new_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.log");
        fs::write(&path, "one\ntwo\nthr").unwrap();

        let first = read_from(&path, 0).unwrap();
        assert_eq!(first.lines, ["one", "two"]);

        fs::write(&path, "one\ntwo\nthree\nfour\n").unwrap();
        let next = read_from(&path, first.cursor).unwrap();
        assert_eq!(next.lines, ["three", "four"]);
        assert_eq!(read_from(&path, next.cursor).unwrap().lines.len(), 0);
    }

    #[test]
    fn each_new_output_file_is_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let store = RunningServiceStore::new(dir.path().to_path_buf());

        let first = store.new_log_file("app-1").unwrap();
        let second = store.new_log_file("app-1").unwrap();

        assert_ne!(first, second);
    }

    #[test]
    fn tail_keeps_the_last_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.log");
        fs::write(&path, "one\ntwo\nthree\n").unwrap();

        assert_eq!(tail(&path, 2).unwrap().lines, ["two", "three"]);
    }
}
