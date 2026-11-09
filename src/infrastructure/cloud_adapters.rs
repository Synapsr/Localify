//! Connects the Localify Cloud use cases to the API, the credentials file,
//! the daemon and the config. The CLI and `DesktopApi` build them the same way.

use std::collections::BTreeMap;
use std::future::Future;
use std::path::Path;
use std::process::Command;

use anyhow::Result;

use crate::application::cloud_account::CloudAccounts;
use crate::application::cloud_billing::CloudBilling;
use crate::application::ports::{
    Addon, AddonPrices, BillingPage, BillingPageKind, CheckoutProduct, CloudAccount,
    CloudAccountStore, CloudApi, CloudApiError, Currency, DaemonConnection, DaemonConnectionError,
    DeviceAuthorization, DeviceTokenPoll, LocalDomainOwner, LocalDomains, NewPublicUrl, Plan,
    PlanAddons, PlanCounts, PlanInterval, PlanStatus, Protection, ProtectionChoice, ProtectionMode,
    PublicTunnel, PublicUrl, PublicUrlPasswords, PublicUrlState, PublicUrlWithPassword,
    RuntimeBlock, RuntimeState, SavedPassword, SavedPasswords, ServingMachine, SignInRequest,
    SlugSuggestion, TunnelState, TunnelStatus, UpgradeHint,
};
use crate::application::public_urls::PublicUrls;
use crate::config::LocalifyPaths;
use crate::domain::{DomainName, PublicSlug, RegistrationSource};
use crate::infrastructure::cloud::api::{
    self, CloudClient, CreatePublicUrlRequest, ProtectionRequest,
};
use crate::infrastructure::cloud::credentials::{CloudCredentials, CredentialsStore};
use crate::infrastructure::cloud::http::{self, CloudError, CloudHttp, block_on};
use crate::infrastructure::cloud::passwords::{PasswordStore, StoredPassword};
use crate::infrastructure::cloud::types;
use crate::infrastructure::config::{ConfigStore, ConfiguredDomain};
use crate::infrastructure::mgmt_client::MgmtSocketClient;

/// Everything the Localify Cloud use cases need on this machine.
pub struct CloudAdapters {
    pub api: HttpCloudApi,
    pub credentials: CredentialsStore,
    pub passwords: PasswordStore,
    pub daemon: MgmtSocketClient,
    pub domains: ConfigLocalDomains,
}

impl CloudAdapters {
    pub fn new(config_path: &Path, paths: &LocalifyPaths) -> Self {
        Self {
            api: HttpCloudApi,
            credentials: CredentialsStore::in_data_dir(&paths.data_dir),
            passwords: PasswordStore::in_data_dir(&paths.data_dir),
            daemon: MgmtSocketClient::new(&paths.socket_path),
            domains: ConfigLocalDomains {
                config: ConfigStore::new(config_path.to_path_buf()),
                daemon: MgmtSocketClient::new(&paths.socket_path),
            },
        }
    }

    pub fn accounts(&self) -> CloudAccounts<'_> {
        CloudAccounts::new(&self.api, &self.credentials, &self.passwords, &self.daemon)
    }

    pub fn public_urls(&self) -> PublicUrls<'_> {
        PublicUrls::new(
            &self.api,
            &self.credentials,
            &self.passwords,
            &self.daemon,
            &self.domains,
        )
    }

    pub fn billing(&self) -> CloudBilling<'_> {
        CloudBilling::new(&self.api, &self.credentials, &self.daemon)
    }
}

/// The Localify Cloud API over HTTPS. Each call runs on its own small
/// runtime, because the CLI and `DesktopApi` are synchronous.
pub struct HttpCloudApi;

impl HttpCloudApi {
    fn call<T, F, Fut>(api_base: &str, token: Option<&str>, call: F) -> Result<T, CloudApiError>
    where
        F: FnOnce(CloudClient) -> Fut,
        Fut: Future<Output = Result<T, CloudError>>,
    {
        let http = CloudHttp::new(api_base, token.map(str::to_string)).map_err(api_error)?;
        block_on(call(CloudClient::new(http)))
            .map_err(|error| CloudApiError::Failed(format!("{error:#}")))?
            .map_err(api_error)
    }

    fn signed_in<T, F, Fut>(account: &CloudAccount, call: F) -> Result<T, CloudApiError>
    where
        F: FnOnce(CloudClient) -> Fut,
        Fut: Future<Output = Result<T, CloudError>>,
    {
        Self::call(&account.api_base, Some(&account.token), call)
    }
}

impl CloudApi for HttpCloudApi {
    fn start_sign_in(&self) -> Result<DeviceAuthorization, CloudApiError> {
        let code = Self::call(&http::api_base(), None, |client| async move {
            client.start_device_flow().await
        })?;
        Ok(DeviceAuthorization {
            device_code: code.device_code,
            user_code: code.user_code,
            verification_uri: code.verification_uri,
            interval: code.interval,
            expires_in: code.expires_in,
        })
    }

    fn poll_sign_in(&self, request: &SignInRequest) -> Result<DeviceTokenPoll, CloudApiError> {
        let api_base = http::api_base();
        let result = Self::call(&api_base, None, |client| async move {
            Ok::<_, CloudError>(
                client
                    .poll_device_token(
                        &request.device_code,
                        &request.machine_name,
                        request.ingress_port,
                    )
                    .await,
            )
        })?;
        let error = match result {
            Ok(token) => {
                return Ok(DeviceTokenPoll::Granted(CloudAccount {
                    api_base,
                    token: token.token,
                    login: token.user.login,
                    machine_id: token.machine.id,
                    machine_name: token.machine.name,
                }));
            }
            Err(error) => error,
        };
        match error {
            CloudError::Api {
                code,
                message,
                interval,
                ..
            } if is_device_flow_state(&code) => Ok(match code.as_str() {
                "authorization_pending" => DeviceTokenPoll::Pending,
                "slow_down" => DeviceTokenPoll::SlowDown { interval },
                "access_denied" => DeviceTokenPoll::Denied(message),
                "not_allowed" => DeviceTokenPoll::NotAllowed(message),
                _ => DeviceTokenPoll::Expired(message),
            }),
            error => Err(api_error(error)),
        }
    }

    fn sign_out(&self, account: &CloudAccount) -> Result<(), CloudApiError> {
        Self::signed_in(account, |client| async move { client.logout().await })
    }

    fn plan(&self, account: &CloudAccount) -> Result<Option<Plan>, CloudApiError> {
        let me = Self::signed_in(account, |client| async move { client.me().await })?;
        me.plan.map(plan).transpose()
    }

    fn checkout(
        &self,
        account: &CloudAccount,
        product: CheckoutProduct,
    ) -> Result<BillingPage, CloudApiError> {
        let (product, units) = match product {
            CheckoutProduct::Pro(PlanInterval::Month) => ("pro_monthly", None),
            CheckoutProduct::Pro(PlanInterval::Year) => ("pro_yearly", None),
            CheckoutProduct::Addon(Addon::Machine, units) => ("addon_machine", Some(units.value())),
            CheckoutProduct::Addon(Addon::Urls, units) => ("addon_urls", Some(units.value())),
        };
        let page = Self::signed_in(account, |client| async move {
            client.checkout(product, units).await
        })?;
        let kind = match page.kind.as_str() {
            "checkout" => BillingPageKind::Checkout,
            "portal" => BillingPageKind::Portal,
            other => return Err(unreadable(&format!("a billing page of kind '{other}'"))),
        };
        Ok(BillingPage {
            kind,
            url: https_url(page.url)?,
        })
    }

    fn billing_portal(&self, account: &CloudAccount) -> Result<String, CloudApiError> {
        Self::signed_in(
            account,
            |client| async move { client.billing_portal().await },
        )
        .and_then(https_url)
    }

    fn addon_prices(&self, account: &CloudAccount) -> Result<AddonPrices, CloudApiError> {
        let pricing = Self::call(&account.api_base, None, |client| async move {
            client.pricing().await
        })?;
        let addons = pricing.addons;
        if addons.machine.currency != addons.urls.currency {
            return Err(unreadable("add-on prices in two currencies"));
        }
        Ok(AddonPrices {
            currency: currency(&addons.machine.currency)?,
            machine: addons.machine.amount,
            url_pack: addons.urls.amount,
        })
    }

    fn suggest_slug(
        &self,
        account: &CloudAccount,
        local_domain: &DomainName,
    ) -> Result<SlugSuggestion, CloudApiError> {
        Self::signed_in(account, |client| async move {
            client.suggest_slug(local_domain.as_str()).await
        })
        .map(|suggestion| SlugSuggestion {
            slug: suggestion.slug,
            url: suggestion.url,
        })
    }

    fn slug_available(
        &self,
        account: &CloudAccount,
        slug: &PublicSlug,
    ) -> Result<bool, CloudApiError> {
        Self::signed_in(account, |client| async move {
            client.slug_availability(slug.as_str()).await
        })
        .map(|answer| answer.available)
    }

    fn list_public_urls(&self, account: &CloudAccount) -> Result<Vec<PublicUrl>, CloudApiError> {
        let urls = Self::signed_in(
            account,
            |client| async move { client.list_public_urls().await },
        )?;
        Ok(urls.into_iter().map(public_url).collect())
    }

    fn create_public_url(
        &self,
        account: &CloudAccount,
        request: &NewPublicUrl,
    ) -> Result<PublicUrlWithPassword, CloudApiError> {
        let protection = protection_request(&request.protection);
        let created = Self::signed_in(account, |client| async move {
            client
                .create_public_url(&CreatePublicUrlRequest {
                    local_domain: request.local_domain.as_str(),
                    wildcard: request.wildcard,
                    slug: request.slug.as_ref().map(|slug| slug.as_str()),
                    protection: &protection,
                })
                .await
        })?;
        Ok(PublicUrlWithPassword {
            public_url: public_url(created.public_url),
            password: created.password,
        })
    }

    fn remove_public_url(&self, account: &CloudAccount, id: &str) -> Result<(), CloudApiError> {
        Self::signed_in(account, |client| async move {
            client.delete_public_url(id).await
        })
    }

    fn set_protection(
        &self,
        account: &CloudAccount,
        id: &str,
        protection: &ProtectionChoice,
    ) -> Result<PublicUrlWithPassword, CloudApiError> {
        let protection = protection_request(protection);
        let changed = Self::signed_in(account, |client| async move {
            client.set_protection(id, &protection).await
        })?;
        Ok(PublicUrlWithPassword {
            public_url: public_url(changed.public_url),
            password: changed.password,
        })
    }

    fn claim_public_url(
        &self,
        account: &CloudAccount,
        id: &str,
    ) -> Result<PublicUrl, CloudApiError> {
        Self::signed_in(account, |client| async move { client.claim(id).await }).map(public_url)
    }
}

/// The answers of the device token route that are a state of the sign-in
/// rather than a failure.
fn is_device_flow_state(code: &str) -> bool {
    matches!(
        code,
        "authorization_pending" | "slow_down" | "access_denied" | "not_allowed" | "expired_token"
    )
}

fn api_error(error: CloudError) -> CloudApiError {
    match error {
        CloudError::Api {
            status,
            code,
            message,
            upgrade,
            ..
        } => CloudApiError::Rejected {
            status,
            code,
            message,
            upgrade: upgrade.map(upgrade_hint),
        },
        error @ CloudError::Unreachable { .. } => CloudApiError::Unreachable(error.to_string()),
        error @ CloudError::InvalidResponse(_) => CloudApiError::Failed(error.to_string()),
    }
}

fn upgrade_hint(upgrade: types::Upgrade) -> UpgradeHint {
    UpgradeHint {
        kind: upgrade.kind,
        command: upgrade.command,
    }
}

fn currency(code: &str) -> Result<Currency, CloudApiError> {
    match code {
        "USD" => Ok(Currency::Usd),
        "EUR" => Ok(Currency::Eur),
        other => Err(unreadable(&format!("the currency '{other}'"))),
    }
}

/// What a person reads when the cloud speaks a newer contract than this
/// build.
fn unreadable(what: &str) -> CloudApiError {
    CloudApiError::Failed(
        CloudError::InvalidResponse(format!("{what} is unknown to this version")).to_string(),
    )
}

/// A billing page opens in a browser: anything but an `https://` URL could
/// be a file or an app, so it is refused.
fn https_url(url: String) -> Result<String, CloudApiError> {
    if url.starts_with("https://") {
        Ok(url)
    } else {
        Err(unreadable("a billing page that is not an https:// URL"))
    }
}

fn plan(plan: api::Plan) -> Result<Plan, CloudApiError> {
    let status = match plan.status.as_str() {
        "none" => PlanStatus::None,
        "trialing" => PlanStatus::Trialing,
        "active" => PlanStatus::Active,
        "past_due" => PlanStatus::PastDue,
        "canceling" => PlanStatus::Canceling,
        "override" => PlanStatus::Override,
        other => return Err(unreadable(&format!("the plan status '{other}'"))),
    };
    let interval = match plan.interval.as_deref() {
        None => None,
        Some("month") => Some(PlanInterval::Month),
        Some("year") => Some(PlanInterval::Year),
        Some(other) => return Err(unreadable(&format!("the plan interval '{other}'"))),
    };
    let counts = |counts: api::PlanCounts| PlanCounts {
        serving_machines: counts.serving_machines,
        public_urls: counts.public_urls,
    };
    Ok(Plan {
        status,
        entitled: plan.entitled,
        interval,
        trial_ends_at: plan.trial_ends_at,
        renews_at: plan.renews_at,
        ends_at: plan.ends_at,
        trial_available: plan.trial_available,
        limits: counts(plan.limits),
        usage: counts(plan.usage),
        addons: PlanAddons {
            machines: plan.addons.machines,
            url_packs: plan.addons.url_packs,
        },
    })
}

fn protection_request(choice: &ProtectionChoice) -> ProtectionRequest {
    match choice {
        ProtectionChoice::Password(password) => ProtectionRequest::Password(password.clone()),
        ProtectionChoice::Public => ProtectionRequest::Public,
    }
}

fn public_url(url: types::PublicUrl) -> PublicUrl {
    PublicUrl {
        id: url.id,
        slug: url.slug,
        url: url.url,
        local_domain: url.local_domain,
        wildcard: url.wildcard,
        machine: url.machine.map(|machine| ServingMachine {
            id: machine.id,
            name: machine.name,
        }),
        protection: Protection {
            mode: match url.protection.mode {
                types::ProtectionMode::Password => ProtectionMode::Password,
                types::ProtectionMode::Public => ProtectionMode::Public,
            },
            version: url.protection.version,
        },
        state: url.state.map(|state| match state {
            types::PublicUrlState::Serving => PublicUrlState::Serving,
            types::PublicUrlState::Suspended => PublicUrlState::Suspended,
            types::PublicUrlState::TakenDown => PublicUrlState::TakenDown,
        }),
        created_at: url.created_at,
    }
}

impl CloudAccountStore for CredentialsStore {
    fn load(&self) -> Result<Option<CloudAccount>> {
        Ok(
            CredentialsStore::load(self)?.map(|credentials| CloudAccount {
                api_base: credentials.api_base,
                token: credentials.token,
                login: credentials.login,
                machine_id: credentials.machine_id,
                machine_name: credentials.machine_name,
            }),
        )
    }

    fn save(&self, account: &CloudAccount) -> Result<()> {
        CredentialsStore::save(
            self,
            &CloudCredentials {
                api_base: account.api_base.clone(),
                token: account.token.clone(),
                login: account.login.clone(),
                machine_id: account.machine_id.clone(),
                machine_name: account.machine_name.clone(),
            },
        )
    }

    fn delete(&self) -> Result<()> {
        CredentialsStore::delete(self)
    }
}

impl PublicUrlPasswords for PasswordStore {
    fn load(&self) -> Result<SavedPasswords> {
        Ok(saved_passwords(PasswordStore::load(self)?))
    }

    fn update(&self, change: &mut dyn FnMut(&mut SavedPasswords)) -> Result<SavedPasswords> {
        let stored = PasswordStore::update(self, |stored| {
            let mut saved = saved_passwords(std::mem::take(stored));
            change(&mut saved);
            *stored = saved
                .into_iter()
                .map(|(id, saved)| {
                    let stored = StoredPassword {
                        version: saved.version,
                        password: saved.password,
                    };
                    (id, stored)
                })
                .collect();
        })?;
        Ok(saved_passwords(stored))
    }
}

fn saved_passwords(stored: BTreeMap<String, StoredPassword>) -> SavedPasswords {
    stored
        .into_iter()
        .map(|(id, stored)| {
            let saved = SavedPassword {
                version: stored.version,
                password: stored.password,
            };
            (id, saved)
        })
        .collect()
}

impl PublicTunnel for MgmtSocketClient {
    fn status(&self) -> Result<TunnelStatus, DaemonConnectionError> {
        let status = self.public_status()?;
        Ok(TunnelStatus {
            state: match status.tunnel {
                types::TunnelState::Stopped => TunnelState::Stopped,
                types::TunnelState::Starting => TunnelState::Starting,
                types::TunnelState::Connected => TunnelState::Connected,
                types::TunnelState::Error => TunnelState::Error,
            },
            connections: status.connections,
            error: status.error,
            runtime: status.runtime.map(|runtime| match runtime {
                types::RuntimeStatus::Idle => RuntimeState::Idle,
                types::RuntimeStatus::Provisioning => RuntimeState::Provisioning,
                types::RuntimeStatus::Ready => RuntimeState::Ready,
                types::RuntimeStatus::Blocked => RuntimeState::Blocked,
                types::RuntimeStatus::Banned => RuntimeState::Banned,
            }),
            blocked: status.blocked.map(|blocked| RuntimeBlock {
                reason: blocked.reason,
                message: blocked.message,
                upgrade: blocked.upgrade.map(upgrade_hint),
            }),
        })
    }

    fn refresh(&self) -> Result<(), DaemonConnectionError> {
        self.public_refresh()
    }
}

/// The local domains in the config, and those Docker and other providers
/// registered with the running daemon.
pub struct ConfigLocalDomains {
    config: ConfigStore,
    daemon: MgmtSocketClient,
}

impl LocalDomains for ConfigLocalDomains {
    fn owner(&self, domain: &DomainName) -> Result<LocalDomainOwner> {
        let config = self.config.load()?;
        let registered =
            |found: &ConfiguredDomain| found.registration.pattern().base_domain() == domain;
        if let Some(project_id) = config.public_url_project(domain.as_str()) {
            let name = config
                .configured_projects()
                .into_iter()
                .find(|project| project.id == project_id)
                .map_or(project_id, |project| project.name);
            return Ok(if config.configured_domains().iter().any(registered) {
                LocalDomainOwner::Project(name)
            } else {
                LocalDomainOwner::ProjectNamespace(name)
            });
        }
        if config.configured_domains().iter().any(registered) {
            return Ok(LocalDomainOwner::NoProject);
        }
        // Docker and other providers register with the daemon only. A daemon
        // that is not running cannot tell, and serves nothing anyway.
        let external = self.daemon.list_registrations().is_ok_and(|registrations| {
            registrations.iter().any(|registration| {
                registration.source() == RegistrationSource::External
                    && registration.pattern().base_domain() == domain
            })
        });
        Ok(if external {
            LocalDomainOwner::External
        } else {
            LocalDomainOwner::NotRegistered
        })
    }

    /// Only the domains of the URL's own project: the daemon serves no other.
    fn labels_under(&self, domain: &DomainName) -> Result<Vec<String>> {
        let config = self.config.load()?;
        let Some(project_id) = config.public_url_project(domain.as_str()) else {
            return Ok(Vec::new());
        };
        let mut labels = config
            .configured_domains()
            .iter()
            .filter(|found| found.project_id.as_ref() == Some(&project_id))
            .filter_map(|found| label_under(found.registration.pattern().base_domain(), domain))
            .collect::<Vec<_>>();
        labels.sort();
        labels.dedup();
        Ok(labels)
    }
}

/// `web--feat` for `web--feat.journal.localify` under `journal.localify`.
fn label_under(name: &DomainName, parent: &DomainName) -> Option<String> {
    name.as_str()
        .strip_suffix(parent.as_str())?
        .strip_suffix('.')
        .filter(|label| !label.is_empty())
        .map(str::to_string)
}

/// The name this machine shows on Localify Cloud: the computer's name on
/// macOS, as in System Settings, and the host name elsewhere.
pub fn computer_name() -> String {
    let name = macos_computer_name()
        .or_else(|| {
            std::fs::read_to_string("/proc/sys/kernel/hostname")
                .ok()
                .and_then(non_empty)
        })
        .or_else(|| command_output("hostname", &[]))
        .unwrap_or_else(|| "Unnamed machine".to_string());
    machine_name(&name, crate::branding::BRANDING.is_dev_profile())
}

/// The API's limit, in UTF-16 units as JavaScript counts them.
const MAX_MACHINE_NAME: usize = 64;

/// Signing in with a name already in use replaces that machine, so the dev
/// profile, which runs next to the installed app, gets a name of its own.
fn machine_name(computer: &str, dev_profile: bool) -> String {
    let suffix = if dev_profile { " (dev)" } else { "" };
    let mut kept = String::new();
    for c in computer.trim().chars() {
        if kept.encode_utf16().count() + c.len_utf16() + suffix.len() > MAX_MACHINE_NAME {
            break;
        }
        kept.push(c);
    }
    format!("{}{suffix}", kept.trim_end())
}

#[cfg(target_os = "macos")]
fn macos_computer_name() -> Option<String> {
    command_output("scutil", &["--get", "ComputerName"])
}

#[cfg(not(target_os = "macos"))]
fn macos_computer_name() -> Option<String> {
    None
}

fn command_output(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    non_empty(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn non_empty(value: String) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_dev_profile_signs_in_as_a_machine_of_its_own() {
        assert_eq!(
            machine_name("Téo's MacBook Pro", false),
            "Téo's MacBook Pro"
        );
        assert_eq!(
            machine_name("Téo's MacBook Pro", true),
            "Téo's MacBook Pro (dev)"
        );
        let long = machine_name(&"m".repeat(80), true);
        assert_eq!(long.encode_utf16().count(), MAX_MACHINE_NAME);
        assert!(long.ends_with(" (dev)"));
    }

    use super::*;
    use crate::application::ports::CloudAccountStore;

    #[test]
    fn credentials_round_trip_through_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = CredentialsStore::in_data_dir(dir.path());
        let account = CloudAccount {
            api_base: "https://api.localify.dev".into(),
            token: "lfy_t".into(),
            login: "teo".into(),
            machine_id: "m1".into(),
            machine_name: "laptop".into(),
        };

        CloudAccountStore::save(&store, &account).unwrap();
        assert_eq!(CloudAccountStore::load(&store).unwrap(), Some(account));
        CloudAccountStore::delete(&store).unwrap();
        assert_eq!(CloudAccountStore::load(&store).unwrap(), None);
    }

    #[test]
    fn saved_passwords_round_trip_through_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = PasswordStore::in_data_dir(dir.path());
        let saved = SavedPasswords::from([(
            "u1".to_string(),
            SavedPassword {
                version: 3,
                password: "open sesame".into(),
            },
        )]);

        let updated = PublicUrlPasswords::update(&store, &mut |all| all.extend(saved.clone()));
        assert_eq!(updated.unwrap(), saved);
        assert_eq!(PublicUrlPasswords::load(&store).unwrap(), saved);
        PublicUrlPasswords::update(&store, &mut |all| all.clear()).unwrap();
        assert!(PublicUrlPasswords::load(&store).unwrap().is_empty());
    }

    #[test]
    fn labels_are_whatever_sits_before_the_parent() {
        let parent = DomainName::new("journal.localify").unwrap();
        let label = |name: &str| label_under(&DomainName::new(name).unwrap(), &parent);

        assert_eq!(
            label("web--feat.journal.localify").as_deref(),
            Some("web--feat")
        );
        assert_eq!(label("a.b.journal.localify").as_deref(), Some("a.b"));
        assert_eq!(label("journal.localify"), None);
        assert_eq!(label("myjournal.localify"), None);
    }

    #[test]
    fn project_domains_and_the_project_namespace_are_exposable() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        let config = ConfigStore::new(config_path.clone());
        let project = config.create_project("Journal").unwrap();
        config
            .add_domain_with_project(
                crate::application::testkit::registration("web.journal.localify"),
                Some(project.id.clone()),
            )
            .unwrap();
        config
            .add_domain(crate::application::testkit::registration("loose.localify"))
            .unwrap();
        config
            .add_domain_with_project(
                crate::application::testkit::registration("web.shop.localify"),
                Some(project.id.clone()),
            )
            .unwrap();
        let domains = ConfigLocalDomains {
            config,
            daemon: MgmtSocketClient::new(&dir.path().join("missing.sock")),
        };
        let owner = |name: &str| {
            let name = name.replace(".localify", crate::branding::BRANDING.domain_suffix());
            domains.owner(&DomainName::new(name).unwrap()).unwrap()
        };

        assert_eq!(
            owner("web.journal.localify"),
            LocalDomainOwner::Project("Journal".into())
        );
        assert_eq!(
            owner("journal.localify"),
            LocalDomainOwner::ProjectNamespace("Journal".into())
        );
        assert_eq!(owner("loose.localify"), LocalDomainOwner::NoProject);
        // A parent of project domains is not a namespace unless it is the project's id.
        assert_eq!(owner("shop.localify"), LocalDomainOwner::NotRegistered);
        assert_eq!(owner("docker.localify"), LocalDomainOwner::NotRegistered);
    }
}
