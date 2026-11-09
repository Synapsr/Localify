//! Automatic updates (ADR 0034, docs/desktop.md, "Automatic updates"): the
//! checks, the status every client reads, and, on macOS, Localify's one
//! installer. Linux only learns that a version exists and says so.

// The installer's parts (extraction, signatures, staging) are called only by
// the macOS installer, but build everywhere so their tests run on Linux too.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

pub mod extract;
#[cfg(target_os = "macos")]
pub mod install;
#[cfg(target_os = "macos")]
pub mod macos;
pub mod release;
pub mod rules;
pub mod state;

use std::collections::hash_map::RandomState;
use std::fs::OpenOptions;
use std::hash::{BuildHasher, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use semver::Version;
use serde::{Deserialize, Serialize};

use self::rules::parse_version;
use self::state::{StateStore, UpdateState};
use crate::branding::{BRANDING, user_home_dir};
use crate::infrastructure::config::ConfigStore;

/// What Linux shows: its CLI is checked by SHA-256 only, so it never
/// installs by itself.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub const LINUX_REASON: &str = "Update available: run the install command again";

/// The updater's files. On macOS they live in the account that owns the
/// bundle, at a path that doesn't depend on the configurable data dir.
#[derive(Debug, Clone)]
pub struct UpdatePaths {
    dir: PathBuf,
}

impl UpdatePaths {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// `~/Library/Application Support/Localify` on macOS. `None` when
    /// `HOME` is unset.
    pub fn for_user() -> Option<Self> {
        if cfg!(target_os = "macos") {
            return user_home_dir().map(|home| {
                Self::new(
                    home.join("Library/Application Support")
                        .join(BRANDING.app_name),
                )
            });
        }
        Some(Self::new(BRANDING.data_dir().join("updates")))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The app holds an exclusive `flock` on it for its lifetime.
    pub fn app_lock(&self) -> PathBuf {
        self.dir.join("app.lock")
    }

    /// Held while a bundle is swapped. Never deleted.
    pub fn update_lock(&self) -> PathBuf {
        self.dir.join("update.lock")
    }

    /// Held while a check runs, so checks coalesce.
    pub fn check_lock(&self) -> PathBuf {
        self.dir.join("check.lock")
    }

    pub fn state_file(&self) -> PathBuf {
        self.dir.join("updates.json")
    }

    /// Holds the bundle the last install replaced.
    pub fn previous_dir(&self) -> PathBuf {
        self.dir.join("previous")
    }

    #[cfg(test)]
    pub fn staging_root(&self) -> PathBuf {
        self.dir.join("staging")
    }

    pub fn log_file(&self) -> PathBuf {
        self.dir.join("updater.log")
    }
}

/// The lock the app takes as its first step and holds for its lifetime,
/// so the installer never replaces it while it runs.
#[allow(dead_code)] // Used by the desktop through the library crate.
pub fn app_lock_path() -> Option<PathBuf> {
    UpdatePaths::for_user().map(|paths| paths.app_lock())
}

/// Appends a line to `updater.log`. Logging never fails an update.
pub fn log(paths: &UpdatePaths, message: &str) {
    let _ = std::fs::create_dir_all(paths.dir());
    let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths.log_file())
    else {
        return;
    };
    let _ = writeln!(file, "{} {message}", unix_now());
}

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// Not cryptographic: spreads schedules and names staging directories.
pub(crate) fn random_u64() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| u64::from(elapsed.subsec_nanos()));
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u64(nanos);
    hasher.finish()
}

/// `[updates] automatic`. A config that can't be read counts as off.
pub fn automatic(config_path: &Path) -> bool {
    ConfigStore::new(config_path.to_path_buf())
        .load()
        .is_ok_and(|config| config.updates.automatic)
}

pub fn set_automatic(config_path: &Path, on: bool) -> Result<()> {
    let store = ConfigStore::new(config_path.to_path_buf());
    let mut config = store.load()?;
    config.updates.automatic = on;
    store.save(&config)?;
    Ok(())
}

/// The version installed here: the app bundle's when this binary runs from
/// one, otherwise this binary's own.
pub fn installed_version() -> Version {
    #[cfg(target_os = "macos")]
    if let Some(version) = macos::running_bundle_version() {
        return version;
    }
    parse_version(env!("CARGO_PKG_VERSION")).unwrap_or_else(|| Version::new(0, 0, 0))
}

/// What `update_status` and `localify update --status` answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateStatus {
    pub installed: String,
    pub available: Option<String>,
    pub state: UpdatePhase,
    /// Why an available update isn't installing by itself, or why the last
    /// check failed.
    pub reason: Option<String>,
    pub automatic: bool,
}

/// `state` in [`UpdateStatus`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UpdatePhase {
    /// No check has run yet.
    Unknown,
    UpToDate,
    Available,
    Checking,
    Installing,
    /// The last check failed.
    Failed,
}

/// What the daemon is doing now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    Idle,
    Checking,
    Installing,
}

/// The status from what is known: `blocker` is why this machine can't
/// install by itself (a destination rule, or Linux).
pub fn describe(
    installed: &Version,
    state: &UpdateState,
    automatic: bool,
    blocker: Option<String>,
    activity: Activity,
) -> UpdateStatus {
    let available = state
        .latest
        .as_ref()
        .and_then(|release| parse_version(&release.version))
        .filter(|offered| rules::offer_is_newer(offered, installed, state.highest().as_ref()));
    let stranded = || {
        state
            .stranded_previous
            .as_deref()
            .filter(|at| at.exists())
            .map(stranded_reason)
    };
    let (phase, reason) = match activity {
        Activity::Checking => (UpdatePhase::Checking, None),
        Activity::Installing => (UpdatePhase::Installing, None),
        Activity::Idle if available.is_some() => (
            UpdatePhase::Available,
            blocker
                .or_else(|| state.install_error.clone())
                .or_else(|| (!automatic).then(auto_off_reason))
                .or_else(stranded),
        ),
        Activity::Idle if state.failures > 0 => (UpdatePhase::Failed, state.last_error.clone()),
        Activity::Idle if state.last_check_at.is_none() => (UpdatePhase::Unknown, stranded()),
        Activity::Idle => (UpdatePhase::UpToDate, stranded()),
    };
    UpdateStatus {
        installed: installed.to_string(),
        available: available.map(|version| version.to_string()),
        state: phase,
        reason,
        automatic,
    }
}

/// The status when `updates.json` can't be trusted: nothing installs, and
/// `error` says how to start again.
pub fn describe_unreadable(
    installed: &Version,
    automatic: bool,
    error: &anyhow::Error,
) -> UpdateStatus {
    UpdateStatus {
        installed: installed.to_string(),
        available: None,
        state: UpdatePhase::Failed,
        reason: Some(format!("{error:#}")),
        automatic,
    }
}

/// Where the bundle the last install replaced is, when it isn't in
/// `previous/`.
pub fn stranded_reason(at: &Path) -> String {
    format!(
        "The version the last update replaced could not be moved to previous/. It is at {}: \
         keep it to go back to that version, or delete it.",
        at.display()
    )
}

fn auto_off_reason() -> String {
    format!(
        "Automatic updates are off. Install it with: {0} update install, or turn them on with: {0} update --auto on",
        BRANDING.cli_command()
    )
}

/// The daemon's side: its checks, its unattended installs, and the status
/// it answers on the management socket.
pub struct Updater {
    paths: UpdatePaths,
    config_path: PathBuf,
    endpoint: Option<String>,
    /// Only a daemon launchd restarts on the new build installs.
    may_install: bool,
    activity: Mutex<Activity>,
}

impl Updater {
    pub fn new(paths: UpdatePaths, config_path: PathBuf, may_install: bool) -> Self {
        Self {
            paths,
            config_path,
            endpoint: release::updater_endpoint(),
            may_install,
            activity: Mutex::new(Activity::Idle),
        }
    }

    /// A build without an updater endpoint, such as a dev build, never
    /// checks.
    pub fn checks(&self) -> bool {
        self.endpoint.is_some()
    }

    pub fn status(&self) -> UpdateStatus {
        let activity = self
            .activity
            .lock()
            .map_or(Activity::Idle, |activity| *activity);
        let installed = installed_version();
        let automatic = automatic(&self.config_path);
        match StateStore::new(&self.paths).load() {
            Ok(state) => describe(&installed, &state, automatic, blocker(), activity),
            Err(error) => describe_unreadable(&installed, automatic, &error),
        }
    }

    pub fn set_automatic(&self, on: bool) -> Result<UpdateStatus> {
        set_automatic(&self.config_path, on)?;
        log(&self.paths, &format!("automatic={on}"));
        Ok(self.status())
    }

    /// One scheduled or hinted check, then, when this Mac may and the app
    /// is closed, an unattended install. Blocking: the daemon runs it on a
    /// blocking thread.
    pub fn cycle(&self) {
        let Some(endpoint) = &self.endpoint else {
            return;
        };
        let checked = {
            let _activity = self.set_activity(Activity::Checking);
            state::check(&self.paths, unix_now(), || {
                release::fetch_manifest(endpoint)
            })
        };
        let release = match checked {
            Ok(checked) => checked.release().cloned(),
            Err(error) => {
                log(&self.paths, &format!("check failed: {error:#}"));
                return;
            }
        };
        #[cfg(target_os = "macos")]
        if let Some(release) = release
            && self.may_install
        {
            self.install_unattended(&release);
        }
        #[cfg(not(target_os = "macos"))]
        let _ = (release, self.may_install);
    }

    #[cfg(target_os = "macos")]
    fn install_unattended(&self, release: &release::Release) {
        let automatic = || automatic(&self.config_path);
        let attempt = || -> Result<Option<install::Installed>> {
            // Why it can't is in the status already.
            let Ok(target) = macos::InstallTarget::current() else {
                return Ok(None);
            };
            let installer = install::Installer {
                paths: &self.paths,
                target,
                public_key: release::updater_public_key(),
                signature: &macos::Codesign,
                processes: &macos::process_paths,
                automatic: &automatic,
            };
            // The app checks run again under the lock.
            if !automatic() || !install::may_download_unattended(&installer, release, unix_now())? {
                return Ok(None);
            }
            let _activity = self.set_activity(Activity::Installing);
            let url = installer_url(release)?;
            let (archive, _digest) = release::download_archive(url)?;
            let staged = installer.stage(release, &archive)?;
            installer
                .replace(staged, install::Mode::Unattended)
                .map(Some)
        };
        let result = attempt();
        record_install(&self.paths, &result);
    }

    fn set_activity(&self, activity: Activity) -> ActivityGuard<'_> {
        if let Ok(mut current) = self.activity.lock() {
            *current = activity;
        }
        ActivityGuard(&self.activity)
    }
}

/// Sets the activity back to idle when the step ends, however it ends.
struct ActivityGuard<'a>(&'a Mutex<Activity>);

impl Drop for ActivityGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut current) = self.0.lock() {
            *current = Activity::Idle;
        }
    }
}

#[cfg(target_os = "macos")]
fn installer_url(release: &release::Release) -> Result<&str> {
    release
        .archive
        .as_ref()
        .map(|archive| archive.url.as_str())
        .ok_or_else(|| anyhow::anyhow!("The update manifest has no archive for this Mac"))
}

/// Logs an install's outcome and keeps its error for the status.
#[cfg(target_os = "macos")]
pub fn record_install(paths: &UpdatePaths, result: &Result<Option<install::Installed>>) {
    let error = match result {
        Ok(None) => return,
        Ok(Some(_)) => None,
        Err(error) => {
            log(paths, &format!("install failed: {error:#}"));
            Some(format!("{error:#}"))
        }
    };
    let _ = StateStore::new(paths).update(paths, |state| state.install_error = error);
}

/// Why this machine can't install updates by itself, if it can't.
fn blocker() -> Option<String> {
    #[cfg(target_os = "macos")]
    return macos::InstallTarget::current()
        .err()
        .map(|error| error.to_string());
    #[cfg(not(target_os = "macos"))]
    Some(LINUX_REASON.to_string())
}

#[cfg(test)]
mod tests {
    use super::release::Release;
    use super::*;

    fn checked(latest: &str) -> UpdateState {
        UpdateState {
            last_check_at: Some(1),
            latest: Some(Release {
                version: latest.into(),
                archive: None,
            }),
            ..UpdateState::default()
        }
    }

    fn v(value: &str) -> Version {
        parse_version(value).unwrap()
    }

    #[test]
    fn a_newer_release_is_available_and_says_why_it_waits() {
        let status = describe(&v("1.6.0"), &checked("1.7.0"), false, None, Activity::Idle);
        assert_eq!(status.state, UpdatePhase::Available);
        assert_eq!(status.available.as_deref(), Some("1.7.0"));
        assert!(status.reason.unwrap().contains("--auto on"));

        let on = describe(&v("1.6.0"), &checked("1.7.0"), true, None, Activity::Idle);
        assert_eq!(on.reason, None);

        let linux = describe(
            &v("1.6.0"),
            &checked("1.7.0"),
            true,
            Some(LINUX_REASON.into()),
            Activity::Idle,
        );
        assert_eq!(linux.reason.as_deref(), Some(LINUX_REASON));
    }

    #[test]
    fn the_same_or_an_older_release_is_up_to_date() {
        let status = describe(&v("1.7.0"), &checked("1.7.0"), true, None, Activity::Idle);
        assert_eq!(status.state, UpdatePhase::UpToDate);
        assert_eq!(status.available, None);
        let never = describe(
            &v("1.7.0"),
            &UpdateState::default(),
            true,
            None,
            Activity::Idle,
        );
        assert_eq!(never.state, UpdatePhase::Unknown);
    }

    #[test]
    fn a_failed_check_and_the_current_activity_show() {
        let mut state = checked("1.6.0");
        state.failures = 2;
        state.last_error = Some("offline".into());
        let failed = describe(&v("1.6.0"), &state, true, None, Activity::Idle);
        assert_eq!(failed.state, UpdatePhase::Failed);
        assert_eq!(failed.reason.as_deref(), Some("offline"));
        let busy = describe(&v("1.6.0"), &state, true, None, Activity::Installing);
        assert_eq!(busy.state, UpdatePhase::Installing);
    }

    #[test]
    fn an_unreadable_state_file_fails_the_status_with_the_way_out() {
        let dir = tempfile::tempdir().unwrap();
        let paths = UpdatePaths::new(dir.path().to_path_buf());
        std::fs::write(paths.state_file(), b"{not json").unwrap();
        let updater = Updater::new(paths, dir.path().join("config.toml"), false);
        let status = updater.status();
        assert_eq!(status.state, UpdatePhase::Failed);
        assert_eq!(status.available, None);
        let reason = status.reason.unwrap();
        assert!(
            reason.contains("updates.json") && reason.contains("delete it"),
            "{reason}"
        );
    }

    #[test]
    fn a_previous_version_left_in_staging_shows_until_it_is_gone() {
        let dir = tempfile::tempdir().unwrap();
        let at = dir.path().join("Localify.app");
        std::fs::create_dir(&at).unwrap();
        let mut state = checked("1.7.0");
        state.stranded_previous = Some(at.clone());
        let status = describe(&v("1.7.0"), &state, true, None, Activity::Idle);
        assert_eq!(status.state, UpdatePhase::UpToDate);
        assert_eq!(status.reason, Some(stranded_reason(&at)));
        std::fs::remove_dir(&at).unwrap();
        let status = describe(&v("1.7.0"), &state, true, None, Activity::Idle);
        assert_eq!(status.reason, None);
    }

    #[test]
    fn the_status_serializes_as_the_socket_answers_it() {
        let status = describe(&v("1.6.0"), &checked("1.7.0"), true, None, Activity::Idle);
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "installed": "1.6.0",
                "available": "1.7.0",
                "state": "available",
                "reason": null,
                "automatic": true,
            })
        );
    }
}
