use std::collections::HashMap;
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
use crate::infrastructure::certs::OnDemandIssuer;
use crate::infrastructure::certs::generator::{build_leaf_cert_params, is_local_domain};

const ON_DEMAND_CERT_CACHE_MAX: usize = 256;

/// Custom certificate resolver that selects certificates based on SNI hostname.
///
/// For a local name with no certificate loaded at startup (a domain added since,
/// a Docker-discovered one, or an unknown one), we generate a certificate signed
/// by the on-demand issuer. This also lets the HTTP layer render a friendly
/// "Domain Not Registered" page instead of the browser showing a TLS error.
#[derive(Debug)]
struct DomainCertResolver {
    /// All registered certificates, stored with their pattern for matching.
    certs: Vec<(DomainPattern, Arc<CertifiedKey>)>,
    issuer: Option<LoadedIssuer>,
    on_demand: RwLock<HashMap<String, Arc<CertifiedKey>>>,
}

#[derive(Debug)]
struct LoadedIssuer {
    cert_der: CertificateDer<'static>,
    key_pem: String,
}

impl ResolvesServerCert for DomainCertResolver {
    fn resolve(&self, client_hello: rustls::server::ClientHello) -> Option<Arc<CertifiedKey>> {
        let hostname = client_hello.server_name()?.to_lowercase();

        // Try cached on-demand certs first (for unregistered but valid local domains).
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

        // Generate an on-demand cert for valid local hostnames. The issuer is
        // constrained to them, so a certificate for any other name is useless.
        let issuer = self.issuer.as_ref()?;
        if DomainName::new(hostname.as_str()).is_err() || !is_local_domain(&hostname) {
            warn!(hostname = %hostname, "TLS: no certificate for domain");
            return None;
        }

        match generate_on_demand_certified_key(hostname.as_str(), issuer) {
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

/// Load all domain certificates into a single TLS acceptor with SNI.
///
/// Reads the leaf keys, so this runs before the daemon drops root. It never
/// issues a certificate from the Root CA: a domain with no certificate on disk
/// is served by the on-demand issuer instead.
pub fn create_tls_acceptor(
    patterns: &[DomainPattern],
    certs_dir: &Path,
    issuer: Option<OnDemandIssuer>,
) -> Result<Option<TlsAcceptor>> {
    let issuer = load_issuer(issuer);

    // If we have neither per-domain certificates nor an issuer to generate
    // on-demand certificates, HTTPS can't be served.
    if patterns.is_empty() && issuer.is_none() {
        return Ok(None);
    }

    let mut certs: Vec<(DomainPattern, Arc<CertifiedKey>)> = Vec::new();

    for pattern in patterns {
        let stem = crate::infrastructure::certs::cert_name(pattern);
        let cert_path = certs_dir.join(format!("{}.crt", stem));
        let key_path = certs_dir.join(format!("{}.key", stem));

        if !cert_path.exists() || !key_path.exists() {
            info!(domain = %pattern, "Certificate missing, using an on-demand certificate");
            continue;
        }

        let loaded_certs = load_certs(&cert_path)?;
        let key = load_private_key(&key_path)?;

        let signing_key = rustls::crypto::aws_lc_rs::sign::any_supported_type(&key)
            .context("Failed to create signing key")?;

        let certified_key = Arc::new(CertifiedKey::new(loaded_certs, signing_key));
        certs.push((pattern.clone(), certified_key));
    }

    // With no certificate loaded and no issuer for on-demand certs, HTTPS can't
    // serve anything — don't start the listener.
    if certs.is_empty() && issuer.is_none() {
        return Ok(None);
    }

    // Most-specific pattern wins (longest base domain).
    certs.sort_by_key(|(p, _)| std::cmp::Reverse(p.specificity()));

    let resolver = Arc::new(DomainCertResolver {
        certs,
        issuer,
        on_demand: RwLock::new(HashMap::new()),
    });

    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_cert_resolver(resolver);

    Ok(Some(TlsAcceptor::from(Arc::new(config))))
}

fn load_issuer(issuer: Option<OnDemandIssuer>) -> Option<LoadedIssuer> {
    let issuer = issuer?;
    match CertificateDer::from_pem_slice(issuer.cert_pem.as_bytes()) {
        Ok(cert_der) => Some(LoadedIssuer {
            cert_der,
            key_pem: issuer.key_pem,
        }),
        Err(e) => {
            warn!(error = %e, "TLS: invalid on-demand issuer (on-demand certificates disabled)");
            None
        }
    }
}

fn generate_on_demand_certified_key(
    hostname: &str,
    issuer: &LoadedIssuer,
) -> Result<Arc<CertifiedKey>> {
    let leaf_key_pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
        .context("Failed to generate leaf key pair")?;

    let san = SanType::DnsName(
        hostname
            .try_into()
            .map_err(|e| anyhow::anyhow!("Invalid hostname for SAN: {}", e))?,
    );
    let params = build_leaf_cert_params(hostname, vec![san]);

    let issuer_key_pair =
        KeyPair::from_pem(&issuer.key_pem).context("Failed to parse issuer key")?;
    let signer = Issuer::from_ca_cert_der(&issuer.cert_der, issuer_key_pair)
        .context("Failed to parse issuer cert")?;

    let cert = params
        .signed_by(&leaf_key_pair, &signer)
        .context("Failed to sign on-demand certificate")?;

    // Clients know the Root CA, not the issuer, so the chain must include it.
    let certs = vec![cert.der().clone(), issuer.cert_der.clone()];
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
    use crate::infrastructure::certs::RootCA;
    use rustls::RootCertStore;
    use rustls::client::WebPkiServerVerifier;
    use rustls::client::danger::ServerCertVerifier;
    use rustls::pki_types::{ServerName, UnixTime};

    fn root_ca() -> (tempfile::TempDir, RootCA) {
        let dir = tempfile::tempdir().unwrap();
        let ca = RootCA::new(dir.path().to_path_buf());
        ca.generate().unwrap();
        (dir, ca)
    }

    /// Verifies a chain the way a TLS client trusting only the Root CA does.
    fn chain_is_trusted_for(ca: &RootCA, certs: &[CertificateDer<'static>], host: &str) -> bool {
        let mut roots = RootCertStore::empty();
        roots
            .add(CertificateDer::from_pem_file(ca.cert_path()).unwrap())
            .unwrap();
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let verifier = WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider)
            .build()
            .unwrap();
        verifier
            .verify_server_cert(
                &certs[0],
                &certs[1..],
                &ServerName::try_from(host.to_string()).unwrap(),
                &[],
                UnixTime::now(),
            )
            .is_ok()
    }

    fn local_host(label: &str) -> String {
        format!("{label}{}", BRANDING.domain_suffix())
    }

    fn resolver_issuer(ca: &RootCA) -> LoadedIssuer {
        load_issuer(Some(ca.issue_on_demand_issuer().unwrap())).unwrap()
    }

    #[test]
    fn an_on_demand_certificate_chains_to_the_root_ca() {
        let (_dir, ca) = root_ca();
        let host = local_host("added-after-startup");

        let key = generate_on_demand_certified_key(&host, &resolver_issuer(&ca)).unwrap();

        assert_eq!(key.cert.len(), 2);
        assert!(chain_is_trusted_for(&ca, &key.cert, &host));
    }

    #[test]
    fn the_on_demand_issuer_cannot_sign_for_other_sites() {
        let (_dir, ca) = root_ca();

        let key = generate_on_demand_certified_key("bank.example.com", &resolver_issuer(&ca));

        assert!(!chain_is_trusted_for(
            &ca,
            &key.unwrap().cert,
            "bank.example.com"
        ));
    }

    #[test]
    fn the_on_demand_issuer_cannot_issue_another_ca() {
        let (_dir, ca) = root_ca();
        let issuer = resolver_issuer(&ca);
        let signer = Issuer::from_ca_cert_der(
            &issuer.cert_der,
            KeyPair::from_pem(&issuer.key_pem).unwrap(),
        )
        .unwrap();
        let host = local_host("deep");

        let sub_ca_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let mut sub_ca = crate::infrastructure::certs::generator::build_on_demand_issuer_params();
        sub_ca.name_constraints = None;
        let sub_ca_cert = sub_ca.signed_by(&sub_ca_key, &signer).unwrap();
        let sub_signer = Issuer::from_ca_cert_der(sub_ca_cert.der(), sub_ca_key).unwrap();

        let leaf_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let san = SanType::DnsName(host.as_str().try_into().unwrap());
        let leaf = build_leaf_cert_params(&host, vec![san])
            .signed_by(&leaf_key, &sub_signer)
            .unwrap();

        let chain = [
            leaf.der().clone(),
            sub_ca_cert.der().clone(),
            issuer.cert_der.clone(),
        ];
        assert!(!chain_is_trusted_for(&ca, &chain, &host));
    }

    #[test]
    fn a_dev_root_ca_cannot_sign_for_other_sites() {
        let dir = tempfile::tempdir().unwrap();
        let ca = crate::infrastructure::certs::ca::tests::dev_root_ca(dir.path());
        let issuer = resolver_issuer(&ca);
        let host = local_host("dev-app");

        let local = generate_on_demand_certified_key(&host, &issuer).unwrap();
        let other = generate_on_demand_certified_key("bank.example.com", &issuer).unwrap();

        assert!(chain_is_trusted_for(&ca, &local.cert, &host));
        assert!(!chain_is_trusted_for(&ca, &other.cert, "bank.example.com"));
    }

    #[test]
    fn a_tls_acceptor_needs_no_certificate_files_when_it_has_an_issuer() {
        let (dir, ca) = root_ca();
        let pattern = DomainPattern::from_name(&local_host("missing-cert"), false).unwrap();

        let acceptor = create_tls_acceptor(
            &[pattern],
            &dir.path().join("certs"),
            Some(ca.issue_on_demand_issuer().unwrap()),
        )
        .unwrap();

        assert!(acceptor.is_some());
        assert!(!dir.path().join("certs").exists());
    }

    #[test]
    fn no_certificates_and_no_issuer_means_no_https() {
        let dir = tempfile::tempdir().unwrap();
        let pattern = DomainPattern::from_name(&local_host("missing-cert"), false).unwrap();

        assert!(
            create_tls_acceptor(&[], dir.path(), None)
                .unwrap()
                .is_none()
        );
        assert!(
            create_tls_acceptor(&[pattern], dir.path(), None)
                .unwrap()
                .is_none()
        );
    }
}
