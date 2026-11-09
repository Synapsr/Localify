//! Updates in the app (docs/desktop.md, "Automatic updates"). The app never
//! installs: it shows the status the daemon answers, changes
//! `[updates] automatic`, runs the one coalesced check when asked, and on
//! `Restart to update` hands off to `localify update install --relaunch`.

use std::io;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow, bail};

use super::DesktopApi;
use crate::branding::BRANDING;
use crate::infrastructure::config::ConfigStore;
use crate::infrastructure::mgmt_client::MgmtSocketClient;
use crate::infrastructure::updates::{
    self, UpdatePaths, UpdatePhase, UpdateStatus, release, state,
};

impl DesktopApi {
    /// On a new install, writes the config with automatic updates on, before
    /// the welcome can save a setting into a config without them. An
    /// install that predates them, or left anything on this machine, is
    /// left as it is (docs/desktop.md, "Automatic updates").
    pub fn start_new_install(&self) -> Result<bool> {
        Ok(ConfigStore::new(self.config_path().to_path_buf()).start_new_install()?)
    }

    /// The daemon's status, which knows whether it is checking or
    /// installing now. With the daemon down, the CLI reads the same status
    /// from the files: only the bundle's `localify` can tell whether its
    /// bundle may be replaced.
    ///
    /// Either describes the daemon's bundle, so it is narrowed to what the
    /// open app can do ([`open_app_blocker`]).
    pub fn update_status(&self) -> Result<UpdateStatus> {
        let status = match self.update_client()?.update_status() {
            Ok(status) => status,
            Err(_) => self.update_cli(&["update", "--status", "--json"])?,
        };
        Ok(for_open_app(status, open_app_blocker()))
    }

    /// `Install updates automatically`. The daemon reads the setting again
    /// before any unattended install, so it needs no restart.
    pub fn set_automatic_updates(&self, automatic: bool) -> Result<UpdateStatus> {
        let status = match self.update_client()?.update_settings(automatic) {
            Ok(status) => status,
            Err(_) => {
                let on = if automatic { "on" } else { "off" };
                self.update_cli(&["update", "--auto", on, "--json"])?
            }
        };
        Ok(for_open_app(status, open_app_blocker()))
    }

    /// `Check now`: the check every caller goes through, so it shares the
    /// machine-wide cooldown and leaves its answer where the daemon reads
    /// it.
    pub fn check_for_updates(&self) -> Result<UpdateStatus> {
        let endpoint = release::updater_endpoint()
            .ok_or_else(|| anyhow!("This build has no update feed: update it by hand."))?;
        let paths = UpdatePaths::for_user()
            .ok_or_else(|| anyhow!("HOME is not set, so updates can't be checked."))?;
        state::check(&paths, updates::unix_now(), || {
            release::fetch_manifest(&endpoint)
        })?;
        self.update_status()
    }

    fn update_client(&self) -> Result<MgmtSocketClient> {
        let (_, paths) = self.load_config_and_paths()?;
        Ok(MgmtSocketClient::new(&paths.socket_path))
    }

    fn update_cli(&self, args: &[&str]) -> Result<UpdateStatus> {
        let output = Command::new(self.binary_path())
            .arg("--config")
            .arg(self.config_path())
            .args(args)
            .stdin(Stdio::null())
            .output()
            .with_context(|| format!("Could not run {}", self.binary_path().display()))?;
        if !output.status.success() {
            bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
        }
        serde_json::from_slice(&output.stdout)
            .context("The update status from the CLI could not be read")
    }
}

/// Why the open app can't install an update itself, if it can't.
/// `Restart to update` hands off to this bundle's own CLI, whose installer
/// replaces this bundle and no other. The daemon's status speaks for the
/// daemon's bundle, which may be another copy: one in /Applications while
/// this one runs from a disk image or Downloads.
pub fn open_app_blocker() -> Option<String> {
    let exe = std::env::current_exe().unwrap_or_default();
    updates::blocker_of(&exe.with_file_name(BRANDING.cli_command()))
}

/// The status, for an app whose own bundle `blocker` keeps from being
/// replaced: no `Restart to update`, and an update that waits says why, as
/// the daemon's own blocker would.
fn for_open_app(mut status: UpdateStatus, blocker: Option<String>) -> UpdateStatus {
    let Some(blocker) = blocker else {
        return status;
    };
    if status.can_install {
        status.can_install = false;
        if status.state == UpdatePhase::Available {
            status.reason = Some(blocker);
        }
    }
    status
}

/// `Restart to update`: starts `localify update install --relaunch` from the
/// bundle's own CLI, in its own session with no terminal, so it outlives the
/// app. It waits for `app.lock`, installs, then opens the app again. The
/// caller quits right after, which releases the lock.
pub fn hand_off_update(cli: &Path) -> Result<()> {
    let mut command = Command::new(cli);
    command
        .args(["update", "install", "--relaunch"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: `setsid` is async-signal-safe and reads no memory of the
    // parent, which is all `pre_exec` asks of what runs between fork and
    // exec.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("Could not start {}", cli.display()))?;
    // Reaped if the app is still there when it ends.
    std::thread::spawn(move || child.wait());
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::infrastructure::config::{Config, ConfigStore};
    use crate::infrastructure::paths::LocalifyPaths;

    fn script(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("localify");
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// An API whose daemon isn't running and whose CLI is `body`.
    fn api_without_daemon(dir: &Path, body: &str) -> DesktopApi {
        let config_path = dir.join("config.toml");
        let mut config = Config::default();
        config.paths = LocalifyPaths {
            data_dir: dir.join("data"),
            pid_file: dir.join("localify.pid"),
            log_file: dir.join("localify.log"),
            certs_dir: dir.join("certs"),
            socket_path: dir.join("localify.sock"),
        };
        ConfigStore::new(config_path.clone()).save(&config).unwrap();
        DesktopApi::new(config_path, script(dir, body))
    }

    const STATUS: &str = r#"{"installed":"1.6.0","available":"1.7.0","state":"available","reason":null,"automatic":true,"lastCheckAt":5,"canInstall":true}"#;

    #[test]
    fn without_the_daemon_the_status_comes_from_the_cli() {
        let dir = tempfile::tempdir().unwrap();
        let args = dir.path().join("args");
        let body = format!("echo \"$@\" > '{}'\necho '{STATUS}'", args.display());
        let api = api_without_daemon(dir.path(), &body);

        let status = api.update_status().unwrap();

        assert_eq!(status.available.as_deref(), Some("1.7.0"));
        assert_eq!(status.last_check_at, Some(5));
        let config = dir.path().join("config.toml");
        assert_eq!(
            fs::read_to_string(args).unwrap().trim(),
            format!("--config {} update --status --json", config.display())
        );
    }

    #[test]
    fn an_app_whose_own_bundle_cant_be_replaced_offers_no_restart() {
        let dir = tempfile::tempdir().unwrap();
        // The daemon's bundle may be replaced; this test binary, like a copy
        // run from a disk image, is not one that may.
        let api = api_without_daemon(dir.path(), &format!("echo '{STATUS}'"));

        let status = api.update_status().unwrap();

        assert!(!status.can_install);
        let reason = status.reason.unwrap();
        assert_eq!(Some(reason), open_app_blocker());
    }

    #[test]
    fn the_open_apps_blocker_keeps_the_reason_of_a_status_that_waits_for_nothing() {
        let status: UpdateStatus = serde_json::from_str(
            r#"{"installed":"1.6.0","available":null,"state":"upToDate","reason":"kept","automatic":true,"lastCheckAt":5,"canInstall":true}"#,
        )
        .unwrap();

        let status = for_open_app(status, Some("moved".into()));

        assert!(!status.can_install);
        assert_eq!(status.reason.as_deref(), Some("kept"));
    }

    #[test]
    fn an_installable_app_keeps_the_daemons_status() {
        let status: UpdateStatus = serde_json::from_str(STATUS).unwrap();

        let status = for_open_app(status, None);

        assert!(status.can_install);
        assert_eq!(status.reason, None);
    }

    #[test]
    fn without_the_daemon_the_cli_changes_the_setting() {
        let dir = tempfile::tempdir().unwrap();
        let args = dir.path().join("args");
        let body = format!("echo \"$@\" > '{}'\necho '{STATUS}'", args.display());
        let api = api_without_daemon(dir.path(), &body);

        api.set_automatic_updates(false).unwrap();

        let args = fs::read_to_string(args).unwrap();
        assert!(args.ends_with("update --auto off --json\n"), "{args}");
    }

    #[test]
    fn a_cli_that_fails_says_why() {
        let dir = tempfile::tempdir().unwrap();
        let api = api_without_daemon(dir.path(), "echo 'HOME is not set' >&2\nexit 1");

        let error = api.update_status().unwrap_err();

        assert_eq!(error.to_string(), "HOME is not set");
    }

    #[test]
    fn the_handoff_runs_the_install_in_its_own_session_without_input() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out");
        let body = format!(
            "if read -r line; then input=some; else input=none; fi\n\
             echo \"$@|$$|$(ps -o pgid= -p $$ | tr -d ' ')|$input\" > '{0}.tmp'\n\
             mv '{0}.tmp' '{0}'",
            out.display()
        );
        let cli = script(dir.path(), &body);

        hand_off_update(&cli).unwrap();

        let started = Instant::now();
        while !out.exists() && started.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(50));
        }
        let line = fs::read_to_string(&out).unwrap();
        let parts: Vec<&str> = line.trim().split('|').collect();
        assert_eq!(parts[0], "update install --relaunch");
        // `setsid` makes it the leader of a new session and process group.
        assert_eq!(parts[1], parts[2]);
        assert_eq!(parts[3], "none");
    }

    #[test]
    fn a_handoff_that_cannot_start_fails_without_quitting_anything() {
        let dir = tempfile::tempdir().unwrap();

        let error = hand_off_update(&dir.path().join("missing")).unwrap_err();

        assert!(error.to_string().starts_with("Could not start"), "{error}");
    }
}
