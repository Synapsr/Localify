use anyhow::Result;

use super::backend::WriteBackend;
use super::context::AppContext;
use crate::application::stop_daemon::StopDaemon;

pub fn execute(ctx: &AppContext) -> Result<()> {
    if let WriteBackend::Helper(client) = WriteBackend::resolve(&ctx.config_path)? {
        client.stop_daemon()?;
        println!("Localify daemon stopped.");
        return Ok(());
    }

    let service = StopDaemon::new(&ctx.pid_file);
    service.execute()?;
    println!("Localify daemon stopped.");
    Ok(())
}
