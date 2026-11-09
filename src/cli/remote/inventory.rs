//! Another machine's services, from its inventory snapshot
//! (docs/remote-machines.md, "Inventory"), and `services status --machine`.

use std::collections::BTreeMap;
use std::io::Write;

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::Value;

use super::{Console, Peer, Refusal, remote_error};
use crate::infrastructure::mgmt_client::MgmtSocketClient;
use crate::infrastructure::remote::wire::{AppPresence, MachineSummary};

/// A service on the target, named as `--project` and `--env` name it there:
/// there is no folder to read on another machine.
#[derive(Debug, Clone, Copy)]
pub struct ServiceQuery<'a> {
    pub service: &'a str,
    pub project: Option<&'a str>,
    /// A checkout other than the primary one, by slug or name.
    pub env: Option<&'a str>,
}

/// A target's whole snapshot, as `remote_inventory` and `remote_watch` give
/// it. Items stay as sent, for JSON, and are read through [`Item`].
#[derive(Debug, Clone, Deserialize)]
pub(super) struct Snapshot {
    pub(super) machine: MachineSummary,
    #[serde(default)]
    pub(super) truncated: bool,
    pub(super) items: Vec<Value>,
}

/// The fields of an `InventoryItem` the CLI reads.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Item {
    pub(super) project: Named,
    pub(super) environment: Environment,
    pub(super) service: ServiceRef,
    pub(super) local_url: String,
    pub(super) owner: Option<String>,
    pub(super) run: Option<Run>,
    pub(super) process: String,
    pub(super) port: Port,
    pub(super) output: Output,
    pub(super) public_url: Option<PublicUrl>,
    #[serde(default)]
    pub(super) controls: BTreeMap<String, Allowed>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct Named {
    pub(super) id: String,
    pub(super) name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct Environment {
    pub(super) slug: Option<String>,
    pub(super) name: String,
    pub(super) primary: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ServiceRef {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) config_rev: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Run {
    pub(super) run_id: Option<String>,
    #[serde(default)]
    pub(super) legacy: bool,
    pub(super) exit: Option<Exit>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct Exit {
    pub(super) code: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct Port {
    pub(super) number: u16,
    pub(super) reachable: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Output {
    pub(super) available: bool,
    pub(super) run_id: Option<String>,
    pub(super) reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct PublicUrl {
    pub(super) url: String,
    pub(super) serving: bool,
    pub(super) reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct Allowed {
    pub(super) allowed: bool,
    pub(super) code: Option<String>,
}

impl Snapshot {
    pub(super) fn parse(value: Value) -> Result<Self> {
        serde_json::from_value(value)
            .context("The daemon sent a snapshot this version of Localify cannot read")
    }

    /// The services, or `not_shared` when the target shares nothing.
    pub(super) fn shared_items(&self, peer: &Peer) -> Result<Vec<(Item, &Value)>> {
        if !self.machine.settings.share {
            return Err(anyhow!(Refusal::from_target("not_shared", "", &peer.name)));
        }
        self.items
            .iter()
            .map(|raw| {
                let item = serde_json::from_value(raw.clone())
                    .context("The daemon sent a service this version of Localify cannot read")?;
                Ok((item, raw))
            })
            .collect()
    }
}

/// One snapshot of the target, every page.
pub(super) fn fetch(client: &MgmtSocketClient, peer: &Peer) -> Result<(Snapshot, Value)> {
    let value = client
        .remote_inventory(&peer.id)
        .map_err(|error| remote_error(error, peer))?;
    Ok((Snapshot::parse(value.clone())?, value))
}

/// The one service `query` names. An ambiguous name lists the matches.
pub(super) fn find<'a, T>(
    items: &'a [(Item, T)],
    query: &ServiceQuery<'_>,
    machine: &str,
) -> Result<&'a (Item, T)> {
    let matches: Vec<&(Item, T)> = items
        .iter()
        .filter(|(item, _)| in_environment(item, query.env))
        .filter(|(item, _)| {
            query
                .project
                .is_none_or(|project| names_project(item, project))
        })
        .filter(|(item, _)| item.service.id == query.service || item.service.name == query.service)
        .collect();
    match matches.as_slice() {
        [one] => Ok(one),
        [] => bail!("{}", no_such_service(items, query, machine)),
        many => {
            let listed: Vec<String> = many
                .iter()
                .map(|(item, _)| format!("  {}", selector(item)))
                .collect();
            bail!(
                "'{}' names more than one service on {machine}. Pass --project with one of:\n{}",
                query.service,
                listed.join("\n")
            )
        }
    }
}

fn in_environment(item: &Item, env: Option<&str>) -> bool {
    match env {
        None => item.environment.primary,
        Some(env) => {
            item.environment.slug.as_deref() == Some(env)
                || item.environment.name.to_lowercase() == env.to_lowercase()
        }
    }
}

fn names_project(item: &Item, project: &str) -> bool {
    item.project.id == project || item.project.name.to_lowercase() == project.to_lowercase()
}

fn no_such_service<T>(items: &[(Item, T)], query: &ServiceQuery<'_>, machine: &str) -> String {
    let known: Vec<String> = items
        .iter()
        .map(|(item, _)| format!("  {}", selector(item)))
        .collect();
    let mut message = format!("No service '{}' on {machine}", query.service);
    if let Some(project) = query.project {
        message.push_str(&format!(" in project '{project}'"));
    }
    if let Some(env) = query.env {
        message.push_str(&format!(" in environment '{env}'"));
    }
    if known.is_empty() {
        message.push_str(&format!(": {machine} has no service."));
    } else {
        message.push_str(&format!(". Its services:\n{}", known.join("\n")));
    }
    message
}

/// How the CLI names a target's service: `web --project app [--env feat]`.
pub(super) fn selector(item: &Item) -> String {
    let mut selector = format!("{} --project {}", item.service.id, item.project.id);
    if let Some(slug) = item
        .environment
        .slug
        .as_deref()
        .filter(|_| !item.environment.primary)
    {
        selector.push_str(&format!(" --env {slug}"));
    }
    selector
}

/// `services status --machine`: every service of the target with its four
/// states; the snapshot as the daemon gave it, as JSON.
pub fn status<O: Write, E: Write>(
    client: &MgmtSocketClient,
    peer: &Peer,
    console: &mut Console<O, E>,
) -> Result<i32> {
    let (snapshot, raw) = fetch(client, peer)?;
    let items = snapshot.shared_items(peer)?;
    if console.json {
        console.json(&raw)?;
        return Ok(0);
    }
    writeln!(console.out, "{}", machine_line(&snapshot.machine))?;
    if items.is_empty() {
        writeln!(console.out, "{} has no service.", peer.name)?;
    }
    for (item, _) in &items {
        writeln!(console.out)?;
        write_item(&mut console.out, item, &peer.name)?;
    }
    if snapshot.truncated {
        writeln!(
            console.out,
            "\n{} has more services than one snapshot holds; these are the first.",
            peer.name
        )?;
    }
    Ok(0)
}

fn machine_line(machine: &MachineSummary) -> String {
    let os = if machine.os == "macos" {
        "macOS"
    } else {
        "Linux"
    };
    let app = match machine.app {
        AppPresence::Running => "app open",
        AppPresence::Absent => "app closed",
    };
    let controls = match (machine.settings.control, machine.settings.background_starts) {
        (false, _) => "remote controls off",
        (true, false) => "remote controls on",
        (true, true) => "remote controls and background starts on",
    };
    format!(
        "{} · {os} · Localify {} · {app} · {controls}",
        machine.name, machine.version
    )
}

/// A service's name, then its states on one line, and its URLs.
pub(super) fn write_item(out: &mut impl Write, item: &Item, machine: &str) -> Result<()> {
    let environment = if item.environment.primary {
        String::new()
    } else {
        format!(" [{}]", item.environment.name)
    };
    writeln!(
        out,
        "{} / {}{environment}",
        item.project.name, item.service.name
    )?;
    let owner = match item.owner.as_deref() {
        Some("app") => ", in the app",
        Some("background") => ", in the background",
        Some("terminal") => ", in a terminal",
        Some("unknown") => ", an unknown process",
        _ => "",
    };
    let port = match item.port.reachable {
        Some(true) => "reachable",
        Some(false) => "not reachable",
        None => "not checked",
    };
    writeln!(
        out,
        "  {}{owner} · port {} {port} · output {}",
        process_state(item),
        item.port.number,
        output_state(item)
    )?;
    writeln!(out, "  {} (opens on {machine} only)", item.local_url)?;
    if let Some(public) = &item.public_url {
        writeln!(out, "  {} ({})", public.url, public_state(public))?;
    }
    Ok(())
}

/// The process state, as the app's chip says it.
pub(super) fn process_state(item: &Item) -> String {
    if item.process == "running" {
        return if item.port.reachable == Some(false) {
            "Starting".to_string()
        } else {
            "Running".to_string()
        };
    }
    match item.run.as_ref().and_then(|run| run.exit.as_ref()?.code) {
        Some(code) if code != 0 => format!("Exited with {code}"),
        _ => "Stopped".to_string(),
    }
}

fn output_state(item: &Item) -> &'static str {
    if item.output.available {
        return "available";
    }
    match item.output.reason.as_deref() {
        Some("terminal") => "in a terminal",
        Some("unknown_owner") => "of an unknown process",
        Some("legacy_run") => "needs a relaunch to share",
        _ => "none yet",
    }
}

fn public_state(public: &PublicUrl) -> &'static str {
    if public.serving {
        return "serving";
    }
    match public.reason.as_deref() {
        Some("suspended") => "suspended",
        Some("taken_down") => "taken down",
        Some("served_elsewhere") => "served elsewhere",
        Some("tunnel_down") => "tunnel down",
        _ => "not serving",
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use serde_json::json;

    pub(in crate::cli::remote) fn item(project: &str, env: Option<&str>, service: &str) -> Value {
        json!({
            "project": { "id": project, "name": project.to_uppercase() },
            "environment": {
                "slug": env, "name": env.unwrap_or("Primary"),
                "primary": env.is_none(), "branch": null,
            },
            "service": { "id": service, "name": service, "configRev": "0123456789abcdef" },
            "localUrl": format!("https://{service}.{project}.localify"),
            "owner": "app",
            "run": {
                "runId": "RRRRRRRRRRRRRRRRRRRRRR", "legacy": false,
                "startedAt": "2026-11-09T09:00:00Z", "profile": null,
                "profileDanger": false, "exit": null,
            },
            "process": "running",
            "port": { "number": 4100, "reachable": true },
            "output": { "available": true, "runId": "RRRRRRRRRRRRRRRRRRRRRR" },
            "publicUrl": null,
            "controls": {
                "start": { "allowed": false, "code": "conflict" },
                "stop": { "allowed": true },
                "restart": { "allowed": true },
            },
        })
    }

    pub(in crate::cli::remote) fn snapshot(share: bool, items: Vec<Value>) -> Value {
        json!({
            "machine": {
                "machineId": "mB", "name": "teo-mini", "version": "1.7.0", "os": "macos",
                "settings": { "share": share, "control": true, "backgroundStarts": false },
                "app": "running",
            },
            "epoch": "e", "rev": 1, "truncated": false, "items": items,
        })
    }

    fn peer() -> Peer {
        Peer {
            id: "mB".into(),
            name: "teo-mini".into(),
        }
    }

    fn items(values: Vec<Value>) -> Vec<(Item, ())> {
        values
            .into_iter()
            .map(|value| (serde_json::from_value(value).unwrap(), ()))
            .collect()
    }

    fn query<'a>(
        service: &'a str,
        project: Option<&'a str>,
        env: Option<&'a str>,
    ) -> ServiceQuery<'a> {
        ServiceQuery {
            service,
            project,
            env,
        }
    }

    #[test]
    fn a_service_is_found_in_the_primary_checkout_unless_env_names_another() {
        let all = items(vec![
            item("app", None, "web"),
            item("app", Some("feat"), "web"),
        ]);

        let (primary, _) = find(&all, &query("web", None, None), "mini").unwrap();
        assert!(primary.environment.primary);
        let (feat, _) = find(&all, &query("web", None, Some("feat")), "mini").unwrap();
        assert_eq!(feat.environment.slug.as_deref(), Some("feat"));
    }

    #[test]
    fn a_name_in_two_projects_needs_project() {
        let all = items(vec![item("app", None, "web"), item("blog", None, "web")]);

        let error = find(&all, &query("web", None, None), "mini")
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("web --project app") && error.contains("web --project blog"),
            "{error}"
        );

        let (found, _) = find(&all, &query("web", Some("BLOG"), None), "mini").unwrap();
        assert_eq!(found.project.id, "blog");
    }

    #[test]
    fn an_unknown_service_lists_the_targets_services() {
        let all = items(vec![
            item("app", None, "web"),
            item("app", Some("feat"), "api"),
        ]);

        let error = find(&all, &query("worker", None, None), "mini")
            .unwrap_err()
            .to_string();

        assert!(error.starts_with("No service 'worker' on mini."), "{error}");
        assert!(error.contains("api --project app --env feat"), "{error}");
    }

    #[test]
    fn a_target_that_shares_nothing_is_refused_not_shared() {
        let snapshot = Snapshot::parse(snapshot(false, Vec::new())).unwrap();

        let error = snapshot.shared_items(&peer()).unwrap_err();

        assert_eq!(error.downcast_ref::<Refusal>().unwrap().code, "not_shared");
    }

    #[test]
    fn an_item_shows_its_four_states() {
        let mut value = item("app", Some("feat"), "web");
        value["port"]["reachable"] = json!(false);
        value["publicUrl"] =
            json!({ "url": "https://web.localify.fyi", "serving": false, "reason": "tunnel_down" });
        let item: Item = serde_json::from_value(value).unwrap();
        let mut out = Vec::new();

        write_item(&mut out, &item, "teo-mini").unwrap();

        assert_eq!(
            String::from_utf8(out).unwrap(),
            "APP / web [feat]\n  Starting, in the app · port 4100 not reachable · output available\n  https://web.app.localify (opens on teo-mini only)\n  https://web.localify.fyi (tunnel down)\n"
        );
    }

    #[test]
    fn a_stopped_service_shows_a_failed_exit() {
        let mut value = item("app", None, "web");
        value["process"] = json!("stopped");
        value["run"]["exit"] = json!({ "at": "2026-11-09T10:42:00Z", "code": 1 });
        let item: Item = serde_json::from_value(value).unwrap();

        assert_eq!(process_state(&item), "Exited with 1");
    }
}
