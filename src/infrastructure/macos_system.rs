//! The only part of Localify on macOS that needs an administrator.
//!
//! Localify's binary never runs as root. Instead the user's process works out
//! what the system should look like, compares it with what is there, and asks
//! for one approval to run a short shell script made of system tools. Everything
//! root writes is inside that script, so nothing changes between the approval and
//! the write. What the script writes: the resolver file, the launchd plist and the
//! terminal command link. CA trust is not in it: that lives in the user's own
//! keychain and needs no administrator. It never changes anything in the
//! user's folder: a process running as the user could plant a link there and steer
//! root through it.
// Both crates compile this module and each uses a part of it.
#![allow(dead_code)]

use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::application::StepOutcome;
use crate::branding::{BRANDING, user_home_dir};
use crate::config::{DaemonConfig, LocalifyPaths};
use crate::infrastructure::certs::ca::pem_is_limited_to_local_domains;
use crate::infrastructure::config::ConfigStore;
use crate::infrastructure::service_sockets::{HTTP_SOCKETS, HTTPS_SOCKETS};

const SYSTEM_KEYCHAIN: &str = "/Library/Keychains/System.keychain";
const RESOLVER_DIR: &str = "/etc/resolver";
const LAUNCH_DAEMONS_DIR: &str = "/Library/LaunchDaemons";
const HELPER_TOOLS_DIR: &str = "/Library/PrivilegedHelperTools";
/// How launchd is told to restart the daemon after an update or `localify restart`.
pub const RESTART_EXIT_CODE: i32 = 75;
/// Matches a daemon older builds started as root. Anchored at the end so it cannot
/// match the shell running this script, whose text contains it.
const ROOT_DAEMON_PATTERN: &str = "[Ll]ocalify.* start --foreground$";
/// The dev profile runs beside an installed app, so it only looks for its own daemon.
const DEV_ROOT_DAEMON_PATTERN: &str = "[L]ocalify Dev.* start --foreground$";

/// `launchctl` target of the daemon service.
fn service_target() -> String {
    format!("system/{}", BRANDING.service_label())
}

pub fn service_plist_path() -> PathBuf {
    PathBuf::from(LAUNCH_DAEMONS_DIR).join(format!("{}.plist", BRANDING.service_label()))
}

fn resolver_file() -> PathBuf {
    PathBuf::from(RESOLVER_DIR).join(BRANDING.domain_label())
}

/// What `/etc/resolver/<domain>` should say.
pub fn resolver_content(port: u16) -> String {
    format!(
        "domain {}\nnameserver 127.0.0.1\nnameserver ::1\nport {}\nsearch_order 1\ntimeout 1\noptions attempts:1\n",
        BRANDING.domain_label(),
        port
    )
}

// ---------- launchd service ----------

/// Everything the daemon's launchd job depends on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceSpec {
    pub user: String,
    pub home: PathBuf,
    pub binary: PathBuf,
    pub config_path: PathBuf,
    pub http_port: u16,
    pub https_port: u16,
    pub lan_access: bool,
    pub ipv6_loopback: bool,
}

/// The job runs as the user from the binary's normal path, so replacing the
/// binary needs no prompt. launchd keeps the listening sockets, as root.
///
/// `KeepAlive` restarts it after a crash or a restart request (a non-zero exit):
/// DNS is served by the daemon itself, so once it is gone names stop resolving
/// and no connection would ever reach the sockets to wake it. A zero exit means
/// the user stopped it, and then only a connection wakes it, which the daemon
/// answers by checking the stop marker and exiting again.
///
/// There is no `StandardOutPath`: launchd could open it as root in a folder the
/// user controls. The daemon writes its own log.
pub fn plist_contents(spec: &ServiceSpec) -> String {
    let arguments = [
        spec.binary.to_string_lossy().to_string(),
        "--config".into(),
        spec.config_path.to_string_lossy().to_string(),
        "start".into(),
        "--foreground".into(),
    ]
    .iter()
    .map(|argument| format!("    <string>{}</string>", escape_xml(argument)))
    .collect::<Vec<_>>()
    .join("\n");

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{label}</string>
  <key>UserName</key>
  <string>{user}</string>
  <key>ProgramArguments</key>
  <array>
{arguments}
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>HOME</key>
    <string>{home}</string>
  </dict>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>ThrottleInterval</key>
  <integer>1</integer>
  <key>Sockets</key>
  <dict>
    <key>{HTTP_SOCKETS}</key>
{http}
    <key>{HTTPS_SOCKETS}</key>
{https}
  </dict>
</dict>
</plist>
"#,
        label = escape_xml(&BRANDING.service_label()),
        user = escape_xml(&spec.user),
        home = escape_xml(&spec.home.to_string_lossy()),
        http = socket_entries(spec.http_port, spec),
        https = socket_entries(spec.https_port, spec),
    )
}

/// With no node name launchd binds every interface, for both address families.
fn socket_entries(port: u16, spec: &ServiceSpec) -> String {
    let mut nodes = Vec::new();
    if spec.lan_access {
        nodes.push(String::new());
    } else {
        nodes.push("        <key>SockNodeName</key>\n        <string>127.0.0.1</string>\n".into());
        if spec.ipv6_loopback {
            nodes.push("        <key>SockNodeName</key>\n        <string>::1</string>\n".into());
        }
    }

    let entries = nodes
        .iter()
        .map(|node| {
            format!(
                "      <dict>\n{node}        <key>SockServiceName</key>\n        <string>{port}</string>\n      </dict>"
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("    <array>\n{entries}\n    </array>")
}

/// The account that owns the installed service, read from our own plist.
fn plist_user(plist: &str) -> Option<&str> {
    let after = plist.split("<key>UserName</key>").nth(1)?;
    after.split("<string>").nth(1)?.split("</string>").next()
}

/// The binary the installed plist runs, read from our own plist.
fn plist_program(plist: &str) -> Option<String> {
    let after = plist.split("<key>ProgramArguments</key>").nth(1)?;
    let value = after.split("<string>").nth(1)?.split("</string>").next()?;
    Some(
        value
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&apos;", "'")
            .replace("&amp;", "&"),
    )
}

pub(crate) fn current_user() -> Result<String> {
    let output = Command::new("/usr/bin/id")
        .arg("-un")
        .output()
        .context("Failed to read the current user name")?;
    let user = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() || user.is_empty() {
        bail!("Failed to read the current user name.");
    }
    Ok(user)
}

/// Whether launchd runs this user's daemon on this config. Another account's service
/// leaves this user with no daemon of their own until they run install.
pub fn service_runs_config(config_path: &Path) -> bool {
    if !BRANDING.uses_launchd_service() {
        return false;
    }
    let Ok(plist) = fs::read_to_string(service_plist_path()) else {
        return false;
    };
    plist_runs(&plist, &current_user().unwrap_or_default(), config_path)
}

fn plist_runs(plist: &str, user: &str, config_path: &Path) -> bool {
    plist_user(plist) == Some(user)
        && plist.contains(&format!(
            "<string>{}</string>",
            escape_xml(&config_path.to_string_lossy())
        ))
}

/// Whether launchd has the job loaded. It can have lost it even with the plist in
/// place: for example someone ran `launchctl bootout` by hand.
fn service_is_loaded() -> bool {
    Command::new("/bin/launchctl")
        .args(["print", &service_target()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

// ---------- what older builds left behind ----------

/// Where older builds put things. Fixed in production, a temp folder in tests.
pub struct LegacyRoots {
    pub launch_daemons: PathBuf,
    pub helper_tools: PathBuf,
    pub etc_dir: PathBuf,
    pub var_run: PathBuf,
    pub var_log: PathBuf,
}

impl LegacyRoots {
    pub fn system() -> Self {
        Self {
            launch_daemons: LAUNCH_DAEMONS_DIR.into(),
            helper_tools: HELPER_TOOLS_DIR.into(),
            etc_dir: "/etc".into(),
            var_run: "/var/run".into(),
            var_log: "/var/log".into(),
        }
    }
}

/// Root-owned leftovers of the old helper and daemon. Each needs the approval.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct LegacyInstall {
    pub helper_labels: Vec<String>,
    pub helper_plists: Vec<PathBuf>,
    pub helper_copies: Vec<PathBuf>,
    pub run_files: Vec<PathBuf>,
    pub config_dir: Option<PathBuf>,
    pub log_dir: Option<PathBuf>,
    pub root_daemon: bool,
}

impl LegacyInstall {
    pub fn detect(roots: &LegacyRoots) -> Self {
        let prefix = BRANDING.legacy_helper_prefix();
        let slug = BRANDING.slug();

        let helper_plists = entries_starting_with(&roots.launch_daemons, &format!("{prefix}."));
        let helper_labels = helper_plists
            .iter()
            .filter_map(|path| path.file_stem())
            .map(|stem| stem.to_string_lossy().to_string())
            .collect();

        let mut run_files = entries_starting_with(&roots.var_run, &format!("{slug}-helper-"));
        let pid_file = roots.var_run.join(format!("{slug}.pid"));
        if pid_file.exists() {
            run_files.push(pid_file);
        }

        let config_dir = Some(roots.etc_dir.join(slug)).filter(|path| path.exists());
        let log_dir = Some(roots.var_log.join(slug)).filter(|path| path.exists());

        Self {
            helper_labels,
            helper_plists,
            helper_copies: entries_starting_with(&roots.helper_tools, &format!("{prefix}.")),
            run_files,
            config_dir,
            log_dir,
            root_daemon: false,
        }
    }

    /// A log folder alone is not an install. Anything can recreate it, like a launchd
    /// job that logs there, and it is not worth an approval. It goes with the rest.
    pub fn is_empty(&self) -> bool {
        self.helper_labels.is_empty()
            && self.helper_plists.is_empty()
            && self.helper_copies.is_empty()
            && self.run_files.is_empty()
            && self.config_dir.is_none()
            && !self.root_daemon
    }

    /// Whether this Mac still has the old root helper, daemon or config.
    pub fn needs_migration() -> bool {
        !Self::on_this_machine().is_empty()
    }

    /// What is on this machine right now, including a running root daemon.
    pub(crate) fn on_this_machine() -> Self {
        Self {
            root_daemon: root_daemon_running(),
            ..Self::detect(&LegacyRoots::system())
        }
    }
}

fn entries_starting_with(dir: &Path, prefix: &str) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(prefix))
        .map(|entry| entry.path())
        .collect();
    paths.sort();
    paths
}

/// Whether this folder, or anything down to `depth` levels inside it, belongs to root.
fn root_daemon_running() -> bool {
    Command::new("/usr/bin/pgrep")
        .args(["-u", "0", "-f", root_daemon_pattern()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Moves the old `/etc` config into the user's folder before anything root runs,
/// so declining the approval loses nothing. Its `[paths]` pointed at root's
/// folders, so they go back to the defaults.
pub fn migrate_legacy_config(config_path: &Path) -> Result<bool> {
    let legacy_path = PathBuf::from("/etc")
        .join(BRANDING.slug())
        .join("config.toml");
    if !BRANDING.uses_system_service() {
        return Ok(false);
    }
    copy_config_to_user_folder(&legacy_path, config_path)
}

fn copy_config_to_user_folder(legacy_path: &Path, config_path: &Path) -> Result<bool> {
    // A folder this user cannot read must not count as no old config: the script removes
    // that folder, so this has to fail and say so.
    if !legacy_path.try_exists().unwrap_or(true) {
        return Ok(false);
    }

    let mut migrated = ConfigStore::new(legacy_path.to_path_buf()).load().with_context(|| {
        format!(
            "Could not read the old config at {0}.\nMake it readable, then try again: sudo chmod 644 {0}",
            legacy_path.display()
        )
    })?;
    // The script removes the old folder, so its domains must not be lost when a newer
    // config already exists. That config keeps its own settings.
    if config_path.exists() {
        let store = ConfigStore::new(config_path.to_path_buf());
        let mut current = store
            .load()
            .context("Failed to read the config in your user folder")?;
        let added = current.adopt_missing_from(migrated);
        if added > 0 {
            store
                .save(&current)
                .context("Failed to save the config in your user folder")?;
        }
        return Ok(added > 0);
    }

    migrated.paths = LocalifyPaths::default();
    // Listening everywhere was the old Linux default rather than a choice.
    if cfg!(target_os = "linux") {
        migrated.daemon.lan_access = false;
    }

    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }
    ConfigStore::new(config_path.to_path_buf())
        .save(&migrated)
        .context("Failed to save the config in your user folder")?;
    Ok(true)
}

// ---------- the approval ----------

/// How to ask for the administrator's approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Approval {
    /// The macOS password dialog, for the app and anything without a terminal.
    Dialog,
    /// `sudo`, for a terminal, which also works over SSH.
    Terminal,
}

impl Approval {
    pub fn for_cli() -> Self {
        if std::io::stdin().is_terminal() {
            Self::Terminal
        } else {
            Self::Dialog
        }
    }
}

const APPROVAL_REASON: &str =
    "Localify needs an administrator's approval to change its DNS and background service.";

/// Runs the script as root, once. Fails if the user declines.
pub fn run_with_approval(script: &str, approval: Approval) -> Result<()> {
    if cfg!(test) {
        bail!("Tests never ask for an administrator's approval.");
    }

    let output = match approval {
        Approval::Terminal => {
            println!("{APPROVAL_REASON}");
            Command::new("/usr/bin/sudo")
                .args(["/bin/sh", "-c", script])
                .stdin(Stdio::inherit())
                .output()
        }
        // Linux has no dialog to show, so with no terminal only passwordless sudo works.
        Approval::Dialog if cfg!(target_os = "linux") => Command::new("/usr/bin/sudo")
            .args(["-n", "/bin/sh", "-c", script])
            .output(),
        Approval::Dialog => Command::new("/usr/bin/osascript")
            .arg("-e")
            .arg(format!(
                "do shell script \"{}\" with prompt \"{}\" with administrator privileges",
                escape_applescript(script),
                escape_applescript(APPROVAL_REASON)
            ))
            .output(),
    }
    .context("Failed to ask for an administrator's approval")?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    if cfg!(target_os = "linux") && stderr.contains("a password is required") {
        bail!(
            "sudo needs a password, and this session has no terminal to ask for it. Run `{}` from a terminal.",
            BRANDING.admin_command("install")
        );
    }
    if stderr.contains("(-128)") || stderr.contains("User canceled") {
        bail!("The administrator approval was declined, so nothing was changed.");
    }
    if stderr.contains("(-60007)") || stderr.contains("(-1713)") || stderr.contains("(-609)") {
        bail!(
            "macOS could not show the approval dialog because this session has no screen.\n\
             Run `{}` from a terminal instead: it uses sudo.",
            BRANDING.admin_command("install")
        );
    }
    bail!(
        "The system setup failed: {}",
        if stderr.trim().is_empty() {
            "no diagnostic output".into()
        } else {
            stderr.trim().to_string()
        }
    );
}

pub(crate) fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r#"'"'"'"#))
}

fn escape_applescript(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

// ---------- the script ----------

/// A symlink to place at `link` pointing at `target`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliLink {
    pub link: PathBuf,
    pub target: PathBuf,
}

/// The system changes that are actually needed, so that nothing is prompted for
/// when everything is already right.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SystemChanges {
    pub legacy: LegacyInstall,
    pub stale_keychain_hashes: Vec<String>,
    pub resolver_content: Option<String>,
    pub service_plist: Option<String>,
    pub service_ports: Vec<u16>,
    pub cli_link: Option<CliLink>,
}

impl SystemChanges {
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    /// The script root runs. It names only system tools by absolute path: the
    /// Localify binary appears in it only as data inside the plist and as a link
    /// target. Cleaning up what is already gone fails harmlessly, so the status of
    /// each command matters only where it calls `fail`.
    pub fn script(&self) -> String {
        let mut script = vec![
            "umask 022".to_string(),
            r#"fail() { echo "$1" >&2; exit 1; }"#.to_string(),
        ];
        // The old daemon stops first, so the ports are free. Its files and trust go last:
        // if anything fails, the config, certificates and logs are still there for
        // another try.
        script.extend(legacy_stop_commands(&self.legacy));
        // The service goes before the resolver and the link: if it fails, the script
        // stops with both still matching the old service.
        script.extend(self.service_commands());
        script.extend(self.resolver_commands());
        script.extend(self.cli_link_commands());
        script.extend(legacy_removal_commands(&self.legacy));
        script.extend(keychain_commands(&self.stale_keychain_hashes));
        script.push("exit 0".to_string());
        script.join("\n")
    }

    fn resolver_commands(&self) -> Vec<String> {
        let Some(content) = &self.resolver_content else {
            return Vec::new();
        };
        let file = shell_quote(&resolver_file().to_string_lossy());
        vec![
            format!("/bin/mkdir -p {RESOLVER_DIR} || fail 'Could not create {RESOLVER_DIR}.'"),
            format!(
                "/usr/bin/printf '%s' {} > {file} || fail 'Could not write the resolver file.'",
                shell_quote(content)
            ),
            format!("/bin/chmod 644 {file}"),
        ]
    }

    fn cli_link_commands(&self) -> Vec<String> {
        let Some(link) = &self.cli_link else {
            return Vec::new();
        };
        let path = link.link.to_string_lossy();
        let parent = link
            .link
            .parent()
            .map(|parent| parent.to_string_lossy().to_string())
            .unwrap_or_else(|| "/".into());
        // Root writes here, so a folder the user could swap for a link must not steer it.
        let checks = link
            .link
            .ancestors()
            .skip(1)
            .filter(|folder| *folder != Path::new("/"))
            .map(|folder| format!("root_controls {}", shell_quote(&folder.to_string_lossy())))
            .collect::<Vec<_>>()
            .join(" && ");
        vec![
            r#"root_controls() { [ ! -L "$1" ] && { [ ! -e "$1" ] || [ "$(/usr/bin/stat -f %u "$1")" = 0 ]; }; }"#
                .to_string(),
            format!("if {checks}; then"),
            format!("/bin/mkdir -p {}", shell_quote(&parent)),
            format!(
                "/bin/ln -sfn {} {} || fail 'Could not link the terminal command.'",
                shell_quote(&link.target.to_string_lossy()),
                shell_quote(&path)
            ),
            "else".to_string(),
            "echo 'Skipped the terminal command link: its folder is not controlled by root.' >&2"
                .to_string(),
            "fi".to_string(),
        ]
    }

    /// Writes the plist and loads it. launchd loads a job even when it could not open
    /// a port, so the script also checks that launchd holds each one on IPv4. If not,
    /// the previous plist goes back and the error says how to check. This can't tell
    /// our socket from another job's on the same port. The daemon catches that case,
    /// in `DaemonListeners::ensure_ipv4`.
    fn service_commands(&self) -> Vec<String> {
        let Some(plist) = &self.service_plist else {
            return Vec::new();
        };
        let path = shell_quote(&service_plist_path().to_string_lossy());
        let previous = shell_quote(&format!(
            "{}.previous",
            service_plist_path().to_string_lossy()
        ));
        let target = shell_quote(&service_target());
        let ports = self
            .service_ports
            .iter()
            .map(u16::to_string)
            .collect::<Vec<_>>()
            .join(" ");
        vec![
            format!("/bin/launchctl bootout {target} 2>/dev/null"),
            format!("[ -f {path} ] && /bin/mv {path} {previous}"),
            format!(
                "/usr/bin/printf '%s' {} > {path} || fail 'Could not write the service plist.'",
                shell_quote(plist)
            ),
            format!("/usr/sbin/chown root:wheel {path} && /bin/chmod 644 {path}"),
            format!("/bin/launchctl enable {target}"),
            format!(
                "n=0; until error=$(/bin/launchctl bootstrap system {path} 2>&1); do n=$((n+1)); [ $n -lt 5 ] || break; /bin/sleep 1; done"
            ),
            format!(
                r#"taken=""; if [ $n -lt 5 ]; then for port in {ports}; do /usr/sbin/lsof -nP -a -p 1 -iTCP:$port -sTCP:LISTEN 2>/dev/null | /usr/bin/grep -q IPv4 || taken="$taken $port"; done; fi"#
            ),
            format!(
                r#"if [ $n -ge 5 ] || [ -n "$taken" ]; then
  /bin/launchctl bootout {target} 2>/dev/null
  /bin/rm -f {path}
  if [ -f {previous} ]; then
    /bin/mv {previous} {path}
    m=0; until /bin/launchctl bootstrap system {path} 2>/dev/null; do m=$((m+1)); [ $m -lt 5 ] || break; /bin/sleep 1; done
  fi
  [ -z "$taken" ] || fail "Another program holds port$taken, so launchd could not open it. Check with: sudo lsof -nP -iTCP -sTCP:LISTEN"
  fail "launchd could not load the service: $error"
fi"#
            ),
            format!("/bin/rm -f {previous}"),
        ]
    }
}

fn root_daemon_pattern() -> &'static str {
    if BRANDING.is_dev_profile() {
        DEV_ROOT_DAEMON_PATTERN
    } else {
        ROOT_DAEMON_PATTERN
    }
}

/// Stops what is running. It has to go first, so the ports are free.
pub(crate) fn legacy_stop_commands(legacy: &LegacyInstall) -> Vec<String> {
    let mut commands = Vec::new();

    for label in &legacy.helper_labels {
        commands.push(format!(
            "/bin/launchctl bootout {} 2>/dev/null",
            shell_quote(&format!("system/{label}"))
        ));
    }
    // A daemon started with sudo has no helper, only these traces of it.
    if !legacy.is_empty() {
        let pattern = shell_quote(root_daemon_pattern());
        commands.push(format!("/usr/bin/pkill -u 0 -f {pattern}"));
        commands.push(format!(
            "n=0; while /usr/bin/pgrep -u 0 -f {pattern} >/dev/null && [ $n -lt 20 ]; do /bin/sleep 0.1; n=$((n+1)); done"
        ));
        commands.push(format!("/usr/bin/pkill -9 -u 0 -f {pattern}"));
    }
    commands
}

/// Removes what the old install left. The scripts run this only once the new service
/// works, so a failure leaves the old files for another try.
pub(crate) fn legacy_removal_commands(legacy: &LegacyInstall) -> Vec<String> {
    let mut commands = Vec::new();

    for path in legacy
        .helper_plists
        .iter()
        .chain(&legacy.helper_copies)
        .chain(&legacy.run_files)
    {
        commands.push(format!(
            "/bin/rm -f {}",
            shell_quote(&path.to_string_lossy())
        ));
    }
    if let Some(dir) = &legacy.config_dir {
        commands.extend(remove_trust_command(&dir.join("ca.crt")));
        commands.push(format!(
            "/bin/rm -rf {}",
            shell_quote(&dir.to_string_lossy())
        ));
    }
    if let Some(dir) = &legacy.log_dir {
        commands.push(format!(
            "/bin/rm -rf {}",
            shell_quote(&dir.to_string_lossy())
        ));
    }
    commands
}

pub(crate) fn legacy_commands(legacy: &LegacyInstall) -> Vec<String> {
    let mut commands = legacy_stop_commands(legacy);
    commands.extend(legacy_removal_commands(legacy));
    commands
}

fn remove_trust_command(cert: &Path) -> Option<String> {
    Some(format!(
        "[ -f {0} ] && /usr/bin/security remove-trusted-cert -d {0} 2>/dev/null",
        shell_quote(&cert.to_string_lossy())
    ))
}

fn keychain_commands(hashes: &[String]) -> Vec<String> {
    hashes
        .iter()
        .map(|hash| {
            format!(
                "/usr/bin/security delete-certificate -Z {} {SYSTEM_KEYCHAIN} 2>/dev/null",
                shell_quote(hash)
            )
        })
        .collect()
}

/// What uninstall takes back out, in one approval.
#[derive(Debug, Default)]
pub struct SystemRemoval {
    pub legacy: LegacyInstall,
    pub keychain_hashes: Vec<String>,
    pub cli_link: Option<PathBuf>,
    pub service: bool,
}

impl SystemRemoval {
    pub fn script(&self) -> String {
        let mut script = vec!["umask 022".to_string()];
        script.extend(legacy_commands(&self.legacy));
        if self.service {
            script.push(format!(
                "/bin/launchctl bootout {} 2>/dev/null",
                shell_quote(&service_target())
            ));
            script.push(format!(
                "/bin/rm -f {}",
                shell_quote(&service_plist_path().to_string_lossy())
            ));
        }
        script.extend(keychain_commands(&self.keychain_hashes));
        script.push(format!(
            "/bin/rm -f {}",
            shell_quote(&resolver_file().to_string_lossy())
        ));
        if let Some(link) = &self.cli_link {
            script.push(format!(
                "/bin/rm -f {}",
                shell_quote(&link.to_string_lossy())
            ));
        }
        script.push("exit 0".to_string());
        script.join("\n")
    }
}

// ---------- reading the current state ----------

/// Fingerprints of every CA with our name in the system keychain.
pub fn system_keychain_ca_hashes() -> Result<Vec<String>> {
    keychain_ca_hashes(Path::new(SYSTEM_KEYCHAIN))
}

fn keychain_ca_hashes(keychain: &Path) -> Result<Vec<String>> {
    let output = Command::new("/usr/bin/security")
        .args(["find-certificate", "-c"])
        .arg(BRANDING.ca_name())
        .args(["-a", "-Z"])
        .arg(keychain)
        .output()
        .context("Failed to read the keychain")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("could not be found") {
            return Ok(Vec::new());
        }
        bail!("Failed to read {}: {}", keychain.display(), stderr.trim());
    }

    Ok(parse_keychain_hashes(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

/// Whether this CA is stored in the login keychain. Trust settings alone are not
/// enough: the daemon sends only the leaf certificate, so clients must find the CA
/// there to build the chain.
pub fn login_keychain_holds(ca_cert: &Path) -> Result<bool> {
    let hash = certificate_sha1(ca_cert)?;
    Ok(keychain_ca_hashes(&login_keychain_path()?)?.contains(&hash))
}

pub fn login_keychain_path() -> Result<PathBuf> {
    let output = Command::new("/usr/bin/security")
        .arg("login-keychain")
        .output()
        .context("Failed to find the login keychain")?;
    let path = String::from_utf8_lossy(&output.stdout)
        .trim()
        .trim_matches('"')
        .to_string();
    if output.status.success() && !path.is_empty() {
        return Ok(path.into());
    }
    let home = user_home_dir().context("HOME is not set for the current user.")?;
    Ok(home.join("Library/Keychains/login.keychain-db"))
}

/// Removes CAs with our name from the login keychain, except the one to keep.
/// Older builds also trusted their CA there, and its key is about to be deleted.
pub fn delete_login_keychain_cas(keep: Option<&str>) -> Result<usize> {
    let keychain = login_keychain_path()?;
    let mut removed = 0;
    for hash in keychain_ca_hashes(&keychain)? {
        if keep == Some(hash.as_str()) {
            continue;
        }
        let output = Command::new("/usr/bin/security")
            .args(["delete-certificate", "-Z", &hash])
            .arg(&keychain)
            .output()
            .context("Failed to update the login keychain")?;
        if !output.status.success() {
            bail!(
                "Failed to remove an old CA from the login keychain: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        removed += 1;
    }
    Ok(removed)
}

/// How long to wait for someone to answer macOS's trust confirmation, so a command
/// on a machine nobody is watching fails instead of hanging.
const TRUST_CONFIRMATION_TIMEOUT: Duration = Duration::from_secs(180);

/// Trusts the CA for this account only: no administrator, no other account, and
/// macOS shows its usual confirmation. Refuses a CA that could sign for other sites.
pub fn trust_in_login_keychain(ca_cert: &Path) -> Result<()> {
    let pem = fs::read_to_string(ca_cert)
        .with_context(|| format!("Failed to read the CA certificate at {}", ca_cert.display()))?;
    if !pem_is_limited_to_local_domains(&pem).map_err(anyhow::Error::from)? {
        bail!(
            "The CA at {} can sign for any site, so {} will not trust it.",
            ca_cert.display(),
            BRANDING.app_name
        );
    }

    let current = certificate_sha1(ca_cert)?;
    delete_login_keychain_cas(Some(&current))?;

    let mut child = Command::new("/usr/bin/security")
        .args(["add-trusted-cert", "-r", "trustRoot", "-k"])
        .arg(login_keychain_path()?)
        .arg(ca_cert)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to ask macOS to trust the CA for this account")?;

    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().context("Failed to wait for macOS")? {
            break status;
        }
        if started.elapsed() > TRUST_CONFIRMATION_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Nobody confirmed the macOS window in time. Run it again and confirm.");
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    if !status.success() {
        let mut stderr = String::new();
        if let Some(mut pipe) = child.stderr.take() {
            let _ = pipe.read_to_string(&mut stderr);
        }
        bail!(
            "macOS did not trust the CA for this account: {}. Run this on the Mac itself, with someone at the screen.",
            stderr.trim()
        );
    }
    Ok(())
}

/// Takes the CA out of the login keychain, for uninstall and for what older builds
/// left there. macOS keeps its record of having trusted that certificate, because
/// removing the record takes one more confirmation on screen. The uninstall deletes
/// the key as well, so only a copy of the key made before that could use the record,
/// and the name constraint limits it to our suffix.
pub fn remove_login_keychain_ca() -> Option<(String, StepOutcome)> {
    match delete_login_keychain_cas(None) {
        Ok(0) => None,
        Ok(_) => Some((
            "Remove CA from your login keychain".into(),
            StepOutcome::Success("Removed.".into()),
        )),
        Err(error) => Some((
            "Remove CA from your login keychain".into(),
            StepOutcome::Warning(error.to_string()),
        )),
    }
}

/// The app's login item. The app removes it through its autostart plugin, but
/// `localify uninstall` runs without the app.
pub fn remove_login_item() -> Option<(String, StepOutcome)> {
    remove_login_item_from(&user_home_dir()?.join("Library/LaunchAgents"))
}

fn remove_login_item_from(launch_agents: &Path) -> Option<(String, StepOutcome)> {
    let path = launch_agents.join(format!("{}.plist", BRANDING.app_name));
    let contents = fs::read_to_string(&path).ok()?;
    // Another app could use the same file name. Ours is the one that passes this flag.
    if !contents.contains("--autostart") {
        return None;
    }
    let outcome = match fs::remove_file(&path) {
        Ok(()) => StepOutcome::Success("Removed.".into()),
        Err(error) => StepOutcome::Warning(error.to_string()),
    };
    Some(("Remove login item".into(), outcome))
}

pub fn parse_keychain_hashes(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| line.strip_prefix("SHA-1 hash:"))
        .map(normalize_hash)
        .filter(|hash| !hash.is_empty())
        .collect()
}

pub fn certificate_sha1(cert_path: &Path) -> Result<String> {
    let output = Command::new("/usr/bin/openssl")
        .arg("x509")
        .arg("-in")
        .arg(cert_path)
        .args(["-noout", "-fingerprint", "-sha1"])
        .output()
        .context("Failed to run openssl to read the CA fingerprint")?;
    if !output.status.success() {
        bail!(
            "Failed to read the CA fingerprint: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    parse_openssl_fingerprint(&String::from_utf8_lossy(&output.stdout))
}

pub fn parse_openssl_fingerprint(output: &str) -> Result<String> {
    output
        .lines()
        .find_map(|line| line.split('=').nth(1))
        .map(normalize_hash)
        .filter(|hash| !hash.is_empty())
        .with_context(|| format!("Unexpected openssl output: {}", output.trim()))
}

fn normalize_hash(value: &str) -> String {
    value.trim().replace(':', "").to_uppercase()
}

/// launchd will run this path from now on, so it has to stay there. macOS runs an
/// app it has not cleared yet from a random temporary path, which is gone later, and
/// a disk image goes away when it is ejected.
fn check_service_binary(binary: &Path) -> Result<()> {
    if binary.to_string_lossy().contains("/AppTranslocation/") || binary.starts_with("/Volumes") {
        bail!(
            "macOS is running {0} from a temporary location. Move {0} to Applications, open it from there, then try again.",
            BRANDING.app_name
        );
    }
    if !binary.exists() {
        bail!(
            "The {} binary was not found at {}.",
            BRANDING.app_name,
            binary.display()
        );
    }
    Ok(())
}

fn is_same_file(first: &Path, second: &Path) -> bool {
    match (fs::canonicalize(first), fs::canonicalize(second)) {
        (Ok(first), Ok(second)) => first == second,
        _ => false,
    }
}

fn ipv6_loopback_available() -> bool {
    std::net::TcpListener::bind("[::1]:0").is_ok()
}

/// What the machine should look like for this user and config.
pub struct SystemRequest<'a> {
    pub config_path: &'a Path,
    pub binary: &'a Path,
    pub daemon: &'a DaemonConfig,
    pub paths: &'a LocalifyPaths,
    pub cli_link: Option<CliLink>,
}

impl SystemRequest<'_> {
    pub(crate) fn service_spec(&self) -> Result<ServiceSpec> {
        check_service_binary(self.binary)?;
        Ok(ServiceSpec {
            user: current_user()?,
            home: user_home_dir().context("HOME is not set for the current user.")?,
            binary: self.service_binary(),
            config_path: self.config_path.to_path_buf(),
            http_port: self.daemon.http_port,
            https_port: self.daemon.https_port,
            lan_access: self.daemon.lan_access,
            ipv6_loopback: ipv6_loopback_available(),
        })
    }

    /// The path the service should run. The app and the terminal command can reach
    /// one binary by two paths, such as through the `/usr/local/bin` link. That is
    /// no reason to ask for an approval, so a plist that already runs it stays.
    fn service_binary(&self) -> PathBuf {
        let installed = fs::read_to_string(service_plist_path())
            .ok()
            .as_deref()
            .and_then(plist_program)
            .map(PathBuf::from);
        match installed {
            Some(installed) if is_same_file(&installed, self.binary) => installed,
            _ => self.binary.to_path_buf(),
        }
    }

    fn service_matches(&self) -> bool {
        let Ok(spec) = self.service_spec() else {
            return false;
        };
        fs::read_to_string(service_plist_path())
            .map(|installed| installed == plist_contents(&spec))
            .unwrap_or(false)
    }

    fn resolver_matches(&self) -> bool {
        fs::read_to_string(resolver_file())
            .map(|installed| installed == resolver_content(self.daemon.dns_port))
            .unwrap_or(false)
    }

    /// Whether the system still needs an approval, for the app and `status`. Trust
    /// is left out: it is reported on its own, and asking again would not help a
    /// machine with no screen.
    pub fn setup_required(&self) -> bool {
        if !LegacyInstall::on_this_machine().is_empty() || !self.resolver_matches() {
            return true;
        }
        BRANDING.uses_launchd_service() && !self.service_matches()
    }

    /// Compares the machine with this request. Any CA with our name in the system
    /// keychain comes from an older build, since trust now lives in the user's own.
    pub fn changes(&self) -> Result<SystemChanges> {
        let legacy = LegacyInstall::on_this_machine();
        let mut changes = SystemChanges {
            resolver_content: (!self.resolver_matches())
                .then(|| resolver_content(self.daemon.dns_port)),
            cli_link: self.cli_link.clone(),
            stale_keychain_hashes: system_keychain_ca_hashes()?,
            ..SystemChanges::default()
        };

        if BRANDING.uses_launchd_service() && (!self.service_matches() || !service_is_loaded()) {
            changes.service_plist = Some(plist_contents(&self.service_spec()?));
            changes.service_ports = vec![self.daemon.http_port, self.daemon.https_port];
        }

        changes.legacy = legacy;
        Ok(changes)
    }
}

/// An old dev helper can leave files in the user's folder owned by root. Root must
/// not fix that: a process running as the user could plant a hard link there, and a
/// `chown` as root would hand over the system file it points to. So the user's
/// process deals with them alone. A readable file becomes the user's by being copied
/// over itself, and anything else moves aside, with the command to remove it.
pub fn reclaim_root_owned_state() {
    let moved = reclaim_entry(&BRANDING.config_dir(), 3, &|_, metadata| {
        metadata.uid() == 0
    });
    for path in moved {
        eprintln!(
            "Moved a file an older version left to root: {0}\nRemove it with: sudo rm -rf {0}",
            shell_quote(&path.to_string_lossy())
        );
    }
}

fn reclaim_entry(
    path: &Path,
    depth: u32,
    is_root_owned: &dyn Fn(&Path, &fs::Metadata) -> bool,
) -> Vec<PathBuf> {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return Vec::new();
    };
    if is_root_owned(path, &metadata) {
        if metadata.is_file() && copy_over_itself(path, &metadata).is_ok() {
            return Vec::new();
        }
        return move_aside(path).into_iter().collect();
    }
    if !metadata.is_dir() || depth == 0 {
        return Vec::new();
    }
    let Ok(entries) = fs::read_dir(path) else {
        return Vec::new();
    };
    entries
        .flatten()
        .flat_map(|entry| reclaim_entry(&entry.path(), depth - 1, is_root_owned))
        .collect()
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn copy_over_itself(path: &Path, metadata: &fs::Metadata) -> io::Result<()> {
    let contents = fs::read(path)?;
    let temp = with_suffix(path, ".reclaiming");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(metadata.permissions().mode() & 0o777)
        .open(&temp)?;
    let written = file
        .write_all(&contents)
        .and_then(|()| fs::rename(&temp, path));
    if written.is_err() {
        let _ = fs::remove_file(&temp);
    }
    written
}

fn move_aside(path: &Path) -> Option<PathBuf> {
    let free = (0..100)
        .map(|index| with_suffix(path, &format!(".root-owned-{index}")))
        .find(|candidate| fs::symlink_metadata(candidate).is_err())?;
    fs::rename(path, &free).ok().map(|()| free)
}

/// Whether this Mac has been through install, so a config change has to be applied
/// to the system too.
pub fn system_setup_done() -> bool {
    resolver_file().exists()
}

/// Everything uninstall has to take back out on this machine, old install included.
pub fn removal_for_this_machine(cli_link: Option<&Path>) -> Result<SystemRemoval> {
    Ok(SystemRemoval {
        legacy: LegacyInstall::on_this_machine(),
        keychain_hashes: system_keychain_ca_hashes().unwrap_or_default(),
        cli_link: cli_link.map(Path::to_path_buf),
        service: BRANDING.uses_launchd_service() && service_plist_path().exists(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> ServiceSpec {
        ServiceSpec {
            user: "dev".into(),
            home: "/Users/dev".into(),
            binary: "/Applications/Localify.app/Contents/MacOS/localify".into(),
            config_path: "/Users/dev/Library/Application Support/Localify Desktop/config.toml"
                .into(),
            http_port: 80,
            https_port: 443,
            lan_access: false,
            ipv6_loopback: true,
        }
    }

    /// Tests can't create files owned by root, so they name the ones to treat that way.
    fn root_owned(names: &'static [&'static str]) -> impl Fn(&Path, &fs::Metadata) -> bool {
        move |path, _| {
            path.file_name()
                .is_some_and(|name| names.iter().any(|candidate| name == *candidate))
        }
    }

    #[test]
    fn a_hard_link_to_a_system_file_leaves_that_file_alone() {
        let dir = tempfile::tempdir().unwrap();
        let system_file = dir.path().join("system-file");
        fs::write(&system_file, "system").unwrap();
        let state = dir.path().join("state");
        fs::create_dir(&state).unwrap();
        fs::hard_link(&system_file, state.join("planted")).unwrap();

        let moved = reclaim_entry(&state, 3, &root_owned(&["planted"]));

        assert!(moved.is_empty());
        assert_eq!(fs::read_to_string(state.join("planted")).unwrap(), "system");
        let system = fs::metadata(&system_file).unwrap();
        assert_ne!(
            fs::metadata(state.join("planted")).unwrap().ino(),
            system.ino()
        );
        assert_eq!(system.nlink(), 1);
    }

    #[test]
    fn what_cannot_be_copied_moves_aside_without_following_links() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let certs = state.join("certs");
        fs::create_dir_all(&certs).unwrap();
        fs::write(certs.join("inside"), "kept").unwrap();
        let elsewhere = dir.path().join("elsewhere");
        fs::write(&elsewhere, "elsewhere").unwrap();
        std::os::unix::fs::symlink(&elsewhere, state.join("link")).unwrap();
        fs::write(state.join("mine"), "mine").unwrap();

        let mut moved = reclaim_entry(&state, 3, &root_owned(&["certs", "link"]));
        moved.sort();

        assert_eq!(
            moved,
            vec![
                state.join("certs.root-owned-0"),
                state.join("link.root-owned-0")
            ]
        );
        assert_eq!(
            fs::read_to_string(state.join("certs.root-owned-0/inside")).unwrap(),
            "kept"
        );
        assert_eq!(fs::read_to_string(&elsewhere).unwrap(), "elsewhere");
        assert_eq!(fs::read_to_string(state.join("mine")).unwrap(), "mine");
    }

    #[test]
    fn root_never_changes_owners_in_the_users_folder() {
        for script in [all_changes().script(), full_removal().script()] {
            assert!(!script.contains("chown -R"), "{script}");
        }
    }

    #[test]
    fn the_terminal_link_is_only_written_through_folders_root_controls() {
        let script = SystemChanges {
            cli_link: Some(CliLink {
                link: "/usr/local/bin/localify".into(),
                target: "/Applications/Localify.app/Contents/MacOS/localify".into(),
            }),
            ..SystemChanges::default()
        }
        .script();

        assert_valid_sh(&script);
        for folder in ["/usr/local/bin", "/usr/local"] {
            assert!(
                script.contains(&format!("root_controls '{folder}'")),
                "{script}"
            );
        }
    }

    fn empty_roots(dir: &Path) -> LegacyRoots {
        let root = |name: &str| {
            let path = dir.join(name);
            fs::create_dir_all(&path).unwrap();
            path
        };
        LegacyRoots {
            launch_daemons: root("LaunchDaemons"),
            helper_tools: root("PrivilegedHelperTools"),
            etc_dir: root("etc"),
            var_run: root("run"),
            var_log: root("log"),
        }
    }

    #[test]
    fn the_service_runs_as_the_user_from_the_binary_path() {
        let plist = plist_contents(&spec());

        assert!(plist.contains("<key>UserName</key>\n  <string>dev</string>"));
        assert!(
            plist.contains("<string>/Applications/Localify.app/Contents/MacOS/localify</string>")
        );
        assert!(plist.contains("<key>HOME</key>\n    <string>/Users/dev</string>"));
        assert_eq!(plist_user(&plist), Some("dev"));
    }

    #[test]
    fn a_service_only_counts_for_its_own_user_and_config() {
        let plist = plist_contents(&spec());
        let config = spec().config_path;

        assert!(plist_runs(&plist, "dev", &config));
        assert!(!plist_runs(&plist, "other", &config));
        assert!(!plist_runs(&plist, "dev", Path::new("/tmp/another.toml")));
    }

    #[test]
    fn it_reads_back_the_binary_the_plist_runs() {
        let plist = plist_contents(&ServiceSpec {
            binary: "/Applications/Localify & Dev.app/localify".into(),
            ..spec()
        });

        assert_eq!(
            plist_program(&plist).as_deref(),
            Some("/Applications/Localify & Dev.app/localify")
        );
    }

    #[test]
    fn a_translocated_app_cannot_become_the_service_binary() {
        let error = check_service_binary(Path::new(
            "/private/var/folders/x/AppTranslocation/y/d/Localify.app/Contents/MacOS/localify",
        ))
        .unwrap_err();

        assert!(error.to_string().contains("Move"));
    }

    #[test]
    fn an_app_on_a_disk_image_cannot_become_the_service_binary() {
        let error = check_service_binary(Path::new(
            "/Volumes/Localify/Localify.app/Contents/MacOS/localify",
        ))
        .unwrap_err();

        assert!(error.to_string().contains("Move"));
    }

    #[test]
    fn the_service_starts_at_boot_and_restarts_quickly_only_after_a_failure() {
        let plist = plist_contents(&spec());

        assert!(plist.contains("<key>RunAtLoad</key>\n  <true/>"));
        assert!(plist.contains("<key>ThrottleInterval</key>\n  <integer>1</integer>"));
        assert!(plist.contains("<key>SuccessfulExit</key>\n    <false/>"));
    }

    #[test]
    fn the_service_listens_on_loopback_by_default() {
        let plist = plist_contents(&spec());

        assert_eq!(plist.matches("<string>127.0.0.1</string>").count(), 2);
        assert_eq!(plist.matches("<string>::1</string>").count(), 2);
        assert_eq!(plist.matches("<string>80</string>").count(), 2);
        assert_eq!(plist.matches("<string>443</string>").count(), 2);
        assert!(plist.contains("<key>HTTP</key>"));
        assert!(plist.contains("<key>HTTPS</key>"));
    }

    #[test]
    fn the_service_skips_ipv6_loopback_when_it_is_missing() {
        let plist = plist_contents(&ServiceSpec {
            ipv6_loopback: false,
            ..spec()
        });

        assert_eq!(plist.matches("<string>127.0.0.1</string>").count(), 2);
        assert!(!plist.contains("::1"));
    }

    #[test]
    fn the_lan_opt_in_binds_every_interface() {
        let plist = plist_contents(&ServiceSpec {
            lan_access: true,
            ..spec()
        });

        assert!(!plist.contains("SockNodeName"));
        assert_eq!(plist.matches("<key>SockServiceName</key>").count(), 2);
    }

    #[test]
    fn the_plist_has_no_log_paths_for_launchd_to_open_as_root() {
        let plist = plist_contents(&spec());

        assert!(!plist.contains("StandardOutPath"));
        assert!(!plist.contains("StandardErrorPath"));
    }

    #[test]
    fn plist_values_are_escaped_for_xml() {
        let plist = plist_contents(&ServiceSpec {
            binary: "/Applications/Localify & Dev.app/localify".into(),
            ..spec()
        });

        assert!(plist.contains("Localify &amp; Dev.app"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn plutil_accepts_the_plist_in_both_network_modes() {
        let dir = tempfile::tempdir().unwrap();
        for (name, spec) in [
            ("loopback.plist", spec()),
            (
                "lan.plist",
                ServiceSpec {
                    lan_access: true,
                    ..spec()
                },
            ),
        ] {
            let path = dir.path().join(name);
            fs::write(&path, plist_contents(&spec)).unwrap();
            let output = Command::new("/usr/bin/plutil")
                .arg("-lint")
                .arg(&path)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stdout)
            );
        }
    }

    #[test]
    fn a_clean_machine_has_nothing_to_migrate() {
        let dir = tempfile::tempdir().unwrap();

        assert!(LegacyInstall::detect(&empty_roots(dir.path())).is_empty());
    }

    #[test]
    fn it_finds_an_old_helper_daemon_and_copy() {
        let dir = tempfile::tempdir().unwrap();
        let roots = empty_roots(dir.path());
        let prefix = BRANDING.legacy_helper_prefix();
        fs::write(roots.launch_daemons.join(format!("{prefix}.501.plist")), "").unwrap();
        fs::write(roots.launch_daemons.join("com.other.plist"), "").unwrap();
        fs::write(roots.helper_tools.join(format!("{prefix}.501")), "").unwrap();
        fs::write(
            roots
                .var_run
                .join(format!("{}-helper-501.sock", BRANDING.slug())),
            "",
        )
        .unwrap();
        fs::write(roots.var_run.join(format!("{}.pid", BRANDING.slug())), "1").unwrap();

        let legacy = LegacyInstall::detect(&roots);

        assert_eq!(legacy.helper_labels, vec![format!("{prefix}.501")]);
        assert_eq!(legacy.helper_plists.len(), 1);
        assert_eq!(legacy.helper_copies.len(), 1);
        assert_eq!(legacy.run_files.len(), 2);
        assert!(!legacy.is_empty());
    }

    #[test]
    fn it_removes_the_apps_login_item() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!("{}.plist", BRANDING.app_name));
        fs::write(
            &path,
            "<string>localify-desktop</string><string>--autostart</string>",
        )
        .unwrap();

        let (_, outcome) = remove_login_item_from(dir.path()).unwrap();

        assert!(matches!(outcome, StepOutcome::Success(_)));
        assert!(!path.exists());
    }

    #[test]
    fn it_leaves_another_apps_login_item_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!("{}.plist", BRANDING.app_name));
        fs::write(
            &path,
            "<string>/Applications/Other.app/Contents/MacOS/other</string>",
        )
        .unwrap();

        assert!(remove_login_item_from(dir.path()).is_none());
        assert!(path.exists());
    }

    #[test]
    fn no_login_item_means_nothing_to_report() {
        let dir = tempfile::tempdir().unwrap();

        assert!(remove_login_item_from(dir.path()).is_none());
    }

    #[test]
    fn a_log_folder_alone_is_not_an_install() {
        let dir = tempfile::tempdir().unwrap();
        let roots = empty_roots(dir.path());
        fs::create_dir_all(roots.var_log.join(BRANDING.slug())).unwrap();

        let legacy = LegacyInstall::detect(&roots);

        assert!(legacy.log_dir.is_some());
        assert!(legacy.is_empty());
    }

    #[test]
    fn it_finds_a_config_and_logs_in_system_folders() {
        let dir = tempfile::tempdir().unwrap();
        let roots = empty_roots(dir.path());
        fs::create_dir_all(roots.etc_dir.join(BRANDING.slug())).unwrap();
        fs::create_dir_all(roots.var_log.join(BRANDING.slug())).unwrap();

        let legacy = LegacyInstall::detect(&roots);

        assert_eq!(legacy.config_dir, Some(roots.etc_dir.join(BRANDING.slug())));
        assert_eq!(legacy.log_dir, Some(roots.var_log.join(BRANDING.slug())));
    }

    fn all_changes() -> SystemChanges {
        SystemChanges {
            legacy: LegacyInstall {
                helper_labels: vec!["dev.localify.helper.501".into()],
                helper_plists: vec!["/Library/LaunchDaemons/dev.localify.helper.501.plist".into()],
                helper_copies: vec![
                    "/Library/PrivilegedHelperTools/dev.localify.helper.501".into(),
                ],
                run_files: vec!["/var/run/localify.pid".into()],
                config_dir: Some("/etc/localify".into()),
                log_dir: Some("/var/log/localify".into()),
                root_daemon: true,
            },
            stale_keychain_hashes: vec!["AABB".into()],
            resolver_content: Some(resolver_content(1053)),
            service_plist: Some(plist_contents(&spec())),
            service_ports: vec![80, 443],
            cli_link: Some(CliLink {
                link: "/usr/local/bin/localify".into(),
                target: "/Applications/Localify.app/Contents/MacOS/localify".into(),
            }),
        }
    }

    fn assert_valid_sh(script: &str) {
        let output = Command::new("/bin/sh")
            .args(["-n", "-c", script])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn nothing_to_do_means_no_prompt() {
        assert!(SystemChanges::default().is_empty());
    }

    #[test]
    fn the_install_script_is_valid_shell() {
        assert_valid_sh(&all_changes().script());
    }

    fn full_removal() -> SystemRemoval {
        SystemRemoval {
            legacy: all_changes().legacy,
            keychain_hashes: vec!["AABB".into()],
            cli_link: Some("/usr/local/bin/localify".into()),
            service: true,
        }
    }

    #[test]
    fn the_removal_script_is_valid_shell_and_takes_everything_out() {
        let script = full_removal().script();

        assert_valid_sh(&script);
        for expected in [
            format!("bootout '{}'", service_target()),
            format!("rm -f '{}'", service_plist_path().display()),
            format!("rm -f '{}'", resolver_file().display()),
            "rm -f '/usr/local/bin/localify'".to_string(),
            "delete-certificate -Z 'AABB'".to_string(),
            "rm -rf '/etc/localify'".to_string(),
        ] {
            assert!(script.contains(&expected), "missing: {expected}");
        }
    }

    #[test]
    fn a_failed_cleanup_step_does_not_fail_the_scripts() {
        let install = all_changes().script();
        let removal = full_removal().script();

        assert!(install.ends_with("exit 0"));
        assert!(removal.ends_with("exit 0"));
    }

    #[test]
    fn the_binary_never_runs_as_root() {
        let binary = "/Applications/Localify.app/Contents/MacOS/localify";
        let script = SystemChanges {
            service_plist: Some("PLIST".into()),
            ..all_changes()
        }
        .script();

        let mentions: Vec<&str> = script.lines().filter(|l| l.contains(binary)).collect();
        assert_eq!(mentions.len(), 1, "{mentions:?}");
        assert!(mentions[0].starts_with("/bin/ln -sfn"));
        assert!(!script.contains("--config"));
    }

    #[test]
    fn the_old_install_goes_before_the_service_needs_the_ports() {
        let script = all_changes().script();

        let helper = script
            .find("bootout 'system/dev.localify.helper.501'")
            .unwrap();
        let daemon = script.find("pkill -u 0 -f").unwrap();
        let bootstrap = script.find("launchctl bootstrap system").unwrap();

        assert!(helper < daemon && daemon < bootstrap);
    }

    #[test]
    fn the_old_files_stay_until_the_new_setup_is_in_place() {
        let script = all_changes().script();

        let bootstrap = script.find("launchctl bootstrap system").unwrap();
        let link = script.find("/bin/ln -sfn").unwrap();
        let trust = script.find("remove-trusted-cert").unwrap();
        let plist = script
            .find("/bin/rm -f '/Library/LaunchDaemons/dev.localify.helper.501.plist'")
            .unwrap();
        let folder = script.find("/bin/rm -rf '/etc/localify'").unwrap();
        let keychain = script.find("delete-certificate -Z 'AABB'").unwrap();

        for removal in [trust, plist, folder, keychain] {
            assert!(bootstrap < removal && link < removal);
        }
    }

    #[test]
    fn the_old_ca_loses_its_trust_before_its_folder_goes() {
        let script = all_changes().script();

        let trust = script
            .find("remove-trusted-cert -d '/etc/localify/ca.crt'")
            .unwrap();
        let folder = script.find("/bin/rm -rf '/etc/localify'").unwrap();
        let keychain = script.find("delete-certificate -Z 'AABB'").unwrap();

        assert!(trust < folder);
        assert!(keychain > folder);
    }

    #[test]
    fn a_daemon_started_with_sudo_goes_even_with_no_helper_to_remove() {
        let legacy = LegacyInstall {
            config_dir: Some("/etc/localify".into()),
            ..LegacyInstall::default()
        };

        let commands = legacy_commands(&legacy).join("\n");

        assert!(commands.contains("pkill -u 0 -f"));
        assert!(commands.contains("pkill -9 -u 0 -f"));
    }

    #[test]
    fn a_clean_machine_kills_nothing() {
        assert!(legacy_commands(&LegacyInstall::default()).is_empty());
    }

    #[test]
    fn a_port_launchd_could_not_open_restores_the_previous_service() {
        let script = SystemChanges {
            service_plist: Some("PLIST".into()),
            service_ports: vec![80, 443],
            ..SystemChanges::default()
        }
        .script();

        let bootstrap = script.find("launchctl bootstrap system").unwrap();
        let check = script.find("for port in 80 443; do").unwrap();
        let rollback = script.find(r#"[ -n "$taken" ]; then"#).unwrap();
        let unload = script[rollback..].find("launchctl bootout").unwrap();
        let restore = script[rollback..].find("/bin/mv ").unwrap();
        let error = script[rollback..]
            .find("Another program holds port")
            .unwrap();

        assert!(bootstrap < check && check < rollback);
        assert!(unload < restore && restore < error);

        let syntax = Command::new("/bin/sh")
            .args(["-n", "-c", &script])
            .output()
            .unwrap();
        assert!(syntax.status.success(), "{syntax:?}");
    }

    #[test]
    fn a_refused_port_leaves_the_resolver_and_the_link_alone() {
        let script = all_changes().script();

        let refused = script.find("Another program holds port").unwrap();
        let resolver = script.find("Could not write the resolver file").unwrap();
        let link = script.find("/bin/ln ").unwrap();

        assert!(refused < resolver && refused < link);
    }

    #[test]
    fn root_never_touches_ca_trust_for_the_current_ca() {
        let script = all_changes().script();

        assert!(!script.contains("add-trusted-cert"), "{script}");
    }

    #[test]
    fn the_dev_pattern_spares_an_installed_app() {
        let pattern = |text: &str, config: &str| {
            let output = Command::new("/usr/bin/grep")
                .args(["-E", "-c", text])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .and_then(|mut child| {
                    if let Some(stdin) = child.stdin.as_mut() {
                        stdin.write_all(config.as_bytes())?;
                    }
                    drop(child.stdin.take());
                    child.wait_with_output()
                })
                .unwrap();
            String::from_utf8_lossy(&output.stdout).trim() == "1"
        };
        let installed = "/Applications/Localify.app/Contents/MacOS/localify --config /etc/localify/config.toml start --foreground\n";
        let dev = "/x/target/debug/localify --config /Users/me/Library/Application Support/Localify Dev/config.toml start --foreground\n";

        assert!(pattern(ROOT_DAEMON_PATTERN, installed));
        assert!(pattern(ROOT_DAEMON_PATTERN, dev));
        assert!(!pattern(DEV_ROOT_DAEMON_PATTERN, installed));
        assert!(pattern(DEV_ROOT_DAEMON_PATTERN, dev));
    }

    #[test]
    fn the_script_cannot_match_its_own_root_daemon_pattern() {
        // pkill -f reads the full command line of the shell running the script.
        // Anchored at the end, the pattern only matches when the line ends there.
        let script = all_changes().script();

        assert!(ROOT_DAEMON_PATTERN.ends_with("$"));
        assert!(DEV_ROOT_DAEMON_PATTERN.ends_with("$"));
        assert!(!script.ends_with(" start --foreground"));
    }

    #[test]
    fn resolver_content_points_at_the_dns_port_on_loopback() {
        let content = resolver_content(1053);

        assert!(content.ends_with('\n'));
        assert!(content.contains(&format!("domain {}", BRANDING.domain_label())));
        assert!(content.contains("nameserver 127.0.0.1"));
        assert!(content.contains("nameserver ::1"));
        assert!(content.contains("port 1053"));
        assert!(content.contains("search_order 1"));
        assert!(content.contains("timeout 1"));
        assert!(content.contains("options attempts:1"));
    }

    #[test]
    fn parses_keychain_hashes() {
        let output = r#"
keychain: "/Library/Keychains/System.keychain"
SHA-1 hash: 11AA22BB33CC44DD55EE66FF77889900AABBCCDD
keychain: "/Library/Keychains/System.keychain"
SHA-1 hash: AA:BB:CC:DD:EE:FF:00:11:22:33:44:55:66:77:88:99:00:11:22:33
"#;

        assert_eq!(
            parse_keychain_hashes(output),
            vec![
                "11AA22BB33CC44DD55EE66FF77889900AABBCCDD".to_string(),
                "AABBCCDDEEFF0011223344556677889900112233".to_string(),
            ]
        );
    }

    #[test]
    fn parses_an_openssl_fingerprint() {
        let output =
            "sha1 Fingerprint=AA:BB:CC:DD:EE:FF:00:11:22:33:44:55:66:77:88:99:00:11:22:33\n";

        assert_eq!(
            parse_openssl_fingerprint(output).unwrap(),
            "AABBCCDDEEFF0011223344556677889900112233"
        );
    }

    #[test]
    fn migrating_keeps_settings_and_resets_root_paths() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("legacy.toml");
        fs::write(
            &legacy,
            "[daemon]\nhttp_port = 8081\n[paths]\npid_file = \"/old/root/localify.pid\"\n",
        )
        .unwrap();
        let target = dir.path().join("state").join("config.toml");

        assert!(copy_config_to_user_folder(&legacy, &target).unwrap());

        let config = ConfigStore::new(target).load().unwrap();
        assert_eq!(config.daemon.http_port, 8081);
        assert_eq!(config.paths.pid_file, LocalifyPaths::default().pid_file);
    }

    #[test]
    fn migrating_keeps_old_domains_when_a_newer_config_exists() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("legacy.toml");
        fs::write(
            &legacy,
            "[daemon]\nhttp_port = 8081\n[domains.\"old.localify\"]\npattern = \"old.localify\"\nhttps_enabled = true\n[[domains.\"old.localify\".routes]]\npath = \"/\"\ntarget = \"127.0.0.1:3000\"\n",
        )
        .unwrap();
        let target = dir.path().join("config.toml");
        fs::write(&target, "[daemon]\nhttp_port = 9000\n").unwrap();

        assert!(copy_config_to_user_folder(&legacy, &target).unwrap());

        let text = fs::read_to_string(&target).unwrap();
        assert!(text.contains("old.localify"), "{text}");
        let config = ConfigStore::new(target).load().unwrap();
        assert_eq!(config.daemon.http_port, 9000);
    }

    #[test]
    fn migrating_never_overwrites_a_config_in_the_users_folder() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("legacy.toml");
        fs::write(&legacy, "[daemon]\nhttp_port = 8081\n").unwrap();
        let target = dir.path().join("config.toml");
        fs::write(&target, "[daemon]\nhttp_port = 9000\n").unwrap();

        assert!(!copy_config_to_user_folder(&legacy, &target).unwrap());

        let config = ConfigStore::new(target).load().unwrap();
        assert_eq!(config.daemon.http_port, 9000);
    }
}
