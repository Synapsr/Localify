use super::ca::RootCA;
use super::trust_store::{TrustStore, get_trust_store};
use super::{CertError, CertificateGenerator};
use crate::application::ports::{CertificateError, CertificateManager};
use crate::branding::BRANDING;
use crate::domain::DomainPattern;
use crate::infrastructure::paths::LocalifyPaths;

/// High-level service for certificate operations
pub struct CertificateService {
    generator: CertificateGenerator,
    ca: RootCA,
}

impl CertificateService {
    /// Create a new CertificateService with paths from LocalifyPaths
    pub fn new(paths: &LocalifyPaths) -> Self {
        Self {
            generator: CertificateGenerator::new(paths.data_dir.clone(), paths.certs_dir.clone()),
            ca: RootCA::new(paths.data_dir.clone()),
        }
    }

    /// Initialize the Root CA (called during `localify install`).
    ///
    /// On macOS this only creates the CA: the install then trusts it in the login
    /// keychain. A CA from an older build has no name constraint, so it is replaced
    /// along with the leaf certificates it signed.
    pub fn init_ca(&self) -> Result<(), CertError> {
        if cfg!(target_os = "macos")
            && self.ca.exists()
            && !self.ca.is_limited_to_local_domains()?
        {
            self.generator.delete_all()?;
            self.ca.delete()?;
        }

        if !self.ca.exists() {
            // Generate CA
            self.ca.generate()?;
        }

        if cfg!(target_os = "macos") {
            return Ok(());
        }

        let cert_path = self.ca.cert_path();
        let trust_store = get_trust_store()?;
        if !trust_store.is_ca_trusted(&cert_path)? {
            trust_store.add_ca(&cert_path)?;
        }

        Ok(())
    }

    /// Check if the Root CA exists and is trusted
    pub fn is_ca_installed(&self) -> Result<bool, CertError> {
        if !self.ca.exists() {
            return Ok(false);
        }

        if cfg!(target_os = "macos") && !self.ca.is_limited_to_local_domains()? {
            return Ok(false);
        }

        let trust_store = get_trust_store()?;
        trust_store.is_ca_trusted(&self.ca.cert_path())
    }

    /// Whether the CA exists, whether or not the system trusts it yet.
    pub fn ca_exists(&self) -> bool {
        self.ca.exists()
    }

    /// Generate a certificate for a domain pattern (signed by the Root CA).
    ///
    /// For exact patterns, generates a single-domain cert.
    /// For wildcard patterns, generates a cert with SANs for base + *.base.
    pub fn create_and_install(&self, pattern: &DomainPattern) -> Result<(), CertError> {
        if !self.ca.exists() {
            return Err(CertError::GenerationError(format!(
                "Root CA not found. Run '{}' first.",
                BRANDING.admin_command("install")
            )));
        }

        let cert = self.generator.generate(pattern)?;
        self.generator.save(&cert)?;
        Ok(())
    }

    /// Remove certificate files for a domain pattern.
    pub fn remove(&self, pattern: &DomainPattern) -> Result<(), CertError> {
        self.generator.delete(pattern)
    }

    /// Check if certificate exists for a domain pattern.
    pub fn exists(&self, pattern: &DomainPattern) -> bool {
        self.generator.exists(pattern)
    }

    /// Check if certificate is trusted (CA is trusted = all certs trusted)
    pub fn is_trusted(&self) -> Result<bool, CertError> {
        self.is_ca_installed()
    }

    /// Remove the Root CA (for uninstall)
    pub fn remove_ca(&self) -> Result<(), CertError> {
        let trust_store = get_trust_store()?;
        trust_store.remove_ca(&self.ca.cert_path())?;
        self.ca.delete()?;
        Ok(())
    }
}

impl CertificateManager for CertificateService {
    fn init_ca(&self) -> Result<(), CertificateError> {
        CertificateService::init_ca(self).map_err(|e| CertificateError::OperationFailed(e.into()))
    }

    fn is_ca_installed(&self) -> Result<bool, CertificateError> {
        CertificateService::is_ca_installed(self)
            .map_err(|e| CertificateError::OperationFailed(e.into()))
    }

    fn ca_exists(&self) -> bool {
        CertificateService::ca_exists(self)
    }

    fn create_and_install(&self, pattern: &DomainPattern) -> Result<(), CertificateError> {
        CertificateService::create_and_install(self, pattern)
            .map_err(|e| CertificateError::OperationFailed(e.into()))
    }

    fn remove(&self, pattern: &DomainPattern) -> Result<(), CertificateError> {
        CertificateService::remove(self, pattern)
            .map_err(|e| CertificateError::OperationFailed(e.into()))
    }

    fn remove_ca(&self) -> Result<(), CertificateError> {
        CertificateService::remove_ca(self).map_err(|e| CertificateError::OperationFailed(e.into()))
    }

    fn exists(&self, pattern: &DomainPattern) -> bool {
        CertificateService::exists(self, pattern)
    }

    fn is_trusted(&self) -> Result<bool, CertificateError> {
        CertificateService::is_trusted(self)
            .map_err(|e| CertificateError::OperationFailed(e.into()))
    }
}
