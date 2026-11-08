//! Gathers the facts of this machine's inventory (docs/remote-machines.md,
//! "Inventory") and projects them: the configured projects, environments and
//! services, the launch profiles of their checkouts, the run records in
//! `services/`, a TCP connect to each service's port, and the public URLs the
//! daemon last learned from Localify Cloud. A profile's values never leave
//! this module: only each service's revision does.

use std::collections::{BTreeSet, HashMap};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, UNIX_EPOCH};

use anyhow::Result;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use crate::application::inventory::{
    AppPresence, Context, EnvironmentRef, ProfileChecks, ProjectRef, PublicUrlFacts,
    PublicUrlState as UrlState, Revisions, RunExit, RunFacts, RunOrigin, RunProcess, ServiceFacts,
    ServiceRef, Snapshot, TargetSettings, inventory_item,
};
use crate::domain::RouteTarget;
use crate::infrastructure::cloud::types::{PublicUrl, PublicUrlState};
use crate::infrastructure::config::{
    Config, ConfigStore, ConfiguredProjectEnvironment, ConfiguredService,
};
use crate::infrastructure::launch_profiles::LaunchProfiles;
use crate::infrastructure::launch_rev::{LaunchDefinition, LaunchRevKey, random_bytes};
use crate::infrastructure::running_services::{
    RunState, RunningService, RunningServiceStore, ServiceKey, ServiceOrigin,
};

/// How long a port has to accept a connection.
const PORT_TIMEOUT: Duration = Duration::from_millis(200);
/// `<slug>--<label>` is one DNS label.
const MAX_DNS_LABEL: usize = 63;

/// What the daemon knows of public URLs: Localify Cloud's last answer, this
/// machine's id, and whether its tunnel is connected.
#[derive(Debug, Default)]
pub struct PublicFacts {
    pub urls: Vec<PublicUrl>,
    pub machine_id: Option<String>,
    pub tunnel_connected: bool,
}

/// The daemon's inventory: where its config and secret live, and the counter
/// that numbers its snapshots for as long as it runs.
pub struct LocalInventory {
    config_path: PathBuf,
    data_dir: PathBuf,
    /// `None`: the user's own `services/`, found when a snapshot is taken.
    services_dir: Option<PathBuf>,
    revisions: Mutex<Revisions>,
}

impl LocalInventory {
    /// A new epoch: 16 random bytes, base64url.
    pub fn new(config_path: PathBuf, data_dir: PathBuf) -> Result<Self> {
        let mut epoch = [0_u8; 16];
        random_bytes(&mut epoch)?;
        Ok(Self {
            config_path,
            data_dir,
            services_dir: None,
            revisions: Mutex::new(Revisions::new(URL_SAFE_NO_PAD.encode(epoch))),
        })
    }

    /// Reads run records from `dir` instead of the user's `services/`.
    #[cfg(test)]
    pub fn with_services_dir(mut self, dir: PathBuf) -> Self {
        self.services_dir = Some(dir);
        self
    }

    /// A fresh snapshot, numbered: the revision only moves when its content
    /// changed. Settings and the app bridge come later (PR 2 and PR 4), so
    /// until then every control is off and the app counts as absent.
    pub fn snapshot(&self, public: &PublicFacts) -> Result<Snapshot> {
        let config = ConfigStore::new(self.config_path.clone()).load()?;
        let store = match &self.services_dir {
            Some(dir) => RunningServiceStore::new(dir.clone()),
            None => RunningServiceStore::new_default()?,
        };
        let key = LaunchRevKey::load_or_create(&self.data_dir)?;
        let context = Context {
            settings: TargetSettings::default(),
            app: AppPresence::Absent,
            tunnel_connected: public.tunnel_connected,
        };
        let items = gather(&config, &store, &key, public)?
            .into_iter()
            .map(|facts| inventory_item(facts, &context))
            .collect();
        self.revisions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .snapshot(items)
    }
}

/// One service in one environment, with the port it listens on.
struct Placed {
    environment: ConfiguredProjectEnvironment,
    service: ConfiguredService,
    port: u16,
}

/// The facts of every configured service with a proxy target, in every
/// environment of its project. A static or targetless service runs nothing.
pub fn gather(
    config: &Config,
    store: &RunningServiceStore,
    key: &LaunchRevKey,
    public: &PublicFacts,
) -> Result<Vec<ServiceFacts>> {
    let placed = place_services(config);
    let reachable = probe_ports(placed.iter().map(|placed| placed.port).collect());
    let records = store.list()?;
    let observed = Observed {
        config,
        store,
        key,
        public,
        records: &records,
    };
    // Services of one checkout share its profiles.
    let mut checkouts: HashMap<Option<String>, Result<Option<LaunchProfiles>>> = HashMap::new();
    placed
        .into_iter()
        .map(|placed| {
            let profiles = checkouts
                .entry(placed.service.project_root_path.clone())
                .or_insert_with(|| load_profiles(placed.service.project_root_path.as_deref()));
            let reachable = reachable.get(&placed.port).copied().unwrap_or(false);
            service_facts(placed, reachable, profiles, &observed)
        })
        .collect()
}

/// What every service's facts are read against.
struct Observed<'a> {
    config: &'a Config,
    store: &'a RunningServiceStore,
    key: &'a LaunchRevKey,
    public: &'a PublicFacts,
    records: &'a [RunningService],
}

/// Every service of the primary checkout, as `localify dev` runs it without
/// `--env`, and the services each other environment maps.
fn place_services(config: &Config) -> Vec<Placed> {
    let services = config.configured_services();
    let mut placed = Vec::new();
    for environment in config.configured_project_environments(None) {
        let of_project = services
            .iter()
            .filter(|service| service.project_id == environment.project_id);
        for service in of_project {
            let service = if environment.primary {
                Some(service.clone())
            } else {
                config.environment_service(service.clone(), &environment)
            };
            let Some(service) = service else { continue };
            let Some(RouteTarget::Proxy(target)) = &service.primary_target else {
                continue;
            };
            let port = target.port().value();
            placed.push(Placed {
                environment: environment.clone(),
                service,
                port,
            });
        }
    }
    placed
}

fn load_profiles(root: Option<&str>) -> Result<Option<LaunchProfiles>> {
    match root {
        Some(root) => LaunchProfiles::load(Path::new(root)),
        None => Ok(None),
    }
}

fn service_facts(
    placed: Placed,
    reachable: bool,
    profiles: &Result<Option<LaunchProfiles>>,
    observed: &Observed<'_>,
) -> Result<ServiceFacts> {
    let Placed {
        environment,
        service,
        port,
    } = placed;
    let key = ServiceKey {
        project_id: service.project_id.clone(),
        environment: Some(environment.slug.clone()),
        service_id: service.id.clone(),
    };
    let definition = LaunchDefinition::new(&service, key.environment.as_deref(), profiles);
    let record = observed.records.iter().find(|record| record.key == key);
    Ok(ServiceFacts {
        project: ProjectRef {
            id: service.project_id.clone(),
            name: environment.project_name.clone(),
        },
        environment: EnvironmentRef {
            slug: (!environment.primary).then(|| environment.slug.clone()),
            name: environment.name.clone(),
            primary: environment.primary,
            branch: environment.git_branch.clone(),
        },
        service: ServiceRef {
            id: service.id.clone(),
            name: service.name.clone(),
            config_rev: observed.key.revision(&definition)?,
        },
        local_url: local_url(observed.config, &service),
        port,
        reachable,
        run: record.map(|record| run_facts(observed.store, record, profiles)),
        public_url: public_url_facts(&service.primary_domain, observed.public),
        profiles: ProfileChecks {
            start: profile_allowed(profiles, None),
            restart: profile_allowed(
                profiles,
                record.and_then(|record| record.profile.as_deref()),
            ),
        },
    })
}

/// From the record and the run's session. A record with no run id, or an app
/// run with no stream (a shell tab of an app from before run PTYs), is a
/// legacy run: it shows no run id and no output.
fn run_facts(
    store: &RunningServiceStore,
    record: &RunningService,
    profiles: &Result<Option<LaunchProfiles>>,
) -> RunFacts {
    let legacy =
        record.run.is_none() || (record.origin == ServiceOrigin::App && !record.has_stream());
    RunFacts {
        run_id: record.run_id().filter(|_| !legacy).map(ToString::to_string),
        origin: match record.origin {
            ServiceOrigin::App => RunOrigin::App,
            ServiceOrigin::Background => RunOrigin::Background,
            ServiceOrigin::Terminal => RunOrigin::Terminal,
        },
        legacy,
        has_stream: record.has_stream(),
        process: match store.state(record) {
            RunState::Running => RunProcess::Running,
            RunState::Stopped => RunProcess::Stopped,
            RunState::Unknown => RunProcess::Unproven,
        },
        started_at: rfc3339(record.started_at),
        profile: record.profile.clone(),
        profile_danger: profile_danger(profiles, record.profile.as_deref()),
        exit: record.exit.as_ref().map(|exit| RunExit {
            at: rfc3339(exit.at),
            code: exit.code,
        }),
    }
}

/// A profile a remote launch may use: it resolves under the effective rules
/// of `LaunchProfiles::load` and is not dangerous. `None` is the effective
/// default, which is never dangerous. Profiles that do not load allow none.
fn profile_allowed(profiles: &Result<Option<LaunchProfiles>>, name: Option<&str>) -> bool {
    match profiles {
        Err(_) => false,
        Ok(None) => name.is_none(),
        Ok(Some(profiles)) => profiles
            .select(name)
            .is_ok_and(|profile| profile.is_none_or(|profile| !profile.danger)),
    }
}

fn profile_danger(profiles: &Result<Option<LaunchProfiles>>, name: Option<&str>) -> bool {
    let (Ok(Some(profiles)), Some(name)) = (profiles, name) else {
        return false;
    };
    profiles
        .select(Some(name))
        .is_ok_and(|profile| profile.is_some_and(|profile| profile.danger))
}

/// The URL that serves the service's domain: one on that domain, or a
/// wildcard URL on its parent, at `<slug>--<label>`.
fn public_url_facts(domain: &str, public: &PublicFacts) -> Option<PublicUrlFacts> {
    let domain = domain.trim();
    let domain = domain
        .strip_prefix("*.")
        .unwrap_or(domain)
        .to_ascii_lowercase();
    let exact = public
        .urls
        .iter()
        .find(|url| url.local_domain.eq_ignore_ascii_case(&domain))
        .map(|url| (url, url.url.clone()));
    let (url, address) = exact.or_else(|| {
        public
            .urls
            .iter()
            .filter(|url| url.wildcard)
            .find_map(|url| {
                let parent = format!(".{}", url.local_domain.to_ascii_lowercase());
                let label = domain.strip_suffix(&parent)?;
                Some((url, environment_address(url, label)?))
            })
    })?;
    Some(PublicUrlFacts {
        url: address,
        state: match url.state {
            None | Some(PublicUrlState::Serving) => UrlState::Serving,
            Some(PublicUrlState::Suspended) => UrlState::Suspended,
            Some(PublicUrlState::TakenDown) => UrlState::TakenDown,
        },
        served_here: url
            .machine
            .as_ref()
            .is_some_and(|machine| Some(&machine.id) == public.machine_id.as_ref()),
    })
}

/// `https://<slug>--<label>.<base>` for one environment of a wildcard URL,
/// when that is one DNS label.
fn environment_address(url: &PublicUrl, label: &str) -> Option<String> {
    let first_label = format!("{}--{label}", url.slug.to_ascii_lowercase());
    let valid = !label.is_empty() && !label.contains('.') && first_label.len() <= MAX_DNS_LABEL;
    let host = url.url.strip_prefix("https://")?.split('/').next()?;
    let (_, base) = host.split_once('.')?;
    valid.then(|| format!("https://{first_label}.{}", base.to_ascii_lowercase()))
}

/// The service's URL on this machine, as `localify dev` prints it.
fn local_url(config: &Config, service: &ConfiguredService) -> String {
    let domain = service.primary_domain.trim();
    let host = domain.strip_prefix("*.").unwrap_or(domain);
    let (scheme, port, default) = if config.https_enabled_for(domain) {
        ("https", config.daemon.https_port, 443)
    } else {
        ("http", config.daemon.http_port, 80)
    };
    if port == default {
        format!("{scheme}://{host}")
    } else {
        format!("{scheme}://{host}:{port}")
    }
}

/// Connects to every port on 127.0.0.1 at once, so a snapshot waits at most
/// one timeout.
fn probe_ports(ports: BTreeSet<u16>) -> HashMap<u16, bool> {
    thread::scope(|scope| {
        let probes: Vec<_> = ports
            .into_iter()
            .map(|port| {
                let address = SocketAddr::from(([127, 0, 0, 1], port));
                (
                    port,
                    scope.spawn(move || TcpStream::connect_timeout(&address, PORT_TIMEOUT).is_ok()),
                )
            })
            .collect();
        probes
            .into_iter()
            .map(|(port, probe)| (port, probe.join().unwrap_or(false)))
            .collect()
    })
}

fn rfc3339(unix_seconds: u64) -> String {
    humantime::format_rfc3339_seconds(UNIX_EPOCH + Duration::from_secs(unix_seconds)).to_string()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::net::TcpListener;

    use super::*;
    use crate::application::inventory::{OutputReason, Owner, ProcessState, Refusal};
    use crate::infrastructure::process;
    use crate::infrastructure::running_services::{RunIdentity, RunOutput};
    use crate::infrastructure::runs::RunId;

    const PROFILES: &str = r#"
[profiles.default]
sources = [{ file = ".env" }]

[profiles.prod]
danger = true
sources = [{ file = ".env.prod" }]

[profiles.prod.env]
SEND_EMAILS = "value-one"
"#;

    /// A project with one service, its primary checkout and a worktree.
    struct Machine {
        dir: tempfile::TempDir,
        port: u16,
        feature_port: u16,
    }

    impl Machine {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            fs::create_dir_all(dir.path().join("app")).unwrap();
            fs::create_dir_all(dir.path().join("feature")).unwrap();
            Self {
                dir,
                port: free_port(),
                feature_port: free_port(),
            }
        }

        fn path(&self, name: &str) -> PathBuf {
            self.dir.path().join(name)
        }

        fn config_toml(&self) -> String {
            format!(
                r#"
[daemon]
https_port = 8443

[domains."web.localify"]
pattern = "web.localify"
https_enabled = true
project = "app"

[[domains."web.localify".routes]]
path = "/"
target = "127.0.0.1:{port}"

[domains."web.feature.localify"]
pattern = "web.feature.localify"
project = "app"

[[domains."web.feature.localify".routes]]
path = "/"
target = "127.0.0.1:{feature_port}"

[projects.app]
name = "App"
root_path = "{root}"

[[projects.app.services]]
name = "web"
primary_domain = "web.localify"
command = ["pnpm", "dev"]

[[projects.app.environments]]
name = "Feature"
slug = "feature"
root_path = "{feature}"
git_branch = "feature/login"

[[projects.app.environments.services]]
service = "web"
domain = "web.feature.localify"
"#,
                port = self.port,
                feature_port = self.feature_port,
                root = self.path("app").display(),
                feature = self.path("feature").display(),
            )
        }

        fn config(&self) -> Config {
            toml::from_str(&self.config_toml()).unwrap()
        }

        fn store(&self) -> RunningServiceStore {
            RunningServiceStore::new(self.path("services"))
        }

        fn key(&self) -> LaunchRevKey {
            LaunchRevKey::load_or_create(&self.path("data")).unwrap()
        }

        fn gather_with(&self, config: &Config) -> Vec<ServiceFacts> {
            gather(config, &self.store(), &self.key(), &PublicFacts::default()).unwrap()
        }

        /// The primary checkout's service.
        fn primary(&self) -> ServiceFacts {
            self.gather_with(&self.config())
                .into_iter()
                .find(|facts| facts.environment.primary)
                .unwrap()
        }

        fn write_record(&self, record: &RunningService) {
            fs::create_dir_all(self.path("services")).unwrap();
            fs::write(
                self.path("services").join("app--primary--web.json"),
                serde_json::to_string(record).unwrap(),
            )
            .unwrap();
        }
    }

    fn free_port() -> u16 {
        TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    /// A run whose executor is this test process: it runs.
    fn live_record(origin: ServiceOrigin, output: Option<RunOutput>) -> RunningService {
        let me = process::process_info(std::process::id()).unwrap();
        RunningService {
            key: ServiceKey {
                project_id: "app".into(),
                environment: Some("primary".into()),
                service_id: "web".into(),
            },
            pid: me.pid,
            run: Some(RunIdentity {
                run_id: RunId::generate().unwrap(),
                start: me.start,
                sid: None,
                pgid: None,
                boot_id: process::boot_id().unwrap(),
                last_alive_at: process::monotonic_now(),
                output,
            }),
            process_started_at: None,
            url: "https://web.localify:8443".into(),
            port: 3000,
            origin,
            profile: Some("prod".into()),
            started_at: 1_794_000_000,
            log_file: None,
            exit: None,
        }
    }

    fn open() -> Context {
        Context {
            settings: TargetSettings {
                share: true,
                control: true,
                background_starts: true,
            },
            app: AppPresence::Running,
            tunnel_connected: true,
        }
    }

    #[test]
    fn the_primary_checkout_and_each_environment_are_listed() {
        let machine = Machine::new();

        let facts = machine.gather_with(&machine.config());

        assert_eq!(facts.len(), 2);
        let primary = &facts[0];
        assert_eq!(primary.environment.slug, None);
        assert_eq!(primary.local_url, "https://web.localify:8443");
        assert_eq!(primary.port, machine.port);
        let feature = &facts[1];
        assert_eq!(feature.environment.slug.as_deref(), Some("feature"));
        assert_eq!(feature.environment.branch.as_deref(), Some("feature/login"));
        assert_eq!(feature.local_url, "http://web.feature.localify");
        assert_eq!(feature.port, machine.feature_port);
        assert_ne!(primary.service.config_rev, feature.service.config_rev);
    }

    #[test]
    fn an_inline_profile_value_changes_the_revision() {
        let machine = Machine::new();
        let file = machine.path("app").join("localify.toml");
        fs::write(&file, PROFILES).unwrap();

        let first = machine.primary().service.config_rev;
        let unchanged = machine.primary().service.config_rev;
        fs::write(&file, PROFILES.replace("value-one", "value-two")).unwrap();
        let edited = machine.primary().service.config_rev;

        assert_eq!(first, unchanged);
        assert_ne!(first, edited);
        assert_eq!(first.len(), 16);
    }

    #[test]
    fn the_service_entry_changes_the_revision_too() {
        let machine = Machine::new();
        let before = machine.primary().service.config_rev;

        let edited: Config =
            toml::from_str(&machine.config_toml().replace("\"dev\"", "\"start\"")).unwrap();
        let after = machine.gather_with(&edited).remove(0).service.config_rev;

        assert_ne!(before, after);
    }

    #[test]
    fn profile_values_never_appear_in_the_snapshot() {
        let machine = Machine::new();
        fs::write(machine.path("app").join("localify.toml"), PROFILES).unwrap();
        machine.write_record(&live_record(
            ServiceOrigin::Background,
            Some(RunOutput::Pipe),
        ));

        let items: Vec<_> = machine
            .gather_with(&machine.config())
            .into_iter()
            .map(|facts| inventory_item(facts, &open()))
            .collect();
        let json = serde_json::to_string(&items).unwrap();

        assert!(!json.contains("value-one"));
        assert!(!json.contains("SEND_EMAILS"));
        assert!(!json.contains(".env.prod"));
    }

    #[test]
    fn a_background_run_of_a_live_executor_runs_with_its_output() {
        let machine = Machine::new();
        let record = live_record(ServiceOrigin::Background, Some(RunOutput::Pipe));
        machine.write_record(&record);

        let item = inventory_item(machine.primary(), &open());

        assert_eq!(item.process, ProcessState::Running);
        assert_eq!(item.owner, Some(Owner::Background));
        assert!(!item.port.reachable);
        assert!(item.output.available);
        assert_eq!(
            item.output.run_id.as_deref(),
            record.run_id().map(RunId::as_str)
        );
        let run = item.run.unwrap();
        assert_eq!(run.started_at, "2026-11-06T21:20:00Z");
        assert!(!run.legacy);
        assert_eq!(run.profile.as_deref(), Some("prod"));
    }

    #[test]
    fn a_reachable_port_with_no_record_is_unknown() {
        let machine = Machine::new();
        let _listener = TcpListener::bind(("127.0.0.1", machine.port)).unwrap();

        let item = inventory_item(machine.primary(), &open());

        assert!(item.port.reachable);
        assert_eq!(item.process, ProcessState::Stopped);
        assert_eq!(item.owner, Some(Owner::Unknown));
        assert_eq!(item.output.reason, Some(OutputReason::UnknownOwner));
    }

    #[test]
    fn legacy_runs_show_as_legacy_with_no_output() {
        let machine = Machine::new();
        // From an app before run ids: no identity, a whole-shell file.
        let mut old = live_record(ServiceOrigin::App, None);
        old.run = None;
        old.pid = u32::MAX - 1;
        old.process_started_at = Some("Thu Nov  5 10:00:00 2026".into());
        old.log_file = Some(machine.path("whole-shell.log"));
        // From a shell tab with a run id, but no run PTY: no stream.
        let shell_tab = live_record(ServiceOrigin::App, None);

        for record in [old, shell_tab] {
            machine.write_record(&record);
            let item = inventory_item(machine.primary(), &open());
            let run = item.run.clone().unwrap();

            assert!(run.legacy);
            assert_eq!(run.run_id, None);
            assert_eq!(item.output.reason, Some(OutputReason::LegacyRun));
        }
        let shell_tab = inventory_item(machine.primary(), &open());
        assert_eq!(shell_tab.process, ProcessState::Running);
        assert_eq!(shell_tab.controls.stop.code, Some(Refusal::LegacyRun));
    }

    #[test]
    fn controls_are_off_until_settings_exist() {
        let machine = Machine::new();
        fs::write(machine.path("config.toml"), machine.config_toml()).unwrap();
        let inventory = LocalInventory::new(machine.path("config.toml"), machine.path("data"))
            .unwrap()
            .with_services_dir(machine.path("services"));

        let snapshot = inventory.snapshot(&PublicFacts::default()).unwrap();

        assert_eq!(snapshot.items.len(), 2);
        assert!(snapshot.items.iter().all(|item| {
            item.controls.start.code == Some(Refusal::ControlDisabled)
                && item.controls.restart.code == Some(Refusal::ControlDisabled)
        }));
        assert_eq!(inventory.snapshot(&PublicFacts::default()).unwrap().rev, 1);
    }

    #[test]
    fn profiles_decide_what_a_control_may_launch() {
        let dir = tempfile::tempdir().unwrap();
        // Implicit Infisical profiles: default, staging, and a dangerous prod.
        fs::write(dir.path().join(".infisical.yaml"), "workspaceId: w\n").unwrap();
        let implicit = LaunchProfiles::load(dir.path());

        assert!(profile_allowed(&implicit, None));
        assert!(profile_allowed(&implicit, Some("staging")));
        assert!(!profile_allowed(&implicit, Some("prod")));
        assert!(!profile_allowed(&implicit, Some("removed")));
        assert!(profile_danger(&implicit, Some("prod")));
        assert!(!profile_danger(&implicit, Some("staging")));

        assert!(profile_allowed(&Ok(None), None));
        assert!(!profile_allowed(&Ok(None), Some("default")));

        fs::write(dir.path().join("localify.toml"), "[profiles.x]\nbad = 1\n").unwrap();
        let invalid = LaunchProfiles::load(dir.path());
        assert!(invalid.is_err());
        assert!(!profile_allowed(&invalid, None));
    }

    fn public_url(id: &str, local_domain: &str, wildcard: bool, extra: &str) -> PublicUrl {
        serde_json::from_str(&format!(
            r#"{{"id":"{id}","slug":"app-a8f3","url":"https://app-a8f3.localify.fyi",
                "localDomain":"{local_domain}","wildcard":{wildcard},
                "protection":{{"mode":"password"}},"createdAt":"2026-11-05T18:00:00Z"{extra}}}"#
        ))
        .unwrap()
    }

    #[test]
    fn a_service_finds_its_public_url_exact_or_wildcard() {
        let here = r#","machine":{"id":"m1","name":"laptop"},"state":"serving""#;
        let elsewhere = r#","machine":null,"state":"suspended""#;
        let public = PublicFacts {
            urls: vec![
                public_url("ns", "app.localify", true, here),
                public_url("exact", "web.localify", false, elsewhere),
            ],
            machine_id: Some("m1".into()),
            tunnel_connected: true,
        };

        let exact = public_url_facts("web.localify", &public).unwrap();
        assert_eq!(exact.state, UrlState::Suspended);
        assert!(!exact.served_here);

        let environment = public_url_facts("api.app.localify", &public).unwrap();
        assert_eq!(environment.url, "https://app-a8f3--api.localify.fyi");
        assert!(environment.served_here);
        assert_eq!(environment.state, UrlState::Serving);

        assert!(public_url_facts("a.b.app.localify", &public).is_none());
        let long = format!("{}.app.localify", "x".repeat(60));
        assert!(public_url_facts(&long, &public).is_none());
        assert!(public_url_facts("other.localify", &public).is_none());
    }
}
