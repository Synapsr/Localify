//! Proton Pass access through the official `pass-cli`.
//!
//! Values are read from `pass-cli` stdout into memory. Nothing is written to
//! disk, and errors never echo stdout, which is where values appear.

use std::io::ErrorKind;
use std::process::{Command, Output, Stdio};

use anyhow::{Result, bail};

use super::SecretStore;
use crate::infrastructure::process;

const PASS_CLI: &str = "pass-cli";
const NOTE_FIELD: &str = "note";

/// Messages `pass-cli` 2.x prints when a vault, item or field does not exist.
const NOT_FOUND_MARKERS: &[&str] = &[
    "Could not find vault",
    "Could not find item",
    "No item found with title",
    "Field does not exist",
];

/// Reads notes and `pass://` references with the user's `pass-cli` session.
pub struct ProtonPassCli;

impl SecretStore for ProtonPassCli {
    fn read_note(&self, vault: &str, title: &str) -> Result<Option<String>> {
        let output = run_pass_cli(&[
            "item",
            "view",
            "--vault-name",
            vault,
            "--item-title",
            title,
            "--field",
            NOTE_FIELD,
        ])?;
        interpret(output)
    }

    fn read_reference(&self, reference: &str) -> Result<Option<String>> {
        let output = run_pass_cli(&["item", "view", reference])?;
        interpret(output)
    }
}

/// Metadata used by `localify profile init` to suggest vaults and check that
/// every service has its note. Titles only, never item content.
impl ProtonPassCli {
    pub fn vault_names(&self) -> Result<Vec<String>> {
        let listing: VaultListing = list_json(&["vault", "list", "--output", "json"])?;
        Ok(listing.vaults.into_iter().map(|vault| vault.name).collect())
    }

    pub fn item_titles(&self, vault: &str) -> Result<Vec<String>> {
        let listing: ItemListing = list_json(&["item", "list", vault, "--output", "json"])?;
        Ok(listing.items.into_iter().map(|item| item.title).collect())
    }
}

#[derive(serde::Deserialize)]
struct VaultListing {
    vaults: Vec<NamedVault>,
}

#[derive(serde::Deserialize)]
struct NamedVault {
    name: String,
}

#[derive(serde::Deserialize)]
struct ItemListing {
    items: Vec<TitledItem>,
}

#[derive(serde::Deserialize)]
struct TitledItem {
    title: String,
}

fn list_json<T: serde::de::DeserializeOwned>(args: &[&str]) -> Result<T> {
    let output = run_pass_cli(args)?;
    if !output.status.success() {
        return match interpret(output) {
            Err(error) => Err(error),
            Ok(_) => bail!("Proton Pass CLI could not find that vault."),
        };
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| anyhow::anyhow!("Unexpected Proton Pass CLI output: {error}"))
}

fn run_pass_cli(args: &[&str]) -> Result<Output> {
    // Known while it runs: a stop while the profile resolves ends it too.
    match process::helpers::helpers().output(Command::new(PASS_CLI).args(args).stdin(Stdio::null()))
    {
        Ok(output) => Ok(output),
        Err(error) if error.kind() == ErrorKind::NotFound => bail!(
            "Proton Pass CLI ('{PASS_CLI}') is not installed or not on PATH.\n\
             Install it with: brew install protonpass/tap/pass-cli\n\
             Then sign in with: {PASS_CLI} login"
        ),
        Err(error) => bail!("Failed to run '{PASS_CLI}': {error}"),
    }
}

fn interpret(output: Output) -> Result<Option<String>> {
    if output.status.success() {
        let mut value = String::from_utf8(output.stdout).map_err(|_| {
            anyhow::anyhow!("Proton Pass returned a value that is not valid UTF-8.")
        })?;
        if value.ends_with('\n') {
            value.pop();
        }
        return Ok(Some(value));
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    if NOT_FOUND_MARKERS
        .iter()
        .any(|marker| stderr.contains(marker))
    {
        return Ok(None);
    }

    if !session_is_active() {
        bail!(
            "Proton Pass CLI is not signed in, or its session expired.\n\
             Run: {PASS_CLI} login"
        );
    }

    bail!("Proton Pass CLI failed: {}", error_summary(&stderr))
}

fn session_is_active() -> bool {
    process::helpers::helpers()
        .output(Command::new(PASS_CLI).arg("info").stdin(Stdio::null()))
        .is_ok_and(|output| output.status.success())
}

/// Keeps the `pass-cli` error chain on one line. stderr carries messages
/// such as "Invalid reference format", never item values.
fn error_summary(stderr: &str) -> String {
    let summary = stderr
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && *line != "Caused by:")
        .map(|line| line.trim_start_matches("Error: "))
        .collect::<Vec<_>>()
        .join(" / ");
    if summary.is_empty() {
        "no error message".into()
    } else {
        summary
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_summary_flattens_the_cause_chain() {
        let stderr = "Error: Invalid item reference\n\nCaused by:\n    Invalid reference format. Expected: pass://SHARE_ID/ITEM_ID[/FIELD], got: [x]\n";

        assert_eq!(
            error_summary(stderr),
            "Invalid item reference / Invalid reference format. Expected: pass://SHARE_ID/ITEM_ID[/FIELD], got: [x]"
        );
    }
}
