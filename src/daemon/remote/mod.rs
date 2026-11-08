//! The relay link (`docs/remote-machines.md`, "Relay", "Daemon"; ADRs 0028
//! and 0029): one outbound WebSocket to the account's relay while this
//! machine is signed in, holds a key, and its account has `remoteAccess`.
//! Every session over it, pairing included, is a Noise session between
//! paired machines; the relay only routes frames.
//!
//! The connection loop here decides when to connect and how long to wait;
//! one actor per connection ([`actor`]) owns the socket's sessions, so no
//! session survives a reconnect and nothing queued is ever replayed.

mod actor;
mod backoff;
mod lease;
pub mod machines;
mod pacer;
mod socket;

#[cfg(test)]
mod fake_relay;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use serde::Serialize;
use tokio::sync::{Notify, broadcast, mpsc, oneshot};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use self::actor::{Command, Failure};
pub use self::actor::{Event, PairEvent};
use self::backoff::{Backoff, Ending, Next, Stop, up_to};
use self::lease::LeaseKeeper;
use self::machines::{Facts, MachineState, MachineView, RelayState, SessionState};
use crate::daemon::public_tunnel::Clocks;
use crate::infrastructure::cloud::api::{CloudClient, RemoteAccount};
use crate::infrastructure::cloud::credentials::{CloudCredentials, CredentialsStore};
use crate::infrastructure::cloud::http::{CloudError, CloudHttp};
use crate::infrastructure::cloud::passwords::PasswordStore;
use crate::infrastructure::cloud::types::RosterMachine;
use crate::infrastructure::remote::noise::{StaticKeys, decode_key, fingerprint};
use crate::infrastructure::remote::store::{DEFAULT_SETTINGS, RemoteStore, SettingsChange};
use crate::infrastructure::remote::wire::{Hello, Presence, TargetSettings};

const KEY_MISSING: &str =
    "This machine's remote key is missing. Sign out and sign in again, then pair it again.";
const SIGNED_OUT: &str =
    "Localify Cloud no longer accepts this machine's sign-in. Sign in again with `localify login`.";
const NEEDS_PRO: &str = "Remote access needs Localify Cloud Pro.";
const BANNED: &str = "Localify Cloud suspended this account, so remote access is off. To appeal, write to abuse@localify.dev.";
const OUTDATED: &str =
    "This version of Localify can no longer reach other machines. Update Localify.";
const REPLACED: &str = "Another Localify on this machine took over its relay connection.";
const KEY_TAKEN: &str = "Another machine of Localify Cloud already holds this machine's remote key. Sign out and sign in again, then pair it again.";
const TOO_MANY_LINKED: &str = "This account already links 10 machines for remote access. Remove one with `localify machines remove <machine>`, then run `localify machines` again.";
const NO_REMOTE: &str = "Localify Cloud does not offer remote access yet.";
/// How long `localify machines` waits for probed machines to say hello.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
/// A signed-out machine looks again this often, in case a sign-in did not
/// say so.
const SIGNED_OUT_RECHECK: Duration = Duration::from_secs(5 * 60);

/// Every delay of the link. Tests shorten them; the defaults are the
/// contract's.
#[derive(Debug, Clone, Copy)]
pub struct Timings {
    pub handshake_expiry: Duration,
    pub keep_every: Duration,
    pub not_responding_after: Duration,
    pub viewer_gives_up_after: Duration,
    pub target_gives_up_after: Duration,
    pub pairing_timeout: Duration,
    pub idle_after: Duration,
    pub lifetime: Duration,
    pub tick: Duration,
    pub ping_every: Duration,
    pub relay_silence: Duration,
    pub backoff_unit: Duration,
    pub backoff_cap: Duration,
    pub deploy_jitter: Duration,
    pub wake_jitter: Duration,
    pub stable_after: Duration,
    pub lease_renew: Duration,
    pub lease_renew_jitter: Duration,
    /// The first wait after a renewal fails, doubling up to 16 times.
    pub lease_retry: Duration,
    /// After `4402`, the first lease request waits this long, plus up to
    /// `lapsed_spread`.
    pub lapsed_first: Duration,
    pub lapsed_spread: Duration,
    /// While leases answer `402`, the link asks again this often.
    pub lapsed_recheck: Duration,
    pub clock_check: Duration,
    pub retry_after_drop: Duration,
}

impl Default for Timings {
    fn default() -> Self {
        let seconds = Duration::from_secs;
        Self {
            handshake_expiry: seconds(30),
            keep_every: seconds(30),
            not_responding_after: seconds(15),
            viewer_gives_up_after: seconds(60),
            target_gives_up_after: seconds(90),
            pairing_timeout: seconds(120),
            idle_after: seconds(120),
            lifetime: seconds(3600),
            tick: seconds(1),
            ping_every: seconds(30),
            relay_silence: seconds(75),
            backoff_unit: seconds(1),
            backoff_cap: seconds(300),
            deploy_jitter: seconds(10),
            wake_jitter: seconds(2),
            stable_after: seconds(60),
            lease_renew: seconds(45),
            lease_renew_jitter: seconds(5),
            lease_retry: seconds(1),
            lapsed_first: seconds(30),
            lapsed_spread: seconds(30),
            lapsed_recheck: seconds(15 * 60),
            clock_check: seconds(20),
            retry_after_drop: seconds(5),
        }
    }
}

/// Why the link is off, for a person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Blocked {
    pub code: String,
    pub message: String,
}

/// The answer to `remote_status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteStatus {
    pub relay: RelayState,
    pub blocked: Option<Blocked>,
    /// RFC 3339: when the next attempt starts, while reconnecting.
    pub retry_at: Option<String>,
    pub settings: TargetSettings,
    pub fingerprint: Option<String>,
}

/// What the link knows, for the management socket.
pub(super) struct View {
    relay: RelayState,
    blocked: Option<Blocked>,
    retry_at: Option<String>,
    presence: HashMap<String, Presence>,
    roster: Vec<RosterMachine>,
    /// This machine's sessions as a viewer, by peer.
    sessions: HashMap<String, SessionState>,
    settings: HashMap<String, TargetSettings>,
    failures: HashMap<String, String>,
}

impl Default for View {
    fn default() -> Self {
        Self {
            relay: RelayState::Off,
            blocked: None,
            retry_at: None,
            presence: HashMap::new(),
            roster: Vec::new(),
            sessions: HashMap::new(),
            settings: HashMap::new(),
            failures: HashMap::new(),
        }
    }
}

impl View {
    fn name_of(&self, machine_id: &str) -> String {
        self.roster
            .iter()
            .find(|machine| machine.id == machine_id)
            .map_or_else(|| machine_id.to_string(), |machine| machine.name.clone())
    }
}

/// The current connection, while there is one.
#[derive(Clone)]
struct Conn {
    commands: mpsc::UnboundedSender<Command>,
    lease: Arc<LeaseKeeper>,
    cloud: Arc<CloudClient>,
    self_id: String,
}

struct Shared {
    store: RemoteStore,
    credentials: CredentialsStore,
    timings: Timings,
    view: Mutex<View>,
    conn: Mutex<Option<Conn>>,
    events: broadcast::Sender<Event>,
    /// Clients of `remote_events`: a person who can confirm a pairing here.
    subscribers: AtomicUsize,
    /// The pairing attempts of the last hour on this machine as a target.
    /// The link keeps them, so a new connection does not reset the limit.
    pair_attempts: Mutex<VecDeque<Instant>>,
    refresh: Notify,
    #[cfg(test)]
    wake: std::sync::atomic::AtomicBool,
}

impl Shared {
    fn view(&self) -> MutexGuard<'_, View> {
        self.view
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn pair_attempts(&self) -> MutexGuard<'_, VecDeque<Instant>> {
        self.pair_attempts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn conn(&self) -> Option<Conn> {
        self.conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn set_conn(&self, conn: Option<Conn>) {
        *self
            .conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = conn;
    }

    fn emit(&self, event: Event) {
        // Nobody listening is fine.
        let _ = self.events.send(event);
    }

    fn set_relay(&self, relay: RelayState, blocked: Option<Blocked>, retry_at: Option<String>) {
        let mut view = self.view();
        view.relay = relay;
        view.blocked = blocked;
        view.retry_at = retry_at;
        if relay != RelayState::Connected {
            view.presence.clear();
            view.sessions.clear();
        }
    }

    /// Whether the machine woke from sleep since `clocks` were read.
    fn woke(&self, clocks: &Clocks) -> bool {
        #[cfg(test)]
        if self.wake.swap(false, Ordering::SeqCst) {
            return true;
        }
        clocks.slept()
    }
}

/// The handle the daemon's management socket holds.
#[derive(Clone)]
pub struct RemoteLink {
    shared: Arc<Shared>,
}

/// A `remote_events` client. While one exists, a person can confirm a
/// pairing on this machine.
pub struct Subscription {
    pub events: broadcast::Receiver<Event>,
    shared: Arc<Shared>,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.shared.subscribers.fetch_sub(1, Ordering::SeqCst);
    }
}

impl RemoteLink {
    pub fn new(data_dir: &Path) -> Self {
        Self::with_timings(data_dir, Timings::default())
    }

    pub fn with_timings(data_dir: &Path, timings: Timings) -> Self {
        let (events, _) = broadcast::channel(64);
        Self {
            shared: Arc::new(Shared {
                store: RemoteStore::in_data_dir(data_dir),
                credentials: CredentialsStore::in_data_dir(data_dir),
                timings,
                view: Mutex::new(View::default()),
                conn: Mutex::new(None),
                events,
                subscribers: AtomicUsize::new(0),
                pair_attempts: Mutex::new(VecDeque::new()),
                refresh: Notify::new(),
                #[cfg(test)]
                wake: std::sync::atomic::AtomicBool::new(false),
            }),
        }
    }

    /// Looks at the sign-in and the plan again, after `public_refresh`.
    pub fn refresh(&self) {
        self.shared.refresh.notify_one();
    }

    pub fn status(&self) -> RemoteStatus {
        let view = self.shared.view();
        let fingerprint = self
            .shared
            .store
            .load_identity()
            .ok()
            .flatten()
            .and_then(|identity| decode_key(&identity.public_key).ok())
            .map(|key| fingerprint(&key));
        RemoteStatus {
            relay: view.relay,
            blocked: view.blocked.clone(),
            retry_at: view.retry_at.clone(),
            settings: self.shared.store.settings().unwrap_or(DEFAULT_SETTINGS),
            fingerprint,
        }
    }

    pub fn change_settings(&self, change: SettingsChange) -> Result<TargetSettings> {
        self.shared.store.update_settings(change)
    }

    /// The roster with each machine's state. With `probe`, first opens a
    /// short session to each paired machine online, so the answer says
    /// whether it shares and still answers.
    pub async fn machines(&self, probe: bool) -> Result<Vec<MachineView>> {
        let conn = self.shared.conn();
        if let Some(conn) = &conn {
            self.refresh_roster(conn).await;
        }
        let mut views = self.list()?;
        if probe && conn.is_some() {
            let probes = views
                .iter()
                .filter(|view| {
                    !view.is_self
                        && matches!(
                            view.state,
                            MachineState::Connected | MachineState::NotSharing
                        )
                })
                .map(|view| self.open(&view.id, false));
            let _ =
                tokio::time::timeout(PROBE_TIMEOUT, futures_util::future::join_all(probes)).await;
            views = self.list()?;
        }
        Ok(views)
    }

    fn list(&self) -> Result<Vec<MachineView>> {
        let pins = self.shared.store.pins()?;
        let view = self.shared.view();
        Ok(machines::list(&Facts {
            relay: view.relay,
            roster: &view.roster,
            pins: &pins,
            presence: &view.presence,
            sessions: &view.sessions,
            settings: &view.settings,
            failures: &view.failures,
        }))
    }

    async fn refresh_roster(&self, conn: &Conn) {
        match conn.cloud.machines().await {
            Ok(roster) => self.shared.view().roster = roster,
            Err(error) => warn!(error = %error, "Could not read the account's machines"),
        }
    }

    fn connected(&self) -> Result<Conn> {
        let relay = self.shared.view().relay;
        match (self.shared.conn(), relay) {
            (Some(conn), RelayState::Connected) => Ok(conn),
            _ => {
                let status = self.status();
                let reason = status.blocked.map_or_else(
                    || "it is not connected to Localify Cloud right now".to_string(),
                    |blocked| blocked.message,
                );
                bail!("Remote access is not available: {reason}")
            }
        }
    }

    /// Opens a session to a paired machine and waits for its `hello`. With
    /// `hold`, the session stays open; otherwise it ends right after.
    async fn open(&self, peer: &str, hold: bool) -> Result<Hello, Failure> {
        let conn = self
            .connected()
            .map_err(|error| Failure::new("unavailable", error.to_string()))?;
        conn.lease.ensure().await.map_err(|_| {
            Failure::new("unauthorized", "Remote access is not available right now.")
        })?;
        let (reply, answer) = oneshot::channel();
        let command = Command::Open {
            peer: peer.to_string(),
            hold,
            reply,
        };
        conn.commands
            .send(command)
            .map_err(|_| Failure::new("unavailable", "The relay connection closed."))?;
        let wait = self.shared.timings.handshake_expiry + Duration::from_secs(5);
        match tokio::time::timeout(wait, answer).await {
            Ok(Ok(result)) => result,
            _ => Err(Failure::new("timeout", "The machine did not answer.")),
        }
    }

    /// Starts pairing with `peer`: the events say the code, then whether
    /// it paired.
    pub async fn pair(&self, peer: &str) -> Result<mpsc::Receiver<PairEvent>> {
        let conn = self.connected()?;
        if peer == conn.self_id {
            bail!("This is this machine: pair it from another one.");
        }
        self.refresh_roster(&conn).await;
        let name = self.shared.view().name_of(peer);
        let online = self.shared.view().presence.contains_key(peer);
        if !online {
            bail!("{name} is offline. Open Localify on it, then try again.");
        }
        conn.lease.ensure().await.map_err(|_| {
            anyhow!("Remote access is not available right now. Try again in a moment.")
        })?;
        let (events, receiver) = mpsc::channel(16);
        conn.commands
            .send(Command::Pair {
                peer: peer.to_string(),
                events,
            })
            .map_err(|_| anyhow!("The relay connection closed. Try again."))?;
        Ok(receiver)
    }

    /// A person's answer to the code of a pairing, on either side.
    pub async fn answer(&self, pairing_id: &str, accept: bool) -> Result<()> {
        let conn = self.connected()?;
        let (reply, answered) = oneshot::channel();
        conn.commands
            .send(Command::Answer {
                pairing_id: pairing_id.to_string(),
                accept,
                reply,
            })
            .map_err(|_| anyhow!("The relay connection closed."))?;
        match answered.await {
            Ok(true) => Ok(()),
            _ => bail!(
                "No pairing {pairing_id} is waiting for an answer: it was cancelled or timed out."
            ),
        }
    }

    /// Deletes the pin, and tells the peer when a session is open.
    pub fn unpair(&self, peer: &str) -> Result<bool> {
        let mut removed = false;
        self.shared.store.update_pins(|pins| {
            removed = pins.remove(peer).is_some();
        })?;
        self.shared.view().failures.remove(peer);
        self.shared.view().settings.remove(peer);
        if let Some(conn) = self.shared.conn() {
            let _ = conn.commands.send(Command::Unpair {
                peer: peer.to_string(),
            });
        }
        Ok(removed)
    }

    pub fn subscribe(&self) -> Subscription {
        self.shared.subscribers.fetch_add(1, Ordering::SeqCst);
        Subscription {
            events: self.shared.events.subscribe(),
            shared: Arc::clone(&self.shared),
        }
    }
}

/// Starts the connection loop; it stops with `cancel`.
pub fn spawn(link: RemoteLink, cancel: CancellationToken) {
    tokio::spawn(run(link.shared, cancel));
}

/// What the link waits for before it tries again.
enum Wait {
    Retry(Duration),
    /// `public_refresh`, or at most this long.
    Refresh(Option<Duration>),
    /// Remote access lapsed: after this long, ask for a lease, and connect
    /// again once one is granted; until then ask every `lapsed_recheck`.
    /// `public_refresh` (a plan change) cuts any wait short.
    Lapsed(Duration),
}

struct Ready {
    credentials: CloudCredentials,
    account: RemoteAccount,
    keys: StaticKeys,
    http: CloudHttp,
    cloud: Arc<CloudClient>,
}

async fn run(shared: Arc<Shared>, cancel: CancellationToken) {
    let mut backoff = Backoff::default();
    let mut uploaded_for: Option<String> = None;
    let mut ever_connected = false;
    loop {
        let wait = match prepare(&shared, &mut backoff, &mut uploaded_for).await {
            Ok(ready) => {
                let relay = if ever_connected {
                    RelayState::Reconnecting
                } else {
                    RelayState::Connecting
                };
                shared.set_relay(relay, None, None);
                let token = ready.credentials.token.clone();
                let (ending, up_for) = connect_and_serve(&shared, ready, &cancel).await;
                ever_connected |= up_for > Duration::ZERO;
                if cancel.is_cancelled() {
                    return;
                }
                let next = backoff.next(&ending, up_for, &shared.timings);
                info!(?ending, up_for = ?up_for, "The relay connection ended");
                plan(&shared, &mut uploaded_for, next, &token)
            }
            Err(wait) => wait,
        };
        if !wait_before_next(&shared, wait, &cancel).await {
            return;
        }
    }
}

/// Checks the sign-in, the plan and the key, and uploads the key once.
async fn prepare(
    shared: &Shared,
    backoff: &mut Backoff,
    uploaded_for: &mut Option<String>,
) -> Result<Ready, Wait> {
    let credentials = match shared.credentials.load() {
        Ok(Some(credentials)) => credentials,
        Ok(None) | Err(_) => {
            shared.set_relay(RelayState::Off, None, None);
            return Err(Wait::Refresh(Some(SIGNED_OUT_RECHECK)));
        }
    };
    let http = CloudHttp::new(
        credentials.api_base.clone(),
        Some(credentials.token.clone()),
    )
    .map_err(|error| block(shared, "unreachable", &error.to_string(), None))?;
    let cloud = Arc::new(CloudClient::new(http.clone()));
    let account = match cloud.remote_account().await {
        Ok(Some(account)) => account,
        Ok(None) => {
            return Err(block(
                shared,
                "unsupported",
                NO_REMOTE,
                Some(shared.timings.lapsed_recheck),
            ));
        }
        Err(error) => return Err(cloud_refusal(shared, backoff, &error, &credentials.token)),
    };
    if !account.remote_access {
        return Err(lapsed(shared, shared.timings.lapsed_recheck));
    }
    let identity = shared
        .store
        .identity_for(&credentials.machine_id)
        .map_err(|error| block(shared, "remote_key_unreadable", &format!("{error:#}"), None))?;
    let keys = identity
        .keys()
        .map_err(|error| block(shared, "remote_key_unreadable", &format!("{error:#}"), None))?;
    if uploaded_for.as_deref() != Some(credentials.machine_id.as_str()) {
        match cloud.put_remote_key(&identity.public_key).await {
            Ok(()) => *uploaded_for = Some(credentials.machine_id.clone()),
            Err(CloudError::Api {
                status: 409, code, ..
            }) if code == "remote_key_set" => {
                return Err(block(shared, "remote_key_missing", KEY_MISSING, None));
            }
            Err(CloudError::Api {
                status: 409, code, ..
            }) if code == "remote_key_taken" => {
                return Err(block(shared, "remote_key_taken", KEY_TAKEN, None));
            }
            Err(CloudError::Api {
                status: 403, code, ..
            }) if code == "quota_exceeded" => {
                return Err(block(
                    shared,
                    "quota_exceeded",
                    TOO_MANY_LINKED,
                    Some(shared.timings.lapsed_recheck),
                ));
            }
            Err(CloudError::Api {
                status: 409 | 422,
                code,
                message,
                ..
            }) => {
                return Err(block(shared, &code, &message, None));
            }
            Err(error) => return Err(cloud_refusal(shared, backoff, &error, &credentials.token)),
        }
    }
    Ok(Ready {
        credentials,
        account,
        keys,
        http,
        cloud,
    })
}

fn block(shared: &Shared, code: &str, message: &str, recheck: Option<Duration>) -> Wait {
    info!(code, "Remote access is off");
    let blocked = Blocked {
        code: code.to_string(),
        message: message.to_string(),
    };
    shared.set_relay(RelayState::Blocked, Some(blocked), None);
    Wait::Refresh(recheck)
}

/// Remote access lapsed: blocked, until a lease is granted again.
fn lapsed(shared: &Shared, first: Duration) -> Wait {
    match block(shared, "subscription_required", NEEDS_PRO, None) {
        Wait::Refresh(_) => Wait::Lapsed(first),
        other => other,
    }
}

/// Localify Cloud no longer accepts the token: this machine signs out here,
/// as `logout` does, unless `cloud.json` already holds another sign-in.
fn signed_out_by_cloud(shared: &Shared, token: &str) -> Wait {
    match shared.credentials.load() {
        Ok(Some(credentials)) if credentials.token == token => {
            info!("Localify Cloud revoked this machine's token; signing out here");
            if let Err(error) = shared.credentials.delete() {
                warn!(error = %format!("{error:#}"), "Could not delete this machine's sign-in");
            }
            let data_dir = shared.credentials.data_dir();
            if let Err(error) = PasswordStore::in_data_dir(&data_dir).update(|saved| saved.clear())
            {
                warn!(error = %format!("{error:#}"), "Could not delete the saved passwords");
            }
        }
        // A new sign-in, or none: nothing of this one is left.
        _ => return Wait::Retry(Duration::ZERO),
    }
    block(shared, "signed_out", SIGNED_OUT, None)
}

fn cloud_refusal(shared: &Shared, backoff: &mut Backoff, error: &CloudError, token: &str) -> Wait {
    match error {
        CloudError::Api { status: 401, .. } => signed_out_by_cloud(shared, token),
        CloudError::Api {
            status: 403, code, ..
        } if code == "banned" => block(shared, "banned", BANNED, None),
        CloudError::Api { status: 402, .. } => lapsed(shared, shared.timings.lapsed_recheck),
        _ => {
            let next = backoff.next(&Ending::Unreachable, Duration::ZERO, &shared.timings);
            match next {
                Next::Retry(wait) | Next::UploadKey(wait) => Wait::Retry(wait),
                Next::Stop(_) => Wait::Retry(shared.timings.backoff_cap),
            }
        }
    }
}

/// One connection, from the upgrade to its end. Returns how it ended and
/// how long it was up.
async fn connect_and_serve(
    shared: &Arc<Shared>,
    ready: Ready,
    cancel: &CancellationToken,
) -> (Ending, Duration) {
    let socket = tokio::select! {
        _ = cancel.cancelled() => return (Ending::Lost, Duration::ZERO),
        socket = socket::connect(&ready.credentials.api_base, &ready.credentials.token) => socket,
    };
    let socket = match socket {
        Ok(socket) => socket,
        Err(ending) => return (ending, Duration::ZERO),
    };
    let started = Instant::now();
    let (commands, receiver) = mpsc::unbounded_channel();
    let lease = Arc::new(LeaseKeeper::new(
        CloudClient::new(ready.http.clone()),
        shared.timings,
    ));
    let conn = Conn {
        commands: commands.clone(),
        lease: Arc::clone(&lease),
        cloud: Arc::clone(&ready.cloud),
        self_id: ready.credentials.machine_id.clone(),
    };
    shared.set_conn(Some(conn.clone()));
    // The names pins are saved with.
    if let Ok(roster) = ready.cloud.machines().await {
        shared.view().roster = roster;
    }
    let context = actor::Context {
        self_id: ready.credentials.machine_id,
        token: ready.credentials.token,
        account: ready.account.user_id,
        keys: ready.keys,
        lease,
        cloud: Arc::clone(&ready.cloud),
    };
    let ending = actor::run(
        Arc::clone(shared),
        context,
        socket,
        receiver,
        commands,
        cancel,
    )
    .await;
    shared.set_conn(None);
    (ending, started.elapsed())
}

fn plan(shared: &Shared, uploaded_for: &mut Option<String>, next: Next, token: &str) -> Wait {
    match next {
        Next::Retry(wait) => Wait::Retry(wait),
        Next::UploadKey(wait) => {
            *uploaded_for = None;
            Wait::Retry(wait)
        }
        Next::Stop(Stop::SignedOut) => signed_out_by_cloud(shared, token),
        // `random(30, 60)` s, so an account's machines do not all ask at once.
        Next::Stop(Stop::Lapsed) => lapsed(
            shared,
            shared.timings.lapsed_first + up_to(shared.timings.lapsed_spread),
        ),
        Next::Stop(Stop::Banned) => block(shared, "banned", BANNED, None),
        Next::Stop(Stop::Outdated) => block(shared, "unsupported_protocol", OUTDATED, None),
        Next::Stop(Stop::Replaced) => block(shared, "replaced", REPLACED, None),
    }
}

/// Waits as `wait` says. A wake from sleep cuts a retry short, to one
/// attempt within `wake_jitter`. `false` on shutdown.
async fn wait_before_next(shared: &Shared, wait: Wait, cancel: &CancellationToken) -> bool {
    match wait {
        Wait::Retry(delay) => {
            let retry_at = time::OffsetDateTime::now_utc() + delay;
            let retry_at = retry_at.replace_nanosecond(0).ok().and_then(|at| {
                at.format(&time::format_description::well_known::Rfc3339)
                    .ok()
            });
            let relay = shared.view().relay;
            let relay = match relay {
                RelayState::Connecting | RelayState::Blocked | RelayState::Off => {
                    RelayState::Reconnecting
                }
                other => other,
            };
            shared.set_relay(relay, None, retry_at);
            let mut deadline = Instant::now() + delay;
            loop {
                let clocks = Clocks::now();
                tokio::select! {
                    _ = cancel.cancelled() => return false,
                    _ = shared.refresh.notified() => return true,
                    _ = tokio::time::sleep_until(deadline) => return true,
                    _ = tokio::time::sleep(shared.timings.clock_check) => {
                        if shared.woke(&clocks) {
                            info!("This machine woke from sleep; reconnecting to the relay");
                            deadline = deadline.min(Instant::now() + up_to(shared.timings.wake_jitter));
                        }
                    }
                }
            }
        }
        Wait::Refresh(limit) => {
            let limit = limit.unwrap_or(Duration::from_secs(365 * 24 * 3600));
            tokio::select! {
                _ = cancel.cancelled() => false,
                _ = shared.refresh.notified() => true,
                _ = tokio::time::sleep(limit) => true,
            }
        }
        Wait::Lapsed(first) => {
            let mut delay = first;
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => return false,
                    _ = shared.refresh.notified() => return true,
                    _ = tokio::time::sleep(delay) => {}
                }
                if lease_granted(shared).await {
                    return true;
                }
                delay = shared.timings.lapsed_recheck;
            }
        }
    }
}

/// Whether Localify Cloud grants a lease again. Without credentials there is
/// nothing to wait for: the next pass says the machine is signed out.
async fn lease_granted(shared: &Shared) -> bool {
    let Ok(Some(credentials)) = shared.credentials.load() else {
        return true;
    };
    let Ok(http) = CloudHttp::new(credentials.api_base, Some(credentials.token)) else {
        return true;
    };
    match CloudClient::new(http).remote_lease().await {
        Ok(_) => true,
        Err(error) => {
            info!(error = %error, "Remote access is still lapsed");
            false
        }
    }
}
