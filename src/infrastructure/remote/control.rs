//! The control executor on a target (`docs/remote-machines.md`, "Controls";
//! ADR 0031): checks 4 to 9 of "Target checks", "Who acts", and the state
//! machine of one control, from its request entry to its result. The
//! daemon's actor makes checks 1 to 3 (the pin and `hello`, the switches, a
//! fresh lease) before it hands a control here, off its own task: everything
//! here blocks.
//!
//! What a control sees and does on this machine goes through [`Host`]: the
//! daemon's own config, run records, app bridge and executable
//! ([`LocalHost`]), or a fake in tests. The decisions, the request entries and
//! the timing are this module's, whatever the host.

use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow};

use super::requests::{
    Fingerprint, LinkedLaunch, Published, Report, RequestEntries, RequestEntry, Stopped,
    SweepCursor, Via,
};
use super::wire::{Control, ControlAction, ControlOutcome, ControlPhase};
use crate::application::inventory::{
    AppPresence, Context, RunFacts, RunOrigin, RunProcess, ServiceFacts, Switches, inventory_item,
};
use crate::branding::BRANDING;
use crate::infrastructure::app_bridge::{self, Handoff, LaunchRequest, Requester, ServiceRef};
use crate::infrastructure::config::ConfigStore;
use crate::infrastructure::inventory::look_up;
use crate::infrastructure::launch_profiles::allowed_remotely;
use crate::infrastructure::launch_rev::LaunchRevKey;
use crate::infrastructure::process::{self, NativeProcesses, Signal};
use crate::infrastructure::running_services::{
    RunningServiceStore, SERVICE_ORIGIN_ENV, ServiceKey, ServiceOrigin,
};
use crate::infrastructure::runs::RunId;
use crate::infrastructure::runs::launches::{
    ExitReason, LaunchReport, RefusalCode, reconcile, unix_ms_now,
};
use crate::infrastructure::runs::output;
use crate::infrastructure::shell_env::Environment;

/// How many of a failed run's last lines its result carries.
const FAILURE_LINES: usize = 40;
/// The most a result's message takes once JSON-escaped, so a
/// `control.result` stays well within the 32 KiB of a session message.
const MESSAGE_BUDGET: usize = 16 * 1024;
/// A peer creates at most this many request entries in [`ENTRY_WINDOW`]:
/// each one is kept 24 hours.
const ENTRIES_PER_WINDOW: usize = 20;
const ENTRY_WINDOW: Duration = Duration::from_secs(10 * 60);
/// Entries past their retention are swept at most this often, looking at
/// this many entries a pass.
const SWEEP_EVERY: Duration = Duration::from_secs(10 * 60);
const SWEEP_BUDGET: usize = 512;

const EXPIRED: &str = "The request took too long to reach this machine. Nothing was started.";
const BUSY: &str = "Something else is starting or stopping this service here.";
const CHANGED: &str = "The service changed since it was looked at.";

/// The contract's delays. Tests shorten them.
#[derive(Debug, Clone, Copy)]
pub struct ControlTimings {
    /// A launch expires this long after it is dispatched.
    pub dispatch_budget: Duration,
    /// A started run has this long, from its admission, to listen.
    pub settle: Duration,
    /// How often a launch's record and its port are looked at.
    pub poll: Duration,
    /// A control's `expiresAt` may be at most this far ahead.
    pub ahead: Duration,
}

impl Default for ControlTimings {
    fn default() -> Self {
        Self {
            dispatch_budget: Duration::from_secs(15),
            settle: Duration::from_secs(90),
            poll: Duration::from_millis(250),
            ahead: Duration::from_secs(60),
        }
    }
}

/// A control from a peer, once the actor checked who sent it.
#[derive(Debug, Clone)]
pub struct Order {
    /// The peer's machine id.
    pub peer: String,
    /// Its name as this machine pinned it, for "Started by" in the app.
    pub peer_name: String,
    pub control: Control,
}

/// One service, as a control looks at it right before it acts.
#[derive(Debug, Clone)]
pub struct Look {
    pub facts: ServiceFacts,
    /// Its record's key: the environment's slug, the primary's included.
    pub key: ServiceKey,
    /// Whether the profile a start names may be launched remotely.
    pub profile_allowed: bool,
    /// Whether the service's record still names the legacy run the control
    /// expects, proven or not.
    pub names_legacy: bool,
}

/// The run a stop ends, as the control expects it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expected {
    Run(RunId),
    Legacy(String),
}

/// A start the daemon spawns itself, with the app closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetachedLaunch {
    pub launch_id: RunId,
    /// Unix milliseconds.
    pub expires_at: u64,
    pub key: ServiceKey,
    pub profile: Option<String>,
    pub expect_rev: String,
    pub machine_id: String,
    pub request_id: String,
    pub replaces: Option<RunId>,
    pub replaces_legacy: Option<String>,
}

/// A detached start's process, until its launch is decided.
pub trait Spawned: Send {
    /// Once the launch is decided: an `expired` one has its process's
    /// session killed, an admitted one is left running. Either way the
    /// process is reaped.
    fn settle(self: Box<Self>, admitted: bool);
}

/// What a control sees and does on this machine.
pub trait Host: Send + Sync {
    /// The service `wanted` names, `environment` `None` for the primary
    /// checkout, or `None` when the config has no such service.
    fn look(
        &self,
        wanted: &ServiceKey,
        profile: Option<&str>,
        legacy_id: Option<&str>,
    ) -> Result<Option<Look>>;
    /// Ends the run `expected` names, checked again right before it is
    /// signalled, and returns once it has ended. A run that changed is
    /// refused with its report.
    fn stop(&self, key: &ServiceKey, expected: &Expected) -> Result<(), Report>;
    /// Hands a launch to the app, once.
    fn launch_in_app(&self, request: &LaunchRequest) -> Handoff;
    fn launch_detached(&self, launch: &DetachedLaunch) -> Result<Box<dyn Spawned>>;
    /// What happened to a launch, from its record. With `expires_at`
    /// passed and no record, `expired` is published first.
    fn report(&self, launch_id: &RunId, expires_at: Option<u64>) -> Result<LaunchReport>;
    fn listens(&self, port: u16) -> bool;
    /// The last lines a run printed, for a failure.
    fn last_lines(&self, run_id: &RunId, count: usize) -> String;
}

/// Runs the controls of this machine's peers, one per service at a time.
pub struct ControlRunner {
    host: Arc<dyn Host>,
    entries: RequestEntries,
    timings: ControlTimings,
    /// The services with a control in flight.
    busy: Mutex<HashSet<ServiceKey>>,
    /// The requests being worked on, by peer and request id.
    working: Mutex<HashSet<(String, String)>>,
    /// When each peer created its last request entries, within
    /// [`ENTRY_WINDOW`].
    created: Mutex<HashMap<String, VecDeque<Instant>>>,
    /// When entries past their retention were last swept, and where the
    /// next pass goes on.
    swept: Mutex<SweepState>,
}

/// When the last sweep ran, and where the next one goes on.
#[derive(Default)]
struct SweepState {
    at: Option<Instant>,
    resume: Option<SweepCursor>,
}

/// `too_many` when `peer` created [`ENTRIES_PER_WINDOW`] entries within
/// [`ENTRY_WINDOW`], with how long until one more fits.
fn too_many(
    created: &mut HashMap<String, VecDeque<Instant>>,
    peer: &str,
    now: Instant,
) -> Option<Report> {
    created.retain(|_, times| {
        times.retain(|at| now.duration_since(*at) < ENTRY_WINDOW);
        !times.is_empty()
    });
    let times = created.get(peer)?;
    if times.len() < ENTRIES_PER_WINDOW {
        return None;
    }
    let free = times.front().map_or(ENTRY_WINDOW, |oldest| {
        ENTRY_WINDOW.saturating_sub(now.duration_since(*oldest))
    });
    Some(Report::refused(
        "too_many",
        format!(
            "That machine sent too many controls here lately. Try again in {} s.",
            free.as_secs().max(1)
        ),
    ))
}

/// What a control reads again as it goes, on this machine now.
pub struct Live<'a> {
    /// The settings and the app.
    pub switches: &'a dyn Fn() -> Switches,
    /// Whether the peer is still paired, with the key its session
    /// authenticated: an unpairing or a new key since cancels what was not
    /// dispatched yet.
    pub paired: &'a dyn Fn() -> bool,
}

/// What a control goes on to do once every check passed.
struct Plan {
    stop: Option<Expected>,
    launch: Option<Dispatch>,
}

/// Where a launch went.
enum Handed {
    App { unanswered: bool },
    Detached(Box<dyn Spawned>),
}

struct Dispatch {
    via: Via,
    profile: Option<String>,
    replaces: Option<RunId>,
    replaces_legacy: Option<String>,
}

impl ControlRunner {
    pub fn new(host: Arc<dyn Host>, entries: RequestEntries, timings: ControlTimings) -> Self {
        Self {
            host,
            entries,
            timings,
            busy: Mutex::new(HashSet::new()),
            working: Mutex::new(HashSet::new()),
            created: Mutex::new(HashMap::new()),
            swept: Mutex::new(SweepState::default()),
        }
    }

    /// Runs one control, from check 4 on: a request already recorded is
    /// never run again, and gets its report. `live` reads the settings, the
    /// app and the pairing as they are now; `progress` hears `dispatched`
    /// and `started`. Returns the result.
    pub fn run(
        &self,
        order: &Order,
        live: &Live<'_>,
        progress: &mut dyn FnMut(ControlPhase),
    ) -> Report {
        let (peer, request_id) = (&order.peer, &order.control.request_id);
        let fingerprint = Fingerprint::of(&order.control);
        self.sweep_now_and_then();
        match self.entries.read(peer, request_id) {
            Ok(Some(entry)) => return self.again(peer, request_id, &entry, &fingerprint),
            Ok(None) => {}
            Err(error) => return unavailable(&error),
        }
        if let Err(report) = self.count_entry(peer) {
            return report;
        }
        // Published before anything changes, so every outcome from here on,
        // a refusal included, is linked to it.
        let entry = RequestEntry::new(fingerprint.clone(), unix_ms_now());
        let _working = Held::enter(&self.working, (peer.clone(), request_id.clone()));
        match self.entries.publish(peer, request_id, &entry) {
            Ok(Published::New) => {}
            Ok(Published::Existing(entry)) => {
                return self.again(peer, request_id, &entry, &fingerprint);
            }
            Err(error) => return unavailable(&error),
        }
        let report = self.fresh(order, live, progress);
        if report.is_final() {
            let _ = self.entries.update(peer, request_id, |entry| {
                entry.result = Some(report.clone());
            });
        }
        report
    }

    /// `control.status`: the entry's report, without running anything.
    pub fn status(&self, peer: &str, request_id: &str) -> Report {
        self.sweep_now_and_then();
        match self.entries.read(peer, request_id) {
            Ok(Some(entry)) => self.report_entry(peer, request_id, &entry),
            Ok(None) => Report::refused(
                "unknown_request",
                "This machine has no record of that request.",
            ),
            Err(error) => unavailable(&error),
        }
    }

    /// Whether a control under `request_id` may go on, before the target
    /// asks for its lease: a request id with an entry always may (it gets
    /// its report), a new one only while `peer` is within its entry limit.
    /// [`ControlRunner::run`] counts the entry when it publishes it.
    pub fn admits(&self, peer: &str, request_id: &str) -> Result<(), Report> {
        if matches!(self.entries.read(peer, request_id), Ok(Some(_))) {
            return Ok(());
        }
        let mut created = lock(&self.created);
        match too_many(&mut created, peer, Instant::now()) {
            Some(refused) => Err(refused),
            None => Ok(()),
        }
    }

    /// A new request entry for `peer`, unless it made too many lately:
    /// each one is kept 24 hours, so fresh request ids cannot fill the disk.
    fn count_entry(&self, peer: &str) -> Result<(), Report> {
        let now = Instant::now();
        let mut created = lock(&self.created);
        if let Some(refused) = too_many(&mut created, peer, now) {
            return Err(refused);
        }
        created.entry(peer.to_string()).or_default().push_back(now);
        Ok(())
    }

    /// Sweeps the entries past their retention, at most every
    /// [`SWEEP_EVERY`] and [`SWEEP_BUDGET`] entries a pass, on from where
    /// the last pass stopped: a publication never scans them all.
    fn sweep_now_and_then(&self) {
        let now = Instant::now();
        let from = {
            let mut swept = lock(&self.swept);
            if swept
                .at
                .is_some_and(|at| now.duration_since(at) < SWEEP_EVERY)
            {
                return;
            }
            swept.at = Some(now);
            swept.resume.take()
        };
        let pass = self.entries.sweep(from.as_ref(), SWEEP_BUDGET);
        lock(&self.swept).resume = pass.ok().and_then(|pass| pass.resume);
    }

    /// A request id that already has an entry: the same control gets its
    /// report, another one `request_mismatch`.
    fn again(
        &self,
        peer: &str,
        request_id: &str,
        entry: &RequestEntry,
        fingerprint: &Fingerprint,
    ) -> Report {
        if entry.fingerprint != *fingerprint {
            return Report::refused(
                "request_mismatch",
                "This request doesn't match the one this machine recorded.",
            );
        }
        self.report_entry(peer, request_id, entry)
    }

    fn report_entry(&self, peer: &str, request_id: &str, entry: &RequestEntry) -> Report {
        if let Some(result) = &entry.result {
            return result.clone();
        }
        let service = &entry.fingerprint.service.service_id;
        if let Some(launch) = &entry.launch {
            return self.launch_report(launch, service);
        }
        let working = lock(&self.working).contains(&(peer.to_string(), request_id.to_string()));
        if working {
            return Report::new(
                ControlOutcome::Unknown,
                None,
                "This machine is still working on it.",
            );
        }
        // Nothing works on it any more: Localify restarted here meanwhile.
        match (&entry.stopped, entry.fingerprint.action) {
            (Some(_), ControlAction::Stop) => Report::new(ControlOutcome::Done, None, "Stopped."),
            (Some(_), _) => Report::new(
                ControlOutcome::Failed,
                None,
                format!(
                    "Localify restarted on this machine after it stopped {service} for the restart, before it started it again. {service} is stopped: start it again."
                ),
            ),
            (None, _) => Report::new(
                ControlOutcome::Failed,
                None,
                "Localify restarted on this machine before the request finished. Look at the service, then try again.",
            ),
        }
    }

    /// Checks 5 to 9, then who acts.
    fn fresh(
        &self,
        order: &Order,
        live: &Live<'_>,
        progress: &mut dyn FnMut(ControlPhase),
    ) -> Report {
        let control = &order.control;
        if let Err(report) = self.check_expiry(&control.expires_at) {
            return report;
        }
        if let Err(report) = check_arguments(control) {
            return report;
        }
        let wanted = ServiceKey {
            project_id: control.service.project_id.clone(),
            environment: control.service.environment.clone(),
            service_id: control.service.service_id.clone(),
        };
        let look = match self.host.look(
            &wanted,
            control.profile.as_deref(),
            control.expect.legacy_id.as_deref(),
        ) {
            Ok(Some(look)) => look,
            Ok(None) => {
                return Report::refused(
                    "unknown_service",
                    "This service no longer exists on this machine.",
                );
            }
            Err(error) => return unavailable(&error),
        };
        if control.action == ControlAction::Start && !look.profile_allowed {
            return refused_profile(control.profile.as_deref());
        }
        let Some(_busy) = Held::enter(&self.busy, look.key.clone()) else {
            return Report::refused("busy", BUSY);
        };
        if let Err(report) = state_matches(control, &look) {
            return report;
        }
        let plan = match self.owner_allows(control, &look, (live.switches)()) {
            Ok(plan) => plan,
            Err(report) => return report,
        };
        self.execute(order, &look, plan, live, progress)
    }

    /// Check 5: not past `expiresAt` in this machine's clock, and at most
    /// 60 seconds ahead.
    fn check_expiry(&self, expires_at: &str) -> Result<(), Report> {
        let Some(expires) = rfc3339_ms(expires_at) else {
            return Err(Report::refused(
                "bad_request",
                format!("'{expires_at}' is not an RFC 3339 time."),
            ));
        };
        let now = unix_ms_now();
        let ahead = u64::try_from(self.timings.ahead.as_millis()).unwrap_or(u64::MAX);
        if now >= expires || expires > now.saturating_add(ahead) {
            return Err(Report::refused("expired", EXPIRED));
        }
        Ok(())
    }

    /// Check 9 and "Who acts": the owner allows it, the replacement of a
    /// restart included, all decided before anything stops.
    fn owner_allows(
        &self,
        control: &Control,
        look: &Look,
        switches: Switches,
    ) -> Result<Plan, Report> {
        let context = Context {
            settings: switches.settings,
            app: switches.app,
            tunnel_connected: false,
        };
        let controls = inventory_item(look.facts.clone(), &context).controls;
        let allowed = match control.action {
            ControlAction::Start => controls.start,
            ControlAction::Stop => controls.stop,
            ControlAction::Restart => controls.restart,
        };
        if let Some(code) = allowed.code.filter(|_| !allowed.allowed) {
            return Err(Report::refused(code.code(), refusal_message(code.code())));
        }
        let run = look.facts.run.as_ref();
        let expected = match (
            &control.expect.legacy_id,
            run.and_then(|run| run.run_id.as_ref()),
        ) {
            (Some(legacy_id), _) => Some(Expected::Legacy(legacy_id.clone())),
            (None, Some(run_id)) => Some(Expected::Run(
                RunId::parse(run_id).map_err(|error| unavailable(&error))?,
            )),
            (None, None) => None,
        };
        Ok(plan(control, run, expected, switches.app))
    }

    fn execute(
        &self,
        order: &Order,
        look: &Look,
        plan: Plan,
        live: &Live<'_>,
        progress: &mut dyn FnMut(ControlPhase),
    ) -> Report {
        let (peer, request_id) = (&order.peer, &order.control.request_id);
        // Right before anything is dispatched: an unpairing, a new key or a
        // switch turned off since the checks, and nothing new starts.
        if !(live.paired)() {
            return Report::refused(
                "not_paired",
                "This machine is no longer paired with that one.",
            );
        }
        let now = (live.switches)().settings;
        if !(now.share && now.control) {
            return Report::refused("control_disabled", refusal_message("control_disabled"));
        }
        progress(ControlPhase::Dispatched);
        if let Some(expected) = &plan.stop {
            if let Err(report) = self.host.stop(&look.key, expected) {
                return report;
            }
            let stopped = Stopped {
                run_id: match expected {
                    Expected::Run(run_id) => Some(run_id.to_string()),
                    Expected::Legacy(_) => None,
                },
                legacy_id: match expected {
                    Expected::Legacy(legacy_id) => Some(legacy_id.clone()),
                    Expected::Run(_) => None,
                },
                at: unix_ms_now(),
            };
            let _ = self.entries.update(peer, request_id, |entry| {
                entry.stopped = Some(stopped);
            });
        }
        let Some(dispatch) = plan.launch else {
            return Report::new(ControlOutcome::Done, None, "Stopped.");
        };
        match self.dispatch(order, look, dispatch, live, progress) {
            Ok(report) => report,
            Err(error) => Report::new(
                ControlOutcome::Failed,
                None,
                format!("The service could not be started: {error:#}"),
            ),
        }
    }

    /// Hands the launch to its owner, with an id and an expiry fixed now,
    /// then follows it: its decision, until the expiry at most, then its
    /// settling. It is never handed to anyone else.
    fn dispatch(
        &self,
        order: &Order,
        look: &Look,
        dispatch: Dispatch,
        live: &Live<'_>,
        progress: &mut dyn FnMut(ControlPhase),
    ) -> Result<Report> {
        let control = &order.control;
        let launch_id = RunId::generate()?;
        let budget = u64::try_from(self.timings.dispatch_budget.as_millis()).unwrap_or(u64::MAX);
        let expires_at = unix_ms_now().saturating_add(budget);
        let linked = LinkedLaunch {
            id: launch_id.clone(),
            expires_at,
            via: dispatch.via,
            port: look.facts.port,
        };
        self.entries
            .update(&order.peer, &control.request_id, |entry| {
                entry.launch = Some(linked.clone());
            })?;
        let handed = match self.hand_off(order, look, dispatch, &linked)? {
            Ok(handed) => handed,
            Err(report) => return Ok(report),
        };
        let (spawned, unanswered) = match handed {
            Handed::App { unanswered } => (None, unanswered),
            Handed::Detached(spawned) => (Some(spawned), false),
        };
        let decision = if unanswered {
            self.after_expiry(&linked.id, linked.expires_at)
        } else {
            self.decision(&linked.id, linked.expires_at)
        };
        let admitted = matches!(
            decision,
            Ok(LaunchReport::Starting | LaunchReport::Running | LaunchReport::Exited { .. })
                | Ok(LaunchReport::Replaced { .. })
        );
        if let Some(spawned) = spawned {
            spawned.settle(admitted);
        }
        // A run's output goes out only while this machine shares it.
        let shared = || (live.switches)().settings.share;
        match decision? {
            LaunchReport::Starting | LaunchReport::Running => {
                progress(ControlPhase::Started);
                Ok(self.settle(&linked, &look.key.service_id, &shared))
            }
            other => Ok(self.describe(other, &linked, &look.key.service_id, shared())),
        }
    }

    /// Hands the launch to the app, or spawns it detached, once. A launch
    /// that cannot have started is answered at once.
    fn hand_off(
        &self,
        order: &Order,
        look: &Look,
        dispatch: Dispatch,
        linked: &LinkedLaunch,
    ) -> Result<Result<Handed, Report>> {
        let request_id = order.control.request_id.clone();
        if dispatch.via == Via::Background {
            let spawned = self.host.launch_detached(&DetachedLaunch {
                launch_id: linked.id.clone(),
                expires_at: linked.expires_at,
                key: look.key.clone(),
                profile: dispatch.profile,
                expect_rev: look.facts.service.config_rev.clone(),
                machine_id: order.peer.clone(),
                request_id,
                replaces: dispatch.replaces,
                replaces_legacy: dispatch.replaces_legacy,
            })?;
            return Ok(Ok(Handed::Detached(spawned)));
        }
        let request = LaunchRequest {
            launch_id: linked.id.clone(),
            expires_at: linked.expires_at,
            service: ServiceRef {
                project_id: look.key.project_id.clone(),
                environment: look.key.environment.clone(),
                service_id: look.key.service_id.clone(),
            },
            profile: dispatch.profile,
            expect_rev: Some(look.facts.service.config_rev.clone()),
            replaces: dispatch.replaces,
            replaces_legacy: dispatch.replaces_legacy,
            requested_by: Some(Requester {
                machine_id: order.peer.clone(),
                name: order.peer_name.clone(),
                request_id,
            }),
        };
        Ok(match self.host.launch_in_app(&request) {
            Handoff::Accepted => Ok(Handed::App { unanswered: false }),
            // It may have started: only the record can tell, once the
            // launch has expired.
            Handoff::Unanswered(_) => Ok(Handed::App { unanswered: true }),
            Handoff::Absent => Err(app_closed(&look.key.service_id)),
            Handoff::Refused(refusal) => Err(Report::refused(&refusal.code, refusal.message)),
        })
    }

    /// The launch's decision, waiting for it until its expiry has passed,
    /// when the reconciler's `expired` decides it if nothing else did.
    fn decision(&self, launch_id: &RunId, expires_at: u64) -> Result<LaunchReport> {
        loop {
            match self.host.report(launch_id, None)? {
                LaunchReport::Unknown if unix_ms_now() > expires_at => {
                    return self.host.report(launch_id, Some(expires_at));
                }
                LaunchReport::Unknown => thread::sleep(self.timings.poll),
                decided => return Ok(decided),
            }
        }
    }

    /// A launch the app never answered: once its expiry has passed, its
    /// record is decided, `expired` when nothing admitted it, and no run
    /// with its id can start any more.
    fn after_expiry(&self, launch_id: &RunId, expires_at: u64) -> Result<LaunchReport> {
        loop {
            let now = unix_ms_now();
            if now > expires_at {
                return self.host.report(launch_id, Some(expires_at));
            }
            thread::sleep(Duration::from_millis((expires_at - now + 1).min(250)));
        }
    }

    /// From its admission, a run has the settling time to listen: `done`
    /// once it does, `failed` when it exits first, `starting` after that.
    /// The launch's own record comes first: a port another process listens
    /// on never makes a launch that ended a success.
    fn settle(&self, launch: &LinkedLaunch, service: &str, shared: &dyn Fn() -> bool) -> Report {
        let deadline = Instant::now() + self.timings.settle;
        loop {
            match self.host.report(&launch.id, None) {
                Ok(LaunchReport::Running) if self.host.listens(launch.port) => {
                    return started(service, &launch.id);
                }
                Ok(LaunchReport::Starting | LaunchReport::Running | LaunchReport::Unknown) => {}
                Ok(other) => return self.describe(other, launch, service, shared()),
                Err(error) => return unavailable(&error),
            }
            if Instant::now() >= deadline {
                return Report::new(
                    ControlOutcome::Starting,
                    None,
                    format!("Started {service} on this machine, not listening yet."),
                )
                .with_run(Some(launch.id.to_string()));
            }
            thread::sleep(self.timings.poll);
        }
    }

    /// A linked launch, as its record says now.
    fn launch_report(&self, launch: &LinkedLaunch, service: &str) -> Report {
        match self.host.report(&launch.id, Some(launch.expires_at)) {
            Ok(LaunchReport::Running) if self.host.listens(launch.port) => {
                started(service, &launch.id)
            }
            // Asked only while this machine shares: its output may go out.
            Ok(report) => self.describe(report, launch, service, true),
            Err(error) => unavailable(&error),
        }
    }

    /// What one launch report means for the control that dispatched it. A
    /// failure carries the run's last lines when `output` allows it.
    fn describe(
        &self,
        report: LaunchReport,
        launch: &LinkedLaunch,
        service: &str,
        output: bool,
    ) -> Report {
        let run = Some(launch.id.to_string());
        match report {
            LaunchReport::Unknown => Report::new(
                ControlOutcome::Unknown,
                None,
                "Its launch has no record yet.",
            )
            .with_run(run),
            LaunchReport::Starting | LaunchReport::Running => Report::new(
                ControlOutcome::Starting,
                None,
                format!("Started {service} on this machine, not listening yet."),
            )
            .with_run(run),
            LaunchReport::Exited {
                code,
                signal,
                reason,
            } => {
                let head = exited(service, code, signal, reason);
                let lines = if output {
                    self.host.last_lines(&launch.id, FAILURE_LINES)
                } else {
                    String::new()
                };
                let message = with_last_lines(head, &lines, MESSAGE_BUDGET);
                Report::new(ControlOutcome::Failed, None, message).with_run(run)
            }
            LaunchReport::Replaced { by } => Report::refused(
                "conflict",
                format!("Another launch replaced {service} meanwhile."),
            )
            .with_run(Some(by.to_string())),
            LaunchReport::Refused { code, message } => Report::refused(refusal_code(code), message),
            LaunchReport::Expired => match launch.via {
                Via::App => app_closed(service),
                Via::Background => Report::new(ControlOutcome::Failed, Some("expired"), EXPIRED),
            },
        }
    }
}

/// Who acts, once the owner allowed it: a stop of the expected run, a
/// launch through the run's owner (the app, or the background), or both
/// for a restart, which keeps the run's profile.
fn plan(
    control: &Control,
    run: Option<&RunFacts>,
    expected: Option<Expected>,
    app: AppPresence,
) -> Plan {
    let via = |origin: Option<RunOrigin>| match origin {
        Some(RunOrigin::Background) => Via::Background,
        Some(RunOrigin::App) => Via::App,
        _ if app == AppPresence::Running => Via::App,
        _ => Via::Background,
    };
    match control.action {
        ControlAction::Stop => Plan {
            stop: expected,
            launch: None,
        },
        ControlAction::Start => Plan {
            stop: None,
            launch: Some(Dispatch {
                via: via(None),
                profile: control.profile.clone(),
                replaces: None,
                replaces_legacy: None,
            }),
        },
        ControlAction::Restart => Plan {
            launch: Some(Dispatch {
                via: via(run.map(|run| run.origin)),
                // A restart keeps the run's profile.
                profile: run.and_then(|run| run.profile.clone()),
                replaces: match &expected {
                    Some(Expected::Run(run_id)) => Some(run_id.clone()),
                    _ => None,
                },
                replaces_legacy: control.expect.legacy_id.clone(),
            }),
            stop: expected,
        },
    }
}

/// Check 6, for what a control may name beyond the service: a profile only
/// for a start, and a legacy run only for a restart.
fn check_arguments(control: &Control) -> Result<(), Report> {
    if control.action != ControlAction::Start && control.profile.is_some() {
        return Err(Report::refused(
            "refused_argument",
            "A restart keeps the run's profile, and a stop takes none.",
        ));
    }
    if control.action != ControlAction::Restart && control.expect.legacy_id.is_some() {
        return Err(Report::refused(
            "refused_argument",
            "expect.legacyId names the legacy run a restart relaunches.",
        ));
    }
    Ok(())
}

/// Check 8: for a start, nothing runs and the port is free; for a stop or a
/// restart, the running run is the one expected. A start or a restart also
/// needs the revision the viewer saw.
fn state_matches(control: &Control, look: &Look) -> Result<(), Report> {
    let facts = &look.facts;
    let live = facts
        .run
        .as_ref()
        .filter(|run| run.process != RunProcess::Stopped);
    let changed = |what: &str| Err(Report::refused("conflict", format!("{CHANGED} {what}")));
    match control.action {
        ControlAction::Start if live.is_some() || facts.reachable == Some(true) => {
            return changed("It is running already.");
        }
        ControlAction::Start => {}
        ControlAction::Stop | ControlAction::Restart => {
            if let Some(legacy_id) = &control.expect.legacy_id {
                if control.expect.run_id.is_some() || !look.names_legacy {
                    return changed(&format!("It no longer runs legacy run {legacy_id}."));
                }
            } else {
                let current = live.map(|run| run.run_id.as_deref());
                if current != Some(control.expect.run_id.as_deref()) {
                    return changed("Another run is in place, or none.");
                }
            }
        }
    }
    if control.action != ControlAction::Stop
        && facts.service.config_rev != control.expect.config_rev
    {
        return changed("Its configuration or launch profiles changed.");
    }
    Ok(())
}

fn started(service: &str, launch_id: &RunId) -> Report {
    Report::new(
        ControlOutcome::Done,
        None,
        format!("{service} is running on this machine."),
    )
    .with_run(Some(launch_id.to_string()))
}

fn app_closed(service: &str) -> Report {
    Report::new(
        ControlOutcome::Failed,
        None,
        format!(
            "The Localify app on this machine closed before starting {service}. Nothing was started."
        ),
    )
}

fn refused_profile(profile: Option<&str>) -> Report {
    Report::refused(
        "refused_profile",
        format!(
            "Profile '{}' cannot be started from another machine: it is dangerous, unknown or removed.",
            profile.unwrap_or("default")
        ),
    )
}

fn unavailable(error: &dyn std::fmt::Display) -> Report {
    Report::new(
        ControlOutcome::Failed,
        Some("unavailable"),
        format!("This machine could not act on it: {error:#}"),
    )
}

fn refusal_code(code: RefusalCode) -> &'static str {
    match code {
        RefusalCode::Conflict => "conflict",
        RefusalCode::Expired => "expired",
        RefusalCode::RefusedProfile => "refused_profile",
    }
}

fn refusal_message(code: &str) -> &'static str {
    match code {
        "control_disabled" => "This machine doesn't allow remote controls.",
        "background_disabled" => {
            "Localify isn't open on this machine, and it doesn't allow background starts."
        }
        "not_owned" => "This runs in a terminal on this machine. Stop it there.",
        "legacy_run" => "Relaunch it on that machine to enable remote control.",
        "owner_gone" => "Its app is gone: stop it, then start it again.",
        "refused_profile" => {
            "Its profile is dangerous, unknown or removed: start it on this machine."
        }
        _ => CHANGED,
    }
}

fn exited(
    service: &str,
    code: Option<i32>,
    signal: Option<i32>,
    reason: Option<ExitReason>,
) -> String {
    match (reason, code, signal) {
        (Some(ExitReason::InterruptedBeforeRegistration), _, _) => {
            format!("Localify stopped before {service} started; nothing ran.")
        }
        (Some(ExitReason::RegistrationFailed), _, _) => {
            format!("{service} could not be recorded, so it did not start.")
        }
        (Some(ExitReason::NotStarted), _, _) => {
            format!(
                "{service} did not start: something failed before it ran, such as a profile source."
            )
        }
        (Some(ExitReason::Stopped), _, _) => format!("{service} was stopped."),
        (Some(ExitReason::MaxDuration), _, _) => {
            format!("{service} reached its profile's maximum duration.")
        }
        (Some(ExitReason::ExecutorLost), _, _) => {
            format!("{service}'s localify dev died before it recorded how the service ended.")
        }
        (None, Some(code), _) => format!("{service} exited with code {code}."),
        (None, None, Some(signal)) => format!("{service} was ended by signal {signal}."),
        (None, None, None) => format!("{service} exited."),
    }
}

/// `head`, then as many of the last `lines` as fit in `budget` bytes once
/// JSON-escaped, the newest kept; what was cut says so. A last line too long
/// on its own keeps its end.
fn with_last_lines(head: String, lines: &str, budget: usize) -> String {
    const CUT: &str = "[earlier output cut]";
    if lines.trim().is_empty() {
        return head;
    }
    // The head, the blank line after it, and the cut mark with its newline.
    let mut used = escaped_len(&head) + 4 + escaped_len(CUT) + 2;
    let mut kept: Vec<&str> = Vec::new();
    let mut cut = false;
    for line in lines.lines().rev() {
        let cost = escaped_len(line) + 2;
        if used + cost > budget {
            if kept.is_empty() {
                kept.push(tail_within(line, budget.saturating_sub(used)));
            }
            cut = true;
            break;
        }
        used += cost;
        kept.push(line);
    }
    kept.reverse();
    let mut message = head;
    message.push_str("\n\n");
    if cut {
        message.push_str(CUT);
        message.push('\n');
    }
    message.push_str(&kept.join("\n"));
    message
}

/// The longest end of `line` within `budget` bytes once JSON-escaped.
fn tail_within(line: &str, budget: usize) -> &str {
    let mut used = 0;
    let mut start = line.len();
    for (index, c) in line.char_indices().rev() {
        used += escaped_char_len(c);
        if used > budget {
            break;
        }
        start = index;
    }
    &line[start..]
}

/// The length of `text` inside a JSON string, as `serde_json` escapes it.
fn escaped_len(text: &str) -> usize {
    text.chars().map(escaped_char_len).sum()
}

fn escaped_char_len(c: char) -> usize {
    match c {
        '"' | '\\' | '\n' | '\r' | '\t' | '\u{8}' | '\u{c}' => 2,
        c if u32::from(c) < 0x20 => 6,
        c => c.len_utf8(),
    }
}

/// RFC 3339 as Unix milliseconds.
fn rfc3339_ms(text: &str) -> Option<u64> {
    let at =
        time::OffsetDateTime::parse(text, &time::format_description::well_known::Rfc3339).ok()?;
    u64::try_from(at.unix_timestamp_nanos() / 1_000_000).ok()
}

/// One member of a set, held until dropped; `None` while another holds it.
struct Held<'a, T: std::hash::Hash + Eq> {
    set: &'a Mutex<HashSet<T>>,
    member: Option<T>,
}

impl<'a, T: std::hash::Hash + Eq + Clone> Held<'a, T> {
    fn enter(set: &'a Mutex<HashSet<T>>, member: T) -> Option<Self> {
        let entered = lock(set).insert(member.clone());
        // Built only once in: dropping one removes its member.
        entered.then(|| Self {
            set,
            member: Some(member),
        })
    }
}

impl<T: std::hash::Hash + Eq> Drop for Held<'_, T> {
    fn drop(&mut self) {
        if let Some(member) = self.member.take() {
            lock(self.set).remove(&member);
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

// --- This machine ---------------------------------------------------------

/// Reads something the daemon knows and this layer does not: the shell
/// environment, the checkouts the app knows.
pub type Reader<T> = Arc<dyn Fn() -> T + Send + Sync>;

/// The daemon's own config, run records, app bridge and executable.
pub struct LocalHost {
    pub config_path: PathBuf,
    pub data_dir: PathBuf,
    pub services_dir: PathBuf,
    /// This executable, canonicalized once at start: never looked up on
    /// `PATH`.
    pub executable: Result<PathBuf, String>,
    /// The user's shell environment, or the daemon's own when it could not
    /// be read.
    pub environment: Reader<Environment>,
    /// What selects this installation, which a run always gets whatever the
    /// shell sets: `HOME`, the install profile, and `XDG_STATE_HOME`.
    pub installation: Vec<(String, Option<OsString>)>,
    pub project_roots: Reader<HashMap<String, PathBuf>>,
}

impl LocalHost {
    fn store(&self) -> RunningServiceStore {
        RunningServiceStore::new(self.services_dir.clone())
    }

    /// The executable a detached start runs: this process's own, canonical.
    pub fn own_executable() -> Result<PathBuf, String> {
        std::env::current_exe()
            .and_then(|path| path.canonicalize())
            .map_err(|error| format!("Failed to find this executable: {error}"))
    }

    /// This process's own installation: what the daemon was started with.
    pub fn own_installation() -> Vec<(String, Option<OsString>)> {
        ["HOME", BRANDING.profile_env(), "XDG_STATE_HOME"]
            .into_iter()
            .map(|key| (key.to_string(), std::env::var_os(key)))
            .collect()
    }

    /// The whole environment of a detached `localify dev`: the shell's, with
    /// what selects this installation taken from [`LocalHost::installation`].
    fn detached_environment(&self) -> Vec<(OsString, OsString)> {
        let mut environment: Vec<(OsString, OsString)> = (self.environment)();
        for (key, value) in &self.installation {
            environment.retain(|(name, _)| name != key.as_str());
            if let Some(value) = value {
                environment.push((key.into(), value.clone()));
            }
        }
        environment.retain(|(name, _)| name != SERVICE_ORIGIN_ENV && name != "PWD");
        environment
    }

    fn home(&self) -> Option<PathBuf> {
        self.installation
            .iter()
            .find(|(key, _)| key == "HOME")
            .and_then(|(_, value)| value.clone())
            .map(PathBuf::from)
    }
}

impl Host for LocalHost {
    fn look(
        &self,
        wanted: &ServiceKey,
        profile: Option<&str>,
        legacy_id: Option<&str>,
    ) -> Result<Option<Look>> {
        let config = ConfigStore::new(self.config_path.clone()).load()?;
        let key = LaunchRevKey::load_or_create(&self.data_dir)?;
        let roots = (self.project_roots)();
        let Some(looked) = look_up(&config, &self.store(), &key, &roots, wanted)? else {
            return Ok(None);
        };
        let names_legacy = legacy_id.is_some_and(|legacy_id| {
            looked
                .record
                .as_ref()
                .is_some_and(|record| record.names_legacy(legacy_id))
        });
        Ok(Some(Look {
            profile_allowed: allowed_remotely(&looked.profiles, profile),
            names_legacy,
            facts: looked.facts,
            key: looked.key,
        }))
    }

    fn stop(&self, key: &ServiceKey, expected: &Expected) -> Result<(), Report> {
        let store = self.store();
        let record = store
            .get(key)
            .map_err(|error| unavailable(&error))?
            .ok_or_else(|| Report::refused("conflict", format!("{CHANGED} Nothing runs.")))?;
        match expected {
            Expected::Run(run_id) => {
                if record.run_id() != Some(run_id) || !store.is_running(&record) {
                    return Err(Report::refused(
                        "conflict",
                        format!("{CHANGED} Run {run_id} is no longer the one running."),
                    ));
                }
            }
            // Its identity again, right before it is signalled.
            Expected::Legacy(legacy_id) => {
                if !record.names_legacy(legacy_id) {
                    return Err(Report::refused(
                        "conflict",
                        format!("{CHANGED} It no longer runs legacy run {legacy_id}."),
                    ));
                }
                if record.legacy_id().as_deref() != Some(legacy_id) {
                    return Err(Report::refused("legacy_run", refusal_message("legacy_run")));
                }
            }
        }
        store.stop(&record).map(|_| ()).map_err(|error| {
            Report::new(
                ControlOutcome::Failed,
                None,
                format!("It could not be stopped: {error:#}"),
            )
        })
    }

    fn launch_in_app(&self, request: &LaunchRequest) -> Handoff {
        app_bridge::launch(&app_bridge::socket_path(&self.data_dir), request)
    }

    fn launch_detached(&self, launch: &DetachedLaunch) -> Result<Box<dyn Spawned>> {
        let executable = self.executable.clone().map_err(|error| anyhow!(error))?;
        let mut command = Command::new(executable);
        command
            .arg("--config")
            .arg(&self.config_path)
            .arg("dev")
            .arg(&launch.key.service_id)
            .arg("--project")
            .arg(&launch.key.project_id);
        if let Some(environment) = &launch.key.environment {
            command.arg("--env").arg(environment);
        }
        if let Some(profile) = &launch.profile {
            command.arg("--profile").arg(profile);
        }
        // Each id in one argument: a base64url id may start with `-`.
        command
            .arg(format!("--remote-launch={}", launch.launch_id))
            .arg(format!("--launch-expires={}", launch.expires_at))
            .arg(format!("--expect-rev={}", launch.expect_rev))
            .arg(format!(
                "--request={}:{}",
                launch.machine_id, launch.request_id
            ));
        if let Some(replaces) = &launch.replaces {
            command.arg(format!("--replace={replaces}"));
        }
        if let Some(legacy_id) = &launch.replaces_legacy {
            command.arg(format!("--replace-legacy={legacy_id}"));
        }
        // The executor itself, with what `dev --detach` would give it: the
        // background origin, its messages in the launch's log, and its own
        // session, whose leader is this process's child. Nothing in between,
        // so the daemon knows the process that holds the service's lock.
        let log_path = self.store().launch_log(&launch.launch_id)?;
        let log = File::options()
            .create(true)
            .append(true)
            .open(&log_path)
            .with_context(|| format!("Failed to open '{}'", log_path.display()))?;
        let home = self.home();
        command
            .env_clear()
            .envs(self.detached_environment())
            .env(SERVICE_ORIGIN_ENV, ServiceOrigin::Background.as_env())
            .current_dir(
                home.filter(|home| home.is_dir())
                    .unwrap_or_else(|| "/".into()),
            )
            .stdin(Stdio::null())
            .stdout(log.try_clone().context("Failed to share the launch log")?)
            .stderr(log);
        // SAFETY: the closure runs in the child between fork and exec and only
        // calls setsid, which is async-signal-safe and touches no memory.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command
            .spawn()
            .context("Failed to start localify dev in the background")?;
        Ok(Box::new(DetachedChild(child)))
    }

    fn report(&self, launch_id: &RunId, expires_at: Option<u64>) -> Result<LaunchReport> {
        reconcile(&self.store(), &NativeProcesses, launch_id, expires_at)
    }

    fn listens(&self, port: u16) -> bool {
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        TcpStream::connect_timeout(&address, Duration::from_millis(200)).is_ok()
    }

    fn last_lines(&self, run_id: &RunId, count: usize) -> String {
        output::tail_lines(&self.store().run_dir(run_id), count)
            .map(|lines| lines.join("\n"))
            .unwrap_or_default()
    }
}

/// The detached `localify dev` itself, the leader of its own session.
struct DetachedChild(Child);

impl Spawned for DetachedChild {
    fn settle(self: Box<Self>, admitted: bool) {
        let mut child = self.0;
        if !admitted {
            // Not reaped yet, so its PID still names its session: whatever
            // it started there goes with it, and so does its lock.
            kill_session(child.id());
            let _ = child.wait();
            return;
        }
        // Reaped once its run ends.
        thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

/// Kills every process of the session `sid` leads.
fn kill_session(sid: u32) {
    let _ = process::send_signal(sid, Signal::Kill);
    for member in process::all_processes() {
        if member.sid == Some(sid) && member.pid != sid {
            let _ = process::send_signal(member.pid, Signal::Kill);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::inventory::{
        EnvironmentRef, ProfileChecks, ProjectRef, ServiceRef as ServiceNames, TargetSettings,
    };
    use crate::infrastructure::remote::wire::{ControlExpect, ControlService, SessionMessage};

    const REV: &str = "0123456789abcdef";

    /// One service that runs nothing, an app that takes every launch, and
    /// the launch reports a test lines up: the last one repeats.
    struct Stub {
        reports: Mutex<VecDeque<LaunchReport>>,
        listens: bool,
        lines: String,
    }

    impl Stub {
        fn new(reports: Vec<LaunchReport>, listens: bool, lines: String) -> Arc<Self> {
            Arc::new(Self {
                reports: Mutex::new(reports.into()),
                listens,
                lines,
            })
        }
    }

    impl Host for Stub {
        fn look(
            &self,
            wanted: &ServiceKey,
            _: Option<&str>,
            _: Option<&str>,
        ) -> Result<Option<Look>> {
            let facts = ServiceFacts {
                project: ProjectRef {
                    id: "app".into(),
                    name: "App".into(),
                },
                environment: EnvironmentRef {
                    slug: None,
                    name: "Primary".into(),
                    primary: true,
                    branch: None,
                },
                service: ServiceNames {
                    id: "web".into(),
                    name: "web".into(),
                    config_rev: REV.into(),
                },
                local_url: "https://web.localify".into(),
                port: 4100,
                reachable: Some(false),
                run: None,
                public_url: None,
                profiles: ProfileChecks {
                    start: true,
                    restart: true,
                },
            };
            Ok(Some(Look {
                facts,
                key: wanted.clone(),
                profile_allowed: true,
                names_legacy: false,
            }))
        }

        fn stop(&self, _: &ServiceKey, _: &Expected) -> Result<(), Report> {
            Ok(())
        }

        fn launch_in_app(&self, _: &LaunchRequest) -> Handoff {
            Handoff::Accepted
        }

        fn launch_detached(&self, _: &DetachedLaunch) -> Result<Box<dyn Spawned>> {
            Err(anyhow!("No background starts here."))
        }

        fn report(&self, _: &RunId, _: Option<u64>) -> Result<LaunchReport> {
            let mut reports = lock(&self.reports);
            let report = if reports.len() > 1 {
                reports.pop_front()
            } else {
                reports.front().cloned()
            };
            Ok(report.unwrap_or(LaunchReport::Unknown))
        }

        fn listens(&self, _: u16) -> bool {
            self.listens
        }

        fn last_lines(&self, _: &RunId, _: usize) -> String {
            self.lines.clone()
        }
    }

    fn runner(host: Arc<Stub>, dir: &std::path::Path) -> ControlRunner {
        let timings = ControlTimings {
            dispatch_budget: Duration::from_millis(500),
            settle: Duration::from_millis(300),
            poll: Duration::from_millis(10),
            ahead: Duration::from_secs(60),
        };
        ControlRunner::new(host, RequestEntries::new(dir.to_path_buf()), timings)
    }

    fn request_id(n: usize) -> String {
        format!("{n:q>22}")
    }

    fn order(peer: &str, action: ControlAction, request: &str, expires_in: i64) -> Order {
        let expires = time::OffsetDateTime::now_utc() + time::Duration::seconds(expires_in);
        Order {
            peer: peer.into(),
            peer_name: peer.into(),
            control: Control {
                id: "c1".into(),
                action,
                service: ControlService {
                    project_id: "app".into(),
                    environment: None,
                    service_id: "web".into(),
                },
                profile: None,
                request_id: request.into(),
                expect: ControlExpect {
                    run_id: None,
                    config_rev: REV.into(),
                    legacy_id: None,
                },
                expires_at: expires
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap(),
            },
        }
    }

    fn start(runner: &ControlRunner, order: &Order) -> Report {
        let switches = || Switches {
            settings: TargetSettings {
                share: true,
                control: true,
                background_starts: false,
            },
            app: AppPresence::Running,
        };
        let paired = || true;
        let live = Live {
            switches: &switches,
            paired: &paired,
        };
        runner.run(order, &live, &mut |_| {})
    }

    fn exited(code: i32) -> LaunchReport {
        LaunchReport::Exited {
            code: Some(code),
            signal: None,
            reason: None,
        }
    }

    /// Launch A runs, then exits, while another process listens on its
    /// port: A's own record wins, and the control failed.
    #[test]
    fn a_launch_that_ended_fails_even_when_something_else_listens() {
        let dir = tempfile::tempdir().unwrap();
        let host = Stub::new(vec![LaunchReport::Running, exited(3)], true, String::new());
        let runner = runner(host, dir.path());

        let report = start(
            &runner,
            &order("mA", ControlAction::Start, &request_id(1), 30),
        );

        assert_eq!(report.outcome, ControlOutcome::Failed, "{report:?}");
        assert!(report.message.contains("exited with code 3"), "{report:?}");
    }

    /// 40 long lines, full of what JSON escapes, never make the result too
    /// big to send: the newest lines stay, and the cut is said.
    #[test]
    fn a_failure_s_output_fits_in_one_session_message() {
        let dir = tempfile::tempdir().unwrap();
        let line = format!("{}\"\u{1}", "x".repeat(1600));
        let lines: Vec<String> = (0..40).map(|n| format!("{n:02}{line}")).collect();
        let host = Stub::new(vec![exited(1)], false, lines.join("\n"));
        let runner = runner(host, dir.path());

        let report = start(
            &runner,
            &order("mA", ControlAction::Start, &request_id(2), 30),
        );

        let message =
            SessionMessage::ControlResult(report.answer("c1234567890123456789012345678901"));
        let size = serde_json::to_vec(&message).unwrap().len();
        assert!(size <= 32 * 1024, "{size} bytes");
        assert!(report.message.contains("[earlier output cut]"));
        assert!(report.message.ends_with(lines.last().unwrap().as_str()));
        assert!(report.message.starts_with("web exited with code 1."));
    }

    #[test]
    fn a_line_too_long_on_its_own_keeps_its_end() {
        let line = format!("start{}end", "\u{1}".repeat(10_000));
        let message = with_last_lines("Head.".into(), &line, 1024);

        assert!(escaped_len(&message) <= 1024, "{}", escaped_len(&message));
        assert!(message.ends_with("end"));
        assert!(message.contains("[earlier output cut]"));
    }

    /// A restart whose stop was recorded, left without its launch by a
    /// daemon restart, failed: only a stop is done once stopped.
    #[test]
    fn a_restart_interrupted_after_its_stop_failed() {
        let dir = tempfile::tempdir().unwrap();
        let runner = runner(Stub::new(vec![], false, String::new()), dir.path());
        let entries = RequestEntries::new(dir.path().to_path_buf());
        for (n, action) in [(3, ControlAction::Restart), (4, ControlAction::Stop)] {
            let order = order("mA", action, &request_id(n), 30);
            let entry = RequestEntry::new(Fingerprint::of(&order.control), 1);
            entries.publish("mA", &request_id(n), &entry).unwrap();
            entries
                .update("mA", &request_id(n), |entry| {
                    entry.stopped = Some(Stopped {
                        run_id: Some("RRRRRRRRRRRRRRRRRRRRRR".into()),
                        legacy_id: None,
                        at: 2,
                    });
                })
                .unwrap();
        }

        let restart = runner.status("mA", &request_id(3));
        assert_eq!(restart.outcome, ControlOutcome::Failed, "{restart:?}");
        assert!(restart.message.contains("stopped web"), "{restart:?}");
        let stop = runner.status("mA", &request_id(4));
        assert_eq!(stop.outcome, ControlOutcome::Done, "{stop:?}");
    }

    /// Fresh request ids cannot fill the disk: past the window's entries, a
    /// peer is refused before an entry is written; another peer is not.
    #[test]
    fn a_peer_creates_a_bounded_number_of_request_entries() {
        let dir = tempfile::tempdir().unwrap();
        let runner = runner(Stub::new(vec![], false, String::new()), dir.path());
        // Each one already expired: refused once its entry is published.
        for n in 0..ENTRIES_PER_WINDOW {
            let report = start(
                &runner,
                &order("mA", ControlAction::Start, &request_id(n), -1),
            );
            assert_eq!(report.code.as_deref(), Some("expired"), "{report:?}");
        }

        let refused = start(
            &runner,
            &order("mA", ControlAction::Start, &request_id(99), -1),
        );
        assert_eq!(refused.code.as_deref(), Some("too_many"), "{refused:?}");
        let kept = std::fs::read_dir(dir.path().join("mA")).unwrap().count();
        assert_eq!(kept, ENTRIES_PER_WINDOW);
        // Asking again about a recorded request makes no entry, and works.
        let again = start(
            &runner,
            &order("mA", ControlAction::Start, &request_id(0), -1),
        );
        assert_eq!(again.code.as_deref(), Some("expired"));
        let other = start(
            &runner,
            &order("mB", ControlAction::Start, &request_id(1), -1),
        );
        assert_eq!(other.code.as_deref(), Some("expired"));
    }
}
