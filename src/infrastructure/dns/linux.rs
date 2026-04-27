use super::{DnsError, DnsService, map_dns_error};
use crate::application::ports::{DnsConfigError, DnsManager};
use crate::branding::BRANDING;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::Duration;

const RESOLVED_DROP_IN_DIR: &str = "/etc/systemd/resolved.conf.d";

/// Linux DNS service using a systemd-resolved drop-in configuration.
///
/// Configures systemd-resolved to route the configured local suffix to the DNS
/// server — the Linux equivalent of macOS's `/etc/resolver/<suffix>`.
///
/// The `~<suffix>` routing domain ensures only matching lookups use this DNS
/// server; all other queries continue to use the normal per-link DNS servers.
pub struct LinuxDnsService;

impl LinuxDnsService {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LinuxDnsService {
    fn default() -> Self {
        Self::new()
    }
}

impl DnsService for LinuxDnsService {
    fn setup(&self, port: u16) -> Result<(), DnsError> {
        let drop_in_file = BRANDING.resolved_drop_in_file();
        let routing_domain = BRANDING.domain_label();

        // Ensure the drop-in directory exists
        let dir = Path::new(RESOLVED_DROP_IN_DIR);
        if !dir.exists() {
            fs::create_dir_all(dir).map_err(|e| {
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    DnsError::PermissionDenied
                } else {
                    DnsError::WriteError {
                        path: dir.to_path_buf(),
                        source: e,
                    }
                }
            })?;
        }

        // Write the drop-in config. The routing domain tells systemd-resolved
        // to send only matching suffix queries to this DNS server.
        let content = format!(
            "# Managed by {} — do not edit.\n\
             [Resolve]\n\
             DNS=127.0.0.1:{port}\n\
             Domains=~{routing_domain}\n",
            BRANDING.app_name
        );

        fs::write(&drop_in_file, content).map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                DnsError::PermissionDenied
            } else {
                DnsError::WriteError {
                    path: drop_in_file.clone(),
                    source: e,
                }
            }
        })?;

        // Restart systemd-resolved to pick up the new config
        restart_resolved()?;

        Ok(())
    }

    fn cleanup(&self) -> Result<(), DnsError> {
        let drop_in_file = BRANDING.resolved_drop_in_file();
        let path = drop_in_file.as_path();
        if path.exists() {
            fs::remove_file(path).map_err(|e| {
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    DnsError::PermissionDenied
                } else {
                    DnsError::RemoveError {
                        path: path.to_path_buf(),
                        source: e,
                    }
                }
            })?;

            restart_resolved()?;
        }
        Ok(())
    }

    fn validate(&self) -> Result<(), DnsError> {
        // Verify the drop-in file exists
        if !BRANDING.resolved_drop_in_file().exists() {
            return Err(DnsError::ValidationFailed(format!(
                "DNS drop-in file does not exist. Run 'sudo {} install' to set up DNS.",
                BRANDING.cli_command()
            )));
        }

        // Check that systemd-resolved has picked up the config.
        // Retry a few times as the service may need a moment after restart.
        const MAX_RETRIES: u32 = 5;
        const RETRY_DELAY_MS: u64 = 500;

        for attempt in 1..=MAX_RETRIES {
            let output = Command::new("resolvectl")
                .args(["status"])
                .output()
                .map_err(|e| {
                    DnsError::ValidationFailed(format!("Failed to run resolvectl: {}", e))
                })?;

            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);

                // Look for our DNS server and routing domain in the global section
                let has_dns = stdout.contains("127.0.0.1");
                let has_domain = stdout.contains(&format!("~{}", BRANDING.domain_label()));

                if has_dns && has_domain {
                    return Ok(());
                }
            }

            if attempt < MAX_RETRIES {
                thread::sleep(Duration::from_millis(RETRY_DELAY_MS));
            }
        }

        Err(DnsError::ValidationFailed(format!(
            "DNS routing for {} not found in resolvectl output after multiple attempts.\n\
                 Try running 'sudo {} install' again.",
            BRANDING.domain_suffix(),
            BRANDING.cli_command()
        )))
    }

    fn is_configured(&self) -> bool {
        BRANDING.resolved_drop_in_file().exists()
    }
}

impl DnsManager for LinuxDnsService {
    fn setup(&self, port: u16) -> Result<(), DnsConfigError> {
        DnsService::setup(self, port).map_err(map_dns_error)
    }

    fn cleanup(&self) -> Result<(), DnsConfigError> {
        DnsService::cleanup(self).map_err(map_dns_error)
    }

    fn validate(&self) -> Result<(), DnsConfigError> {
        DnsService::validate(self).map_err(map_dns_error)
    }

    fn is_configured(&self) -> bool {
        DnsService::is_configured(self)
    }
}

/// Restart systemd-resolved to apply configuration changes.
fn restart_resolved() -> Result<(), DnsError> {
    let output = Command::new("systemctl")
        .args(["restart", "systemd-resolved"])
        .output()
        .map_err(|e| {
            DnsError::ValidationFailed(format!("Failed to restart systemd-resolved: {}", e))
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("Permission denied") || stderr.contains("Access denied") {
            return Err(DnsError::PermissionDenied);
        }
        return Err(DnsError::ValidationFailed(format!(
            "Failed to restart systemd-resolved: {}",
            stderr.trim()
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_dropin_content_format() {
        let port = 1053;
        let routing_domain = BRANDING.domain_label();
        let content = format!(
            "# Managed by {} — do not edit.\n\
             [Resolve]\n\
             DNS=127.0.0.1:{port}\n\
             Domains=~{routing_domain}\n",
            BRANDING.app_name
        );
        assert!(content.contains("[Resolve]"));
        assert!(content.contains("DNS=127.0.0.1:1053"));
        assert!(content.contains(&format!("Domains=~{routing_domain}")));
        assert!(content.starts_with('#'));
    }
}
