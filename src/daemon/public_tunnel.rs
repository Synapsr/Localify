//! Keeps this machine's tunnel in line with Localify Cloud
//! (`docs/public-urls.md`, section "Tunnel"). One task reads the
//! credentials and fetches the runtime, granting the ingress what it may
//! serve; another runs `cloudflared` while at least one URL is served here.

use std::collections::hash_map::RandomState;
use std::fmt;
use std::fs::File;
use std::hash::{BuildHasher, Hasher};
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use http_body_util::{BodyExt, Empty};
use hyper::body::Bytes;
use tokio::process::{Child, Command};
use tokio::sync::{Notify, watch};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::public_ingress::{IngressAccess, IngressGrant};
use crate::infrastructure::cloud::credentials::{CloudCredentials, CredentialsStore};
use crate::infrastructure::cloud::http::{CloudError, CloudHttp};
use crate::infrastructure::cloud::runtime::{ReadyRuntime, Runtime, fetch_runtime};
use crate::infrastructure::cloud::types::{
    PublicStatus, PublicUrl, PublicUrlState, RuntimeBlock, RuntimeStatus, TunnelState,
};
use crate::infrastructure::cloudflared;

const REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// After a failed fetch the daemon retries sooner, up to the normal interval.
const FIRST_RETRY: Duration = Duration::from_secs(10);
/// When a `provisioning` answer gives no `retryAfter`.
const PROVISIONING_RETRY: Duration = Duration::from_secs(3);
/// Added to `retryAfter`, at most this share of it, so machines spread out.
const RETRY_JITTER_PERCENT: u32 = 20;
/// A refresh on wake or on a failing `cloudflared` waits up to this long, so
/// machines waking together do not all ask at once.
const PROMPT_JITTER: Duration = Duration::from_secs(5);
/// Refreshes on wake or on a failing `cloudflared` come at most this often.
const MIN_PROMPT_GAP: Duration = Duration::from_secs(30);
/// How often the daemon compares the wall clock with the monotonic one.
const CLOCK_TICK: Duration = Duration::from_secs(20);
/// The wall clock moving this much more than the monotonic one means the
/// machine slept.
const SLEEP_GAP: Duration = Duration::from_secs(60);
/// `cloudflared` exiting twice within this window asks for the runtime again:
/// its tunnel may have been reaped or rotated.
const EXIT_WINDOW: Duration = Duration::from_secs(2 * 60);
const FIRST_RESTART: Duration = Duration::from_secs(2);
const MAX_RESTART: Duration = Duration::from_secs(60);
/// A `cloudflared` that ran this long before it stopped restarts quickly again.
const STABLE_RUN: Duration = Duration::from_secs(60);
const READY_TIMEOUT: Duration = Duration::from_secs(1);
const LOG_FILE_NAME: &str = "cloudflared.log";
/// In the data directory: an empty config, so `cloudflared` reads no other.
const CONFIG_FILE_NAME: &str = "cloudflared.yml";
const SIGNED_OUT_BY_CLOUD: &str =
    "Localify Cloud no longer accepts this machine's sign-in. Sign in again with `localify login`.";
/// The `403` codes of Localify Cloud that refuse this machine until something
/// changes (`docs/public-urls.md`, "API"). Any other `403` is a passing failure.
const TERMINAL_403_CODES: &[&str] = &["banned", "suspended", "taken_down", "quota_exceeded"];
const BANNED: &str = "Localify Cloud suspended this account for breaking its terms, so this machine serves no public URL. To appeal, write to abuse@localify.dev.";

/// The tunnel token. Its `Debug` leaves the value out, so it never reaches a log.
#[derive(Clone, PartialEq, Eq)]
struct TunnelToken(String);

impl fmt::Debug for TunnelToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TunnelToken(<redacted>)")
    }
}

/// The tunnel `cloudflared` should run: a new generation is a new tunnel.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Tunnel {
    generation: u64,
    token: TunnelToken,
}

#[derive(Debug, Clone, Default)]
struct Parts {
    signed_in: bool,
    served_urls: Vec<String>,
    /// Every URL of the last `ready` runtime, served here or not, and the
    /// account and machine they were fetched for: what the inventory says
    /// about each service's URL.
    known_urls: Vec<PublicUrl>,
    account: Option<Account>,
    /// Why the cloud side does not serve, for a person.
    cloud_error: Option<String>,
    /// Why public traffic cannot reach this daemon at all.
    ingress_error: Option<String>,
    /// The last state Localify Cloud gave; kept through a failed fetch.
    runtime: Option<RuntimeStatus>,
    blocked: Option<RuntimeBlock>,
    process: Process,
}

/// Who signed this machine in, without the token: a sign-in as someone else
/// or on another cloud makes the known URLs someone else's.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Account {
    api_base: String,
    login: String,
    machine_id: String,
}

impl Account {
    fn of(credentials: &CloudCredentials) -> Self {
        Self {
            api_base: credentials.api_base.clone(),
            login: credentials.login.clone(),
            machine_id: credentials.machine_id.clone(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
enum Process {
    #[default]
    Stopped,
    /// Wanted, and being downloaded or started.
    Starting,
    Running,
    Failed(String),
}

/// The handle the management socket holds: the status, and a way to ask
/// for a fetch now.
#[derive(Clone)]
pub struct PublicTunnel {
    parts: Arc<Mutex<Parts>>,
    refresh: Arc<Notify>,
    /// A refresh the daemon asks itself for, with jitter and a minimum gap.
    prompt: Arc<Notify>,
    metrics_port: u16,
}

impl PublicTunnel {
    pub fn new(ingress_port: u16) -> Self {
        Self {
            parts: Arc::default(),
            refresh: Arc::default(),
            prompt: Arc::default(),
            metrics_port: ingress_port.saturating_add(10),
        }
    }

    /// Fetches the runtime in the background; the caller does not wait.
    pub fn refresh(&self) {
        self.refresh.notify_one();
    }

    /// Records that the ingress listener could not start.
    pub fn ingress_failed(&self, error: String) {
        self.parts().ingress_error = Some(error);
    }

    /// This machine's status, with `connections` read from the metrics
    /// server of a running `cloudflared`.
    pub async fn status(&self) -> PublicStatus {
        let parts = self.parts().clone();
        let connections = match parts.process {
            Process::Running => Some(ready_connections(self.metrics_port).await),
            _ => None,
        };
        describe(parts, connections)
    }

    /// The URLs of the last `ready` runtime, and this machine's id, for the
    /// inventory. Empty while signed out or before the first answer.
    pub fn known_urls(&self) -> (Vec<PublicUrl>, Option<String>) {
        let parts = self.parts();
        let machine_id = parts
            .account
            .as_ref()
            .map(|account| account.machine_id.clone());
        (parts.known_urls.clone(), machine_id)
    }

    /// `known_urls` while `current`, the credentials on disk now, are those
    /// they were fetched for. A sign-out or a sign-in as someone else reaches
    /// the daemon at its next fetch; until then, nothing.
    pub fn known_urls_for(
        &self,
        current: Option<&CloudCredentials>,
    ) -> (Vec<PublicUrl>, Option<String>) {
        let parts = self.parts();
        match (&parts.account, current) {
            (Some(account), Some(current)) if *account == Account::of(current) => {
                (parts.known_urls.clone(), Some(account.machine_id.clone()))
            }
            _ => (Vec::new(), None),
        }
    }

    fn parts(&self) -> MutexGuard<'_, Parts> {
        self.parts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn set_process(&self, process: Process) {
        self.parts().process = process;
    }
}

fn describe(parts: Parts, connections: Option<u32>) -> PublicStatus {
    let (tunnel, connections, error) = match (parts.process, connections) {
        (Process::Failed(message), _) => (TunnelState::Error, 0, Some(message)),
        (_, _) if parts.signed_in && parts.ingress_error.is_some() => {
            (TunnelState::Error, 0, parts.ingress_error)
        }
        (Process::Running, Some(count)) if count > 0 => (TunnelState::Connected, count, None),
        (Process::Running | Process::Starting, _) => (TunnelState::Starting, 0, None),
        (Process::Stopped, _) => match parts.cloud_error {
            Some(message) => (TunnelState::Error, 0, Some(message)),
            None => (TunnelState::Stopped, 0, None),
        },
    };
    PublicStatus {
        signed_in: parts.signed_in,
        tunnel,
        connections,
        error,
        served_urls: parts.served_urls,
        runtime: parts.runtime,
        blocked: parts.blocked,
    }
}

/// Where the supervisor finds and writes its files.
pub struct TunnelFiles {
    pub data_dir: PathBuf,
    /// The daemon log; `cloudflared.log` goes next to it.
    pub daemon_log: PathBuf,
}

/// Starts both tasks. They stop with `cancel`, and `cloudflared` with them.
pub fn spawn(
    tunnel: PublicTunnel,
    access: IngressAccess,
    files: TunnelFiles,
    cancel: CancellationToken,
) {
    let (desired_tx, desired_rx) = watch::channel(None);
    let store = CredentialsStore::in_data_dir(&files.data_dir);
    let log = files
        .daemon_log
        .parent()
        .map(|dir| dir.join(LOG_FILE_NAME))
        .unwrap_or_else(|| files.data_dir.join(LOG_FILE_NAME));
    let process = ProcessFiles {
        data_dir: files.data_dir,
        log,
        metrics_port: tunnel.metrics_port,
    };
    tokio::spawn(watch_runtime(
        tunnel.clone(),
        store,
        access,
        desired_tx,
        cancel.clone(),
    ));
    tokio::spawn(supervise(tunnel, desired_rx, process, cancel));
}

// --- Runtime -----------------------------------------------------------------

/// When to fetch the runtime next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Next {
    /// At the regular interval.
    Regular,
    /// After this long, as the answer asked.
    After(Duration),
    /// The cloud could not be reached: soon, backing off.
    Retry,
}

async fn watch_runtime(
    tunnel: PublicTunnel,
    store: CredentialsStore,
    access: IngressAccess,
    desired: watch::Sender<Option<Tunnel>>,
    cancel: CancellationToken,
) {
    let mut failures: u32 = 0;
    let mut last_prompt: Option<Instant> = None;
    loop {
        let next = tokio::select! {
            _ = cancel.cancelled() => return,
            next = check_runtime(&tunnel, &store, &access, &desired) => next,
        };
        let wait = match next {
            Next::Regular => REFRESH_INTERVAL,
            Next::After(delay) => delay,
            Next::Retry => retry_delay(failures.saturating_add(1)),
        };
        failures = match next {
            Next::Retry => failures.saturating_add(1),
            _ => 0,
        };
        if !wait_for_next_fetch(&tunnel, wait, &mut last_prompt, &cancel).await {
            return;
        }
    }
}

/// Waits `wait`, less when `public_refresh` arrives, when the machine wakes
/// from sleep, or when `cloudflared` keeps exiting. `false` on shutdown.
async fn wait_for_next_fetch(
    tunnel: &PublicTunnel,
    wait: Duration,
    last_prompt: &mut Option<Instant>,
    cancel: &CancellationToken,
) -> bool {
    let mut deadline = Instant::now() + wait;
    loop {
        let clocks = Clocks::now();
        tokio::select! {
            _ = cancel.cancelled() => return false,
            _ = tunnel.refresh.notified() => return true,
            _ = tokio::time::sleep_until(deadline) => return true,
            _ = tunnel.prompt.notified() => {
                deadline = deadline.min(prompted_fetch(last_prompt));
            }
            _ = tokio::time::sleep(CLOCK_TICK) => {
                if clocks.slept() {
                    info!("This machine woke from sleep; fetching its public URLs again");
                    deadline = deadline.min(prompted_fetch(last_prompt));
                }
            }
        }
    }
}

/// When a refresh the daemon asked itself for runs: soon, with jitter, but
/// never within `MIN_PROMPT_GAP` of the previous one.
fn prompted_fetch(last_prompt: &mut Option<Instant>) -> Instant {
    let soon = Instant::now() + jitter(PROMPT_JITTER);
    let at = match *last_prompt {
        Some(last) => soon.max(last + MIN_PROMPT_GAP),
        None => soon,
    };
    *last_prompt = Some(at);
    at
}

/// The wall clock and the monotonic one, read together. The monotonic clock
/// stands still while the machine sleeps; the wall clock does not.
pub(crate) struct Clocks {
    wall: SystemTime,
    monotonic: std::time::Instant,
}

impl Clocks {
    pub(crate) fn now() -> Self {
        Self {
            wall: SystemTime::now(),
            monotonic: std::time::Instant::now(),
        }
    }

    pub(crate) fn slept(&self) -> bool {
        slept_between(
            self.wall.elapsed().unwrap_or_default(),
            self.monotonic.elapsed(),
        )
    }
}

pub(crate) fn slept_between(wall: Duration, monotonic: Duration) -> bool {
    wall > monotonic.saturating_add(SLEEP_GAP)
}

fn retry_delay(failures: u32) -> Duration {
    let factor = 2u32.saturating_pow(failures.saturating_sub(1));
    FIRST_RETRY.saturating_mul(factor).min(REFRESH_INTERVAL)
}

/// `retryAfter`, plus up to `RETRY_JITTER_PERCENT` of it.
fn after_retry(retry_after: Duration) -> Duration {
    retry_after + jitter(retry_after * RETRY_JITTER_PERCENT / 100)
}

/// A random duration up to `max`. The standard library seeds each
/// `RandomState` afresh, which is random enough to spread machines out.
fn jitter(max: Duration) -> Duration {
    let random = RandomState::new().build_hasher().finish();
    max.mul_f64((random % 1_000) as f64 / 1_000.0)
}

/// One pass. What it learned is already applied when it returns.
async fn check_runtime(
    tunnel: &PublicTunnel,
    store: &CredentialsStore,
    access: &IngressAccess,
    desired: &watch::Sender<Option<Tunnel>>,
) -> Next {
    let credentials = match store.load() {
        Ok(Some(credentials)) => credentials,
        Ok(None) => {
            stand_down(tunnel, access, desired, false, None);
            return Next::Regular;
        }
        Err(error) => {
            stand_down(tunnel, access, desired, false, Some(format!("{error:#}")));
            return Next::Regular;
        }
    };
    // The API base saved at sign-in, never LOCALIFY_CLOUD_API, so the token
    // only goes where it came from.
    let http = match CloudHttp::new(
        credentials.api_base.clone(),
        Some(credentials.token.clone()),
    ) {
        Ok(http) => http,
        Err(error) => {
            stand_down(tunnel, access, desired, true, Some(error.to_string()));
            return Next::Regular;
        }
    };
    let generation = desired
        .borrow()
        .as_ref()
        .map_or(0, |tunnel| tunnel.generation);
    match fetch_runtime(&http, generation).await {
        Ok(runtime) => apply(tunnel, access, desired, &credentials, runtime),
        Err(error) => refused(tunnel, access, desired, error),
    }
}

/// Acts on an answer, as the table in `docs/public-urls.md`, "Tunnel", says.
fn apply(
    tunnel: &PublicTunnel,
    access: &IngressAccess,
    desired: &watch::Sender<Option<Tunnel>>,
    credentials: &CloudCredentials,
    runtime: Runtime,
) -> Next {
    let (status, blocked, next) = match runtime {
        Runtime::Ready(ready) => return serve(tunnel, access, desired, credentials, ready),
        Runtime::Idle => (RuntimeStatus::Idle, None, Next::Regular),
        Runtime::Provisioning { retry_after } => (
            RuntimeStatus::Provisioning,
            None,
            Next::After(after_retry(retry_after.unwrap_or(PROVISIONING_RETRY))),
        ),
        Runtime::Blocked { block, retry_after } => {
            info!(reason = %block.reason, "Localify Cloud blocks this machine's public URLs");
            let next = retry_after.map_or(Next::Regular, |delay| Next::After(after_retry(delay)));
            (RuntimeStatus::Blocked, Some(block), next)
        }
    };
    stand_down(tunnel, access, desired, true, None);
    let mut parts = tunnel.parts();
    parts.runtime = Some(status);
    parts.blocked = blocked;
    next
}

/// An error answer. A refusal is final until something changes; a cloud
/// that cannot answer leaves what is served alone.
fn refused(
    tunnel: &PublicTunnel,
    access: &IngressAccess,
    desired: &watch::Sender<Option<Tunnel>>,
    error: CloudError,
) -> Next {
    match error {
        CloudError::Api { status: 401, .. } => {
            info!("Localify Cloud refused this machine's token; public URLs stop here");
            stand_down(
                tunnel,
                access,
                desired,
                false,
                Some(SIGNED_OUT_BY_CLOUD.into()),
            );
        }
        CloudError::Api {
            status: 402,
            code,
            message,
            upgrade,
            ..
        } if code == "subscription_required" => {
            stand_down(tunnel, access, desired, true, None);
            let mut parts = tunnel.parts();
            parts.runtime = Some(RuntimeStatus::Blocked);
            parts.blocked = Some(RuntimeBlock {
                reason: code,
                message,
                upgrade,
            });
        }
        // Only the API's own refusals, by their code: a 403 from anything in
        // between, such as a Cloudflare challenge page, is no reason to stop.
        CloudError::Api {
            status: 403,
            code,
            message,
            ..
        } if TERMINAL_403_CODES.contains(&code.as_str()) => {
            info!(code = %code, "Localify Cloud refuses this machine's public URLs");
            let banned = code == "banned";
            let error = if banned { BANNED.to_string() } else { message };
            stand_down(tunnel, access, desired, true, Some(error));
            if banned {
                tunnel.parts().runtime = Some(RuntimeStatus::Banned);
            }
        }
        error => {
            // A URL already served keeps its tunnel through a network blip.
            warn!(error = %error, "Could not fetch this machine's public URLs");
            let mut parts = tunnel.parts();
            parts.signed_in = true;
            parts.cloud_error = Some(error.to_string());
            return Next::Retry;
        }
    }
    Next::Regular
}

/// No tunnel, and every public request refused: signed out, revoked,
/// unusable credentials, or nothing this machine may serve.
fn stand_down(
    tunnel: &PublicTunnel,
    access: &IngressAccess,
    desired: &watch::Sender<Option<Tunnel>>,
    signed_in: bool,
    error: Option<String>,
) {
    access.revoke();
    desired.send_if_modified(|current| current.take().is_some());
    let mut parts = tunnel.parts();
    parts.signed_in = signed_in;
    parts.served_urls.clear();
    parts.known_urls.clear();
    parts.account = None;
    parts.cloud_error = error;
    parts.runtime = None;
    parts.blocked = None;
}

/// Serves a `ready` runtime. The ingress keeps its grant across a new
/// generation: only `cloudflared` changes.
fn serve(
    tunnel: &PublicTunnel,
    access: &IngressAccess,
    desired: &watch::Sender<Option<Tunnel>>,
    credentials: &CloudCredentials,
    ready: ReadyRuntime,
) -> Next {
    let known_urls = ready.public_urls.clone();
    // The API answers this machine's URLs only; keep to them all the same,
    // and to those it says may serve.
    let urls: Vec<_> = ready
        .public_urls
        .into_iter()
        .filter(|url| {
            url.machine
                .as_ref()
                .is_some_and(|machine| machine.id == credentials.machine_id)
                && matches!(url.state, None | Some(PublicUrlState::Serving))
        })
        .collect();
    // Without Localify's own ingress on its port, the tunnel would hand
    // public traffic, and the ingress secret, to whatever program holds it.
    let ingress_down = tunnel.parts().ingress_error.is_some();
    if urls.is_empty() || ingress_down {
        stand_down(tunnel, access, desired, true, None);
        let mut parts = tunnel.parts();
        parts.runtime = Some(RuntimeStatus::Ready);
        parts.known_urls = known_urls;
        parts.account = Some(Account::of(credentials));
        return Next::Regular;
    }
    let current = desired.borrow().clone();
    let wanted = match (ready.token, current) {
        (Some(token), _) => Tunnel {
            generation: ready.generation,
            token: TunnelToken(token),
        },
        (None, Some(current)) if current.generation == ready.generation => current,
        (None, _) => {
            let message = format!(
                "Localify Cloud sent no token for tunnel generation {}. Localify asks again shortly.",
                ready.generation
            );
            warn!(error = %message, "Could not serve this machine's public URLs");
            tunnel.parts().cloud_error = Some(message);
            return Next::Retry;
        }
    };
    access.grant(IngressGrant::new(&ready.ingress_secret, &urls));
    desired.send_if_modified(|current| {
        let changed = current.as_ref() != Some(&wanted);
        *current = Some(wanted);
        changed
    });
    let mut parts = tunnel.parts();
    parts.signed_in = true;
    parts.served_urls = urls.into_iter().map(|url| url.id).collect();
    parts.known_urls = known_urls;
    parts.account = Some(Account::of(credentials));
    parts.cloud_error = None;
    parts.runtime = Some(RuntimeStatus::Ready);
    parts.blocked = None;
    Next::Regular
}

// --- cloudflared -------------------------------------------------------------

struct ProcessFiles {
    data_dir: PathBuf,
    log: PathBuf,
    metrics_port: u16,
}

/// Runs `cloudflared` with the wanted tunnel, restarts it with backoff, and
/// stops it when no tunnel is wanted any more or on shutdown. A new
/// generation replaces the old process: by then Localify Cloud has deleted
/// the old tunnel, and two `cloudflared` cannot share the metrics port.
/// Exiting twice within `EXIT_WINDOW` asks for the runtime again, which is
/// how a machine learns its tunnel was reaped or rotated.
async fn supervise(
    tunnel: PublicTunnel,
    mut desired: watch::Receiver<Option<Tunnel>>,
    files: ProcessFiles,
    cancel: CancellationToken,
) {
    let mut child: Option<Child> = None;
    let mut current: Option<Tunnel> = None;
    let mut next_start: Option<Instant> = None;
    let mut started_at = Instant::now();
    let mut last_exit: Option<Instant> = None;
    let mut failures: u32 = 0;
    loop {
        let wanted = desired.borrow_and_update().clone();
        if wanted != current {
            stop(&mut child).await;
            last_exit = None;
            tunnel.set_process(match wanted {
                Some(_) => Process::Starting,
                None => Process::Stopped,
            });
            current = wanted;
            failures = 0;
            next_start = current.as_ref().map(|_| Instant::now());
        }
        tokio::select! {
            _ = cancel.cancelled() => break,
            changed = desired.changed() => if changed.is_err() { break },
            exit = wait_for_exit(&mut child) => {
                child = None;
                failures = if started_at.elapsed() >= STABLE_RUN {
                    1
                } else {
                    failures.saturating_add(1)
                };
                let delay = restart_delay(failures);
                let message = exit_message(exit, delay, &files.log);
                warn!(error = %message, "cloudflared stopped");
                tunnel.set_process(Process::Failed(message));
                next_start = Some(Instant::now() + delay);
                if last_exit.is_some_and(|last| last.elapsed() < EXIT_WINDOW) {
                    tunnel.prompt.notify_one();
                }
                last_exit = Some(Instant::now());
            }
            _ = sleep_until(next_start) => {
                next_start = None;
                let Some(wanted) = current.clone() else { continue };
                match start(&files, &wanted.token, &cancel).await {
                    Ok(started) => {
                        child = Some(started);
                        started_at = Instant::now();
                        tunnel.set_process(Process::Running);
                    }
                    Err(error) => {
                        failures = failures.saturating_add(1);
                        let delay = restart_delay(failures);
                        let message =
                            format!("{error:#}. Localify tries again in {} s.", delay.as_secs());
                        warn!(error = %message, "Could not start cloudflared");
                        tunnel.set_process(Process::Failed(message));
                        next_start = Some(Instant::now() + delay);
                    }
                }
            }
        }
    }
    stop(&mut child).await;
    tunnel.set_process(Process::Stopped);
}

async fn start(
    files: &ProcessFiles,
    token: &TunnelToken,
    cancel: &CancellationToken,
) -> Result<Child> {
    let binary = tokio::select! {
        _ = cancel.cancelled() => anyhow::bail!("The daemon is stopping"),
        binary = cloudflared::ensure_installed(&files.data_dir) => binary?,
    };
    // Without a config of its own, `cloudflared` reads `~/.cloudflared/config.yml`,
    // which belongs to the person's other tunnels.
    let config = files.data_dir.join(CONFIG_FILE_NAME);
    std::fs::write(&config, "{}\n")
        .with_context(|| format!("Could not write {}", config.display()))?;
    spawn_cloudflared(&binary, &config, token, files.metrics_port, &files.log)
}

/// The token goes through the environment only: a command line is visible
/// to every user of the machine. HTTP/2 over TCP, because on a network that
/// blocks UDP, `cloudflared` tries QUIC for about two minutes before it falls
/// back.
fn spawn_cloudflared(
    binary: &Path,
    config: &Path,
    token: &TunnelToken,
    metrics_port: u16,
    log: &Path,
) -> Result<Child> {
    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("Could not create {}", dir.display()))?;
    }
    let output = File::create(log).with_context(|| format!("Could not write {}", log.display()))?;
    let errors = output
        .try_clone()
        .with_context(|| format!("Could not write {}", log.display()))?;
    let child = Command::new(binary)
        .arg("--config")
        .arg(config)
        .arg("tunnel")
        .arg("--no-autoupdate")
        .arg("--protocol")
        .arg("http2")
        .arg("--metrics")
        .arg(format!("127.0.0.1:{metrics_port}"))
        .arg("run")
        .env("TUNNEL_TOKEN", &token.0)
        .stdin(Stdio::null())
        .stdout(Stdio::from(output))
        .stderr(Stdio::from(errors))
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("Could not start {}", binary.display()))?;
    info!(pid = child.id().unwrap_or_default(), "cloudflared started");
    Ok(child)
}

async fn stop(child: &mut Option<Child>) {
    let Some(mut running) = child.take() else {
        return;
    };
    let _ = running.start_kill();
    let _ = tokio::time::timeout(Duration::from_secs(5), running.wait()).await;
    info!("cloudflared stopped");
}

async fn wait_for_exit(child: &mut Option<Child>) -> std::io::Result<ExitStatus> {
    match child {
        Some(child) => child.wait().await,
        None => std::future::pending().await,
    }
}

async fn sleep_until(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

fn restart_delay(failures: u32) -> Duration {
    let factor = 2u32.saturating_pow(failures.saturating_sub(1));
    FIRST_RESTART.saturating_mul(factor).min(MAX_RESTART)
}

fn exit_message(exit: std::io::Result<ExitStatus>, delay: Duration, log: &Path) -> String {
    let how = match exit {
        Ok(status) => match status.code() {
            Some(code) => format!("with exit code {code}"),
            None => "after a signal".to_string(),
        },
        Err(error) => format!("({error})"),
    };
    format!(
        "cloudflared stopped {how}. Localify restarts it in {} s; its output is in {}.",
        delay.as_secs(),
        log.display()
    )
}

/// `readyConnections` from `cloudflared`'s `/ready`, or 0 when it does not
/// answer yet.
async fn ready_connections(metrics_port: u16) -> u32 {
    let client = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
        .build_http::<Empty<Bytes>>();
    let Ok(uri) = format!("http://127.0.0.1:{metrics_port}/ready").parse::<hyper::Uri>() else {
        return 0;
    };
    let read = async {
        let response = client.get(uri).await.ok()?;
        let body = response.into_body().collect().await.ok()?.to_bytes();
        let ready: serde_json::Value = serde_json::from_slice(&body).ok()?;
        ready["readyConnections"]
            .as_u64()
            .and_then(|count| u32::try_from(count).ok())
    };
    tokio::time::timeout(READY_TIMEOUT, read)
        .await
        .ok()
        .flatten()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::process::process_exists;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    const URLS: &str = r#"[
        {"id":"u1","slug":"journal-a8f3","url":"https://journal-a8f3.localify.fyi",
         "localDomain":"journal.localify","wildcard":false,"machine":{"id":"m1","name":"laptop"},
         "protection":{"mode":"password"},"state":"serving","createdAt":"2026-11-05T18:00:00Z"},
        {"id":"u2","slug":"other-0000","url":"https://other-0000.localify.fyi",
         "localDomain":"other.localify","wildcard":false,"machine":{"id":"m2","name":"runner"},
         "protection":{"mode":"password"},"state":"serving","createdAt":"2026-11-05T18:00:00Z"},
        {"id":"u3","slug":"paused","url":"https://paused.localify.fyi",
         "localDomain":"paused.localify","wildcard":false,"machine":{"id":"m1","name":"laptop"},
         "protection":{"mode":"password"},"state":"suspended","createdAt":"2026-11-05T18:00:00Z"}]"#;

    /// A `ready` runtime at `generation`, with `token` when given.
    fn ready(generation: u64, token: Option<&str>) -> String {
        let token = token.map_or("null".to_string(), |token| format!("\"{token}\""));
        format!(
            r#"{{"state":"ready","blocked":null,"tunnel":{{"generation":{generation},"token":{token}}},
                "ingressSecret":"s3cret","publicUrls":{URLS},"retryAfter":null}}"#
        )
    }

    fn blocked(reason: &str, retry_after: Option<u64>) -> String {
        let retry_after = retry_after.map_or("null".to_string(), |seconds| seconds.to_string());
        format!(
            r#"{{"state":"blocked","blocked":{{"reason":"{reason}","message":"Not now."}},
                "tunnel":null,"ingressSecret":null,"publicUrls":[],"retryAfter":{retry_after}}}"#
        )
    }

    const IDLE: &str = r#"{"state":"idle","blocked":null,"tunnel":null,"ingressSecret":null,
        "publicUrls":[],"retryAfter":null}"#;

    /// Answers `/v2/…` with `v2` and `/v1/…` with `v1`, and keeps the request
    /// lines.
    struct FakeApi {
        base: String,
        requests: Arc<Mutex<Vec<String>>>,
    }

    impl FakeApi {
        async fn v2(status: &'static str, body: impl Into<String>) -> Self {
            Self::serve((status, body.into()), ("404 Not Found", String::new())).await
        }

        async fn serve(v2: (&'static str, String), v1: (&'static str, String)) -> Self {
            let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let requests = Arc::new(Mutex::new(Vec::new()));
            let seen = requests.clone();
            tokio::spawn(async move {
                while let Ok((mut stream, _)) = listener.accept().await {
                    let mut buffer = vec![0; 4096];
                    let read = stream.read(&mut buffer).await.unwrap_or(0);
                    let request = String::from_utf8_lossy(&buffer[..read]).to_string();
                    let line = request.lines().next().unwrap_or_default().to_string();
                    let (status, body) = if line.contains(" /v2/") { &v2 } else { &v1 };
                    seen.lock().unwrap().push(line);
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                }
            });
            Self { base, requests }
        }

        fn requests(&self) -> Vec<String> {
            self.requests.lock().unwrap().clone()
        }
    }

    fn sign_in(dir: &Path, api_base: &str) -> CredentialsStore {
        let store = CredentialsStore::in_data_dir(dir);
        store
            .save(&CloudCredentials {
                api_base: api_base.into(),
                token: "lfy_t".into(),
                login: "teo".into(),
                machine_id: "m1".into(),
                machine_name: "laptop".into(),
            })
            .unwrap();
        store
    }

    fn generation(number: u64, token: &str) -> Option<Tunnel> {
        Some(Tunnel {
            generation: number,
            token: TunnelToken(token.into()),
        })
    }

    struct Fixture {
        dir: tempfile::TempDir,
        tunnel: PublicTunnel,
        access: IngressAccess,
        desired: watch::Sender<Option<Tunnel>>,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                dir: tempfile::tempdir().unwrap(),
                tunnel: PublicTunnel::new(1),
                access: IngressAccess::default(),
                desired: watch::channel(None).0,
            }
        }

        /// One pass against `api`, signed in.
        async fn check(&self, api: &FakeApi) -> Next {
            let store = sign_in(self.dir.path(), &api.base);
            self.check_store(&store).await
        }

        async fn check_store(&self, store: &CredentialsStore) -> Next {
            check_runtime(&self.tunnel, store, &self.access, &self.desired).await
        }

        /// Serves generation 1 with `tok-1`.
        async fn serving(self) -> Self {
            self.check(&FakeApi::v2("200 OK", ready(1, Some("tok-1"))).await)
                .await;
            assert_eq!(self.wanted(), generation(1, "tok-1"));
            self
        }

        fn wanted(&self) -> Option<Tunnel> {
            self.desired.borrow().clone()
        }

        async fn status(&self) -> PublicStatus {
            self.tunnel.status().await
        }
    }

    #[tokio::test]
    async fn a_machine_that_is_not_signed_in_serves_nothing() {
        let fixture = Fixture::new();

        let next = fixture
            .check_store(&CredentialsStore::in_data_dir(fixture.dir.path()))
            .await;

        assert_eq!(next, Next::Regular);
        assert!(!fixture.access.accepts_secret("s3cret"));
        assert_eq!(fixture.wanted(), None);
        let status = fixture.status().await;
        assert!(!status.signed_in);
        assert_eq!(status.tunnel, TunnelState::Stopped);
        assert_eq!(status.runtime, None);
    }

    #[tokio::test]
    async fn a_ready_runtime_grants_the_ingress_and_wants_its_generation() {
        let fixture = Fixture::new();
        let api = FakeApi::v2("200 OK", ready(1, Some("tok-1"))).await;

        assert_eq!(fixture.check(&api).await, Next::Regular);

        assert!(fixture.access.accepts_secret("s3cret"));
        assert!(!fixture.access.accepts_secret("s3cre"));
        assert_eq!(fixture.wanted(), generation(1, "tok-1"));
        let status = fixture.status().await;
        assert!(status.signed_in);
        assert_eq!(status.runtime, Some(RuntimeStatus::Ready));
        // u2 belongs to another machine and u3 is suspended: both are left out.
        assert_eq!(status.served_urls, vec!["u1".to_string()]);
        // The inventory still says why they are not served here.
        let (known, machine_id) = fixture.tunnel.known_urls();
        let ids: Vec<_> = known.iter().map(|url| url.id.as_str()).collect();
        assert_eq!(ids, ["u1", "u2", "u3"]);
        assert_eq!(machine_id.as_deref(), Some("m1"));
        assert_eq!(
            api.requests(),
            ["GET /v2/machines/self/runtime?generation=0 HTTP/1.1"]
        );
    }

    #[tokio::test]
    async fn known_urls_are_given_only_to_the_account_they_were_fetched_for() {
        let fixture = Fixture::new();
        let api = FakeApi::v2("200 OK", ready(1, Some("tok-1"))).await;
        let store = sign_in(fixture.dir.path(), &api.base);
        fixture.check_store(&store).await;
        let current = store.load().unwrap();

        let (urls, machine_id) = fixture.tunnel.known_urls_for(current.as_ref());
        assert_eq!(urls.len(), 3);
        assert_eq!(machine_id.as_deref(), Some("m1"));

        // Signed out, before the next fetch.
        assert_eq!(fixture.tunnel.known_urls_for(None), (Vec::new(), None));
        // Signed in again as someone else.
        let mut other = current.unwrap();
        other.login = "ana".into();
        assert_eq!(
            fixture.tunnel.known_urls_for(Some(&other)),
            (Vec::new(), None)
        );
    }

    #[tokio::test]
    async fn the_same_generation_keeps_the_tunnel_and_names_it_in_the_request() {
        let fixture = Fixture::new().serving().await;
        let api = FakeApi::v2("200 OK", ready(1, None)).await;

        assert_eq!(fixture.check(&api).await, Next::Regular);

        assert_eq!(fixture.wanted(), generation(1, "tok-1"));
        assert!(fixture.access.accepts_secret("s3cret"));
        assert_eq!(
            api.requests(),
            ["GET /v2/machines/self/runtime?generation=1 HTTP/1.1"]
        );
    }

    #[tokio::test]
    async fn a_new_generation_replaces_the_token_and_keeps_the_ingress() {
        let fixture = Fixture::new().serving().await;
        let mut changes = fixture.desired.subscribe();
        changes.mark_unchanged();

        fixture
            .check(&FakeApi::v2("200 OK", ready(2, Some("tok-2"))).await)
            .await;

        assert_eq!(fixture.wanted(), generation(2, "tok-2"));
        assert!(changes.has_changed().unwrap());
        // Granted throughout: the ingress never saw a revoke.
        assert!(fixture.access.accepts_secret("s3cret"));
    }

    #[tokio::test]
    async fn a_missing_token_for_an_unknown_generation_keeps_what_is_served() {
        let fixture = Fixture::new().serving().await;

        let next = fixture
            .check(&FakeApi::v2("200 OK", ready(2, None)).await)
            .await;

        assert_eq!(next, Next::Retry);
        assert_eq!(fixture.wanted(), generation(1, "tok-1"));
        assert!(fixture.access.accepts_secret("s3cret"));
        assert!(
            fixture
                .status()
                .await
                .error
                .unwrap()
                .contains("generation 2")
        );
    }

    #[tokio::test]
    async fn a_cloud_without_v2_serves_through_v1() {
        let fixture = Fixture::new();
        let v1 =
            format!(r#"{{"tunnelToken":"tok-v1","ingressSecret":"s3cret","publicUrls":{URLS}}}"#);
        let api = FakeApi::serve(
            (
                "404 Not Found",
                r#"{"error":"not_found","message":"No."}"#.into(),
            ),
            ("200 OK", v1),
        )
        .await;

        assert_eq!(fixture.check(&api).await, Next::Regular);

        assert_eq!(fixture.wanted(), generation(0, "tok-v1"));
        assert!(fixture.access.accepts_secret("s3cret"));
        assert_eq!(
            api.requests(),
            [
                "GET /v2/machines/self/runtime?generation=0 HTTP/1.1",
                "GET /v1/machines/self/runtime HTTP/1.1"
            ]
        );
    }

    #[tokio::test]
    async fn no_tunnel_runs_when_the_ingress_could_not_listen() {
        let fixture = Fixture::new();
        fixture.tunnel.ingress_failed("port taken".into());

        fixture
            .check(&FakeApi::v2("200 OK", ready(1, Some("tok-1"))).await)
            .await;

        assert!(!fixture.access.accepts_secret("s3cret"));
        assert_eq!(fixture.wanted(), None);
        let status = fixture.status().await;
        assert_eq!(status.tunnel, TunnelState::Error);
        assert_eq!(status.error.as_deref(), Some("port taken"));
    }

    #[tokio::test]
    async fn idle_stops_the_tunnel_and_drops_the_secret() {
        let fixture = Fixture::new().serving().await;

        let next = fixture.check(&FakeApi::v2("200 OK", IDLE).await).await;

        assert_eq!(next, Next::Regular);
        assert!(!fixture.access.accepts_secret("s3cret"));
        assert_eq!(fixture.wanted(), None);
        let status = fixture.status().await;
        assert_eq!(status.tunnel, TunnelState::Stopped);
        assert_eq!(status.runtime, Some(RuntimeStatus::Idle));
        assert!(status.signed_in && status.error.is_none());
    }

    #[tokio::test]
    async fn provisioning_runs_nothing_and_asks_again_after_retry_after() {
        let fixture = Fixture::new();
        let provisioning = r#"{"state":"provisioning","blocked":null,"tunnel":null,
            "ingressSecret":null,"publicUrls":[],"retryAfter":3}"#;

        let next = fixture
            .check(&FakeApi::v2("200 OK", provisioning).await)
            .await;

        let Next::After(delay) = next else {
            panic!("expected a retry after 3 s, got {next:?}");
        };
        assert!(delay >= Duration::from_secs(3) && delay <= Duration::from_millis(3600));
        assert_eq!(fixture.wanted(), None);
        assert!(!fixture.access.accepts_secret("s3cret"));
        assert_eq!(
            fixture.status().await.runtime,
            Some(RuntimeStatus::Provisioning)
        );
    }

    #[tokio::test]
    async fn a_blocked_runtime_stops_serving_stays_signed_in_and_says_why() {
        let fixture = Fixture::new().serving().await;

        let next = fixture
            .check(&FakeApi::v2("200 OK", blocked("subscription_required", None)).await)
            .await;

        // Final until a plan or URL change: no early retry.
        assert_eq!(next, Next::Regular);
        assert!(!fixture.access.accepts_secret("s3cret"));
        assert_eq!(fixture.wanted(), None);
        let status = fixture.status().await;
        assert!(status.signed_in);
        assert_eq!(status.tunnel, TunnelState::Stopped);
        assert_eq!(status.runtime, Some(RuntimeStatus::Blocked));
        assert_eq!(
            status.blocked,
            Some(RuntimeBlock {
                reason: "subscription_required".into(),
                message: "Not now.".into(),
                upgrade: None,
            })
        );
        assert!(status.served_urls.is_empty());
        assert!(fixture.tunnel.known_urls().0.is_empty());
    }

    #[tokio::test]
    async fn the_account_cap_asks_again_after_its_retry_after() {
        let fixture = Fixture::new();

        let next = fixture
            .check(&FakeApi::v2("200 OK", blocked("capacity", Some(60))).await)
            .await;

        let Next::After(delay) = next else {
            panic!("expected a retry after 60 s, got {next:?}");
        };
        assert!(delay >= Duration::from_secs(60) && delay <= Duration::from_secs(72));
    }

    #[tokio::test]
    async fn a_402_blocks_like_a_runtime_without_a_plan() {
        let fixture = Fixture::new().serving().await;
        let refused = r#"{"error":"subscription_required","message":"Subscribe."}"#;

        let next = fixture
            .check(&FakeApi::v2("402 Payment Required", refused).await)
            .await;

        assert_eq!(next, Next::Regular);
        assert_eq!(fixture.wanted(), None);
        assert!(!fixture.access.accepts_secret("s3cret"));
        let status = fixture.status().await;
        assert!(status.signed_in);
        assert_eq!(status.runtime, Some(RuntimeStatus::Blocked));
        assert_eq!(status.blocked.unwrap().reason, "subscription_required");
    }

    #[tokio::test]
    async fn a_banned_account_stops_everything_and_stays_signed_in() {
        let fixture = Fixture::new().serving().await;
        let refused = r#"{"error":"banned","message":"Banned."}"#;

        let next = fixture
            .check(&FakeApi::v2("403 Forbidden", refused).await)
            .await;

        assert_eq!(next, Next::Regular);
        assert_eq!(fixture.wanted(), None);
        assert!(!fixture.access.accepts_secret("s3cret"));
        let status = fixture.status().await;
        assert!(status.signed_in);
        assert_eq!(status.runtime, Some(RuntimeStatus::Banned));
        assert_eq!(status.tunnel, TunnelState::Error);
        assert!(status.error.unwrap().contains("abuse@localify.dev"));
    }

    #[tokio::test]
    async fn a_refusal_without_a_known_code_keeps_what_is_served() {
        let fixture = Fixture::new().serving().await;
        let refusals = [
            // A Cloudflare challenge page, not the API.
            ("403 Forbidden", "<html>Just a moment…</html>"),
            ("402 Payment Required", "<html>Payment required</html>"),
            ("403 Forbidden", r#"{"error":"forbidden","message":"No."}"#),
        ];

        for (status, body) in refusals {
            let next = fixture.check(&FakeApi::v2(status, body).await).await;

            assert_eq!(next, Next::Retry, "{status} {body}");
            assert_eq!(fixture.wanted(), generation(1, "tok-1"), "{status} {body}");
            assert!(fixture.access.accepts_secret("s3cret"), "{status} {body}");
            let status = fixture.status().await;
            assert_eq!(status.runtime, Some(RuntimeStatus::Ready));
            assert_eq!(status.served_urls, vec!["u1".to_string()]);
        }
    }

    #[tokio::test]
    async fn a_suspended_account_stops_serving_and_says_why() {
        let fixture = Fixture::new().serving().await;
        let refused = r#"{"error":"suspended","message":"This account is suspended."}"#;

        let next = fixture
            .check(&FakeApi::v2("403 Forbidden", refused).await)
            .await;

        assert_eq!(next, Next::Regular);
        assert_eq!(fixture.wanted(), None);
        let status = fixture.status().await;
        assert!(status.signed_in);
        assert_eq!(status.error.as_deref(), Some("This account is suspended."));
    }

    #[tokio::test]
    async fn a_revoked_sign_in_stops_everything_and_says_so() {
        let fixture = Fixture::new().serving().await;
        let revoked = r#"{"error":"unauthorized","message":"Sign in again."}"#;

        fixture
            .check(&FakeApi::v2("401 Unauthorized", revoked).await)
            .await;

        assert!(!fixture.access.accepts_secret("s3cret"));
        assert_eq!(fixture.wanted(), None);
        let status = fixture.status().await;
        assert!(!status.signed_in);
        assert_eq!(status.tunnel, TunnelState::Error);
        assert!(status.error.unwrap().contains("Sign in again"));
        assert!(status.served_urls.is_empty());
    }

    #[tokio::test]
    async fn deleted_credentials_stop_everything() {
        let fixture = Fixture::new().serving().await;
        let store = CredentialsStore::in_data_dir(fixture.dir.path());

        store.delete().unwrap();
        fixture.check_store(&store).await;

        assert!(!fixture.access.accepts_secret("s3cret"));
        assert_eq!(fixture.wanted(), None);
        assert!(!fixture.status().await.signed_in);
    }

    #[tokio::test]
    async fn an_unreachable_cloud_keeps_what_is_already_served() {
        let fixture = Fixture::new().serving().await;

        for failing in ["502 Bad Gateway", "429 Too Many Requests"] {
            let next = fixture.check(&FakeApi::v2(failing, "<html>").await).await;

            assert_eq!(next, Next::Retry);
            assert!(fixture.access.accepts_secret("s3cret"));
            assert_eq!(fixture.wanted(), generation(1, "tok-1"));
            let status = fixture.status().await;
            assert_eq!(status.runtime, Some(RuntimeStatus::Ready));
            assert_eq!(status.served_urls, vec!["u1".to_string()]);
        }

        let gone = sign_in(fixture.dir.path(), "http://127.0.0.1:1");
        assert_eq!(fixture.check_store(&gone).await, Next::Retry);
        assert_eq!(fixture.wanted(), generation(1, "tok-1"));
    }

    #[tokio::test]
    async fn a_first_fetch_that_fails_is_an_error_for_a_person() {
        let fixture = Fixture::new();

        let next = fixture
            .check(&FakeApi::v2("502 Bad Gateway", "<html>").await)
            .await;

        assert_eq!(next, Next::Retry);
        let status = fixture.status().await;
        assert!(status.signed_in);
        assert_eq!(status.tunnel, TunnelState::Error);
        assert!(status.error.unwrap().contains("502"));
    }

    #[tokio::test]
    async fn credentials_for_a_plain_http_remote_api_are_not_used() {
        let fixture = Fixture::new();

        fixture
            .check_store(&sign_in(fixture.dir.path(), "http://api.example"))
            .await;

        assert_eq!(fixture.wanted(), None);
        assert_eq!(fixture.status().await.tunnel, TunnelState::Error);
    }

    #[test]
    fn the_status_reports_the_most_useful_problem() {
        let parts = |process| Parts {
            signed_in: true,
            served_urls: vec!["u1".into()],
            cloud_error: Some("offline".into()),
            ingress_error: None,
            runtime: Some(RuntimeStatus::Ready),
            blocked: None,
            process,
            ..Parts::default()
        };
        let status = describe(parts(Process::Running), Some(4));
        assert_eq!(
            (status.tunnel, status.connections),
            (TunnelState::Connected, 4)
        );
        assert_eq!(status.error, None);
        assert_eq!(status.runtime, Some(RuntimeStatus::Ready));

        let status = describe(parts(Process::Running), Some(0));
        assert_eq!(status.tunnel, TunnelState::Starting);

        let status = describe(parts(Process::Starting), None);
        assert_eq!((status.tunnel, status.error), (TunnelState::Starting, None));

        let status = describe(parts(Process::Failed("crashed".into())), None);
        assert_eq!(
            (status.tunnel, status.error.as_deref()),
            (TunnelState::Error, Some("crashed"))
        );

        let status = describe(parts(Process::Stopped), None);
        assert_eq!(
            (status.tunnel, status.error.as_deref()),
            (TunnelState::Error, Some("offline"))
        );

        let mut no_port = parts(Process::Running);
        no_port.ingress_error = Some("port taken".into());
        let status = describe(no_port, Some(4));
        assert_eq!(status.error.as_deref(), Some("port taken"));
    }

    #[test]
    fn retries_back_off_up_to_their_cap() {
        assert_eq!(retry_delay(1), FIRST_RETRY);
        assert_eq!(retry_delay(2), FIRST_RETRY * 2);
        assert_eq!(retry_delay(40), REFRESH_INTERVAL);
        assert_eq!(restart_delay(1), FIRST_RESTART);
        assert_eq!(restart_delay(40), MAX_RESTART);
        for _ in 0..50 {
            let delay = after_retry(Duration::from_secs(10));
            assert!(delay >= Duration::from_secs(10) && delay <= Duration::from_secs(12));
            assert!(jitter(PROMPT_JITTER) <= PROMPT_JITTER);
        }
    }

    #[test]
    fn a_wall_clock_ahead_of_the_monotonic_one_means_the_machine_slept() {
        let tick = Duration::from_secs(20);
        assert!(!slept_between(tick, tick));
        assert!(!slept_between(tick + Duration::from_secs(30), tick));
        assert!(slept_between(tick + Duration::from_secs(61), tick));
        // A wall clock set back is no sleep.
        assert!(!slept_between(Duration::ZERO, tick));
    }

    #[tokio::test]
    async fn prompted_refreshes_keep_their_minimum_gap() {
        let mut last = None;
        let first = prompted_fetch(&mut last);
        assert!(first <= Instant::now() + PROMPT_JITTER);

        let second = prompted_fetch(&mut last);
        assert!(second >= first + MIN_PROMPT_GAP);
    }

    #[test]
    fn the_token_never_shows_in_debug_output() {
        assert!(!format!("{:?}", generation(1, "tok-1")).contains("tok-1"));
    }

    /// Records its arguments, whether it got the token, and its pid, then
    /// waits like `cloudflared` would.
    const FAKE_CLOUDFLARED: &str = r#"#!/bin/sh
dir="$(dirname "$0")"
printf '%s\n' "$@" > "$dir/args"
printf "%s" "$TUNNEL_TOKEN" > "$dir/token"
echo $$ > "$dir/pid"
echo "fresh output"
exec sleep 30
"#;

    async fn wait_until(condition: impl Fn() -> bool) {
        for _ in 0..500 {
            if condition() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("timed out");
    }

    fn install_fake(data_dir: &Path, script: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let binary = cloudflared::binary_path(data_dir);
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        std::fs::write(&binary, script).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        binary.parent().unwrap().to_path_buf()
    }

    #[tokio::test]
    async fn cloudflared_runs_with_the_token_in_its_environment_and_stops_when_unwanted() {
        let dir = tempfile::tempdir().unwrap();
        let bin = install_fake(dir.path(), FAKE_CLOUDFLARED);
        let log = dir.path().join("logs").join(LOG_FILE_NAME);
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();
        std::fs::write(&log, "stale output from the previous run\n").unwrap();
        let tunnel = PublicTunnel::new(7381);
        let (desired, wanted) = watch::channel(None);
        let cancel = CancellationToken::new();
        let files = ProcessFiles {
            data_dir: dir.path().to_path_buf(),
            log: log.clone(),
            metrics_port: 7391,
        };
        let task = tokio::spawn(supervise(tunnel.clone(), wanted, files, cancel.clone()));

        desired.send_replace(generation(1, "tok-1"));
        wait_until(|| bin.join("pid").exists()).await;

        let args = std::fs::read_to_string(bin.join("args")).unwrap();
        assert_eq!(
            args.lines().collect::<Vec<_>>(),
            [
                "--config",
                dir.path().join(CONFIG_FILE_NAME).to_str().unwrap(),
                "tunnel",
                "--no-autoupdate",
                "--protocol",
                "http2",
                "--metrics",
                "127.0.0.1:7391",
                "run"
            ]
        );
        assert!(!args.contains("tok-1"));
        assert_eq!(
            std::fs::read_to_string(dir.path().join(CONFIG_FILE_NAME)).unwrap(),
            "{}\n"
        );
        assert_eq!(std::fs::read_to_string(bin.join("token")).unwrap(), "tok-1");
        wait_until(|| std::fs::read_to_string(&log).is_ok_and(|text| text == "fresh output\n"))
            .await;
        assert_eq!(tunnel.parts().process, Process::Running);
        let pid: u32 = std::fs::read_to_string(bin.join("pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();

        desired.send_replace(None);
        wait_until(|| !process_exists(pid)).await;
        wait_until(|| tunnel.parts().process == Process::Stopped).await;

        cancel.cancel();
        task.await.unwrap();
    }

    #[tokio::test]
    async fn shutdown_kills_cloudflared() {
        let dir = tempfile::tempdir().unwrap();
        let bin = install_fake(dir.path(), FAKE_CLOUDFLARED);
        let tunnel = PublicTunnel::new(7381);
        let (desired, wanted) = watch::channel(generation(1, "tok-1"));
        let cancel = CancellationToken::new();
        let files = ProcessFiles {
            data_dir: dir.path().to_path_buf(),
            log: dir.path().join(LOG_FILE_NAME),
            metrics_port: 7391,
        };
        let task = tokio::spawn(supervise(tunnel.clone(), wanted, files, cancel.clone()));
        wait_until(|| bin.join("pid").exists()).await;
        let pid: u32 = std::fs::read_to_string(bin.join("pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();

        cancel.cancel();
        task.await.unwrap();

        assert!(!process_exists(pid));
        assert_eq!(tunnel.parts().process, Process::Stopped);
        drop(desired);
    }

    #[tokio::test]
    async fn a_cloudflared_that_exits_is_an_error_pointing_at_its_log() {
        let dir = tempfile::tempdir().unwrap();
        install_fake(dir.path(), "#!/bin/sh\necho 'failed to connect'\nexit 3\n");
        let log = dir.path().join(LOG_FILE_NAME);
        let tunnel = PublicTunnel::new(7381);
        let (_desired, wanted) = watch::channel(generation(1, "tok-1"));
        let cancel = CancellationToken::new();
        let files = ProcessFiles {
            data_dir: dir.path().to_path_buf(),
            log: log.clone(),
            metrics_port: 7391,
        };
        let task = tokio::spawn(supervise(tunnel.clone(), wanted, files, cancel.clone()));

        wait_until(|| matches!(tunnel.parts().process, Process::Failed(_))).await;

        let status = tunnel.status().await;
        assert_eq!(status.tunnel, TunnelState::Error);
        let error = status.error.unwrap();
        assert!(error.contains("exit code 3"), "{error}");
        assert!(error.contains(&log.display().to_string()), "{error}");
        cancel.cancel();
        task.await.unwrap();
    }

    #[tokio::test]
    async fn a_cloudflared_that_exits_twice_asks_for_the_runtime_again() {
        let dir = tempfile::tempdir().unwrap();
        install_fake(dir.path(), "#!/bin/sh\nexit 1\n");
        let tunnel = PublicTunnel::new(7381);
        let (_desired, wanted) = watch::channel(generation(1, "tok-1"));
        let cancel = CancellationToken::new();
        let files = ProcessFiles {
            data_dir: dir.path().to_path_buf(),
            log: dir.path().join(LOG_FILE_NAME),
            metrics_port: 7391,
        };
        let task = tokio::spawn(supervise(tunnel.clone(), wanted, files, cancel.clone()));

        // The first exit restarts it after 2 s; the second one prompts.
        tokio::time::timeout(Duration::from_secs(10), tunnel.prompt.notified())
            .await
            .expect("no refresh was asked for");

        cancel.cancel();
        task.await.unwrap();
    }

    #[tokio::test]
    async fn a_new_generation_restarts_cloudflared_on_its_token() {
        let dir = tempfile::tempdir().unwrap();
        let bin = install_fake(dir.path(), FAKE_CLOUDFLARED);
        let tunnel = PublicTunnel::new(7381);
        let (desired, wanted) = watch::channel(generation(1, "tok-1"));
        let cancel = CancellationToken::new();
        let files = ProcessFiles {
            data_dir: dir.path().to_path_buf(),
            log: dir.path().join(LOG_FILE_NAME),
            metrics_port: 7391,
        };
        let task = tokio::spawn(supervise(tunnel.clone(), wanted, files, cancel.clone()));
        let pid_of = || {
            std::fs::read_to_string(bin.join("pid"))
                .ok()
                .and_then(|pid| pid.trim().parse::<u32>().ok())
        };
        wait_until(|| pid_of().is_some()).await;
        let first = pid_of().unwrap();

        desired.send_replace(generation(2, "tok-2"));

        wait_until(|| {
            std::fs::read_to_string(bin.join("token")).is_ok_and(|token| token == "tok-2")
        })
        .await;
        wait_until(|| !process_exists(first)).await;
        wait_until(|| pid_of().is_some_and(|pid| pid != first)).await;
        cancel.cancel();
        task.await.unwrap();
    }
}
