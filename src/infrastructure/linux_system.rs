//! What Localify changes on a Linux machine with systemd, the counterpart of
//! `macos_system`. One `sudo` runs a shell script of system tools: the Localify binary
//! never runs as root. systemd opens the ports and hands them to a daemon running as
//! the user, so after the install nothing of Localify's runs as root.
// Both crates compile this module, and its text builders are tested on every platform.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Result, bail};

use crate::branding::BRANDING;
use crate::infrastructure::certs::ca::pem_is_limited_to_local_domains;
use crate::infrastructure::macos_system::{
    LegacyInstall, ServiceSpec, SystemRequest, legacy_commands, shell_quote,
};
use crate::infrastructure::service_sockets::{HTTP_SOCKETS, HTTPS_SOCKETS};

const UNIT_DIR: &str = "/etc/systemd/system";
const RESOLVED_DROP_IN_DIR: &str = "/etc/systemd/resolved.conf.d";
const SYSTEMCTL: &str = "/usr/bin/systemctl";
const UPDATE_CA: &str = "/usr/sbin/update-ca-certificates";
const RESOLVECTL: &str = "/usr/bin/resolvectl";

/// Where the system certificate store takes local CAs on Debian and Ubuntu.
pub const CA_STORE_FILE: &str = "/usr/local/share/ca-certificates/localify-ca.crt";

fn service_unit_name() -> String {
    format!("{}.service", BRANDING.slug())
}

fn http_socket_name() -> String {
    format!("{}-http.socket", BRANDING.slug())
}

fn https_socket_name() -> String {
    format!("{}-https.socket", BRANDING.slug())
}

/// Names of every unit, in the order systemd should start them.
fn unit_names() -> [String; 3] {
    [http_socket_name(), https_socket_name(), service_unit_name()]
}

fn unit_path(name: &str) -> PathBuf {
    Path::new(UNIT_DIR).join(name)
}

/// A value for a unit file, quoted for systemd. `%` starts a specifier there.
fn unit_quote(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('%', "%%");
    format!("\"{escaped}\"")
}

/// `ExecStart` also expands `$`.
fn exec_quote(value: &str) -> String {
    unit_quote(value).replace('$', "$$")
}

/// A line break in a path would start a new setting in the unit file.
fn reject_control_characters(spec: &ServiceSpec) -> Result<()> {
    let binary = spec.binary.to_string_lossy();
    let config = spec.config_path.to_string_lossy();
    let home = spec.home.to_string_lossy();
    for value in [spec.user.as_str(), &binary, &config, &home] {
        if value.chars().any(char::is_control) {
            bail!("Cannot write a systemd unit for a path with control characters: {value:?}");
        }
    }
    Ok(())
}

/// The daemon runs as the user, from the binary's normal path, so replacing the binary
/// needs no password. A zero exit means the user stopped it, and only a connection wakes
/// it then. `StartLimitIntervalSec=0` keeps systemd from failing the sockets when
/// connections wake a stopped daemon many times in a row.
pub fn service_unit(spec: &ServiceSpec) -> String {
    let http = http_socket_name();
    let https = https_socket_name();
    let exec = [
        spec.binary.to_string_lossy().as_ref(),
        "--config",
        spec.config_path.to_string_lossy().as_ref(),
        "start",
        "--foreground",
    ]
    .map(exec_quote)
    .join(" ");

    format!(
        "# Managed by {app}. Do not edit.\n\
         [Unit]\n\
         Description={app}\n\
         Requires={http} {https}\n\
         After={http} {https} network.target\n\
         StartLimitIntervalSec=0\n\
         \n\
         [Service]\n\
         User={user}\n\
         Environment={home}\n\
         ExecStart={exec}\n\
         Sockets={http} {https}\n\
         Restart=on-failure\n\
         RestartSec=1\n\
         NoNewPrivileges=yes\n\
         \n\
         [Install]\n\
         WantedBy=multi-user.target\n",
        app = BRANDING.app_name,
        user = spec.user,
        home = unit_quote(&format!("HOME={}", spec.home.display())),
    )
}

/// One unit per name, because `FileDescriptorName` applies to a whole socket unit.
pub fn socket_unit(fd_name: &str, port: u16, spec: &ServiceSpec) -> String {
    let (ipv4, ipv6) = if spec.lan_access {
        ("0.0.0.0", "[::]")
    } else {
        ("127.0.0.1", "[::1]")
    };
    let mut listen = format!("ListenStream={ipv4}:{port}\n");
    if spec.ipv6_loopback {
        listen.push_str(&format!("ListenStream={ipv6}:{port}\n"));
    }

    format!(
        "# Managed by {app}. Do not edit.\n\
         [Unit]\n\
         Description={app} {fd_name} ports\n\
         \n\
         [Socket]\n\
         {listen}\
         BindIPv6Only=ipv6-only\n\
         FileDescriptorName={fd_name}\n\
         Service={service}\n\
         \n\
         [Install]\n\
         WantedBy=sockets.target\n",
        app = BRANDING.app_name,
        service = service_unit_name(),
    )
}

fn unit_files(spec: &ServiceSpec) -> Vec<(PathBuf, String)> {
    vec![
        (
            unit_path(&http_socket_name()),
            socket_unit(HTTP_SOCKETS, spec.http_port, spec),
        ),
        (
            unit_path(&https_socket_name()),
            socket_unit(HTTPS_SOCKETS, spec.https_port, spec),
        ),
        (unit_path(&service_unit_name()), service_unit(spec)),
    ]
}

/// Sends the local suffix to the daemon's DNS server, and nothing else.
pub fn resolved_drop_in(port: u16) -> String {
    format!(
        "# Managed by {} — do not edit.\n\
         [Resolve]\n\
         DNS=127.0.0.1:{port}\n\
         Domains=~{}\n",
        BRANDING.app_name,
        BRANDING.domain_label()
    )
}

/// What root still has to change on this machine.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SystemChanges {
    pub legacy: LegacyInstall,
    pub units: Vec<(PathBuf, String)>,
    pub resolver_content: Option<String>,
    pub ca_pem: Option<String>,
}

impl SystemChanges {
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    /// The script root runs. It names only system tools by absolute path. The service
    /// goes first: if it fails, the script stops with the old resolver and CA in place.
    pub fn script(&self) -> String {
        let mut script = vec![
            "umask 022".to_string(),
            r#"fail() { echo "$1" >&2; exit 1; }"#.to_string(),
        ];
        script.extend(legacy_commands(&self.legacy));
        script.extend(self.service_commands());
        script.extend(self.resolver_commands());
        script.extend(self.ca_commands());
        script.push("exit 0".to_string());
        script.join("\n")
    }

    /// Writes the units and starts them. systemd refuses a socket whose port is taken,
    /// but `enable --now` still exits 0, so the script asks about each socket. If one
    /// failed, the previous units go back and the error says how to check.
    fn service_commands(&self) -> Vec<String> {
        if self.units.is_empty() {
            return Vec::new();
        }
        let names = unit_names().map(|name| shell_quote(&name)).join(" ");
        let previous = |path: &Path| shell_quote(&format!("{}.previous", path.display()));
        let current = |path: &Path| shell_quote(&path.to_string_lossy());

        let mut commands = vec![format!("{SYSTEMCTL} stop {names} 2>/dev/null")];
        for (path, content) in &self.units {
            commands.push(format!(
                "[ -f {0} ] && /bin/mv {0} {1}",
                current(path),
                previous(path)
            ));
            commands.push(format!(
                "/usr/bin/printf '%s' {} > {} || fail 'Could not write {}.'",
                shell_quote(content),
                current(path),
                path.display()
            ));
            commands.push(format!("/bin/chmod 644 {}", current(path)));
        }
        commands.push(format!("{SYSTEMCTL} daemon-reload"));

        let restore: Vec<String> = self
            .units
            .iter()
            .map(|(path, _)| {
                format!(
                    "  /bin/rm -f {0}\n  if [ -f {1} ]; then /bin/mv {1} {0}; had_previous=1; fi",
                    current(path),
                    previous(path)
                )
            })
            .collect();
        let sockets = [http_socket_name(), https_socket_name()]
            .map(|name| shell_quote(&name))
            .join(" ");
        commands.push(format!(
            r#"{SYSTEMCTL} enable --now {names} >/dev/null 2>&1
failed=""
for unit in {sockets}; do {SYSTEMCTL} is-active --quiet "$unit" || failed="$failed $unit"; done
if [ -n "$failed" ]; then
  {SYSTEMCTL} disable --now {names} 2>/dev/null
  {SYSTEMCTL} reset-failed {names} 2>/dev/null
  had_previous=""
{restore}
  {SYSTEMCTL} daemon-reload
  [ -z "$had_previous" ] || {SYSTEMCTL} enable --now {names} 2>/dev/null
  fail "systemd could not open$failed, usually because another program holds its port, so the setup was not changed. Check with: sudo ss -ltnp"
fi"#,
            restore = restore.join("\n"),
        ));
        for (path, _) in &self.units {
            commands.push(format!("/bin/rm -f {}", previous(path)));
        }
        commands
    }

    fn resolver_commands(&self) -> Vec<String> {
        let Some(content) = &self.resolver_content else {
            return Vec::new();
        };
        let file = shell_quote(&BRANDING.resolved_drop_in_file().to_string_lossy());
        vec![
            format!(
                "/bin/mkdir -p {RESOLVED_DROP_IN_DIR} || fail 'Could not create {RESOLVED_DROP_IN_DIR}.'"
            ),
            format!(
                "/usr/bin/printf '%s' {} > {file} || fail 'Could not write the DNS settings.'",
                shell_quote(content)
            ),
            format!("/bin/chmod 644 {file}"),
            format!(
                "{SYSTEMCTL} restart systemd-resolved || fail 'Could not restart systemd-resolved.'"
            ),
        ]
    }

    fn ca_commands(&self) -> Vec<String> {
        let Some(pem) = &self.ca_pem else {
            return Vec::new();
        };
        let file = shell_quote(CA_STORE_FILE);
        vec![
            format!(
                "/usr/bin/printf '%s' {} > {file} || fail 'Could not write the CA to the system store.'",
                shell_quote(pem)
            ),
            format!("/bin/chmod 644 {file}"),
            format!("{UPDATE_CA} >/dev/null || fail 'update-ca-certificates failed.'"),
        ]
    }
}

/// Everything `SystemChanges` can add, for uninstall.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SystemRemoval {
    pub legacy: LegacyInstall,
}

impl SystemRemoval {
    pub fn script(&self) -> String {
        let names = unit_names().map(|name| shell_quote(&name)).join(" ");
        let mut script = vec!["umask 022".to_string()];
        script.extend(legacy_commands(&self.legacy));
        script.push(format!("{SYSTEMCTL} disable --now {names} 2>/dev/null"));
        for name in unit_names() {
            script.push(format!(
                "/bin/rm -f {}",
                shell_quote(&unit_path(&name).to_string_lossy())
            ));
        }
        script.push(format!("{SYSTEMCTL} daemon-reload"));
        script.push(format!(
            "/bin/rm -f {}",
            shell_quote(&BRANDING.resolved_drop_in_file().to_string_lossy())
        ));
        script.push(format!("{SYSTEMCTL} restart systemd-resolved 2>/dev/null"));
        script.push(format!("/bin/rm -f {}", shell_quote(CA_STORE_FILE)));
        script.push(format!("{UPDATE_CA} --fresh >/dev/null 2>&1"));
        script.push("exit 0".to_string());
        script.join("\n")
    }
}

fn file_holds(path: &Path, content: &str) -> bool {
    fs::read_to_string(path).is_ok_and(|installed| installed == content)
}

fn service_enabled() -> bool {
    Command::new(SYSTEMCTL)
        .args(["is-enabled", "--quiet", &service_unit_name()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Compares the machine with this request. Fails before any password when the machine
/// lacks what the setup relies on.
pub fn changes(request: &SystemRequest) -> Result<SystemChanges> {
    if !Path::new(RESOLVECTL).exists() {
        bail!(
            "{} needs systemd-resolved to answer for its domains. On Debian and Ubuntu: sudo apt install systemd-resolved",
            BRANDING.app_name
        );
    }
    if !Path::new(UPDATE_CA).exists() {
        bail!(
            "{} needs update-ca-certificates for its certificate. On Debian and Ubuntu: sudo apt install ca-certificates",
            BRANDING.app_name
        );
    }

    let spec = request.service_spec()?;
    reject_control_characters(&spec)?;
    let units = unit_files(&spec);
    let units_current = units
        .iter()
        .all(|(path, content)| file_holds(path, content));

    let drop_in = resolved_drop_in(request.daemon.dns_port);
    let resolver_current = file_holds(&BRANDING.resolved_drop_in_file(), &drop_in);

    let ca_cert = request.paths.data_dir.join("ca.crt");
    let ca_pem = match fs::read_to_string(&ca_cert) {
        Ok(pem) if !file_holds(Path::new(CA_STORE_FILE), &pem) => {
            if !pem_is_limited_to_local_domains(&pem).map_err(anyhow::Error::from)? {
                bail!(
                    "The CA at {} can sign for any site, so {} will not trust it.",
                    ca_cert.display(),
                    BRANDING.app_name
                );
            }
            Some(pem)
        }
        _ => None,
    };

    Ok(SystemChanges {
        legacy: LegacyInstall::on_this_machine(),
        units: if units_current && service_enabled() {
            Vec::new()
        } else {
            units
        },
        resolver_content: (!resolver_current).then_some(drop_in),
        ca_pem,
    })
}

pub fn removal_for_this_machine() -> SystemRemoval {
    SystemRemoval {
        legacy: LegacyInstall::on_this_machine(),
    }
}

/// Whether the installed service runs this config, as this user.
pub fn service_runs_config(config_path: &Path) -> bool {
    if !BRANDING.uses_systemd_service() {
        return false;
    }
    let Ok(unit) = fs::read_to_string(unit_path(&service_unit_name())) else {
        return false;
    };
    let Ok(user) = crate::infrastructure::macos_system::current_user() else {
        return false;
    };
    unit_runs(&unit, &user, config_path)
}

fn unit_runs(unit: &str, user: &str, config_path: &Path) -> bool {
    unit.lines().any(|line| line == format!("User={user}"))
        && unit.contains(&exec_quote(&config_path.to_string_lossy()))
}

pub fn system_setup_done() -> bool {
    BRANDING.resolved_drop_in_file().exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> ServiceSpec {
        ServiceSpec {
            user: "dev".into(),
            home: "/home/dev".into(),
            binary: "/home/dev/.local/bin/localify".into(),
            config_path: "/home/dev/.config/localify/config.toml".into(),
            http_port: 80,
            https_port: 443,
            lan_access: false,
            ipv6_loopback: true,
        }
    }

    fn all_changes() -> SystemChanges {
        SystemChanges {
            legacy: LegacyInstall::default(),
            units: unit_files(&spec()),
            resolver_content: Some(resolved_drop_in(1053)),
            ca_pem: Some("PEM".into()),
        }
    }

    #[test]
    fn the_daemon_runs_as_the_user_and_never_as_root() {
        let unit = service_unit(&spec());

        assert!(unit.lines().any(|line| line == "User=dev"), "{unit}");
        assert!(unit.contains("NoNewPrivileges=yes"));
        assert!(unit.contains(
            r#"ExecStart="/home/dev/.local/bin/localify" "--config" "/home/dev/.config/localify/config.toml" "start" "--foreground""#
        ));
    }

    #[test]
    fn a_stop_stays_stopped_and_a_failure_restarts() {
        let unit = service_unit(&spec());

        assert!(unit.contains("Restart=on-failure"));
        assert!(unit.contains("StartLimitIntervalSec=0"));
        assert!(unit.contains("WantedBy=multi-user.target"));
    }

    #[test]
    fn the_default_stays_on_this_machine() {
        let unit = socket_unit(HTTP_SOCKETS, 80, &spec());

        assert!(unit.contains("ListenStream=127.0.0.1:80\n"), "{unit}");
        assert!(unit.contains("ListenStream=[::1]:80\n"));
        assert!(!unit.contains("0.0.0.0"));
        assert!(unit.contains("FileDescriptorName=HTTP\n"));
    }

    #[test]
    fn opting_in_opens_every_interface_on_both_families() {
        let spec = ServiceSpec {
            lan_access: true,
            ..spec()
        };
        let unit = socket_unit(HTTPS_SOCKETS, 443, &spec);

        assert!(unit.contains("ListenStream=0.0.0.0:443\n"), "{unit}");
        assert!(unit.contains("ListenStream=[::]:443\n"));
        assert!(unit.contains("BindIPv6Only=ipv6-only\n"));
    }

    #[test]
    fn no_ipv6_loopback_means_no_ipv6_socket() {
        let spec = ServiceSpec {
            ipv6_loopback: false,
            ..spec()
        };

        assert!(!socket_unit(HTTP_SOCKETS, 80, &spec).contains("[::1]"));
    }

    #[test]
    fn unit_values_cannot_expand_or_break_out() {
        assert_eq!(unit_quote(r#"a"b%c\d"#), r#""a\"b%%c\\d""#);
        assert_eq!(exec_quote("/a$b c"), r#""/a$$b c""#);

        let bad = ServiceSpec {
            config_path: "/home/dev/x\nUser=root".into(),
            ..spec()
        };
        assert!(reject_control_characters(&bad).is_err());
        assert!(reject_control_characters(&spec()).is_ok());
    }

    #[test]
    fn the_unit_names_the_user_and_config_it_serves() {
        let unit = service_unit(&spec());
        let config = spec().config_path;

        assert!(unit_runs(&unit, "dev", &config));
        assert!(!unit_runs(&unit, "other", &config));
        assert!(!unit_runs(&unit, "dev", Path::new("/other/config.toml")));
    }

    #[test]
    fn a_taken_port_changes_nothing_else() {
        let script = all_changes().script();

        let start = script.find("enable --now").unwrap();
        let rollback = script.find("systemd could not open").unwrap();
        let resolver = script.find("Could not write the DNS settings").unwrap();
        let ca = script.find("Could not write the CA").unwrap();

        assert!(start < rollback && rollback < resolver && resolver < ca);
    }

    #[test]
    fn each_socket_is_checked_because_enable_now_exits_zero_on_failure() {
        let script = all_changes().script();

        let start = script.find("enable --now").unwrap();
        let check = script.find("is-active --quiet \"$unit\"").unwrap();
        assert!(start < check);
        assert!(script.contains(&http_socket_name()));
        assert!(script.contains(&https_socket_name()));
    }

    #[test]
    fn a_failed_start_puts_the_previous_units_back() {
        let script = all_changes().script();

        let failed = script.find(r#"if [ -n "$failed" ]; then"#).unwrap();
        let tail = &script[failed..];
        let disable = tail.find("disable --now").unwrap();
        let restore = tail.find("/bin/mv").unwrap();
        let reload = tail.find("daemon-reload").unwrap();
        let again = tail.find(r#"[ -z "$had_previous" ]"#).unwrap();

        assert!(disable < restore && restore < reload && reload < again);
    }

    #[test]
    fn root_runs_only_system_tools() {
        let script = all_changes().script();

        assert!(
            !script.contains("/home/dev/.local/bin/localify --"),
            "{script}"
        );
        for line in script.lines().filter(|line| line.contains("--config")) {
            assert!(line.starts_with("ExecStart="), "{line}");
        }
    }

    #[test]
    fn the_scripts_are_valid_shell() {
        for script in [all_changes().script(), SystemRemoval::default().script()] {
            let syntax = Command::new("/bin/sh")
                .args(["-n", "-c", &script])
                .output()
                .unwrap();
            assert!(syntax.status.success(), "{syntax:?}\n{script}");
        }
    }

    #[test]
    fn uninstall_takes_back_everything_the_install_adds() {
        let script = SystemRemoval::default().script();

        for name in unit_names() {
            assert!(script.contains(&name), "{name}");
        }
        assert!(script.contains("disable --now"));
        assert!(script.contains(&BRANDING.resolved_drop_in_file().display().to_string()));
        assert!(script.contains(CA_STORE_FILE));
        assert!(script.contains("--fresh"));
    }

    #[test]
    fn nothing_to_change_is_an_empty_set_of_changes() {
        assert!(SystemChanges::default().is_empty());
        assert!(!all_changes().is_empty());
    }
}
