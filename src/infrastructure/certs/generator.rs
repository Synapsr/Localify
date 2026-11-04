use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose,
    GeneralSubtree, IsCa, KeyPair, KeyUsagePurpose, NameConstraints, PKCS_ECDSA_P256_SHA256,
    SanType,
};
use std::fs;
use std::path::PathBuf;
use time::{Duration, OffsetDateTime};

use super::CertError;
use super::ca::RootCA;
use crate::branding::BRANDING;
use crate::domain::DomainPattern;

/// Longer than any leaf it signs, and renewed whenever the daemon restarts.
const ON_DEMAND_ISSUER_VALIDITY_DAYS: i64 = 398;

/// Represents a generated certificate with its key pair
pub struct Certificate {
    /// File stem used for saving (e.g. "myapp.localify" or "__wildcard__.myapp.localify")
    pub file_stem: String,
    pub cert_pem: String,
    pub key_pem: String,
}

/// Build certificate parameters for a domain leaf certificate.
///
/// Sets up the Distinguished Name, 1-year validity, key usage for
/// server authentication, and the given Subject Alternative Names.
pub(crate) fn build_leaf_cert_params(common_name: &str, sans: Vec<SanType>) -> CertificateParams {
    let mut params = CertificateParams::default();

    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, common_name);
    dn.push(DnType::OrganizationName, BRANDING.local_org_name());
    params.distinguished_name = dn;

    let now = OffsetDateTime::now_utc();
    params.not_before = now;
    params.not_after = now + Duration::days(365);

    params.subject_alt_names = sans;

    params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyEncipherment,
    ];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];

    params
}

/// Build the standard Localify CA certificate parameters.
pub(crate) fn build_ca_cert_params() -> CertificateParams {
    let mut params = CertificateParams::default();

    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, BRANDING.ca_name());
    dn.push(DnType::OrganizationName, BRANDING.app_name);
    params.distinguished_name = dn;

    let now = OffsetDateTime::now_utc();
    params.not_before = now;
    params.not_after = now + Duration::days(3650);

    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];

    params
}

/// Limits a CA to the names Localify serves. Applied to anything whose key a
/// process of the user can hold, so that key can never sign for another site.
pub(crate) fn local_domains_only() -> NameConstraints {
    NameConstraints {
        permitted_subtrees: vec![GeneralSubtree::DnsName(BRANDING.domain_label().to_string())],
        excluded_subtrees: Vec::new(),
    }
}

/// Build parameters for the issuer the daemon signs on-demand certificates with.
///
/// The daemon runs as the user, so it holds this short-lived, name-constrained
/// CA instead of the Root CA. It cannot issue further CAs or sign for any name
/// outside the local suffix.
pub(crate) fn build_on_demand_issuer_params() -> CertificateParams {
    let mut params = CertificateParams::default();

    let mut dn = DistinguishedName::new();
    dn.push(
        DnType::CommonName,
        format!("{} On-Demand Issuer", BRANDING.ca_name()),
    );
    dn.push(DnType::OrganizationName, BRANDING.app_name);
    params.distinguished_name = dn;

    let now = OffsetDateTime::now_utc();
    params.not_before = now;
    params.not_after = now + Duration::days(ON_DEMAND_ISSUER_VALIDITY_DAYS);

    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    params.name_constraints = Some(local_domains_only());

    params
}

/// Whether the Root CA may sign for this name. The CA is trusted system-wide, so
/// it signs only for the local suffix, whoever asks.
pub(crate) fn is_local_domain(name: &str) -> bool {
    name.ends_with(BRANDING.domain_suffix())
}

pub struct CertificateGenerator {
    base_dir: PathBuf,
    certs_dir: PathBuf,
}

impl CertificateGenerator {
    /// Create a CertificateGenerator with explicit directories
    pub fn new(base_dir: PathBuf, certs_dir: PathBuf) -> Self {
        Self {
            base_dir,
            certs_dir,
        }
    }

    /// Generate a certificate for the given domain pattern, signed by the Root CA.
    ///
    /// For exact patterns, generates a single-domain cert.
    /// For wildcard patterns, generates a cert with SANs for base + *.base.
    pub fn generate(&self, pattern: &DomainPattern) -> Result<Certificate, CertError> {
        if !is_local_domain(pattern.base_domain().as_str()) {
            return Err(CertError::GenerationError(format!(
                "{} is not under {}, so {} will not issue a certificate for it.",
                pattern.base_domain(),
                BRANDING.domain_suffix(),
                BRANDING.app_name
            )));
        }

        let ca = RootCA::new(self.base_dir.clone());

        if !ca.exists() {
            return Err(CertError::GenerationError(format!(
                "Root CA not found. Run 'sudo {} install' first.",
                BRANDING.cli_command()
            )));
        }

        // Generate ECDSA P-256 key pair
        let key_pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
            .map_err(|e| CertError::GenerationError(e.to_string()))?;

        let sans = build_sans(pattern)?;
        let params = build_leaf_cert_params(pattern.base_domain().as_str(), sans);

        // Sign certificate with CA
        let cert_pem = ca.sign_certificate(params, &key_pair)?;

        Ok(Certificate {
            file_stem: super::cert_name(pattern),
            cert_pem,
            key_pem: key_pair.serialize_pem(),
        })
    }

    /// Save a certificate to disk
    pub fn save(&self, cert: &Certificate) -> Result<(), CertError> {
        // Ensure certs directory exists
        fs::create_dir_all(&self.certs_dir).map_err(|e| CertError::WriteError {
            path: self.certs_dir.clone(),
            source: e,
        })?;

        let cert_path = self.certs_dir.join(format!("{}.crt", cert.file_stem));
        let key_path = self.certs_dir.join(format!("{}.key", cert.file_stem));

        // Write certificate
        fs::write(&cert_path, &cert.cert_pem).map_err(|e| CertError::WriteError {
            path: cert_path.clone(),
            source: e,
        })?;

        // Owner read/write only, from the moment the file exists
        crate::infrastructure::file_security::write_private_file(
            &key_path,
            cert.key_pem.as_bytes(),
        )
        .map_err(|e| CertError::WriteError {
            path: key_path.clone(),
            source: e,
        })?;

        Ok(())
    }

    /// Delete certificate files for a domain pattern
    pub fn delete(&self, pattern: &DomainPattern) -> Result<(), CertError> {
        let stem = super::cert_name(pattern);
        let cert_path = self.certs_dir.join(format!("{}.crt", stem));
        let key_path = self.certs_dir.join(format!("{}.key", stem));

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

    /// Check if certificate exists for a domain pattern
    pub fn exists(&self, pattern: &DomainPattern) -> bool {
        let stem = super::cert_name(pattern);
        let cert_path = self.certs_dir.join(format!("{}.crt", stem));
        let key_path = self.certs_dir.join(format!("{}.key", stem));
        cert_path.exists() && key_path.exists()
    }
}

/// Build Subject Alternative Names for the given pattern.
fn build_sans(pattern: &DomainPattern) -> Result<Vec<SanType>, CertError> {
    let base_str = pattern.base_domain().as_str();

    let base_san =
        SanType::DnsName(base_str.try_into().map_err(|e| {
            CertError::GenerationError(format!("Invalid domain name for SAN: {}", e))
        })?);

    match pattern {
        DomainPattern::Exact(_) => Ok(vec![base_san]),
        DomainPattern::Wildcard(_) => {
            let wildcard_str = format!("*.{}", base_str);
            let wildcard_san = SanType::DnsName(wildcard_str.try_into().map_err(|e| {
                CertError::GenerationError(format!("Invalid wildcard name for SAN: {}", e))
            })?);
            Ok(vec![base_san, wildcard_san])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::DomainName;
    use crate::infrastructure::certs::ca::RootCA;
    use tempfile::TempDir;

    #[test]
    fn test_certificate_generation() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let base_dir = temp_dir.path().to_path_buf();
        let certs_dir = base_dir.join("certs");

        let ca = RootCA::new(base_dir.clone());
        ca.generate().expect("Failed to generate test CA");

        let domain = DomainName::new("test.localify").unwrap();
        let pattern = DomainPattern::Exact(domain);
        let generator = CertificateGenerator::new(base_dir, certs_dir);

        let cert = generator.generate(&pattern).unwrap();

        assert_eq!(cert.file_stem, "test.localify");
        assert!(cert.cert_pem.contains("BEGIN CERTIFICATE"));
        assert!(cert.key_pem.contains("BEGIN PRIVATE KEY"));
    }

    #[test]
    fn the_root_ca_only_signs_for_local_domains() {
        let temp_dir = TempDir::new().unwrap();
        let base_dir = temp_dir.path().to_path_buf();
        RootCA::new(base_dir.clone()).generate().unwrap();
        let generator = CertificateGenerator::new(base_dir.clone(), base_dir.join("certs"));
        let other_site = DomainPattern::Exact(DomainName::new("bank.example.com").unwrap());

        let error = generator.generate(&other_site).err().unwrap();

        assert!(error.to_string().contains("will not issue"));
    }

    #[test]
    fn leaf_cert_params_include_server_auth_eku() {
        let params = build_leaf_cert_params(
            "test.localify",
            vec![SanType::DnsName("test.localify".try_into().unwrap())],
        );

        assert_eq!(
            params.extended_key_usages,
            vec![ExtendedKeyUsagePurpose::ServerAuth]
        );
    }
}
