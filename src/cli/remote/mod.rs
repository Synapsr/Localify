//! `--machine`: the services of another machine of the account, through this
//! machine's daemon, which holds the session to it (docs/remote-machines.md,
//! "CLI"). Status and output come from its snapshots and reads; `dev`, `stop`
//! and `restart` become controls. What the other machine refuses is said as
//! the app says it ("Errors"), and exits with [`REFUSED`].

mod control;
mod inventory;
mod logs;

use std::io::{self, Stderr, Stdout, Write};

use anyhow::{Result, anyhow};
use serde::Serialize;

pub use control::control;
pub use inventory::{ServiceQuery, status};
pub use logs::{LogsQuery, logs};

pub use crate::infrastructure::remote::wire::ControlAction;

use super::machines::{Machine as Listed, candidates, daemon_error, machines, resolve};
use super::output::wants_json;
use crate::application::ports::DaemonConnectionError;
use crate::infrastructure::mgmt_client::MgmtSocketClient;

/// The exit code of a refusal: by the other machine, or before anything
/// was sent to it.
pub const REFUSED: i32 = 2;
/// The exit code of a control whose outcome could not be learned.
pub const LOST: i32 = 3;

/// Where a command with `--machine` acts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Machine {
    /// This machine's own id or name: as without `--machine`.
    Local,
    Remote(Peer),
}

/// Another machine of the account, as the roster names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Peer {
    pub id: String,
    pub name: String,
}

/// Resolves `--machine`: an exact id, else an exact name ignoring case,
/// among the account's active machines. Pins of machines that left the
/// account are not candidates.
pub fn machine(query: &str, client: &MgmtSocketClient) -> Result<Machine> {
    pick(&machines(client, false)?, query)
}

fn pick(listed: &[Listed], query: &str) -> Result<Machine> {
    let active: Vec<Listed> = listed
        .iter()
        .filter(|machine| machine.state != "signed_out")
        .cloned()
        .collect();
    let found = resolve(&candidates(&active), query)?;
    if found.is_self {
        return Ok(Machine::Local);
    }
    Ok(Machine::Remote(Peer {
        id: found.id,
        name: found.name,
    }))
}

/// Runs `remote` on the machine `query` names, or `local` when it names
/// this one, as without `--machine`.
pub fn on_machine(
    query: &str,
    client: &MgmtSocketClient,
    local: impl FnOnce() -> Result<()>,
    remote: impl FnOnce(&Peer) -> Result<i32>,
) -> Result<i32> {
    match machine(query, client)? {
        Machine::Local => local().map(|()| 0),
        Machine::Remote(peer) => remote(&peer),
    }
}

/// Where a remote command writes: a person reads text, anything else one
/// JSON document on stdout, with progress on stderr.
pub struct Console<O, E> {
    pub json: bool,
    pub out: O,
    pub err: E,
}

impl Console<Stdout, Stderr> {
    pub fn standard(json_flag: bool) -> Self {
        Self {
            json: wants_json(json_flag),
            out: io::stdout(),
            err: io::stderr(),
        }
    }
}

impl<O: Write, E: Write> Console<O, E> {
    /// A line beside the result: on stdout for a person, on stderr with JSON.
    fn note(&mut self, line: &str) -> Result<()> {
        if self.json {
            writeln!(self.err, "{line}")?;
        } else {
            writeln!(self.out, "{line}")?;
        }
        Ok(())
    }

    fn json(&mut self, value: &impl Serialize) -> Result<()> {
        writeln!(self.out, "{}", serde_json::to_string_pretty(value)?)?;
        Ok(())
    }
}

/// Runs a `--machine` command, then ends the process with its exit code. A
/// [`Refusal`] is printed as one, and exits with [`REFUSED`].
pub fn run(
    json_flag: bool,
    command: impl FnOnce(&mut Console<Stdout, Stderr>) -> Result<i32>,
) -> Result<()> {
    let mut console = Console::standard(json_flag);
    let result = command(&mut console);
    let code = finish(&mut console, result)?;
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

/// The exit code of a command's result, once a refusal is printed.
pub(crate) fn finish<O: Write, E: Write>(
    console: &mut Console<O, E>,
    result: Result<i32>,
) -> Result<i32> {
    match result {
        Ok(code) => Ok(code),
        Err(error) => match error.downcast::<Refusal>() {
            Ok(refusal) => refusal.print(console).map(|()| REFUSED),
            Err(error) => Err(error),
        },
    }
}

/// Something the other machine refused, or the CLI before sending it: the
/// contract's code, what the viewer says, and the target's own words when
/// they add something.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, thiserror::Error)]
#[error("{message}")]
pub struct Refusal {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl Refusal {
    /// A refusal decided here, with its own message.
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            detail: None,
        }
    }

    /// A target's refusal, said as the viewer says it.
    pub fn from_target(code: &str, target_message: &str, machine: &str) -> Self {
        let message = viewer_message(code, machine, target_message);
        let detail = (!target_message.is_empty() && target_message != message)
            .then(|| target_message.to_string());
        Self {
            code: code.to_string(),
            message,
            detail,
        }
    }

    fn print<O: Write, E: Write>(&self, console: &mut Console<O, E>) -> Result<()> {
        if console.json {
            #[derive(Serialize)]
            struct Refused<'a> {
                outcome: &'static str,
                #[serde(flatten)]
                refusal: &'a Refusal,
            }
            return console.json(&Refused {
                outcome: "refused",
                refusal: self,
            });
        }
        writeln!(console.err, "{}", self.message)?;
        if let Some(detail) = &self.detail {
            writeln!(console.err, "{detail}")?;
        }
        Ok(())
    }
}

/// A daemon error on a remote command: a refusal with its code when the
/// other machine, or the way to it, refused.
fn remote_error(error: DaemonConnectionError, peer: &Peer) -> anyhow::Error {
    match error {
        DaemonConnectionError::Remote { code, message } => {
            anyhow!(Refusal::from_target(&code, &message, &peer.name))
        }
        other => daemon_error(other),
    }
}

/// What the viewer says for a target's error code: the table of "Errors",
/// as the app's `remoteErrorMessage` (desktop-ui/src/lib/util.remote.ts)
/// says it. A code it does not know keeps the target's message.
pub fn viewer_message(code: &str, machine: &str, fallback: &str) -> String {
    match code {
        "not_paired" => format!("This machine is not paired with {machine}. Pair them again."),
        "key_mismatch" => "Don't trust this machine until you pair it again.".to_string(),
        "not_shared" => format!(
            "{machine} doesn't share its projects. Turn on sharing in Localify on {machine}."
        ),
        "control_disabled" => format!("{machine} doesn't allow remote controls."),
        "background_disabled" => format!(
            "Localify isn't open on {machine}, and {machine} doesn't allow background starts."
        ),
        "not_owned" => format!("This runs in a terminal on {machine}. Stop it there."),
        "legacy_run" => "Relaunch it on that machine to enable remote control.".to_string(),
        "owner_gone" => "Stop it, then start it again.".to_string(),
        "refused_profile" => format!("Start it on {machine} to use this profile."),
        "unknown_service" => format!("This service no longer exists on {machine}."),
        "conflict" => format!("{machine} changed in the meantime."),
        "expired" => {
            format!("The request took too long to reach {machine}. Nothing was started.")
        }
        "busy" => format!("Something else is starting or stopping it on {machine}."),
        "too_many" => {
            format!("This machine sent {machine} too many requests lately. Try again later.")
        }
        "unauthorized" | "lease_expired" => "Remote access is not available right now.".to_string(),
        "subscription_required" => "Remote access needs Localify Cloud Pro.".to_string(),
        "unsupported" => format!("Update Localify on {machine}."),
        "peer_offline" => format!("{machine} is offline."),
        "not_connected" => format!("Not connected to {machine}."),
        "connecting" => format!("Still connecting to {machine}. Try again once it is connected."),
        "not_responding" => format!("{machine} isn't responding right now."),
        "revoked" => format!("{machine} was removed from the account."),
        "unpaired" => format!("{machine} is no longer paired with this machine."),
        _ => fallback.to_string(),
    }
}

/// `dev --machine` takes a service, `--project`, `--env` and `--profile`
/// only: the target decides where and how its services run. `given` names
/// the other options that were passed.
pub fn only_target_options(given: &[&str]) -> Result<()> {
    if given.is_empty() {
        return Ok(());
    }
    Err(anyhow!(Refusal::new(
        "refused_argument",
        format!(
            "With --machine, dev takes only a service, --project, --env and --profile: the other machine decides how its services start. Remove {}.",
            given.join(", ")
        ),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(id: &str, name: &str, is_self: bool, state: &str) -> Listed {
        serde_json::from_value(serde_json::json!({
            "id": id, "name": name, "self": is_self, "paired": !is_self, "state": state,
        }))
        .unwrap()
    }

    fn roster() -> Vec<Listed> {
        vec![
            listed("m1", "teo-mini", true, "connected"),
            listed("m2", "Laptop", false, "connected"),
            listed("m3", "desk", false, "offline"),
            listed("m4", "old-desk", false, "signed_out"),
        ]
    }

    fn remote(id: &str, name: &str) -> Machine {
        Machine::Remote(Peer {
            id: id.into(),
            name: name.into(),
        })
    }

    #[test]
    fn a_machine_is_found_by_id_then_by_name_ignoring_case() {
        assert_eq!(pick(&roster(), "m2").unwrap(), remote("m2", "Laptop"));
        assert_eq!(pick(&roster(), "LAPTOP").unwrap(), remote("m2", "Laptop"));
        // Offline is still active: the command says why it can't reach it.
        assert_eq!(pick(&roster(), "desk").unwrap(), remote("m3", "desk"));
    }

    #[test]
    fn this_machines_own_id_or_name_acts_locally() {
        assert_eq!(pick(&roster(), "m1").unwrap(), Machine::Local);
        assert_eq!(pick(&roster(), "Teo-Mini").unwrap(), Machine::Local);
    }

    #[test]
    fn two_names_differing_only_in_case_list_both_ids() {
        let mut machines = roster();
        machines.push(listed("m5", "LAPTOP", false, "offline"));

        let error = pick(&machines, "laptop").unwrap_err().to_string();

        assert!(
            error.contains("Laptop (m2)") && error.contains("LAPTOP (m5)"),
            "{error}"
        );
    }

    #[test]
    fn an_unknown_name_lists_the_active_roster() {
        let error = pick(&roster(), "nas").unwrap_err().to_string();

        assert!(error.contains("No machine named 'nas'"), "{error}");
        assert!(
            error.contains("Laptop (m2)") && error.contains("desk (m3)"),
            "{error}"
        );
        // A machine that left the account is neither listed nor found.
        assert!(!error.contains("old-desk"), "{error}");
        assert!(pick(&roster(), "old-desk").is_err());
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

    #[test]
    fn a_refusal_exits_2_with_its_code_in_json() {
        let mut console = console(true);
        let refusal = Refusal::from_target("control_disabled", "", "teo-mini");

        let code = finish(&mut console, Err(anyhow!(refusal))).unwrap();

        assert_eq!(code, REFUSED);
        let printed: serde_json::Value = serde_json::from_slice(&console.out).unwrap();
        assert_eq!(printed["outcome"], "refused");
        assert_eq!(printed["code"], "control_disabled");
        assert_eq!(
            printed["message"],
            "teo-mini doesn't allow remote controls."
        );
        assert!(console.err.is_empty());
    }

    #[test]
    fn a_refusal_for_a_person_is_its_message_on_stderr() {
        let mut console = console(false);
        let refusal = Refusal::from_target(
            "conflict",
            "The service changed since it was looked at. It is running already.",
            "teo-mini",
        );

        let code = finish(&mut console, Err(anyhow!(refusal))).unwrap();

        assert_eq!(code, REFUSED);
        assert!(console.out.is_empty());
        assert_eq!(
            text(&console.err),
            "teo-mini changed in the meantime.\nThe service changed since it was looked at. It is running already.\n"
        );
    }

    #[test]
    fn any_other_error_stays_an_error() {
        let mut console = console(true);

        assert!(finish(&mut console, Err(anyhow!("daemon down"))).is_err());
        assert_eq!(finish(&mut console, Ok(0)).unwrap(), 0);
    }

    #[test]
    fn an_unknown_code_keeps_the_targets_message() {
        let refusal = Refusal::from_target("output_unavailable", "No stream.", "mini");

        assert_eq!(refusal.message, "No stream.");
        assert_eq!(refusal.detail, None);
    }

    #[test]
    fn dev_on_another_machine_refuses_every_other_option() {
        assert!(only_target_options(&[]).is_ok());

        let error = only_target_options(&["--detach", "a command after --"]).unwrap_err();
        let refusal = error.downcast_ref::<Refusal>().unwrap();
        assert_eq!(refusal.code, "refused_argument");
        assert!(
            refusal
                .message
                .ends_with("Remove --detach, a command after --."),
            "{}",
            refusal.message
        );
    }
}
