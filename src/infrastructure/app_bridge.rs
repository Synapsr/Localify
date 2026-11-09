//! The app bridge (docs/remote-machines.md, "The app bridge"): the Unix
//! socket the desktop app listens on while it runs, `<data dir>/app-bridge.sock`,
//! through which the daemon and `localify dev --in-app` hand it a launch to
//! run in a service tab. One JSON request per connection, one JSON answer, as
//! on the management socket.
//!
//! This is the protocol and its client. The app's side is
//! `localify::desktop::app_bridge`.

use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::infrastructure::process::NativeProcesses;
use crate::infrastructure::remote::wire::AppPresence;
use crate::infrastructure::running_services::RunningServiceStore;
use crate::infrastructure::runs::RunId;
use crate::infrastructure::runs::launches::{LaunchReport, reconcile, unix_ms_now};

/// In the data dir.
pub const SOCKET_NAME: &str = "app-bridge.sock";
/// How long the app has to answer a launch once it was sent: it answers once
/// the run PTY is spawned.
pub const ACK_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a `hello` waits: the app answers it at once.
const HELLO_TIMEOUT: Duration = Duration::from_millis(500);

/// `<data dir>/app-bridge.sock`.
pub fn socket_path(data_dir: &Path) -> PathBuf {
    data_dir.join(SOCKET_NAME)
}

/// A launch handed to the app: it spawns `localify dev` for it in a run PTY,
/// fenced by the launch's id and expiry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LaunchRequest {
    /// The run's id and its launch record's.
    pub launch_id: RunId,
    /// Unix milliseconds: `--launch-expires`.
    pub expires_at: u64,
    pub service: ServiceRef,
    pub profile: Option<String>,
    /// The `configRev` a remote launch was checked against: `--expect-rev`.
    pub expect_rev: Option<String>,
    /// The run whose tab the new run takes, and which it replaces, as
    /// `--replace=<runId>`, once its dispatcher stopped it. `None` opens a
    /// new tab.
    pub replaces: Option<RunId>,
    /// A legacy run's `legacyId`, for a relaunch. The executor does not take
    /// `--replace-legacy` yet, so the app refuses one with `unsupported`.
    #[serde(default)]
    pub replaces_legacy: Option<String>,
    /// The paired machine that asked: a remote launch, run with
    /// `--remote-launch` and `--request`. `None` for a launch of this
    /// machine's own, such as `localify dev --in-app`, run with `--launch-id`.
    pub requested_by: Option<Requester>,
}

/// The service a launch runs, by its ids.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServiceRef {
    pub project_id: String,
    /// The environment's slug; `None` for the primary checkout.
    pub environment: Option<String>,
    pub service_id: String,
}

/// Who asked for a remote launch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Requester {
    pub machine_id: String,
    /// The machine's name, shown on the tab: "Started by <name>".
    pub name: String,
    pub request_id: String,
}

/// An answer's error: a code of docs/remote-machines.md, "Errors".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BridgeError {
    pub code: String,
    pub message: String,
}

/// The one JSON line the app answers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BridgeAnswer {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<BridgeError>,
}

/// What the app says of itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppHello {
    pub version: String,
    pub bridge: u32,
}

/// What became of a launch sent to the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Handoff {
    /// The app spawned the run PTY.
    Accepted,
    /// No app listens: nothing was sent, and the app runs nothing for it.
    Absent,
    /// The app refused it before spawning anything.
    Refused(BridgeError),
    /// Sent, and no answer came within [`ACK_TIMEOUT`], or the app closed
    /// the connection first: it may have spawned the run. Only the launch
    /// record can tell, once the launch has expired ([`settle_unanswered`]).
    Unanswered(String),
}

/// Whether the app runs, as its `hello` says. A missing socket, a refused
/// connection or no answer in time is an absent app.
pub fn probe(socket: &Path) -> AppPresence {
    match hello(socket) {
        Some(_) => AppPresence::Running,
        None => AppPresence::Absent,
    }
}

/// The app's `hello`, or `None` when it does not answer one.
pub fn hello(socket: &Path) -> Option<AppHello> {
    let mut stream = UnixStream::connect(socket).ok()?;
    let answer = exchange(
        &mut stream,
        &serde_json::json!({ "cmd": "hello" }),
        HELLO_TIMEOUT,
    )
    .ok()?;
    serde_json::from_value(answer.data?).ok()
}

/// Hands `launch` to the app and waits up to [`ACK_TIMEOUT`] for its
/// answer. Never retried: an unanswered launch may have started.
pub fn launch(socket: &Path, launch: &LaunchRequest) -> Handoff {
    let Ok(mut stream) = UnixStream::connect(socket) else {
        return Handoff::Absent;
    };
    let mut request = match serde_json::to_value(launch) {
        Ok(request) => request,
        Err(error) => return Handoff::Unanswered(error.to_string()),
    };
    request["cmd"] = serde_json::json!("launch");
    match exchange(&mut stream, &request, ACK_TIMEOUT) {
        Ok(answer) if answer.ok => Handoff::Accepted,
        Ok(BridgeAnswer {
            error: Some(error), ..
        }) => Handoff::Refused(error),
        Ok(_) => Handoff::Unanswered("The app answered without saying why it refused.".into()),
        Err(error) => Handoff::Unanswered(error.to_string()),
    }
}

/// What happened to a launch the app never answered: once its expiry has
/// passed, the launch record is decided, `expired` when nothing admitted it
/// by then, so no run with that id can start any more, and the report says
/// what the app did. Never falls back to another start.
pub fn settle_unanswered(
    store: &RunningServiceStore,
    launch_id: &RunId,
    expires_at: u64,
) -> Result<LaunchReport> {
    loop {
        let now = unix_ms_now();
        if now > expires_at {
            break;
        }
        thread::sleep(Duration::from_millis((expires_at - now + 1).min(250)));
    }
    reconcile(store, &NativeProcesses, launch_id, Some(expires_at))
}

/// Sends one request line and reads one answer line within `timeout`.
fn exchange(
    stream: &mut UnixStream,
    request: &serde_json::Value,
    timeout: Duration,
) -> io::Result<BridgeAnswer> {
    stream.set_write_timeout(Some(timeout))?;
    stream.set_read_timeout(Some(timeout))?;
    stream.write_all(format!("{request}\n").as_bytes())?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    if line.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "The app closed the connection without answering.",
        ));
    }
    serde_json::from_str(&line).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixListener;
    use std::time::Instant;

    use super::*;

    fn request() -> LaunchRequest {
        LaunchRequest {
            launch_id: RunId::generate().unwrap(),
            expires_at: unix_ms_now() + 15_000,
            service: ServiceRef {
                project_id: "app".into(),
                environment: None,
                service_id: "web".into(),
            },
            profile: None,
            expect_rev: Some("0123456789abcdef".into()),
            replaces: None,
            replaces_legacy: None,
            requested_by: Some(Requester {
                machine_id: "machine-a".into(),
                name: "A".into(),
                request_id: "req-1".into(),
            }),
        }
    }

    /// A server that answers each connection with `answer`, or, with
    /// `None`, reads the request and never answers.
    fn serve(answer: Option<&'static str>) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let socket = socket_path(dir.path());
        let listener = UnixListener::bind(&socket).unwrap();
        thread::spawn(move || {
            let mut held = Vec::new();
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut line = String::new();
                BufReader::new(&stream).read_line(&mut line).unwrap();
                match answer {
                    Some(answer) => stream.write_all(answer.as_bytes()).unwrap(),
                    None => held.push(stream),
                }
            }
        });
        (dir, socket)
    }

    #[test]
    fn a_launch_is_sent_as_the_contract_names_it() {
        let launch = request();
        let value = serde_json::to_value(&launch).unwrap();
        for key in [
            "launchId",
            "expiresAt",
            "service",
            "profile",
            "expectRev",
            "replaces",
            "replacesLegacy",
            "requestedBy",
        ] {
            assert!(value.get(key).is_some(), "{key} is missing from {value}");
        }
        assert_eq!(value["service"]["projectId"], "app");
        assert_eq!(value["requestedBy"]["requestId"], "req-1");
        let back: LaunchRequest = serde_json::from_value(value).unwrap();
        assert_eq!(back, launch);
    }

    #[test]
    fn a_missing_socket_is_an_absent_app() {
        let dir = tempfile::tempdir().unwrap();
        let socket = socket_path(dir.path());

        assert_eq!(probe(&socket), AppPresence::Absent);
        assert_eq!(launch(&socket, &request()), Handoff::Absent);
    }

    #[test]
    fn hello_and_launch_read_the_app_s_answers() {
        let (_dir, socket) = serve(Some(
            "{\"ok\":true,\"data\":{\"version\":\"1.6.0\",\"bridge\":1,\"accepted\":true}}\n",
        ));
        assert_eq!(probe(&socket), AppPresence::Running);
        assert_eq!(hello(&socket).unwrap().bridge, 1);
        assert_eq!(launch(&socket, &request()), Handoff::Accepted);

        let (_dir, socket) = serve(Some(
            "{\"ok\":false,\"error\":{\"code\":\"unsupported\",\"message\":\"no\"}}\n",
        ));
        assert_eq!(
            launch(&socket, &request()),
            Handoff::Refused(BridgeError {
                code: "unsupported".into(),
                message: "no".into()
            })
        );
    }

    #[test]
    fn a_launch_with_no_answer_in_five_seconds_is_unanswered() {
        let (_dir, socket) = serve(None);
        let started = Instant::now();

        let handoff = launch(&socket, &request());

        let waited = started.elapsed();
        assert!(matches!(handoff, Handoff::Unanswered(_)), "{handoff:?}");
        assert!(waited >= ACK_TIMEOUT, "gave up after {waited:?}");
        assert!(waited < ACK_TIMEOUT + Duration::from_secs(2), "{waited:?}");
        assert_eq!(
            probe(&socket),
            AppPresence::Absent,
            "an app that never answers is absent"
        );
    }

    #[test]
    fn an_unanswered_launch_is_settled_after_its_expiry_as_expired() {
        let dir = tempfile::tempdir().unwrap();
        let store = RunningServiceStore::new(dir.path().join("services"));
        let id = RunId::generate().unwrap();
        let expires_at = unix_ms_now() + 300;

        let report = settle_unanswered(&store, &id, expires_at).unwrap();

        assert!(unix_ms_now() > expires_at);
        assert_eq!(report, LaunchReport::Expired);
    }
}
