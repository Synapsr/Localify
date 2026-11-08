//! `localify inventory`, hidden: this machine's inventory as the daemon
//! builds it (docs/remote-machines.md, "Inventory"), for tests and for the
//! relay that will send it to paired machines. Nothing sends it anywhere yet.

use anyhow::{Context, Result, anyhow};

use crate::application::inventory::Have;
use crate::application::ports::DaemonConnectionError;
use crate::branding::BRANDING;
use crate::cli::context::AppContext;

/// Prints the snapshot as JSON, or `{unchanged, epoch, rev}` when `have`
/// (`<epoch>:<rev>`) names its revision.
pub fn show(have: Option<&str>, ctx: &AppContext) -> Result<()> {
    let have = have.map(parse_have).transpose()?;
    let snapshot = ctx
        .mgmt_client
        .inventory(have.as_ref())
        .map_err(|error| match error {
            DaemonConnectionError::NotRunning => anyhow!(
                "The inventory comes from the {} daemon, which is not running. Start it with: {} start",
                BRANDING.app_name,
                BRANDING.cli_command()
            ),
            other => anyhow!("Failed to read the inventory from the daemon: {other}"),
        })?;
    println!("{}", serde_json::to_string_pretty(&snapshot)?);
    Ok(())
}

fn parse_have(value: &str) -> Result<Have> {
    let (epoch, rev) = value
        .rsplit_once(':')
        .with_context(|| format!("--have '{value}' is not <epoch>:<rev>"))?;
    let rev = rev
        .parse()
        .with_context(|| format!("--have '{value}': the revision is not a number"))?;
    Ok(Have {
        epoch: epoch.to_string(),
        rev,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn have_reads_epoch_and_revision() {
        let have = parse_have("a-b_c:12").unwrap();

        assert_eq!(
            have,
            Have {
                epoch: "a-b_c".into(),
                rev: 12
            }
        );
        assert!(parse_have("no-revision").is_err());
        assert!(parse_have("epoch:x").is_err());
    }
}
