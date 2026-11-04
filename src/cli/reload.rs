use std::path::Path;

use anyhow::Result;
use localify::desktop::dto::StartDaemonRequest;

use super::backend::WriteBackend;
use super::context::AppContext;
use crate::application::restart_daemon::RestartDaemon;

pub fn execute(verbose: bool, config_path: &Path, ctx: &AppContext) -> Result<()> {
    if let WriteBackend::Helper(client) = WriteBackend::resolve(config_path)? {
        println!("Starting Localify daemon...");
        let launch = client.reload_daemon(StartDaemonRequest { verbose })?;
        super::start::print_daemon_started(&launch);
        println!("Daemon reloaded with updated configuration.");
        return Ok(());
    }

    let service = RestartDaemon::new(&ctx.pid_file, &ctx.config_store);
    let ready = service.reload()?;

    println!("Starting Localify daemon...");
    super::start::execute(
        false,
        verbose,
        None,
        config_path,
        &ready.paths,
        &ready.daemon_config,
    )?;

    println!("Daemon reloaded with updated configuration.");
    Ok(())
}
