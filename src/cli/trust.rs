use std::path::Path;

use anyhow::Result;

use super::context::direct_api;

/// Trusts the CA for browsers on this account. The install leaves this for later, so
/// that it asks for one password only.
pub fn execute(config_path: &Path) -> Result<()> {
    let api = direct_api(config_path)?;
    if api.daemon_status()?.ca_installed {
        println!("HTTPS is already trusted.");
        return Ok(());
    }

    println!("Confirm in the macOS window that opens.");
    api.trust_ca()?;
    println!("HTTPS is trusted for this account. If a browser still warns, quit and reopen it.");
    Ok(())
}
