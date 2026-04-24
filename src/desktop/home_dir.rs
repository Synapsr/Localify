use std::env;
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
#[cfg(target_os = "macos")]
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};

pub fn user_home_dir() -> Result<PathBuf> {
    if let Some(home_dir) = env::var_os("HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(home_dir));
    }

    fallback_user_home_dir()
}

#[cfg(target_os = "macos")]
fn fallback_user_home_dir() -> Result<PathBuf> {
    let output = Command::new("id")
        .arg("-P")
        .output()
        .context("Failed to resolve the current macOS account details")?;

    if !output.status.success() {
        bail!(
            "HOME is not set for the current user. Localify also could not resolve the current macOS account details: {}",
            cli_failure_message(&output)
        );
    }

    parse_macos_home_dir_from_id_output(&output.stdout)
}

#[cfg(not(target_os = "macos"))]
fn fallback_user_home_dir() -> Result<PathBuf> {
    Err(anyhow!("HOME is not set for the current user."))
}

#[cfg(target_os = "macos")]
fn parse_macos_home_dir_from_id_output(raw: &[u8]) -> Result<PathBuf> {
    let decoded = String::from_utf8_lossy(raw);
    let fields: Vec<&str> = decoded.trim().split(':').collect();
    let home_dir = fields.get(8).copied().filter(|value| !value.is_empty());

    match home_dir {
        Some(path) => Ok(PathBuf::from(path)),
        None => Err(anyhow!(
            "HOME is not set for the current user. Localify also could not resolve the current macOS home folder."
        )),
    }
}

#[cfg(target_os = "macos")]
fn cli_failure_message(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        "The command returned no diagnostic output.".into()
    } else {
        stderr
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn parses_home_dir_from_macos_id_output() {
        let output = b"dev:*:501:20::0:0:Dev:/Users/dev:/bin/zsh\n";

        let home_dir = parse_macos_home_dir_from_id_output(output).unwrap();

        assert_eq!(home_dir, Path::new("/Users/dev"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn rejects_missing_home_field_in_macos_id_output() {
        let output = b"dev:*:501:20::0:0:Dev::/bin/zsh\n";

        let error = parse_macos_home_dir_from_id_output(output).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("HOME is not set for the current user.")
        );
    }
}
