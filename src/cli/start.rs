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
use crate::infrastructure::dns::check_system_resolution;
use crate::infrastructure::network::get_lan_ip;
use crate::infrastructure::paths::RoxyPaths;
use crate::infrastructure::pid::PidFile;
use crate::infrastructure::port_check::ensure_daemon_ports_available;

pub fn execute(
    foreground: bool,
    verbose: bool,
    config_path: &Path,
    paths: &RoxyPaths,
    daemon_config: &DaemonConfig,
) -> Result<()> {
    let pid_file = PidFile::new(paths.pid_file.clone());
    let service = StartDaemon::new(&pid_file, daemon_config);
    let ready = service.preflight()?;
    ensure_daemon_ports_available(ready.http_port, ready.https_port, daemon_config.dns_port)?;

    if foreground {
        // Run in foreground (blocking) — daemon module lives in binary crate
        return crate::daemon::lifecycle::run(verbose, config_path, paths);
    }

    let exe = env::current_exe()?;
    let child = spawn_background_daemon(&exe, config_path, verbose)?;
    let pid = wait_for_daemon_start(child, &pid_file, &paths.log_file)?;

    let lan_ip = get_lan_ip();
    println!("Localify daemon started (PID: {})", pid);
    println!(
        "Listening on 0.0.0.0:{} (HTTP) and 0.0.0.0:{} (HTTPS)",
        ready.http_port, ready.https_port
    );
    println!("LAN IP: {}", lan_ip);
    print_dns_resolution_warning();
    if !lan_ip.is_loopback() {
        println!("\nAccess from other devices: https://yourdomain.localify");
    }
    println!("\nUse 'localify status' to check status");
    println!("Use 'localify stop' to stop the daemon");

    Ok(())
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
