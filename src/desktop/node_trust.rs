use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::branding::BRANDING;
use crate::desktop::home_dir::user_home_dir;

const START_MARKER: &str = "# >>> localify node trust >>>";
const END_MARKER: &str = "# <<< localify node trust <<<";

#[derive(Debug, Clone)]
pub struct NodeTrustStatus {
    pub ready: bool,
    pub ca_cert_path: PathBuf,
    pub shell_snippet_path: PathBuf,
    pub shell_integration_ready: bool,
    pub launchctl_ready: bool,
}

#[derive(Debug, Clone)]
pub struct NodeTrustSetup {
    pub status: NodeTrustStatus,
    pub already_ready: bool,
}

#[derive(Debug, Clone)]
pub struct NodeTrustRemoval {
    pub removed: bool,
}

pub fn status(ca_cert_path: &Path) -> Result<NodeTrustStatus> {
    if BRANDING.is_dev_profile() {
        return dev_status(ca_cert_path);
    }

    let home_dir = user_home_dir()?;
    let snippet_path = shell_snippet_path(&home_dir);
    let expected_snippet = shell_snippet_contents(ca_cert_path);
    let expected_block = shell_source_block(&snippet_path);
    let snippet_ready = file_contents_match(&snippet_path, &expected_snippet)?;
    let shell_integration_ready = shell_rc_paths(&home_dir)
        .iter()
        .map(|path| file_contains(path, &expected_block))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .all(|ready| ready);
    let launchctl_ready = launchctl_environment_ready(ca_cert_path)?;

    Ok(NodeTrustStatus {
        ready: ca_cert_path.exists() && snippet_ready && shell_integration_ready && launchctl_ready,
        ca_cert_path: ca_cert_path.to_path_buf(),
        shell_snippet_path: snippet_path,
        shell_integration_ready,
        launchctl_ready,
    })
}

pub fn install(ca_cert_path: &Path) -> Result<NodeTrustSetup> {
    if BRANDING.is_dev_profile() {
        let status = dev_status(ca_cert_path)?;
        return Ok(NodeTrustSetup {
            already_ready: status.ready,
            status,
        });
    }

    if !ca_cert_path.exists() {
        bail!(
            "{} could not enable Node.js certificate trust because the CA certificate was not found at {}.",
            BRANDING.app_name,
            ca_cert_path.display()
        );
    }

    let existing = status(ca_cert_path)?;
    if existing.ready {
        return Ok(NodeTrustSetup {
            status: existing,
            already_ready: true,
        });
    }

    let home_dir = user_home_dir()?;
    let snippet_path = shell_snippet_path(&home_dir);
    let snippet_contents = shell_snippet_contents(ca_cert_path);
    let source_block = shell_source_block(&snippet_path);

    ensure_parent_dir(&snippet_path)?;
    write_file_if_changed(&snippet_path, &snippet_contents)?;

    for rc_path in shell_rc_paths(&home_dir) {
        upsert_managed_block(&rc_path, &source_block)?;
    }

    set_launchctl_environment(ca_cert_path)?;

    Ok(NodeTrustSetup {
        status: status(ca_cert_path)?,
        already_ready: false,
    })
}

pub fn uninstall(ca_cert_path: &Path) -> Result<NodeTrustRemoval> {
    if BRANDING.is_dev_profile() {
        let _ = ca_cert_path;
        return Ok(NodeTrustRemoval { removed: false });
    }

    let home_dir = user_home_dir()?;
    let snippet_path = shell_snippet_path(&home_dir);
    let mut removed = false;

    for rc_path in shell_cleanup_paths(&home_dir) {
        removed |= remove_managed_block(&rc_path)?;
        removed |= remove_legacy_node_trust_lines(&rc_path, ca_cert_path, &snippet_path)?;
    }

    if snippet_path.exists() {
        fs::remove_file(&snippet_path).with_context(|| {
            format!(
                "Failed to remove the {} Node.js trust snippet at {}",
                BRANDING.app_name,
                snippet_path.display()
            )
        })?;
        removed = true;
    }

    prune_empty_dirs(snippet_path.parent())?;
    removed |= unset_launchctl_environment()?;

    let _ = ca_cert_path;

    Ok(NodeTrustRemoval { removed })
}

fn dev_status(ca_cert_path: &Path) -> Result<NodeTrustStatus> {
    let home_dir = user_home_dir()?;
    let snippet_path = shell_snippet_path(&home_dir);
    Ok(NodeTrustStatus {
        ready: ca_cert_path.exists(),
        ca_cert_path: ca_cert_path.to_path_buf(),
        shell_snippet_path: snippet_path,
        shell_integration_ready: true,
        launchctl_ready: true,
    })
}

fn config_home_dir(home_dir: &Path) -> PathBuf {
    env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir.join(".config"))
}

fn shell_snippet_path(home_dir: &Path) -> PathBuf {
    config_home_dir(home_dir)
        .join(BRANDING.slug())
        .join("env")
        .join("node-trust.sh")
}

fn shell_rc_paths(home_dir: &Path) -> [PathBuf; 2] {
    [home_dir.join(".zshrc"), home_dir.join(".bashrc")]
}

fn shell_cleanup_paths(home_dir: &Path) -> [PathBuf; 8] {
    [
        home_dir.join(".zshenv"),
        home_dir.join(".zprofile"),
        home_dir.join(".zshrc"),
        home_dir.join(".zlogin"),
        home_dir.join(".bash_profile"),
        home_dir.join(".bash_login"),
        home_dir.join(".bashrc"),
        home_dir.join(".profile"),
    ]
}

fn shell_snippet_contents(ca_cert_path: &Path) -> String {
    format!(
        "# Added by {app}. Enables Node.js trust for local HTTPS certificates.\nexport NODE_EXTRA_CA_CERTS={ca}\n",
        app = BRANDING.app_name,
        ca = shell_escape(&ca_cert_path.to_string_lossy()),
    )
}

fn shell_source_block(snippet_path: &Path) -> String {
    let quoted = shell_escape(&snippet_path.to_string_lossy());
    format!("{START_MARKER}\nif [ -f {quoted} ]; then\n  . {quoted}\nfi\n{END_MARKER}\n")
}

fn shell_escape(value: &str) -> String {
    format!("'{}'", value.replace('\'', r#"'"'"'"#))
}

fn file_contents_match(path: &Path, expected: &str) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }

    let contents =
        fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?;
    Ok(contents == expected)
}

fn file_contains(path: &Path, expected: &str) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }

    let contents =
        fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?;
    Ok(contents.contains(expected))
}

fn ensure_parent_dir(path: &Path) -> Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };

    fs::create_dir_all(parent).with_context(|| format!("Failed to create {}", parent.display()))
}

fn write_file_if_changed(path: &Path, contents: &str) -> Result<()> {
    if file_contents_match(path, contents)? {
        return Ok(());
    }

    fs::write(path, contents).with_context(|| format!("Failed to write {}", path.display()))
}

fn upsert_managed_block(path: &Path, block: &str) -> Result<()> {
    let existing = if path.exists() {
        fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?
    } else {
        String::new()
    };
    let (stripped, _) = strip_managed_block(&existing);
    let trimmed = stripped.trim_end();
    let updated = if trimmed.is_empty() {
        block.to_string()
    } else {
        format!("{trimmed}\n\n{block}")
    };

    if updated == existing {
        return Ok(());
    }

    fs::write(path, updated).with_context(|| format!("Failed to write {}", path.display()))
}

fn remove_managed_block(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }

    let existing =
        fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?;
    let (stripped, removed) = strip_managed_block(&existing);

    if !removed {
        return Ok(false);
    }

    let updated = if stripped.trim().is_empty() {
        String::new()
    } else {
        format!("{}\n", stripped.trim_end())
    };
    fs::write(path, updated).with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(true)
}

fn remove_legacy_node_trust_lines(
    path: &Path,
    ca_cert_path: &Path,
    snippet_path: &Path,
) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }

    let existing =
        fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?;
    let (stripped, removed) = strip_legacy_node_trust_lines(&existing, ca_cert_path, snippet_path);

    if !removed {
        return Ok(false);
    }

    let updated = if stripped.trim().is_empty() {
        String::new()
    } else {
        format!("{}\n", stripped.trim_end())
    };
    fs::write(path, updated).with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(true)
}

fn strip_legacy_node_trust_lines(
    contents: &str,
    ca_cert_path: &Path,
    snippet_path: &Path,
) -> (String, bool) {
    let has_localify_node_trust = contents.split_inclusive('\n').any(|line| {
        let trimmed = line.trim_end_matches(['\r', '\n']).trim();
        is_localify_node_extra_certs_line(trimmed, ca_cert_path)
            || references_node_trust_snippet(trimmed, snippet_path)
            || is_localify_node_trust_comment(trimmed)
    });

    if !has_localify_node_trust {
        return (contents.to_string(), false);
    }

    let mut stripped = String::new();
    let mut removed = false;

    for line in contents.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\r', '\n']).trim();
        let should_remove = is_localify_node_extra_certs_line(trimmed, ca_cert_path)
            || references_node_trust_snippet(trimmed, snippet_path)
            || is_localify_node_trust_comment(trimmed)
            || is_node_use_system_ca_line(trimmed);

        if should_remove {
            removed = true;
            continue;
        }

        stripped.push_str(line);
    }

    (stripped, removed)
}

fn strip_managed_block(contents: &str) -> (String, bool) {
    let mut stripped = String::new();
    let mut in_block = false;
    let mut removed = false;

    for line in contents.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed == START_MARKER {
            in_block = true;
            removed = true;
            continue;
        }

        if in_block {
            if trimmed == END_MARKER {
                in_block = false;
            }
            continue;
        }

        stripped.push_str(line);
    }

    if in_block {
        removed = true;
    }

    (stripped, removed)
}

fn is_localify_node_extra_certs_line(trimmed: &str, ca_cert_path: &Path) -> bool {
    extract_env_assignment_value(trimmed, "NODE_EXTRA_CA_CERTS")
        .map(|value| value == ca_cert_path.to_string_lossy() || value == "/etc/localify/ca.crt")
        .unwrap_or(false)
}

fn references_node_trust_snippet(trimmed: &str, snippet_path: &Path) -> bool {
    trimmed.contains(snippet_path.to_string_lossy().as_ref())
        || trimmed.contains("/localify/env/node-trust.sh")
}

fn is_localify_node_trust_comment(trimmed: &str) -> bool {
    trimmed
        == format!(
            "# Added by {}. Enables Node.js trust for local HTTPS certificates.",
            BRANDING.app_name
        )
}

fn is_node_use_system_ca_line(trimmed: &str) -> bool {
    extract_env_assignment_value(trimmed, "NODE_USE_SYSTEM_CA")
        .map(|value| value == "1")
        .unwrap_or(false)
}

fn extract_env_assignment_value<'a>(trimmed: &'a str, key: &str) -> Option<&'a str> {
    let value = trimmed
        .strip_prefix("export ")?
        .strip_prefix(key)?
        .strip_prefix('=')?;
    Some(unquote_shell_value(value.trim()))
}

fn unquote_shell_value(value: &str) -> &str {
    value
        .strip_prefix('\'')
        .and_then(|inner| inner.strip_suffix('\''))
        .or_else(|| {
            value
                .strip_prefix('"')
                .and_then(|inner| inner.strip_suffix('"'))
        })
        .unwrap_or(value)
}

fn prune_empty_dirs(path: Option<&Path>) -> Result<()> {
    let Some(path) = path else {
        return Ok(());
    };

    let env_dir = path;
    if env_dir.is_dir() && fs::read_dir(env_dir)?.next().is_none() {
        fs::remove_dir(env_dir)
            .with_context(|| format!("Failed to remove {}", env_dir.display()))?;
    }

    if let Some(localify_dir) = env_dir.parent()
        && localify_dir.is_dir()
        && fs::read_dir(localify_dir)?.next().is_none()
    {
        fs::remove_dir(localify_dir)
            .with_context(|| format!("Failed to remove {}", localify_dir.display()))?;
    }

    Ok(())
}

#[cfg(target_os = "macos")]
fn launchctl_environment_ready(ca_cert_path: &Path) -> Result<bool> {
    Ok(launchctl_getenv("NODE_USE_SYSTEM_CA")?.is_none()
        && launchctl_getenv("NODE_EXTRA_CA_CERTS")?.as_deref()
            == Some(ca_cert_path.to_string_lossy().as_ref()))
}

#[cfg(not(target_os = "macos"))]
fn launchctl_environment_ready(_ca_cert_path: &Path) -> Result<bool> {
    Ok(true)
}

#[cfg(target_os = "macos")]
fn set_launchctl_environment(ca_cert_path: &Path) -> Result<()> {
    if launchctl_getenv("NODE_USE_SYSTEM_CA")?.is_some() {
        run_launchctl(["unsetenv", "NODE_USE_SYSTEM_CA"])?;
    }
    run_launchctl([
        "setenv",
        "NODE_EXTRA_CA_CERTS",
        ca_cert_path.to_string_lossy().as_ref(),
    ])?;
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn set_launchctl_environment(_ca_cert_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(target_os = "macos")]
fn unset_launchctl_environment() -> Result<bool> {
    let had_system_ca = launchctl_getenv("NODE_USE_SYSTEM_CA")?.is_some();
    let had_extra_ca = launchctl_getenv("NODE_EXTRA_CA_CERTS")?.is_some();

    if had_system_ca {
        run_launchctl(["unsetenv", "NODE_USE_SYSTEM_CA"])?;
    }

    if had_extra_ca {
        run_launchctl(["unsetenv", "NODE_EXTRA_CA_CERTS"])?;
    }

    Ok(had_system_ca || had_extra_ca)
}

#[cfg(not(target_os = "macos"))]
fn unset_launchctl_environment() -> Result<bool> {
    Ok(false)
}

#[cfg(target_os = "macos")]
fn launchctl_getenv(key: &str) -> Result<Option<String>> {
    let output = Command::new("launchctl")
        .args(["getenv", key])
        .output()
        .with_context(|| format!("Failed to launch launchctl getenv {key}"))?;

    if !output.status.success() {
        bail!(
            "launchctl getenv {key} failed: {}",
            command_output(&output)
                .unwrap_or_else(|| "launchctl returned no diagnostic output.".into())
        );
    }

    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((!value.is_empty()).then_some(value))
}

#[cfg(target_os = "macos")]
fn run_launchctl<const N: usize>(args: [&str; N]) -> Result<()> {
    let output = Command::new("launchctl")
        .args(args)
        .output()
        .with_context(|| format!("Failed to launch launchctl {}", args.join(" ")))?;

    if output.status.success() {
        return Ok(());
    }

    bail!(
        "launchctl {} failed: {}",
        args.join(" "),
        command_output(&output)
            .unwrap_or_else(|| "launchctl returned no diagnostic output.".into())
    )
}

#[cfg(target_os = "macos")]
fn command_output(output: &std::process::Output) -> Option<String> {
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    match (stdout.is_empty(), stderr.is_empty()) {
        (false, true) => Some(stdout),
        (true, false) => Some(stderr),
        (false, false) => Some(format!("{stdout}\n{stderr}")),
        (true, true) => None,
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        END_MARKER, START_MARKER, shell_escape, strip_legacy_node_trust_lines, strip_managed_block,
    };

    #[test]
    fn strip_managed_block_removes_existing_block() {
        let contents =
            format!("export FOO=1\n\n{START_MARKER}\nmanaged\n{END_MARKER}\n\nexport BAR=2\n");

        let (stripped, removed) = strip_managed_block(&contents);

        assert!(removed);
        assert_eq!(stripped, "export FOO=1\n\n\nexport BAR=2\n");
    }

    #[test]
    fn strip_managed_block_is_noop_without_markers() {
        let contents = "export FOO=1\n";
        let (stripped, removed) = strip_managed_block(contents);

        assert!(!removed);
        assert_eq!(stripped, contents);
    }

    #[test]
    fn shell_escape_quotes_single_quotes() {
        assert_eq!(shell_escape("my'app"), "'my'\"'\"'app'");
    }

    #[test]
    fn shell_snippet_exports_only_localify_ca() {
        let contents = super::shell_snippet_contents(Path::new("/etc/localify/ca.crt"));

        assert!(contents.contains("export NODE_EXTRA_CA_CERTS='/etc/localify/ca.crt'"));
        assert!(!contents.contains("NODE_USE_SYSTEM_CA"));
    }

    #[test]
    fn strip_legacy_node_trust_lines_removes_localify_exports() {
        let contents = "\
export PATH=/usr/bin\n\
# Added by Localify. Enables Node.js trust for local HTTPS certificates.\n\
export NODE_USE_SYSTEM_CA=1\n\
export NODE_EXTRA_CA_CERTS='/etc/localify/ca.crt'\n\
";

        let (stripped, removed) = strip_legacy_node_trust_lines(
            contents,
            Path::new("/etc/localify/ca.crt"),
            Path::new("/Users/test/.config/localify/env/node-trust.sh"),
        );

        assert!(removed);
        assert_eq!(stripped, "export PATH=/usr/bin\n");
    }

    #[test]
    fn strip_legacy_node_trust_lines_keeps_unrelated_node_extra_ca_certs() {
        let contents = "\
export NODE_USE_SYSTEM_CA=1\n\
export NODE_EXTRA_CA_CERTS='/tmp/company-ca.crt'\n\
";

        let (stripped, removed) = strip_legacy_node_trust_lines(
            contents,
            Path::new("/etc/localify/ca.crt"),
            Path::new("/Users/test/.config/localify/env/node-trust.sh"),
        );

        assert!(!removed);
        assert_eq!(stripped, contents);
    }

    #[test]
    fn strip_legacy_node_trust_lines_removes_unmanaged_snippet_source() {
        let contents = "\
source ~/.zshrc\n\
. '/Users/test/.config/localify/env/node-trust.sh'\n\
export NODE_USE_SYSTEM_CA=1\n\
";

        let (stripped, removed) = strip_legacy_node_trust_lines(
            contents,
            Path::new("/etc/localify/ca.crt"),
            Path::new("/Users/test/.config/localify/env/node-trust.sh"),
        );

        assert!(removed);
        assert_eq!(stripped, "source ~/.zshrc\n");
    }
}
