//! `localify update`: what is installed and available, the automatic
//! setting, and an install a person asks for (docs/desktop.md, "Automatic
//! updates"). It needs no app and no window, so it works over SSH on a
//! runner nobody sits at.

use std::path::Path;

use anyhow::{Result, anyhow};

use super::output::{print_json, wants_json};
use crate::branding::BRANDING;
use crate::infrastructure::mgmt_client::MgmtSocketClient;
use crate::infrastructure::updates::{self, UpdatePaths, UpdatePhase, UpdateStatus, Updater};

fn update_paths() -> Result<UpdatePaths> {
    UpdatePaths::for_user().ok_or_else(|| anyhow!("HOME is not set, so updates can't be managed."))
}

/// The daemon's status, which knows whether it is checking or installing
/// now, or the same status read here when the daemon isn't running.
fn read_status(client: &MgmtSocketClient, config_path: &Path) -> Result<UpdateStatus> {
    if let Ok(status) = client.update_status() {
        return Ok(status);
    }
    let updater = Updater::new(update_paths()?, config_path.to_path_buf(), false);
    Ok(updater.status())
}

/// `localify update` and `localify update --status`.
pub fn status(json: bool, client: &MgmtSocketClient, config_path: &Path) -> Result<()> {
    print_status(&read_status(client, config_path)?, json)
}

/// `localify update --auto on|off`. The daemon reads the setting again
/// right before any unattended install, so it needs no restart.
pub fn set_automatic(
    on: bool,
    json: bool,
    client: &MgmtSocketClient,
    config_path: &Path,
) -> Result<()> {
    updates::set_automatic(config_path, on)?;
    if let Ok(paths) = update_paths() {
        updates::log(&paths, &format!("automatic={on} by localify update"));
    }
    print_status(&read_status(client, config_path)?, json)
}

fn print_status(status: &UpdateStatus, json: bool) -> Result<()> {
    if wants_json(json) {
        return print_json(status);
    }
    println!("Installed: {} {}", BRANDING.app_name, status.installed);
    let available = status.available.as_deref().unwrap_or_default();
    match status.state {
        UpdatePhase::Available => println!("Available: {} {available}", BRANDING.app_name),
        UpdatePhase::UpToDate => println!("Up to date."),
        UpdatePhase::Unknown => println!("Not checked yet."),
        UpdatePhase::Checking => println!("Checking for updates..."),
        UpdatePhase::Installing => println!("Installing {} {available}...", BRANDING.app_name),
        UpdatePhase::Failed => println!("The last check failed."),
    }
    let automatic = if status.automatic { "on" } else { "off" };
    println!("Automatic updates: {automatic}");
    if let Some(reason) = &status.reason {
        println!("{reason}");
    }
    Ok(())
}

/// `localify update install [--relaunch]`.
#[cfg(target_os = "macos")]
pub fn install(
    relaunch: bool,
    json: bool,
    client: &MgmtSocketClient,
    config_path: &Path,
) -> Result<()> {
    let paths = update_paths()?;
    let result = macos_install::run(&paths, relaunch);
    updates::record_install(&paths, &result);
    if relaunch {
        macos_install::open_app();
    }
    let installed = result?;
    if wants_json(json) {
        return print_json(&read_status(client, config_path)?);
    }
    match installed {
        Some(version) => println!(
            "Installed {} {version}. The daemon restarts on it in a few seconds.",
            BRANDING.app_name
        ),
        None => println!(
            "{} {} is up to date.",
            BRANDING.app_name,
            updates::installed_version()
        ),
    }
    Ok(())
}

/// Linux has no install path: its CLI is checked by SHA-256 only.
#[cfg(not(target_os = "macos"))]
pub fn install(
    _relaunch: bool,
    _json: bool,
    _client: &MgmtSocketClient,
    _config_path: &Path,
) -> Result<()> {
    Err(anyhow!(
        "{} on Linux never installs updates by itself. {}.",
        BRANDING.app_name,
        updates::LINUX_REASON
    ))
}

#[cfg(target_os = "macos")]
mod macos_install {
    use std::process::Command;
    use std::time::Duration;

    use anyhow::{Result, anyhow};
    use semver::Version;

    use crate::infrastructure::updates::install::{Installer, Mode};
    use crate::infrastructure::updates::macos::{self, BUNDLE_ID, InstallTarget};
    use crate::infrastructure::updates::{UpdatePaths, release, state, unix_now};

    /// How long `--relaunch` waits for the app to quit.
    const APP_QUIT_WAIT: Duration = Duration::from_secs(120);

    /// Checks (within the machine-wide cooldown), downloads, verifies and
    /// swaps in. `None` when there is nothing newer.
    pub(super) fn run(paths: &UpdatePaths, relaunch: bool) -> Result<Option<Version>> {
        let target = InstallTarget::current()?;
        let endpoint = release::updater_endpoint()
            .ok_or_else(|| anyhow!("This build has no update feed: update it by hand."))?;
        let checked = state::check(paths, unix_now(), || release::fetch_manifest(&endpoint))?;
        let Some(release) = checked.release() else {
            return Ok(None);
        };
        let installer = Installer {
            paths,
            target,
            public_key: release::updater_public_key(),
            signature: &macos::Codesign,
            processes: &macos::process_paths,
            // A person asked: the setting doesn't apply.
            automatic: &|| true,
        };
        let Some(version) = installer.wants(release)? else {
            return Ok(None);
        };
        let url = release
            .archive
            .as_ref()
            .map(|archive| archive.url.as_str())
            .ok_or_else(|| anyhow!("The update manifest has no archive for this Mac"))?;
        eprintln!("Downloading Localify {version}...");
        let (archive, _digest) = release::download_archive(url)?;
        let staged = installer.stage(release, &archive)?;
        let wait_for_app = if relaunch {
            eprintln!("Waiting for Localify to quit...");
            APP_QUIT_WAIT
        } else {
            Duration::ZERO
        };
        installer
            .replace(staged, Mode::Confirmed { wait_for_app })
            .map(Some)
    }

    /// Opens the app in this login session, the new version when the
    /// install worked and the one still there when it didn't, so a person
    /// who clicked Restart to update is never left without it.
    pub(super) fn open_app() {
        let mut open = Command::new("/usr/bin/open");
        match InstallTarget::current() {
            Ok(target) => open.arg(target.bundle),
            Err(_) => open.args(["-b", BUNDLE_ID]),
        };
        if let Err(error) = open.status() {
            eprintln!("Could not open Localify: {error}");
        }
    }
}
