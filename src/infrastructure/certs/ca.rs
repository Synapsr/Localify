use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair, PKCS_ECDSA_P256_SHA256};
use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;
use std::fs;
use std::path::PathBuf;

use super::CertError;
use crate::branding::BRANDING;

/// A CA the daemon can hold after it stops being root. See
/// [`RootCA::issue_on_demand_issuer`].
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct OnDemandIssuer {
    pub cert_pem: String,
    pub key_pem: String,
}

/// Localify Root Certificate Authority
///
/// This CA is used to sign all domain certificates. Users only need to trust
/// the CA once, and all domain certificates will be automatically trusted.
pub struct RootCA {
    data_dir: PathBuf,
}

impl RootCA {
    /// Create a RootCA with the given base directory
    pub fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }

    /// Path to the CA certificate
    pub fn cert_path(&self) -> PathBuf {
        self.data_dir.join("ca.crt")
    }

    /// Path to the CA private key
    pub fn key_path(&self) -> PathBuf {
        self.data_dir.join("ca.key")
    }

    /// Check if the CA already exists
    pub fn exists(&self) -> bool {
        self.cert_path().exists() && self.key_path().exists()
    }

    /// Generate a new Root CA certificate
    pub fn generate(&self) -> Result<(), CertError> {
        // A dev install's CA key belongs to the user, so it may only sign for
        // dev domains. The production key stays readable only by root.
        self.generate_with_limit(BRANDING.is_dev_profile())
    }

    fn generate_with_limit(&self, local_domains_only: bool) -> Result<(), CertError> {
        // Ensure directory exists
        fs::create_dir_all(&self.data_dir).map_err(|e| CertError::WriteError {
            path: self.data_dir.clone(),
            source: e,
        })?;

        // Generate ECDSA P-256 key pair
        let key_pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
            .map_err(|e| CertError::GenerationError(e.to_string()))?;

        // Configure CA certificate parameters (shared DN + validity + key usage)
        let mut params = super::generator::build_ca_cert_params();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        if local_domains_only {
            params.name_constraints = Some(super::generator::local_domains_only());
        }

        // Generate the self-signed CA certificate
        let cert = params
            .self_signed(&key_pair)
            .map_err(|e| CertError::GenerationError(e.to_string()))?;

        // Save certificate
        let cert_path = self.cert_path();
        fs::write(&cert_path, cert.pem()).map_err(|e| CertError::WriteError {
            path: cert_path.clone(),
            source: e,
        })?;

        // Owner read/write only, from the moment the file exists
        let key_path = self.key_path();
        crate::infrastructure::file_security::write_private_file(
            &key_path,
            key_pair.serialize_pem().as_bytes(),
        )
        .map_err(|e| CertError::WriteError {
            path: key_path.clone(),
            source: e,
        })?;

        Ok(())
    }

    /// Load the CA key pair for signing
    pub fn load_key_pair(&self) -> Result<KeyPair, CertError> {
        let key_pem = fs::read_to_string(self.key_path()).map_err(|e| CertError::ReadError {
            path: self.key_path(),
            source: e,
        })?;

        KeyPair::from_pem(&key_pem).map_err(|e| CertError::GenerationError(e.to_string()))
    }

    fn load_cert_der(&self) -> Result<CertificateDer<'static>, CertError> {
        CertificateDer::from_pem_file(self.cert_path()).map_err(|e| CertError::ReadError {
            path: self.cert_path(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidData, e),
        })
    }

    /// Sign a certificate with this CA
    /// Returns the signed certificate PEM
    pub fn sign_certificate(
        &self,
        params: CertificateParams,
        key_pair: &KeyPair,
    ) -> Result<String, CertError> {
        let ca_key_pair = self.load_key_pair()?;
        let ca_cert_der = self.load_cert_der()?;
        let issuer = Issuer::from_ca_cert_der(&ca_cert_der, ca_key_pair)
            .map_err(|e| CertError::GenerationError(e.to_string()))?;

        // Sign the domain certificate
        let cert = params
            .signed_by(key_pair, &issuer)
            .map_err(|e| CertError::GenerationError(e.to_string()))?;

        Ok(cert.pem())
    }

    /// Mints a fresh issuer for the daemon's on-demand certificates, so the daemon
    /// never loads the Root CA key.
    ///
    /// A separate root process runs this and prints the result: the daemon keeps
    /// running as the user, and memory it freed could still hold what it read.
    pub fn issue_on_demand_issuer(&self) -> Result<OnDemandIssuer, CertError> {
        let ca_key_pair = self.load_key_pair()?;
        let ca_cert_der = self.load_cert_der()?;
        let issuer = Issuer::from_ca_cert_der(&ca_cert_der, ca_key_pair)
            .map_err(|e| CertError::GenerationError(e.to_string()))?;

        let key_pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
            .map_err(|e| CertError::GenerationError(e.to_string()))?;
        let cert = super::generator::build_on_demand_issuer_params()
            .signed_by(&key_pair, &issuer)
            .map_err(|e| CertError::GenerationError(e.to_string()))?;

        Ok(OnDemandIssuer {
            cert_pem: cert.pem(),
            key_pem: key_pair.serialize_pem(),
        })
    }

    /// Delete the CA certificate and key (used by uninstall)
    pub fn delete(&self) -> Result<(), CertError> {
        let cert_path = self.cert_path();
        let key_path = self.key_path();

        if cert_path.exists() {
            fs::remove_file(&cert_path).map_err(|e| CertError::DeleteError {
                path: cert_path,
                source: e,
            })?;
        }

        if key_path.exists() {
            fs::remove_file(&key_path).map_err(|e| CertError::DeleteError {
                path: key_path,
                source: e,
            })?;
        }

        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn the_ca_key_is_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let ca = RootCA::new(dir.path().to_path_buf());

        ca.generate().unwrap();

        let mode = fs::metadata(ca.key_path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o7777, 0o600);
    }

    /// Builds what a dev install holds: a Root CA limited to local names, whose
    /// key the user can read.
    pub(crate) fn dev_root_ca(dir: &std::path::Path) -> RootCA {
        let ca = RootCA::new(dir.to_path_buf());
        ca.generate_with_limit(true).unwrap();
        ca
    }

    #[test]
    fn test_ca_paths() {
        let ca = RootCA::new(PathBuf::from(format!("/tmp/test-{}", BRANDING.slug())));
        assert!(ca.cert_path().to_string_lossy().contains("ca.crt"));
        assert!(ca.key_path().to_string_lossy().contains("ca.key"));
    }
}
