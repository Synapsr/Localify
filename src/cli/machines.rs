//! `localify machines`: the other machines of this Localify Cloud account,
//! pairing with them, and what they may see and do here
//! (`docs/remote-machines.md`, "CLI"). Pairing and removal need a person at
//! a terminal: an agent cannot pair or remove machines.

use std::io::{self, BufRead, Write};

use anyhow::{Context, Result, anyhow, bail};
use dialoguer::Confirm;
use serde::Deserialize;
use serde_json::{Value, json};

use super::output::{can_prompt, print_json, wants_json};
use crate::application::ports::DaemonConnectionError;
use crate::branding::BRANDING;
use crate::cli::context::AppContext;
use crate::config::LocalifyPaths;
use crate::infrastructure::cloud::api::CloudClient;
use crate::infrastructure::cloud::credentials::CredentialsStore;
use crate::infrastructure::cloud::http::{CloudHttp, block_on};
use crate::infrastructure::mgmt_client::MgmtSocketClient;

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Switch {
    On,
    Off,
}

impl Switch {
    fn value(self) -> bool {
        self == Self::On
    }
}

/// One machine of `remote_machines`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Machine {
    pub(super) id: String,
    pub(super) name: String,
    #[serde(rename = "self")]
    pub(super) is_self: bool,
    paired: bool,
    pub(super) state: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    sharing: Option<bool>,
}

pub(super) fn daemon_error(error: DaemonConnectionError) -> anyhow::Error {
    match error {
        DaemonConnectionError::NotRunning => anyhow!(
            "Remote machines go through the {} daemon, which is not running. Start it with: {} start",
            BRANDING.app_name,
            BRANDING.cli_command()
        ),
        DaemonConnectionError::ProtocolError(message)
        | DaemonConnectionError::Remote { message, .. } => anyhow!(message),
        other => anyhow!("Could not reach the daemon: {other}"),
    }
}

pub(super) fn machines(client: &MgmtSocketClient, probe: bool) -> Result<Vec<Machine>> {
    let list = client.remote_machines(probe).map_err(daemon_error)?;
    serde_json::from_value(list).context("The daemon sent a machine list this version cannot read")
}

/// `localify machines`: name, id, state, paired, sharing, version.
pub fn list(json_flag: bool, ctx: &AppContext) -> Result<()> {
    let status = ctx.mgmt_client.remote_status().map_err(daemon_error)?;
    let list = ctx
        .mgmt_client
        .remote_machines(true)
        .map_err(daemon_error)?;
    if wants_json(json_flag) {
        return print_json(&json!({ "status": status, "machines": list }));
    }
    let machines: Vec<Machine> = serde_json::from_value(list)
        .context("The daemon sent a machine list this version cannot read")?;
    println!("Remote access: {}", relay_line(&status));
    if let Some(print) = status["fingerprint"].as_str() {
        println!("This machine's fingerprint: {print}");
    }
    if machines.is_empty() {
        return Ok(());
    }
    println!();
    let rows: Vec<[String; 6]> = machines.iter().map(row).collect();
    let header = ["NAME", "ID", "STATE", "PAIRED", "SHARING", "VERSION"].map(str::to_string);
    let widths: Vec<usize> = (0..6)
        .map(|column| {
            std::iter::once(&header)
                .chain(&rows)
                .map(|row| row[column].chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    for row in std::iter::once(&header).chain(&rows) {
        let cells: Vec<String> = row
            .iter()
            .zip(&widths)
            .map(|(cell, width)| format!("{cell:<width$}"))
            .collect();
        println!("{}", cells.join("  ").trim_end());
    }
    Ok(())
}

fn relay_line(status: &Value) -> String {
    match status["relay"].as_str().unwrap_or("off") {
        "connected" => "connected".to_string(),
        "connecting" => "connecting…".to_string(),
        "reconnecting" => match status["retryAt"].as_str() {
            Some(at) => format!("reconnecting… (next attempt at {at})"),
            None => "reconnecting…".to_string(),
        },
        "blocked" => format!(
            "off. {}",
            status["blocked"]["message"].as_str().unwrap_or_default()
        ),
        _ => format!(
            "off. Sign in with `{} login` to see your other machines.",
            BRANDING.cli_command()
        ),
    }
}

fn row(machine: &Machine) -> [String; 6] {
    let name = if machine.is_self {
        format!("{} (this machine)", machine.name)
    } else {
        machine.name.clone()
    };
    let yes_no = |value: bool| if value { "yes" } else { "no" }.to_string();
    [
        name,
        machine.id.clone(),
        state_label(&machine.state).to_string(),
        if machine.is_self {
            "—".into()
        } else {
            yes_no(machine.paired)
        },
        machine.sharing.map_or_else(|| "—".to_string(), yes_no),
        machine.version.clone().unwrap_or_else(|| "—".into()),
    ]
}

fn state_label(state: &str) -> &str {
    match state {
        "offline" => "offline",
        "connecting" => "connecting…",
        "reconnecting" => "reconnecting…",
        "connected" => "connected",
        "not_responding" => "not responding",
        "incompatible" => "update needed",
        "not_paired" => "not paired",
        "key_mismatch" => "key mismatch: don't trust it until you pair it again",
        "not_sharing" => "paired · not sharing",
        "signed_out" => "signed out",
        "unavailable" => "unavailable",
        other => other,
    }
}

/// A machine the person named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Candidate {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) is_self: bool,
}

/// An exact machine id, else an exact name ignoring case. A name that
/// matches two machines lists both.
pub(super) fn resolve(candidates: &[Candidate], query: &str) -> Result<Candidate> {
    if let Some(found) = candidates.iter().find(|machine| machine.id == query) {
        return Ok(found.clone());
    }
    let named: Vec<&Candidate> = candidates
        .iter()
        .filter(|machine| machine.name.to_lowercase() == query.to_lowercase())
        .collect();
    match named.as_slice() {
        [one] => Ok((*one).clone()),
        [] => {
            let known: Vec<String> = candidates
                .iter()
                .map(|machine| format!("  {} ({})", machine.name, machine.id))
                .collect();
            if known.is_empty() {
                bail!("No machine named '{query}': this account has no other machine yet.");
            }
            bail!(
                "No machine named '{query}'. The account's machines:\n{}",
                known.join("\n")
            )
        }
        many => {
            let listed: Vec<String> = many
                .iter()
                .map(|machine| format!("  {} ({})", machine.name, machine.id))
                .collect();
            bail!(
                "'{query}' names more than one machine. Use its id:\n{}",
                listed.join("\n")
            )
        }
    }
}

pub(super) fn candidates(machines: &[Machine]) -> Vec<Candidate> {
    machines
        .iter()
        .map(|machine| Candidate {
            id: machine.id.clone(),
            name: machine.name.clone(),
            is_self: machine.is_self,
        })
        .collect()
}

fn require_person(what: &str) -> Result<()> {
    if can_prompt() {
        return Ok(());
    }
    bail!(
        "{what} needs a person at a terminal on this machine: run `{} machines {}` yourself.",
        BRANDING.cli_command(),
        what.to_lowercase()
    )
}

/// Reads one line and accepts only `yes`.
fn typed_yes(prompt: &str) -> Result<bool> {
    print!("{prompt} ");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().lock().read_line(&mut answer)?;
    Ok(answer.trim().eq_ignore_ascii_case("yes"))
}

/// `localify machines pair <machine>`, as the viewer.
pub fn pair(query: &str, ctx: &AppContext) -> Result<()> {
    require_person("Pair")?;
    let target = resolve(&candidates(&machines(&ctx.mgmt_client, false)?), query)?;
    if target.is_self {
        bail!("{} is this machine: pair it from another one.", target.name);
    }
    let mut stream = ctx
        .mgmt_client
        .remote_stream(&json!({ "cmd": "remote_pair", "machineId": target.id }))
        .map_err(daemon_error)?;
    println!("Asking {} to pair…", target.name);
    while let Some(event) = stream.next_answer().map_err(daemon_error)? {
        match event["event"].as_str() {
            Some("code") => {
                let code = event["code"].as_str().unwrap_or_default();
                let pairing_id = event["pairingId"].as_str().unwrap_or_default();
                println!("Pair with {}", target.name);
                let accept = typed_yes(&format!(
                    "Check that {} shows this code: {code}. Type yes if it does:",
                    target.name
                ))?;
                answer(ctx, pairing_id, accept)?;
            }
            Some("paired") => {
                println!(
                    "Paired with {}. It shows this machine's name; it shares projects and output, or allows controls, only once its owner turns that on.",
                    target.name
                );
                return Ok(());
            }
            Some("cancelled") => {
                bail!("{}", cancelled_message(&event));
            }
            _ => {}
        }
    }
    bail!("Pairing cancelled. Nothing was saved.")
}

fn answer(ctx: &AppContext, pairing_id: &str, accept: bool) -> Result<()> {
    match ctx.mgmt_client.remote_pair_answer(pairing_id, accept) {
        Ok(()) => Ok(()),
        Err(error) => Err(daemon_error(error)),
    }
}

fn cancelled_message(event: &Value) -> String {
    let message = event["message"].as_str().unwrap_or_default();
    if message.is_empty() || message == "Pairing cancelled. Nothing was saved." {
        "Pairing cancelled. Nothing was saved.".to_string()
    } else {
        format!("Pairing cancelled. Nothing was saved. {message}")
    }
}

/// `localify machines pair --accept`: waits here for another machine's
/// request, and asks the person to confirm its code.
pub fn accept(ctx: &AppContext) -> Result<()> {
    require_person("Pair --accept")?;
    let mut stream = ctx
        .mgmt_client
        .remote_stream(&json!({ "cmd": "remote_events" }))
        .map_err(daemon_error)?;
    let mut waiting_for: Option<String> = None;
    while let Some(event) = stream.next_answer().map_err(daemon_error)? {
        let pairing_id = event["pairingId"].as_str().unwrap_or_default().to_string();
        let name = event["name"]
            .as_str()
            .unwrap_or("The other machine")
            .to_string();
        match event["event"].as_str() {
            Some("subscribed") => println!(
                "Waiting for a pairing request. On the other machine, run `{} machines pair <this machine>`. Ctrl-C stops.",
                BRANDING.cli_command()
            ),
            Some("pair_request") if waiting_for.is_none() => {
                let code = event["code"].as_str().unwrap_or_default();
                let accept = typed_yes(&format!(
                    "{name} wants to pair. Code: {code}. Type yes if {name} shows the same code:"
                ))?;
                answer(ctx, &pairing_id, accept)?;
                waiting_for = Some(pairing_id);
            }
            Some("paired") if waiting_for.as_deref() == Some(pairing_id.as_str()) => {
                println!(
                    "Paired with {name}. It sees this machine's name only, until you turn on sharing: {} machines settings --share on",
                    BRANDING.cli_command()
                );
                return Ok(());
            }
            Some("pair_cancelled") if waiting_for.as_deref() == Some(pairing_id.as_str()) => {
                bail!("Pairing cancelled. Nothing was saved.")
            }
            _ => {}
        }
    }
    bail!("The daemon stopped waiting for a pairing request.")
}

/// `localify machines unpair <machine>`: deletes the pin, and tells the
/// machine when a session is open.
pub fn unpair(query: &str, json_flag: bool, ctx: &AppContext) -> Result<()> {
    let machines = machines(&ctx.mgmt_client, false)?;
    let paired: Vec<Machine> = machines
        .into_iter()
        .filter(|machine| machine.paired)
        .collect();
    let target = resolve(&candidates(&paired), query)?;
    let removed = ctx
        .mgmt_client
        .remote_unpair(&target.id)
        .map_err(daemon_error)?;
    if wants_json(json_flag) {
        return print_json(&json!({ "machineId": target.id, "unpaired": removed }));
    }
    println!(
        "Unpaired {}. Pair it again to see it from here.",
        target.name
    );
    Ok(())
}

/// `localify machines remove <machine>`: signs it out of the account.
pub fn remove(
    query: &str,
    yes: bool,
    json_flag: bool,
    ctx: &AppContext,
    paths: &LocalifyPaths,
) -> Result<()> {
    require_person("Remove")?;
    let Some(credentials) = CredentialsStore::in_data_dir(&paths.data_dir).load()? else {
        bail!(
            "This machine is not signed in to Localify Cloud. Sign in with: {} login",
            BRANDING.cli_command()
        );
    };
    let http = CloudHttp::new(credentials.api_base, Some(credentials.token))?;
    let cloud = CloudClient::new(http);
    let roster = block_on(cloud.machines())??;
    let roster: Vec<Candidate> = roster
        .into_iter()
        .map(|machine| Candidate {
            id: machine.id,
            name: machine.name,
            is_self: machine.is_self,
        })
        .collect();
    let target = resolve(&roster, query)?;
    if target.is_self {
        bail!(
            "{} is this machine. Sign it out with: {} logout",
            target.name,
            BRANDING.cli_command()
        );
    }
    let question = format!(
        "Remove {} ({}) from the account? It is signed out at once, and its pins go on every machine.",
        target.name, target.id
    );
    if !yes
        && !Confirm::new()
            .with_prompt(question)
            .default(false)
            .interact()?
    {
        println!("Nothing was removed.");
        return Ok(());
    }
    block_on(cloud.remove_machine(&target.id))??;
    // The relay tells every connected peer; this one forgets its pin now.
    let _ = ctx.mgmt_client.remote_unpair(&target.id);
    if wants_json(json_flag) {
        return print_json(&json!({ "removed": target.id }));
    }
    println!("Removed {} from the account.", target.name);
    Ok(())
}

/// `localify machines settings`: shows the three switches, or changes them.
pub fn settings(
    share: Option<Switch>,
    control: Option<Switch>,
    background_starts: Option<Switch>,
    json_flag: bool,
    ctx: &AppContext,
) -> Result<()> {
    let settings = if share.is_none() && control.is_none() && background_starts.is_none() {
        ctx.mgmt_client.remote_status().map_err(daemon_error)?["settings"].take()
    } else {
        let mut change = json!({});
        for (key, switch) in [
            ("share", share),
            ("control", control),
            ("backgroundStarts", background_starts),
        ] {
            if let Some(switch) = switch {
                change[key] = json!(switch.value());
            }
        }
        ctx.mgmt_client
            .remote_settings(change)
            .map_err(daemon_error)?
    };
    if wants_json(json_flag) {
        return print_json(&settings);
    }
    let on_off = |key: &str| {
        if settings[key].as_bool().unwrap_or(false) {
            "on"
        } else {
            "off"
        }
    };
    println!(
        "Share projects and output with paired machines: {}",
        on_off("share")
    );
    println!(
        "Allow paired machines to start, stop and restart services: {}",
        on_off("control")
    );
    println!(
        "Allow background starts when the app is closed: {}",
        on_off("backgroundStarts")
    );
    if settings["share"].as_bool().unwrap_or(false) {
        println!(
            "Output can contain secrets your services print, and what you type into service tabs."
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(id: &str, name: &str) -> Candidate {
        Candidate {
            id: id.into(),
            name: name.into(),
            is_self: false,
        }
    }

    #[test]
    fn a_machine_resolves_by_id_then_by_name_ignoring_case() {
        let all = [candidate("m1", "teo-mini"), candidate("m2", "Laptop")];
        assert_eq!(resolve(&all, "m2").unwrap().name, "Laptop");
        assert_eq!(resolve(&all, "laptop").unwrap().id, "m2");
        assert_eq!(resolve(&all, "TEO-MINI").unwrap().id, "m1");
    }

    #[test]
    fn a_name_of_two_machines_lists_both_with_their_ids() {
        let all = [candidate("m1", "mac"), candidate("m2", "Mac")];
        let error = resolve(&all, "mac").unwrap_err().to_string();
        assert!(error.contains("m1") && error.contains("m2"), "{error}");
    }

    #[test]
    fn an_unknown_name_lists_the_roster() {
        let all = [candidate("m1", "teo-mini")];
        let error = resolve(&all, "desk").unwrap_err().to_string();
        assert!(error.contains("teo-mini (m1)"), "{error}");
    }
}
