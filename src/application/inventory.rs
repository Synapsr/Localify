//! This machine's inventory (docs/remote-machines.md, "Inventory"): a
//! replacement snapshot of its configured projects, environments and
//! services, never a stream of events. Each service carries four states kept
//! apart, because they disagree often and each one is shown: whether a process
//! of its run is alive, whether its port answers, whether its output can be
//! read, and whether its public URL serves. The projection here is pure: the
//! infrastructure gathers the facts, and nothing is sent anywhere yet.

use serde::Serialize;
use sha2::{Digest, Sha256};

/// The snapshot's shape. A change of meaning takes a new one.
pub const SCHEMA: u32 = 1;
/// Bytes of items per page: a page is at most 32 KiB, and the rest is room
/// for the machine summary and the message around them.
pub const PAGE_BUDGET: usize = 30 * 1024;
/// More items than fit in this many pages are left out, and the snapshot
/// says it is truncated.
pub const MAX_PAGES: usize = 32;

// --- Facts --------------------------------------------------------------

/// What the infrastructure observed about one service in one environment.
#[derive(Debug, Clone)]
pub struct ServiceFacts {
    pub project: ProjectRef,
    pub environment: EnvironmentRef,
    pub service: ServiceRef,
    pub local_url: String,
    pub port: u16,
    /// A TCP connect to the port on 127.0.0.1 succeeded.
    pub reachable: bool,
    /// The service's last recorded run.
    pub run: Option<RunFacts>,
    pub public_url: Option<PublicUrlFacts>,
    pub profiles: ProfileChecks,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectRef {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EnvironmentRef {
    /// `None` for the primary checkout.
    pub slug: Option<String>,
    pub name: String,
    pub primary: bool,
    pub branch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceRef {
    pub id: String,
    pub name: String,
    pub config_rev: String,
}

#[derive(Debug, Clone)]
pub struct RunFacts {
    /// `None` for a run recorded by a version without run ids.
    pub run_id: Option<String>,
    pub origin: RunOrigin,
    /// Started from a shell tab by an app from before run PTYs.
    pub legacy: bool,
    /// The run has an output stream (a run PTY or background pipes).
    pub has_stream: bool,
    pub process: RunProcess,
    /// RFC 3339.
    pub started_at: String,
    pub profile: Option<String>,
    pub profile_danger: bool,
    pub exit: Option<RunExit>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOrigin {
    App,
    Background,
    Terminal,
}

/// Whether a process proven to belong to the run is alive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunProcess {
    Running,
    Stopped,
    /// A legacy run whose PID Localify cannot prove is still the service.
    Unproven,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunExit {
    /// RFC 3339.
    pub at: String,
    pub code: Option<i32>,
}

/// The public URL that serves this service's domain.
#[derive(Debug, Clone)]
pub struct PublicUrlFacts {
    pub url: String,
    pub state: PublicUrlState,
    /// Localify Cloud assigns it to this machine.
    pub served_here: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicUrlState {
    Serving,
    Suspended,
    TakenDown,
}

/// Whether the profiles a control would launch with are allowed: resolved
/// and not dangerous (docs/remote-machines.md, "What a control may name").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfileChecks {
    /// The effective default, for a start.
    pub start: bool,
    /// The last run's own profile, for a restart.
    pub restart: bool,
}

/// This machine's switches for paired machines. All start off.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TargetSettings {
    pub share: bool,
    pub control: bool,
    pub background_starts: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppPresence {
    Running,
    Absent,
}

/// What holds for every service of the machine.
#[derive(Debug, Clone, Copy)]
pub struct Context {
    pub settings: TargetSettings,
    pub app: AppPresence,
    /// This machine's tunnel is connected.
    pub tunnel_connected: bool,
}

// --- Items --------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InventoryItem {
    pub project: ProjectRef,
    pub environment: EnvironmentRef,
    pub service: ServiceRef,
    /// Resolves on this machine only.
    pub local_url: String,
    /// `None`: nothing runs and the port is free.
    pub owner: Option<Owner>,
    pub run: Option<RunView>,
    pub process: ProcessState,
    pub port: PortState,
    pub output: OutputState,
    pub public_url: Option<PublicUrlView>,
    pub controls: Controls,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Owner {
    /// A run PTY of the desktop app.
    App,
    /// A detached executor.
    Background,
    /// A terminal Localify does not own.
    Terminal,
    /// A process listens on the port, and no running record explains it.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunView {
    pub run_id: Option<String>,
    pub legacy: bool,
    pub started_at: String,
    pub profile: Option<String>,
    pub profile_danger: bool,
    pub exit: Option<RunExit>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProcessState {
    Running,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PortState {
    pub number: u16,
    pub reachable: bool,
}

/// `{available: true, runId}` or `{available: false, reason}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputState {
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<OutputReason>,
}

impl OutputState {
    fn available(run_id: String) -> Self {
        Self {
            available: true,
            run_id: Some(run_id),
            reason: None,
        }
    }

    fn unavailable(reason: OutputReason) -> Self {
        Self {
            available: false,
            run_id: None,
            reason: Some(reason),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputReason {
    Terminal,
    UnknownOwner,
    NoRun,
    LegacyRun,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PublicUrlView {
    pub url: String,
    pub serving: bool,
    pub reason: Option<PublicUrlReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicUrlReason {
    Suspended,
    TakenDown,
    ServedElsewhere,
    TunnelDown,
}

/// What a paired machine may do right now, with the code a refusal would
/// carry, so a button is disabled with its reason before anyone clicks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Controls {
    pub start: Control,
    pub stop: Control,
    pub restart: Control,
}

impl Controls {
    fn all(control: Control) -> Self {
        Self {
            start: control,
            stop: control,
            restart: control,
        }
    }
}

/// `{allowed: true}` or `{allowed: false, code}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Control {
    pub allowed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<Refusal>,
}

impl Control {
    const ALLOWED: Self = Self {
        allowed: true,
        code: None,
    };

    fn refused(code: Refusal) -> Self {
        Self {
            allowed: false,
            code: Some(code),
        }
    }
}

/// The codes of docs/remote-machines.md, "Errors", that a control can be
/// refused with before it is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    ControlDisabled,
    BackgroundDisabled,
    NotOwned,
    LegacyRun,
    OwnerGone,
    RefusedProfile,
    Conflict,
}

/// One service's item. Each state is computed from its own source only.
pub fn inventory_item(facts: ServiceFacts, context: &Context) -> InventoryItem {
    let process = process_state(facts.run.as_ref());
    let owner = owner(facts.run.as_ref(), facts.reachable);
    let controls = controls(&facts, owner, context);
    InventoryItem {
        output: output_state(facts.run.as_ref(), owner),
        public_url: facts
            .public_url
            .map(|url| public_url_view(url, context.tunnel_connected)),
        controls,
        run: facts.run.map(|run| RunView {
            run_id: run.run_id,
            legacy: run.legacy,
            started_at: run.started_at,
            profile: run.profile,
            profile_danger: run.profile_danger,
            exit: run.exit,
        }),
        project: facts.project,
        environment: facts.environment,
        service: facts.service,
        local_url: facts.local_url,
        owner,
        process,
        port: PortState {
            number: facts.port,
            reachable: facts.reachable,
        },
    }
}

/// From the run's session only: an unproven legacy PID is not a process.
fn process_state(run: Option<&RunFacts>) -> ProcessState {
    match run.map(|run| run.process) {
        Some(RunProcess::Running) => ProcessState::Running,
        _ => ProcessState::Stopped,
    }
}

/// Who holds the service: the running run's origin, or `Unknown` when the
/// port answers and no running record explains it.
fn owner(run: Option<&RunFacts>, reachable: bool) -> Option<Owner> {
    match run {
        Some(run) if run.process == RunProcess::Running => Some(match run.origin {
            RunOrigin::App => Owner::App,
            RunOrigin::Background => Owner::Background,
            RunOrigin::Terminal => Owner::Terminal,
        }),
        _ if reachable => Some(Owner::Unknown),
        _ => None,
    }
}

/// From the run's stream only. A legacy run never exports anything, not even
/// its whole-shell file.
fn output_state(run: Option<&RunFacts>, owner: Option<Owner>) -> OutputState {
    let Some(run) = run else {
        return OutputState::unavailable(match owner {
            Some(Owner::Unknown) => OutputReason::UnknownOwner,
            _ => OutputReason::NoRun,
        });
    };
    if run.legacy {
        return OutputState::unavailable(OutputReason::LegacyRun);
    }
    if owner == Some(Owner::Unknown) {
        return OutputState::unavailable(OutputReason::UnknownOwner);
    }
    match (&run.run_id, run.has_stream) {
        (Some(run_id), true) => OutputState::available(run_id.clone()),
        _ => OutputState::unavailable(OutputReason::Terminal),
    }
}

/// From the URL's state, the machine that serves it, and this machine's
/// tunnel; never from the service's own port.
fn public_url_view(url: PublicUrlFacts, tunnel_connected: bool) -> PublicUrlView {
    let reason = match url.state {
        PublicUrlState::TakenDown => Some(PublicUrlReason::TakenDown),
        PublicUrlState::Suspended => Some(PublicUrlReason::Suspended),
        PublicUrlState::Serving if !url.served_here => Some(PublicUrlReason::ServedElsewhere),
        PublicUrlState::Serving if !tunnel_connected => Some(PublicUrlReason::TunnelDown),
        PublicUrlState::Serving => None,
    };
    PublicUrlView {
        url: url.url,
        serving: reason.is_none(),
        reason,
    }
}

/// docs/remote-machines.md, "Who acts": every control goes through whoever
/// owns the service.
fn controls(facts: &ServiceFacts, owner: Option<Owner>, context: &Context) -> Controls {
    let settings = context.settings;
    if !(settings.share && settings.control) {
        return Controls::all(Control::refused(Refusal::ControlDisabled));
    }
    let legacy_alive = facts
        .run
        .as_ref()
        .is_some_and(|run| run.legacy && run.process != RunProcess::Stopped);
    if legacy_alive {
        return Controls::all(Control::refused(Refusal::LegacyRun));
    }
    let conflict = Control::refused(Refusal::Conflict);
    match owner {
        Some(Owner::Terminal | Owner::Unknown) => {
            Controls::all(Control::refused(Refusal::NotOwned))
        }
        None => Controls {
            start: start_control(facts.profiles, context),
            stop: conflict,
            restart: conflict,
        },
        Some(owner) => Controls {
            start: conflict,
            stop: Control::ALLOWED,
            restart: restart_control(owner, facts.profiles, context),
        },
    }
}

fn start_control(profiles: ProfileChecks, context: &Context) -> Control {
    if !profiles.start {
        Control::refused(Refusal::RefusedProfile)
    } else if context.app == AppPresence::Running || context.settings.background_starts {
        Control::ALLOWED
    } else {
        Control::refused(Refusal::BackgroundDisabled)
    }
}

/// Every reason to refuse the replacement is decided before anything stops.
fn restart_control(owner: Owner, profiles: ProfileChecks, context: &Context) -> Control {
    let owner_refusal = match owner {
        Owner::App if context.app == AppPresence::Absent => Some(Refusal::OwnerGone),
        Owner::Background if !context.settings.background_starts => {
            Some(Refusal::BackgroundDisabled)
        }
        _ => None,
    };
    match owner_refusal {
        Some(code) => Control::refused(code),
        None if !profiles.restart => Control::refused(Refusal::RefusedProfile),
        None => Control::ALLOWED,
    }
}

// --- Snapshot -------------------------------------------------------------

/// A replacement snapshot: every item, sorted, as far as [`MAX_PAGES`] pages
/// of [`PAGE_BUDGET`] bytes hold them.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub schema: u32,
    pub epoch: String,
    pub rev: u64,
    /// How many pages the items take on the wire, at least one.
    pub pages: usize,
    /// More items than the pages hold were left out.
    pub truncated: bool,
    pub items: Vec<InventoryItem>,
}

/// The revision a viewer already holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct Have {
    pub epoch: String,
    pub rev: u64,
}

impl Snapshot {
    /// Whether a viewer holding `have` already has this snapshot.
    pub fn is_held_by(&self, have: Option<&Have>) -> bool {
        have.is_some_and(|have| have.epoch == self.epoch && have.rev == self.rev)
    }
}

/// Numbers snapshots: `epoch` is chosen once per daemon, and `rev` starts at
/// 1 and goes up only when the content changes, so an unchanged snapshot is
/// recognized and never sent again.
#[derive(Debug)]
pub struct Revisions {
    epoch: String,
    rev: u64,
    last: Option<[u8; 32]>,
}

impl Revisions {
    pub fn new(epoch: String) -> Self {
        Self {
            epoch,
            rev: 0,
            last: None,
        }
    }

    /// Sorts and bounds `items`, and numbers the snapshot they make.
    pub fn snapshot(&mut self, mut items: Vec<InventoryItem>) -> anyhow::Result<Snapshot> {
        items.sort_by(|left, right| sort_key(left).cmp(&sort_key(right)));
        let (items, pages, truncated) = bound(items)?;
        let digest: [u8; 32] = Sha256::digest(serde_json::to_vec(&(&items, truncated))?).into();
        if self.last != Some(digest) {
            self.rev += 1;
            self.last = Some(digest);
        }
        Ok(Snapshot {
            schema: SCHEMA,
            epoch: self.epoch.clone(),
            rev: self.rev,
            pages,
            truncated,
            items,
        })
    }
}

/// Project name, environment (primary first), then service.
fn sort_key(item: &InventoryItem) -> (&str, &str, bool, Option<&str>, &str, &str) {
    (
        &item.project.name,
        &item.project.id,
        !item.environment.primary,
        item.environment.slug.as_deref(),
        &item.service.name,
        &item.service.id,
    )
}

/// Fills pages in order, each with at most [`PAGE_BUDGET`] bytes of items.
/// An item too large for a page of its own, and every item past
/// [`MAX_PAGES`], is left out. Returns what is kept, the page count, and
/// whether anything was left out.
fn bound(items: Vec<InventoryItem>) -> anyhow::Result<(Vec<InventoryItem>, usize, bool)> {
    let mut kept = Vec::new();
    let mut pages = 0;
    let mut used = PAGE_BUDGET;
    let mut truncated = false;
    for item in items {
        // Its separator in the page's array included.
        let size = serde_json::to_vec(&item)?.len() + 1;
        if size > PAGE_BUDGET {
            truncated = true;
            continue;
        }
        if used + size > PAGE_BUDGET {
            if pages == MAX_PAGES {
                truncated = true;
                break;
            }
            pages += 1;
            used = 0;
        }
        used += size;
        kept.push(item);
    }
    // An empty snapshot is still one page.
    Ok((kept, pages.max(1), truncated))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN_ID: &str = "AAAAAAAAAAAAAAAAAAAAAA";

    fn facts() -> ServiceFacts {
        ServiceFacts {
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
            service: ServiceRef {
                id: "web".into(),
                name: "web".into(),
                config_rev: "0123456789abcdef".into(),
            },
            local_url: "https://web.localify".into(),
            port: 3000,
            reachable: false,
            run: None,
            public_url: None,
            profiles: ProfileChecks {
                start: true,
                restart: true,
            },
        }
    }

    fn run(origin: RunOrigin, process: RunProcess) -> RunFacts {
        RunFacts {
            run_id: Some(RUN_ID.into()),
            origin,
            legacy: false,
            has_stream: origin != RunOrigin::Terminal,
            process,
            started_at: "2026-11-08T10:00:00Z".into(),
            profile: None,
            profile_danger: false,
            exit: None,
        }
    }

    fn legacy(process: RunProcess) -> RunFacts {
        RunFacts {
            run_id: None,
            legacy: true,
            has_stream: false,
            ..run(RunOrigin::App, process)
        }
    }

    fn with(run: Option<RunFacts>, reachable: bool) -> ServiceFacts {
        ServiceFacts {
            run,
            reachable,
            ..facts()
        }
    }

    /// Everything on, the app running.
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

    fn item(facts: ServiceFacts) -> InventoryItem {
        inventory_item(facts, &open())
    }

    fn refused(code: Refusal) -> Control {
        Control::refused(code)
    }

    #[test]
    fn running_but_not_reachable_yet_is_starting_with_its_owner() {
        let item = item(with(
            Some(run(RunOrigin::Background, RunProcess::Running)),
            false,
        ));

        assert_eq!(item.process, ProcessState::Running);
        assert!(!item.port.reachable);
        assert_eq!(item.owner, Some(Owner::Background));
        assert_eq!(item.output, OutputState::available(RUN_ID.into()));
    }

    #[test]
    fn reachable_with_no_record_is_unknown_and_has_no_output() {
        let item = item(with(None, true));

        assert_eq!(item.process, ProcessState::Stopped);
        assert_eq!(item.owner, Some(Owner::Unknown));
        assert_eq!(
            item.output,
            OutputState::unavailable(OutputReason::UnknownOwner)
        );
        assert_eq!(item.controls, Controls::all(refused(Refusal::NotOwned)));
    }

    #[test]
    fn reachable_after_the_run_stopped_is_unknown_too() {
        let item = item(with(Some(run(RunOrigin::App, RunProcess::Stopped)), true));

        assert_eq!(item.process, ProcessState::Stopped);
        assert_eq!(item.owner, Some(Owner::Unknown));
        assert_eq!(
            item.output,
            OutputState::unavailable(OutputReason::UnknownOwner)
        );
    }

    #[test]
    fn nothing_running_and_a_free_port_has_no_owner() {
        let item = item(with(None, false));

        assert_eq!(item.owner, None);
        assert_eq!(item.run, None);
        assert_eq!(item.output, OutputState::unavailable(OutputReason::NoRun));
        assert_eq!(item.controls.start, Control::ALLOWED);
        assert_eq!(item.controls.stop, refused(Refusal::Conflict));
    }

    #[test]
    fn a_stopped_run_keeps_its_output_and_exit() {
        let mut stopped = run(RunOrigin::Background, RunProcess::Stopped);
        stopped.exit = Some(RunExit {
            at: "2026-11-08T10:05:00Z".into(),
            code: Some(1),
        });

        let item = item(with(Some(stopped), false));

        assert_eq!(item.owner, None);
        assert_eq!(item.process, ProcessState::Stopped);
        assert_eq!(item.output, OutputState::available(RUN_ID.into()));
        assert_eq!(item.run.unwrap().exit.unwrap().code, Some(1));
    }

    #[test]
    fn a_terminal_run_shows_its_state_but_no_output_or_controls() {
        let item = item(with(
            Some(run(RunOrigin::Terminal, RunProcess::Running)),
            true,
        ));

        assert_eq!(item.owner, Some(Owner::Terminal));
        assert_eq!(item.process, ProcessState::Running);
        assert_eq!(
            item.output,
            OutputState::unavailable(OutputReason::Terminal)
        );
        assert_eq!(item.controls, Controls::all(refused(Refusal::NotOwned)));
    }

    #[test]
    fn legacy_runs_show_as_legacy_with_no_output() {
        for (process, reachable) in [
            (RunProcess::Running, true),
            (RunProcess::Unproven, true),
            (RunProcess::Unproven, false),
            (RunProcess::Stopped, false),
        ] {
            let item = item(with(Some(legacy(process)), reachable));
            let view = item.run.clone().unwrap();

            assert!(view.legacy && view.run_id.is_none());
            assert_eq!(
                item.output,
                OutputState::unavailable(OutputReason::LegacyRun),
                "{process:?}"
            );
        }
    }

    #[test]
    fn a_live_or_unproven_legacy_run_refuses_every_control() {
        for process in [RunProcess::Running, RunProcess::Unproven] {
            let item = item(with(Some(legacy(process)), true));

            assert_eq!(item.controls, Controls::all(refused(Refusal::LegacyRun)));
        }
        let unproven = item(with(Some(legacy(RunProcess::Unproven)), true));
        assert_eq!(unproven.process, ProcessState::Stopped);
        assert_eq!(unproven.owner, Some(Owner::Unknown));
    }

    #[test]
    fn a_legacy_run_that_ended_can_be_started_again() {
        let item = item(with(Some(legacy(RunProcess::Stopped)), false));

        assert_eq!(item.owner, None);
        assert_eq!(item.controls.start, Control::ALLOWED);
    }

    #[test]
    fn controls_are_off_until_both_share_and_control_are_on() {
        for settings in [
            TargetSettings::default(),
            TargetSettings {
                share: true,
                ..TargetSettings::default()
            },
        ] {
            let context = Context { settings, ..open() };
            let item = inventory_item(with(None, false), &context);

            assert_eq!(
                item.controls,
                Controls::all(refused(Refusal::ControlDisabled))
            );
        }
    }

    #[test]
    fn a_start_without_the_app_needs_background_starts() {
        let mut context = Context {
            app: AppPresence::Absent,
            ..open()
        };
        context.settings.background_starts = false;
        assert_eq!(
            inventory_item(with(None, false), &context).controls.start,
            refused(Refusal::BackgroundDisabled)
        );

        context.settings.background_starts = true;
        assert_eq!(
            inventory_item(with(None, false), &context).controls.start,
            Control::ALLOWED
        );
    }

    #[test]
    fn a_running_service_cannot_be_started_again() {
        let item = item(with(Some(run(RunOrigin::App, RunProcess::Running)), true));

        assert_eq!(item.controls.start, refused(Refusal::Conflict));
        assert_eq!(item.controls.stop, Control::ALLOWED);
        assert_eq!(item.controls.restart, Control::ALLOWED);
    }

    #[test]
    fn an_app_run_that_outlived_its_app_stops_but_does_not_restart() {
        let context = Context {
            app: AppPresence::Absent,
            ..open()
        };
        let item = inventory_item(
            with(Some(run(RunOrigin::App, RunProcess::Running)), true),
            &context,
        );

        assert_eq!(item.controls.stop, Control::ALLOWED);
        assert_eq!(item.controls.restart, refused(Refusal::OwnerGone));
    }

    #[test]
    fn a_background_restart_needs_background_starts() {
        let mut context = open();
        context.settings.background_starts = false;
        let item = inventory_item(
            with(Some(run(RunOrigin::Background, RunProcess::Running)), true),
            &context,
        );

        assert_eq!(item.controls.stop, Control::ALLOWED);
        assert_eq!(item.controls.restart, refused(Refusal::BackgroundDisabled));
    }

    #[test]
    fn a_profile_no_longer_allowed_refuses_the_launch_it_would_make() {
        let mut stopped = with(None, false);
        stopped.profiles.start = false;
        assert_eq!(
            item(stopped).controls.start,
            refused(Refusal::RefusedProfile)
        );

        let mut running = with(Some(run(RunOrigin::App, RunProcess::Running)), true);
        running.profiles.restart = false;
        let item = item(running);
        assert_eq!(item.controls.restart, refused(Refusal::RefusedProfile));
        assert_eq!(item.controls.stop, Control::ALLOWED);
    }

    #[test]
    fn a_public_url_serves_only_when_its_state_machine_and_tunnel_allow() {
        let url = |state, served_here| PublicUrlFacts {
            url: "https://app.localify.fyi".into(),
            state,
            served_here,
        };
        let cases = [
            (url(PublicUrlState::Serving, true), true, None),
            (
                url(PublicUrlState::Serving, true),
                false,
                Some(PublicUrlReason::TunnelDown),
            ),
            (
                url(PublicUrlState::Serving, false),
                true,
                Some(PublicUrlReason::ServedElsewhere),
            ),
            (
                url(PublicUrlState::Suspended, true),
                true,
                Some(PublicUrlReason::Suspended),
            ),
            (
                url(PublicUrlState::TakenDown, true),
                true,
                Some(PublicUrlReason::TakenDown),
            ),
        ];
        for (url, tunnel_connected, reason) in cases {
            let view = public_url_view(url, tunnel_connected);
            assert_eq!(view.reason, reason);
            assert_eq!(view.serving, reason.is_none());
        }
    }

    #[test]
    fn the_four_states_move_independently() {
        // The service's port answering changes nothing but the port and who
        // holds it; the URL serves whatever the process does.
        let mut serving = with(Some(run(RunOrigin::App, RunProcess::Running)), false);
        serving.public_url = Some(PublicUrlFacts {
            url: "https://app.localify.fyi".into(),
            state: PublicUrlState::Serving,
            served_here: true,
        });
        let quiet = item(serving.clone());
        let listening = item(ServiceFacts {
            reachable: true,
            ..serving
        });

        assert_eq!(quiet.process, listening.process);
        assert_eq!(quiet.output, listening.output);
        assert_eq!(quiet.public_url, listening.public_url);
        assert!(quiet.public_url.unwrap().serving);
        assert_ne!(quiet.port, listening.port);
    }

    #[test]
    fn the_item_reads_as_the_contract_writes_it() {
        let json = serde_json::to_value(item(with(None, true))).unwrap();

        assert_eq!(json["owner"], "unknown");
        assert_eq!(json["process"], "stopped");
        assert_eq!(json["service"]["configRev"], "0123456789abcdef");
        assert_eq!(json["environment"]["slug"], serde_json::Value::Null);
        assert_eq!(
            json["output"],
            serde_json::json!({"available": false, "reason": "unknown_owner"})
        );
        assert_eq!(
            json["controls"]["start"],
            serde_json::json!({"allowed": false, "code": "not_owned"})
        );
        assert_eq!(json["localUrl"], "https://web.localify");
        assert_eq!(json["publicUrl"], serde_json::Value::Null);
    }

    #[test]
    fn an_unchanged_snapshot_keeps_its_revision() {
        let mut revisions = Revisions::new("epoch".into());
        let first = revisions.snapshot(vec![item(with(None, false))]).unwrap();
        let again = revisions.snapshot(vec![item(with(None, false))]).unwrap();
        let changed = revisions.snapshot(vec![item(with(None, true))]).unwrap();

        assert_eq!((first.rev, again.rev, changed.rev), (1, 1, 2));
        let held = Have {
            epoch: "epoch".into(),
            rev: 1,
        };
        assert!(again.is_held_by(Some(&held)));
        assert!(!changed.is_held_by(Some(&held)));
        assert!(!changed.is_held_by(None));
        let other_epoch = Have {
            epoch: "another".into(),
            rev: 2,
        };
        assert!(!changed.is_held_by(Some(&other_epoch)));
    }

    #[test]
    fn items_are_sorted_by_project_environment_and_service() {
        let named = |project: &str, slug: Option<&str>, service: &str| {
            let mut facts = facts();
            facts.project.name = project.into();
            facts.environment.primary = slug.is_none();
            facts.environment.slug = slug.map(str::to_string);
            facts.service.name = service.into();
            item(facts)
        };
        let mut revisions = Revisions::new("epoch".into());

        let snapshot = revisions
            .snapshot(vec![
                named("b", None, "web"),
                named("a", Some("feature"), "api"),
                named("a", None, "web"),
                named("a", None, "api"),
            ])
            .unwrap();

        let order: Vec<_> = snapshot
            .items
            .iter()
            .map(|item| {
                (
                    item.project.name.as_str(),
                    item.environment.slug.as_deref(),
                    item.service.name.as_str(),
                )
            })
            .collect();
        assert_eq!(
            order,
            [
                ("a", None, "api"),
                ("a", None, "web"),
                ("a", Some("feature"), "api"),
                ("b", None, "web"),
            ]
        );
    }

    #[test]
    fn a_snapshot_is_bounded_to_32_pages_of_items() {
        // Every item the same size.
        let numbered = |index: usize| {
            let mut facts = facts();
            facts.service.id = format!("{index:06}");
            facts.service.name = format!("{index:06}");
            item(facts)
        };
        let size = serde_json::to_vec(&numbered(0)).unwrap().len() + 1;
        let per_page = PAGE_BUDGET / size;
        let items: Vec<_> = (0..per_page * MAX_PAGES + 5).map(numbered).collect();
        let mut revisions = Revisions::new("epoch".into());

        let snapshot = revisions.snapshot(items).unwrap();

        assert!(snapshot.truncated);
        assert_eq!(snapshot.pages, MAX_PAGES);
        assert_eq!(snapshot.items.len(), per_page * MAX_PAGES);
    }

    #[test]
    fn an_item_too_large_for_a_page_is_left_out() {
        let mut huge = facts();
        huge.project.name = "x".repeat(PAGE_BUDGET);
        let mut revisions = Revisions::new("epoch".into());

        let snapshot = revisions.snapshot(vec![item(huge), item(facts())]).unwrap();

        assert!(snapshot.truncated);
        assert_eq!(snapshot.items.len(), 1);
        assert_eq!(snapshot.pages, 1);
    }
}
