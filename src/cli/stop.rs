use anyhow::Result;

use super::context::AppContext;
use crate::application::stop_daemon::StopDaemon;
use crate::config::DaemonConfig;
use crate::infrastructure::managed_daemon::ManagedDaemon;
use crate::infrastructure::paths::LocalifyPaths;

pub fn execute(
    ctx: &AppContext,
    paths: &LocalifyPaths,
    daemon_config: &DaemonConfig,
) -> Result<()> {
    if let Some(managed) = ManagedDaemon::find(&ctx.config_path, paths, daemon_config) {
        managed.stop()?;
        println!("Localify daemon stopped. It stays stopped until 'localify start'.");
        return Ok(());
    }

    let service = StopDaemon::new(&ctx.daemon);
    service.execute()?;
    println!("Localify daemon stopped.");
    Ok(())
}
