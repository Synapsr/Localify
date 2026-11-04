use super::{DnsError, DnsService, map_dns_error};
use crate::application::ports::{DnsConfigError, DnsManager};
use crate::branding::BRANDING;
use std::fs;
use std::process::Command;
use std::thread;
use std::time::Duration;

pub struct MacOsDnsService;

impl MacOsDnsService {
    pub fn new() -> Self {
        Self
    }
}

impl Default for MacOsDnsService {
    fn default() -> Self {
        Self::new()
    }
}

impl DnsService for MacOsDnsService {
    /// Writing the resolver file takes root, so it happens in the install's one
    /// approval. See `macos_system`. This process never has the rights.
    fn setup(&self, _port: u16) -> Result<(), DnsError> {
        Err(DnsError::PermissionDenied)
    }

    fn cleanup(&self) -> Result<(), DnsError> {
        if BRANDING.resolver_file().exists() {
            return Err(DnsError::PermissionDenied);
        }
        Ok(())
    }

    fn validate(&self) -> Result<(), DnsError> {
        // First, verify the resolver file exists and has correct content
        let resolver_file = BRANDING.resolver_file();
        let path = resolver_file.as_path();
        if !path.exists() {
            return Err(DnsError::ValidationFailed(format!(
                "Resolver file does not exist at {}",
                resolver_file.display()
            )));
        }

        let content = fs::read_to_string(path).map_err(|e| {
            DnsError::ValidationFailed(format!("Failed to read resolver file: {}", e))
        })?;

        if !content.contains("nameserver") || !content.contains("127.0.0.1") {
            return Err(DnsError::ValidationFailed(
                "Resolver file has incorrect content".into(),
            ));
        }

        // Use scutil --dns to verify macOS has registered the resolver
        // Retry a few times as macOS may take a moment to pick up the change
        const MAX_RETRIES: u32 = 5;
        const RETRY_DELAY_MS: u64 = 500;

        for attempt in 1..=MAX_RETRIES {
            let output = Command::new("scutil")
                .args(["--dns"])
                .output()
                .map_err(|e| DnsError::ValidationFailed(format!("Failed to run scutil: {}", e)))?;

            let stdout = String::from_utf8_lossy(&output.stdout);

            // Check if our resolver is listed (flexible spacing check)
            let has_resolver = stdout.lines().any(|line| {
                let trimmed = line.trim();
                trimmed.starts_with("domain") && trimmed.contains(BRANDING.domain_label())
            });

            if has_resolver {
                return Ok(());
            }

            if attempt < MAX_RETRIES {
                thread::sleep(Duration::from_millis(RETRY_DELAY_MS));
            }
        }

        Err(DnsError::ValidationFailed(format!(
            "DNS resolver for {} not found in scutil output after multiple attempts. \
                 Try running 'sudo killall -HUP mDNSResponder' to refresh DNS.",
            BRANDING.domain_suffix()
        )))
    }

    fn is_configured(&self) -> bool {
        BRANDING.resolver_file().exists()
    }
}

impl DnsManager for MacOsDnsService {
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
