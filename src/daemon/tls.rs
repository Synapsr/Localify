use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::RwLock;

use anyhow::{Context, Result};
use rcgen::{Issuer, KeyPair, PKCS_ECDSA_P256_SHA256, SanType};
use rustls::ServerConfig;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::ResolvesServerCert;
use rustls::sign::CertifiedKey;
use tokio_rustls::TlsAcceptor;
use tracing::{info, warn};

use crate::domain::{DomainName, DomainPattern};
use crate::infrastructure::certs::CertificateGenerator;
use crate::infrastructure::certs::generator::{build_leaf_cert_params, is_local_domain};

const ON_DEMAND_CERT_CACHE_MAX: usize = 256;

/// Custom certificate resolver that selects certificates based on SNI hostname.
///
/// For unknown `.localify` domains, we generate an on-demand certificate signed by
/// Localify's local Root CA. This allows the HTTP layer to render a friendly
/// "Domain Not Registered" page instead of the browser showing a TLS error.
#[derive(Debug)]
struct DomainCertResolver {
    /// All registered certificates, stored with their pattern for matching.
    certs: Vec<(DomainPattern, Arc<CertifiedKey>)>,
    ca_cert_der: Option<CertificateDer<'static>>,
    ca_key_pem: Option<String>,
    on_demand: RwLock<HashMap<String, Arc<CertifiedKey>>>,
}

impl ResolvesServerCert for DomainCertResolver {
    fn resolve(&self, client_hello: rustls::server::ClientHello) -> Option<Arc<CertifiedKey>> {
        let hostname = client_hello.server_name()?.to_lowercase();

        // Try cached on-demand certs first (for unregistered but valid .localify domains).
        if let Some(cert) = self.on_demand.read().ok()?.get(hostname.as_str()).cloned() {
            return Some(cert);
        }

        // Find the first registered cert whose pattern matches the hostname.
        // Certs are pre-sorted by specificity (most specific first).
        for (pattern, cert) in &self.certs {
            if pattern.matches_hostname(&hostname) {
                return Some(cert.clone());
            }
        }

        // Generate an on-demand cert for valid `.localify` hostnames if we
        // can read the local CA private key.
        let ca_cert_der = self.ca_cert_der.as_ref()?;
        let ca_key_pem = self.ca_key_pem.as_deref()?;
        if !can_sign_on_demand(&hostname) {
            warn!(hostname = %hostname, "TLS: no certificate for domain");
            return None;
        }

        match generate_on_demand_certified_key(hostname.as_str(), ca_cert_der, ca_key_pem) {
            Ok(cert) => {
                if let Ok(mut cache) = self.on_demand.write() {
                    // Bound memory: on-demand certs are cheap to regenerate.
                    if cache.len() >= ON_DEMAND_CERT_CACHE_MAX {
                        cache.clear();
                    }
                    cache.insert(hostname, cert.clone());
                }
                Some(cert)
            }
            Err(e) => {
                warn!(hostname = %hostname, error = %e, "TLS: failed to generate on-demand certificate");
                None
            }
        }
    }
}

/// The CA only signs for valid names under the local suffix, whoever asks. Its
/// name constraint would make any other certificate useless anyway.
fn can_sign_on_demand(hostname: &str) -> bool {
    DomainName::new(hostname).is_ok() && is_local_domain(hostname)
}

/// Load all domain certificates into a single TLS acceptor with SNI
pub fn create_tls_acceptor(
    patterns: &[DomainPattern],
    certs_dir: &Path,
    data_dir: &Path,
) -> Result<Option<TlsAcceptor>> {
    let (ca_cert_der, ca_key_pem) = match load_ca_material(data_dir) {
        Ok(material) => material,
        Err(e) => {
            warn!(
                error = %e,
                "TLS: failed to load Localify CA material (on-demand certificates disabled)"
            );
            (None, None)
        }
    };

    // If we have neither per-domain certificates nor a Root CA to generate
    // on-demand certificates, HTTPS can't be served.
    if patterns.is_empty() && ca_key_pem.is_none() {
        return Ok(None);
    }

    let mut certs: Vec<(DomainPattern, Arc<CertifiedKey>)> = Vec::new();

    let generator = CertificateGenerator::new(data_dir.to_path_buf(), certs_dir.to_path_buf());

    for pattern in patterns {
        let stem = crate::infrastructure::certs::cert_name(pattern);
        let cert_path = certs_dir.join(format!("{}.crt", stem));
        let key_path = certs_dir.join(format!("{}.key", stem));

        if !cert_path.exists() || !key_path.exists() {
            info!(domain = %pattern, "Certificate missing, generating automatically");
            match generator.generate(pattern) {
                Ok(cert) => {
                    if let Err(e) = generator.save(&cert) {
                        warn!(domain = %pattern, error = %e, "Failed to save auto-generated certificate, skipping HTTPS for this domain");
                        continue;
                    }
                }
                Err(e) => {
                    warn!(domain = %pattern, error = %e, "Failed to auto-generate certificate, skipping HTTPS for this domain");
                    continue;
                }
            }
        }

        let loaded_certs = load_certs(&cert_path)?;
        let key = load_private_key(&key_path)?;

        let signing_key = rustls::crypto::aws_lc_rs::sign::any_supported_type(&key)
            .context("Failed to create signing key")?;

        let certified_key = Arc::new(CertifiedKey::new(loaded_certs, signing_key));
        certs.push((pattern.clone(), certified_key));
    }

    // If all certificate loads/generations failed and we have no CA key for
    // on-demand certs, HTTPS can't serve anything — don't start the listener.
    if certs.is_empty() && ca_key_pem.is_none() {
        return Ok(None);
    }

    // Most-specific pattern wins (longest base domain).
    certs.sort_by_key(|(p, _)| std::cmp::Reverse(p.specificity()));

    let resolver = Arc::new(DomainCertResolver {
        certs,
        ca_cert_der,
        ca_key_pem,
        on_demand: RwLock::new(HashMap::new()),
    });

    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_cert_resolver(resolver);

    Ok(Some(TlsAcceptor::from(Arc::new(config))))
}

fn load_ca_material(data_dir: &Path) -> Result<(Option<CertificateDer<'static>>, Option<String>)> {
    let ca_cert_path = data_dir.join("ca.crt");
    let ca_key_path = data_dir.join("ca.key");
    if !ca_cert_path.exists() || !ca_key_path.exists() {
        return Ok((None, None));
    }

    let cert = CertificateDer::from_pem_file(&ca_cert_path)
        .with_context(|| format!("Failed to read CA certificate: {}", ca_cert_path.display()))?;
    let pem = fs::read_to_string(&ca_key_path)
        .with_context(|| format!("Failed to read CA private key: {}", ca_key_path.display()))?;
    Ok((Some(cert), Some(pem)))
}

fn generate_on_demand_certified_key(
    hostname: &str,
    ca_cert_der: &CertificateDer<'_>,
    ca_key_pem: &str,
) -> Result<Arc<CertifiedKey>> {
    let leaf_key_pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
        .context("Failed to generate leaf key pair")?;

    let san = SanType::DnsName(
        hostname
            .try_into()
            .map_err(|e| anyhow::anyhow!("Invalid hostname for SAN: {}", e))?,
    );
    let params = build_leaf_cert_params(hostname, vec![san]);

    let ca_key_pair = KeyPair::from_pem(ca_key_pem).context("Failed to parse CA key")?;
    let issuer =
        Issuer::from_ca_cert_der(ca_cert_der, ca_key_pair).context("Failed to parse CA cert")?;

    let cert = params
        .signed_by(&leaf_key_pair, &issuer)
        .context("Failed to sign on-demand certificate")?;

    let certs = vec![cert.der().clone()];
    let key = PrivateKeyDer::try_from(leaf_key_pair.serialize_der())
        .map_err(|e| anyhow::anyhow!("Failed to parse generated private key: {}", e))?;

    let signing_key = rustls::crypto::aws_lc_rs::sign::any_supported_type(&key)
        .context("Failed to create signing key")?;

    Ok(Arc::new(CertifiedKey::new(certs, signing_key)))
}

fn load_certs(path: &Path) -> Result<Vec<CertificateDer<'static>>> {
    let certs: Vec<_> = CertificateDer::pem_file_iter(path)
        .with_context(|| format!("Failed to open cert file: {}", path.display()))?
        .collect::<Result<Vec<_>, _>>()
        .context("Failed to parse certificates")?;

    Ok(certs)
}

fn load_private_key(path: &Path) -> Result<PrivateKeyDer<'static>> {
    PrivateKeyDer::from_pem_file(path)
        .with_context(|| format!("Failed to load private key from: {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::branding::BRANDING;
    use crate::infrastructure::certs::ca::RootCA;
    use crate::infrastructure::certs::ca::tests::is_trusted_for;

    #[test]
    fn only_valid_names_under_the_local_suffix_get_a_certificate_on_demand() {
        assert!(can_sign_on_demand(&format!(
            "added-while-running{}",
            BRANDING.domain_suffix()
        )));
        assert!(can_sign_on_demand(&format!(
            "deep.sub.app{}",
            BRANDING.domain_suffix()
        )));

        assert!(!can_sign_on_demand("bank.example.com"));
        assert!(!can_sign_on_demand("localhost"));
        assert!(!can_sign_on_demand("evil.localify.example.com"));
    }

    #[test]
    fn a_certificate_made_on_demand_is_trusted_through_the_ca() {
        let dir = tempfile::tempdir().unwrap();
        let ca = RootCA::new(dir.path().to_path_buf());
        ca.generate().unwrap();
        let (cert, key) = load_ca_material(dir.path()).unwrap();
        let host = format!("added-while-running{}", BRANDING.domain_suffix());

        let certified =
            generate_on_demand_certified_key(&host, &cert.unwrap(), &key.unwrap()).unwrap();

        assert!(is_trusted_for(&ca, &certified.cert[0], &host));
    }
}
