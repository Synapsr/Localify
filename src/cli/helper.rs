use std::env;
use std::fs;
#[cfg_attr(not(target_os = "macos"), allow(unused_imports))]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(not(target_os = "macos"))]
use std::process::Stdio;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::infrastructure::certs::RootCA;
use crate::infrastructure::paths::LocalifyPaths;
use localify::branding::BRANDING;
use localify::desktop::DesktopApi;
use localify::infrastructure::dns::{DnsService, get_dns_service};
#[cfg(target_os = "macos")]
use localify::infrastructure::privilege::UnprivilegedUser;
// The library's copy: the helper's `DesktopApi` reads the user from there.
use localify::infrastructure::privilege::set_acting_user;
use localify::system_helper::{SystemHelperClient, current_uid, helper_socket_path, serve};
#[cfg(not(target_os = "macos"))]
use localify::system_helper_binary::binary_is_root_protected;
#[cfg(target_os = "macos")]
use localify::system_helper_binary::{
    ensure_build_may_install_helper, helper_binary_path, helper_plist_path,
    helper_service_is_current, helper_service_label, install_helper_binary, installed_helper_uids,
    launchd_plist_contents, remove_if_present,
};

pub fn ensure_running(config_path: &Path, socket: PathBuf, uid: u32) -> Result<()> {
    ensure_helper_is_for_a_user(uid)?;
    let client = SystemHelperClient::new(socket.clone());
    if client.ping().is_ok() && helper_matches_current_executable(uid)? {
        return Ok(());
    }

    start_helper_service(config_path, &socket, uid)?;
    wait_for_helper(&client)
}

pub fn serve_forever(config_path: &Path, uid: u32, socket: PathBuf) -> Result<()> {
    ensure_helper_is_for_a_user(uid)?;
    set_acting_user(uid);
    let binary_path = env::current_exe().context(format!(
        "Failed to resolve the {} executable path",
        BRANDING.slug()
    ))?;
    #[cfg(not(target_os = "macos"))]
    ensure_root_runs_protected_binary(&binary_path)?;
    let api = DesktopApi::new_direct(config_path.to_path_buf(), binary_path).with_daemon_uid(uid);
    serve(&socket, uid, config_path, &api)
}

/// The helper starts the daemon as this user, so it must name one. The dev
/// profile needs no helper: its ports are unprivileged and its files are the user's.
fn ensure_helper_is_for_a_user(uid: u32) -> Result<()> {
    if uid == 0 {
        bail!(
            "The {} system helper must be installed for a non-root user.",
            BRANDING.app_name
        );
    }
    if BRANDING.is_dev_profile() {
        bail!(
            "The dev profile does not use a root helper. Run {} without sudo.",
            BRANDING.cli_command()
        );
    }
    Ok(())
}

/// Prints the on-demand issuer for the daemon that started this process, or
/// `null` when there is no Root CA yet.
pub fn issue_daemon_ca(paths: &LocalifyPaths) -> Result<()> {
    let ca = RootCA::new(paths.data_dir.clone());
    let issuer = if ca.exists() {
        Some(ca.issue_on_demand_issuer()?)
    } else {
        None
    };
    println!("{}", serde_json::to_string(&issuer)?);
    Ok(())
}

/// Writes the two things the dev profile cannot do as the user: the resolver
/// file and the `localify-dev` link. The port and target come from the command an
/// administrator just approved.
pub fn dev_system_setup(dns_port: u16, cli_target: &Path) -> Result<()> {
    ensure_dev_profile()?;
    get_dns_service()?.setup(dns_port)?;
    DesktopApi::link_cli_command(cli_target)
}

pub fn dev_system_remove(cli_target: &Path) -> Result<()> {
    ensure_dev_profile()?;
    get_dns_service()?.cleanup()?;
    DesktopApi::unlink_cli_command(cli_target)
}

/// Removes what a root dev helper left behind. Older builds ran the dev daemon
/// as root from a launchd service, with state in the user's own folder.
#[cfg(target_os = "macos")]
pub fn remove_dev_helper(uid: u32) -> Result<()> {
    ensure_dev_profile()?;
    let user = UnprivilegedUser::lookup(uid)?;

    uninstall_launchd_service(uid)?;
    stop_root_dev_daemon();
    let cli_link = BRANDING.cli_link_path();
    if fs::read_link(&cli_link).ok().as_deref() == Some(helper_binary_path(uid).as_path()) {
        remove_if_present(&cli_link)?;
    }
    remove_old_dev_ca_trust()?;
    remove_root_owned_entries(&user.home, Path::new(LEGACY_DEV_STATE_SUBDIR), user.uid)
}

#[cfg(not(target_os = "macos"))]
pub fn remove_dev_helper(_uid: u32) -> Result<()> {
    ensure_dev_profile()
}

/// These run as root with paths chosen by the current profile, so they must not
/// run for production by accident.
fn ensure_dev_profile() -> Result<()> {
    if BRANDING.is_dev_profile() {
        return Ok(());
    }
    bail!(
        "This command only applies to the dev profile. Run it with {}=dev.",
        BRANDING.profile_env()
    )
}

/// Older dev daemons ran as root from their own process group, and removing the
/// service leaves them running on the dev ports. The match comes from the process
/// table of root's own processes, never from a PID file in the user's folder.
#[cfg(target_os = "macos")]
fn stop_root_dev_daemon() {
    let pattern = format!(
        "{}/config\\.toml start --foreground",
        BRANDING.dev_desktop_state_dir_name
    );
    let _ = Command::new("/usr/bin/pkill")
        .args(["-TERM", "-u", "0", "-f", &pattern])
        .output();
}

/// Older dev installs trusted their CA system-wide, and its key is about to go.
#[cfg(target_os = "macos")]
fn remove_old_dev_ca_trust() -> Result<()> {
    use localify::infrastructure::certs::trust_store::{TrustStore, get_trust_store};

    get_trust_store()?.remove_ca(Path::new(""))?;
    Ok(())
}

#[cfg(target_os = "macos")]
const LEGACY_DEV_STATE_SUBDIR: &str = "Library/Application Support/Localify Dev";

/// Deletes the entries directly inside `home/subdir` that belong to root, as
/// root, without following links anywhere on the way.
///
/// Every directory from the home folder down belongs to the user, who could swap
/// one for a link at any moment. So the walk opens one component at a time with
/// `O_NOFOLLOW` and moves into it by file descriptor, and removal then uses paths
/// relative to that directory. Changes this process's working directory, so run
/// it only from a one-shot command.
#[cfg(target_os = "macos")]
fn remove_root_owned_entries(home: &Path, subdir: &Path, uid: u32) -> Result<()> {
    remove_entries_owned_by(home, subdir, uid, 0)
}

#[cfg(target_os = "macos")]
fn remove_entries_owned_by(home: &Path, subdir: &Path, uid: u32, doomed_owner: u32) -> Result<()> {
    use std::os::unix::fs::MetadataExt;

    let home = fs::canonicalize(home)
        .with_context(|| format!("Failed to resolve the home folder {}", home.display()))?;
    env::set_current_dir(&home)
        .with_context(|| format!("Failed to enter the home folder {}", home.display()))?;

    for component in subdir.components() {
        match enter_directory_without_following_links(component.as_os_str(), uid, doomed_owner) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "Refusing to clean '{}' under {}: {} is not a plain folder of this user",
                        subdir.display(),
                        home.display(),
                        component.as_os_str().display()
                    )
                });
            }
        }
    }

    for entry in fs::read_dir(".").context("Failed to list the old dev state")? {
        let entry = entry.context("Failed to list the old dev state")?;
        let name = entry.file_name();
        // The old daemon may still be removing its own PID file and socket.
        let Ok(metadata) = fs::symlink_metadata(&name) else {
            continue;
        };
        if metadata.uid() != doomed_owner {
            continue;
        }
        let path = Path::new(&name);
        let removed = if metadata.is_dir() {
            fs::remove_dir_all(path)
        } else {
            fs::remove_file(path)
        };
        match removed {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                return Err(error).with_context(|| format!("Failed to remove {}", name.display()));
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn enter_directory_without_following_links(
    name: &std::ffi::OsStr,
    uid: u32,
    doomed_owner: u32,
) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    let directory = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
        .open(name)?;
    let owner = directory.metadata()?.uid();
    if owner != uid && owner != doomed_owner {
        return Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
    }

    // SAFETY: `directory` is an open descriptor for the length of this call.
    match unsafe { libc::fchdir(directory.as_raw_fd()) } {
        0 => Ok(()),
        _ => Err(std::io::Error::last_os_error()),
    }
}

pub fn uninstall_service(uid: u32) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        uninstall_launchd_service(uid)
    }

    #[cfg(not(target_os = "macos"))]
    {
        let socket_path = helper_socket_path(uid);
        let _ = fs::remove_file(socket_path);
        Ok(())
    }
}

/// Removes every installed helper, for `sudo localify uninstall`. Does nothing
/// without root, for example when uninstalling a custom user-owned config.
pub fn uninstall_all_services() -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        if current_uid()? != 0 {
            return Ok(());
        }
        let cli_link = BRANDING.cli_link_path();
        let cli_target = fs::read_link(&cli_link).ok();
        for uid in installed_helper_uids()? {
            uninstall_launchd_service(uid)?;
            if cli_target.as_deref() == Some(helper_binary_path(uid).as_path()) {
                remove_if_present(&cli_link).with_context(|| {
                    format!("Failed to remove the CLI link at {}", cli_link.display())
                })?;
            }
        }
    }

    Ok(())
}

/// Asks the running helper to remove its own service, for `localify uninstall`
/// without sudo, and waits until it is gone.
pub fn remove_service_through_helper(client: &SystemHelperClient) -> Result<()> {
    let uid = current_uid()?;
    client.remove_service()?;
    for _ in 0..50 {
        if helper_service_is_gone(client, uid) {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }

    bail!(
        "The {} system helper did not remove itself. Run: sudo {} helper uninstall-service --uid {uid}",
        BRANDING.app_name,
        BRANDING.cli_command()
    )
}

#[cfg(target_os = "macos")]
fn helper_service_is_gone(_client: &SystemHelperClient, uid: u32) -> bool {
    !helper_plist_path(uid).exists() && !helper_binary_path(uid).exists()
}

#[cfg(not(target_os = "macos"))]
fn helper_service_is_gone(client: &SystemHelperClient, _uid: u32) -> bool {
    client.ping().is_err()
}

fn wait_for_helper(client: &SystemHelperClient) -> Result<()> {
    for _ in 0..50 {
        if client.ping().is_ok() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }

    bail!(
        "{} system helper did not become ready in time.",
        BRANDING.app_name
    )
}

#[cfg(target_os = "macos")]
fn start_helper_service(config_path: &Path, socket: &Path, uid: u32) -> Result<()> {
    install_launchd_service(config_path, socket, uid)
}

#[cfg(not(target_os = "macos"))]
fn start_helper_service(config_path: &Path, socket: &Path, uid: u32) -> Result<()> {
    spawn_helper_process(config_path, socket, uid)
}

#[cfg(not(target_os = "macos"))]
fn spawn_helper_process(config_path: &Path, socket: &Path, uid: u32) -> Result<()> {
    let executable = env::current_exe().context(format!(
        "Failed to resolve the {} executable path",
        BRANDING.slug()
    ))?;
    ensure_root_runs_protected_binary(&executable)?;
    let child = Command::new(executable)
        .arg("--config")
        .arg(config_path)
        .arg("helper")
        .arg("serve")
        .arg("--socket")
        .arg(socket)
        .arg("--uid")
        .arg(uid.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context(format!(
            "Failed to launch the {} system helper",
            BRANDING.app_name
        ))?;

    println!(
        "{} system helper started (PID: {})",
        BRANDING.app_name,
        child.id()
    );
    Ok(())
}

/// Without launchd, a root helper still restarts the daemon from its own path,
/// so it must not start from a file another user could replace.
#[cfg(not(target_os = "macos"))]
fn ensure_root_runs_protected_binary(executable: &Path) -> Result<()> {
    if current_uid()? == 0 && !binary_is_root_protected(executable) {
        bail!(
            "Refusing to run the {} system helper as root from '{}': a non-root user could replace it.\n\
             Install {} in a directory only root can write to, or run the helper without sudo.",
            BRANDING.app_name,
            executable.display(),
            BRANDING.cli_command()
        );
    }
    Ok(())
}

/// Runs only as root, from an administrator-approved command.
#[cfg(target_os = "macos")]
fn install_launchd_service(config_path: &Path, socket: &Path, uid: u32) -> Result<()> {
    ensure_build_may_install_helper()?;
    let executable = env::current_exe().context(format!(
        "Failed to resolve the {} executable path",
        BRANDING.slug()
    ))?;
    install_helper_binary(&executable, uid)?;

    let plist_path = helper_plist_path(uid);
    let label = helper_service_label(uid);
    let plist = launchd_plist_contents(config_path, socket, uid);

    fs::write(&plist_path, plist).with_context(|| {
        format!(
            "Failed to write the {} helper plist at {}",
            BRANDING.app_name,
            plist_path.display()
        )
    })?;

    let mut permissions = fs::metadata(&plist_path)?.permissions();
    permissions.set_mode(0o644);
    fs::set_permissions(&plist_path, permissions)?;

    let service_target = format!("system/{label}");
    let _ = run_launchctl(&["bootout", &service_target]);
    run_launchctl(&["bootstrap", "system", plist_path.to_string_lossy().as_ref()])?;
    run_launchctl(&["enable", &service_target])?;
    run_launchctl(&["kickstart", "-k", &service_target])?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn uninstall_launchd_service(uid: u32) -> Result<()> {
    let label = helper_service_label(uid);
    let plist_path = helper_plist_path(uid);
    let binary_path = helper_binary_path(uid);
    let socket_path = helper_socket_path(uid);
    let service_target = format!("system/{label}");

    let _ = run_launchctl(&["bootout", &service_target]);

    for path in [&plist_path, &binary_path] {
        remove_if_present(path).with_context(|| {
            format!(
                "Failed to remove the {} helper file at {}",
                BRANDING.app_name,
                path.display()
            )
        })?;
    }

    let _ = fs::remove_file(socket_path);
    Ok(())
}

#[cfg(target_os = "macos")]
fn run_launchctl(args: &[&str]) -> Result<()> {
    let output = Command::new("launchctl")
        .args(args)
        .output()
        .context(format!(
            "Failed to launch launchctl for the {} helper",
            BRANDING.app_name
        ))?;

    if output.status.success() {
        return Ok(());
    }

    bail!(command_output(&output).unwrap_or_else(|| {
        format!(
            "launchctl {} failed without diagnostic output.",
            args.join(" ")
        )
    }))
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn command_output(output: &std::process::Output) -> Option<String> {
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    match (stdout.is_empty(), stderr.is_empty()) {
        (false, true) => Some(stdout),
        (true, false) => Some(stderr),
        (false, false) => Some(format!("{stdout}\n{stderr}")),
        (true, true) => None,
    }
}

#[cfg(target_os = "macos")]
fn helper_matches_current_executable(uid: u32) -> Result<bool> {
    let executable = env::current_exe().context(format!(
        "Failed to resolve the {} executable path",
        BRANDING.slug()
    ))?;
    helper_service_is_current(uid, &executable)
}

#[cfg(not(target_os = "macos"))]
fn helper_matches_current_executable(_uid: u32) -> Result<bool> {
    Ok(true)
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt, symlink};

    /// Runs one ignored test in its own process, because the walk changes the
    /// working directory and tests share a process.
    fn run_in_child(test: &str, dir: &Path) {
        let status = Command::new(env::current_exe().unwrap())
            .args(["--ignored", "--exact", "--nocapture", test])
            .env("LOCALIFY_TEST_DIR", dir)
            .status()
            .unwrap();
        assert!(status.success());
    }

    fn test_dir() -> PathBuf {
        PathBuf::from(env::var_os("LOCALIFY_TEST_DIR").unwrap())
    }

    fn my_uid() -> u32 {
        fs::metadata(test_dir()).unwrap().uid()
    }

    #[test]
    fn the_walk_removes_entries_of_the_given_owner_without_following_a_linked_entry() {
        let dir = fs::canonicalize(tempfile::tempdir().unwrap().keep()).unwrap();
        fs::create_dir_all(dir.join("state/data/certs")).unwrap();
        fs::write(dir.join("state/data/ca.key"), "key").unwrap();
        fs::write(dir.join("state/app.pid"), "1").unwrap();
        fs::create_dir(dir.join("outside")).unwrap();
        fs::write(dir.join("outside/keep"), "keep").unwrap();
        symlink(dir.join("outside"), dir.join("state/link")).unwrap();

        run_in_child("cli::helper::tests::child_removes_everything_it_owns", &dir);

        assert_eq!(fs::read_dir(dir.join("state")).unwrap().count(), 0);
        assert_eq!(fs::read(dir.join("outside/keep")).unwrap(), b"keep");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    #[ignore = "runs in a child process"]
    fn child_removes_everything_it_owns() {
        let uid = my_uid();
        remove_entries_owned_by(&test_dir(), Path::new("state"), uid, uid).unwrap();
    }

    #[test]
    fn the_walk_keeps_entries_of_other_owners() {
        let dir = fs::canonicalize(tempfile::tempdir().unwrap().keep()).unwrap();
        fs::create_dir(dir.join("state")).unwrap();
        fs::write(dir.join("state/config.toml"), "keep").unwrap();

        run_in_child(
            "cli::helper::tests::child_removes_entries_of_another_owner",
            &dir,
        );

        assert_eq!(fs::read(dir.join("state/config.toml")).unwrap(), b"keep");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    #[ignore = "runs in a child process"]
    fn child_removes_entries_of_another_owner() {
        let uid = my_uid();
        remove_entries_owned_by(&test_dir(), Path::new("state"), uid, uid + 1).unwrap();
    }

    #[test]
    fn the_walk_refuses_a_linked_folder_on_the_way() {
        let dir = fs::canonicalize(tempfile::tempdir().unwrap().keep()).unwrap();
        fs::create_dir(dir.join("elsewhere")).unwrap();
        fs::write(dir.join("elsewhere/victim"), "victim").unwrap();
        symlink(dir.join("elsewhere"), dir.join("state")).unwrap();

        run_in_child(
            "cli::helper::tests::child_refuses_a_linked_state_folder",
            &dir,
        );

        assert_eq!(fs::read(dir.join("elsewhere/victim")).unwrap(), b"victim");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    #[ignore = "runs in a child process"]
    fn child_refuses_a_linked_state_folder() {
        let uid = my_uid();
        let error = remove_entries_owned_by(&test_dir(), Path::new("state"), uid, uid).unwrap_err();
        assert!(error.to_string().contains("Refusing to clean"));
    }

    #[test]
    fn a_missing_state_folder_is_already_clean() {
        let dir = fs::canonicalize(tempfile::tempdir().unwrap().keep()).unwrap();

        run_in_child(
            "cli::helper::tests::child_cleans_a_missing_state_folder",
            &dir,
        );

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    #[ignore = "runs in a child process"]
    fn child_cleans_a_missing_state_folder() {
        let uid = my_uid();
        remove_entries_owned_by(&test_dir(), Path::new("state/nested"), uid, uid).unwrap();
    }
}
