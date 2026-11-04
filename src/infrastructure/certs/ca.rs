use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair, PKCS_ECDSA_P256_SHA256};
use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;
use std::fs;
use std::path::PathBuf;
use x509_parser::extensions::{GeneralName, ParsedExtension};

use super::CertError;
use crate::branding::BRANDING;

/// Localify Root Certificate Authority
///
/// This CA is used to sign all domain certificates. Users only need to trust
/// the CA once, and all domain certificates will be automatically trusted.
///
/// The key belongs to the user and the daemon signs with it, so the certificate
/// carries a critical name constraint: clients reject anything it signs outside
/// the local suffix, whoever holds the key.
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
        params.name_constraints = Some(super::generator::local_domains_only());

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

    /// Whether the CA on disk can only sign for the local suffix. An older CA
    /// without the constraint must be replaced, not trusted again.
    pub fn is_limited_to_local_domains(&self) -> Result<bool, CertError> {
        let path = self.cert_path();
        let pem = fs::read_to_string(&path).map_err(|source| CertError::ReadError {
            path: path.clone(),
            source,
        })?;
        pem_is_limited_to_local_domains(&pem)
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

/// Whether this certificate carries a critical name constraint permitting only the
/// local suffix.
pub fn pem_is_limited_to_local_domains(pem: &str) -> Result<bool, CertError> {
    let invalid =
        |reason: String| CertError::GenerationError(format!("Invalid CA certificate: {reason}"));

    let der = CertificateDer::from_pem_slice(pem.as_bytes()).map_err(|e| invalid(e.to_string()))?;
    let (_, cert) =
        x509_parser::parse_x509_certificate(der.as_ref()).map_err(|e| invalid(e.to_string()))?;

    Ok(cert.extensions().iter().any(|extension| {
        let ParsedExtension::NameConstraints(constraints) = extension.parsed_extension() else {
            return false;
        };
        let permits_only_local = matches!(
            constraints.permitted_subtrees.as_deref(),
            Some([subtree]) if matches!(subtree.base, GeneralName::DNSName(name) if name == BRANDING.domain_label())
        );
        extension.critical && permits_only_local
    }))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use rustls::RootCertStore;
    use rustls::client::WebPkiServerVerifier;
    use rustls::client::danger::ServerCertVerifier;
    use rustls::pki_types::{ServerName, UnixTime};
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;

    fn generated_ca() -> (tempfile::TempDir, RootCA) {
        let dir = tempfile::tempdir().unwrap();
        let ca = RootCA::new(dir.path().to_path_buf());
        ca.generate().unwrap();
        (dir, ca)
    }

    /// Verifies a certificate the way a TLS client trusting only this CA does.
    pub(crate) fn is_trusted_for(ca: &RootCA, leaf: &CertificateDer<'_>, host: &str) -> bool {
        let mut roots = RootCertStore::empty();
        roots.add(ca.load_cert_der().unwrap()).unwrap();
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let verifier = WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider)
            .build()
            .unwrap();
        verifier
            .verify_server_cert(
                leaf,
                &[],
                &ServerName::try_from(host.to_string()).unwrap(),
                &[],
                UnixTime::now(),
            )
            .is_ok()
    }

    fn sign_leaf(ca: &RootCA, host: &str) -> CertificateDer<'static> {
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let san = rcgen::SanType::DnsName(host.try_into().unwrap());
        let params = super::super::generator::build_leaf_cert_params(host, vec![san]);
        let pem = ca.sign_certificate(params, &key).unwrap();
        CertificateDer::from_pem_slice(pem.as_bytes()).unwrap()
    }

    #[test]
    fn the_ca_key_is_owner_only() {
        let (_dir, ca) = generated_ca();

        let mode = fs::metadata(ca.key_path()).unwrap().permissions().mode();

        assert_eq!(mode & 0o7777, 0o600);
    }

    #[test]
    fn a_new_ca_is_limited_to_the_local_suffix_by_a_critical_constraint() {
        let (_dir, ca) = generated_ca();

        assert!(ca.is_limited_to_local_domains().unwrap());
    }

    #[test]
    fn an_older_ca_without_the_constraint_is_detected() {
        let dir = tempfile::tempdir().unwrap();
        let ca = RootCA::new(dir.path().to_path_buf());
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let mut params = super::super::generator::build_ca_cert_params();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        fs::write(ca.cert_path(), params.self_signed(&key).unwrap().pem()).unwrap();

        assert!(!ca.is_limited_to_local_domains().unwrap());
    }

    #[test]
    fn clients_accept_local_names_at_any_depth_and_reject_others() {
        let (_dir, ca) = generated_ca();
        let suffix = BRANDING.domain_suffix();
        let app = format!("app{suffix}");
        let deep = format!("deep.sub.app{suffix}");

        assert!(is_trusted_for(&ca, &sign_leaf(&ca, &app), &app));
        assert!(is_trusted_for(&ca, &sign_leaf(&ca, &deep), &deep));
        assert!(!is_trusted_for(
            &ca,
            &sign_leaf(&ca, "bank.example.com"),
            "bank.example.com"
        ));
    }

    #[test]
    fn test_ca_paths() {
        let ca = RootCA::new(PathBuf::from(format!("/tmp/test-{}", BRANDING.slug())));
        assert!(ca.cert_path().to_string_lossy().contains("ca.crt"));
        assert!(ca.key_path().to_string_lossy().contains("ca.key"));
    }
}
