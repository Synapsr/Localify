use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::infrastructure::privilege::run_as_acting_user;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortConflictKind {
    Http,
    Https,
    Dns,
}

impl PortConflictKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
            Self::Dns => "dns",
        }
    }

    fn display_name(self) -> &'static str {
        match self {
            Self::Http => "HTTP",
            Self::Https => "HTTPS",
            Self::Dns => "DNS",
        }
    }

    fn transport(self) -> PortTransport {
        match self {
            Self::Http | Self::Https => PortTransport::Tcp,
            Self::Dns => PortTransport::Udp,
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "http" => Ok(Self::Http),
            "https" => Ok(Self::Https),
            "dns" => Ok(Self::Dns),
            _ => bail!("Unsupported daemon port kind '{value}'."),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortConflictProcess {
    pub kind: PortConflictKind,
    pub port: u16,
    pub pid: u32,
    pub process_name: String,
    pub user: String,
    pub command: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PortTransport {
    Tcp,
    Udp,
}

impl PortTransport {
    fn lsof_selector(self, port: u16) -> String {
        match self {
            Self::Tcp => format!("-iTCP:{port}"),
            Self::Udp => format!("-iUDP:{port}"),
        }
    }
}

pub fn list_daemon_port_conflicts(
    http_port: u16,
    https_port: u16,
    dns_port: u16,
) -> Result<Vec<PortConflictProcess>> {
    let mut conflicts = Vec::new();

    conflicts.extend(inspect_port_conflicts(PortConflictKind::Http, http_port)?);
    conflicts.extend(inspect_port_conflicts(PortConflictKind::Https, https_port)?);
    conflicts.extend(inspect_port_conflicts(PortConflictKind::Dns, dns_port)?);

    Ok(conflicts)
}

pub fn kill_daemon_port_conflict(
    http_port: u16,
    https_port: u16,
    dns_port: u16,
    kind: &str,
    port: u16,
    pid: u32,
) -> Result<PortConflictProcess> {
    let expected_kind = PortConflictKind::parse(kind)?;
    let expected_port = match expected_kind {
        PortConflictKind::Http => http_port,
        PortConflictKind::Https => https_port,
        PortConflictKind::Dns => dns_port,
    };

    if port != expected_port {
        bail!(
            "{} port mismatch: expected {}, got {}.",
            expected_kind.display_name(),
            expected_port,
            port
        );
    }

    let conflict = list_daemon_port_conflicts(http_port, https_port, dns_port)?
        .into_iter()
        .find(|conflict| {
            conflict.kind == expected_kind && conflict.port == port && conflict.pid == pid
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "{} port {} is no longer blocked by PID {}.",
                expected_kind.display_name(),
                port,
                pid
            )
        })?;

    // Signals as the user, even from the root helper: the caller can name any
    // process that listens on a daemon port, and this must not stop one that
    // belongs to someone else.
    let mut kill = Command::new("kill");
    kill.args(["-TERM", &pid.to_string()]);
    run_as_acting_user(&mut kill);
    let output = kill.output().with_context(|| {
        format!(
            "Failed to signal PID {} on {} port {}.",
            pid,
            expected_kind.display_name(),
            port
        )
    })?;

    if !output.status.success() {
        bail!(
            "{}",
            command_error(&output).unwrap_or_else(|| {
                format!(
                    "Failed to stop PID {} on {} port {}.",
                    pid,
                    expected_kind.display_name(),
                    port
                )
            })
        );
    }

    Ok(conflict)
}

fn inspect_port_conflicts(kind: PortConflictKind, port: u16) -> Result<Vec<PortConflictProcess>> {
    let mut args = vec![
        "-nP".to_string(),
        kind.transport().lsof_selector(port),
        "-Fpc".to_string(),
    ];

    if kind.transport() == PortTransport::Tcp {
        args.push("-sTCP:LISTEN".to_string());
    }

    let output = Command::new("lsof")
        .args(args.iter().map(String::as_str))
        .output()
        .with_context(|| format!("Failed to inspect {} port {}.", kind.display_name(), port))?;

    if !output.status.success() && output.stdout.is_empty() {
        return Ok(Vec::new());
    }

    if !output.status.success() {
        bail!(
            "{}",
            command_error(&output).unwrap_or_else(|| {
                format!("Failed to inspect {} port {}.", kind.display_name(), port)
            })
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let summaries = parse_lsof_machine_output(&stdout);
    let conflicts = summaries
        .into_iter()
        .map(|summary| {
            let (user, command) = describe_process(summary.pid, &summary.process_name);
            PortConflictProcess {
                kind,
                port,
                pid: summary.pid,
                process_name: summary.process_name,
                user,
                command,
            }
        })
        .collect();

    Ok(conflicts)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PortConflictSummary {
    pid: u32,
    process_name: String,
}

fn parse_lsof_machine_output(output: &str) -> Vec<PortConflictSummary> {
    let mut conflicts = Vec::new();
    let mut current_pid = None;
    let mut current_process_name = None;

    for line in output.lines() {
        if let Some(pid) = line.strip_prefix('p') {
            if let (Some(current_pid), Some(process_name)) =
                (current_pid.take(), current_process_name.take())
            {
                conflicts.push(PortConflictSummary {
                    pid: current_pid,
                    process_name,
                });
            }

            current_pid = pid.parse::<u32>().ok();
            current_process_name = None;
            continue;
        }

        if let Some(process_name) = line.strip_prefix('c') {
            current_process_name = Some(process_name.to_string());
        }
    }

    if let (Some(current_pid), Some(process_name)) = (current_pid, current_process_name) {
        conflicts.push(PortConflictSummary {
            pid: current_pid,
            process_name,
        });
    }

    conflicts
}

fn describe_process(pid: u32, fallback_process_name: &str) -> (String, String) {
    let output = match Command::new("ps")
        .args(["-o", "user=", "-o", "command=", "-p", &pid.to_string()])
        .output()
    {
        Ok(output) if output.status.success() => output,
        _ => {
            return (String::new(), fallback_process_name.to_string());
        }
    };

    let line = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if line.is_empty() {
        return (String::new(), fallback_process_name.to_string());
    }

    let mut parts = line.splitn(2, char::is_whitespace);
    let user = parts.next().unwrap_or_default().trim().to_string();
    let command = parts
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback_process_name)
        .to_string();

    (user, command)
}

fn command_error(output: &std::process::Output) -> Option<String> {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();

    if !stderr.is_empty() {
        Some(stderr)
    } else if !stdout.is_empty() {
        Some(stdout)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::parse_lsof_machine_output;

    #[test]
    fn parse_lsof_machine_output_collects_pid_and_process_name() {
        let parsed = parse_lsof_machine_output("p123\ncpython\np456\ncnginx\n");

        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].pid, 123);
        assert_eq!(parsed[0].process_name, "python");
        assert_eq!(parsed[1].pid, 456);
        assert_eq!(parsed[1].process_name, "nginx");
    }
}
