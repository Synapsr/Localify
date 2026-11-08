use std::collections::{BTreeSet, HashMap};
use std::env;
use std::fs;
use std::io::{self, BufRead, IsTerminal, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use dialoguer::{Select, theme::ColorfulTheme};
use localify::desktop::ProjectRootStore;
use localify::project_init::{LocalAppUpdatePolicy, detect_folder_init_plan};
use serde::Serialize;

use super::context::{AppContext, effective_current_dir};
use super::init_project::{self, InitProjectArgs};
use super::launch_profile::{self, LaunchProfileArgs, PreparedProfile, SessionTarget};
use super::project_env::{current_worktree_environment, ensure_implicit_worktree_environment};
use super::service_supervisor::{
    self, Capture, LaunchEnd, LaunchOutcome, SupervisedLaunch, Supervision,
};
use crate::application::ports::DaemonControl;
use crate::branding::BRANDING;
use crate::domain::launch_profile::{LaunchEnvironment, layer_environment};
use crate::domain::value_objects::{ProxyTarget, RouteTarget};
use crate::infrastructure::config::{ConfiguredProjectEnvironment, ConfiguredService};
use crate::infrastructure::inventory::public_origins;
#[cfg(target_os = "macos")]
use crate::infrastructure::ios_simulator_trust::trust_booted_ios_simulators;
use crate::infrastructure::launch_plan::{
    self, LaunchCommand, LaunchPlan, apply_local_dev_launch, local_dev_launch_for_service,
    service_cwd_match_path, service_with_dev_launch, service_with_local_dev_launch,
    service_with_project_root, service_workdir,
};
use crate::infrastructure::paths::LocalifyPaths;
use crate::infrastructure::project_marker::{
    ensure_marker_is_excluded, read_project_marker, write_project_marker,
};
use crate::infrastructure::running_services::{
    Admission, NewRun, RunLock, RunOutput, RunningService, RunningServiceStore, SERVICE_ORIGIN_ENV,
    ServiceKey, ServiceOrigin,
};
use crate::infrastructure::runs::RunId;
use crate::infrastructure::runs::finish::Finisher;
use crate::infrastructure::runs::output::{
    self as run_output, EndReason, FinalizeLock, RunEnd, StreamSink, StreamWriter,
};
use localify::project_init::{
    apply_next_allowed_dev_origin, apply_vite_allowed_host, find_existing_next_config_path,
    find_existing_vite_config_path,
};

const DEFAULT_URL_ENV_KEYS: &[&str] = &[
    "LOCALIFY_URL",
    "LOCALIFY_ORIGIN",
    "NEXT_PUBLIC_APP_URL",
    "AUTH_URL",
    "NEXTAUTH_URL",
    "BETTER_AUTH_URL",
    "APP_URL",
    "SITE_URL",
    "VITE_LOCALIFY_URL",
    "VITE_APP_URL",
    "VITE_PUBLIC_APP_URL",
    "VITE_SITE_URL",
];

/// Better Auth adds it to its `trustedOrigins` option, split on commas.
const TRUSTED_ORIGINS_ENV_KEY: &str = "BETTER_AUTH_TRUSTED_ORIGINS";
const PUBLIC_URL_ENV_KEY: &str = "LOCALIFY_PUBLIC_URL";

const DEFAULT_DOMAIN_ENV_KEYS: &[&str] = &[
    "LOCALIFY_DOMAIN",
    "NEXT_PUBLIC_APP_DOMAIN",
    "VITE_APP_DOMAIN",
];

const ENV_FILE_NAMES: &[&str] = &[
    ".env",
    ".env.local",
    ".env.development",
    ".env.development.local",
    ".env.localify",
    ".env.localify.local",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    Run,
    Dev,
}

impl RunMode {
    fn command_name(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::Dev => "dev",
        }
    }
}

pub struct RunArgs {
    pub mode: RunMode,
    pub service_name: Option<String>,
    pub project: Option<String>,
    pub environment: Option<String>,
    pub init_domain: Option<String>,
    pub override_command: Vec<String>,
    pub launch_profile: LaunchProfileArgs,
    /// The run id a dispatcher chose (`--launch-id`); `None` picks one.
    pub launch_id: Option<String>,
    /// What to do when the service already runs.
    pub replace: Replace,
}

pub fn list(ctx: &AppContext, json: bool) -> Result<()> {
    let ports = daemon_ports(ctx)?;
    let services = ctx.config_store.list_services()?;

    if json {
        let output = services
            .iter()
            .map(|service| {
                let endpoint = service_endpoint(service, ports, ctx)?;
                Ok(ServiceOutput::from_service(service, endpoint))
            })
            .collect::<Result<Vec<_>>>()?;
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    if services.is_empty() {
        println!("No services configured.");
        println!("\nCreate a project with:");
        println!("  {} init-project my-app", BRANDING.cli_command());
        return Ok(());
    }

    println!("Services:\n");
    for service in services {
        let endpoint = service_endpoint(&service, ports, ctx)?;
        println!(
            "  {} [{}] -> {}",
            service.name,
            service.id,
            service_url(&service, endpoint)
        );
        println!(
            "    project: {} [{}]{}",
            service.project_name,
            service.project_id,
            if service.project_enabled {
                ""
            } else {
                " (disabled)"
            }
        );
        println!("    primary domain: {}", service.primary_domain);

        let additional_domains = service
            .domains
            .iter()
            .filter(|domain| domain.as_str() != service.primary_domain)
            .collect::<Vec<_>>();
        if !additional_domains.is_empty() {
            println!(
                "    additional domains: {}",
                additional_domains
                    .iter()
                    .map(|domain| domain.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }

        if let Some(target) = &service.primary_target {
            println!("    target: {target}");
        }
        if let Some(path) = &service.path {
            println!("    path: {path}");
        }
        if let Some(command) = &service.command {
            println!("    command: {}", command.join(" "));
        }
        println!(
            "    source: {}",
            if service.inferred {
                "inferred"
            } else {
                "explicit"
            }
        );
        println!();
    }

    Ok(())
}

pub fn get(
    service_name: String,
    project: Option<String>,
    environment: Option<String>,
    json: bool,
    ctx: &AppContext,
) -> Result<()> {
    let ports = daemon_ports(ctx)?;
    let service = resolve_service(&service_name, project.as_deref(), ctx)?;
    let service = resolve_environment_service(service, environment.as_deref(), ctx)?;
    let endpoint = service_endpoint(&service, ports, ctx)?;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&ServiceOutput::from_service(&service, endpoint))?
        );
        return Ok(());
    }

    println!("{}", service_url(&service, endpoint));
    Ok(())
}

pub fn run(
    args: RunArgs,
    config_path: &Path,
    policy: LocalAppUpdatePolicy,
    ctx: &AppContext,
    paths: &LocalifyPaths,
) -> Result<i32> {
    let RunArgs {
        mode,
        service_name,
        project,
        environment,
        init_domain,
        override_command,
        launch_profile,
        launch_id,
        replace,
    } = args;
    let run_id = match launch_id.as_deref() {
        Some(id) => RunId::parse(id)?,
        None => RunId::generate()?,
    };
    if init_domain.is_some()
        && (service_name.is_some() || project.is_some() || environment.is_some())
    {
        bail!(
            "--domain only applies when '{} dev' initializes a project from the current folder. For an existing service, use '{} project domain set <service> <domain>'.",
            BRANDING.cli_command(),
            BRANDING.cli_command()
        );
    }

    let has_override_command = !override_command.is_empty();
    let ports = daemon_ports(ctx)?;
    let (service, effective_environment) = resolve_runnable_service(
        RunTarget {
            service: service_name.as_deref(),
            project: project.as_deref(),
            environment: environment.as_deref(),
            init_domain: init_domain.as_deref(),
        },
        mode,
        config_path,
        policy,
        ctx,
    )?;
    let effective_environment = effective_environment.as_deref();

    let command = if !has_override_command {
        LaunchCommand::Detected(service.command.clone().ok_or_else(|| {
            anyhow::anyhow!(
                "Service '{}' has no detected command. Run it with an override, for example: {} {} {} -- pnpm dev",
                service.id,
                BRANDING.cli_command(),
                mode.command_name(),
                service.id
            )
        })?)
    } else {
        LaunchCommand::Override(override_command)
    };

    let proxy_target = service_proxy_target(&service)?;
    if !is_supported_run_host(proxy_target.host()) {
        bail!(
            "Service '{}' points to '{}'. {} {} injects HOST=127.0.0.1 and only supports route targets on 127.0.0.1 or localhost in this slice. Update the route target or run the service manually.",
            service.id,
            proxy_target,
            BRANDING.cli_command(),
            mode.command_name()
        );
    }

    let port = proxy_target.port().value();
    let key = service_key(&service, effective_environment, ctx);
    // Before anything runs, profile sources included: one executor per service.
    let store = RunningServiceStore::new_default()?;
    let lock = admit(&store, &key, replace)?;
    ensure_port_available(port, &service)?;

    let LaunchPlan {
        argv: command,
        workdir,
        project_root,
        ..
    } = launch_plan::resolve(&service, command, port)?;
    warn_if_daemon_is_stopped(ctx);

    // Before the profile resolves: a stop meanwhile ends the helpers it runs.
    let origin = run_origin(launch_id.is_some());
    let supervision = Supervision::start(origin == ServiceOrigin::App);
    let profile = launch_profile::prepare(
        &launch_profile,
        &project_root,
        &workdir,
        &effective_current_dir()?,
    );
    // Also when it failed: a stop that refused its next helper is still
    // ending the others, and `localify dev` exits only once it has.
    supervision.resolved();
    let profile = profile?;

    let endpoint = service_endpoint(&service, ports, ctx)?;
    let url = service_url(&service, endpoint);
    let app_host = service_public_host(&service);
    let app_domain = service_public_domain(&service, endpoint);
    ensure_next_allowed_dev_origin(&workdir, &app_host);
    ensure_vite_allowed_host(&workdir, &app_host);
    let mut url_envs =
        url_environment_overrides(&workdir, &url, &app_domain, &profile_keys(profile.as_ref()))?;
    url_envs.extend(public_url_overrides(
        &app_host,
        &[workdir.as_path(), project_root.as_path()],
        profile.as_ref(),
        ctx,
    )?);
    eprintln!("Starting '{}' at {}", service.id, url);
    eprintln!("  route target: {proxy_target}");
    eprintln!("  injected HOST: 127.0.0.1");
    eprintln!("  injected PORT: {port}");
    eprintln!("  injected LOCALIFY_URL: {url}");
    eprintln!("  injected URL envs: {}", env_key_list(&url_envs));
    eprintln!("  cwd: {}", workdir.display());
    if let Some(profile) = &profile {
        launch_profile::print_summary(profile);
    }

    let ca_cert_path = paths.data_dir.join("ca.crt");
    let environment = service_environment(port, &url_envs, &ca_cert_path, profile.as_ref());
    if profile.is_some() {
        // The profile's trusted origins are merged in, not replaced.
        let replaced: Vec<String> = environment
            .replaced_by_runtime
            .iter()
            .filter(|key| *key != TRUSTED_ORIGINS_ENV_KEY)
            .cloned()
            .collect();
        launch_profile::print_conflicts(&environment.shadowed_by_shell, &replaced);
    }
    eprintln!("$ {}", command.join(" "));
    trust_ios_simulators_for_dev(mode, &ca_cert_path);
    let background = origin == ServiceOrigin::Background;
    let record = store.register(
        &lock,
        NewRun {
            key: key.clone(),
            run_id: run_id.clone(),
            url,
            port,
            origin,
            profile: profile
                .as_ref()
                .and_then(|profile| profile.name())
                .map(str::to_string),
            output: match origin {
                ServiceOrigin::Background => Some(RunOutput::Pipe),
                ServiceOrigin::App => Some(RunOutput::Pty),
                ServiceOrigin::Terminal => None,
            },
        },
    )?;

    let stream = background
        .then(|| StreamWriter::create(&store.run_dir(&run_id)).map(StreamSink::new))
        .transpose();
    let run = RegisteredRun {
        store: &store,
        record: &record,
        stream: stream.as_ref().ok().cloned().flatten(),
    };
    let end = stream.and_then(|stream| {
        supervise(Supervised {
            profile: profile.as_ref(),
            service: &service,
            key: &key,
            command: &command,
            workdir: &workdir,
            environment: &environment,
            stream,
            run: &run,
            socket_path: &paths.socket_path,
            supervision,
        })
    });
    run.finish(end.as_ref().ok())?;
    drop(lock);
    launch_outcome(profile.as_ref(), &service, end?.outcome)
}

/// Who started this `localify dev`. The app's run PTY, which reads the run's
/// output, always passes the launch id it chose: without one, `app` comes
/// from a shell tab of an app from before run PTYs, a terminal like any other.
fn run_origin(dispatched: bool) -> ServiceOrigin {
    match ServiceOrigin::from_env(env::var(SERVICE_ORIGIN_ENV).ok().as_deref()) {
        ServiceOrigin::App if !dispatched => ServiceOrigin::Terminal,
        origin => origin,
    }
}

/// A run whose record is written: whatever happens next, its processes are
/// ended before `localify dev` exits, and it is finalized unless the app
/// reads its run PTY and finalizes it once it has read it all.
struct RegisteredRun<'a> {
    store: &'a RunningServiceStore,
    record: &'a RunningService,
    stream: Option<StreamSink>,
}

impl RegisteredRun<'_> {
    fn heartbeat(&self) {
        if let Some(run_id) = self.record.run_id() {
            let _ = self.store.heartbeat(&self.record.key, run_id);
        }
    }

    /// The executor's own end of its run: the stream finalized, `complete`
    /// unless some output could not be read, then the exit recorded. A run
    /// PTY's stream is left to the app. A run whose processes outlived the
    /// cleanup stays unfinalized, so its next launch is refused as for a live
    /// run.
    fn finish(&self, end: Option<&LaunchEnd>) -> Result<()> {
        let Some(run_id) = self.record.run_id() else {
            return Ok(());
        };
        if let Some(Err(error)) = end.map(|end| &end.cleanup) {
            bail!(
                "'{}' stopped, but {error:#}\nIts run stays open, and it cannot start again until they are gone.",
                self.record.key.service_id
            );
        }
        let dir = self.store.run_dir(run_id);
        if self.record.origin == ServiceOrigin::App {
            self.leave_to_reader(&dir)?;
        } else if let Some(_finalizing) = FinalizeLock::acquire(&dir)? {
            let offset = self.stream.as_ref().map_or(0, StreamSink::close);
            let capture = end.map_or(Capture::Complete, |end| end.capture.clone());
            let mut finished = match capture {
                Capture::Complete => RunEnd::now(EndReason::Complete, offset),
                Capture::Unfinished | Capture::Failed(_) => {
                    RunEnd::now(EndReason::Interrupted, offset)
                }
            };
            if let Capture::Failed(error) = capture {
                service_supervisor::report(&format!(
                    "Warning: writing the output of '{}' failed ({error}). The service ran on, and what it printed after that was dropped.\n",
                    self.record.key.service_id
                ));
                finished.error = Some(error);
            }
            if run_output::read_end(&dir)?.is_none() {
                run_output::publish_end(&dir, &finished)?;
            }
        }
        let code = end.and_then(|end| end.outcome.code());
        self.store.mark_exited(&self.record.key, run_id, code)
    }
}

impl RegisteredRun<'_> {
    /// A run PTY's stream is the app's to finalize, once it has read it all.
    /// When the app is gone, nobody reads the PTY any more: what it wrote is
    /// finalized `interrupted` here, so the run ends without waiting for
    /// another finisher.
    fn leave_to_reader(&self, dir: &Path) -> Result<()> {
        if run_output::reader_alive(dir)? {
            return Ok(());
        }
        if let Some(_finalizing) = FinalizeLock::acquire(dir)?
            && run_output::read_end(dir)?.is_none()
            && !run_output::reader_alive(dir)?
        {
            let (_, written) = run_output::extent(dir)?;
            run_output::publish_end(dir, &RunEnd::now(EndReason::Interrupted, written))?;
        }
        Ok(())
    }
}

struct Supervised<'a> {
    profile: Option<&'a PreparedProfile>,
    service: &'a ConfiguredService,
    key: &'a ServiceKey,
    command: &'a [String],
    workdir: &'a Path,
    environment: &'a LaunchEnvironment,
    stream: Option<StreamSink>,
    run: &'a RegisteredRun<'a>,
    socket_path: &'a Path,
    supervision: Supervision,
}

/// Runs the service under the supervisor, with or without a launch profile.
fn supervise(launch: Supervised<'_>) -> Result<LaunchEnd> {
    let session = match launch.profile {
        Some(profile) => launch_profile::start_launch_session(
            profile,
            SessionTarget {
                project_id: launch.service.project_id.clone(),
                environment_slug: launch.key.environment.clone(),
                service_id: launch.service.id.clone(),
                target_port: launch.run.record.port,
                browser_indicator: profile.resolved.browser_indicator,
            },
            launch.socket_path,
        )?,
        None => None,
    };
    let dangerous_name = launch
        .profile
        .filter(|profile| profile.is_dangerous())
        .and_then(|profile| profile.name().map(|name| (profile, name)));
    if let Some((profile, name)) = dangerous_name {
        let service_label = format!("{} ({})", launch.service.id, launch.service.project_name);
        launch_profile::print_danger_banner(profile, &service_label);
        launch_profile::set_danger_title(name, &launch.service.id);
    }

    let heartbeat = || launch.run.heartbeat();
    let end = service_supervisor::run(SupervisedLaunch {
        command: launch.command,
        workdir: launch.workdir,
        environment: launch.environment,
        mask_patterns: launch.profile.and_then(PreparedProfile::mask_patterns),
        stream: launch.stream,
        max_duration: launch.profile.and_then(PreparedProfile::max_duration),
        session,
        heartbeat: &heartbeat,
        heartbeat_every: service_supervisor::RUN_HEARTBEAT,
        finisher: &Finisher::native(),
        leads_session: launch
            .run
            .record
            .run
            .as_ref()
            .is_some_and(|run| run.sid.is_some()),
        app_terminal: launch.run.record.origin == ServiceOrigin::App,
        supervision: Some(launch.supervision),
    });
    if dangerous_name.is_some() {
        launch_profile::reset_title();
    }
    end
}

/// What `dev` was asked to run, before resolution.
#[derive(Clone, Copy)]
pub(crate) struct RunTarget<'a> {
    pub service: Option<&'a str>,
    pub project: Option<&'a str>,
    pub environment: Option<&'a str>,
    pub init_domain: Option<&'a str>,
}

/// Finds the service `dev` runs, creating the project from the current folder
/// and the environment of the current worktree when they do not exist yet.
/// Returns it with its environment slug.
pub(crate) fn resolve_runnable_service(
    target: RunTarget<'_>,
    mode: RunMode,
    config_path: &Path,
    policy: LocalAppUpdatePolicy,
    ctx: &AppContext,
) -> Result<(ConfiguredService, Option<String>)> {
    // A moved project folder would otherwise resolve to nothing and trigger a
    // duplicate auto-init below; the in-project marker re-links it first.
    if let Ok(cwd) = effective_current_dir()
        && let Some(healed) = super::project::heal_moved_project_root(&ctx.config_store, &cwd)
    {
        eprintln!(
            "Re-linked project '{}' [{}] to '{}' (folder moved).",
            healed.project_name,
            healed.project_id,
            healed.root_path.display()
        );
    }

    let implicit_environment = if target.environment.is_none() {
        ensure_implicit_worktree_environment(target.project, ctx)?
    } else {
        None
    };
    let effective_project = target.project.or_else(|| {
        implicit_environment
            .as_ref()
            .map(|environment| environment.project_id.as_str())
    });
    let effective_environment = target.environment.or_else(|| {
        implicit_environment
            .as_ref()
            .map(|environment| environment.slug.as_str())
    });

    if let Some(environment) = &implicit_environment {
        eprintln!(
            "Using project environment '{}' [{}] for worktree.",
            environment.name, environment.slug
        );
    }

    ensure_run_context_is_initialized(
        target.service,
        effective_project,
        effective_environment,
        target.init_domain,
        config_path,
        policy,
        ctx,
    )?;

    let service = resolve_run_service(
        target.service,
        effective_project,
        effective_environment,
        ctx,
    )?;
    let worktree = effective_environment
        .filter(|_| is_worktree_environment(&service.project_id, effective_environment, ctx));
    ensure_service_folder_exists(&service, worktree)?;
    let service = service_with_launch_for_mode(service, mode);
    if !service.project_enabled {
        bail!(
            "Service '{}' belongs to disabled project '{}'. Enable the project before running it.",
            service.id,
            service.project_id
        );
    }

    // Backfill the marker for pre-marker projects so a future move can
    // auto-heal. Never overwrites an existing marker, but keeps it out of Git
    // again, for a project that became a repository after it got its marker.
    // A worktree gets none: it would show as a change there, and a moved
    // project could heal to it.
    if !is_worktree_environment(&service.project_id, effective_environment, ctx)
        && let Some(root) = service.project_root_path.as_deref()
    {
        let root = Path::new(root);
        if root.is_dir() {
            let _ = match read_project_marker(root) {
                None => write_project_marker(root, &service.project_id),
                Some(_) => ensure_marker_is_excluded(root),
            };
        }
    }

    Ok((service, effective_environment.map(str::to_string)))
}

/// A project or worktree folder that is gone leaves nothing to run: says so,
/// with the fix, instead of failing later on a command it cannot find there.
/// `worktree` names the environment when the service runs in a worktree.
fn ensure_service_folder_exists(service: &ConfiguredService, worktree: Option<&str>) -> Result<()> {
    let Some(root) = service.project_root_path.as_deref() else {
        return Ok(());
    };
    if Path::new(root).is_dir() {
        return Ok(());
    }
    let cli = BRANDING.cli_command();
    let project = &service.project_id;
    match worktree {
        Some(environment) => bail!(
            "Environment '{environment}' of project '{project}' is in {root}, which no longer exists. If the worktree was deleted, remove the environment with: {cli} project env prune --project {project}"
        ),
        None => bail!(
            "Project '{project}' is in {root}, which no longer exists. If the project moved, point Localify at its new folder: {cli} project update {project} --root <folder>"
        ),
    }
}

/// A service resolved for a launch somewhere else than this process: the app's
/// terminal or the background.
pub(crate) struct LaunchTarget {
    pub key: ServiceKey,
    pub url: String,
    pub port: u16,
}

pub(crate) fn resolve_launch_target(
    target: RunTarget<'_>,
    config_path: &Path,
    ctx: &AppContext,
) -> Result<LaunchTarget> {
    let ports = daemon_ports(ctx)?;
    let (service, environment) = resolve_runnable_service(
        target,
        RunMode::Dev,
        config_path,
        LocalAppUpdatePolicy::runtime_only(),
        ctx,
    )?;
    let port = service_proxy_target(&service)?.port().value();
    let url = service_url(&service, service_endpoint(&service, ports, ctx)?);
    Ok(LaunchTarget {
        key: service_key(&service, environment.as_deref(), ctx),
        url,
        port,
    })
}

fn service_key(
    service: &ConfiguredService,
    environment: Option<&str>,
    ctx: &AppContext,
) -> ServiceKey {
    ServiceKey {
        project_id: service.project_id.clone(),
        environment: environment_slug(service, environment, ctx),
        service_id: service.id.clone(),
    }
}

/// What a launch does when the service already runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Replace {
    /// Ask the person at the terminal.
    Ask,
    /// Stop whatever runs, and start again (`--replace`).
    Yes,
    /// Stop that run only, which a person agreed to stop (`--replace=<runId>`).
    Approved(RunId),
    /// Leave it running, and start nothing.
    No,
}

impl Replace {
    /// `--replace` stops it, `--replace=<runId>` only that run. Otherwise a
    /// person at a terminal is asked, and anything else (an agent, a script,
    /// CI, `< /dev/null`) leaves it.
    pub fn for_launch(flag: Option<Option<&str>>) -> Result<Self> {
        Ok(match flag {
            Some(Some(run_id)) => Self::Approved(RunId::parse(run_id)?),
            Some(None) => Self::Yes,
            None if io::stdin().is_terminal() && io::stderr().is_terminal() => Self::Ask,
            None => Self::No,
        })
    }

    /// Whether to stop `running`, asking on the terminal when it should.
    pub(crate) fn stops(&self, running: &RunningService) -> Result<bool> {
        match self {
            Self::Yes => Ok(true),
            Self::Approved(run_id) => Ok(running.run_id() == Some(run_id)),
            Self::No => Ok(false),
            Self::Ask => confirm_replace(running, &mut io::stdin().lock(), &mut io::stderr()),
        }
    }
}

/// How long a replacing launch waits for the run it stopped to let go of the
/// service's lock.
const RELEASE_TIMEOUT: Duration = Duration::from_secs(10);

/// Takes the service's lock for this launch. A run that is already going is
/// stopped, as `stop` would, and finalized first when `replace` says so. Only
/// the run chosen is replaced, the one a person agreed to stop or else the
/// one seen first: when the service holds another run once the lock is
/// free, someone else restarted it, and this launch starts nothing, so two
/// replacing launches never both start. A run of an older version is never
/// stopped here.
pub(crate) fn admit(
    store: &RunningServiceStore,
    key: &ServiceKey,
    replace: Replace,
) -> Result<RunLock> {
    let mut chosen = match &replace {
        Replace::Approved(run_id) => Some(run_id.clone()),
        _ => None,
    };
    let mut stopped_at: Option<Instant> = None;
    loop {
        let running = match store.lock(key)? {
            Admission::Admitted(lock) => {
                // The record only changes under this lock now.
                let current = store.get(key)?;
                let current = current.as_ref().and_then(RunningService::run_id);
                if let Some(chosen) = &chosen {
                    match current {
                        Some(current) if current == chosen => {}
                        Some(current) => bail!("{}", restarted_by_someone_else(key, current)),
                        None => bail!(
                            "'{}' is no longer running (run {chosen}), and nothing was started.",
                            key.service_id
                        ),
                    }
                }
                return Ok(lock);
            }
            Admission::Running(running) => running,
        };
        let Some(running) = running else {
            bail!(
                "'{}' is already running: another launch is starting it. Check it with: {} services status",
                key.service_id,
                BRANDING.cli_command()
            );
        };
        let Some(run_id) = running.run_id().cloned() else {
            bail!("{}", older_version_running(&running));
        };
        if let Some(chosen) = &chosen
            && *chosen != run_id
        {
            bail!("{}", restarted_by_someone_else(key, &run_id));
        }
        if let Some(since) = stopped_at {
            // The run just stopped is letting go of the lock.
            if since.elapsed() >= RELEASE_TIMEOUT {
                bail!(
                    "'{}' was stopped, but it still holds the service's lock (PID {}).",
                    key.service_id,
                    running.pid
                );
            }
            thread::sleep(Duration::from_millis(100));
            continue;
        }
        if !replace.stops(&running)? {
            bail!("{}", already_running(&running));
        }
        crate::infrastructure::test_pause::pause_at("replace-chosen");
        eprintln!(
            "Stopping '{}' (PID {}) to start it again...",
            key.service_id, running.pid
        );
        store.stop(&running)?;
        chosen = Some(run_id);
        stopped_at = Some(Instant::now());
    }
}

fn restarted_by_someone_else(key: &ServiceKey, run_id: &RunId) -> String {
    format!(
        "'{}' was restarted by someone else (run {run_id}), and is left as it is; nothing was started.",
        key.service_id
    )
}

/// A run with no run id, from before Localify could prove what is its own.
pub(crate) fn older_version_running(running: &RunningService) -> String {
    format!(
        "'{}' runs from an older version of {} (PID {}), which a launch does not stop. Stop it first with: {} stop {}",
        running.key.service_id,
        BRANDING.app_name,
        running.pid,
        BRANDING.cli_command(),
        crate::infrastructure::running_services::service_selector(&running.key)
    )
}

pub(crate) fn already_running(running: &RunningService) -> String {
    format!(
        "'{}' is already running (PID {}) at {}. Stop it first with: {} stop {}, or start it with --replace.",
        running.key.service_id,
        running.pid,
        running.url,
        BRANDING.cli_command(),
        crate::infrastructure::running_services::service_selector(&running.key)
    )
}

/// Asks whether to stop the running service and start it again. Enter or
/// `y` says yes; anything else, an end of input included, says no.
fn confirm_replace(
    running: &RunningService,
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> Result<bool> {
    let since = crate::infrastructure::process::lstart_label(running.started_at)
        .unwrap_or_else(|| "a while".into());
    write!(
        output,
        "{} is already running (since {since}, pid {}). Stop it and start again? [Y/n] ",
        running.key.service_id, running.pid
    )?;
    output.flush()?;
    let mut answer = String::new();
    if input.read_line(&mut answer)? == 0 {
        return Ok(false);
    }
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "" | "y" | "yes"
    ))
}

/// Which run of a service `stop`, `restart` and `logs` act on. Inside a
/// worktree, the service of that worktree's environment, like `dev`.
pub(crate) fn resolve_service_key(
    service_name: &str,
    project: Option<&str>,
    environment: Option<&str>,
    ctx: &AppContext,
) -> Result<ServiceKey> {
    let implicit = match environment {
        Some(_) => None,
        None => current_worktree_environment(project, ctx)?,
    };
    let project = project.or(implicit.as_ref().map(|env| env.project_id.as_str()));
    let environment = environment.or(implicit.as_ref().map(|env| env.slug.as_str()));
    let service = resolve_service(service_name, project, ctx)?;
    let service = resolve_environment_service(service, environment, ctx)?;
    Ok(service_key(&service, environment, ctx))
}

fn launch_outcome(
    profile: Option<&PreparedProfile>,
    service: &ConfiguredService,
    outcome: LaunchOutcome,
) -> Result<i32> {
    match outcome {
        LaunchOutcome::Exited(status) if !status.success() => {
            service_supervisor::report(&format!("Service '{}' exited with {status}\n", service.id));
            Ok(outcome.exit_code())
        }
        LaunchOutcome::Expired(duration) => bail!(
            "Service '{}' was stopped: launch profile '{}' reached its max_duration of {}.",
            service.id,
            profile.and_then(PreparedProfile::name).unwrap_or_default(),
            humantime::format_duration(duration)
        ),
        LaunchOutcome::Exited(_) | LaunchOutcome::Interrupted => Ok(outcome.exit_code()),
    }
}

/// Builds the service environment in priority order: Localify runtime
/// variables, then the shell, then the launch profile.
fn service_environment(
    port: u16,
    url_envs: &[(String, String)],
    ca_cert_path: &Path,
    profile: Option<&PreparedProfile>,
) -> LaunchEnvironment {
    let mut runtime = launch_profile::profile_runtime_variables(profile);
    runtime.set.push(("PORT".into(), port.to_string()));
    runtime.set.push(("HOST".into(), "127.0.0.1".into()));
    runtime.set.extend(url_envs.iter().cloned());
    if ca_cert_path.exists() {
        runtime.set.push((
            "NODE_EXTRA_CA_CERTS".into(),
            ca_cert_path.display().to_string(),
        ));
        runtime.unset.push("NODE_USE_SYSTEM_CA".into());
    }

    let profile_variables = profile
        .map(|profile| profile.resolved.pairs())
        .unwrap_or_default();
    layer_environment(
        &runtime,
        |key| env::var_os(key).is_some(),
        &profile_variables,
        &launch_profile::output_defaults(profile),
    )
}

fn profile_keys(profile: Option<&PreparedProfile>) -> Vec<String> {
    profile
        .map(|profile| {
            profile
                .resolved
                .variables
                .iter()
                .map(|variable| variable.key.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// Slug of the project environment the service runs in, so the desktop can
/// put the launch profile badge on the right row.
fn environment_slug(
    service: &ConfiguredService,
    requested: Option<&str>,
    ctx: &AppContext,
) -> Option<String> {
    let environments = ctx
        .config_store
        .list_project_environments(Some(&service.project_id))
        .ok()?;
    let environment = match requested {
        Some(value) => environments
            .iter()
            .find(|environment| environment_matches(environment, value)),
        None => environments.iter().find(|environment| environment.primary),
    };
    environment.map(|environment| environment.slug.clone())
}

#[cfg(target_os = "macos")]
fn trust_ios_simulators_for_dev(mode: RunMode, cert_path: &Path) {
    if mode != RunMode::Dev {
        return;
    }

    if let Err(error) = trust_booted_ios_simulators(cert_path) {
        eprintln!("Warning: could not trust the Localify CA in booted iOS Simulators: {error}");
    }
}

#[cfg(not(target_os = "macos"))]
fn trust_ios_simulators_for_dev(_mode: RunMode, _cert_path: &Path) {}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ServiceOutput {
    id: String,
    name: String,
    project_id: String,
    project_name: String,
    project_enabled: bool,
    primary_domain: String,
    url: String,
    domains: Vec<String>,
    path: Option<String>,
    command: Option<Vec<String>>,
    target: Option<String>,
    inferred: bool,
}

impl ServiceOutput {
    fn from_service(service: &ConfiguredService, endpoint: ServiceEndpoint) -> Self {
        Self {
            id: service.id.clone(),
            name: service.name.clone(),
            project_id: service.project_id.clone(),
            project_name: service.project_name.clone(),
            project_enabled: service.project_enabled,
            primary_domain: service.primary_domain.clone(),
            url: service_url(service, endpoint),
            domains: service.domains.clone(),
            path: service.path.clone(),
            command: service.command.clone(),
            target: service.primary_target.as_ref().map(ToString::to_string),
            inferred: service.inferred,
        }
    }
}

fn resolve_service(
    service_name: &str,
    project: Option<&str>,
    ctx: &AppContext,
) -> Result<ConfiguredService> {
    let project_roots = load_project_roots();
    let services = ctx
        .config_store
        .list_services()?
        .into_iter()
        .map(|service| service_with_project_root(service, &project_roots))
        .collect::<Vec<_>>();
    let cwd = effective_current_dir().ok();
    let cwd_project_id = project
        .is_none()
        .then(|| {
            cwd.as_deref()
                .and_then(|cwd| project_id_for_cwd(&services, cwd))
        })
        .flatten();

    let matches = services
        .into_iter()
        .filter(|service| service_matches(service, service_name))
        .filter(|service| {
            project
                .map(|project| project_matches(service, project))
                .unwrap_or(true)
        })
        .filter(|service| {
            cwd_project_id
                .as_deref()
                .map(|project_id| service.project_id == project_id)
                .unwrap_or(true)
        })
        .collect::<Vec<_>>();

    match matches.as_slice() {
        [service] => Ok(service.clone()),
        [] => {
            let scope = project
                .map(|project| format!(" in project '{project}'"))
                .or_else(|| cwd_project_id.map(|project_id| format!(" in project '{project_id}'")))
                .unwrap_or_default();
            bail!(
                "No service '{service_name}' found{scope}. Run '{} services list' to inspect configured services.",
                BRANDING.cli_command()
            );
        }
        services => {
            let options = services
                .iter()
                .map(|service| {
                    format!(
                        "  {} get --project {} {}",
                        BRANDING.cli_command(),
                        service.project_id,
                        service.id
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            bail!("Service '{service_name}' is ambiguous. Use --project:\n{options}");
        }
    }
}

fn resolve_run_service(
    service_name: Option<&str>,
    project: Option<&str>,
    environment: Option<&str>,
    ctx: &AppContext,
) -> Result<ConfiguredService> {
    if let Some(service_name) = service_name {
        let service = resolve_service(service_name, project, ctx)?;
        return resolve_environment_service(service, environment, ctx);
    }

    let services = ctx
        .config_store
        .list_services()?
        .into_iter()
        .filter(|service| {
            project
                .map(|project| project_matches(service, project))
                .unwrap_or(true)
        })
        .collect::<Vec<_>>();
    let services = resolve_environment_candidates(services, environment, ctx)?;
    resolve_service_from_context(services, project, environment)
}

fn resolve_environment_candidates(
    services: Vec<ConfiguredService>,
    environment: Option<&str>,
    ctx: &AppContext,
) -> Result<Vec<ConfiguredService>> {
    let Some(environment) = environment else {
        return Ok(services);
    };

    let mut resolved = Vec::new();
    let mut first_error = None;

    for service in services {
        match resolve_environment_service(service, Some(environment), ctx) {
            Ok(service) => resolved.push(service),
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(error.to_string());
                }
            }
        }
    }

    if resolved.is_empty()
        && let Some(error) = first_error
    {
        bail!("{error}");
    }

    Ok(resolved)
}

fn resolve_service_from_context(
    services: Vec<ConfiguredService>,
    project: Option<&str>,
    environment: Option<&str>,
) -> Result<ConfiguredService> {
    if services.is_empty() {
        let scope = project
            .map(|project| format!(" in project '{project}'"))
            .unwrap_or_default();
        bail!(
            "No services configured{scope}. Run '{} services list' to inspect configured services.",
            BRANDING.cli_command()
        );
    }

    let project_roots = load_project_roots();
    let services = services
        .into_iter()
        .map(|service| service_with_project_root(service, &project_roots))
        .collect::<Vec<_>>();
    let cwd = effective_current_dir()?;
    let cwd_project_id = project
        .is_none()
        .then(|| project_id_for_cwd(&services, &cwd))
        .flatten();
    let mut candidates = services
        .into_iter()
        .filter(|service| {
            cwd_project_id
                .as_deref()
                .map(|project_id| service.project_id == project_id)
                .unwrap_or(true)
        })
        .map(service_with_local_dev_launch)
        .collect::<Vec<_>>();

    candidates.sort_by(|left, right| {
        left.project_id
            .cmp(&right.project_id)
            .then_with(|| left.id.cmp(&right.id))
    });

    if let Some(service) = service_for_cwd(&candidates, &cwd) {
        return Ok(service);
    }

    match candidates.as_slice() {
        [service] => Ok(service.clone()),
        [] => bail!(
            "No service could be inferred from '{}'. Run '{} services list' or pass a service explicitly.",
            cwd.display(),
            BRANDING.cli_command()
        ),
        services if is_interactive_terminal() => prompt_service_selection(services, environment),
        services => {
            let options = run_options(services, environment);
            bail!(
                "Multiple services match '{}'. Pass one explicitly:\n{options}",
                cwd.display()
            );
        }
    }
}

fn ensure_run_context_is_initialized(
    service_name: Option<&str>,
    project: Option<&str>,
    environment: Option<&str>,
    init_domain: Option<&str>,
    config_path: &Path,
    policy: LocalAppUpdatePolicy,
    ctx: &AppContext,
) -> Result<()> {
    if service_name.is_some() || project.is_some() || environment.is_some() {
        return Ok(());
    }

    let cwd = effective_current_dir()?;
    let services = services_with_project_roots(ctx)?;
    if run_context_has_configured_project_or_service(&services, &cwd) {
        if let Some(domain) = init_domain {
            bail!(
                "Cannot use --domain '{domain}' because a Localify project is already configured for '{}'. Update the existing service domain with:\n  {} project domain set <service> {domain}",
                cwd.display(),
                BRANDING.cli_command()
            );
        }
        return Ok(());
    }

    let root = auto_init_root_for_cwd(&cwd)?;
    if !auto_init_root_has_runnable_service(&root)? {
        bail!(
            "No Localify project is configured for '{}', and no runnable dev app was detected. Run '{} init-project --yes' from the app root, or pass an existing service explicitly.",
            cwd.display(),
            BRANDING.cli_command()
        );
    }

    eprintln!(
        "No Localify project found for '{}'. Initializing it first...",
        root.display()
    );
    with_current_dir(&root, || {
        init_project::execute(
            InitProjectArgs {
                name: None,
                domain: None,
                target: None,
                with_api: false,
                api_domain: None,
                api_target: None,
                project_only: false,
                yes: !is_interactive_terminal(),
                write_scripts: false,
                detected_primary_domain: init_domain.map(ToOwned::to_owned),
            },
            config_path,
            policy,
        )
    })
}

/// Project checkout containing the current directory, with each service's
/// folder as `localify dev` resolves it.
pub struct ProjectCheckout {
    pub project_name: String,
    pub root: PathBuf,
    /// Service id and folder relative to `root`, empty for a service at the
    /// root. Services missing from this checkout are left out.
    pub services: Vec<(String, String)>,
}

pub fn current_project_checkout(ctx: &AppContext) -> Result<ProjectCheckout> {
    let cwd = effective_current_dir()?;
    let services = services_with_project_roots(ctx)?;
    let Some((project_id, root)) = checkout_root_for_cwd(&services, &cwd, ctx) else {
        bail!(
            "No {} project contains '{}'. Run '{} dev' or '{} init-project' in the project folder first.",
            BRANDING.app_name,
            cwd.display(),
            BRANDING.cli_command(),
            BRANDING.cli_command()
        );
    };

    let project_services: Vec<ConfiguredService> = services
        .into_iter()
        .filter(|service| service.project_id == project_id)
        .collect();
    let project_name = project_services
        .first()
        .map(|service| service.project_name.clone())
        .unwrap_or_else(|| project_id.clone());
    let mut folders: Vec<(String, String)> = project_services
        .into_iter()
        .filter_map(|mut service| {
            service.project_root_path = Some(root.display().to_string());
            let service = service_with_launch_for_mode(service, RunMode::Dev);
            let workdir = service_workdir(&service).ok()?;
            let relative = workdir.strip_prefix(&root).ok()?;
            Some((service.id, relative.display().to_string()))
        })
        .collect();
    folders.sort();

    Ok(ProjectCheckout {
        project_name,
        root,
        services: folders,
    })
}

/// Most specific project or worktree root that contains `cwd`.
fn checkout_root_for_cwd(
    services: &[ConfiguredService],
    cwd: &Path,
    ctx: &AppContext,
) -> Option<(String, PathBuf)> {
    let project_roots = services.iter().filter_map(|service| {
        let root = PathBuf::from(service.project_root_path.as_ref()?);
        Some((service.project_id.clone(), root))
    });
    let environment_roots = ctx
        .config_store
        .list_project_environments(None)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|environment| {
            Some((
                environment.project_id,
                PathBuf::from(environment.root_path?),
            ))
        });

    project_roots
        .chain(environment_roots)
        .filter(|(_, root)| cwd.starts_with(root))
        .max_by_key(|(_, root)| root.components().count())
}

fn services_with_project_roots(ctx: &AppContext) -> Result<Vec<ConfiguredService>> {
    let project_roots = load_project_roots();
    Ok(ctx
        .config_store
        .list_services()?
        .into_iter()
        .map(|service| service_with_project_root(service, &project_roots))
        .collect())
}

fn run_context_has_configured_project_or_service(
    services: &[ConfiguredService],
    cwd: &Path,
) -> bool {
    project_id_for_cwd(services, cwd).is_some()
        || service_for_cwd(
            &services
                .iter()
                .cloned()
                .map(service_with_local_dev_launch)
                .collect::<Vec<_>>(),
            cwd,
        )
        .is_some()
}

fn auto_init_root_for_cwd(cwd: &Path) -> Result<PathBuf> {
    let canonical_cwd = fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    for ancestor in cwd.ancestors() {
        if !looks_like_workspace_root(ancestor)? {
            continue;
        }
        if init_plan_contains_cwd(ancestor, &canonical_cwd)? {
            return Ok(ancestor.to_path_buf());
        }
    }

    Ok(cwd.to_path_buf())
}

fn looks_like_workspace_root(path: &Path) -> Result<bool> {
    if path.join("pnpm-workspace.yaml").is_file() {
        return Ok(true);
    }

    let package_json_path = path.join("package.json");
    if !package_json_path.is_file() {
        return Ok(false);
    }

    let content = fs::read_to_string(&package_json_path)
        .with_context(|| format!("Failed to read '{}'", package_json_path.display()))?;
    let manifest: serde_json::Value = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse '{}'", package_json_path.display()))?;
    Ok(manifest.get("workspaces").is_some())
}

fn init_plan_contains_cwd(root: &Path, cwd: &Path) -> Result<bool> {
    let canonical_root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let plan = detect_folder_init_plan(root, None)?;
    Ok(plan.services.iter().any(|service| {
        let Some(service_path) = service.service_path.as_deref() else {
            return cwd == canonical_root || cwd.starts_with(&canonical_root);
        };
        let service_root = canonical_root.join(service_path);
        cwd == service_root || cwd.starts_with(service_root)
    }))
}

fn auto_init_root_has_runnable_service(root: &Path) -> Result<bool> {
    let plan = detect_folder_init_plan(root, None)?;
    Ok(plan
        .services
        .iter()
        .any(|service| service.enabled && service.command.is_some()))
}

fn with_current_dir<T>(dir: &Path, action: impl FnOnce() -> Result<T>) -> Result<T> {
    let original_dir = env::current_dir().context("Failed to determine the current directory")?;
    env::set_current_dir(dir).with_context(|| format!("Failed to enter '{}'", dir.display()))?;
    let result = action();
    let restore_result = env::set_current_dir(&original_dir)
        .with_context(|| format!("Failed to restore '{}'", original_dir.display()));

    match (result, restore_result) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(restore_error)) => Err(error.context(restore_error.to_string())),
    }
}

fn service_for_cwd(services: &[ConfiguredService], cwd: &Path) -> Option<ConfiguredService> {
    let mut matches = services
        .iter()
        .filter_map(|service| service_cwd_match(service, cwd))
        .collect::<Vec<_>>();

    matches.sort_by(|(_, left), (_, right)| {
        right
            .components()
            .count()
            .cmp(&left.components().count())
            .then_with(|| left.cmp(right))
    });

    let (_, best_path) = matches.first()?;
    let best_len = best_path.components().count();
    let best_matches = matches
        .into_iter()
        .filter(|(_, path)| path.components().count() == best_len)
        .map(|(service, _)| service)
        .collect::<Vec<_>>();

    match best_matches.as_slice() {
        [service] => Some(service.clone()),
        _ => None,
    }
}

fn service_cwd_match(
    service: &ConfiguredService,
    cwd: &Path,
) -> Option<(ConfiguredService, PathBuf)> {
    let root_path = PathBuf::from(service.project_root_path.as_ref()?);
    if let Some(service_path) = service_cwd_match_path(service, &root_path)
        && cwd.starts_with(&service_path)
    {
        return Some((service.clone(), service_path));
    }

    pathless_service_cwd_match(service, cwd, &root_path)
        .or_else(|| local_dev_service_cwd_match(service, cwd, &root_path))
}

fn pathless_service_cwd_match(
    service: &ConfiguredService,
    cwd: &Path,
    root_path: &Path,
) -> Option<(ConfiguredService, PathBuf)> {
    if service
        .path
        .as_deref()
        .is_some_and(|path| !path.trim().is_empty())
    {
        return None;
    }
    if !cwd.starts_with(root_path) {
        return None;
    }

    let relative_path = cwd.strip_prefix(root_path).ok()?;
    if relative_path.as_os_str().is_empty() {
        return None;
    }

    let folder_name = cwd.file_name()?.to_string_lossy();
    if !service.id.eq_ignore_ascii_case(&folder_name)
        && !service.name.eq_ignore_ascii_case(&folder_name)
    {
        return None;
    }

    let mut service = service.clone();
    service.path = Some(relative_path.display().to_string());
    Some((service, cwd.to_path_buf()))
}

fn local_dev_service_cwd_match(
    service: &ConfiguredService,
    cwd: &Path,
    root_path: &Path,
) -> Option<(ConfiguredService, PathBuf)> {
    if service
        .path
        .as_deref()
        .is_some_and(|path| !path.trim().is_empty())
    {
        return None;
    }

    let launch = local_dev_launch_for_service(service, root_path)?;
    if !cwd.starts_with(&launch.package_dir) {
        return None;
    }

    let service = apply_local_dev_launch(service.clone(), root_path, &launch);

    Some((service, launch.package_dir))
}

fn service_with_launch_for_mode(service: ConfiguredService, mode: RunMode) -> ConfiguredService {
    match mode {
        RunMode::Run => service_with_local_dev_launch(service),
        RunMode::Dev => service_with_dev_launch(service),
    }
}

fn prompt_service_selection(
    services: &[ConfiguredService],
    environment: Option<&str>,
) -> Result<ConfiguredService> {
    let labels = services
        .iter()
        .map(service_selection_label)
        .collect::<Vec<_>>();
    let selection = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Select a service to run")
        .items(&labels)
        .default(0)
        .interact_opt()
        .context("Failed to read service selection")?;

    match selection {
        Some(index) => services.get(index).cloned().ok_or_else(|| {
            anyhow::anyhow!(
                "Invalid service selection. Pass one explicitly:\n{}",
                run_options(services, environment)
            )
        }),
        None => bail!(
            "Service selection cancelled. Pass one explicitly:\n{}",
            run_options(services, environment)
        ),
    }
}

fn service_selection_label(service: &ConfiguredService) -> String {
    let path = service.path.as_deref().unwrap_or(".");
    let command = service
        .command
        .as_ref()
        .map(|command| command.join(" "))
        .unwrap_or_else(|| "no command configured".into());
    format!("{} ({path}) - {command}", service.id)
}

fn run_options(services: &[ConfiguredService], environment: Option<&str>) -> String {
    services
        .iter()
        .map(|service| {
            let env_flag = environment
                .map(|environment| format!(" --env {environment}"))
                .unwrap_or_default();
            format!(
                "  {} dev {} --project {}{}",
                BRANDING.cli_command(),
                service.id,
                service.project_id,
                env_flag
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_interactive_terminal() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

fn service_matches(service: &ConfiguredService, service_name: &str) -> bool {
    service.id == service_name || service.name.eq_ignore_ascii_case(service_name)
}

fn project_matches(service: &ConfiguredService, project: &str) -> bool {
    service.project_id == project || service.project_name.eq_ignore_ascii_case(project)
}

fn resolve_environment_service(
    service: ConfiguredService,
    environment: Option<&str>,
    ctx: &AppContext,
) -> Result<ConfiguredService> {
    let Some(environment) = environment else {
        return Ok(service);
    };

    let environments = ctx
        .config_store
        .list_project_environments(Some(&service.project_id))?;
    let matches = environments
        .into_iter()
        .filter(|candidate| environment_matches(candidate, environment))
        .collect::<Vec<_>>();

    let environment = match matches.as_slice() {
        [environment] => environment.clone(),
        [] => bail!(
            "Project environment '{environment}' was not found for project '{}'. Create it with: {} project env ensure --project {} --name {}",
            service.project_id,
            BRANDING.cli_command(),
            service.project_id,
            environment
        ),
        _ => bail!(
            "Project environment '{environment}' is ambiguous in project '{}'. Use its slug.",
            service.project_id
        ),
    };

    service_for_environment(service, &environment, ctx)
}

fn is_worktree_environment(project_id: &str, environment: Option<&str>, ctx: &AppContext) -> bool {
    let Some(environment) = environment else {
        return false;
    };
    ctx.config_store
        .list_project_environments(Some(project_id))
        .ok()
        .and_then(|environments| {
            environments
                .into_iter()
                .find(|candidate| environment_matches(candidate, environment))
        })
        .is_some_and(|environment| !environment.primary)
}

fn environment_matches(environment: &ConfiguredProjectEnvironment, value: &str) -> bool {
    environment.slug == value || environment.name.eq_ignore_ascii_case(value)
}

fn service_for_environment(
    service: ConfiguredService,
    environment: &ConfiguredProjectEnvironment,
    ctx: &AppContext,
) -> Result<ConfiguredService> {
    let (service_id, project_id) = (service.id.clone(), service.project_id.clone());
    launch_plan::environment_service(&ctx.config_store.load()?, service, environment).ok_or_else(
        || {
            anyhow::anyhow!(
                "Service '{service_id}' is unavailable in environment '{}'. Re-run '{} project env ensure --project {project_id} --name {}'.",
                environment.slug,
                BRANDING.cli_command(),
                environment.slug
            )
        },
    )
}

fn project_id_for_cwd(services: &[ConfiguredService], cwd: &Path) -> Option<String> {
    let mut candidates = services
        .iter()
        .filter_map(|service| {
            let root_path = service.project_root_path.as_ref()?;
            let root_path = PathBuf::from(root_path);
            if cwd.starts_with(&root_path) {
                Some((service.project_id.clone(), root_path))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();

    candidates.sort_by(|(_, left), (_, right)| {
        right
            .components()
            .count()
            .cmp(&left.components().count())
            .then_with(|| left.cmp(right))
    });
    candidates.dedup_by(|left, right| left.0 == right.0);

    let (project_id, root_path) = candidates.first()?;
    let root_len = root_path.components().count();
    let equally_specific = candidates
        .iter()
        .filter(|(_, candidate_root)| candidate_root.components().count() == root_len)
        .map(|(candidate_project_id, _)| candidate_project_id)
        .collect::<std::collections::HashSet<_>>();

    if equally_specific.len() == 1 {
        Some(project_id.clone())
    } else {
        None
    }
}

fn load_project_roots() -> HashMap<String, PathBuf> {
    ProjectRootStore::new_default()
        .and_then(|store| store.load())
        .unwrap_or_default()
        .into_iter()
        .map(|(project_id, root_path)| (project_id, PathBuf::from(root_path)))
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DaemonPorts {
    http: u16,
    https: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ServiceEndpoint {
    scheme: &'static str,
    port: u16,
}

impl ServiceEndpoint {
    const fn http(port: u16) -> Self {
        Self {
            scheme: "http",
            port,
        }
    }

    const fn https(port: u16) -> Self {
        Self {
            scheme: "https",
            port,
        }
    }

    fn is_default_port(self) -> bool {
        matches!((self.scheme, self.port), ("http", 80) | ("https", 443))
    }
}

fn daemon_ports(ctx: &AppContext) -> Result<DaemonPorts> {
    let config = ctx.config_store.load()?.daemon;

    Ok(DaemonPorts {
        http: config.http_port,
        https: config.https_port,
    })
}

fn service_endpoint(
    service: &ConfiguredService,
    ports: DaemonPorts,
    ctx: &AppContext,
) -> Result<ServiceEndpoint> {
    let https_enabled = ctx
        .config_store
        .find_registration_for_domain(&service.primary_domain)?
        .map(|registration| registration.is_https_enabled())
        .unwrap_or(false);

    if https_enabled {
        Ok(ServiceEndpoint::https(ports.https))
    } else {
        Ok(ServiceEndpoint::http(ports.http))
    }
}

fn service_public_host(service: &ConfiguredService) -> String {
    let primary_domain = service.primary_domain.trim();
    let public_host = primary_domain
        .strip_prefix("*.")
        .unwrap_or(primary_domain)
        .to_string();
    debug_assert!(!public_host.starts_with("*."));
    public_host
}

fn service_public_domain(service: &ConfiguredService, endpoint: ServiceEndpoint) -> String {
    let public_host = service_public_host(service);
    if endpoint.is_default_port() {
        public_host
    } else {
        format!("{public_host}:{}", endpoint.port)
    }
}

fn service_url(service: &ConfiguredService, endpoint: ServiceEndpoint) -> String {
    format!(
        "{}://{}",
        endpoint.scheme,
        service_public_domain(service, endpoint)
    )
}

fn service_proxy_target(service: &ConfiguredService) -> Result<&ProxyTarget> {
    match service.primary_target.as_ref() {
        Some(RouteTarget::Proxy(target)) => Ok(target),
        Some(RouteTarget::StaticFiles(path)) => bail!(
            "Service '{}' primary domain points to static files at '{}', not a runnable proxy target.",
            service.id,
            path.display()
        ),
        None => bail!(
            "Service '{}' has no root proxy route for primary domain '{}'. Add a '/' route or pass through the existing service manually.",
            service.id,
            service.primary_domain
        ),
    }
}

fn is_supported_run_host(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost")
}

fn ensure_port_available(port: u16, service: &ConfiguredService) -> Result<()> {
    match TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => {
            drop(listener);
            Ok(())
        }
        Err(error) => bail!(
            "Port {port} for service '{}' is already in use ({error}). Stop that process or update the route target. Inspect with: lsof -nP -iTCP:{port} -sTCP:LISTEN",
            service.id
        ),
    }
}

fn warn_if_daemon_is_stopped(ctx: &AppContext) {
    if matches!(ctx.daemon.is_running(), Ok(false)) {
        eprintln!(
            "Localify daemon does not appear to be running. Start it separately with '{} start'.",
            BRANDING.cli_command()
        );
    }
}

fn ensure_next_allowed_dev_origin(workdir: &Path, app_host: &str) {
    let Some(next_config_path) = find_existing_next_config_path(workdir) else {
        return;
    };

    match apply_next_allowed_dev_origin(&next_config_path, app_host) {
        Ok(true) => eprintln!(
            "  updated Next.js allowedDevOrigins: {}",
            next_config_path.display()
        ),
        Ok(false) => {}
        Err(error) => eprintln!(
            "  skipped Next.js allowedDevOrigins update: {} - {}",
            next_config_path.display(),
            error
        ),
    }
}

fn ensure_vite_allowed_host(workdir: &Path, app_host: &str) {
    let Some(vite_config_path) = find_existing_vite_config_path(workdir) else {
        return;
    };

    match apply_vite_allowed_host(&vite_config_path, app_host) {
        Ok(true) => eprintln!(
            "  updated Vite server.allowedHosts: {}",
            vite_config_path.display()
        ),
        Ok(false) => {}
        Err(error) => eprintln!(
            "  skipped Vite server.allowedHosts update: {} - {}",
            vite_config_path.display(),
            error
        ),
    }
}

/// URL and domain variables Localify points at the local domain. Keys found
/// in the service's env files or in its launch profile are included, so a
/// production profile cannot send OAuth callbacks or links to production.
fn url_environment_overrides(
    workdir: &Path,
    url: &str,
    app_domain: &str,
    profile_keys: &[String],
) -> Result<Vec<(String, String)>> {
    let mut keys = BTreeSet::new();
    for key in DEFAULT_URL_ENV_KEYS
        .iter()
        .chain(DEFAULT_DOMAIN_ENV_KEYS.iter())
    {
        keys.insert((*key).to_string());
    }
    for key in discover_url_env_keys(workdir)? {
        keys.insert(key);
    }
    for key in profile_keys {
        if is_valid_env_key(key) && is_safe_url_env_key(key) {
            keys.insert(key.clone());
        }
    }

    Ok(keys
        .into_iter()
        .filter_map(|key| {
            let value = url_env_value(&key, url, app_domain)?;
            Some((key, value.to_string()))
        })
        .collect())
}

/// Better Auth's trusted origins and `LOCALIFY_PUBLIC_URL` for this
/// machine's public URLs that reach the service (docs/README.md, "Apps That
/// Check Origin"). The daemon answers with what it last learned, so a launch
/// never waits on the network; without a daemon or a sign-in, nothing.
fn public_url_overrides(
    app_host: &str,
    env_dirs: &[&Path],
    profile: Option<&PreparedProfile>,
    ctx: &AppContext,
) -> Result<Vec<(String, String)>> {
    let origins = match ctx.mgmt_client.public_urls() {
        Ok((urls, Some(machine_id))) => public_origins(app_host, &urls, &machine_id),
        _ => Vec::new(),
    };
    if origins.is_empty() {
        return Ok(Vec::new());
    }
    let trusted = match process_env_value(TRUSTED_ORIGINS_ENV_KEY, profile) {
        Some(value) => UserValue::Process(value),
        None => match env_file_defining(TRUSTED_ORIGINS_ENV_KEY, env_dirs)? {
            Some(file) => UserValue::EnvFile(file),
            None => UserValue::Unset,
        },
    };
    let public_url_set = process_env_value(PUBLIC_URL_ENV_KEY, profile).is_some()
        || env_file_defining(PUBLIC_URL_ENV_KEY, env_dirs)?.is_some();
    let (envs, hint) = public_url_environment(&origins, &trusted, public_url_set);
    if let Some(hint) = hint {
        eprintln!("  {hint}");
    }
    Ok(envs)
}

/// Where the user set a variable, if anywhere.
#[derive(Debug)]
enum UserValue {
    /// In the shell or the launch profile: the value the service gets.
    Process(String),
    /// Only in an env file, which a variable in the process would hide.
    EnvFile(PathBuf),
    Unset,
}

/// The public origins join trusted origins set in the process, which win
/// over env files anyway. Set in an env file only, the framework decides
/// which value applies, so Localify leaves it alone and says what to add,
/// in the returned hint. A `LOCALIFY_PUBLIC_URL` the user set stays theirs.
fn public_url_environment(
    origins: &[String],
    trusted: &UserValue,
    public_url_set: bool,
) -> (Vec<(String, String)>, Option<String>) {
    let Some(primary) = origins.first() else {
        return (Vec::new(), None);
    };
    let mut envs = Vec::new();
    let mut hint = None;
    match trusted {
        UserValue::Process(value) => {
            let mut merged: Vec<&str> = Vec::new();
            for origin in value
                .split(',')
                .map(str::trim)
                .chain(origins.iter().map(String::as_str))
            {
                if !origin.is_empty() && !merged.contains(&origin) {
                    merged.push(origin);
                }
            }
            envs.push((TRUSTED_ORIGINS_ENV_KEY.to_string(), merged.join(",")));
        }
        UserValue::EnvFile(file) => {
            hint = Some(format!(
                "{TRUSTED_ORIGINS_ENV_KEY} is set in {}: add {} to it so Better Auth accepts your public URL.",
                file.display(),
                origins.join(",")
            ));
        }
        UserValue::Unset => envs.push((TRUSTED_ORIGINS_ENV_KEY.to_string(), origins.join(","))),
    }
    if !public_url_set {
        envs.push((PUBLIC_URL_ENV_KEY.to_string(), primary.clone()));
    }
    (envs, hint)
}

/// The value `key` reaches the service with: the shell's, which wins over
/// the launch profile's.
fn process_env_value(key: &str, profile: Option<&PreparedProfile>) -> Option<String> {
    env::var(key).ok().or_else(|| {
        profile?
            .resolved
            .pairs()
            .into_iter()
            .find_map(|(existing, value)| (existing == key).then_some(value))
    })
}

/// The first env file in `dirs` with a line that assigns `key`. Values are
/// never read: quotes, comments and multiline values are the framework's.
fn env_file_defining(key: &str, dirs: &[&Path]) -> Result<Option<PathBuf>> {
    for dir in dirs {
        for file_name in ENV_FILE_NAMES {
            let path = dir.join(file_name);
            if !path.is_file() {
                continue;
            }
            let content = std::fs::read_to_string(&path)
                .with_context(|| format!("Failed to read env file '{}'", path.display()))?;
            if content.lines().any(|line| assigns_env_key(line, key)) {
                return Ok(Some(path));
            }
        }
    }
    Ok(None)
}

/// `^\s*(export\s+)?KEY\s*=`.
fn assigns_env_key(line: &str, key: &str) -> bool {
    let line = line.trim_start();
    let line = match line.strip_prefix("export") {
        Some(rest) if rest.starts_with(char::is_whitespace) => rest.trim_start(),
        _ => line,
    };
    line.strip_prefix(key)
        .is_some_and(|rest| rest.trim_start().starts_with('='))
}

fn discover_url_env_keys(workdir: &Path) -> Result<Vec<String>> {
    let mut keys = Vec::new();
    for file_name in ENV_FILE_NAMES {
        let path = workdir.join(file_name);
        if !path.is_file() {
            continue;
        }
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read env file '{}'", path.display()))?;
        for line in content.lines() {
            if let Some(key) = parse_env_key(line)
                && is_safe_url_env_key(&key)
            {
                keys.push(key);
            }
        }
    }
    Ok(keys)
}

fn parse_env_key(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }

    let assignment = trimmed.strip_prefix("export ").unwrap_or(trimmed);
    let key = assignment.split_once('=')?.0.trim();
    is_valid_env_key(key).then(|| key.to_string())
}

fn is_valid_env_key(key: &str) -> bool {
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn is_safe_url_env_key(key: &str) -> bool {
    let key = key.to_ascii_uppercase();
    !contains_sensitive_env_fragment(&key) && url_env_value_kind(&key).is_some()
}

fn url_env_value<'a>(key: &str, url: &'a str, app_domain: &'a str) -> Option<&'a str> {
    match url_env_value_kind(&key.to_ascii_uppercase())? {
        UrlEnvValueKind::Url => Some(url),
        UrlEnvValueKind::Domain => Some(app_domain),
    }
}

fn url_env_value_kind(key: &str) -> Option<UrlEnvValueKind> {
    if DEFAULT_URL_ENV_KEYS.contains(&key) {
        return Some(UrlEnvValueKind::Url);
    }
    if DEFAULT_DOMAIN_ENV_KEYS.contains(&key) {
        return Some(UrlEnvValueKind::Domain);
    }

    let base = public_env_base_name(key);
    if is_app_url_env_base(base) {
        return Some(UrlEnvValueKind::Url);
    }
    if is_app_domain_env_base(base) {
        return Some(UrlEnvValueKind::Domain);
    }

    None
}

fn public_env_base_name(key: &str) -> &str {
    for prefix in [
        "NEXT_PUBLIC_",
        "NUXT_PUBLIC_",
        "EXPO_PUBLIC_",
        "REACT_APP_",
        "VITE_",
        "PUBLIC_",
    ] {
        if let Some(base) = key.strip_prefix(prefix) {
            return base;
        }
    }
    key
}

fn is_app_url_env_base(base: &str) -> bool {
    matches!(
        base,
        "APP_URL"
            | "WEB_URL"
            | "SITE_URL"
            | "BASE_URL"
            | "PUBLIC_URL"
            | "FRONTEND_URL"
            | "CLIENT_URL"
            | "ROOT_URL"
            | "LOCALIFY_URL"
            | "AUTH_URL"
            | "BETTER_AUTH_URL"
            | "APP_ORIGIN"
            | "WEB_ORIGIN"
            | "SITE_ORIGIN"
            | "ORIGIN"
            | "URL"
    )
}

fn is_app_domain_env_base(base: &str) -> bool {
    matches!(
        base,
        "APP_DOMAIN"
            | "WEB_DOMAIN"
            | "SITE_DOMAIN"
            | "PUBLIC_DOMAIN"
            | "FRONTEND_DOMAIN"
            | "CLIENT_DOMAIN"
            | "LOCALIFY_DOMAIN"
            | "DOMAIN"
    )
}

fn contains_sensitive_env_fragment(key: &str) -> bool {
    [
        "SECRET", "TOKEN", "PASSWORD", "PASS", "KEY", "DSN", "DATABASE", "POSTGRES", "MYSQL",
        "MONGO", "REDIS", "SMTP", "RESEND", "STRIPE", "S3", "AWS", "R2", "BUCKET", "STORAGE",
        "DIRECTUS", "WEBHOOK", "PRIVATE", "INTERNAL",
    ]
    .iter()
    .any(|fragment| key.contains(fragment))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UrlEnvValueKind {
    Url,
    Domain,
}

fn env_key_list(envs: &[(String, String)]) -> String {
    envs.iter()
        .map(|(key, _)| key.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::launch_plan::{
        command_invokes_dev_localify, inferred_path_from_configured_route_launch,
        prepare_local_dev_command,
    };

    fn running_web() -> RunningService {
        serde_json::from_value(serde_json::json!({
            "projectId": "app", "environment": null, "serviceId": "web", "pid": 4242,
            "url": "https://web.localify", "port": 3000, "origin": "terminal",
            "profile": null, "startedAt": 1_794_000_000u64, "logFile": null, "exit": null
        }))
        .unwrap()
    }

    fn answer(typed: &str) -> (bool, String) {
        let mut asked = Vec::new();
        let replace = confirm_replace(&running_web(), &mut typed.as_bytes(), &mut asked).unwrap();
        (replace, String::from_utf8(asked).unwrap())
    }

    #[test]
    fn enter_or_yes_replaces_a_running_service_and_anything_else_leaves_it() {
        let (replace, asked) = answer("\n");
        assert!(replace);
        assert!(
            asked.starts_with("web is already running (since "),
            "{asked}"
        );
        assert!(
            asked.ends_with(", pid 4242). Stop it and start again? [Y/n] "),
            "{asked}"
        );

        assert!(answer("y\n").0);
        assert!(answer("YES\n").0);
        assert!(!answer("n\n").0);
        assert!(!answer("later\n").0);
        // No answer at all, as from a closed input, is a no.
        assert!(!answer("").0);
    }

    fn test_service(id: &str, root_path: &str, service_path: Option<&str>) -> ConfiguredService {
        ConfiguredService {
            id: id.into(),
            name: id.into(),
            project_id: "demo".into(),
            project_name: "Demo".into(),
            project_enabled: true,
            project_root_path: Some(root_path.into()),
            primary_domain: format!("{id}.demo.localify"),
            domains: vec![format!("{id}.demo.localify")],
            path: service_path.map(Into::into),
            command: Some(vec!["pnpm".into(), "dev".into()]),
            primary_target: None,
            inferred: false,
        }
    }

    fn test_service_without_root(id: &str, service_path: Option<&str>) -> ConfiguredService {
        ConfiguredService {
            id: id.into(),
            name: id.into(),
            project_id: "demo".into(),
            project_name: "Demo".into(),
            project_enabled: true,
            project_root_path: None,
            primary_domain: format!("{id}.demo.localify"),
            domains: vec![format!("{id}.demo.localify")],
            path: service_path.map(Into::into),
            command: Some(vec!["pnpm".into(), "dev".into()]),
            primary_target: None,
            inferred: false,
        }
    }

    #[test]
    fn a_project_folder_that_is_gone_says_how_to_point_at_the_new_one() {
        let temp_dir = tempfile::tempdir().unwrap();
        let gone = temp_dir.path().join("moved-away");
        let service = test_service("web", &gone.display().to_string(), Some("apps/web"));

        let error = ensure_service_folder_exists(&service, None)
            .unwrap_err()
            .to_string();
        assert!(error.contains(&gone.display().to_string()), "{error}");
        assert!(error.contains("no longer exists"), "{error}");
        assert!(
            error.contains(&format!(
                "{} project update demo --root <folder>",
                BRANDING.cli_command()
            )),
            "{error}"
        );

        let error = ensure_service_folder_exists(&service, Some("feature-x"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("Environment 'feature-x'"), "{error}");
        assert!(
            error.contains(&format!(
                "{} project env prune --project demo",
                BRANDING.cli_command()
            )),
            "{error}"
        );

        let here = test_service("web", &temp_dir.path().display().to_string(), None);
        assert!(ensure_service_folder_exists(&here, None).is_ok());
        assert!(
            ensure_service_folder_exists(&test_service_without_root("web", None), None).is_ok()
        );
    }

    #[test]
    fn run_host_support_matches_injected_host_contract() {
        assert!(is_supported_run_host("127.0.0.1"));
        assert!(is_supported_run_host("localhost"));
        assert!(!is_supported_run_host("::1"));
        assert!(!is_supported_run_host("127.0.0.2"));
    }

    #[test]
    fn service_url_strips_only_one_wildcard_prefix() {
        let service = ConfiguredService {
            id: "web".into(),
            name: "web".into(),
            project_id: "demo".into(),
            project_name: "Demo".into(),
            project_enabled: true,
            project_root_path: None,
            primary_domain: "*.demo.localify".into(),
            domains: vec!["*.demo.localify".into()],
            path: None,
            command: None,
            primary_target: None,
            inferred: true,
        };

        assert_eq!(
            service_url(&service, ServiceEndpoint::https(443)),
            "https://demo.localify"
        );
    }

    #[test]
    fn service_url_includes_non_default_endpoint_port() {
        let service = ConfiguredService {
            id: "web".into(),
            name: "web".into(),
            project_id: "demo".into(),
            project_name: "Demo".into(),
            project_enabled: true,
            project_root_path: None,
            primary_domain: "feat-auth.demo.localify.test".into(),
            domains: vec!["feat-auth.demo.localify.test".into()],
            path: None,
            command: None,
            primary_target: None,
            inferred: true,
        };

        assert_eq!(
            service_url(&service, ServiceEndpoint::https(8443)),
            "https://feat-auth.demo.localify.test:8443"
        );
        assert_eq!(
            service_url(&service, ServiceEndpoint::http(8080)),
            "http://feat-auth.demo.localify.test:8080"
        );
    }

    #[test]
    fn service_selection_label_includes_path_and_command() {
        let service = test_service("web", "/repo", Some("apps/app"));

        assert_eq!(
            service_selection_label(&service),
            "web (apps/app) - pnpm dev"
        );
    }

    #[test]
    fn service_selection_label_handles_root_and_missing_command() {
        let mut service = test_service("server", "/repo", None);
        service.command = None;

        assert_eq!(
            service_selection_label(&service),
            "server (.) - no command configured"
        );
    }

    #[test]
    fn prepare_local_dev_command_adds_vite_runtime_flags() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            temp_dir.path().join("package.json"),
            r#"{"scripts":{"dev:bare":"ALCHEMY=false vite dev"}}"#,
        )
        .unwrap();

        let command = prepare_local_dev_command(
            vec!["pnpm".into(), "run".into(), "dev:bare".into()],
            temp_dir.path(),
            temp_dir.path(),
            58522,
        )
        .unwrap();

        assert_eq!(
            command,
            vec![
                "pnpm",
                "run",
                "dev:bare",
                "--host",
                "127.0.0.1",
                "--port",
                "58522"
            ]
        );
    }

    #[test]
    fn prepare_local_dev_command_adds_next_runtime_flags_with_npm_separator() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            temp_dir.path().join("package.json"),
            r#"{"scripts":{"dev":"next dev"}}"#,
        )
        .unwrap();

        let command = prepare_local_dev_command(
            vec!["npm".into(), "run".into(), "dev".into()],
            temp_dir.path(),
            temp_dir.path(),
            4010,
        )
        .unwrap();

        assert_eq!(
            command,
            vec![
                "npm",
                "run",
                "dev",
                "--",
                "--hostname",
                "127.0.0.1",
                "--port",
                "4010"
            ]
        );
    }

    #[test]
    fn prepare_local_dev_command_skips_matching_next_runtime_flags() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            temp_dir.path().join("package.json"),
            r#"{"scripts":{"dev":"next dev --hostname 127.0.0.1 --port 4010"}}"#,
        )
        .unwrap();

        let command = prepare_local_dev_command(
            vec!["pnpm".into(), "run".into(), "dev".into()],
            temp_dir.path(),
            temp_dir.path(),
            4010,
        )
        .unwrap();

        assert_eq!(command, vec!["pnpm", "run", "dev"]);
    }

    #[test]
    fn prepare_local_dev_command_overrides_stale_next_runtime_flags() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            temp_dir.path().join("package.json"),
            r#"{"scripts":{"dev":"next dev --hostname 0.0.0.0 --port=3000"}}"#,
        )
        .unwrap();

        let command = prepare_local_dev_command(
            vec!["pnpm".into(), "run".into(), "dev".into()],
            temp_dir.path(),
            temp_dir.path(),
            4010,
        )
        .unwrap();

        assert_eq!(
            command,
            vec![
                "pnpm",
                "run",
                "dev",
                "--hostname",
                "127.0.0.1",
                "--port",
                "4010"
            ]
        );
    }

    #[test]
    fn prepare_local_dev_command_opens_next_intl_apps_to_every_interface() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            temp_dir.path().join("package.json"),
            r#"{"scripts":{"dev":"next dev --hostname 127.0.0.1"},"dependencies":{"next-intl":"^4.14.9"}}"#,
        )
        .unwrap();

        let command = prepare_local_dev_command(
            vec!["pnpm".into(), "run".into(), "dev".into()],
            temp_dir.path(),
            temp_dir.path(),
            4010,
        )
        .unwrap();

        assert_eq!(
            command,
            vec![
                "pnpm",
                "run",
                "dev",
                "--hostname",
                "0.0.0.0",
                "--port",
                "4010"
            ]
        );
    }

    #[test]
    fn prepare_local_dev_command_finds_next_intl_in_the_monorepo_root() {
        let temp_dir = tempfile::tempdir().unwrap();
        let app_dir = temp_dir.path().join("apps/web");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(
            temp_dir.path().join("package.json"),
            r#"{"private":true,"dependencies":{"next-intl":"^4.14.9"}}"#,
        )
        .unwrap();
        std::fs::write(
            app_dir.join("package.json"),
            r#"{"scripts":{"dev":"next dev"}}"#,
        )
        .unwrap();

        let command = prepare_local_dev_command(
            vec!["pnpm".into(), "run".into(), "dev".into()],
            &app_dir,
            temp_dir.path(),
            4010,
        )
        .unwrap();

        assert_eq!(
            command,
            vec![
                "pnpm",
                "run",
                "dev",
                "--hostname",
                "0.0.0.0",
                "--port",
                "4010"
            ]
        );
    }

    #[test]
    fn prepare_local_dev_command_ignores_next_intl_above_the_project_root() {
        let temp_dir = tempfile::tempdir().unwrap();
        let project_root = temp_dir.path().join("project");
        std::fs::create_dir_all(&project_root).unwrap();
        std::fs::write(
            temp_dir.path().join("package.json"),
            r#"{"dependencies":{"next-intl":"^4.14.9"}}"#,
        )
        .unwrap();
        std::fs::write(
            project_root.join("package.json"),
            r#"{"scripts":{"dev":"next dev"}}"#,
        )
        .unwrap();

        let command = prepare_local_dev_command(
            vec!["pnpm".into(), "run".into(), "dev".into()],
            &project_root,
            &project_root,
            4010,
        )
        .unwrap();

        assert_eq!(
            command,
            vec![
                "pnpm",
                "run",
                "dev",
                "--hostname",
                "127.0.0.1",
                "--port",
                "4010"
            ]
        );
    }

    #[test]
    fn prepare_local_dev_command_keeps_vite_on_loopback_next_to_next_intl() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            temp_dir.path().join("package.json"),
            r#"{"scripts":{"dev":"vite"},"devDependencies":{"next-intl":"^4.14.9"}}"#,
        )
        .unwrap();

        let command = prepare_local_dev_command(
            vec!["pnpm".into(), "run".into(), "dev".into()],
            temp_dir.path(),
            temp_dir.path(),
            4010,
        )
        .unwrap();

        assert_eq!(
            command,
            vec![
                "pnpm",
                "run",
                "dev",
                "--host",
                "127.0.0.1",
                "--port",
                "4010"
            ]
        );
    }

    #[test]
    fn prepare_local_dev_command_ignores_a_broken_parent_manifest() {
        let temp_dir = tempfile::tempdir().unwrap();
        let app_dir = temp_dir.path().join("apps/web");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(temp_dir.path().join("package.json"), "{ not json").unwrap();
        std::fs::write(
            app_dir.join("package.json"),
            r#"{"scripts":{"dev":"next dev"}}"#,
        )
        .unwrap();

        let command = prepare_local_dev_command(
            vec!["pnpm".into(), "run".into(), "dev".into()],
            &app_dir,
            temp_dir.path(),
            4010,
        )
        .unwrap();

        assert_eq!(
            command,
            vec![
                "pnpm",
                "run",
                "dev",
                "--hostname",
                "127.0.0.1",
                "--port",
                "4010"
            ]
        );
    }

    #[test]
    fn prepare_local_dev_command_adds_email_runtime_port() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            temp_dir.path().join("package.json"),
            r#"{"scripts":{"dev":"email dev --port 3003"}}"#,
        )
        .unwrap();

        let command = prepare_local_dev_command(
            vec!["pnpm".into(), "run".into(), "dev".into()],
            temp_dir.path(),
            temp_dir.path(),
            52168,
        )
        .unwrap();

        assert_eq!(command, vec!["pnpm", "run", "dev", "--port", "52168"]);
    }

    #[test]
    fn ensure_next_allowed_dev_origin_updates_existing_next_config() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("next.config.js");
        std::fs::write(
            &config_path,
            "module.exports = {\n  reactStrictMode: true,\n};\n",
        )
        .unwrap();

        ensure_next_allowed_dev_origin(temp_dir.path(), "fix-branch.demo.localify");
        ensure_next_allowed_dev_origin(temp_dir.path(), "fix-branch.demo.localify");

        // The project's site covers every worktree: none of them changes the file again.
        ensure_next_allowed_dev_origin(temp_dir.path(), "other.demo.localify");
        let updated = std::fs::read_to_string(config_path).unwrap();
        assert!(updated.contains("allowedDevOrigins: ['demo.localify', '**.demo.localify']"));
        assert!(!updated.contains("fix-branch"));
    }

    #[test]
    fn ensure_vite_allowed_host_updates_existing_vite_config() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("vite.config.ts");
        std::fs::write(
            &config_path,
            "import { defineConfig } from 'vite';\n\nexport default defineConfig({\n  plugins: [],\n});\n",
        )
        .unwrap();

        ensure_vite_allowed_host(temp_dir.path(), "fix-branch.demo.localify");
        ensure_vite_allowed_host(temp_dir.path(), "fix-branch.demo.localify");

        ensure_vite_allowed_host(temp_dir.path(), "other.demo.localify");
        let updated = std::fs::read_to_string(config_path).unwrap();
        assert!(updated.contains("server: { allowedHosts: ['.demo.localify'] }"));
        assert_eq!(updated.matches("demo.localify").count(), 1);
    }

    #[test]
    fn ensure_vite_allowed_host_skips_unsupported_vite_config() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("vite.config.ts");
        let original = "import { defineConfig } from 'vite';\nconst config = loadConfig();\nexport default defineConfig(config);\n";
        std::fs::write(&config_path, original).unwrap();

        ensure_vite_allowed_host(temp_dir.path(), "fix-branch.demo.localify");

        let updated = std::fs::read_to_string(config_path).unwrap();
        assert_eq!(updated, original);
    }

    #[test]
    fn url_environment_overrides_include_default_runtime_keys() {
        let temp_dir = tempfile::tempdir().unwrap();
        let url = "https://feat.demo.localify.test:8443";
        let domain = "feat.demo.localify.test:8443";

        let envs = url_environment_overrides(temp_dir.path(), url, domain, &[]).unwrap();

        assert_eq!(env_value(&envs, "LOCALIFY_URL"), Some(url));
        assert_eq!(env_value(&envs, "NEXT_PUBLIC_APP_URL"), Some(url));
        assert_eq!(env_value(&envs, "AUTH_URL"), Some(url));
        assert_eq!(env_value(&envs, "VITE_APP_URL"), Some(url));
        assert_eq!(env_value(&envs, "LOCALIFY_DOMAIN"), Some(domain));
        assert_eq!(env_value(&envs, "NEXT_PUBLIC_APP_DOMAIN"), Some(domain));
    }

    #[test]
    fn url_environment_overrides_discovers_safe_env_file_keys_only() {
        let temp_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            temp_dir.path().join(".env"),
            "\
# ignored
VITE_SITE_URL=https://old.example
PUBLIC_DOMAIN=old.example
export WEB_ORIGIN=https://old.example
DATABASE_URL=postgres://secret
STRIPE_PUBLIC_URL=https://stripe.example
NEXT_PUBLIC_API_KEY=secret
DIRECTUS_URL=https://directus.example
INVALID-NAME=https://old.example
",
        )
        .unwrap();

        let url = "https://feat.demo.localify.test:8443";
        let domain = "feat.demo.localify.test:8443";
        let envs = url_environment_overrides(temp_dir.path(), url, domain, &[]).unwrap();

        assert_eq!(env_value(&envs, "VITE_SITE_URL"), Some(url));
        assert_eq!(env_value(&envs, "WEB_ORIGIN"), Some(url));
        assert_eq!(env_value(&envs, "PUBLIC_DOMAIN"), Some(domain));
        assert_eq!(env_value(&envs, "DATABASE_URL"), None);
        assert_eq!(env_value(&envs, "STRIPE_PUBLIC_URL"), None);
        assert_eq!(env_value(&envs, "NEXT_PUBLIC_API_KEY"), None);
        assert_eq!(env_value(&envs, "DIRECTUS_URL"), None);
        assert_eq!(env_value(&envs, "INVALID-NAME"), None);
    }

    fn public_url(
        slug: &str,
        local_domain: &str,
        wildcard: bool,
        machine: &str,
    ) -> crate::infrastructure::cloud::types::PublicUrl {
        serde_json::from_value(serde_json::json!({
            "id": slug, "slug": slug, "url": format!("https://{slug}.localify.fyi"),
            "localDomain": local_domain, "wildcard": wildcard,
            "machine": { "id": machine, "name": "laptop" },
            "protection": { "mode": "password" }, "createdAt": "2026-11-08T10:00:00Z"
        }))
        .unwrap()
    }

    fn public_envs(domain: &str, trusted: &UserValue) -> Vec<(String, String)> {
        let urls = [
            public_url("journal", "journal.localify", false, "m1"),
            public_url("louez", "louez.localify", true, "m1"),
            public_url("louez-ana", "louez.localify", true, "teammate-machine"),
        ];
        public_url_environment(&public_origins(domain, &urls, "m1"), trusted, false).0
    }

    #[test]
    fn an_exact_public_url_is_trusted_and_named() {
        let envs = public_envs("journal.localify", &UserValue::Unset);

        assert_eq!(
            env_value(&envs, TRUSTED_ORIGINS_ENV_KEY),
            Some("https://journal.localify.fyi")
        );
        assert_eq!(
            env_value(&envs, PUBLIC_URL_ENV_KEY),
            Some("https://journal.localify.fyi")
        );
    }

    #[test]
    fn a_wildcard_project_url_trusts_the_host_of_the_service_label_only() {
        let envs = public_envs("api.louez.localify", &UserValue::Unset);
        assert_eq!(
            env_value(&envs, TRUSTED_ORIGINS_ENV_KEY),
            Some("https://louez--api.localify.fyi")
        );
        assert_eq!(
            env_value(&envs, PUBLIC_URL_ENV_KEY),
            Some("https://louez--api.localify.fyi")
        );

        let project = public_envs("louez.localify", &UserValue::Unset);
        assert_eq!(
            env_value(&project, TRUSTED_ORIGINS_ENV_KEY),
            Some("https://louez.localify.fyi")
        );
    }

    #[test]
    fn public_origins_join_trusted_origins_set_in_the_process() {
        let set = UserValue::Process(
            "https://louez.localify, https://louez--api.localify.fyi,".to_string(),
        );
        let envs = public_envs("api.louez.localify", &set);

        assert_eq!(
            env_value(&envs, TRUSTED_ORIGINS_ENV_KEY),
            Some("https://louez.localify,https://louez--api.localify.fyi")
        );

        let origins = ["https://louez.localify.fyi".to_string()];
        let (kept, _) = public_url_environment(&origins, &UserValue::Unset, true);
        assert_eq!(env_value(&kept, PUBLIC_URL_ENV_KEY), None);
    }

    #[test]
    fn trusted_origins_set_in_an_env_file_are_left_alone_with_a_hint() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        let app = root.join("apps/web");
        std::fs::create_dir_all(&app).unwrap();
        // A multiline, commented value: only the key is matched.
        std::fs::write(
            root.join(".env.local"),
            "# trusted\n  export BETTER_AUTH_TRUSTED_ORIGINS = \"https://a.example,\n\
             https://b.example\" # both\n",
        )
        .unwrap();
        std::fs::write(app.join(".env"), "BETTER_AUTH_TRUSTED_ORIGINS_X=1\n").unwrap();

        let file = env_file_defining(TRUSTED_ORIGINS_ENV_KEY, &[app.as_path(), root])
            .unwrap()
            .unwrap();
        assert_eq!(file, root.join(".env.local"));
        assert!(
            env_file_defining(PUBLIC_URL_ENV_KEY, &[app.as_path(), root])
                .unwrap()
                .is_none()
        );

        let origins = ["https://louez.localify.fyi".to_string()];
        let (envs, hint) = public_url_environment(&origins, &UserValue::EnvFile(file), false);
        assert_eq!(env_value(&envs, TRUSTED_ORIGINS_ENV_KEY), None);
        assert_eq!(
            env_value(&envs, PUBLIC_URL_ENV_KEY),
            Some("https://louez.localify.fyi")
        );
        let hint = hint.unwrap();
        assert!(hint.starts_with("BETTER_AUTH_TRUSTED_ORIGINS is set in "));
        assert!(hint.ends_with(
            ".env.local: add https://louez.localify.fyi to it so Better Auth accepts your public URL."
        ));
    }

    #[test]
    fn env_key_assignments_are_matched_by_key_only() {
        let key = TRUSTED_ORIGINS_ENV_KEY;
        assert!(assigns_env_key("BETTER_AUTH_TRUSTED_ORIGINS=", key));
        assert!(assigns_env_key(
            "\texport\tBETTER_AUTH_TRUSTED_ORIGINS =x",
            key
        ));
        assert!(!assigns_env_key("# BETTER_AUTH_TRUSTED_ORIGINS=x", key));
        assert!(!assigns_env_key("exportBETTER_AUTH_TRUSTED_ORIGINS=x", key));
        assert!(!assigns_env_key("BETTER_AUTH_TRUSTED_ORIGINS_DEV=x", key));
    }

    #[test]
    fn nothing_is_injected_without_a_public_url_of_this_machine() {
        assert!(public_envs("other.localify", &UserValue::Unset).is_empty());
        let set = UserValue::Process("https://a.example".into());
        assert_eq!(public_url_environment(&[], &set, false), (Vec::new(), None));

        let theirs = [public_url("louez-ana", "louez.localify", true, "m2")];
        assert!(public_origins("louez.localify", &theirs, "m1").is_empty());
        for (_, value) in public_envs("api.louez.localify", &UserValue::Unset) {
            assert!(!value.contains("louez-ana") && !value.contains('*'));
        }
    }

    #[test]
    fn parse_env_key_accepts_exported_dotenv_assignments() {
        assert_eq!(
            parse_env_key("export NEXT_PUBLIC_APP_URL=https://old.example").as_deref(),
            Some("NEXT_PUBLIC_APP_URL")
        );
        assert_eq!(parse_env_key(" # comment"), None);
        assert_eq!(parse_env_key("BAD-NAME=value"), None);
    }

    fn env_value<'a>(envs: &'a [(String, String)], key: &str) -> Option<&'a str> {
        envs.iter()
            .find(|(env_key, _)| env_key == key)
            .map(|(_, value)| value.as_str())
    }

    #[test]
    fn service_for_cwd_detects_nested_service_path() {
        let services = vec![
            test_service("api", "/repo", Some("apps/api")),
            test_service("web", "/repo", Some("apps/web")),
        ];

        let service = service_for_cwd(&services, Path::new("/repo/apps/web/src")).unwrap();

        assert_eq!(service.id, "web");
    }

    #[test]
    fn service_for_cwd_does_not_pick_root_service_when_at_project_root() {
        let services = vec![
            test_service("api", "/repo", Some("apps/api")),
            test_service("web", "/repo", None),
        ];

        let service = service_for_cwd(&services, Path::new("/repo"));

        assert!(service.is_none());
    }

    #[test]
    fn service_for_cwd_uses_most_specific_matching_service() {
        let services = vec![
            test_service("root", "/repo", Some(".")),
            test_service("web", "/repo", Some("apps/web")),
        ];

        let service = service_for_cwd(&services, Path::new("/repo/apps/web")).unwrap();

        assert_eq!(service.id, "web");
    }

    #[test]
    fn service_for_cwd_detects_pathless_service_from_folder_name() {
        let services = vec![
            test_service("api", "/repo", None),
            test_service("web", "/repo", None),
        ];

        let service = service_for_cwd(&services, Path::new("/repo/web")).unwrap();

        assert_eq!(service.id, "web");
        assert_eq!(service.path.as_deref(), Some("web"));
    }

    #[test]
    fn service_for_cwd_prefers_pathless_folder_match_over_root_service() {
        let services = vec![
            test_service("root", "/repo", Some(".")),
            test_service("web", "/repo", None),
        ];

        let service = service_for_cwd(&services, Path::new("/repo/web")).unwrap();

        assert_eq!(service.id, "web");
        assert_eq!(service.path.as_deref(), Some("web"));
    }

    #[test]
    fn service_with_project_root_uses_local_metadata_fallback() {
        let roots = HashMap::from([("demo".into(), PathBuf::from("/repo"))]);

        let service = service_with_project_root(test_service_without_root("web", None), &roots);

        assert_eq!(service.project_root_path.as_deref(), Some("/repo"));
    }

    #[test]
    fn run_context_has_configured_project_or_service_when_cwd_is_inside_project() {
        let services = vec![
            test_service("api", "/repo", Some("apps/api")),
            test_service("web", "/repo", Some("apps/web")),
        ];

        assert!(run_context_has_configured_project_or_service(
            &services,
            Path::new("/repo/apps/web")
        ));
        assert!(!run_context_has_configured_project_or_service(
            &services,
            Path::new("/other/apps/web")
        ));
    }

    #[test]
    fn auto_init_root_for_cwd_prefers_workspace_root_for_nested_app() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        let app_dir = root.join("apps/web");
        let nested_dir = app_dir.join("src/routes");
        std::fs::create_dir_all(&nested_dir).unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"private":true,"workspaces":["apps/*"],"packageManager":"pnpm@10.0.0"}"#,
        )
        .unwrap();
        std::fs::write(
            app_dir.join("package.json"),
            r#"{"name":"web","scripts":{"dev":"vite"}}"#,
        )
        .unwrap();

        let init_root = auto_init_root_for_cwd(&nested_dir).unwrap();

        assert_eq!(init_root, root);
        assert!(auto_init_root_has_runnable_service(&init_root).unwrap());
    }

    #[test]
    fn auto_init_root_has_runnable_service_rejects_plain_folder() {
        let temp_dir = tempfile::tempdir().unwrap();

        assert!(!auto_init_root_has_runnable_service(temp_dir.path()).unwrap());
    }

    #[test]
    fn service_for_cwd_detects_pathless_service_from_matching_local_dev_port() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        let app_dir = root.join("apps/web");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"private":true,"workspaces":["apps/*"],"packageManager":"pnpm@10.0.0"}"#,
        )
        .unwrap();
        std::fs::write(
            app_dir.join("package.json"),
            r#"{"name":"web","scripts":{"dev-localify":"next dev --hostname 127.0.0.1 --port 52165"}}"#,
        )
        .unwrap();
        let mut service = test_service("acme", &root.display().to_string(), None);
        service.command = None;
        service.primary_target = Some(RouteTarget::Proxy(ProxyTarget::parse("52165").unwrap()));

        let service = service_for_cwd(&[service], &app_dir).unwrap();

        assert_eq!(service.id, "acme");
        assert_eq!(service.path.as_deref(), Some("apps/web"));
        assert_eq!(service.command, None);
    }

    #[test]
    fn service_with_local_dev_launch_keeps_legacy_dev_localify_path_only() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        let app_dir = root.join("apps/web");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"private":true,"workspaces":["apps/*"],"packageManager":"pnpm@10.0.0"}"#,
        )
        .unwrap();
        std::fs::write(
            app_dir.join("package.json"),
            r#"{"name":"web","scripts":{"dev-localify":"next dev --hostname 127.0.0.1 --port 52165"}}"#,
        )
        .unwrap();
        let mut service = test_service("acme", &root.display().to_string(), None);
        service.command = None;
        service.primary_target = Some(RouteTarget::Proxy(ProxyTarget::parse("52165").unwrap()));

        let service = service_with_local_dev_launch(service);

        assert_eq!(service.path.as_deref(), Some("apps/web"));
        assert_eq!(service.command, None);
    }

    #[test]
    fn service_with_local_dev_launch_replaces_persisted_dev_localify_command() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        let app_dir = root.join("apps/web");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"private":true,"workspaces":["apps/*"],"packageManager":"pnpm@10.0.0"}"#,
        )
        .unwrap();
        std::fs::write(
            app_dir.join("package.json"),
            r#"{"name":"web","scripts":{"dev-localify":"next dev --hostname 127.0.0.1 --port 52165","dev":"next dev"}}"#,
        )
        .unwrap();
        let mut service = test_service("acme", &root.display().to_string(), None);
        service.command = Some(vec!["pnpm".into(), "run".into(), "dev-localify".into()]);
        service.primary_target = Some(RouteTarget::Proxy(ProxyTarget::parse("52165").unwrap()));

        let service = service_with_local_dev_launch(service);

        assert_eq!(service.path.as_deref(), Some("apps/web"));
        assert_eq!(
            service.command,
            Some(vec!["pnpm".into(), "run".into(), "dev".into()])
        );
    }

    #[test]
    fn service_with_local_dev_launch_uses_portable_script_for_worktree_port() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        let app_dir = root.join("apps/web");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{
  "private": true,
  "workspaces": ["apps/*"],
  "packageManager": "pnpm@10.0.0",
  "scripts": {
    "dev": "turbo run dev"
  }
}"#,
        )
        .unwrap();
        std::fs::write(
            app_dir.join("package.json"),
            r#"{
  "name": "web",
  "scripts": {
    "dev-localify": "next dev --hostname 127.0.0.1 --port 61092",
    "dev": "next dev"
  }
}"#,
        )
        .unwrap();
        let mut service = test_service("store", &root.display().to_string(), None);
        service.command = None;
        service.primary_target = Some(RouteTarget::Proxy(ProxyTarget::parse("63612").unwrap()));

        let service = service_with_local_dev_launch(service);

        assert_eq!(service.path.as_deref(), Some("apps/web"));
        assert_eq!(
            service.command,
            Some(vec!["pnpm".into(), "run".into(), "dev".into()])
        );
    }

    #[test]
    fn service_with_dev_launch_uses_dev_script_from_legacy_localify_port() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        let app_dir = root.join("apps/web");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"private":true,"workspaces":["apps/*"],"packageManager":"pnpm@10.0.0"}"#,
        )
        .unwrap();
        std::fs::write(
            app_dir.join("package.json"),
            r#"{"name":"web","scripts":{"dev-localify":"next dev --hostname 127.0.0.1 --port 52165","dev":"next dev"}}"#,
        )
        .unwrap();
        let mut service = test_service("acme", &root.display().to_string(), None);
        service.command = None;
        service.primary_target = Some(RouteTarget::Proxy(ProxyTarget::parse("52165").unwrap()));

        let service = service_with_dev_launch(service);

        assert_eq!(service.path.as_deref(), Some("apps/web"));
        assert_eq!(
            service.command,
            Some(vec!["pnpm".into(), "run".into(), "dev".into()])
        );
    }

    #[test]
    fn service_with_dev_launch_replaces_persisted_dev_localify_command() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        let app_dir = root.join("apps/web");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"private":true,"workspaces":["apps/*"],"packageManager":"pnpm@10.0.0"}"#,
        )
        .unwrap();
        std::fs::write(
            app_dir.join("package.json"),
            r#"{"name":"web","scripts":{"dev-localify":"echo Localify URL: https://store.localify && AUTH_URL=https://store.localify NEXT_PUBLIC_APP_URL=https://store.localify NEXT_PUBLIC_APP_DOMAIN=store.localify next dev --hostname 127.0.0.1 --port 61092","dev":"next dev"}}"#,
        )
        .unwrap();
        let mut service = test_service("store", &root.display().to_string(), Some("apps/web"));
        service.command = Some(vec!["pnpm".into(), "run".into(), "dev-localify".into()]);
        service.primary_target = Some(RouteTarget::Proxy(ProxyTarget::parse("52199").unwrap()));

        let service = service_with_dev_launch(service);

        assert_eq!(
            service.command,
            Some(vec!["pnpm".into(), "run".into(), "dev".into()])
        );
    }

    #[test]
    fn command_invokes_dev_localify_detects_legacy_package_runner_forms() {
        assert!(command_invokes_dev_localify(&[
            "pnpm".into(),
            "dev-localify".into()
        ]));
        assert!(command_invokes_dev_localify(&[
            "pnpm".into(),
            "run".into(),
            "dev-localify".into()
        ]));
        assert!(command_invokes_dev_localify(&[
            "yarn".into(),
            "dev-localify".into()
        ]));
        assert!(!command_invokes_dev_localify(&[
            "pnpm".into(),
            "run".into(),
            "dev".into()
        ]));
    }

    #[test]
    fn worktree_dev_launch_reuses_inferred_package_path() {
        let temp_dir = tempfile::tempdir().unwrap();
        let primary_root = temp_dir.path().join("primary");
        let worktree_root = temp_dir.path().join("worktree");
        let primary_app_dir = primary_root.join("apps/org-space");
        let worktree_app_dir = worktree_root.join("apps/org-space");
        std::fs::create_dir_all(&primary_app_dir).unwrap();
        std::fs::create_dir_all(&worktree_app_dir).unwrap();
        for root in [&primary_root, &worktree_root] {
            std::fs::write(
                root.join("package.json"),
                r#"{"private":true,"workspaces":["apps/*"],"packageManager":"pnpm@10.0.0"}"#,
            )
            .unwrap();
        }
        std::fs::write(
            primary_app_dir.join("package.json"),
            r#"{"name":"org-space","scripts":{"dev-localify":"next dev --hostname 127.0.0.1 --port 52164","dev":"next dev"}}"#,
        )
        .unwrap();
        std::fs::write(
            worktree_app_dir.join("package.json"),
            r#"{"name":"org-space","scripts":{"dev":"next dev"}}"#,
        )
        .unwrap();

        let mut service = test_service("acteur", &primary_root.display().to_string(), None);
        service.command = None;
        service.primary_target = Some(RouteTarget::Proxy(ProxyTarget::parse("52164").unwrap()));
        let inferred_path = inferred_path_from_configured_route_launch(&service);
        service.project_root_path = Some(worktree_root.display().to_string());
        service.path = inferred_path;
        service.primary_target = Some(RouteTarget::Proxy(ProxyTarget::parse("54481").unwrap()));

        let service = service_with_dev_launch(service);

        assert_eq!(service.path.as_deref(), Some("apps/org-space"));
        assert_eq!(
            service.command,
            Some(vec!["pnpm".into(), "run".into(), "dev".into()])
        );
    }

    #[test]
    fn service_with_dev_launch_uses_matching_service_folder_without_legacy_port() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        let app_dir = root.join("apps/email");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"private":true,"workspaces":["apps/*"],"packageManager":"pnpm@10.0.0"}"#,
        )
        .unwrap();
        std::fs::write(
            app_dir.join("package.json"),
            r#"{"name":"email","scripts":{"dev":"email dev --port 3003"}}"#,
        )
        .unwrap();
        let mut service = test_service("email", &root.display().to_string(), None);
        service.command = None;
        service.primary_target = Some(RouteTarget::Proxy(ProxyTarget::parse("52168").unwrap()));

        let service = service_with_dev_launch(service);

        assert_eq!(service.path.as_deref(), Some("apps/email"));
        assert_eq!(
            service.command,
            Some(vec!["pnpm".into(), "run".into(), "dev".into()])
        );
    }
}
