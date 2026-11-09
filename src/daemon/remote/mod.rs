//! The relay link (`docs/remote-machines.md`, "Relay", "Daemon"; ADRs 0028
//! and 0029): one outbound WebSocket to the account's relay while this
//! machine is signed in, holds a key, and its account has `remoteAccess`.
//! Every session over it, pairing included, is a Noise session between
//! paired machines; the relay only routes frames.
//!
//! The connection loop here decides when to connect and how long to wait;
//! one actor per connection ([`actor`]) owns the socket's sessions, so no
//! session survives a reconnect and nothing queued is ever replayed.

mod access;
mod actor;
mod backoff;
mod lease;
pub mod local;
pub mod machines;
mod pacer;
mod socket;
mod viewing;

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

pub use self::actor::Failure;
use self::actor::{Command, TapEvent, TapRequest};
pub use self::actor::{Event, PairEvent};
use self::backoff::{Backoff, Ending, Next, Stop, random_fraction, up_to};
use self::lease::{LeaseError, LeaseKeeper};
use self::local::LocalSource;
use self::machines::{Facts, MachineState, MachineView, RelayState, SessionState};
pub use self::viewing::{
    AccessEvent, Asking, ControlAsk, ControlEvent, FollowEvent, Following, RemoteInventory,
};
use crate::daemon::public_tunnel::Clocks;
use crate::infrastructure::cloud::api::{CloudClient, RemoteAccount};
use crate::infrastructure::cloud::credentials::{CloudCredentials, CredentialsStore};
use crate::infrastructure::cloud::http::{
    CloudError, CloudHttp, DEFAULT_API_BASE, api_base_override,
};
use crate::infrastructure::cloud::passwords::PasswordStore;
use crate::infrastructure::cloud::types::RosterMachine;
use crate::infrastructure::remote::noise::{StaticKeys, decode_key, fingerprint};
use crate::infrastructure::remote::store::{DEFAULT_SETTINGS, RemoteStore, SettingsChange};
use crate::infrastructure::remote::wire::{
    AccessOutcome, Hello, Presence, ProtoRange, TargetSettings,
};

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
/// A listing reads the roster from Localify Cloud again once it is older
/// than this, so the app can poll the listing without calling the cloud.
/// Presence of an unknown machine and revocations read it at once.
const ROSTER_FRESH: Duration = Duration::from_secs(30);
/// A signed-out machine looks again this often, in case a sign-in did not
/// say so.
const SIGNED_OUT_RECHECK: Duration = Duration::from_secs(5 * 60);

/// The code of the `401` the Worker answers for an unknown or revoked
/// token (`apps/cloud/src/api.ts`, `signedOut`).
const REVOKED_CODE: &str = "unauthorized";
const UNREACHABLE: &str =
    "Can't reach Localify Cloud right now. Remote access comes back on its own.";

/// Whether a `401` revokes this machine's sign-in. Only one from the API
/// base the machine signed in with, when that is `https://api.localify.dev`
/// or the explicit `LOCALIFY_CLOUD_API` override, with the Worker's JSON
/// error and its revocation code. Any other `401` (an HTML page, an empty
/// body, another JSON) is something in between: retried, state kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Revocation {
    trusted: bool,
}

impl Revocation {
    fn for_api_base(shared: &Shared, api_base: &str) -> Self {
        let trusted = api_base == DEFAULT_API_BASE
            || api_base_override().as_deref() == Some(api_base)
            || shared.trusts_for_tests(api_base);
        Self { trusted }
    }

    pub fn revokes(&self, error: &CloudError) -> bool {
        self.trusted
            && matches!(error, CloudError::Api { status: 401, code, .. } if code == REVOKED_CODE)
    }

    /// A refused upgrade: the same rules, on the body `socket` parsed.
    fn revokes_upgrade(&self, ending: &Ending) -> bool {
        self.trusted
            && matches!(ending, Ending::Refused { status: 401, code: Some(code), .. } if code == REVOKED_CODE)
    }
}

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
    /// While a viewer watches, the target looks for changes this often.
    pub watch_every: Duration,
    /// A watch or a follow ends this long after the last `keep` or `ack`
    /// that names it.
    pub watch_expiry: Duration,
    /// While a follow is open, the target looks for new output this often.
    pub follow_poll: Duration,
    /// How often a target reads its switches from the file, for a change
    /// made without the management socket, which tells it at once.
    pub settings_recheck: Duration,
    /// An access request nobody answered ends `expired` after this.
    pub access_expiry: Duration,
    /// A pinned peer asks for access at most once this often.
    pub access_every: Duration,
    /// After a refusal, a peer asks again only after this.
    pub access_after_refusal: Duration,
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
            watch_every: seconds(2),
            watch_expiry: seconds(120),
            follow_poll: Duration::from_millis(250),
            settings_recheck: seconds(1),
            access_expiry: seconds(120),
            access_every: seconds(10 * 60),
            access_after_refusal: seconds(3600),
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
    /// Why background starts run with the daemon's own environment: the
    /// user's shell environment could not be read.
    pub shell_environment_error: Option<String>,
}

/// What the link knows, for the management socket.
pub(super) struct View {
    relay: RelayState,
    blocked: Option<Blocked>,
    retry_at: Option<String>,
    presence: HashMap<String, Presence>,
    roster: Vec<RosterMachine>,
    /// When the roster was last read from Localify Cloud.
    roster_read: Option<Instant>,
    /// This machine's sessions as a viewer, by peer.
    sessions: HashMap<String, SessionState>,
    settings: HashMap<String, TargetSettings>,
    failures: HashMap<String, String>,
    /// The last whole snapshot of each peer, so a request can say which
    /// revision it holds.
    inventories: HashMap<String, RemoteInventory>,
    /// The session version agreed with each peer, from both `hello`s of
    /// this machine's open session as its viewer.
    protos: HashMap<String, u32>,
}

impl Default for View {
    fn default() -> Self {
        Self {
            relay: RelayState::Off,
            blocked: None,
            retry_at: None,
            presence: HashMap::new(),
            roster: Vec::new(),
            roster_read: None,
            sessions: HashMap::new(),
            settings: HashMap::new(),
            failures: HashMap::new(),
            inventories: HashMap::new(),
            protos: HashMap::new(),
        }
    }
}

impl View {
    fn set_roster(&mut self, roster: Vec<RosterMachine>) {
        self.roster = roster;
        self.roster_read = Some(Instant::now());
    }

    /// Whether the roster is older than [`ROSTER_FRESH`], or was never read.
    fn roster_is_stale(&self) -> bool {
        self.roster_read
            .is_none_or(|read| read.elapsed() >= ROSTER_FRESH)
    }

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
    /// What this machine shares as a target.
    source: Arc<dyn LocalSource>,
    timings: Timings,
    view: Mutex<View>,
    conn: Mutex<Option<Conn>>,
    events: broadcast::Sender<Event>,
    /// Clients of `remote_events`: a person who can confirm a pairing here.
    subscribers: AtomicUsize,
    /// Those of them that show the access dialog: a person who can answer
    /// an access request here.
    access_subscribers: AtomicUsize,
    /// The access requests of each peer and the dialogs shown, kept by the
    /// link so a new connection does not reset the limits.
    access_history: Mutex<access::History>,
    /// The revisions inventories go out as, across relay connections.
    sent_revisions: Mutex<local::SentRevisions>,
    /// The pairing attempts of the last hour on this machine as a target.
    /// The link keeps them, so a new connection does not reset the limit.
    pair_attempts: Mutex<VecDeque<Instant>>,
    refresh: Notify,
    #[cfg(test)]
    wake: std::sync::atomic::AtomicBool,
    /// The most answers that ever waited to be sent as a target, in bytes.
    #[cfg(test)]
    pending_peak: AtomicUsize,
    /// The fake cloud's base, trusted for revocation as the override is.
    #[cfg(test)]
    trusted_base: Mutex<Option<String>>,
    /// The session versions this link says it speaks, as an older build would.
    #[cfg(test)]
    proto: Mutex<ProtoRange>,
    /// Holds the next access answer before it takes the settings lock.
    #[cfg(test)]
    answer_pause: Mutex<Option<AnswerPause>>,
}

/// Where a test holds an access answer: `reached` fires there, and the
/// answer goes on once `release` hears from the test, or loses it.
#[cfg(test)]
struct AnswerPause {
    reached: oneshot::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
}

impl Shared {
    fn view(&self) -> MutexGuard<'_, View> {
        self.view
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn access_history(&self) -> MutexGuard<'_, access::History> {
        self.access_history
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The session versions this machine speaks.
    #[cfg(not(test))]
    fn ours(&self) -> ProtoRange {
        ProtoRange::OURS
    }

    #[cfg(test)]
    fn ours(&self) -> ProtoRange {
        *self
            .proto
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Blocks the actor where a test paused the next access answer, as a
    /// writer holding the settings lock would.
    #[cfg(test)]
    fn pause_answer(&self) {
        let pause = self
            .answer_pause
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(pause) = pause {
            let _ = pause.reached.send(());
            let _ = pause.release.recv();
        }
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
            view.protos.clear();
        }
    }

    #[cfg(test)]
    fn trusts_for_tests(&self, api_base: &str) -> bool {
        self.trusted_base
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_deref()
            == Some(api_base)
    }

    #[cfg(not(test))]
    fn trusts_for_tests(&self, _: &str) -> bool {
        false
    }

    /// The machine id of the sign-in `cloud.json` holds now.
    fn signed_in_as(&self) -> Result<String> {
        match self.credentials.load()? {
            Some(credentials) => Ok(credentials.machine_id),
            None => bail!(
                "This machine is not signed in to Localify Cloud. Sign in with: localify login"
            ),
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
/// pairing on this machine; while one that shows the access dialog exists,
/// a person can answer an access request.
pub struct Subscription {
    pub events: broadcast::Receiver<Event>,
    /// Whether it shows the access dialog: it hears access events only then.
    pub access: bool,
    shared: Arc<Shared>,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.shared.subscribers.fetch_sub(1, Ordering::SeqCst);
        if self.access {
            self.shared
                .access_subscribers
                .fetch_sub(1, Ordering::SeqCst);
        }
    }
}

impl RemoteLink {
    pub fn new(data_dir: &Path, source: Arc<dyn LocalSource>) -> Self {
        Self::with_timings(data_dir, source, Timings::default())
    }

    pub fn with_timings(data_dir: &Path, source: Arc<dyn LocalSource>, timings: Timings) -> Self {
        let (events, _) = broadcast::channel(64);
        Self {
            shared: Arc::new(Shared {
                store: RemoteStore::in_data_dir(data_dir),
                credentials: CredentialsStore::in_data_dir(data_dir),
                source,
                timings,
                view: Mutex::new(View::default()),
                conn: Mutex::new(None),
                events,
                subscribers: AtomicUsize::new(0),
                access_subscribers: AtomicUsize::new(0),
                access_history: Mutex::new(access::History::default()),
                sent_revisions: Mutex::new(local::SentRevisions::default()),
                pair_attempts: Mutex::new(VecDeque::new()),
                refresh: Notify::new(),
                #[cfg(test)]
                wake: std::sync::atomic::AtomicBool::new(false),
                #[cfg(test)]
                pending_peak: AtomicUsize::new(0),
                #[cfg(test)]
                trusted_base: Mutex::new(None),
                #[cfg(test)]
                proto: Mutex::new(ProtoRange::OURS),
                #[cfg(test)]
                answer_pause: Mutex::new(None),
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
            shell_environment_error: self.shared.source.shell_environment_failure(),
        }
    }

    /// Reads the shell environment of background starts again, and says
    /// why it still could not be read.
    pub async fn reload_shell_environment(&self) -> Option<String> {
        let source = Arc::clone(&self.shared.source);
        tokio::task::spawn_blocking(move || source.reload_shell_environment())
            .await
            .unwrap_or_else(|error| Some(format!("The shell could not be asked: {error}")))
    }

    /// The switches and the app, as this machine's own inventory decides its
    /// controls with them.
    pub fn switches(&self) -> crate::application::inventory::Switches {
        crate::application::inventory::Switches {
            settings: self
                .shared
                .store
                .settings()
                .unwrap_or(DEFAULT_SETTINGS)
                .into(),
            app: self.shared.source.app().into(),
        }
    }

    /// Saves the switches, then waits until the connection acted on them:
    /// with `share` off, every follow and waiting answer has ended before
    /// this returns, so nothing read afterwards reaches a paired machine.
    pub async fn change_settings(&self, change: SettingsChange) -> Result<TargetSettings> {
        let machine_id = self.shared.signed_in_as()?;
        let settings = self.shared.store.update_settings(&machine_id, change)?;
        if let Some(conn) = self.shared.conn() {
            let (reply, done) = oneshot::channel();
            let command = Command::SettingsChanged { reply };
            // A closed connection serves nothing; the next one reads the file.
            if conn.commands.send(command).is_ok() {
                let _ = done.await;
            }
        }
        Ok(settings)
    }

    /// Speaks only `proto` from the next session on, as an older build.
    #[cfg(test)]
    pub fn speak(&self, proto: ProtoRange) {
        *self
            .shared
            .proto
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = proto;
    }

    /// Holds the next access answer before it takes the settings lock, as
    /// another writer holding it would: the receiver fires once it waits
    /// there, and it goes on when the sender sends or drops.
    #[cfg(test)]
    pub fn pause_next_answer(&self) -> (oneshot::Receiver<()>, std::sync::mpsc::Sender<()>) {
        let (reached, at_pause) = oneshot::channel();
        let (release, released) = std::sync::mpsc::channel();
        *self
            .shared
            .answer_pause
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(AnswerPause {
            reached,
            release: released,
        });
        (at_pause, release)
    }

    /// Lets the fake cloud's `401`s revoke, as `LOCALIFY_CLOUD_API` would.
    #[cfg(test)]
    pub fn trust_api_base(&self, api_base: &str) {
        *self
            .shared
            .trusted_base
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(api_base.to_string());
    }

    /// The roster with each machine's state. With `probe`, first opens a
    /// short session to each paired machine online, so the answer says
    /// whether it shares and still answers. The roster is read again when
    /// probing, or once it is older than [`ROSTER_FRESH`].
    pub async fn machines(&self, probe: bool) -> Result<Vec<MachineView>> {
        let conn = self.shared.conn();
        if let Some(conn) = &conn
            && (probe || self.shared.view().roster_is_stale())
        {
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
            protos: &view.protos,
            settings: &view.settings,
            failures: &view.failures,
        }))
    }

    async fn refresh_roster(&self, conn: &Conn) {
        match conn.cloud.machines().await {
            Ok(roster) => self.shared.view().set_roster(roster),
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
        if let Err(error) = conn.lease.ensure().await {
            return Err(lease_refused(&conn, &error));
        }
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
        if let Err(error) = conn.lease.ensure().await {
            bail!("{}", lease_refused(&conn, &error).message);
        }
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
        let machine_id = self.shared.signed_in_as()?;
        self.shared.store.update_pins(&machine_id, |pins| {
            removed = pins.remove(peer).is_some();
        })?;
        self.shared.view().failures.remove(peer);
        self.shared.view().settings.remove(peer);
        self.shared.view().inventories.remove(peer);
        if let Some(conn) = self.shared.conn() {
            let _ = conn.commands.send(Command::Unpair {
                peer: peer.to_string(),
            });
        }
        Ok(removed)
    }

    /// Subscribes to `remote_events`. With `access`, the client shows the
    /// access dialog, so access requests wait for its answer instead of
    /// being answered `nobody`.
    pub fn subscribe(&self, access: bool) -> Subscription {
        self.shared.subscribers.fetch_add(1, Ordering::SeqCst);
        if access {
            self.shared
                .access_subscribers
                .fetch_add(1, Ordering::SeqCst);
        }
        Subscription {
            events: self.shared.events.subscribe(),
            access,
            shared: Arc::clone(&self.shared),
        }
    }

    /// `remote_access_answer`: a person's answer in the access dialog. It
    /// changes the switches, or suppresses the asker, only if the request
    /// still waits, has not expired, the asker is still paired with the
    /// same key, and nothing wrote the settings since it was recorded.
    pub async fn answer_access(
        &self,
        request_id: &str,
        allow: bool,
        suppress: bool,
    ) -> Result<AccessOutcome> {
        // Requests end with their connection: without one, none waits.
        let Some(conn) = self.shared.conn() else {
            return Ok(AccessOutcome::Cancelled);
        };
        let (reply, answered) = oneshot::channel();
        conn.commands
            .send(Command::AccessAnswer {
                request_id: request_id.to_string(),
                allow,
                suppress,
                reply,
            })
            .map_err(|_| anyhow!("The relay connection closed."))?;
        Ok(answered.await.unwrap_or(AccessOutcome::Cancelled))
    }

    /// `remote_access_suppressed`: the peers whose requests are answered
    /// `declined` at once.
    pub fn suppressed(&self) -> Result<Vec<SuppressedPeer>> {
        Ok(self
            .shared
            .store
            .suppressions()?
            .into_iter()
            .map(|(machine_id, suppressed)| SuppressedPeer {
                machine_id,
                name: suppressed.name,
                since: suppressed.since,
            })
            .collect())
    }

    /// `remote_access_restore`: lets `peer` ask again. `true` when it was
    /// suppressed.
    pub fn restore_access(&self, peer: &str) -> Result<bool> {
        let machine_id = self.shared.signed_in_as()?;
        let mut removed = false;
        self.shared
            .store
            .update_suppressions(&machine_id, |suppressed| {
                removed = suppressed.remove(peer).is_some();
            })?;
        Ok(removed)
    }
}

/// A line of `remote_access_suppressed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SuppressedPeer {
    pub machine_id: String,
    pub name: String,
    /// RFC 3339.
    pub since: String,
}

/// Why a viewer got no lease, as its caller says it. A revoked token also
/// ends the connection, and the machine signs out, as for a renewal; a lapse
/// is the plan's; anything else, the cloud could not be reached.
fn lease_refused(conn: &Conn, error: &LeaseError) -> Failure {
    match error {
        LeaseError::Refused { status: 401, .. } => {
            let _ = conn.commands.send(Command::Revoked);
            Failure::new("signed_out", SIGNED_OUT)
        }
        LeaseError::Refused { status: 403, .. } => Failure::new("banned", BANNED),
        LeaseError::Refused { .. } => Failure::new("subscription_required", NEEDS_PRO),
        LeaseError::Unreachable(_) => Failure::new("unreachable", UNREACHABLE),
    }
}

/// Starts the connection loop; it stops with `cancel`.
pub fn spawn(link: RemoteLink, cancel: CancellationToken) {
    tokio::spawn(run(link.shared, cancel));
}

/// What the link waits for before it tries again.
enum Wait {
    Retry(Duration),
    /// A retry after Localify Cloud could not be reached, or answered in a
    /// way that is not its own: said so in the status.
    Unreachable(Duration),
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
                let revocation = Revocation::for_api_base(&shared, &ready.credentials.api_base);
                let (ending, up_for) = connect_and_serve(&shared, ready, &cancel).await;
                ever_connected |= up_for > Duration::ZERO;
                if cancel.is_cancelled() {
                    return;
                }
                info!(?ending, up_for = ?up_for, "The relay connection ended");
                // A `401` on the upgrade that is not the cloud's revocation
                // is something in between: retried.
                let ending = match ending {
                    Ending::Refused { status: 401, .. } if !revocation.revokes_upgrade(&ending) => {
                        Ending::Unreachable
                    }
                    other => other,
                };
                let next = backoff.next(&ending, up_for, &shared.timings);
                match (ending, next) {
                    (Ending::Unreachable, Next::Retry(wait)) => Wait::Unreachable(wait),
                    (_, next) => plan(&shared, &mut uploaded_for, next, &token),
                }
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
        Err(error) => return Err(cloud_refusal(shared, backoff, &error, &credentials)),
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
            Err(error) => return Err(cloud_refusal(shared, backoff, &error, &credentials)),
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
    // Compared and deleted under the sign-in lock `login` takes: a newer
    // sign-in is never taken with the revoked one.
    let passwords = PasswordStore::in_data_dir(&shared.credentials.data_dir());
    let forget_passwords = || {
        if let Err(error) = passwords.update(|saved| saved.clear()) {
            warn!(error = %format!("{error:#}"), "Could not delete the saved passwords");
        }
    };
    match shared.credentials.delete_if_token(token, forget_passwords) {
        Ok(true) => info!("Localify Cloud revoked this machine's token; signed out here"),
        // A new sign-in, or none: nothing of this one is left.
        Ok(false) => return Wait::Retry(Duration::ZERO),
        Err(error) => {
            warn!(error = %format!("{error:#}"), "Could not delete this machine's sign-in");
        }
    }
    block(shared, "signed_out", SIGNED_OUT, None)
}

fn cloud_refusal(
    shared: &Shared,
    backoff: &mut Backoff,
    error: &CloudError,
    credentials: &CloudCredentials,
) -> Wait {
    if Revocation::for_api_base(shared, &credentials.api_base).revokes(error) {
        return signed_out_by_cloud(shared, &credentials.token);
    }
    match error {
        CloudError::Api {
            status: 403, code, ..
        } if code == "banned" => block(shared, "banned", BANNED, None),
        CloudError::Api { status: 402, .. } => lapsed(shared, shared.timings.lapsed_recheck),
        // Unreachable, a 5xx, or a 401 that is not the cloud's revocation.
        _ => {
            let next = backoff.next(&Ending::Unreachable, Duration::ZERO, &shared.timings);
            match next {
                Next::Retry(wait) | Next::UploadKey(wait) => Wait::Unreachable(wait),
                Next::Stop(_) => Wait::Unreachable(shared.timings.backoff_cap),
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
        Revocation::for_api_base(shared, &ready.credentials.api_base),
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
        shared.view().set_roster(roster);
    }
    let context = actor::Context {
        self_id: ready.credentials.machine_id,
        name: ready.credentials.machine_name,
        source: Arc::clone(&shared.source),
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
    let (delay, blocked) = match wait {
        Wait::Retry(delay) => (delay, None),
        Wait::Unreachable(delay) => (
            delay,
            Some(Blocked {
                code: "unreachable".into(),
                message: UNREACHABLE.into(),
            }),
        ),
        other => return wait_for_change(shared, other, cancel).await,
    };
    let retry_at = time::OffsetDateTime::now_utc() + delay;
    let retry_at = retry_at.replace_nanosecond(0).ok().and_then(|at| {
        at.format(&time::format_description::well_known::Rfc3339)
            .ok()
    });
    let relay = shared.view().relay;
    let relay = match relay {
        RelayState::Connecting | RelayState::Blocked | RelayState::Off => RelayState::Reconnecting,
        other => other,
    };
    shared.set_relay(relay, blocked, retry_at);
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

/// Waits for `public_refresh`, or as long as `wait` says.
async fn wait_for_change(shared: &Shared, wait: Wait, cancel: &CancellationToken) -> bool {
    match wait {
        Wait::Retry(_) | Wait::Unreachable(_) => true,
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
            let mut failures = 0_u32;
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => return false,
                    _ = shared.refresh.notified() => return true,
                    _ = tokio::time::sleep(delay) => {}
                }
                match lease_granted(shared).await {
                    Granted::Yes => return true,
                    Granted::No => {
                        // Still the plan's, after an answer from the cloud.
                        failures = 0;
                        let _ = lapsed(shared, Duration::ZERO);
                        delay = shared.timings.lapsed_recheck;
                    }
                    // Suspended meanwhile: stopped, as a `4403` stops it.
                    Granted::Banned => {
                        let wait = block(shared, "banned", BANNED, None);
                        return Box::pin(wait_for_change(shared, wait, cancel)).await;
                    }
                    // No answer of the cloud's own: not a lapse. Asked
                    // again sooner, with backoff, up to the usual interval.
                    Granted::Unreachable => {
                        failures = failures.saturating_add(1);
                        let blocked = Blocked {
                            code: "unreachable".into(),
                            message: UNREACHABLE.into(),
                        };
                        shared.set_relay(RelayState::Reconnecting, Some(blocked), None);
                        delay = lease::retry_delay(
                            failures,
                            shared.timings.lease_retry,
                            random_fraction(),
                        )
                        .min(shared.timings.lapsed_recheck);
                    }
                    // Revoked meanwhile: signed out, until a new sign-in.
                    Granted::Revoked(token) => {
                        let wait = signed_out_by_cloud(shared, &token);
                        return Box::pin(wait_for_change(shared, wait, cancel)).await;
                    }
                }
            }
        }
    }
}

enum Granted {
    Yes,
    /// Refused by the plan: `402`, or a `403` other than a ban.
    No,
    /// `403 banned`: the account is suspended.
    Banned,
    /// No answer, a 5xx, or a `401` that is not the cloud's revocation.
    Unreachable,
    /// The token is revoked; the token it was.
    Revoked(String),
}

/// Whether Localify Cloud grants a lease again. Without credentials there is
/// nothing to wait for: the next pass says the machine is signed out.
async fn lease_granted(shared: &Shared) -> Granted {
    let Ok(Some(credentials)) = shared.credentials.load() else {
        return Granted::Yes;
    };
    let revocation = Revocation::for_api_base(shared, &credentials.api_base);
    let Ok(http) = CloudHttp::new(
        credentials.api_base.clone(),
        Some(credentials.token.clone()),
    ) else {
        return Granted::Yes;
    };
    match CloudClient::new(http).remote_lease().await {
        Ok(_) => Granted::Yes,
        Err(error) if revocation.revokes(&error) => Granted::Revoked(credentials.token.clone()),
        Err(CloudError::Api {
            status: 403, code, ..
        }) if code == "banned" => {
            info!("Localify Cloud suspended this account");
            Granted::Banned
        }
        Err(CloudError::Api {
            status: 402 | 403, ..
        }) => {
            info!("Remote access is still lapsed");
            Granted::No
        }
        Err(error) => {
            info!(error = %error, "Localify Cloud did not answer a lease request");
            Granted::Unreachable
        }
    }
}
