//! `dev`, `stop` and `restart` with `--machine`: a control that the target
//! runs through whoever owns the service there (docs/remote-machines.md,
//! "Controls"). The daemon sends a control only over a session already
//! open, so a watch holds one open, and its first snapshot is the state the
//! control expects. A control prints its progress, then its result; an
//! outcome that could not be learned prints the fresh state and exits with
//! [`LOST`].

use std::io::{ErrorKind, Write};
use std::time::Duration;

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::inventory::{Item, ServiceQuery, Snapshot, fetch, find, write_item};
use super::{Console, LOST, Peer, REFUSED, Refusal, remote_error, viewer_message};
use crate::application::ports::DaemonConnectionError;
use crate::infrastructure::mgmt_client::{MgmtSocketClient, MgmtStream};
use crate::infrastructure::remote::wire::{
    AppPresence, ControlAction, ControlOutcome, ControlPhase,
};

/// How long the first snapshot may take: a lease, a session and every page.
const SNAPSHOT_WAIT: Duration = Duration::from_secs(60);
/// The exit code of a control that ran and failed.
const FAILED: i32 = 1;

/// Starts, stops or restarts `query`'s service on `peer`. `profile` is for a
/// start only; a restart keeps the run's profile.
pub fn control<O: Write, E: Write>(
    client: &MgmtSocketClient,
    peer: &Peer,
    action: ControlAction,
    query: &ServiceQuery<'_>,
    profile: Option<&str>,
    console: &mut Console<O, E>,
) -> Result<i32> {
    let mut watch = client
        .remote_stream(&json!({ "cmd": "remote_watch", "machineId": peer.id }))
        .map_err(|error| remote_error(error, peer))?;
    let snapshot = first_snapshot(&mut watch, peer)?;
    let items = snapshot.shared_items(peer)?;
    let (item, _) = find(&items, query, &peer.name)?;
    allowed(item, action, &peer.name)?;
    let asked = Asked {
        peer,
        action,
        service: &item.service.name,
        ids: json!({
            "projectId": item.project.id,
            "environment": item.environment.slug,
            "serviceId": item.service.id,
        }),
        app_running: snapshot.machine.app == AppPresence::Running,
    };
    let request = json!({
        "cmd": "remote_control",
        "machineId": peer.id,
        "action": action,
        "service": asked.ids,
        "profile": profile.filter(|_| action == ControlAction::Start),
        "expect": {
            "runId": item.run.as_ref().and_then(|run| run.run_id.clone()),
            "configRev": item.service.config_rev,
        },
    });
    let mut stream = client
        .remote_stream(&request)
        .map_err(|error| remote_error(error, peer))?;
    let events = std::iter::from_fn(|| stream.next_answer().transpose());
    let ended = follow(events, &asked, console)?;
    drop(watch);
    report(ended, &asked, console, || fresh(client, peer, query))
}

/// What a control was asked for, to say what happens to it.
struct Asked<'a> {
    peer: &'a Peer,
    action: ControlAction,
    service: &'a str,
    ids: Value,
    app_running: bool,
}

/// The watch's first snapshot, or why the target can't be watched.
fn first_snapshot(watch: &mut MgmtStream, peer: &Peer) -> Result<Snapshot> {
    let not_responding = || anyhow!(Refusal::from_target("not_responding", "", &peer.name));
    watch.set_timeout(Some(SNAPSHOT_WAIT))?;
    loop {
        let event = match watch.next_answer() {
            Ok(Some(event)) => event,
            Ok(None) => return Err(not_responding()),
            Err(DaemonConnectionError::ConnectionFailed(error)) if timed_out(&error) => {
                return Err(not_responding());
            }
            Err(error) => return Err(remote_error(error, peer)),
        };
        match event["event"].as_str() {
            Some("snapshot") => return Snapshot::parse(event),
            Some("lost") => {
                return Err(anyhow!(Refusal::from_target(
                    event["code"].as_str().unwrap_or_default(),
                    event["message"].as_str().unwrap_or_default(),
                    &peer.name,
                )));
            }
            _ => continue,
        }
    }
}

fn timed_out(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<std::io::Error>()
        .is_some_and(|error| matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut))
}

/// Refuses before sending what the snapshot already refuses: the same
/// checks the target makes, with the code it would give.
fn allowed(item: &Item, action: ControlAction, machine: &str) -> Result<()> {
    let service = &item.service.name;
    // A live legacy run is relaunched from the app, after a confirmation. One
    // that ended is only a record: the target's `controls` say what it allows.
    let live_legacy = item.process == "running" && item.run.as_ref().is_some_and(|run| run.legacy);
    let code = match item.controls.get(action.as_str()) {
        Some(state) if state.allowed && live_legacy => "legacy_run",
        Some(state) if !state.allowed => state.code.as_deref().unwrap_or("conflict"),
        _ => return Ok(()),
    };
    let refusal = match (code, action) {
        ("conflict", ControlAction::Start) => Refusal::new(
            "conflict",
            format!("'{service}' already runs on {machine}."),
        ),
        ("conflict", _) => Refusal::new(
            "conflict",
            format!("'{service}' isn't running on {machine}."),
        ),
        (code, _) => Refusal::from_target(code, "", machine),
    };
    Err(anyhow!(refusal))
}

/// One event of `remote_control`.
#[derive(Debug, Deserialize)]
#[serde(
    tag = "event",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
enum Event {
    Sent,
    Progress {
        phase: ControlPhase,
    },
    Result {
        outcome: ControlOutcome,
        code: Option<String>,
        message: String,
        run_id: Option<String>,
    },
    Unknown,
    Lost {
        code: String,
    },
}

/// How a control ended, for this CLI.
#[derive(Debug, PartialEq, Eq)]
enum Ended {
    Result {
        outcome: ControlOutcome,
        code: Option<String>,
        message: String,
        run_id: Option<String>,
    },
    Lost {
        code: String,
    },
}

/// Prints the progress of the control until its result or `lost`.
fn follow<O: Write, E: Write>(
    events: impl Iterator<Item = Result<Value, DaemonConnectionError>>,
    asked: &Asked<'_>,
    console: &mut Console<O, E>,
) -> Result<Ended> {
    // A stop says the same thing when sent and when dispatched.
    let mut said = String::new();
    for event in events {
        let event = event.map_err(|error| remote_error(error, asked.peer))?;
        let event: Event = serde_json::from_value(event).map_err(|error| {
            anyhow!("The daemon sent a control event this version cannot read: {error}")
        })?;
        let line = match event {
            Event::Sent => progress(asked, None),
            Event::Progress { phase } => progress(asked, Some(phase)),
            Event::Unknown => format!("Outcome unknown. Checking {}…", asked.peer.name),
            Event::Result {
                outcome,
                code,
                message,
                run_id,
            } => {
                return Ok(Ended::Result {
                    outcome,
                    code,
                    message,
                    run_id,
                });
            }
            Event::Lost { code } => return Ok(Ended::Lost { code }),
        };
        if line != said {
            console.note(&line)?;
            said = line;
        }
    }
    // The daemon closed the stream without an outcome.
    Ok(Ended::Lost {
        code: "closed".to_string(),
    })
}

/// What a control is doing, as the app's chip says it.
fn progress(asked: &Asked<'_>, phase: Option<ControlPhase>) -> String {
    let machine = &asked.peer.name;
    match (asked.action, phase) {
        (ControlAction::Stop, _) => format!("Stopping '{}' on {machine}…", asked.service),
        (_, Some(ControlPhase::Started)) => format!("Started on {machine}. Waiting for its port…"),
        (_, Some(ControlPhase::Dispatched)) if asked.app_running => {
            format!("Starting in the app on {machine}…")
        }
        (_, Some(ControlPhase::Dispatched)) => format!("Starting in the background on {machine}…"),
        (ControlAction::Restart, None) => format!("Restarting '{}' on {machine}…", asked.service),
        (ControlAction::Start, None) => format!("Starting '{}' on {machine}…", asked.service),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Outcome<'a> {
    outcome: &'a str,
    action: ControlAction,
    machine: &'a Peer,
    service: &'a Value,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    run_id: Option<String>,
    /// For `lost`: the service as the target shows it now.
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<Value>,
}

/// Prints the result and gives the exit code: 0 once done or started, 1
/// when the run failed, a refusal for [`super::REFUSED`], [`LOST`] when the
/// outcome could not be learned. `fresh` reads the service's state again.
fn report<O: Write, E: Write>(
    ended: Ended,
    asked: &Asked<'_>,
    console: &mut Console<O, E>,
    fresh: impl FnOnce() -> Result<(Item, Value)>,
) -> Result<i32> {
    let machine = &asked.peer.name;
    let (outcome, code, message, run_id) = match ended {
        Ended::Lost { code } => return lost(&code, asked, console, fresh),
        Ended::Result {
            outcome: ControlOutcome::Unknown,
            ..
        } => return lost("unknown", asked, console, fresh),
        Ended::Result {
            outcome: outcome @ (ControlOutcome::Refused | ControlOutcome::Conflict),
            code,
            message,
            ..
        } => {
            let code = code.unwrap_or_else(|| outcome_name(outcome).to_string());
            let refusal = Refusal::from_target(&code, &message, machine);
            if code == "conflict" {
                return conflict(refusal, machine, console, fresh);
            }
            return Err(anyhow!(refusal));
        }
        Ended::Result {
            outcome,
            code,
            message,
            run_id,
        } => (outcome, code, message, run_id),
    };
    let text = match outcome {
        ControlOutcome::Done if asked.action == ControlAction::Stop => {
            format!("'{}' stopped on {machine}.", asked.service)
        }
        ControlOutcome::Done => format!("'{}' is running on {machine}.", asked.service),
        ControlOutcome::Starting => format!("Started on {machine}, not listening yet."),
        _ if message.is_empty() => format!("'{}' failed to start on {machine}.", asked.service),
        _ => message,
    };
    let exit = if outcome == ControlOutcome::Failed {
        FAILED
    } else {
        0
    };
    if console.json {
        console.json(&Outcome {
            outcome: outcome_name(outcome),
            action: asked.action,
            machine: asked.peer,
            service: &asked.ids,
            message: text,
            code,
            run_id,
            status: None,
        })?;
    } else if exit == 0 {
        writeln!(console.out, "{text}")?;
    } else {
        writeln!(console.err, "{text}")?;
    }
    Ok(exit)
}

/// `conflict`: the target changed since the snapshot the control expected.
/// Says so with the service as it is now, and exits with [`REFUSED`]. The
/// control is never sent again.
fn conflict<O: Write, E: Write>(
    refusal: Refusal,
    machine: &str,
    console: &mut Console<O, E>,
    fresh: impl FnOnce() -> Result<(Item, Value)>,
) -> Result<i32> {
    let now = fresh();
    if console.json {
        let mut document = serde_json::to_value(&refusal)?;
        document["outcome"] = json!("refused");
        if let Ok((_, raw)) = &now {
            document["status"] = raw.clone();
        }
        console.json(&document)?;
        return Ok(REFUSED);
    }
    refusal.print(console)?;
    match now {
        Ok((item, _)) => write_item(&mut console.out, &item, machine)?,
        Err(error) => writeln!(
            console.err,
            "Its current state could not be read: {error:#}"
        )?,
    }
    Ok(REFUSED)
}

/// An outcome the target could not tell: says so, then shows the service
/// as it is now, and exits with [`LOST`]. The control is never sent again.
fn lost<O: Write, E: Write>(
    code: &str,
    asked: &Asked<'_>,
    console: &mut Console<O, E>,
    fresh: impl FnOnce() -> Result<(Item, Value)>,
) -> Result<i32> {
    let machine = &asked.peer.name;
    let message = match code {
        "not_shared" => format!("{machine} stopped sharing before it said what happened."),
        "not_paired" | "key_mismatch" => format!(
            "{machine} didn't say what happened. {}",
            viewer_message(code, machine, "")
        ),
        _ => format!("{machine} didn't say what happened. Its current state is below."),
    };
    let now = fresh();
    if console.json {
        console.json(&Outcome {
            outcome: "lost",
            action: asked.action,
            machine: asked.peer,
            service: &asked.ids,
            message,
            code: Some(code.to_string()),
            run_id: None,
            status: now.ok().map(|(_, raw)| raw),
        })?;
        return Ok(LOST);
    }
    writeln!(console.err, "{message}")?;
    match now {
        Ok((item, _)) => write_item(&mut console.out, &item, machine)?,
        Err(error) => writeln!(
            console.err,
            "Its current state could not be read: {error:#}"
        )?,
    }
    Ok(LOST)
}

fn outcome_name(outcome: ControlOutcome) -> &'static str {
    match outcome {
        ControlOutcome::Done => "done",
        ControlOutcome::Starting => "starting",
        ControlOutcome::Refused => "refused",
        ControlOutcome::Conflict => "conflict",
        ControlOutcome::Failed => "failed",
        ControlOutcome::Unknown => "unknown",
    }
}

/// The service as the target shows it now.
fn fresh(
    client: &MgmtSocketClient,
    peer: &Peer,
    query: &ServiceQuery<'_>,
) -> Result<(Item, Value)> {
    let (snapshot, _) = fetch(client, peer)?;
    let items = snapshot.shared_items(peer)?;
    let (item, raw) = find(&items, query, &peer.name)?;
    Ok((item.clone(), (*raw).clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::remote::inventory::tests::item;

    fn peer() -> Peer {
        Peer {
            id: "mB".into(),
            name: "teo-mini".into(),
        }
    }

    fn asked(peer: &Peer, action: ControlAction) -> Asked<'_> {
        Asked {
            peer,
            action,
            service: "web",
            ids: json!({ "projectId": "app", "environment": null, "serviceId": "web" }),
            app_running: true,
        }
    }

    fn console(json: bool) -> Console<Vec<u8>, Vec<u8>> {
        Console {
            json,
            out: Vec::new(),
            err: Vec::new(),
        }
    }

    fn text(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    fn run(
        events: Vec<Value>,
        action: ControlAction,
        console: &mut Console<Vec<u8>, Vec<u8>>,
    ) -> Result<i32> {
        let peer = peer();
        let asked = asked(&peer, action);
        let ended = follow(events.into_iter().map(Ok), &asked, console)?;
        let fresh = || {
            let raw = item("app", None, "web");
            Ok((serde_json::from_value(raw.clone())?, raw))
        };
        let result = report(ended, &asked, console, fresh);
        super::super::finish(console, result)
    }

    #[test]
    fn a_control_prints_its_progress_then_its_result() {
        let mut console = console(false);
        let events = vec![
            json!({ "event": "sent", "requestId": "q" }),
            json!({ "event": "progress", "phase": "dispatched" }),
            json!({ "event": "progress", "phase": "started" }),
            json!({ "event": "result", "outcome": "done", "code": null, "message": "", "runId": "R2" }),
        ];

        let code = run(events, ControlAction::Restart, &mut console).unwrap();

        assert_eq!(code, 0);
        assert_eq!(
            text(&console.out),
            "Restarting 'web' on teo-mini…\nStarting in the app on teo-mini…\nStarted on teo-mini. Waiting for its port…\n'web' is running on teo-mini.\n"
        );
    }

    #[test]
    fn in_json_progress_goes_to_stderr_and_the_result_is_one_document() {
        let mut console = console(true);
        let events = vec![
            json!({ "event": "sent", "requestId": "q" }),
            json!({ "event": "result", "outcome": "done", "code": null, "message": "", "runId": null }),
        ];

        let code = run(events, ControlAction::Stop, &mut console).unwrap();

        assert_eq!(code, 0);
        assert_eq!(text(&console.err), "Stopping 'web' on teo-mini…\n");
        let printed: Value = serde_json::from_slice(&console.out).unwrap();
        assert_eq!(printed["outcome"], "done");
        assert_eq!(printed["action"], "stop");
        assert_eq!(printed["message"], "'web' stopped on teo-mini.");
        assert_eq!(printed["service"]["serviceId"], "web");
    }

    #[test]
    fn a_refusal_from_the_target_exits_2_with_its_code() {
        for json in [true, false] {
            let mut console = console(json);
            let events = vec![json!({
                "event": "result", "outcome": "refused", "code": "background_disabled",
                "message": "Localify isn't open on this machine, and it doesn't allow background starts.",
                "runId": null,
            })];

            let code = run(events, ControlAction::Start, &mut console).unwrap();

            assert_eq!(code, REFUSED);
            let said =
                "Localify isn't open on teo-mini, and teo-mini doesn't allow background starts.";
            if json {
                let printed: Value = serde_json::from_slice(&console.out).unwrap();
                assert_eq!(printed["code"], "background_disabled");
                assert_eq!(printed["message"], said);
            } else {
                assert!(
                    text(&console.err).starts_with(said),
                    "{}",
                    text(&console.err)
                );
            }
        }
    }

    #[test]
    fn a_refusal_before_sending_has_no_sent_event_and_the_connection_state() {
        let mut console = console(true);
        let events = vec![json!({
            "event": "result", "outcome": "refused", "code": "peer_offline",
            "message": "Offline.", "runId": null,
        })];

        assert_eq!(
            run(events, ControlAction::Stop, &mut console).unwrap(),
            REFUSED
        );
        let printed: Value = serde_json::from_slice(&console.out).unwrap();
        assert_eq!(printed["message"], "teo-mini is offline.");
    }

    #[test]
    fn a_lost_outcome_prints_the_fresh_status_and_exits_3() {
        let events = || {
            vec![
                json!({ "event": "sent", "requestId": "q" }),
                json!({ "event": "unknown", "code": "closed", "message": "Session lost." }),
                json!({ "event": "lost", "code": "timeout", "message": "No outcome." }),
            ]
        };
        let mut human = console(false);
        assert_eq!(
            run(events(), ControlAction::Restart, &mut human).unwrap(),
            LOST
        );
        assert!(text(&human.out).contains("Outcome unknown. Checking teo-mini…\n"));
        assert!(
            text(&human.out).contains("APP / web\n  Running, in the app"),
            "{}",
            text(&human.out)
        );
        assert!(text(&human.err).starts_with("teo-mini didn't say what happened."));

        let mut json = console(true);
        assert_eq!(
            run(events(), ControlAction::Restart, &mut json).unwrap(),
            LOST
        );
        let printed: Value = serde_json::from_slice(&json.out).unwrap();
        assert_eq!(printed["outcome"], "lost");
        assert_eq!(printed["code"], "timeout");
        assert_eq!(printed["status"]["process"], "running");
    }

    #[test]
    fn a_conflict_prints_the_fresh_state_and_exits_2() {
        let events = || {
            vec![
                json!({ "event": "sent", "requestId": "q" }),
                json!({
                    "event": "result", "outcome": "conflict", "code": "conflict",
                    "message": "The run changed.", "runId": null,
                }),
            ]
        };

        let mut human = console(false);
        assert_eq!(
            run(events(), ControlAction::Restart, &mut human).unwrap(),
            REFUSED
        );
        assert!(
            text(&human.err).starts_with("teo-mini changed in the meantime.\n"),
            "{}",
            text(&human.err)
        );
        assert!(
            text(&human.out).contains("APP / web\n  Running, in the app"),
            "{}",
            text(&human.out)
        );

        let mut json = console(true);
        assert_eq!(
            run(events(), ControlAction::Restart, &mut json).unwrap(),
            REFUSED
        );
        let printed: Value = serde_json::from_slice(&json.out).unwrap();
        assert_eq!(printed["outcome"], "refused");
        assert_eq!(printed["code"], "conflict");
        assert_eq!(printed["message"], "teo-mini changed in the meantime.");
        assert_eq!(printed["status"]["process"], "running");
    }

    #[test]
    fn a_failed_start_exits_1_with_the_targets_last_lines() {
        let mut console = console(false);
        let events = vec![json!({
            "event": "result", "outcome": "failed", "code": "failed",
            "message": "web exited with code 1.\nError: port in use", "runId": "R2",
        })];

        assert_eq!(
            run(events, ControlAction::Start, &mut console).unwrap(),
            FAILED
        );
        assert_eq!(
            text(&console.err),
            "web exited with code 1.\nError: port in use\n"
        );
    }

    #[test]
    fn what_the_snapshot_refuses_is_never_sent() {
        let web = |controls: Value, legacy: bool| -> Item {
            let mut value = item("app", None, "web");
            value["controls"] = controls;
            value["run"]["legacy"] = json!(legacy);
            serde_json::from_value(value).unwrap()
        };
        let running = web(
            json!({
                "start": { "allowed": false, "code": "conflict" },
                "stop": { "allowed": true },
                "restart": { "allowed": false, "code": "owner_gone" },
            }),
            false,
        );
        let refused = |item: &Item, action| {
            let error = allowed(item, action, "teo-mini").unwrap_err();
            error.downcast_ref::<Refusal>().unwrap().clone()
        };

        assert!(allowed(&running, ControlAction::Stop, "teo-mini").is_ok());
        let start = refused(&running, ControlAction::Start);
        assert_eq!(
            (start.code.as_str(), start.message.as_str()),
            ("conflict", "'web' already runs on teo-mini.")
        );
        assert_eq!(
            refused(&running, ControlAction::Restart).message,
            "Stop it, then start it again."
        );

        let legacy = web(json!({ "restart": { "allowed": true } }), true);
        assert_eq!(refused(&legacy, ControlAction::Restart).code, "legacy_run");
    }

    #[test]
    fn a_legacy_run_that_ended_does_not_block_a_fresh_start() {
        let mut value = item("app", None, "web");
        value["process"] = json!("stopped");
        value["owner"] = json!(null);
        value["run"]["legacy"] = json!(true);
        value["run"]["runId"] = json!(null);
        value["controls"] = json!({
            "start": { "allowed": true },
            "stop": { "allowed": false, "code": "conflict" },
            "restart": { "allowed": false, "code": "conflict" },
        });
        let stopped: Item = serde_json::from_value(value).unwrap();

        assert!(allowed(&stopped, ControlAction::Start, "teo-mini").is_ok());
        let error = allowed(&stopped, ControlAction::Stop, "teo-mini").unwrap_err();
        assert_eq!(error.downcast_ref::<Refusal>().unwrap().code, "conflict");
    }
}
