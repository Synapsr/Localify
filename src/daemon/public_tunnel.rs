//! Keeps this machine's tunnel in line with Localify Cloud
//! (`docs/public-urls.md`, section "Tunnel"). One task reads the
//! credentials and fetches the runtime, granting the ingress what it may
//! serve; another runs `cloudflared` while at least one URL is served here.

use std::fmt;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

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
use crate::infrastructure::cloud::runtime::fetch_runtime;
use crate::infrastructure::cloud::types::{MachineRuntime, PublicStatus, TunnelState};
use crate::infrastructure::cloudflared;

const REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// After a failed fetch the daemon retries sooner, up to the normal interval.
const FIRST_RETRY: Duration = Duration::from_secs(10);
const FIRST_RESTART: Duration = Duration::from_secs(2);
const MAX_RESTART: Duration = Duration::from_secs(60);
/// A `cloudflared` that ran this long before it stopped restarts quickly again.
const STABLE_RUN: Duration = Duration::from_secs(60);
const READY_TIMEOUT: Duration = Duration::from_secs(1);
const LOG_FILE_NAME: &str = "cloudflared.log";
const SIGNED_OUT_BY_CLOUD: &str =
    "Localify Cloud no longer accepts this machine's sign-in. Sign in again with `localify login`.";

/// The tunnel token. Its `Debug` leaves the value out, so it never reaches a log.
#[derive(Clone, PartialEq, Eq)]
struct TunnelToken(String);

impl fmt::Debug for TunnelToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TunnelToken(<redacted>)")
    }
}

#[derive(Debug, Clone, Default)]
struct Parts {
    signed_in: bool,
    served_urls: Vec<String>,
    /// Why the cloud side does not serve, for a person.
    cloud_error: Option<String>,
    /// Why public traffic cannot reach this daemon at all.
    ingress_error: Option<String>,
    process: Process,
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
    metrics_port: u16,
}

impl PublicTunnel {
    pub fn new(ingress_port: u16) -> Self {
        Self {
            parts: Arc::default(),
            refresh: Arc::default(),
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

async fn watch_runtime(
    tunnel: PublicTunnel,
    store: CredentialsStore,
    access: IngressAccess,
    desired: watch::Sender<Option<TunnelToken>>,
    cancel: CancellationToken,
) {
    let mut failures: u32 = 0;
    loop {
        let fetched = tokio::select! {
            _ = cancel.cancelled() => return,
            fetched = check_runtime(&tunnel, &store, &access, &desired) => fetched,
        };
        let wait = if fetched {
            failures = 0;
            REFRESH_INTERVAL
        } else {
            failures = failures.saturating_add(1);
            retry_delay(failures)
        };
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tunnel.refresh.notified() => {}
            _ = tokio::time::sleep(wait) => {}
        }
    }
}

fn retry_delay(failures: u32) -> Duration {
    let factor = 2u32.saturating_pow(failures.saturating_sub(1));
    FIRST_RETRY.saturating_mul(factor).min(REFRESH_INTERVAL)
}

/// One pass: `false` when the cloud could not be reached and the daemon
/// should retry soon. What it learned is already applied.
async fn check_runtime(
    tunnel: &PublicTunnel,
    store: &CredentialsStore,
    access: &IngressAccess,
    desired: &watch::Sender<Option<TunnelToken>>,
) -> bool {
    let credentials = match store.load() {
        Ok(Some(credentials)) => credentials,
        Ok(None) => {
            stand_down(tunnel, access, desired, false, None);
            return true;
        }
        Err(error) => {
            stand_down(tunnel, access, desired, false, Some(format!("{error:#}")));
            return true;
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
            return true;
        }
    };
    match fetch_runtime(&http).await {
        Ok(runtime) => {
            serve(tunnel, access, desired, &credentials, runtime);
            true
        }
        Err(CloudError::Api { status: 401, .. }) => {
            info!("Localify Cloud refused this machine's token; public URLs stop here");
            stand_down(
                tunnel,
                access,
                desired,
                false,
                Some(SIGNED_OUT_BY_CLOUD.into()),
            );
            true
        }
        Err(error) => {
            // A URL already served keeps its tunnel through a network blip.
            warn!(error = %error, "Could not fetch this machine's public URLs");
            let mut parts = tunnel.parts();
            parts.signed_in = true;
            parts.cloud_error = Some(error.to_string());
            false
        }
    }
}

/// Signed out, revoked, or unusable credentials: no tunnel, and every public
/// request refused.
fn stand_down(
    tunnel: &PublicTunnel,
    access: &IngressAccess,
    desired: &watch::Sender<Option<TunnelToken>>,
    signed_in: bool,
    error: Option<String>,
) {
    access.revoke();
    desired.send_if_modified(|current| current.take().is_some());
    let mut parts = tunnel.parts();
    parts.signed_in = signed_in;
    parts.served_urls.clear();
    parts.cloud_error = error;
}

fn serve(
    tunnel: &PublicTunnel,
    access: &IngressAccess,
    desired: &watch::Sender<Option<TunnelToken>>,
    credentials: &CloudCredentials,
    runtime: MachineRuntime,
) {
    // The API answers this machine's URLs only; keep to them all the same.
    let urls: Vec<_> = runtime
        .public_urls
        .into_iter()
        .filter(|url| {
            url.machine
                .as_ref()
                .is_some_and(|machine| machine.id == credentials.machine_id)
        })
        .collect();
    // Without Localify's own ingress on its port, the tunnel would hand
    // public traffic, and the ingress secret, to whatever program holds it.
    let ingress_down = tunnel.parts().ingress_error.is_some();
    let wanted = if urls.is_empty() || ingress_down {
        access.revoke();
        None
    } else {
        access.grant(IngressGrant::new(&runtime.ingress_secret, &urls));
        Some(TunnelToken(runtime.tunnel_token))
    };
    desired.send_if_modified(|current| {
        let changed = *current != wanted;
        *current = wanted;
        changed
    });
    let mut parts = tunnel.parts();
    parts.signed_in = true;
    parts.served_urls = urls.into_iter().map(|url| url.id).collect();
    parts.cloud_error = None;
}

// --- cloudflared -------------------------------------------------------------

struct ProcessFiles {
    data_dir: PathBuf,
    log: PathBuf,
    metrics_port: u16,
}

/// Runs `cloudflared` with the wanted token, restarts it with backoff, and
/// stops it when no token is wanted any more or on shutdown.
async fn supervise(
    tunnel: PublicTunnel,
    mut desired: watch::Receiver<Option<TunnelToken>>,
    files: ProcessFiles,
    cancel: CancellationToken,
) {
    let mut child: Option<Child> = None;
    let mut current: Option<TunnelToken> = None;
    let mut next_start: Option<Instant> = None;
    let mut started_at = Instant::now();
    let mut failures: u32 = 0;
    loop {
        let wanted = desired.borrow_and_update().clone();
        if wanted != current {
            stop(&mut child).await;
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
            }
            _ = sleep_until(next_start) => {
                next_start = None;
                let Some(token) = current.clone() else { continue };
                match start(&files, &token, &cancel).await {
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
    spawn_cloudflared(&binary, token, files.metrics_port, &files.log)
}

/// The token goes through the environment only: a command line is visible
/// to every user of the machine.
fn spawn_cloudflared(
    binary: &Path,
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
        .arg("tunnel")
        .arg("--no-autoupdate")
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
    use crate::infrastructure::process::{ProcessControl, get_process_control};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    const RUNTIME: &str = r#"{"tunnelToken":"tok-1","ingressSecret":"s3cret","publicUrls":[
        {"id":"u1","slug":"journal-a8f3","url":"https://journal-a8f3.localify.fyi",
         "localDomain":"journal.localify","wildcard":false,"machine":{"id":"m1","name":"laptop"},
         "protection":{"mode":"password"},"createdAt":"2026-11-05T18:00:00Z"},
        {"id":"u2","slug":"other-0000","url":"https://other-0000.localify.fyi",
         "localDomain":"other.localify","wildcard":false,"machine":{"id":"m2","name":"runner"},
         "protection":{"mode":"password"},"createdAt":"2026-11-05T18:00:00Z"}]}"#;

    /// Answers every request with `status` and `body`.
    async fn fake_api(status: &'static str, body: &'static str) -> String {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut buffer = vec![0; 4096];
                let _ = stream.read(&mut buffer).await;
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });
        base
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

    struct Fixture {
        tunnel: PublicTunnel,
        access: IngressAccess,
        desired: watch::Sender<Option<TunnelToken>>,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                tunnel: PublicTunnel::new(1),
                access: IngressAccess::default(),
                desired: watch::channel(None).0,
            }
        }

        async fn check(&self, store: &CredentialsStore) -> bool {
            check_runtime(&self.tunnel, store, &self.access, &self.desired).await
        }

        fn wanted(&self) -> Option<TunnelToken> {
            self.desired.borrow().clone()
        }

        async fn status(&self) -> PublicStatus {
            self.tunnel.status().await
        }
    }

    #[tokio::test]
    async fn a_machine_that_is_not_signed_in_serves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = Fixture::new();

        assert!(
            fixture
                .check(&CredentialsStore::in_data_dir(dir.path()))
                .await
        );

        assert!(!fixture.access.accepts_secret("s3cret"));
        assert_eq!(fixture.wanted(), None);
        let status = fixture.status().await;
        assert!(!status.signed_in);
        assert_eq!(status.tunnel, TunnelState::Stopped);
    }

    #[tokio::test]
    async fn a_served_url_grants_the_ingress_and_wants_the_tunnel() {
        let dir = tempfile::tempdir().unwrap();
        let store = sign_in(dir.path(), &fake_api("200 OK", RUNTIME).await);
        let fixture = Fixture::new();

        assert!(fixture.check(&store).await);

        assert!(fixture.access.accepts_secret("s3cret"));
        assert!(!fixture.access.accepts_secret("s3cre"));
        assert_eq!(fixture.wanted(), Some(TunnelToken("tok-1".into())));
        let status = fixture.status().await;
        assert!(status.signed_in);
        // u2 belongs to another machine and is left out.
        assert_eq!(status.served_urls, vec!["u1".to_string()]);
    }

    #[tokio::test]
    async fn no_tunnel_runs_when_the_ingress_could_not_listen() {
        let dir = tempfile::tempdir().unwrap();
        let store = sign_in(dir.path(), &fake_api("200 OK", RUNTIME).await);
        let fixture = Fixture::new();
        fixture.tunnel.ingress_failed("port taken".into());

        assert!(fixture.check(&store).await);

        assert!(!fixture.access.accepts_secret("s3cret"));
        assert_eq!(fixture.wanted(), None);
        let status = fixture.status().await;
        assert_eq!(status.tunnel, TunnelState::Error);
        assert_eq!(status.error.as_deref(), Some("port taken"));
    }

    #[tokio::test]
    async fn no_url_served_here_stops_the_tunnel_and_drops_the_secret() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = Fixture::new();
        fixture
            .check(&sign_in(dir.path(), &fake_api("200 OK", RUNTIME).await))
            .await;

        let empty = r#"{"tunnelToken":"tok-1","ingressSecret":"s3cret","publicUrls":[]}"#;
        assert!(
            fixture
                .check(&sign_in(dir.path(), &fake_api("200 OK", empty).await))
                .await
        );

        assert!(!fixture.access.accepts_secret("s3cret"));
        assert_eq!(fixture.wanted(), None);
        assert_eq!(fixture.status().await.tunnel, TunnelState::Stopped);
    }

    #[tokio::test]
    async fn a_revoked_sign_in_stops_everything_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = Fixture::new();
        fixture
            .check(&sign_in(dir.path(), &fake_api("200 OK", RUNTIME).await))
            .await;

        let revoked = fake_api(
            "401 Unauthorized",
            r#"{"error":"unauthorized","message":"Sign in again."}"#,
        )
        .await;
        assert!(fixture.check(&sign_in(dir.path(), &revoked)).await);

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
        let dir = tempfile::tempdir().unwrap();
        let fixture = Fixture::new();
        let store = sign_in(dir.path(), &fake_api("200 OK", RUNTIME).await);
        fixture.check(&store).await;

        store.delete().unwrap();
        fixture.check(&store).await;

        assert!(!fixture.access.accepts_secret("s3cret"));
        assert_eq!(fixture.wanted(), None);
        assert!(!fixture.status().await.signed_in);
    }

    #[tokio::test]
    async fn an_unreachable_cloud_keeps_what_is_already_served() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = Fixture::new();
        fixture
            .check(&sign_in(dir.path(), &fake_api("200 OK", RUNTIME).await))
            .await;

        let failing = fake_api("502 Bad Gateway", "<html>").await;
        assert!(!fixture.check(&sign_in(dir.path(), &failing)).await);

        assert!(fixture.access.accepts_secret("s3cret"));
        assert_eq!(fixture.wanted(), Some(TunnelToken("tok-1".into())));
    }

    #[tokio::test]
    async fn a_first_fetch_that_fails_is_an_error_for_a_person() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = Fixture::new();

        let failing = fake_api("502 Bad Gateway", "<html>").await;
        assert!(!fixture.check(&sign_in(dir.path(), &failing)).await);

        let status = fixture.status().await;
        assert!(status.signed_in);
        assert_eq!(status.tunnel, TunnelState::Error);
        assert!(status.error.unwrap().contains("502"));
    }

    #[tokio::test]
    async fn credentials_for_a_plain_http_remote_api_are_not_used() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = Fixture::new();

        fixture
            .check(&sign_in(dir.path(), "http://api.example"))
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
            process,
        };
        let status = describe(parts(Process::Running), Some(4));
        assert_eq!(
            (status.tunnel, status.connections),
            (TunnelState::Connected, 4)
        );
        assert_eq!(status.error, None);

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
    }

    #[test]
    fn the_token_never_shows_in_debug_output() {
        assert!(!format!("{:?}", TunnelToken("tok-1".into())).contains("tok-1"));
    }

    /// Records its arguments, whether it got the token, and its pid, then
    /// waits like `cloudflared` would.
    const FAKE_CLOUDFLARED: &str = r#"#!/bin/sh
dir="$(dirname "$0")"
printf '%s\n' "$@" > "$dir/args"
[ "$TUNNEL_TOKEN" = "tok-1" ] && echo yes > "$dir/token"
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

        desired.send_replace(Some(TunnelToken("tok-1".into())));
        wait_until(|| bin.join("pid").exists()).await;

        let args = std::fs::read_to_string(bin.join("args")).unwrap();
        assert_eq!(
            args.lines().collect::<Vec<_>>(),
            [
                "tunnel",
                "--no-autoupdate",
                "--metrics",
                "127.0.0.1:7391",
                "run"
            ]
        );
        assert!(!args.contains("tok-1"));
        assert_eq!(std::fs::read_to_string(bin.join("token")).unwrap(), "yes\n");
        wait_until(|| std::fs::read_to_string(&log).is_ok_and(|text| text == "fresh output\n"))
            .await;
        assert_eq!(tunnel.parts().process, Process::Running);
        let pid: u32 = std::fs::read_to_string(bin.join("pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();

        desired.send_replace(None);
        wait_until(|| !get_process_control().process_exists(pid)).await;
        wait_until(|| tunnel.parts().process == Process::Stopped).await;

        cancel.cancel();
        task.await.unwrap();
    }

    #[tokio::test]
    async fn shutdown_kills_cloudflared() {
        let dir = tempfile::tempdir().unwrap();
        let bin = install_fake(dir.path(), FAKE_CLOUDFLARED);
        let tunnel = PublicTunnel::new(7381);
        let (desired, wanted) = watch::channel(Some(TunnelToken("tok-1".into())));
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

        assert!(!get_process_control().process_exists(pid));
        assert_eq!(tunnel.parts().process, Process::Stopped);
        drop(desired);
    }

    #[tokio::test]
    async fn a_cloudflared_that_exits_is_an_error_pointing_at_its_log() {
        let dir = tempfile::tempdir().unwrap();
        install_fake(dir.path(), "#!/bin/sh\necho 'failed to connect'\nexit 3\n");
        let log = dir.path().join(LOG_FILE_NAME);
        let tunnel = PublicTunnel::new(7381);
        let (_desired, wanted) = watch::channel(Some(TunnelToken("tok-1".into())));
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
}
