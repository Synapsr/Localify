use std::path::Path;

use anyhow::{Result, bail};

use super::context::AppContext;
use crate::application::ports::DaemonControl;
use crate::application::restart_daemon::RestartDaemon;
use crate::branding::BRANDING;
use crate::config::DaemonConfig;
use crate::infrastructure::managed_daemon::ManagedDaemon;
use crate::infrastructure::paths::LocalifyPaths;

pub fn execute(
    verbose: bool,
    config_path: &Path,
    ctx: &AppContext,
    paths: &LocalifyPaths,
    daemon_config: &DaemonConfig,
) -> Result<()> {
    if let Some(managed) = ManagedDaemon::find(config_path, paths, daemon_config) {
        if !ctx.daemon.is_running()? {
            bail!(
                "Localify daemon is not running.\nStart it with: {}",
                BRANDING.admin_command("start")
            );
        }
        println!("Restarting Localify daemon...");
        let pid = managed.restart()?;
        super::start::print_daemon_started(pid, daemon_config);
        println!("Daemon reloaded with updated configuration.");
        return Ok(());
    }

    let service = RestartDaemon::new(&ctx.daemon, &ctx.config_store);
    let ready = service.reload()?;

    println!("Starting Localify daemon...");
    super::start::execute(
        false,
        verbose,
        config_path,
        &ready.paths,
        &ready.daemon_config,
    )?;

    println!("Daemon reloaded with updated configuration.");
    Ok(())
}
