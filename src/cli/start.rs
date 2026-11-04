use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use std::env;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus, Stdio};

use crate::application::start_daemon::StartDaemon;
use crate::branding::BRANDING;
use crate::config::DaemonConfig;
use crate::daemon::listeners::DaemonListeners;
use crate::infrastructure::dns::check_system_resolution;
use crate::infrastructure::macos_system::LegacyInstall;
use crate::infrastructure::managed_daemon::ManagedDaemon;
use crate::infrastructure::network::get_lan_ip;
use crate::infrastructure::paths::LocalifyPaths;
use crate::infrastructure::pid::{DaemonHandle, PidFile};
use crate::infrastructure::port_check::ensure_daemon_ports_available;

pub fn execute(
    foreground: bool,
    verbose: bool,
    config_path: &Path,
    paths: &LocalifyPaths,
    daemon_config: &DaemonConfig,
) -> Result<()> {
    if foreground {
        // launchd started this process and already opened the ports for it.
        if let Some(listeners) = DaemonListeners::from_launchd()? {
            return crate::daemon::lifecycle::run(verbose, config_path, paths, Some(listeners));
        }
    } else if let Some(managed) = ManagedDaemon::find(config_path, paths, daemon_config) {
        let pid = managed.start()?;
        print_daemon_started(pid, daemon_config);
        return Ok(());
    }

    if BRANDING.uses_launchd_service() && LegacyInstall::needs_migration() {
        bail!(
            "An older {0} install is still on this Mac and runs as root.\n\
             Run '{1}' once to replace it. It asks for one approval.",
            BRANDING.app_name,
            BRANDING.admin_command("install")
        );
    }

    let daemon = DaemonHandle::new(paths);
    let service = StartDaemon::new(&daemon, daemon_config);
    let ready = service.preflight()?;
    ensure_daemon_ports_available(
        ready.http_port,
        ready.https_port,
        daemon_config.dns_port,
        daemon_config.lan_access,
    )?;

    if foreground {
        // Run in foreground (blocking) — daemon module lives in binary crate
        return crate::daemon::lifecycle::run(verbose, config_path, paths, None);
    }

    let exe = env::current_exe()?;
    let child = spawn_background_daemon(&exe, config_path, verbose)?;
    let pid_file = PidFile::new(paths.pid_file.clone());
    let pid = wait_for_daemon_start(child, &pid_file, &paths.log_file)?;

    print_daemon_started(pid, daemon_config);
    Ok(())
}

/// The startup summary, whichever way the daemon was started.
pub(crate) fn print_daemon_started(pid: u32, daemon_config: &DaemonConfig) {
    let address = if daemon_config.lan_access {
        "0.0.0.0"
    } else {
        "127.0.0.1"
    };
    println!("Localify daemon started (PID: {})", pid);
    println!(
        "Listening on {address}:{} (HTTP) and {address}:{} (HTTPS)",
        daemon_config.http_port, daemon_config.https_port
    );
    print_dns_resolution_warning();
    if daemon_config.lan_access {
        let lan_ip = get_lan_ip();
        println!("LAN IP: {lan_ip}");
        if !lan_ip.is_loopback() {
            println!("\nAccess from other devices: https://yourdomain.localify");
        }
    }
    println!("\nUse 'localify status' to check status");
    println!("Use 'localify stop' to stop the daemon");
}

fn print_dns_resolution_warning() {
    let check = check_system_resolution(Duration::from_millis(1500));
    if check.healthy {
        return;
    }

    println!();
    println!(
        "Warning: system DNS lookup for {} is not reaching Localify.",
        check.hostname
    );
    if let Some(message) = check.message {
        println!("  {}", message);
    }
    println!(
        "  If a VPN is active, its DNS or leak-protection settings may be bypassing macOS /etc/resolver."
    );
}

fn spawn_background_daemon(exe: &Path, config_path: &Path, verbose: bool) -> Result<Child> {
    #[cfg(unix)]
    let mut cmd = if Path::new("/usr/bin/nohup").exists() {
        let mut cmd = Command::new("/usr/bin/nohup");
        cmd.arg(exe);
        cmd
    } else {
        Command::new(exe)
    };

    #[cfg(not(unix))]
    let mut cmd = Command::new(exe);

    cmd.arg("--config")
        .arg(config_path)
        .arg("start")
        .arg("--foreground")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    if verbose {
        cmd.env(BRANDING.env_var("LOG"), "debug");
    }

    #[cfg(unix)]
    {
        cmd.process_group(0);
    }

    cmd.spawn().map_err(|error| {
        anyhow::anyhow!("Failed to launch the {} daemon: {error}", BRANDING.app_name)
    })
}

fn wait_for_daemon_start(mut child: Child, pid_file: &PidFile, log_path: &Path) -> Result<u32> {
    let deadline = Instant::now() + Duration::from_secs(3);

    loop {
        if let Some(pid) = pid_file.get_running_pid()? {
            return Ok(pid);
        }

        if let Some(status) = child.try_wait()? {
            bail!(daemon_start_failure_message(status, log_path));
        }

        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!(daemon_start_timeout_message(log_path));
        }

        thread::sleep(Duration::from_millis(100));
    }
}

fn daemon_start_failure_message(status: ExitStatus, log_path: &Path) -> String {
    let exit_detail = status
        .code()
        .map(|code| format!(" with exit code {code}"))
        .unwrap_or_else(|| " after receiving a termination signal".into());
    let log_excerpt = startup_log_excerpt(log_path);

    if log_excerpt.is_empty() {
        return format!(
            "{} daemon exited during startup{}.\nNo startup logs were written yet. Run `{} start --foreground` in a terminal to inspect the failure directly.",
            BRANDING.app_name,
            exit_detail,
            BRANDING.cli_command()
        );
    }

    format!(
        "{} daemon exited during startup{}.\nRecent log output:\n{}",
        BRANDING.app_name, exit_detail, log_excerpt
    )
}

fn daemon_start_timeout_message(log_path: &Path) -> String {
    let log_excerpt = startup_log_excerpt(log_path);

    if log_excerpt.is_empty() {
        return format!(
            "{} did not become ready within 3 seconds.\nRun `{} start --foreground` in a terminal to inspect the startup failure directly.",
            BRANDING.app_name,
            BRANDING.cli_command()
        );
    }

    format!(
        "{} did not become ready within 3 seconds.\nRecent log output:\n{}",
        BRANDING.app_name, log_excerpt
    )
}

fn startup_log_excerpt(log_path: &Path) -> String {
    if !log_path.exists() {
        return String::new();
    }

    std::fs::read_to_string(log_path)
        .ok()
        .map(|content| {
            let mut lines = content.lines().rev().take(20).collect::<Vec<_>>();
            lines.reverse();
            lines.join("\n").trim().to_string()
        })
        .unwrap_or_default()
}
