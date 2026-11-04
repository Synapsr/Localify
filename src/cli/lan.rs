use std::path::Path;

use anyhow::Result;
use localify::desktop::dto::ConfigUpdateRequest;

use super::context::direct_api;
use crate::branding::BRANDING;
use crate::config::DaemonConfig;
use crate::infrastructure::managed_daemon::ManagedDaemon;
use crate::infrastructure::network::get_lan_ip;
use crate::infrastructure::paths::LocalifyPaths;

/// Turns access from other devices on or off. Localify listens on this machine
/// only by default. On macOS this rewrites the launchd plist, in one approval.
pub fn set(
    enabled: bool,
    config_path: &Path,
    paths: &LocalifyPaths,
    daemon_config: &DaemonConfig,
) -> Result<()> {
    if daemon_config.lan_access == enabled {
        print_status(daemon_config);
        return Ok(());
    }

    let api = direct_api(config_path)?;
    let mut daemon = api.read_config()?.daemon;
    daemon.lan_access = enabled;
    api.update_config(ConfigUpdateRequest {
        daemon: Some(daemon),
        paths: None,
        docker: None,
        agent: None,
    })?;

    print_status(&DaemonConfig {
        lan_access: enabled,
        ..daemon_config.clone()
    });
    // launchd has already restarted a daemon it manages on the new sockets.
    if ManagedDaemon::find(config_path, paths, daemon_config).is_none() {
        println!(
            "Restart the daemon to apply this: {}",
            BRANDING.admin_command("restart")
        );
    }
    Ok(())
}

pub fn print_status(daemon_config: &DaemonConfig) {
    if daemon_config.lan_access {
        println!(
            "Other devices can reach Localify on http://{}",
            get_lan_ip()
        );
        println!("Turn this off with: {} lan disable", BRANDING.cli_command());
    } else {
        println!("Only this machine can reach Localify.");
        println!(
            "To let other devices in: {} lan enable",
            BRANDING.cli_command()
        );
    }
}
