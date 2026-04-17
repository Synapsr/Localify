use std::path::Path;

use anyhow::Result;
use std::env;
use std::process::{Command, Stdio};

use crate::application::start_daemon::StartDaemon;
use crate::branding::BRANDING;
use crate::config::DaemonConfig;
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

    // Fork to background
    let exe = env::current_exe()?;

    let mut cmd = Command::new(exe);
    cmd.arg("--config")
        .arg(config_path)
        .arg("start")
        .arg("--foreground");

    // Pass verbose flag via environment to subprocess
    if verbose {
        cmd.env(BRANDING.env_var("LOG"), "debug");
    }

    let child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    let lan_ip = get_lan_ip();
    println!("Localify daemon started (PID: {})", child.id());
    println!(
        "Listening on 0.0.0.0:{} (HTTP) and 0.0.0.0:{} (HTTPS)",
        ready.http_port, ready.https_port
    );
    println!("LAN IP: {}", lan_ip);
    if !lan_ip.is_loopback() {
        println!("\nAccess from other devices: https://yourdomain.localify");
    }
    println!("\nUse 'localify status' to check status");
    println!("Use 'localify stop' to stop the daemon");

    Ok(())
}
