use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(not(target_os = "macos"))]
use std::process::Stdio;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use roxy::branding::BRANDING;
use roxy::desktop::DesktopApi;
use roxy::system_helper::{SystemHelperClient, helper_socket_path, serve};
#[cfg(target_os = "macos")]
const LAUNCHD_PLIST_DIR: &str = "/Library/LaunchDaemons";

pub fn ensure_running(config_path: &Path, socket: PathBuf, uid: u32) -> Result<()> {
    let client = SystemHelperClient::new(socket.clone());
    if client.ping().is_ok() {
        return Ok(());
    }

    start_helper_service(config_path, &socket, uid)?;
    wait_for_helper(&client)
}

pub fn serve_forever(config_path: &Path, uid: u32, socket: PathBuf) -> Result<()> {
    let binary_path = env::current_exe().context(format!(
        "Failed to resolve the {} executable path",
        BRANDING.slug
    ))?;
    let api = DesktopApi::new_direct(config_path.to_path_buf(), binary_path);
    serve(&socket, uid, &api)
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
        BRANDING.slug
    ))?;
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

#[cfg(target_os = "macos")]
fn install_launchd_service(config_path: &Path, socket: &Path, uid: u32) -> Result<()> {
    let executable = env::current_exe().context(format!(
        "Failed to resolve the {} executable path",
        BRANDING.slug
    ))?;
    let plist_path = launchd_plist_path(uid);
    let label = helper_service_label(uid);
    let plist = launchd_plist_contents(&label, &executable, config_path, socket, uid);

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
    let plist_path = launchd_plist_path(uid);
    let socket_path = helper_socket_path(uid);
    let service_target = format!("system/{label}");

    let _ = run_launchctl(&["bootout", &service_target]);

    if plist_path.exists() {
        fs::remove_file(&plist_path).with_context(|| {
            format!(
                "Failed to remove the {} helper plist at {}",
                BRANDING.app_name,
                plist_path.display()
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

fn helper_service_label(uid: u32) -> String {
    format!("{}.{uid}", BRANDING.helper_service_prefix())
}

#[cfg(target_os = "macos")]
fn launchd_plist_path(uid: u32) -> PathBuf {
    PathBuf::from(LAUNCHD_PLIST_DIR).join(format!("{}.plist", helper_service_label(uid)))
}

fn launchd_plist_contents(
    label: &str,
    executable: &Path,
    config_path: &Path,
    socket: &Path,
    uid: u32,
) -> String {
    let args = [
        executable.to_string_lossy().to_string(),
        "--config".into(),
        config_path.to_string_lossy().to_string(),
        "helper".into(),
        "serve".into(),
        "--socket".into(),
        socket.to_string_lossy().to_string(),
        "--uid".into(),
        uid.to_string(),
    ];

    let program_arguments = args
        .iter()
        .map(|arg| format!("    <string>{}</string>", escape_xml(arg)))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{}</string>
  <key>ProgramArguments</key>
  <array>
{}
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
</dict>
</plist>
"#,
        escape_xml(label),
        program_arguments,
    )
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::{helper_service_label, launchd_plist_contents};
    use std::path::Path;

    #[test]
    fn helper_service_label_is_uid_scoped() {
        assert_eq!(helper_service_label(501), "dev.localify.helper.501");
    }

    #[test]
    fn launchd_plist_contains_expected_program_arguments() {
        let plist = launchd_plist_contents(
            "dev.localify.helper.501",
            Path::new("/Applications/Localify.app/Contents/MacOS/localify"),
            Path::new("/etc/localify/config.toml"),
            Path::new("/var/run/localify-helper-501.sock"),
            501,
        );

        assert!(plist.contains("<string>dev.localify.helper.501</string>"));
        assert!(
            plist.contains("<string>/Applications/Localify.app/Contents/MacOS/localify</string>")
        );
        assert!(plist.contains("<string>--config</string>"));
        assert!(plist.contains("<string>/etc/localify/config.toml</string>"));
        assert!(plist.contains("<string>helper</string>"));
        assert!(plist.contains("<string>serve</string>"));
        assert!(plist.contains("<string>--socket</string>"));
        assert!(plist.contains("<string>/var/run/localify-helper-501.sock</string>"));
        assert!(plist.contains("<string>--uid</string>"));
        assert!(plist.contains("<string>501</string>"));
    }
}
