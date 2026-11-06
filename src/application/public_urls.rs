use std::fmt;

use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;
use tracing::warn;

use super::cloud_account::{TunnelRefresh, refresh_tunnel};
use super::ports::{
    CloudAccount, CloudAccountStore, CloudApi, CloudApiError, LocalDomainOwner, LocalDomains,
    NewPublicUrl, ProtectionChoice, ProtectionMode, PublicTunnel, PublicUrl, PublicUrlPasswords,
    PublicUrlWithPassword, SavedPassword, SavedPasswords, SlugSuggestion,
};
use crate::branding::BRANDING;
use crate::domain::{DomainName, PublicSlug};

/// Where the password page of a public URL posts, and where a link with the
/// password lands (`docs/public-urls.md`).
const ACCESS_PATH: &str = "/__localify/access";

/// A public URL as this machine sees it.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicUrlView {
    #[serde(flatten)]
    pub url: PublicUrl,
    /// This machine serves it.
    pub served_here: bool,
    /// The URL with its current password in the fragment, when this machine
    /// set that password: opening it signs the browser in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password_link: Option<String>,
}

/// Leaves the link out: it carries the password.
impl fmt::Debug for PublicUrlView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PublicUrlView")
            .field("url", &self.url)
            .field("served_here", &self.served_here)
            .field(
                "password_link",
                &self.password_link.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
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

/// A slug, and whether the user may take it.
#[allow(dead_code)] // Only the app checks slugs; the CLI target does not build the app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlugCheck {
    pub slug: String,
    pub available: bool,
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
    passwords: &'a dyn PublicUrlPasswords,
    tunnel: &'a dyn PublicTunnel,
    domains: &'a dyn LocalDomains,
}

impl<'a> PublicUrls<'a> {
    pub fn new(
        api: &'a dyn CloudApi,
        store: &'a dyn CloudAccountStore,
        passwords: &'a dyn PublicUrlPasswords,
        tunnel: &'a dyn PublicTunnel,
        domains: &'a dyn LocalDomains,
    ) -> Self {
        Self {
            api,
            store,
            passwords,
            tunnel,
            domains,
        }
    }

    /// Also forgets the saved passwords whose URL changed its protection
    /// since, from another machine. Only a newer version proves that: the
    /// list may be older than a change this machine is saving right now. The
    /// password of a URL removed from another machine stays until sign-out.
    pub fn list(&self) -> Result<Vec<PublicUrlView>> {
        let account = self.account()?;
        let urls = self
            .api
            .list_public_urls(&account)
            .map_err(with_next_step)?;
        let replaced = |id: &String, saved: &SavedPassword| {
            urls.iter().any(|url| {
                url.id == *id
                    && url
                        .protection
                        .version
                        .is_some_and(|version| version > saved.version)
            })
        };
        let mut saved = self.saved_passwords();
        if saved.iter().any(|(id, password)| replaced(id, password)) {
            saved =
                self.change_passwords(|saved| saved.retain(|id, password| !replaced(id, password)));
        }
        Ok(urls
            .into_iter()
            .map(|url| view(url, &account, &saved))
            .collect())
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
        Ok(view(url.clone(), &account, &self.saved_passwords()))
    }

    /// The first free slug that mirrors the local domain's name, as `create`
    /// without a slug would pick it: `louez` for `louez.localify`, or
    /// `louez-<login>` when a teammate has `louez`.
    #[allow(dead_code)] // Only the app calls it.
    pub fn suggest_slug(&self, local_domain: &str) -> Result<SlugSuggestion> {
        let account = self.account()?;
        let local_domain = parse_local_domain(local_domain)?;
        self.api
            .suggest_slug(&account, &local_domain)
            .map_err(with_next_step)
    }

    /// Checks the format here first, so a malformed slug never reaches
    /// Localify Cloud.
    #[allow(dead_code)] // Only the app calls it.
    pub fn check_slug(&self, slug: &str) -> Result<SlugCheck> {
        let slug = PublicSlug::new(slug)?;
        let account = self.account()?;
        let available = self
            .api
            .slug_available(&account, &slug)
            .map_err(with_next_step)?;
        Ok(SlugCheck {
            slug: slug.to_string(),
            available,
        })
    }

    /// Gives an exposable local domain of this machine a public URL, served
    /// here: a project's domain, or its namespace (`docs/public-urls.md`,
    /// "Exposable local domain"). Anything else, Docker's domains included,
    /// is refused before Localify Cloud is called: the daemon would not serve
    /// it.
    pub fn create(&self, request: CreatePublicUrl) -> Result<Changed<CreatedPublicUrl>> {
        let account = self.account()?;
        let local_domain = parse_local_domain(&request.local_domain)?;
        let slug = request.slug.as_deref().map(PublicSlug::new).transpose()?;
        check_password(&request.protection)?;
        let owner = self.check_owner(&local_domain)?;
        let chosen = chosen_password(&request.protection);

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
        let saved = self.remember(
            &created.public_url,
            created.password.as_deref().or(chosen.as_deref()),
        );
        Ok(Changed {
            value: CreatedPublicUrl {
                public_url: view(created.public_url, &account, &saved),
                password: created.password,
                unservable_hosts,
            },
            refresh: refresh_tunnel(self.tunnel),
        })
    }

    pub fn remove(&self, id: &str) -> Result<Changed<()>> {
        let account = self.account()?;
        let id = required_id(id)?;
        self.api
            .remove_public_url(&account, id)
            .map_err(with_next_step)?;
        if self.saved_passwords().contains_key(id) {
            self.change_passwords(|saved| {
                saved.remove(id);
            });
        }
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
        let chosen = chosen_password(&protection);
        let PublicUrlWithPassword {
            public_url,
            password,
        } = self
            .api
            .set_protection(&account, required_id(id)?, &protection)
            .map_err(with_next_step)?;
        let saved = self.remember(&public_url, password.as_deref().or(chosen.as_deref()));
        Ok(self.changed(PublicUrlAndPassword {
            public_url: view(public_url, &account, &saved),
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
        Ok(self.changed(view(url, &account, &self.saved_passwords())))
    }

    fn account(&self) -> Result<CloudAccount> {
        self.store.load()?.with_context(|| {
            format!(
                "This machine is not signed in to Localify Cloud. Sign in with: {} login",
                BRANDING.cli_command()
            )
        })
    }

    /// Saves the password the URL now asks for, or forgets the old one.
    fn remember(&self, url: &PublicUrl, password: Option<&str>) -> SavedPasswords {
        self.change_passwords(|saved| {
            match (url.protection.mode, url.protection.version, password) {
                (ProtectionMode::Password, Some(version), Some(password)) => {
                    let password = SavedPassword {
                        version,
                        password: password.to_string(),
                    };
                    saved.insert(url.id.clone(), password);
                }
                _ => {
                    saved.remove(&url.id);
                }
            }
        })
    }

    /// The URL has changed by then, so a file that cannot be written only
    /// costs the link: this answer still has it, and the log a warning.
    fn change_passwords(&self, change: impl Fn(&mut SavedPasswords)) -> SavedPasswords {
        self.passwords
            .update(&mut |saved| change(saved))
            .unwrap_or_else(|error| {
                warn!("Public URL passwords could not be saved: {error:#}");
                let mut saved = self.saved_passwords();
                change(&mut saved);
                saved
            })
    }

    /// A file that cannot be read counts as empty: it only costs the links.
    fn saved_passwords(&self) -> SavedPasswords {
        self.passwords.load().unwrap_or_else(|error| {
            warn!("Public URL passwords could not be read: {error:#}");
            SavedPasswords::new()
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

fn view(url: PublicUrl, account: &CloudAccount, saved: &SavedPasswords) -> PublicUrlView {
    let served_here = url
        .machine
        .as_ref()
        .is_some_and(|machine| machine.id == account.machine_id);
    let password_link = saved
        .get(&url.id)
        .and_then(|saved| current_password(&url, saved))
        .map(|password| password_link(&url.url, password));
    PublicUrlView {
        url,
        served_here,
        password_link,
    }
}

/// The saved password, when the URL still asks for it: the protection has
/// not changed since, from this machine or another.
fn current_password<'s>(url: &PublicUrl, saved: &'s SavedPassword) -> Option<&'s str> {
    (url.protection.mode == ProtectionMode::Password
        && url.protection.version == Some(saved.version))
    .then_some(saved.password.as_str())
}

/// `https://<host>/__localify/access#<password>`. The fragment never leaves
/// the browser: the page there reads it and signs in.
fn password_link(url: &str, password: &str) -> String {
    format!(
        "{}{ACCESS_PATH}#{}",
        url.trim_end_matches('/'),
        encode_uri_component(password)
    )
}

/// Percent-encodes like JavaScript's `encodeURIComponent`, which the page
/// reverses with `decodeURIComponent`.
fn encode_uri_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => encoded.push(char::from(byte)),
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

/// The password a person chose, which the API does not send back.
fn chosen_password(protection: &ProtectionChoice) -> Option<String> {
    match protection {
        ProtectionChoice::Password(Some(password)) => Some(password.clone()),
        _ => None,
    }
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

fn parse_local_domain(value: &str) -> Result<DomainName> {
    DomainName::new(value)
        .map_err(|error| anyhow!("'{}' is not a local domain: {error}", value.trim()))
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
        passwords: InMemoryPublicUrlPasswords,
        tunnel: InMemoryPublicTunnel,
        domains: InMemoryLocalDomains,
    }

    impl World {
        fn new() -> Self {
            Self {
                api: InMemoryCloudApi::new(),
                store: InMemoryCloudAccountStore::signed_in(),
                passwords: InMemoryPublicUrlPasswords::new(),
                tunnel: InMemoryPublicTunnel::running(),
                domains: InMemoryLocalDomains::new().with_owner(
                    "journal.localify",
                    LocalDomainOwner::Project("Journal".into()),
                ),
            }
        }

        fn urls(&self) -> PublicUrls<'_> {
            PublicUrls::new(
                &self.api,
                &self.store,
                &self.passwords,
                &self.tunnel,
                &self.domains,
            )
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

    const LINK: &str = "https://journal-a8f3.localify.fyi/__localify/access#four-random-words-here";

    #[test]
    fn a_password_set_here_gives_a_link_that_carries_it() {
        let world = World::new();
        let urls = world.urls();

        let created = urls
            .create(create("journal.localify", false))
            .unwrap()
            .value;
        assert_eq!(created.public_url.password_link.as_deref(), Some(LINK));

        let id = created.public_url.url.id;
        assert_eq!(
            urls.find("journal-a8f3").unwrap().password_link.as_deref(),
            Some(LINK)
        );
        assert_eq!(urls.list().unwrap()[0].password_link.as_deref(), Some(LINK));

        let chosen = urls
            .set_protection(&id, ProtectionChoice::Password(Some("my own pass".into())))
            .unwrap()
            .value;
        assert_eq!(chosen.password, None);
        assert_eq!(
            chosen.public_url.password_link.as_deref(),
            Some("https://journal-a8f3.localify.fyi/__localify/access#my%20own%20pass")
        );
    }

    #[test]
    fn opening_or_removing_a_url_forgets_its_password() {
        let world = World::new();
        let urls = world.urls();
        let id = urls
            .create(create("journal.localify", false))
            .unwrap()
            .value
            .public_url
            .url
            .id;

        let opened = urls.set_protection(&id, ProtectionChoice::Public).unwrap();
        assert_eq!(opened.value.public_url.password_link, None);
        assert!(world.passwords.saved().is_empty());

        urls.set_protection(&id, ProtectionChoice::Password(None))
            .unwrap();
        assert_eq!(world.passwords.saved().len(), 1);
        urls.remove(&id).unwrap();
        assert!(world.passwords.saved().is_empty());
    }

    #[test]
    fn a_password_changed_elsewhere_gives_no_link_and_is_forgotten() {
        let world = World::new();
        let urls = world.urls();
        let id = urls
            .create(create("journal.localify", false))
            .unwrap()
            .value
            .public_url
            .url
            .id;
        // Another machine rotates it: same API, a passwords file of its own.
        let elsewhere = InMemoryPublicUrlPasswords::new();
        PublicUrls::new(
            &world.api,
            &world.store,
            &elsewhere,
            &world.tunnel,
            &world.domains,
        )
        .set_protection(&id, ProtectionChoice::Password(None))
        .unwrap();

        assert_eq!(urls.find(&id).unwrap().password_link, None);
        assert_eq!(urls.list().unwrap()[0].password_link, None);
        assert!(world.passwords.saved().is_empty());
    }

    #[test]
    fn a_list_older_than_a_saved_password_keeps_it() {
        let world = World::new();
        world.api.insert(public_url("u1", "journal-a8f3", false));
        // Saved by a change this list predates, and for a URL created since.
        world
            .passwords
            .update(&mut |saved| {
                for (id, version) in [("u1", 7), ("u-new", 1)] {
                    let password = SavedPassword {
                        version,
                        password: "open sesame".into(),
                    };
                    saved.insert(id.to_string(), password);
                }
            })
            .unwrap();

        world.urls().list().unwrap();

        assert_eq!(world.passwords.saved().len(), 2);
    }

    #[test]
    fn a_password_that_cannot_be_saved_gives_the_link_this_once() {
        let world = World {
            passwords: InMemoryPublicUrlPasswords::failing(),
            ..World::new()
        };
        let urls = world.urls();

        let created = urls
            .create(create("journal.localify", false))
            .unwrap()
            .value;

        assert_eq!(created.password.as_deref(), Some("four-random-words-here"));
        assert_eq!(created.public_url.password_link.as_deref(), Some(LINK));
        assert_eq!(urls.find("journal-a8f3").unwrap().password_link, None);
    }

    #[test]
    fn the_link_encodes_the_password_like_encode_uri_component() {
        assert_eq!(
            password_link("https://shop-1b2c.localify.fyi/", "a b#c%d/é&*'()!~._-"),
            "https://shop-1b2c.localify.fyi/__localify/access#a%20b%23c%25d%2F%C3%A9%26*'()!~._-"
        );
    }

    #[test]
    fn debug_output_never_shows_the_link() {
        let world = World::new();
        let created = world
            .urls()
            .create(create("journal.localify", false))
            .unwrap()
            .value;

        let output = format!("{:?}", created.public_url);
        assert!(!output.contains("four-random-words-here"), "{output}");
    }

    #[test]
    fn a_suggestion_mirrors_the_local_name_and_avoids_taken_ones() {
        let world = World::new();
        let urls = world.urls();

        assert_eq!(
            urls.suggest_slug("Louez.localify").unwrap(),
            SlugSuggestion {
                slug: "louez".into(),
                url: "https://louez.localify.fyi".into(),
            }
        );
        world.api.insert(public_url("u9", "louez", true));
        assert_eq!(
            urls.suggest_slug("louez.localify").unwrap().slug,
            "louez-teo-goulois"
        );
        assert!(urls.suggest_slug("not a domain").is_err());
    }

    #[test]
    fn a_slug_is_checked_here_before_localify_cloud() {
        let world = World::new();
        world.api.insert(public_url("u9", "louez", true));
        let urls = world.urls();

        assert_eq!(
            urls.check_slug(" Louez ").unwrap(),
            SlugCheck {
                slug: "louez".into(),
                available: false
            }
        );
        assert!(urls.check_slug("louez-shop").unwrap().available);

        let error = urls.check_slug("my--app").unwrap_err().to_string();
        assert!(error.contains("'--'"), "{error}");
        assert_eq!(world.api.calls(), ["slug_available", "slug_available"]);
    }
}
