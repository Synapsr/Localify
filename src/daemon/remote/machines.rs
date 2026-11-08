//! The roster merged with pins, presence and sessions: what
//! `remote_machines` answers (`docs/remote-machines.md`, "Daemon" and the
//! states of "Desktop app").

use std::collections::HashMap;

use serde::Serialize;

use crate::infrastructure::cloud::types::RosterMachine;
use crate::infrastructure::remote::noise::{decode_key, fingerprint};
use crate::infrastructure::remote::store::Pins;
use crate::infrastructure::remote::wire::{Presence, PresenceState, ProtoRange, TargetSettings};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayState {
    Off,
    Connecting,
    Connected,
    Reconnecting,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MachineState {
    Offline,
    /// The relay link, or this machine's session handshake, is starting.
    Connecting,
    /// The relay link dropped and is coming back: nothing is known.
    Reconnecting,
    Connected,
    NotResponding,
    Incompatible,
    NotPaired,
    KeyMismatch,
    NotSharing,
    SignedOut,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MachineView {
    pub id: String,
    pub name: String,
    #[serde(rename = "self")]
    pub is_self: bool,
    pub paired: bool,
    pub state: MachineState,
    pub version: Option<String>,
    /// Whether it shares its projects and output, from its last `hello`;
    /// `None` until a session said.
    pub sharing: Option<bool>,
    pub fingerprint: Option<String>,
    /// When the relay saw it come or go, RFC 3339.
    pub since: Option<String>,
}

/// A viewer session's state, as the link keeps it for this listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Handshaking,
    Open { not_responding: bool },
}

/// Everything the listing is computed from.
pub struct Facts<'a> {
    pub relay: RelayState,
    pub roster: &'a [RosterMachine],
    pub pins: &'a Pins,
    pub presence: &'a HashMap<String, Presence>,
    pub sessions: &'a HashMap<String, SessionState>,
    /// The settings of each peer's last `hello` as a target.
    pub settings: &'a HashMap<String, TargetSettings>,
    /// The last peer check that failed, by machine id: `not_paired` or
    /// `key_mismatch`.
    pub failures: &'a HashMap<String, String>,
}

fn print(key: &str) -> Option<String> {
    decode_key(key).ok().map(|key| fingerprint(&key))
}

/// This machine first, then the roster, then the pins of machines that left
/// it.
pub fn list(facts: &Facts<'_>) -> Vec<MachineView> {
    let mut views: Vec<MachineView> = facts
        .roster
        .iter()
        .map(|machine| view(facts, machine))
        .collect();
    views.sort_by_key(|view| !view.is_self);
    for (id, pin) in facts.pins {
        if facts.roster.iter().all(|machine| &machine.id != id) {
            views.push(MachineView {
                id: id.clone(),
                name: pin.name.clone(),
                is_self: false,
                paired: true,
                state: MachineState::SignedOut,
                version: None,
                sharing: None,
                fingerprint: print(&pin.public_key),
                since: None,
            });
        }
    }
    views
}

fn view(facts: &Facts<'_>, machine: &RosterMachine) -> MachineView {
    let pin = facts.pins.get(&machine.id);
    let presence = facts.presence.get(&machine.id);
    MachineView {
        id: machine.id.clone(),
        name: machine.name.clone(),
        is_self: machine.is_self,
        paired: pin.is_some(),
        state: state(facts, machine),
        version: presence.map(|presence| presence.version.clone()),
        sharing: facts
            .settings
            .get(&machine.id)
            .map(|settings| settings.share),
        fingerprint: machine.remote_key.as_deref().and_then(print),
        since: presence.map(|presence| presence.since.clone()),
    }
}

fn state(facts: &Facts<'_>, machine: &RosterMachine) -> MachineState {
    match facts.relay {
        RelayState::Connected => {}
        RelayState::Connecting => return MachineState::Connecting,
        RelayState::Reconnecting => return MachineState::Reconnecting,
        RelayState::Off | RelayState::Blocked => return MachineState::Unavailable,
    }
    if machine.is_self {
        return MachineState::Connected;
    }
    let pin = facts.pins.get(&machine.id);
    let mismatch = match (pin, machine.remote_key.as_deref()) {
        (Some(pin), Some(key)) => pin.public_key != key,
        _ => false,
    };
    let failure = facts.failures.get(&machine.id).map(String::as_str);
    if pin.is_some() && (mismatch || failure == Some("key_mismatch")) {
        return MachineState::KeyMismatch;
    }
    let online = facts
        .presence
        .get(&machine.id)
        .filter(|presence| presence.state == PresenceState::Online);
    let Some(presence) = online else {
        return MachineState::Offline;
    };
    if !presence.proto.overlaps(&ProtoRange::OURS) {
        return MachineState::Incompatible;
    }
    if pin.is_none() || failure == Some("not_paired") {
        return MachineState::NotPaired;
    }
    match facts.sessions.get(&machine.id) {
        Some(SessionState::Handshaking) => MachineState::Connecting,
        Some(SessionState::Open {
            not_responding: true,
        }) => MachineState::NotResponding,
        _ => match facts.settings.get(&machine.id) {
            Some(settings) if !settings.share => MachineState::NotSharing,
            _ => MachineState::Connected,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::remote::store::Pin;

    fn machine(id: &str, key: Option<&str>, is_self: bool) -> RosterMachine {
        RosterMachine {
            id: id.into(),
            name: format!("name-{id}"),
            created_at: "2026-11-01T00:00:00Z".into(),
            remote_key: key.map(str::to_string),
            is_self,
        }
    }

    fn online(id: &str, max: u32) -> Presence {
        Presence {
            machine_id: id.into(),
            state: PresenceState::Online,
            version: "1.4.0".into(),
            proto: ProtoRange { min: 1, max },
            since: "2026-11-08T10:42:00Z".into(),
        }
    }

    fn pin(key: &str) -> Pin {
        Pin {
            public_key: key.into(),
            name: "old-name".into(),
            paired_at: "2026-11-08T10:12:00Z".into(),
        }
    }

    const KEY: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    const OTHER: &str = "AQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

    #[test]
    fn each_machine_gets_the_state_the_contract_names() {
        let roster = vec![
            machine("b", Some(KEY), false),
            machine("self", Some(KEY), true),
            machine("offline", Some(KEY), false),
            machine("old", Some(KEY), false),
            machine("unpaired", Some(KEY), false),
            machine("swapped", Some(OTHER), false),
            machine("quiet", Some(KEY), false),
            machine("closed", Some(KEY), false),
            machine("dialing", Some(KEY), false),
        ];
        let pins: Pins = [
            "b", "offline", "old", "swapped", "quiet", "closed", "dialing", "gone",
        ]
        .into_iter()
        .map(|id| (id.to_string(), pin(KEY)))
        .collect();
        let presence: HashMap<_, _> = ["b", "unpaired", "swapped", "quiet", "closed", "dialing"]
            .into_iter()
            .map(|id| (id.to_string(), online(id, 1)))
            .chain([("old".to_string(), online("old", 0))])
            .collect();
        let sessions = HashMap::from([
            (
                "quiet".to_string(),
                SessionState::Open {
                    not_responding: true,
                },
            ),
            ("dialing".to_string(), SessionState::Handshaking),
        ]);
        let settings = HashMap::from([(
            "closed".to_string(),
            TargetSettings {
                share: false,
                control: false,
                background_starts: false,
            },
        )]);
        let failures = HashMap::new();
        let facts = Facts {
            relay: RelayState::Connected,
            roster: &roster,
            pins: &pins,
            presence: &presence,
            sessions: &sessions,
            settings: &settings,
            failures: &failures,
        };

        let views = list(&facts);
        let state = |id: &str| views.iter().find(|view| view.id == id).unwrap().state;
        assert!(views[0].is_self);
        assert_eq!(state("self"), MachineState::Connected);
        assert_eq!(state("b"), MachineState::Connected);
        assert_eq!(state("offline"), MachineState::Offline);
        assert_eq!(state("old"), MachineState::Incompatible);
        assert_eq!(state("unpaired"), MachineState::NotPaired);
        assert_eq!(state("swapped"), MachineState::KeyMismatch);
        assert_eq!(state("quiet"), MachineState::NotResponding);
        assert_eq!(state("closed"), MachineState::NotSharing);
        assert_eq!(state("dialing"), MachineState::Connecting);
        assert_eq!(state("gone"), MachineState::SignedOut);
        let gone = views.iter().find(|view| view.id == "gone").unwrap();
        assert_eq!(gone.name, "old-name");
        assert!(gone.paired);
    }

    #[test]
    fn without_the_relay_no_machine_has_a_state_of_its_own() {
        let roster = vec![machine("b", Some(KEY), false)];
        let empty_pins = Pins::new();
        let empty = HashMap::new();
        let (sessions, settings, failures) = (HashMap::new(), HashMap::new(), HashMap::new());
        for (relay, expected) in [
            (RelayState::Reconnecting, MachineState::Reconnecting),
            (RelayState::Connecting, MachineState::Connecting),
            (RelayState::Blocked, MachineState::Unavailable),
        ] {
            let facts = Facts {
                relay,
                roster: &roster,
                pins: &empty_pins,
                presence: &empty,
                sessions: &sessions,
                settings: &settings,
                failures: &failures,
            };
            assert_eq!(list(&facts)[0].state, expected);
        }
    }
}
