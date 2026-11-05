use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;

use super::cloud_account::{TunnelRefresh, refresh_tunnel};
use super::ports::{
    CloudAccount, CloudAccountStore, CloudApi, CloudApiError, LocalDomainOwner, LocalDomains,
    NewPublicUrl, ProtectionChoice, PublicTunnel, PublicUrl, PublicUrlWithPassword,
};
use crate::branding::BRANDING;
use crate::domain::{DomainName, PublicSlug};

/// A public URL as this machine sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicUrlView {
    #[serde(flatten)]
    pub url: PublicUrl,
    /// This machine serves it.
    pub served_here: bool,
}

/// A public URL after a change of password. Localify Cloud shows a password
/// it generated this once only.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicUrlAndPassword {
    pub public_url: PublicUrlView,
    pub password: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedPublicUrl {
    pub public_url: PublicUrlView,
    pub password: Option<String>,
    /// Why some local hosts of the URL cannot be served, one sentence each:
    /// a namespace that nothing registers, and environments whose public
    /// name would be too long.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unservable_hosts: Vec<String>,
}

/// The result of a change, and whether the daemon took it.
pub struct Changed<T> {
    pub value: T,
    pub refresh: TunnelRefresh,
}

#[derive(Clone)]
pub struct CreatePublicUrl {
    pub local_domain: String,
    pub wildcard: bool,
    pub slug: Option<String>,
    pub protection: ProtectionChoice,
}

/// Use case: the public URLs of the signed-in user (`docs/public-urls.md`).
/// After every change the daemon fetches this machine's URLs again.
pub struct PublicUrls<'a> {
    api: &'a dyn CloudApi,
    store: &'a dyn CloudAccountStore,
    tunnel: &'a dyn PublicTunnel,
    domains: &'a dyn LocalDomains,
}

impl<'a> PublicUrls<'a> {
    pub fn new(
        api: &'a dyn CloudApi,
        store: &'a dyn CloudAccountStore,
        tunnel: &'a dyn PublicTunnel,
        domains: &'a dyn LocalDomains,
    ) -> Self {
        Self {
            api,
            store,
            tunnel,
            domains,
        }
    }

    pub fn list(&self) -> Result<Vec<PublicUrlView>> {
        let account = self.account()?;
        let urls = self
            .api
            .list_public_urls(&account)
            .map_err(with_next_step)?;
        Ok(urls.into_iter().map(|url| view(url, &account)).collect())
    }

    /// Finds a URL by its id, its slug, its URL, or the host of one of its
    /// environments, as a person may paste any of them.
    pub fn find(&self, reference: &str) -> Result<PublicUrlView> {
        let account = self.account()?;
        let urls = self
            .api
            .list_public_urls(&account)
            .map_err(with_next_step)?;
        let url = find_public_url(&urls, reference).ok_or_else(|| {
            anyhow!(
                "No public URL matches '{}'. List them with: {} public-url list",
                reference.trim(),
                BRANDING.cli_command()
            )
        })?;
        Ok(view(url.clone(), &account))
    }

    /// Gives an exposable local domain of this machine a public URL, served
    /// here: a project's domain, or its namespace (`docs/public-urls.md`,
    /// "Exposable local domain"). Anything else, Docker's domains included,
    /// is refused before Localify Cloud is called: the daemon would not serve
    /// it.
    pub fn create(&self, request: CreatePublicUrl) -> Result<Changed<CreatedPublicUrl>> {
        let account = self.account()?;
        let local_domain = DomainName::new(&request.local_domain).map_err(|error| {
            anyhow!(
                "'{}' is not a local domain: {error}",
                request.local_domain.trim()
            )
        })?;
        let slug = request.slug.as_deref().map(PublicSlug::new).transpose()?;
        check_password(&request.protection)?;
        let owner = self.check_owner(&local_domain)?;

        let created = self
            .api
            .create_public_url(
                &account,
                &NewPublicUrl {
                    local_domain: local_domain.clone(),
                    wildcard: request.wildcard,
                    slug,
                    protection: request.protection,
                },
            )
            .map_err(with_next_step)?;
        let mut unservable_hosts = Vec::new();
        if let LocalDomainOwner::ProjectNamespace(project) = &owner {
            unservable_hosts.push(format!(
                "{local_domain} itself cannot be served: no service of {project} uses it as its domain.{}",
                if created.public_url.wildcard {
                    ""
                } else {
                    " Only a wildcard URL reaches the services under it."
                }
            ));
        }
        if created.public_url.wildcard {
            // The URL exists by now: a failed check must not hide it.
            unservable_hosts.extend(
                self.unservable_hosts(&local_domain, &created.public_url.slug)
                    .unwrap_or_default(),
            );
        }
        Ok(Changed {
            value: CreatedPublicUrl {
                public_url: view(created.public_url, &account),
                password: created.password,
                unservable_hosts,
            },
            refresh: refresh_tunnel(self.tunnel),
        })
    }

    pub fn remove(&self, id: &str) -> Result<Changed<()>> {
        let account = self.account()?;
        self.api
            .remove_public_url(&account, required_id(id)?)
            .map_err(with_next_step)?;
        Ok(self.changed(()))
    }

    /// Sets, rotates or removes the password. Either way, every browser that
    /// signed in has to again.
    pub fn set_protection(
        &self,
        id: &str,
        protection: ProtectionChoice,
    ) -> Result<Changed<PublicUrlAndPassword>> {
        let account = self.account()?;
        check_password(&protection)?;
        let PublicUrlWithPassword {
            public_url,
            password,
        } = self
            .api
            .set_protection(&account, required_id(id)?, &protection)
            .map_err(with_next_step)?;
        Ok(self.changed(PublicUrlAndPassword {
            public_url: view(public_url, &account),
            password,
        }))
    }

    /// Moves a URL to this machine, such as after a reinstall.
    pub fn claim(&self, id: &str) -> Result<Changed<PublicUrlView>> {
        let account = self.account()?;
        let url = self
            .api
            .claim_public_url(&account, required_id(id)?)
            .map_err(with_next_step)?;
        Ok(self.changed(view(url, &account)))
    }

    fn account(&self) -> Result<CloudAccount> {
        self.store.load()?.with_context(|| {
            format!(
                "This machine is not signed in to Localify Cloud. Sign in with: {} login",
                BRANDING.cli_command()
            )
        })
    }

    fn changed<T>(&self, value: T) -> Changed<T> {
        Changed {
            value,
            refresh: refresh_tunnel(self.tunnel),
        }
    }

    /// Who the domain belongs to, when it is exposable.
    fn check_owner(&self, domain: &DomainName) -> Result<LocalDomainOwner> {
        let cli = BRANDING.cli_command();
        match self.domains.owner(domain)? {
            owner @ (LocalDomainOwner::Project(_) | LocalDomainOwner::ProjectNamespace(_)) => {
                Ok(owner)
            }
            LocalDomainOwner::NoProject => bail!(
                "{domain} belongs to no project, and public URLs serve only project domains. Attach it first: {cli} project domain attach {domain} --project <project>"
            ),
            LocalDomainOwner::External => bail!(
                "{domain} comes from Docker or another provider, not from a project, and public URLs serve only project domains. Give the app a project domain instead."
            ),
            LocalDomainOwner::NotRegistered => bail!(
                "{domain} is neither a domain of a project on this machine nor a project's namespace (<project-id>{}). Check the name with: {cli} list-projects",
                BRANDING.domain_suffix()
            ),
        }
    }

    /// The local hosts under a wildcard URL whose public name would not fit
    /// in a DNS label. They are reported, never truncated.
    fn unservable_hosts(&self, domain: &DomainName, slug: &str) -> Result<Vec<String>> {
        let slug = PublicSlug::new(slug)?;
        Ok(self
            .domains
            .labels_under(domain)?
            .into_iter()
            .filter_map(|label| {
                slug.environment_label(&label)
                    .err()
                    .map(|error| format!("{label}.{domain} cannot be served: {error}"))
            })
            .collect())
    }
}

fn view(url: PublicUrl, account: &CloudAccount) -> PublicUrlView {
    let served_here = url
        .machine
        .as_ref()
        .is_some_and(|machine| machine.id == account.machine_id);
    PublicUrlView { url, served_here }
}

/// Adds what to do next to the API's sentence, for the errors a person can
/// act on.
fn with_next_step(error: CloudApiError) -> anyhow::Error {
    let cli = BRANDING.cli_command();
    let CloudApiError::Rejected { code, message, .. } = &error else {
        return error.into();
    };
    match code.as_str() {
        "not_found" => anyhow!("{message} List your public URLs with: {cli} public-url list"),
        "local_domain_taken" => {
            anyhow!("{message} Find the URL it already has with: {cli} public-url list")
        }
        "invalid_local_domain" => anyhow!("{message} Check the name with: {cli} list"),
        _ => error.into(),
    }
}

fn required_id(id: &str) -> Result<&str> {
    let id = id.trim();
    if id.is_empty() {
        bail!("Name the public URL by its id, slug or URL.");
    }
    Ok(id)
}

fn check_password(protection: &ProtectionChoice) -> Result<()> {
    if let ProtectionChoice::Password(Some(password)) = protection
        && password.trim().is_empty()
    {
        bail!("The password cannot be empty. Leave it out to have one generated.");
    }
    Ok(())
}

/// Matches an id, a slug, a public URL or host, or the host of one
/// environment of a wildcard URL (`<slug>--<label>.<domain>`).
pub fn find_public_url<'u>(urls: &'u [PublicUrl], reference: &str) -> Option<&'u PublicUrl> {
    let reference = reference.trim();
    if reference.is_empty() {
        return None;
    }
    if let Some(url) = urls.iter().find(|url| url.id == reference) {
        return Some(url);
    }
    let host = host_of(reference);
    urls.iter().find(|url| {
        let url_host = host_of(&url.url);
        url.slug == host || url_host == host || (url.wildcard && is_environment_of(&host, url))
    })
}

fn is_environment_of(host: &str, url: &PublicUrl) -> bool {
    let Some((first_label, parent)) = host.split_once('.') else {
        return false;
    };
    let Some((slug, label)) = first_label.split_once("--") else {
        return false;
    };
    let url_host = host_of(&url.url);
    slug == url.slug
        && !label.is_empty()
        && url_host.split_once('.').map(|(_, p)| p) == Some(parent)
}

/// `journal-a8f3.localify.fyi` from `https://Journal-a8f3.localify.fyi/path`.
fn host_of(value: &str) -> String {
    let value = value.trim();
    let without_scheme = value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"))
        .unwrap_or(value);
    without_scheme
        .split(['/', '?', '#', ':'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::ProtectionMode;
    use crate::application::testkit::*;

    fn create(domain: &str, wildcard: bool) -> CreatePublicUrl {
        CreatePublicUrl {
            local_domain: domain.into(),
            wildcard,
            slug: None,
            protection: ProtectionChoice::Password(None),
        }
    }

    struct World {
        api: InMemoryCloudApi,
        store: InMemoryCloudAccountStore,
        tunnel: InMemoryPublicTunnel,
        domains: InMemoryLocalDomains,
    }

    impl World {
        fn new() -> Self {
            Self {
                api: InMemoryCloudApi::new(),
                store: InMemoryCloudAccountStore::signed_in(),
                tunnel: InMemoryPublicTunnel::running(),
                domains: InMemoryLocalDomains::new().with_owner(
                    "journal.localify",
                    LocalDomainOwner::Project("Journal".into()),
                ),
            }
        }

        fn urls(&self) -> PublicUrls<'_> {
            PublicUrls::new(&self.api, &self.store, &self.tunnel, &self.domains)
        }
    }

    #[test]
    fn a_signed_out_machine_is_told_to_sign_in() {
        let mut world = World::new();
        world.store = InMemoryCloudAccountStore::signed_out();

        let error = world.urls().list().unwrap_err().to_string();

        assert!(error.contains("login"), "{error}");
    }

    #[test]
    fn creating_a_url_for_a_project_domain_returns_the_password_and_tells_the_daemon() {
        let world = World::new();

        let changed = world
            .urls()
            .create(create("Journal.localify", false))
            .unwrap();

        assert_eq!(
            changed.value.public_url.url.local_domain,
            "journal.localify"
        );
        assert!(changed.value.public_url.served_here);
        assert_eq!(
            changed.value.password.as_deref(),
            Some("four-random-words-here")
        );
        assert_eq!(changed.refresh, TunnelRefresh::Done);
        assert_eq!(world.tunnel.refreshes(), 1);
    }

    #[test]
    fn only_exposable_domains_get_a_public_url() {
        let world = World {
            domains: InMemoryLocalDomains::new()
                .with_owner("db.localify", LocalDomainOwner::External)
                .with_owner("loose.localify", LocalDomainOwner::NoProject),
            ..World::new()
        };
        let urls = world.urls();

        for (domain, expected) in [
            ("db.localify", "Docker"),
            ("loose.localify", "project domain attach"),
            ("missing.localify", "nor a project's namespace"),
        ] {
            let error = urls.create(create(domain, true)).err().unwrap().to_string();
            assert!(error.contains(expected), "{domain}: {error}");
        }
        assert!(world.api.public_urls().is_empty());
    }

    #[test]
    fn a_project_namespace_is_exposable_even_when_no_service_uses_it() {
        let world = World {
            domains: InMemoryLocalDomains::new().with_owner(
                "shop.localify",
                LocalDomainOwner::ProjectNamespace("Shop".into()),
            ),
            ..World::new()
        };
        let urls = world.urls();

        let wildcard = urls.create(create("shop.localify", true)).unwrap().value;
        assert_eq!(wildcard.unservable_hosts.len(), 1);
        assert!(!wildcard.unservable_hosts[0].contains("Only a wildcard"));

        let plain = urls.create(create("shop.localify", false)).unwrap().value;
        assert!(plain.unservable_hosts[0].contains("Only a wildcard URL reaches"));
    }

    #[test]
    fn a_wildcard_url_reports_environments_too_long_to_serve() {
        // journal-a8f3--<label> passes 63 characters from 50 on.
        let long_label = "a".repeat(50);
        let world = World {
            domains: InMemoryLocalDomains::new()
                .with_owner(
                    "journal.localify",
                    LocalDomainOwner::Project("Journal".into()),
                )
                .with_labels(vec!["web".into(), long_label.clone()]),
            ..World::new()
        };

        let created = world
            .urls()
            .create(create("journal.localify", true))
            .unwrap()
            .value;

        assert_eq!(created.unservable_hosts.len(), 1);
        assert!(created.unservable_hosts[0].starts_with(&format!("{long_label}.journal.localify")));
    }

    #[test]
    fn a_stopped_daemon_does_not_fail_a_change() {
        let world = World {
            tunnel: InMemoryPublicTunnel::stopped(),
            ..World::new()
        };

        let changed = world
            .urls()
            .create(create("journal.localify", false))
            .unwrap();

        assert_eq!(changed.refresh, TunnelRefresh::DaemonStopped);
    }

    #[test]
    fn bad_input_never_reaches_the_api() {
        let world = World::new();
        let urls = world.urls();

        let mut request = create("journal.localify", false);
        request.slug = Some("my--app".into());
        assert!(urls.create(request).is_err());

        let mut request = create("journal.localify", false);
        request.protection = ProtectionChoice::Password(Some("  ".into()));
        assert!(urls.create(request).is_err());

        assert!(
            urls.set_protection("u1", ProtectionChoice::Password(Some(String::new())))
                .is_err()
        );
        assert!(urls.remove(" ").is_err());
        assert!(world.api.calls().is_empty());
    }

    #[test]
    fn protection_can_be_rotated_or_removed() {
        let world = World::new();
        let urls = world.urls();
        let id = urls
            .create(create("journal.localify", false))
            .unwrap()
            .value
            .public_url
            .url
            .id;

        let rotated = urls
            .set_protection(&id, ProtectionChoice::Password(None))
            .unwrap();
        assert_eq!(
            rotated.value.password.as_deref(),
            Some("four-random-words-here")
        );

        let opened = urls.set_protection(&id, ProtectionChoice::Public).unwrap();
        assert_eq!(
            opened.value.public_url.url.protection.mode,
            ProtectionMode::Public
        );
        assert_eq!(opened.value.password, None);
        assert_eq!(world.tunnel.refreshes(), 3);
    }

    #[test]
    fn errors_a_person_can_act_on_say_what_to_do() {
        let world = World {
            api: InMemoryCloudApi::new().with_error(CloudApiError::Rejected {
                status: 404,
                code: "not_found".into(),
                message: "No such public URL.".into(),
            }),
            ..World::new()
        };

        let error = world.urls().claim("u404").err().unwrap().to_string();

        assert!(
            error.starts_with("No such public URL. List your public URLs with:"),
            "{error}"
        );
    }

    #[test]
    fn api_errors_keep_their_sentence() {
        let world = World {
            api: InMemoryCloudApi::new().with_error(CloudApiError::Rejected {
                status: 409,
                code: "slug_taken".into(),
                message: "journal-a8f3 is taken. Choose another slug.".into(),
            }),
            ..World::new()
        };

        let error = world
            .urls()
            .create(create("journal.localify", false))
            .err()
            .unwrap();

        assert_eq!(
            error.to_string(),
            "journal-a8f3 is taken. Choose another slug."
        );
    }

    #[test]
    fn a_url_is_found_by_id_slug_url_or_environment_host() {
        let urls = vec![
            public_url("u1", "journal-a8f3", false),
            public_url("u2", "shop-1b2c", true),
        ];

        let id = |reference: &str| find_public_url(&urls, reference).map(|url| url.id.as_str());
        assert_eq!(id("u1"), Some("u1"));
        assert_eq!(id("journal-a8f3"), Some("u1"));
        assert_eq!(id("https://Journal-a8f3.localify.fyi/"), Some("u1"));
        assert_eq!(id("journal-a8f3.localify.fyi"), Some("u1"));
        assert_eq!(
            id("https://shop-1b2c--web--feat.localify.fyi/cart"),
            Some("u2")
        );
        assert_eq!(id("journal-a8f3--web.localify.fyi"), None);
        assert_eq!(id("unknown"), None);
        assert_eq!(id(" "), None);
    }

    #[test]
    fn a_url_served_elsewhere_is_not_served_here() {
        let world = World::new();
        let mut elsewhere = public_url("u9", "runner-0001", false);
        elsewhere.machine = Some(crate::application::ports::ServingMachine {
            id: "other".into(),
            name: "runner".into(),
        });
        world.api.insert(elsewhere);

        let found = world.urls().find("runner-0001").unwrap();

        assert!(!found.served_here);
        let claimed = world.urls().claim(&found.url.id).unwrap().value;
        assert!(claimed.served_here);
    }
}
