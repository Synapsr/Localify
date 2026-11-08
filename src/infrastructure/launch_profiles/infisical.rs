//! Infisical access through the official `infisical` CLI.
//!
//! The CLI keeps the login, the instance domain and the project link
//! (`.infisical.yaml`), so Localify stores no token. Values are read from
//! stdout into memory; errors only quote the CLI's stderr, which never holds
//! values.

use std::io::{ErrorKind, Read};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use serde::Deserialize;

use super::InfisicalStore;
use crate::infrastructure::process;

const INFISICAL_CLI: &str = "infisical";

/// A normal call answers in well under a second.
const CLI_TIMEOUT: Duration = Duration::from_secs(20);

/// Files `infisical init` writes to link a folder to a project, current
/// format first.
const PROJECT_FILES: &[&str] = &[".infisical.yaml", ".infisical.json"];

/// Message of the 403 the API returns when the CLI's login profile uses
/// another organization than the project's.
const WRONG_ORGANIZATION: &str = "does not belong to your selected organization";

/// The Infisical project file at a project root, if `infisical init` ran there.
pub fn project_file(root: &Path) -> Option<PathBuf> {
    PROJECT_FILES
        .iter()
        .map(|name| root.join(name))
        .find(|path| path.is_file())
}

/// Secrets of one Infisical folder, held in memory only.
#[derive(Debug, Default)]
pub struct InfisicalFolder {
    pub entries: Vec<(String, String)>,
    /// Keys whose value this account may not read. Not injected.
    pub hidden_keys: Vec<String>,
    /// Login profile that had access, when it is not the one the CLI
    /// selects on its own in this folder.
    pub login_profile: Option<String>,
}

/// Reads secrets with the user's `infisical` login.
pub struct InfisicalCli;

impl InfisicalStore for InfisicalCli {
    fn read_folder(&self, project_root: &Path, env: &str, path: &str) -> Result<InfisicalFolder> {
        let args = [
            "export", "--env", env, "--path", path, "--format", "json", "--silent",
        ];
        let (stdout, login_profile) = run_with_any_profile(project_root, &args)?;
        let secrets: Vec<ExportedSecret> = serde_json::from_slice(&stdout)
            .map_err(|error| anyhow!("unexpected Infisical CLI output: {error}"))?;

        let mut folder = InfisicalFolder {
            login_profile,
            ..InfisicalFolder::default()
        };
        for secret in secrets {
            match secret.value {
                Some(value) if !secret.hidden => folder.entries.push((secret.key, value)),
                _ => folder.hidden_keys.push(secret.key),
            }
        }
        Ok(folder)
    }
}

/// Folder metadata used by `localify profile init` to check that every
/// service has its folder. Names only, never secrets.
impl InfisicalCli {
    pub fn folder_names(
        &self,
        project_root: &Path,
        env: &str,
        parent: &str,
    ) -> Result<Vec<String>> {
        let args = [
            "secrets", "folders", "get", "--env", env, "--path", parent, "--output", "json",
            "--silent",
        ];
        let (stdout, _) = run_with_any_profile(project_root, &args)?;
        let folders: Vec<FolderDto> = serde_json::from_slice(&stdout)
            .map_err(|error| anyhow!("unexpected Infisical CLI output: {error}"))?;
        Ok(folders.into_iter().map(|folder| folder.name).collect())
    }
}

#[derive(Deserialize)]
struct ExportedSecret {
    key: String,
    #[serde(default)]
    value: Option<String>,
    #[serde(default, rename = "secretValueHidden")]
    hidden: bool,
}

#[derive(Deserialize)]
struct FolderDto {
    #[serde(rename = "folderName")]
    name: String,
}

/// Runs the CLI with the login profile it selects in the project folder. A
/// project of another organization is retried with each saved login profile,
/// so a user with several organizations never switches profiles by hand.
fn run_with_any_profile(project_root: &Path, args: &[&str]) -> Result<(Vec<u8>, Option<String>)> {
    let output = run_cli(project_root, args, None)?;
    if output.status.success() {
        return Ok((output.stdout, None));
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.contains(WRONG_ORGANIZATION) {
        bail!(failure_message(&stderr));
    }

    // A profile that fails for another reason (signed out, no access) must
    // not hide a later one that works, so every failure is kept for the end.
    let mut failures = Vec::new();
    for profile in saved_login_profiles() {
        let output = match run_cli(project_root, args, Some(&profile)) {
            Ok(output) => output,
            Err(error) => {
                failures.push(format!("{error:#} (login profile {profile})"));
                continue;
            }
        };
        if output.status.success() {
            return Ok((output.stdout, Some(profile)));
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.contains(WRONG_ORGANIZATION) {
            failures.push(format!(
                "{} (login profile {profile})",
                failure_message(&stderr)
            ));
        }
    }
    if failures.is_empty() {
        bail!(
            "none of your Infisical login profiles uses the organization of this project; add one with: {INFISICAL_CLI} profile create"
        )
    }
    bail!(failures.join("; "))
}

/// Runs one CLI call, killed after `CLI_TIMEOUT`: with an expired session
/// the CLI may start a browser login and wait for it, which must not hold a
/// service launch.
fn run_cli(project_root: &Path, args: &[&str], profile: Option<&str>) -> Result<Output> {
    let mut command = Command::new(INFISICAL_CLI);
    command
        .args(args)
        .current_dir(project_root)
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Its own group, so a timeout also stops what it started.
        .process_group(0);
    if let Some(profile) = profile {
        command.args(["--profile", profile]);
    }
    let spawned = command.spawn();
    // Known from its spawn: a stop while the profile resolves ends it too.
    let _helper = spawned
        .as_ref()
        .ok()
        .map(process::helpers::Helper::register);
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) if error.kind() == ErrorKind::NotFound => bail!(
            "the Infisical CLI ('{INFISICAL_CLI}') is not installed or not on PATH; install it with: brew install infisical/get-cli/infisical, then sign in with: {INFISICAL_CLI} login"
        ),
        Err(error) => bail!("failed to run '{INFISICAL_CLI}': {error}"),
    };

    let stdout = read_in_background(child.stdout.take());
    let stderr = read_in_background(child.stderr.take());
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() >= CLI_TIMEOUT {
            let _ = Command::new("kill")
                .args(["-KILL", &format!("-{}", child.id())])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            let _ = child.kill();
            let _ = child.wait();
            bail!(
                "the Infisical CLI did not answer within {}s; its session may have expired, sign in again with: {INFISICAL_CLI} login",
                CLI_TIMEOUT.as_secs()
            );
        }
        thread::sleep(Duration::from_millis(25));
    };
    Ok(Output {
        status,
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
}

fn read_in_background(pipe: Option<impl Read + Send + 'static>) -> JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut content = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut content);
        }
        content
    })
}

/// Names of the login profiles the CLI saved (`infisical profile list`).
/// Read from its config file because the command has no JSON output.
fn saved_login_profiles() -> Vec<String> {
    let Some(home) = std::env::var_os("HOME") else {
        return Vec::new();
    };
    let path = Path::new(&home).join(".infisical/infisical-config.json");
    std::fs::read(path)
        .ok()
        .and_then(|content| serde_json::from_slice::<CliConfig>(&content).ok())
        .map(|config| {
            config
                .profiles
                .into_iter()
                .map(|profile| profile.name)
                .collect()
        })
        .unwrap_or_default()
}

#[derive(Deserialize)]
struct CliConfig {
    #[serde(default)]
    profiles: Vec<CliProfile>,
}

#[derive(Deserialize)]
struct CliProfile {
    name: String,
}

/// One line for a failed CLI call. The API's `Message:` line explains the
/// problem best; the request URL and ID around it are noise.
fn failure_message(stderr: &str) -> String {
    let lines: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if lines.iter().any(|line| line.contains("infisical init")) {
        return format!(
            "no Infisical project is linked here; run '{INFISICAL_CLI} init' at the project root"
        );
    }
    let message = lines
        .iter()
        .find_map(|line| line.strip_prefix("Message:"))
        .or_else(|| lines.iter().find_map(|line| line.strip_prefix("error:")))
        .or_else(|| lines.first().copied())
        .map(str::trim)
        .unwrap_or("no error message");
    let message = message
        .split(" Please ensure")
        .next()
        .unwrap_or(message)
        .trim_end_matches('.');
    format!("Infisical: {message}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_message_keeps_the_api_message_only() {
        let stderr = "Request: GET https://infisical.example/api/v4/secrets?environment=dev\nInstance: https://infisical.example\nRequest ID: req-1\nResponse Code: 404 Not Found\nMessage: Folder with path '/web' in environment 'qa' was not found. Please ensure the environment slug and secret path is correct.\n";

        assert_eq!(
            failure_message(stderr),
            "Infisical: Folder with path '/web' in environment 'qa' was not found"
        );
    }

    #[test]
    fn failure_message_explains_a_missing_project_link() {
        let stderr = "Please either run infisical init to connect to a project or pass in project id with --projectId flag\n";

        assert!(failure_message(stderr).contains("'infisical init' at the project root"));
    }

    #[test]
    fn failure_message_falls_back_to_the_cli_error() {
        let stderr =
            "error: profile not found: profile 'x' does not exist\nUnable to fetch secrets\n";

        assert_eq!(
            failure_message(stderr),
            "Infisical: profile not found: profile 'x' does not exist"
        );
    }

    #[test]
    fn finds_the_project_file_at_the_root() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(project_file(dir.path()), None);

        std::fs::write(dir.path().join(".infisical.json"), "{}").unwrap();
        assert_eq!(
            project_file(dir.path()),
            Some(dir.path().join(".infisical.json"))
        );
    }

    #[test]
    fn hidden_values_are_not_exported() {
        let secrets: Vec<ExportedSecret> = serde_json::from_str(
            r#"[{"key":"A","value":"1","secretValueHidden":false,"type":"shared"},{"key":"B","value":"","secretValueHidden":true}]"#,
        )
        .unwrap();

        assert!(!secrets[0].hidden);
        assert!(secrets[1].hidden);
    }
}
