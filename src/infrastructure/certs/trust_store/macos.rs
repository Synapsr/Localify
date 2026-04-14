use std::path::Path;
use std::process::Command;

use super::super::CertError;
use super::TrustStore;

const ROXY_CA_NAME: &str = "Roxy Local Development CA";

/// macOS Keychain trust store implementation
pub struct MacOsTrustStore;

impl MacOsTrustStore {
    pub fn new() -> Self {
        Self
    }
}

impl TrustStore for MacOsTrustStore {
    fn add_ca(&self, cert_path: &Path) -> Result<(), CertError> {
        if self.is_ca_trusted(cert_path)? {
            return Ok(());
        }

        self.remove_ca(cert_path)?;

        let cert_path_str = cert_path.to_str().ok_or_else(|| {
            CertError::TrustStoreError(format!(
                "Certificate path contains invalid UTF-8: {}",
                cert_path.display()
            ))
        })?;

        // Add CA certificate to system keychain as a trusted root
        let output = Command::new("security")
            .args([
                "add-trusted-cert",
                "-d",
                "-r",
                "trustRoot",
                "-k",
                "/Library/Keychains/System.keychain",
                cert_path_str,
            ])
            .output()
            .map_err(|e| {
                CertError::TrustStoreError(format!("Failed to run security command: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);

            if stderr.contains("authorization")
                || stderr.contains("Permission denied")
                || stderr.contains("User canceled")
            {
                return Err(CertError::PermissionDenied);
            }

            return Err(CertError::TrustStoreError(format!(
                "Failed to add CA to Keychain: {}",
                stderr
            )));
        }

        Ok(())
    }

    fn remove_ca(&self, _cert_path: &Path) -> Result<(), CertError> {
        for hash in find_keychain_hashes()? {
            let delete_output = Command::new("security")
                .args([
                    "delete-certificate",
                    "-Z",
                    &hash,
                    "/Library/Keychains/System.keychain",
                ])
                .output()
                .map_err(|e| {
                    CertError::TrustStoreError(format!("Failed to delete CA certificate: {}", e))
                })?;

            if !delete_output.status.success() {
                let stderr = String::from_utf8_lossy(&delete_output.stderr);

                if stderr.contains("authorization") || stderr.contains("Permission denied") {
                    return Err(CertError::PermissionDenied);
                }

                if !stderr.contains("could not be found")
                    && !stderr.contains("The specified item could not be found")
                {
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
        let local_hash = certificate_sha1(cert_path)?;
        let trusted_hashes = find_keychain_hashes()?;
        Ok(trusted_hashes.iter().any(|hash| hash == &local_hash))
    }
}

impl Default for MacOsTrustStore {
    fn default() -> Self {
        Self::new()
    }
}

fn find_keychain_hashes() -> Result<Vec<String>, CertError> {
    let find_output = Command::new("security")
        .args([
            "find-certificate",
            "-c",
            ROXY_CA_NAME,
            "-a",
            "-Z",
            "/Library/Keychains/System.keychain",
        ])
        .output()
        .map_err(|e| CertError::TrustStoreError(format!("Failed to find CA certificate: {}", e)))?;

    if !find_output.status.success() {
        let stderr = String::from_utf8_lossy(&find_output.stderr);
        if stderr.contains("could not be found")
            || stderr.contains("The specified item could not be found")
        {
            return Ok(Vec::new());
        }

        if stderr.contains("authorization") || stderr.contains("Permission denied") {
            return Err(CertError::PermissionDenied);
        }

        return Err(CertError::TrustStoreError(format!(
            "Failed to find CA certificate: {}",
            stderr
        )));
    }

    Ok(parse_security_find_certificate_hashes(
        &String::from_utf8_lossy(&find_output.stdout),
    ))
}

fn certificate_sha1(cert_path: &Path) -> Result<String, CertError> {
    let cert_path_str = cert_path.to_str().ok_or_else(|| {
        CertError::TrustStoreError(format!(
            "Certificate path contains invalid UTF-8: {}",
            cert_path.display()
        ))
    })?;

    let output = Command::new("openssl")
        .args([
            "x509",
            "-in",
            cert_path_str,
            "-noout",
            "-fingerprint",
            "-sha1",
        ])
        .output()
        .map_err(|e| {
            CertError::TrustStoreError(format!(
                "Failed to calculate the CA fingerprint with openssl: {}",
                e
            ))
        })?;

    if !output.status.success() {
        return Err(CertError::TrustStoreError(format!(
            "Failed to calculate the CA fingerprint: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    parse_openssl_fingerprint(&String::from_utf8_lossy(&output.stdout))
}

fn parse_security_find_certificate_hashes(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| line.strip_prefix("SHA-1 hash:"))
        .map(normalize_hash)
        .filter(|hash| !hash.is_empty())
        .collect()
}

fn parse_openssl_fingerprint(output: &str) -> Result<String, CertError> {
    let hash = output
        .lines()
        .find_map(|line| line.split('=').nth(1))
        .map(normalize_hash)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            CertError::TrustStoreError(format!(
                "Unexpected openssl fingerprint output: {}",
                output.trim()
            ))
        })?;

    Ok(hash)
}

fn normalize_hash(value: &str) -> String {
    value.trim().replace(':', "").to_uppercase()
}

#[cfg(test)]
mod tests {
    use super::{
        MacOsTrustStore, parse_openssl_fingerprint, parse_security_find_certificate_hashes,
    };

    #[test]
    fn test_trust_store_creation() {
        let _store = MacOsTrustStore::new();
    }

    #[test]
    fn parses_security_hashes() {
        let output = r#"
keychain: "/Library/Keychains/System.keychain"
version: 512
class: 0x80001000 
attributes:
    "labl"<blob>="Roxy Local Development CA"
SHA-1 hash: 11AA22BB33CC44DD55EE66FF77889900AABBCCDD
keychain: "/Library/Keychains/System.keychain"
SHA-1 hash: AA:BB:CC:DD:EE:FF:00:11:22:33:44:55:66:77:88:99:00:11:22:33
"#;

        assert_eq!(
            parse_security_find_certificate_hashes(output),
            vec![
                "11AA22BB33CC44DD55EE66FF77889900AABBCCDD".to_string(),
                "AABBCCDDEEFF0011223344556677889900112233".to_string(),
            ]
        );
    }

    #[test]
    fn parses_openssl_fingerprint_output() {
        let output =
            "sha1 Fingerprint=AA:BB:CC:DD:EE:FF:00:11:22:33:44:55:66:77:88:99:00:11:22:33\n";

        assert_eq!(
            parse_openssl_fingerprint(output).unwrap(),
            "AABBCCDDEEFF0011223344556677889900112233"
        );
    }
}
