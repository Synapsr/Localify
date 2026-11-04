use std::path::Path;
use std::process::Command;

use super::super::CertError;
use super::TrustStore;
use crate::branding::BRANDING;
use crate::infrastructure::macos_system;

const SYSTEM_KEYCHAIN: &str = "/Library/Keychains/System.keychain";

/// macOS Keychain trust store implementation
pub struct MacOsTrustStore;

impl MacOsTrustStore {
    pub fn new() -> Self {
        Self
    }
}

impl TrustStore for MacOsTrustStore {
    /// Trusting a CA takes an administrator, so it happens in the install's one
    /// approval. See `macos_system`. This process never has the rights.
    fn add_ca(&self, _cert_path: &Path) -> Result<(), CertError> {
        Err(CertError::PermissionDenied)
    }

    fn remove_ca(&self, _cert_path: &Path) -> Result<(), CertError> {
        for hash in keychain_hashes()? {
            let delete_output = Command::new("security")
                .args(["delete-certificate", "-Z", &hash, SYSTEM_KEYCHAIN])
                .output()
                .map_err(|e| {
                    CertError::TrustStoreError(format!("Failed to delete CA certificate: {}", e))
                })?;

            if !delete_output.status.success() {
                let stderr = String::from_utf8_lossy(&delete_output.stderr);

                if stderr.contains("authorization") || stderr.contains("Permission denied") {
                    return Err(CertError::PermissionDenied);
                }

                if !is_missing_macos_certificate_error(&stderr) {
                    return Err(CertError::TrustStoreError(format!(
                        "Failed to delete CA certificate: {}",
                        stderr
                    )));
                }
            }
        }

        Ok(())
    }

    fn is_ca_trusted(&self, cert_path: &Path) -> Result<bool, CertError> {
        // The dev profile trusts its CA in the user's own keychain instead.
        if BRANDING.is_dev_profile() {
            let keychain = macos_system::login_keychain_path()
                .map_err(|e| CertError::TrustStoreError(e.to_string()))?;
            return verify_certificate_trust(cert_path, &keychain.to_string_lossy());
        }

        let local_hash = macos_system::certificate_sha1(cert_path)
            .map_err(|e| CertError::TrustStoreError(e.to_string()))?;
        if !keychain_hashes()?.iter().any(|hash| hash == &local_hash) {
            return Ok(false);
        }

        verify_certificate_trust(cert_path, SYSTEM_KEYCHAIN)
    }
}

impl Default for MacOsTrustStore {
    fn default() -> Self {
        Self::new()
    }
}

fn is_missing_macos_certificate_error(message: &str) -> bool {
    message.contains("could not be found")
        || message.contains("The specified item could not be found")
        || message.contains("Unable to delete certificate matching")
}

fn keychain_hashes() -> Result<Vec<String>, CertError> {
    macos_system::system_keychain_ca_hashes().map_err(|e| CertError::TrustStoreError(e.to_string()))
}

fn verify_certificate_trust(cert_path: &Path, keychain: &str) -> Result<bool, CertError> {
    let cert_path_str = cert_path.to_str().ok_or_else(|| {
        CertError::TrustStoreError(format!(
            "Certificate path contains invalid UTF-8: {}",
            cert_path.display()
        ))
    })?;

    let output = Command::new("security")
        .args([
            "verify-cert",
            "-c",
            cert_path_str,
            "-p",
            "basic",
            "-L",
            "-k",
            keychain,
        ])
        .output()
        .map_err(|e| {
            CertError::TrustStoreError(format!(
                "Failed to verify macOS trust for the CA certificate: {}",
                e
            ))
        })?;

    if output.status.success() {
        return Ok(true);
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);

    if stderr.contains("authorization") || stderr.contains("Permission denied") {
        return Err(CertError::PermissionDenied);
    }

    if stdout.contains("CSSMERR_TP_NOT_TRUSTED")
        || stderr.contains("CSSMERR_TP_NOT_TRUSTED")
        || stdout.contains("certificate verification failed")
        || stderr.contains("certificate verification failed")
    {
        return Ok(false);
    }

    let mut message = stdout.trim().to_string();
    if !stderr.trim().is_empty() {
        if !message.is_empty() {
            message.push(' ');
        }
        message.push_str(stderr.trim());
    }

    Err(CertError::TrustStoreError(format!(
        "Failed to verify macOS trust for the CA certificate: {}",
        message
    )))
}

#[cfg(test)]
mod tests {
    use super::{MacOsTrustStore, is_missing_macos_certificate_error};

    #[test]
    fn test_trust_store_creation() {
        let _store = MacOsTrustStore::new();
    }

    #[test]
    fn missing_delete_message_is_treated_as_not_found() {
        assert!(is_missing_macos_certificate_error(
            "security: SecKeychainSearchCopyNext: Unable to delete certificate matching \"Localify Local Development CA\""
        ));
    }
}
