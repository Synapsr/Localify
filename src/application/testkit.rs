//! In-memory implementations of all application ports for testing.
//!
//! These are real implementations backed by simple data structures.
//! No mock libraries, no magic — just `RefCell<Vec<..>>` and friends.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::net::Ipv4Addr;
use std::time::Duration;

use crate::application::StepOutcome;
use crate::branding::BRANDING;
use crate::config::{DaemonConfig, LocalifyPaths};
use crate::domain::{
    DomainPattern, DomainRegistration, PathPrefix, ProxyTarget, Route, RouteTarget,
};

use super::ports::{
    CertificateError, CertificateManager, CloudAccount, CloudAccountStore, CloudApi, CloudApiError,
    ConfigLoadError, ConfigLoader, DaemonConnection, DaemonConnectionError, DaemonControl,
    DaemonRuntimeInfo, DeviceAuthorization, DeviceTokenPoll, DnsConfigError, DnsManager,
    DomainRepository, IosSimulatorTrust, LocalDomainOwner, LocalDomains, NetworkInfo, NewPublicUrl,
    Protection, ProtectionChoice, ProtectionMode, PublicTunnel, PublicUrl, PublicUrlWithPassword,
    RegistrationProvider, RepositoryError, ServingMachine, SignInRequest, SystemSetup, TunnelState,
    TunnelStatus,
};

// ---------------------------------------------------------------------------
// InMemoryRegistrationProvider
// ---------------------------------------------------------------------------

pub struct InMemoryRegistrationProvider {
    registrations: Vec<DomainRegistration>,
}

impl InMemoryRegistrationProvider {
    pub fn new(registrations: Vec<DomainRegistration>) -> Self {
        Self { registrations }
    }
}

impl RegistrationProvider for InMemoryRegistrationProvider {
    fn name(&self) -> &str {
        "in-memory"
    }

    fn load(&self) -> anyhow::Result<Vec<DomainRegistration>> {
        Ok(self.registrations.clone())
    }
}

// ---------------------------------------------------------------------------
// InMemoryDomainRepository
// ---------------------------------------------------------------------------

pub struct InMemoryDomainRepository {
    domains: RefCell<Vec<DomainRegistration>>,
}

impl InMemoryDomainRepository {
    pub fn new() -> Self {
        Self {
            domains: RefCell::new(Vec::new()),
        }
    }

    pub fn with_domains(domains: Vec<DomainRegistration>) -> Self {
        Self {
            domains: RefCell::new(domains),
        }
    }
}

impl DomainRepository for InMemoryDomainRepository {
    fn get(&self, pattern: &DomainPattern) -> Result<Option<DomainRegistration>, RepositoryError> {
        Ok(self
            .domains
            .borrow()
            .iter()
            .find(|r| r.pattern() == pattern)
            .cloned())
    }

    fn list(&self) -> Result<Vec<DomainRegistration>, RepositoryError> {
        Ok(self.domains.borrow().clone())
    }

    fn add(&self, registration: DomainRegistration) -> Result<(), RepositoryError> {
        let key = registration.display_pattern();
        if self
            .domains
            .borrow()
            .iter()
            .any(|r| r.display_pattern() == key)
        {
            return Err(RepositoryError::DomainExists(key));
        }
        self.domains.borrow_mut().push(registration);
        Ok(())
    }

    fn update(&self, registration: DomainRegistration) -> Result<(), RepositoryError> {
        let key = registration.display_pattern();
        let mut domains = self.domains.borrow_mut();
        let pos = domains
            .iter()
            .position(|r| r.display_pattern() == key)
            .ok_or_else(|| RepositoryError::DomainNotFound(key))?;
        domains[pos] = registration;
        Ok(())
    }

    fn remove(&self, pattern: &DomainPattern) -> Result<(), RepositoryError> {
        let key = pattern.display_pattern();
        let mut domains = self.domains.borrow_mut();
        let pos = domains
            .iter()
            .position(|r| r.display_pattern() == key)
            .ok_or_else(|| RepositoryError::DomainNotFound(key))?;
        domains.remove(pos);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// InMemoryCertificateManager
// ---------------------------------------------------------------------------

pub struct InMemoryCertificateManager {
    ca_installed: RefCell<bool>,
    ca_exists: RefCell<bool>,
    /// When true, `init_ca` creates the CA but the system does not trust it.
    trust_stays_pending: bool,
    certs: RefCell<Vec<String>>,
    /// When true, all cert operations fail.
    fail_operations: bool,
}

impl InMemoryCertificateManager {
    pub fn new() -> Self {
        Self {
            ca_installed: RefCell::new(false),
            ca_exists: RefCell::new(false),
            trust_stays_pending: false,
            certs: RefCell::new(Vec::new()),
            fail_operations: false,
        }
    }

    pub fn always_failing() -> Self {
        Self {
            fail_operations: true,
            ..Self::new()
        }
    }

    pub fn with_ca_installed() -> Self {
        Self {
            ca_installed: RefCell::new(true),
            ca_exists: RefCell::new(true),
            ..Self::new()
        }
    }

    pub fn created_but_untrusted() -> Self {
        Self {
            ca_exists: RefCell::new(true),
            trust_stays_pending: true,
            ..Self::new()
        }
    }
}

impl CertificateManager for InMemoryCertificateManager {
    fn init_ca(&self) -> Result<(), CertificateError> {
        if self.fail_operations {
            return Err(CertificateError::OperationFailed(anyhow::anyhow!(
                "simulated CA failure"
            )));
        }
        *self.ca_exists.borrow_mut() = true;
        *self.ca_installed.borrow_mut() = !self.trust_stays_pending;
        Ok(())
    }

    fn is_ca_installed(&self) -> Result<bool, CertificateError> {
        Ok(*self.ca_installed.borrow())
    }

    fn ca_exists(&self) -> bool {
        *self.ca_exists.borrow()
    }

    fn create_and_install(&self, pattern: &DomainPattern) -> Result<(), CertificateError> {
        if self.fail_operations {
            return Err(CertificateError::OperationFailed(anyhow::anyhow!(
                "simulated cert failure"
            )));
        }
        self.certs.borrow_mut().push(pattern.display_pattern());
        Ok(())
    }

    fn remove(&self, pattern: &DomainPattern) -> Result<(), CertificateError> {
        if self.fail_operations {
            return Err(CertificateError::OperationFailed(anyhow::anyhow!(
                "simulated remove failure"
            )));
        }
        self.certs
            .borrow_mut()
            .retain(|c| *c != pattern.display_pattern());
        Ok(())
    }

    fn remove_ca(&self) -> Result<(), CertificateError> {
        if self.fail_operations {
            return Err(CertificateError::OperationFailed(anyhow::anyhow!(
                "simulated remove-CA failure"
            )));
        }
        *self.ca_installed.borrow_mut() = false;
        Ok(())
    }

    fn exists(&self, pattern: &DomainPattern) -> bool {
        self.certs.borrow().contains(&pattern.display_pattern())
    }

    fn is_trusted(&self) -> Result<bool, CertificateError> {
        Ok(*self.ca_installed.borrow())
    }
}

// ---------------------------------------------------------------------------
// InMemoryConfigLoader
// ---------------------------------------------------------------------------

pub struct InMemoryConfigLoader {
    file_exists: RefCell<bool>,
    daemon_config: DaemonConfig,
    paths: LocalifyPaths,
}

impl InMemoryConfigLoader {
    pub fn new() -> Self {
        Self {
            file_exists: RefCell::new(false),
            daemon_config: DaemonConfig::default(),
            paths: LocalifyPaths::default(),
        }
    }

    pub fn existing() -> Self {
        Self {
            file_exists: RefCell::new(true),
            daemon_config: DaemonConfig::default(),
            paths: LocalifyPaths::default(),
        }
    }
}

impl ConfigLoader for InMemoryConfigLoader {
    fn load(&self) -> Result<(DaemonConfig, LocalifyPaths), ConfigLoadError> {
        Ok((self.daemon_config.clone(), self.paths.clone()))
    }

    fn save_defaults(&self) -> Result<(), ConfigLoadError> {
        *self.file_exists.borrow_mut() = true;
        Ok(())
    }

    fn exists(&self) -> bool {
        *self.file_exists.borrow()
    }
}

// ---------------------------------------------------------------------------
// InMemoryDaemonControl
// ---------------------------------------------------------------------------

pub struct InMemoryDaemonControl {
    running_pid: RefCell<Option<u32>>,
}

impl InMemoryDaemonControl {
    pub fn stopped() -> Self {
        Self {
            running_pid: RefCell::new(None),
        }
    }

    pub fn running(pid: u32) -> Self {
        Self {
            running_pid: RefCell::new(Some(pid)),
        }
    }
}

impl DaemonControl for InMemoryDaemonControl {
    fn is_running(&self) -> anyhow::Result<bool> {
        Ok(self.running_pid.borrow().is_some())
    }

    fn get_running_pid(&self) -> anyhow::Result<Option<u32>> {
        Ok(*self.running_pid.borrow())
    }

    fn stop_gracefully(&self, _timeout: Duration) -> anyhow::Result<()> {
        *self.running_pid.borrow_mut() = None;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// InMemoryDnsManager
// ---------------------------------------------------------------------------

pub struct InMemoryDnsManager {
    configured: RefCell<bool>,
    valid: RefCell<bool>,
}

impl InMemoryDnsManager {
    pub fn new() -> Self {
        Self {
            configured: RefCell::new(false),
            valid: RefCell::new(false),
        }
    }

    pub fn already_configured() -> Self {
        Self {
            configured: RefCell::new(true),
            valid: RefCell::new(true),
        }
    }

    pub fn invalid_existing() -> Self {
        Self {
            configured: RefCell::new(true),
            valid: RefCell::new(false),
        }
    }
}

impl DnsManager for InMemoryDnsManager {
    fn setup(&self, _port: u16) -> Result<(), DnsConfigError> {
        *self.configured.borrow_mut() = true;
        *self.valid.borrow_mut() = true;
        Ok(())
    }

    fn cleanup(&self) -> Result<(), DnsConfigError> {
        *self.configured.borrow_mut() = false;
        *self.valid.borrow_mut() = false;
        Ok(())
    }

    fn validate(&self) -> Result<(), DnsConfigError> {
        if !*self.valid.borrow() {
            return Err(DnsConfigError::ValidationFailed(
                "Resolver file has incorrect content".into(),
            ));
        }
        Ok(())
    }

    fn is_configured(&self) -> bool {
        *self.configured.borrow()
    }
}

// ---------------------------------------------------------------------------
// InMemoryNetworkInfo
// ---------------------------------------------------------------------------

pub struct InMemoryNetworkInfo {
    ip: Option<Ipv4Addr>,
}

impl InMemoryNetworkInfo {
    pub fn with_ip(ip: Ipv4Addr) -> Self {
        Self { ip: Some(ip) }
    }

    pub fn unavailable() -> Self {
        Self { ip: None }
    }
}

impl NetworkInfo for InMemoryNetworkInfo {
    fn lan_ip(&self) -> Option<Ipv4Addr> {
        self.ip
    }
}

// ---------------------------------------------------------------------------
// InMemorySystemSetup
// ---------------------------------------------------------------------------

pub struct InMemorySystemSetup {
    directories_created: RefCell<bool>,
    ios_simulator_trust: RefCell<IosSimulatorTrust>,
    data_exists: RefCell<bool>,
    pid_exists: RefCell<bool>,
    log_exists: RefCell<bool>,
    decline_approval: bool,
    trust_command: Option<String>,
}

impl InMemorySystemSetup {
    pub fn new() -> Self {
        Self {
            directories_created: RefCell::new(false),
            ios_simulator_trust: RefCell::new(IosSimulatorTrust::Trusted {
                device_names: vec!["iPhone Test".into()],
            }),
            data_exists: RefCell::new(false),
            pid_exists: RefCell::new(false),
            log_exists: RefCell::new(false),
            decline_approval: false,
            trust_command: None,
        }
    }

    pub fn declining_approval() -> Self {
        Self {
            decline_approval: true,
            ..Self::new()
        }
    }

    pub fn with_trust_command(command: &str) -> Self {
        Self {
            trust_command: Some(command.into()),
            ..Self::new()
        }
    }

    pub fn with_existing_data() -> Self {
        Self {
            directories_created: RefCell::new(true),
            ios_simulator_trust: RefCell::new(IosSimulatorTrust::Trusted {
                device_names: vec!["iPhone Test".into()],
            }),
            data_exists: RefCell::new(true),
            pid_exists: RefCell::new(true),
            log_exists: RefCell::new(true),
            decline_approval: false,
            trust_command: None,
        }
    }

    pub fn with_ios_simulator_trust(self, trust: IosSimulatorTrust) -> Self {
        *self.ios_simulator_trust.borrow_mut() = trust;
        self
    }
}

impl SystemSetup for InMemorySystemSetup {
    fn apply_system_changes(&self) -> anyhow::Result<Vec<(String, StepOutcome)>> {
        if self.decline_approval {
            anyhow::bail!("The administrator approval was declined.");
        }
        Ok(Vec::new())
    }

    fn remove_system_changes(&self) -> anyhow::Result<Vec<(String, StepOutcome)>> {
        if self.decline_approval {
            anyhow::bail!("The administrator approval was declined.");
        }
        Ok(Vec::new())
    }

    fn ca_trust_command(&self) -> Option<String> {
        self.trust_command.clone()
    }

    fn create_directories(&self) -> anyhow::Result<()> {
        *self.directories_created.borrow_mut() = true;
        Ok(())
    }

    fn trust_ios_simulators(&self) -> anyhow::Result<IosSimulatorTrust> {
        Ok(self.ios_simulator_trust.borrow().clone())
    }

    fn remove_data_directory(&self) -> anyhow::Result<bool> {
        let existed = *self.data_exists.borrow();
        *self.data_exists.borrow_mut() = false;
        Ok(existed)
    }

    fn remove_pid_file(&self) -> bool {
        let existed = *self.pid_exists.borrow();
        *self.pid_exists.borrow_mut() = false;
        existed
    }

    fn remove_log_directory(&self) -> bool {
        let existed = *self.log_exists.borrow();
        *self.log_exists.borrow_mut() = false;
        existed
    }
}

// ---------------------------------------------------------------------------
// InMemoryDaemonConnection
// ---------------------------------------------------------------------------

pub struct InMemoryDaemonConnection {
    registrations: RefCell<Vec<DomainRegistration>>,
}

impl InMemoryDaemonConnection {
    pub fn new(registrations: Vec<DomainRegistration>) -> Self {
        Self {
            registrations: RefCell::new(registrations),
        }
    }
}

impl DaemonConnection for InMemoryDaemonConnection {
    fn status(&self) -> Result<DaemonRuntimeInfo, DaemonConnectionError> {
        Ok(DaemonRuntimeInfo {
            pid: 1234,
            registrations: self.registrations.borrow().clone(),
            http_port: 80,
            https_port: 443,
            dns_port: 1053,
        })
    }

    fn reload(&self) -> Result<(), DaemonConnectionError> {
        Ok(())
    }

    fn list_registrations(&self) -> Result<Vec<DomainRegistration>, DaemonConnectionError> {
        Ok(self.registrations.borrow().clone())
    }
}

// ---------------------------------------------------------------------------
// NotRunningDaemonConnection
// ---------------------------------------------------------------------------

/// A `DaemonConnection` that always returns `NotRunning`.
/// Useful for testing fallback-to-config paths.
pub struct NotRunningDaemonConnection;

impl DaemonConnection for NotRunningDaemonConnection {
    fn status(&self) -> Result<DaemonRuntimeInfo, DaemonConnectionError> {
        Err(DaemonConnectionError::NotRunning)
    }

    fn reload(&self) -> Result<(), DaemonConnectionError> {
        Err(DaemonConnectionError::NotRunning)
    }

    fn list_registrations(&self) -> Result<Vec<DomainRegistration>, DaemonConnectionError> {
        Err(DaemonConnectionError::NotRunning)
    }
}

// ---------------------------------------------------------------------------
// Localify Cloud: InMemoryCloudApi, InMemoryCloudAccountStore,
// InMemoryPublicTunnel, InMemoryLocalDomains
// ---------------------------------------------------------------------------

/// The account `InMemoryCloudAccountStore::signed_in` holds; its machine is `m1`.
pub fn cloud_account() -> CloudAccount {
    CloudAccount {
        api_base: "https://api.localify.dev".into(),
        token: "lfy_test".into(),
        login: "teo-goulois".into(),
        machine_id: "m1".into(),
        machine_name: "laptop".into(),
    }
}

/// A password-protected URL served by `cloud_account`'s machine.
pub fn public_url(id: &str, slug: &str, wildcard: bool) -> PublicUrl {
    PublicUrl {
        id: id.into(),
        slug: slug.into(),
        url: format!("https://{slug}.localify.fyi"),
        local_domain: format!("{}.localify", slug.split('-').next().unwrap_or(slug)),
        wildcard,
        machine: Some(ServingMachine {
            id: "m1".into(),
            name: "laptop".into(),
        }),
        protection: Protection {
            mode: ProtectionMode::Password,
        },
        created_at: "2026-11-05T18:00:00Z".into(),
    }
}

/// Localify Cloud for one user, signed in on machine `m1`. A generated
/// password is always `four-random-words-here`.
pub struct InMemoryCloudApi {
    urls: RefCell<Vec<PublicUrl>>,
    polls: RefCell<VecDeque<DeviceTokenPoll>>,
    sign_out_error: RefCell<Option<CloudApiError>>,
    next_error: RefCell<Option<CloudApiError>>,
    last_sign_in: RefCell<Option<SignInRequest>>,
    calls: RefCell<Vec<&'static str>>,
}

const GENERATED_PASSWORD: &str = "four-random-words-here";

impl InMemoryCloudApi {
    pub fn new() -> Self {
        Self {
            urls: RefCell::new(Vec::new()),
            polls: RefCell::new(VecDeque::new()),
            sign_out_error: RefCell::new(None),
            next_error: RefCell::new(None),
            last_sign_in: RefCell::new(None),
            calls: RefCell::new(Vec::new()),
        }
    }

    /// What the device flow answers, poll after poll; then `Pending`.
    pub fn with_polls(self, polls: Vec<DeviceTokenPoll>) -> Self {
        *self.polls.borrow_mut() = polls.into();
        self
    }

    pub fn with_sign_out_error(self, error: CloudApiError) -> Self {
        *self.sign_out_error.borrow_mut() = Some(error);
        self
    }

    /// The next call about public URLs fails with `error`.
    pub fn with_error(self, error: CloudApiError) -> Self {
        *self.next_error.borrow_mut() = Some(error);
        self
    }

    pub fn insert(&self, url: PublicUrl) {
        self.urls.borrow_mut().push(url);
    }

    pub fn public_urls(&self) -> Vec<PublicUrl> {
        self.urls.borrow().clone()
    }

    pub fn calls(&self) -> Vec<&'static str> {
        self.calls.borrow().clone()
    }

    pub fn last_sign_in_request(&self) -> Option<SignInRequest> {
        self.last_sign_in.borrow().clone()
    }

    fn call(&self, name: &'static str) -> Result<(), CloudApiError> {
        self.calls.borrow_mut().push(name);
        match self.next_error.borrow_mut().take() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn update(
        &self,
        id: &str,
        change: impl FnOnce(&mut PublicUrl),
    ) -> Result<PublicUrl, CloudApiError> {
        let mut urls = self.urls.borrow_mut();
        let url =
            urls.iter_mut()
                .find(|url| url.id == id)
                .ok_or_else(|| CloudApiError::Rejected {
                    status: 404,
                    code: "not_found".into(),
                    message: "No such public URL.".into(),
                })?;
        change(url);
        Ok(url.clone())
    }
}

fn generated_password(protection: &ProtectionChoice) -> Option<String> {
    matches!(protection, ProtectionChoice::Password(None)).then(|| GENERATED_PASSWORD.into())
}

fn protection_mode(protection: &ProtectionChoice) -> ProtectionMode {
    match protection {
        ProtectionChoice::Password(_) => ProtectionMode::Password,
        ProtectionChoice::Public => ProtectionMode::Public,
    }
}

impl CloudApi for InMemoryCloudApi {
    fn start_sign_in(&self) -> Result<DeviceAuthorization, CloudApiError> {
        Ok(DeviceAuthorization {
            device_code: "dc".into(),
            user_code: "ABCD-1234".into(),
            verification_uri: "https://github.com/login/device".into(),
            interval: 5,
            expires_in: 900,
        })
    }

    fn poll_sign_in(&self, request: &SignInRequest) -> Result<DeviceTokenPoll, CloudApiError> {
        *self.last_sign_in.borrow_mut() = Some(request.clone());
        Ok(self
            .polls
            .borrow_mut()
            .pop_front()
            .unwrap_or(DeviceTokenPoll::Pending))
    }

    fn sign_out(&self, _account: &CloudAccount) -> Result<(), CloudApiError> {
        self.calls.borrow_mut().push("sign_out");
        match self.sign_out_error.borrow_mut().take() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn list_public_urls(&self, _account: &CloudAccount) -> Result<Vec<PublicUrl>, CloudApiError> {
        self.call("list")?;
        Ok(self.public_urls())
    }

    fn create_public_url(
        &self,
        _account: &CloudAccount,
        request: &NewPublicUrl,
    ) -> Result<PublicUrlWithPassword, CloudApiError> {
        self.call("create")?;
        let label = request
            .local_domain
            .as_str()
            .split('.')
            .next()
            .unwrap_or("app");
        let slug = match &request.slug {
            Some(slug) => slug.to_string(),
            None => format!("{label}-a8f3"),
        };
        let mut url = public_url(
            &format!("u{}", self.urls.borrow().len() + 1),
            &slug,
            request.wildcard,
        );
        url.local_domain = request.local_domain.to_string();
        url.protection.mode = protection_mode(&request.protection);
        self.insert(url.clone());
        Ok(PublicUrlWithPassword {
            public_url: url,
            password: generated_password(&request.protection),
        })
    }

    fn remove_public_url(&self, _account: &CloudAccount, id: &str) -> Result<(), CloudApiError> {
        self.call("remove")?;
        self.urls.borrow_mut().retain(|url| url.id != id);
        Ok(())
    }

    fn set_protection(
        &self,
        _account: &CloudAccount,
        id: &str,
        protection: &ProtectionChoice,
    ) -> Result<PublicUrlWithPassword, CloudApiError> {
        self.call("set_protection")?;
        let url = self.update(id, |url| url.protection.mode = protection_mode(protection))?;
        Ok(PublicUrlWithPassword {
            public_url: url,
            password: generated_password(protection),
        })
    }

    fn claim_public_url(
        &self,
        account: &CloudAccount,
        id: &str,
    ) -> Result<PublicUrl, CloudApiError> {
        self.call("claim")?;
        self.update(id, |url| {
            url.machine = Some(ServingMachine {
                id: account.machine_id.clone(),
                name: account.machine_name.clone(),
            })
        })
    }
}

pub struct InMemoryCloudAccountStore {
    account: RefCell<Option<CloudAccount>>,
}

impl InMemoryCloudAccountStore {
    /// Signed in with `cloud_account`.
    pub fn signed_in() -> Self {
        Self {
            account: RefCell::new(Some(cloud_account())),
        }
    }

    pub fn signed_out() -> Self {
        Self {
            account: RefCell::new(None),
        }
    }

    pub fn account(&self) -> Option<CloudAccount> {
        self.account.borrow().clone()
    }
}

impl CloudAccountStore for InMemoryCloudAccountStore {
    fn load(&self) -> anyhow::Result<Option<CloudAccount>> {
        Ok(self.account())
    }

    fn save(&self, account: &CloudAccount) -> anyhow::Result<()> {
        *self.account.borrow_mut() = Some(account.clone());
        Ok(())
    }

    fn delete(&self) -> anyhow::Result<()> {
        *self.account.borrow_mut() = None;
        Ok(())
    }
}

/// The daemon's tunnel: connected when the daemon runs.
pub struct InMemoryPublicTunnel {
    running: bool,
    refreshes: Cell<usize>,
}

impl InMemoryPublicTunnel {
    pub fn running() -> Self {
        Self {
            running: true,
            refreshes: Cell::new(0),
        }
    }

    pub fn stopped() -> Self {
        Self {
            running: false,
            refreshes: Cell::new(0),
        }
    }

    /// How many refreshes the running daemon took.
    pub fn refreshes(&self) -> usize {
        self.refreshes.get()
    }
}

impl PublicTunnel for InMemoryPublicTunnel {
    fn status(&self) -> Result<TunnelStatus, DaemonConnectionError> {
        if !self.running {
            return Err(DaemonConnectionError::NotRunning);
        }
        Ok(TunnelStatus {
            state: TunnelState::Connected,
            connections: 4,
            error: None,
        })
    }

    fn refresh(&self) -> Result<(), DaemonConnectionError> {
        if !self.running {
            return Err(DaemonConnectionError::NotRunning);
        }
        self.refreshes.set(self.refreshes.get() + 1);
        Ok(())
    }
}

pub struct InMemoryLocalDomains {
    owners: Vec<(String, LocalDomainOwner)>,
    labels: Vec<String>,
}

impl InMemoryLocalDomains {
    /// Every domain is `NotRegistered` until given an owner.
    pub fn new() -> Self {
        Self {
            owners: Vec::new(),
            labels: Vec::new(),
        }
    }

    pub fn with_owner(mut self, domain: &str, owner: LocalDomainOwner) -> Self {
        self.owners.push((domain.into(), owner));
        self
    }

    /// The labels under any domain.
    pub fn with_labels(mut self, labels: Vec<String>) -> Self {
        self.labels = labels;
        self
    }
}

impl LocalDomains for InMemoryLocalDomains {
    fn owner(&self, domain: &crate::domain::DomainName) -> anyhow::Result<LocalDomainOwner> {
        Ok(self
            .owners
            .iter()
            .find(|(name, _)| name == domain.as_str())
            .map(|(_, owner)| owner.clone())
            .unwrap_or(LocalDomainOwner::NotRegistered))
    }

    fn labels_under(&self, _domain: &crate::domain::DomainName) -> anyhow::Result<Vec<String>> {
        Ok(self.labels.clone())
    }
}

// ---------------------------------------------------------------------------
// Shared test helpers
// ---------------------------------------------------------------------------

/// Build an exact `DomainPattern` from a domain name string.
pub fn exact(name: &str) -> DomainPattern {
    let normalized = name.replace(".localify", BRANDING.domain_suffix());
    DomainPattern::from_name(&normalized, false).unwrap()
}

/// Build a proxy `Route` from a path prefix and port number.
pub fn proxy_route(path: &str, port: u16) -> Route {
    Route::new(
        PathPrefix::new(path).unwrap(),
        RouteTarget::Proxy(ProxyTarget::parse(&port.to_string()).unwrap()),
    )
}

/// Build a simple `DomainRegistration` with one proxy route (/ -> 3000).
pub fn registration(name: &str) -> DomainRegistration {
    DomainRegistration::new(exact(name), vec![proxy_route("/", 3000)])
}
