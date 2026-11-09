//! What the updater remembers between runs, in `updates.json`: the highest
//! version this machine installed, the archives that failed here, and when
//! the last check ran, so a restart doesn't reset the cooldown.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use semver::Version;
use serde::{Deserialize, Serialize};

use super::UpdatePaths;
use super::release::Release;
use super::rules::{self, parse_version};

/// Enough to cover every release a machine could fail on in a row.
const MAX_FAILED_DIGESTS: usize = 20;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateState {
    #[serde(default)]
    pub highest_installed: Option<String>,
    /// SHA-256 of archives that failed verification here, newest last.
    #[serde(default)]
    pub failed_digests: Vec<String>,
    /// Unix seconds.
    #[serde(default)]
    pub last_check_at: Option<u64>,
    /// Failed checks in a row.
    #[serde(default)]
    pub failures: u32,
    #[serde(default)]
    pub last_error: Option<String>,
    /// What the last successful check read.
    #[serde(default)]
    pub latest: Option<Release>,
    /// Why the last install attempt didn't install, until one succeeds.
    #[serde(default)]
    pub install_error: Option<String>,
}

impl UpdateState {
    pub fn highest(&self) -> Option<Version> {
        self.highest_installed.as_deref().and_then(parse_version)
    }

    /// Raises the highest installed version to `version` if it is higher.
    pub fn saw_installed(&mut self, version: &Version) {
        if self.highest().is_none_or(|highest| version > &highest) {
            self.highest_installed = Some(version.to_string());
        }
    }

    pub fn has_failed(&self, digest: &str) -> bool {
        self.failed_digests.iter().any(|failed| failed == digest)
    }

    pub fn record_failed(&mut self, digest: &str) {
        if !self.has_failed(digest) {
            self.failed_digests.push(digest.to_string());
        }
        let excess = self.failed_digests.len().saturating_sub(MAX_FAILED_DIGESTS);
        self.failed_digests.drain(..excess);
    }
}

/// `updates.json`, read and written whole.
pub struct StateStore {
    path: PathBuf,
}

impl StateStore {
    pub fn new(paths: &UpdatePaths) -> Self {
        Self {
            path: paths.state_file(),
        }
    }

    /// A missing or unreadable file reads as a fresh state: the worst that
    /// costs is one early check.
    pub fn load(&self) -> UpdateState {
        fs::read(&self.path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Writes a temporary file and renames it over, so a reader never sees
    /// half a file.
    pub fn save(&self, state: &UpdateState) -> Result<()> {
        let dir = self
            .path
            .parent()
            .ok_or_else(|| anyhow!("The update state has no directory"))?;
        fs::create_dir_all(dir).with_context(|| format!("Could not create {}", dir.display()))?;
        let temporary = self
            .path
            .with_extension(format!("json.{}", std::process::id()));
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temporary)
            .with_context(|| format!("Could not write {}", temporary.display()))?;
        file.write_all(&serde_json::to_vec_pretty(state)?)?;
        file.sync_all()?;
        fs::rename(&temporary, &self.path)
            .with_context(|| format!("Could not write {}", self.path.display()))
    }

    /// Reads, changes and writes the state under the check lock, so two
    /// processes don't lose each other's change.
    pub fn update<T>(
        &self,
        paths: &UpdatePaths,
        change: impl FnOnce(&mut UpdateState) -> T,
    ) -> Result<T> {
        let _lock = lock(paths)?;
        let mut state = self.load();
        let answer = change(&mut state);
        self.save(&state)?;
        Ok(answer)
    }
}

/// Whether a check reached the network or answered from the last one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Checked {
    Fetched(Release),
    /// Within the cooldown of a successful check: its release.
    Recent(Option<Release>),
}

impl Checked {
    pub fn release(&self) -> Option<&Release> {
        match self {
            Self::Fetched(release) => Some(release),
            Self::Recent(release) => release.as_ref(),
        }
    }
}

/// The one check every caller goes through: scheduled, hinted or asked by a
/// person, in the daemon or the CLI. It holds a machine-wide lock, so
/// concurrent callers wait for one check and share its answer, and it
/// reaches the network only once the cooldown has passed. Within the
/// cooldown of a failed check it answers that failure.
pub fn check(
    paths: &UpdatePaths,
    now: u64,
    fetch: impl FnOnce() -> Result<Release>,
) -> Result<Checked> {
    let store = StateStore::new(paths);
    let _lock = lock(paths)?;
    let mut state = store.load();
    if !rules::check_is_due(now, state.last_check_at, state.failures) {
        return match (&state.last_error, state.failures) {
            (Some(error), failures) if failures > 0 => Err(anyhow!(
                "{error}\nThe next check is allowed {} minutes after the last one.",
                rules::cooldown(failures).as_secs() / 60
            )),
            _ => Ok(Checked::Recent(state.latest)),
        };
    }
    state.last_check_at = Some(now);
    let result = fetch();
    match &result {
        Ok(release) => {
            state.failures = 0;
            state.last_error = None;
            state.latest = Some(release.clone());
        }
        Err(error) => {
            state.failures = state.failures.saturating_add(1);
            state.last_error = Some(format!("{error:#}"));
        }
    }
    store.save(&state)?;
    result.map(Checked::Fetched)
}

/// The check lock, a file never deleted, so every process locks the same
/// inode.
fn lock(paths: &UpdatePaths) -> Result<File> {
    let path = paths.check_lock();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("Could not create {}", dir.display()))?;
    }
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&path)
        .with_context(|| format!("Could not open {}", path.display()))?;
    file.lock()
        .with_context(|| format!("Could not lock {}", path.display()))?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    fn release(version: &str) -> Release {
        Release {
            version: version.into(),
            archive: None,
        }
    }

    fn paths() -> (tempfile::TempDir, UpdatePaths) {
        let dir = tempfile::tempdir().unwrap();
        let paths = UpdatePaths::new(dir.path().to_path_buf());
        (dir, paths)
    }

    #[test]
    fn checks_within_the_cooldown_share_the_last_answer() {
        let (_dir, paths) = paths();
        let fetches = Cell::new(0);
        let fetch = || {
            fetches.set(fetches.get() + 1);
            Ok(release("1.7.0"))
        };
        assert_eq!(
            check(&paths, 1_000, fetch).unwrap(),
            Checked::Fetched(release("1.7.0"))
        );
        let again = check(&paths, 1_000 + 599, || {
            fetches.set(fetches.get() + 1);
            Ok(release("9.9.9"))
        })
        .unwrap();
        assert_eq!(again, Checked::Recent(Some(release("1.7.0"))));
        assert_eq!(fetches.get(), 1);

        let later = check(&paths, 1_000 + 600, || Ok(release("1.7.1"))).unwrap();
        assert_eq!(later, Checked::Fetched(release("1.7.1")));
    }

    #[test]
    fn failures_double_the_cooldown_and_survive_a_restart() {
        let (_dir, paths) = paths();
        assert!(check(&paths, 1_000, || Err(anyhow!("offline"))).is_err());
        // A new process reads the persisted failure: 20 minutes now.
        let early = check(&paths, 1_000 + 1_199, || Ok(release("1.7.0")));
        let error = early.unwrap_err().to_string();
        assert!(error.contains("offline"), "{error}");
        assert!(check(&paths, 1_000 + 1_200, || Err(anyhow!("offline"))).is_err());
        assert_eq!(StateStore::new(&paths).load().failures, 2);
        // 40 minutes after the second failure.
        assert!(check(&paths, 2_200 + 2_399, || Ok(release("1.7.0"))).is_err());
        assert!(check(&paths, 2_200 + 2_400, || Ok(release("1.7.0"))).is_ok());
        let state = StateStore::new(&paths).load();
        assert_eq!((state.failures, state.last_error), (0, None));
    }

    #[test]
    fn concurrent_checks_coalesce_into_one_fetch() {
        let (_dir, paths) = paths();
        let fetches = std::sync::atomic::AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(|| {
                    check(&paths, 5_000, || {
                        fetches.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        std::thread::sleep(std::time::Duration::from_millis(50));
                        Ok(release("1.7.0"))
                    })
                    .unwrap()
                });
            }
        });
        assert_eq!(fetches.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn failed_digests_are_remembered_and_bounded() {
        let mut state = UpdateState::default();
        state.record_failed("aa");
        state.record_failed("aa");
        assert_eq!(state.failed_digests, vec!["aa".to_string()]);
        for index in 0..30 {
            state.record_failed(&format!("{index:02}"));
        }
        assert_eq!(state.failed_digests.len(), MAX_FAILED_DIGESTS);
        assert!(!state.has_failed("aa"));
        assert!(state.has_failed("29"));
    }

    #[test]
    fn the_highest_installed_version_only_rises() {
        let mut state = UpdateState::default();
        state.saw_installed(&Version::new(1, 7, 0));
        state.saw_installed(&Version::new(1, 6, 0));
        assert_eq!(state.highest(), Some(Version::new(1, 7, 0)));
    }

    #[test]
    fn a_corrupt_state_file_reads_as_fresh() {
        let (_dir, paths) = paths();
        fs::create_dir_all(paths.dir()).unwrap();
        fs::write(paths.state_file(), b"{not json").unwrap();
        assert_eq!(StateStore::new(&paths).load(), UpdateState::default());
    }
}
