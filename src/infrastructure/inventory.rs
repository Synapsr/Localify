//! Gathers the facts of this machine's inventory (docs/remote-machines.md,
//! "Inventory") and projects them: the configured projects, environments and
//! services, the plan `localify dev` resolves for each, the launch profiles of
//! their checkouts, the run records in `services/`, a TCP connect to each
//! service's port, and the public URLs the daemon last learned from Localify
//! Cloud. A profile's values never leave this module: only each service's
//! revision does.

use std::collections::{BTreeSet, HashMap};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant, UNIX_EPOCH};

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
use crate::infrastructure::launch_plan::{self, LaunchPlan, service_with_project_root};
use crate::infrastructure::launch_profiles::LaunchProfiles;
use crate::infrastructure::launch_rev::{LaunchDefinition, LaunchRevKey, random_bytes};
use crate::infrastructure::running_services::{
    RunState, RunningService, RunningServiceStore, ServiceKey, ServiceOrigin,
};

/// How long one port has to accept a connection.
const PORT_TIMEOUT: Duration = Duration::from_millis(200);
/// How long all the ports of one snapshot have: a port not probed by then is
/// unknown.
const PROBE_DEADLINE: Duration = Duration::from_secs(1);
/// Threads that probe ports, whatever their number.
const PROBE_WORKERS: usize = 16;
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

/// How many probe threads run at once, and the most that ever did.
#[derive(Debug, Default)]
pub struct ProbeGauge {
    live: AtomicUsize,
    peak: AtomicUsize,
}

impl ProbeGauge {
    /// The most probe threads that ran at once.
    #[cfg(test)]
    fn peak(&self) -> usize {
        self.peak.load(Ordering::SeqCst)
    }

    fn enter(&self) -> ProbeSlot<'_> {
        let live = self.live.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(live, Ordering::SeqCst);
        ProbeSlot(self)
    }
}

/// One running probe thread, counted until it ends.
struct ProbeSlot<'a>(&'a ProbeGauge);

impl Drop for ProbeSlot<'_> {
    fn drop(&mut self) {
        self.0.live.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The daemon's inventory: where its config and secret live, the counter
/// that numbers its snapshots for as long as it runs, and the build in
/// progress, of which there is at most one.
pub struct LocalInventory {
    config_path: PathBuf,
    data_dir: PathBuf,
    /// `None`: the user's own `services/`, found when a snapshot is taken.
    services_dir: Option<PathBuf>,
    revisions: Mutex<Revisions>,
    /// Held for a whole build.
    building: Mutex<()>,
    /// How many builds completed, and the last one's snapshot.
    built: AtomicU64,
    last: Mutex<Option<Snapshot>>,
    /// Its probe threads: at most [`PROBE_WORKERS`], since builds never overlap.
    probes: ProbeGauge,
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
            building: Mutex::new(()),
            built: AtomicU64::new(0),
            last: Mutex::new(None),
            probes: ProbeGauge::default(),
        })
    }

    /// Reads run records from `dir` instead of the user's `services/`.
    #[cfg(test)]
    pub fn with_services_dir(mut self, dir: PathBuf) -> Self {
        self.services_dir = Some(dir);
        self
    }

    /// A fresh snapshot, numbered: the revision only moves when its content
    /// changed. One build runs at a time: a request that arrives during one
    /// waits for it and shares its snapshot. `project_roots` are the local
    /// checkouts the app knows, as `localify dev` reads them. Settings and the
    /// app bridge come later (PR 2 and PR 4), so until then every control is
    /// off and the app counts as absent.
    pub fn snapshot(
        &self,
        public: &PublicFacts,
        project_roots: &HashMap<String, PathBuf>,
    ) -> Result<Snapshot> {
        let seen = self.built.load(Ordering::SeqCst);
        let _building = lock(&self.building);
        if self.built.load(Ordering::SeqCst) != seen
            && let Some(last) = lock(&self.last).clone()
        {
            return Ok(last);
        }
        let snapshot = self.build(public, project_roots)?;
        *lock(&self.last) = Some(snapshot.clone());
        self.built.fetch_add(1, Ordering::SeqCst);
        Ok(snapshot)
    }

    fn build(
        &self,
        public: &PublicFacts,
        project_roots: &HashMap<String, PathBuf>,
    ) -> Result<Snapshot> {
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
        let observed = Observed {
            config: &config,
            store: &store,
            key: &key,
            public,
            project_roots,
            probes: &self.probes,
        };
        let items = gather(&observed)?
            .into_iter()
            .map(|facts| inventory_item(facts, &context))
            .collect();
        lock(&self.revisions).snapshot(items)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// What a snapshot is gathered from.
pub struct Observed<'a> {
    pub config: &'a Config,
    pub store: &'a RunningServiceStore,
    pub key: &'a LaunchRevKey,
    pub public: &'a PublicFacts,
    pub project_roots: &'a HashMap<String, PathBuf>,
    pub probes: &'a ProbeGauge,
}

/// One service in one environment, with the port it listens on.
struct Placed {
    environment: ConfiguredProjectEnvironment,
    service: ConfiguredService,
    port: u16,
}

/// The facts of every configured service with a proxy target, in every
/// environment of its project. A static or targetless service runs nothing.
pub fn gather(observed: &Observed<'_>) -> Result<Vec<ServiceFacts>> {
    let placed = place_services(observed.config, observed.project_roots);
    let ports = placed.iter().map(|placed| placed.port).collect();
    let reachable = probe_ports(ports, PROBE_DEADLINE, observed.probes);
    let records = observed.store.list()?;
    // Services of one checkout share its profiles.
    let mut checkouts: HashMap<Option<PathBuf>, Result<Option<LaunchProfiles>>> = HashMap::new();
    placed
        .into_iter()
        .map(|placed| {
            let plan = launch_plan::dev_plan(placed.service.clone(), placed.port);
            let root = profiles_root(&placed.service, plan.as_ref().ok());
            let profiles = checkouts
                .entry(root.clone())
                .or_insert_with(|| load_profiles(root.as_deref()));
            let reachable = reachable.get(&placed.port).copied();
            let record = records.iter().find(|record| record.key == key_of(&placed));
            service_facts(placed, reachable, &plan, profiles, record, observed)
        })
        .collect()
}

/// Every service of the primary checkout, as `localify dev` runs it without
/// `--env`, and the services each other environment maps, each with the
/// local checkout the app knows when the config's is gone.
fn place_services(config: &Config, project_roots: &HashMap<String, PathBuf>) -> Vec<Placed> {
    let services: Vec<_> = config
        .configured_services()
        .into_iter()
        .map(|service| service_with_project_root(service, project_roots))
        .collect();
    let mut placed = Vec::new();
    for environment in config.configured_project_environments(None) {
        let of_project = services
            .iter()
            .filter(|service| service.project_id == environment.project_id);
        for service in of_project {
            let service = if environment.primary {
                Some(service.clone())
            } else {
                launch_plan::environment_service(config, service.clone(), &environment)
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

fn key_of(placed: &Placed) -> ServiceKey {
    ServiceKey {
        project_id: placed.service.project_id.clone(),
        environment: Some(placed.environment.slug.clone()),
        service_id: placed.service.id.clone(),
    }
}

/// Where `localify dev` reads the service's launch profiles: its plan's
/// project root, or the configured one when no plan resolves.
fn profiles_root(service: &ConfiguredService, plan: Option<&LaunchPlan>) -> Option<PathBuf> {
    match plan {
        Some(plan) => Some(plan.project_root.clone()),
        None => service.project_root_path.as_deref().map(PathBuf::from),
    }
}

fn load_profiles(root: Option<&Path>) -> Result<Option<LaunchProfiles>> {
    match root {
        Some(root) => LaunchProfiles::load(root),
        None => Ok(None),
    }
}

fn service_facts(
    placed: Placed,
    reachable: Option<bool>,
    plan: &Result<LaunchPlan>,
    profiles: &Result<Option<LaunchProfiles>>,
    record: Option<&RunningService>,
    observed: &Observed<'_>,
) -> Result<ServiceFacts> {
    let environment_slug = placed.environment.slug.clone();
    let Placed {
        environment,
        service,
        port,
    } = placed;
    let definition = LaunchDefinition::new(&service, Some(&environment_slug), plan, profiles, None);
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
    let legacy = record.is_legacy();
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

/// The origins at which this machine's public URLs reach `domain`: those on
/// that domain first, then a wildcard URL on its parent at `<slug>--<label>`.
/// Only URLs this machine serves and that are not taken down, since another
/// machine's URL reaches another checkout, and only addresses that start
/// with their slug, as the ingress matches them.
pub fn public_origins(domain: &str, urls: &[PublicUrl], machine_id: &str) -> Vec<String> {
    let domain = domain.trim();
    let domain = domain
        .strip_prefix("*.")
        .unwrap_or(domain)
        .to_ascii_lowercase();
    let served: Vec<&PublicUrl> = urls
        .iter()
        .filter(|url| {
            url.machine
                .as_ref()
                .is_some_and(|machine| machine.id == machine_id)
                && url.state != Some(PublicUrlState::TakenDown)
        })
        .collect();
    let exact = served
        .iter()
        .filter(|url| url.local_domain.eq_ignore_ascii_case(&domain))
        .filter_map(|url| own_origin(url));
    let environments = served.iter().filter(|url| url.wildcard).filter_map(|url| {
        own_origin(url)?;
        let parent = format!(".{}", url.local_domain.to_ascii_lowercase());
        environment_address(url, domain.strip_suffix(&parent)?)
    });
    let mut origins = Vec::new();
    for origin in exact.chain(environments) {
        if !origins.contains(&origin) {
            origins.push(origin);
        }
    }
    origins
}

/// `https://<slug>.<base>`, when the URL's address starts with its slug.
fn own_origin(url: &PublicUrl) -> Option<String> {
    let host = url
        .url
        .strip_prefix("https://")?
        .split('/')
        .next()?
        .to_ascii_lowercase();
    host.strip_prefix(&format!("{}.", url.slug.to_ascii_lowercase()))?;
    Some(format!("https://{host}"))
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

/// Connects to each port on 127.0.0.1 from at most [`PROBE_WORKERS`] threads
/// until `deadline` passes. A port missing from the answer was not probed in
/// time, or its probe failed: it is unknown. A thread that cannot be created
/// leaves its share to the others, or every port unknown; nothing panics.
fn probe_ports(ports: BTreeSet<u16>, deadline: Duration, gauge: &ProbeGauge) -> HashMap<u16, bool> {
    let ports: Vec<u16> = ports.into_iter().collect();
    let next = AtomicUsize::new(0);
    let results = Mutex::new(HashMap::new());
    let until = Instant::now() + deadline;
    thread::scope(|scope| {
        let workers: Vec<_> = (0..PROBE_WORKERS.min(ports.len()))
            .map_while(|_| {
                thread::Builder::new()
                    .name("inventory-probe".into())
                    .spawn_scoped(scope, || {
                        let _slot = gauge.enter();
                        probe_worker(&ports, &next, &results, until);
                    })
                    .ok()
            })
            .collect();
        // Joined by hand, so a worker's panic is not the scope's.
        for worker in workers {
            let _ = worker.join();
        }
    });
    results
        .into_inner()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn probe_worker(
    ports: &[u16],
    next: &AtomicUsize,
    results: &Mutex<HashMap<u16, bool>>,
    until: Instant,
) {
    loop {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return;
        }
        let Some(&port) = ports.get(next.fetch_add(1, Ordering::SeqCst)) else {
            return;
        };
        let address = SocketAddr::from(([127, 0, 0, 1], port));
        let reachable = TcpStream::connect_timeout(&address, PORT_TIMEOUT.min(left)).is_ok();
        lock(results).insert(port, reachable);
    }
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

    /// Ports below the ephemeral range, which answer or refuse at once on
    /// loopback. Nothing is bound for them, so no other test, whose free
    /// ports are ephemeral, finds one of its ports taken.
    fn low_ports(count: u16) -> BTreeSet<u16> {
        (1024..1024 + count).collect()
    }

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
            gather(&Observed {
                config,
                store: &self.store(),
                key: &self.key(),
                public: &PublicFacts::default(),
                project_roots: &HashMap::new(),
                probes: &ProbeGauge::default(),
            })
            .unwrap()
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
        assert_eq!(item.port.reachable, Some(false));
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

        assert_eq!(item.port.reachable, Some(true));
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

        let snapshot = inventory
            .snapshot(&PublicFacts::default(), &HashMap::new())
            .unwrap();

        assert_eq!(snapshot.items.len(), 2);
        assert!(snapshot.items.iter().all(|item| {
            item.controls.start.code == Some(Refusal::ControlDisabled)
                && item.controls.restart.code == Some(Refusal::ControlDisabled)
        }));
        let again = inventory
            .snapshot(&PublicFacts::default(), &HashMap::new())
            .unwrap();
        assert_eq!(again.rev, 1);
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

    #[test]
    fn public_origins_are_the_hosts_this_machine_serves_for_the_domain() {
        let here = r#","machine":{"id":"m1","name":"laptop"},"state":"serving""#;
        let exact = public_url("exact", "web.localify", false, here);
        let namespace = public_url("ns", "app.localify", true, here);
        let origins = |domain: &str, urls: &[PublicUrl]| public_origins(domain, urls, "m1");

        assert_eq!(
            origins("web.localify", std::slice::from_ref(&exact)),
            ["https://app-a8f3.localify.fyi"]
        );
        let urls = [namespace];
        assert_eq!(
            origins("api.app.localify", &urls),
            ["https://app-a8f3--api.localify.fyi"]
        );
        assert_eq!(
            origins("app.localify", &urls),
            ["https://app-a8f3.localify.fyi"]
        );
        assert!(origins("a.b.app.localify", &urls).is_empty());
        assert!(origins("other.localify", &urls).is_empty());
    }

    #[test]
    fn public_origins_leave_out_other_machines_and_foreign_addresses() {
        let elsewhere = r#","machine":{"id":"m2","name":"desktop"},"state":"serving""#;
        let taken_down = r#","machine":{"id":"m1","name":"laptop"},"state":"taken_down""#;
        let foreign: PublicUrl = serde_json::from_str(
            r#"{"id":"evil","slug":"app-a8f3","url":"https://evil.example",
                "localDomain":"app.localify","wildcard":true,
                "machine":{"id":"m1","name":"laptop"},
                "protection":{"mode":"password"},"createdAt":"2026-11-05T18:00:00Z"}"#,
        )
        .unwrap();
        let urls = [
            public_url("other", "app.localify", true, elsewhere),
            public_url("down", "app.localify", true, taken_down),
            public_url("unserved", "app.localify", true, r#","machine":null"#),
            foreign,
        ];

        assert!(public_origins("app.localify", &urls, "m1").is_empty());
        assert!(public_origins("api.app.localify", &urls, "m1").is_empty());
    }

    #[test]
    fn changing_only_the_package_manager_changes_the_revision() {
        let machine = Machine::new();
        let manifest = machine.path("app").join("package.json");
        let with_manager = |manager: &str| {
            format!(r#"{{"packageManager":"{manager}","scripts":{{"dev":"vite"}}}}"#)
        };
        fs::write(&manifest, with_manager("pnpm@9.0.0")).unwrap();
        let pnpm = machine.primary().service.config_rev;
        let again = machine.primary().service.config_rev;

        fs::write(&manifest, with_manager("npm@10.0.0")).unwrap();
        let npm = machine.primary().service.config_rev;

        assert_eq!(pnpm, again);
        assert_ne!(pnpm, npm);
    }

    #[test]
    fn the_revision_covers_the_plan_dev_would_run() {
        let machine = Machine::new();
        fs::write(
            machine.path("app").join("package.json"),
            r#"{"packageManager":"pnpm@9.0.0","scripts":{"dev":"vite"}}"#,
        )
        .unwrap();
        let service = machine
            .config()
            .configured_services()
            .into_iter()
            .next()
            .unwrap();

        let plan = launch_plan::dev_plan(service, machine.port).unwrap();

        let port = machine.port.to_string();
        assert_eq!(
            plan.argv,
            ["pnpm", "run", "dev", "--host", "127.0.0.1", "--port", &port]
        );
        assert_eq!(plan.injected, ["--host", "127.0.0.1", "--port", &port]);
        assert_eq!(plan.workdir, machine.path("app"));
    }

    #[test]
    fn many_ports_are_probed_by_a_bounded_pool_within_the_deadline() {
        let ports = low_ports(500);
        let gauge = ProbeGauge::default();

        let started = Instant::now();
        let probed = probe_ports(ports.clone(), PROBE_DEADLINE, &gauge);

        assert!(started.elapsed() < PROBE_DEADLINE + Duration::from_millis(500));
        assert!(gauge.peak() <= PROBE_WORKERS, "{}", gauge.peak());
        assert!(probed.keys().all(|port| ports.contains(port)));
        assert!(!probed.is_empty());
    }

    #[test]
    fn ports_not_probed_before_the_deadline_are_unknown() {
        let probed = probe_ports(low_ports(20), Duration::ZERO, &ProbeGauge::default());

        assert!(probed.is_empty());
    }

    #[test]
    fn overlapping_snapshots_share_one_build_at_a_time() {
        let machine = Machine::new();
        let mut config = machine.config_toml();
        for (index, port) in low_ports(500).into_iter().enumerate() {
            config.push_str(&format!(
                r#"
[domains."s{index}.localify"]
pattern = "s{index}.localify"
project = "many"

[[domains."s{index}.localify".routes]]
path = "/"
target = "127.0.0.1:{port}"
"#
            ));
        }
        config.push_str("\n[projects.many]\nname = \"Many\"\n");
        fs::write(machine.path("config.toml"), config).unwrap();
        let inventory = LocalInventory::new(machine.path("config.toml"), machine.path("data"))
            .unwrap()
            .with_services_dir(machine.path("services"));

        let started = Instant::now();
        let snapshots: Vec<Snapshot> = thread::scope(|scope| {
            let requests: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        inventory
                            .snapshot(&PublicFacts::default(), &HashMap::new())
                            .unwrap()
                    })
                })
                .collect();
            requests
                .into_iter()
                .map(|request| request.join().unwrap())
                .collect()
        });

        assert!(started.elapsed() < Duration::from_secs(10));
        let peak = inventory.probes.peak();
        assert!(peak > 0 && peak <= PROBE_WORKERS, "{peak}");
        assert!(inventory.built.load(Ordering::SeqCst) < 8, "shared builds");
        assert!(snapshots.iter().all(|snapshot| snapshot.rev == 1));
        assert_eq!(snapshots[0].items.len(), 502);
    }
}
