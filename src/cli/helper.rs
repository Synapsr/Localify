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

use localify::branding::BRANDING;
use localify::desktop::DesktopApi;
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
    let client = SystemHelperClient::new(socket.clone());
    if client.ping().is_ok() && helper_matches_current_executable(uid)? {
        return Ok(());
    }

    start_helper_service(config_path, &socket, uid)?;
    wait_for_helper(&client)
}

pub fn serve_forever(config_path: &Path, uid: u32, socket: PathBuf) -> Result<()> {
    let binary_path = env::current_exe().context(format!(
        "Failed to resolve the {} executable path",
        BRANDING.slug()
    ))?;
    #[cfg(not(target_os = "macos"))]
    ensure_root_runs_protected_binary(&binary_path)?;
    let api = DesktopApi::new_direct(config_path.to_path_buf(), binary_path);
    serve(&socket, uid, config_path, &api)
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
