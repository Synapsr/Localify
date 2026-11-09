//! What the updater remembers between runs, in `updates.json`: the highest
//! version this machine installed, the archives and releases that failed
//! here, and when the last check and download ran, so a restart doesn't
//! reset the cooldowns.
//!
//! A missing file is a first use. A file that can't be read or holds
//! something invalid stops every install until it is deleted: reading it as
//! fresh would forget the version floor and the failed releases.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use semver::Version;
use serde::{Deserialize, Serialize};

use super::UpdatePaths;
use super::release::Release;
use super::rules::{self, parse_version};

/// Enough to cover every release a machine could fail on in a row.
const MAX_FAILED: usize = 20;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateState {
    #[serde(default)]
    pub highest_installed: Option<String>,
    /// SHA-256 of archives that failed verification here, newest last.
    #[serde(default)]
    pub failed_digests: Vec<String>,
    /// [`Release::identity`] of releases whose archive failed here, newest
    /// last: skipped before they are downloaded again.
    #[serde(default)]
    pub rejected_releases: Vec<String>,
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
    /// Unix seconds of the daemon's last download, until an install
    /// succeeds.
    #[serde(default)]
    pub last_download_at: Option<u64>,
    /// The bundle the last install replaced, when it couldn't be moved to
    /// `previous/`: it stayed in its staging directory.
    #[serde(default)]
    pub stranded_previous: Option<PathBuf>,
}

impl UpdateState {
    pub fn highest(&self) -> Option<Version> {
        self.highest_installed.as_deref().and_then(parse_version)
    }

    /// Raises the highest installed version to `version` if it is newer.
    pub fn saw_installed(&mut self, version: &Version) {
        if self
            .highest()
            .is_none_or(|highest| rules::is_newer(version, &highest))
        {
            self.highest_installed = Some(version.to_string());
        }
    }

    pub fn has_failed(&self, digest: &str) -> bool {
        self.failed_digests.iter().any(|failed| failed == digest)
    }

    pub fn record_failed(&mut self, digest: &str) {
        remember(&mut self.failed_digests, digest);
    }

    /// Whether the release with this [`Release::identity`] failed here.
    pub fn was_rejected(&self, identity: &str) -> bool {
        self.rejected_releases.iter().any(|seen| seen == identity)
    }

    pub fn record_rejected(&mut self, identity: &str) {
        remember(&mut self.rejected_releases, identity);
    }

    /// Records an unattended download starting now, unless one ran within
    /// the download cooldown: then `false`, and nothing is downloaded.
    pub fn start_download(&mut self, now: u64) -> bool {
        if !rules::download_is_due(now, self.last_download_at) {
            return false;
        }
        self.last_download_at = Some(now);
        true
    }

    /// What a file must hold to be trusted: a valid floor version, and
    /// SHA-256 digests.
    fn validate(&self) -> Result<(), String> {
        if let Some(highest) = &self.highest_installed
            && parse_version(highest).is_none()
        {
            return Err(format!("highestInstalled is not a version: {highest:?}"));
        }
        if let Some(latest) = &self.latest
            && parse_version(&latest.version).is_none()
        {
            return Err(format!(
                "latest.version is not a version: {:?}",
                latest.version
            ));
        }
        let digests = self.failed_digests.iter().chain(&self.rejected_releases);
        if let Some(bad) = digests.into_iter().find(|digest| !is_sha256_hex(digest)) {
            return Err(format!("{bad:?} is not a SHA-256 digest"));
        }
        Ok(())
    }
}

/// Adds `value` once, keeping the newest [`MAX_FAILED`].
fn remember(list: &mut Vec<String>, value: &str) {
    if !list.iter().any(|seen| seen == value) {
        list.push(value.to_string());
    }
    let excess = list.len().saturating_sub(MAX_FAILED);
    list.drain(..excess);
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Why `updates.json` stops installs, and the way out.
fn unreadable(path: &Path, detail: &str) -> anyhow::Error {
    anyhow!(
        "Updates are paused: {} can't be read ({detail}). It holds the oldest version this Mac \
         may install and the releases that failed here, so nothing installs without it. To \
         start again, delete it: the installed version then becomes the oldest one allowed.",
        path.display()
    )
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

    /// A missing file reads as a fresh state, the first use. A file that
    /// can't be read or holds anything invalid is an error, and callers
    /// install nothing.
    pub fn load(&self) -> Result<UpdateState> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(UpdateState::default());
            }
            Err(error) => return Err(unreadable(&self.path, &error.to_string())),
        };
        let state: UpdateState = serde_json::from_slice(&bytes)
            .map_err(|error| unreadable(&self.path, &error.to_string()))?;
        state
            .validate()
            .map_err(|detail| unreadable(&self.path, &detail))?;
        Ok(state)
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
        let mut state = self.load()?;
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
    let mut state = store.load()?;
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
        assert_eq!(StateStore::new(&paths).load().unwrap().failures, 2);
        // 40 minutes after the second failure.
        assert!(check(&paths, 2_200 + 2_399, || Ok(release("1.7.0"))).is_err());
        assert!(check(&paths, 2_200 + 2_400, || Ok(release("1.7.0"))).is_ok());
        let state = StateStore::new(&paths).load().unwrap();
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
            state.record_rejected(&format!("{index:02}"));
        }
        assert_eq!(state.failed_digests.len(), MAX_FAILED);
        assert!(!state.has_failed("aa"));
        assert!(state.has_failed("29"));
        assert_eq!(state.rejected_releases.len(), MAX_FAILED);
        assert!(state.was_rejected("29") && !state.was_rejected("09"));
    }

    #[test]
    fn the_highest_installed_version_only_rises() {
        let mut state = UpdateState::default();
        state.saw_installed(&Version::new(1, 7, 0));
        state.saw_installed(&Version::new(1, 6, 0));
        assert_eq!(state.highest(), Some(Version::new(1, 7, 0)));
    }

    #[test]
    fn the_highest_installed_version_ignores_build_metadata() {
        let mut state = UpdateState::default();
        state.saw_installed(&parse_version("1.7.0").unwrap());
        state.saw_installed(&parse_version("1.7.0+zz").unwrap());
        assert_eq!(state.highest_installed.as_deref(), Some("1.7.0"));
    }

    #[test]
    fn a_missing_state_file_is_a_first_use() {
        let (_dir, paths) = paths();
        assert_eq!(
            StateStore::new(&paths).load().unwrap(),
            UpdateState::default()
        );
    }

    fn write_state(paths: &UpdatePaths, bytes: &[u8]) {
        fs::create_dir_all(paths.dir()).unwrap();
        fs::write(paths.state_file(), bytes).unwrap();
    }

    #[test]
    fn a_corrupt_state_file_stops_everything_and_is_kept() {
        let (_dir, paths) = paths();
        write_state(&paths, b"{not json");
        let store = StateStore::new(&paths);
        let error = store.load().unwrap_err().to_string();
        assert!(error.contains("delete it"), "{error}");
        assert!(error.contains("updates.json"), "{error}");
        // Neither a check nor a change writes a fresh state over it.
        assert!(check(&paths, 1_000, || Ok(release("1.7.0"))).is_err());
        assert!(store.update(&paths, |state| state.failures = 0).is_err());
        assert_eq!(fs::read(paths.state_file()).unwrap(), b"{not json");
    }

    #[test]
    fn an_invalid_floor_or_digest_is_refused() {
        let (_dir, paths) = paths();
        for bytes in [
            &br#"{"highestInstalled": "seven"}"#[..],
            br#"{"latest": {"version": "1.7"}}"#,
            br#"{"failedDigests": ["aa"]}"#,
            br#"{"rejectedReleases": ["../x"]}"#,
        ] {
            write_state(&paths, bytes);
            assert!(StateStore::new(&paths).load().is_err());
        }
        write_state(&paths, br#"{"highestInstalled": "1.7.0"}"#);
        let state = StateStore::new(&paths).load().unwrap();
        assert_eq!(state.highest(), parse_version("1.7.0"));
    }

    #[test]
    fn an_unreadable_state_file_is_not_a_first_use() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, paths) = paths();
        write_state(&paths, b"{}");
        let file = paths.state_file();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).unwrap();
        let loaded = StateStore::new(&paths).load();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(loaded.is_err());
    }

    #[test]
    fn an_unattended_download_is_recorded_and_waits_out_its_cooldown() {
        let (_dir, paths) = paths();
        let store = StateStore::new(&paths);
        assert!(
            store
                .update(&paths, |state| state.start_download(1_000))
                .unwrap()
        );
        // Another process, or a restart, sees it.
        let again = store.update(&paths, |state| state.start_download(1_060));
        assert!(!again.unwrap());
        let later = store.update(&paths, |state| state.start_download(1_000 + 6 * 3600));
        assert!(later.unwrap());
    }
}
