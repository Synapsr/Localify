//! The app bridge (docs/remote-machines.md, "The app bridge"): the Unix
//! socket the desktop app listens on while it runs,
//! `<data dir>/app-bridge/app-bridge.sock`, through which the daemon and
//! `localify dev --in-app` hand it a launch to run in a service tab. One JSON
//! request per connection, one JSON answer, as on the management socket.
//!
//! This is the protocol and its client. The app's side is
//! `localify::desktop::app_bridge`.

use std::fs;
use std::io::{self, Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use socket2::{Domain, SockAddr, Socket, Type};

use crate::infrastructure::process::{NativeProcesses, effective_uid};
use crate::infrastructure::remote::wire::AppPresence;
use crate::infrastructure::running_services::RunningServiceStore;
use crate::infrastructure::runs::RunId;
use crate::infrastructure::runs::launches::{LaunchReport, reconcile, unix_ms_now};

/// The bridge's own folder in the data dir, which the app makes `0700`: the
/// data dir itself may be shared, and is never changed.
pub const DIR_NAME: &str = "app-bridge";
/// In the bridge's folder.
pub const SOCKET_NAME: &str = "app-bridge.sock";
/// How long the app has to answer a launch once the client starts
/// connecting: it answers once the run PTY is spawned.
pub const ACK_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a `hello` takes at most, connecting included: the app answers
/// it at once.
const HELLO_TIMEOUT: Duration = Duration::from_millis(500);
/// The longest answer line the client reads. The app's are a few dozen
/// bytes.
const MAX_ANSWER_BYTES: usize = 16 * 1024;

/// `<data dir>/app-bridge`.
pub fn bridge_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(DIR_NAME)
}

/// `<data dir>/app-bridge/app-bridge.sock`.
pub fn socket_path(data_dir: &Path) -> PathBuf {
    bridge_dir(data_dir).join(SOCKET_NAME)
}

/// Whether an app of this user can listen on the bridge in `data_dir`: its
/// folder is this user's, or does not exist yet in a data dir this user
/// owns and may write, where the app makes it. Not on a layout whose data
/// dir belongs to root, such as `/etc/localify`: there the app has no
/// bridge, and `dev --in-app` opens the app with its deep link instead.
pub fn available(data_dir: &Path) -> bool {
    let uid = effective_uid();
    match fs::symlink_metadata(bridge_dir(data_dir)) {
        Ok(metadata) => metadata.is_dir() && metadata.uid() == uid,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::metadata(data_dir).is_ok_and(|metadata| {
                metadata.is_dir() && metadata.uid() == uid && metadata.mode() & 0o300 == 0o300
            })
        }
        Err(_) => false,
    }
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
    /// A legacy run's `legacyId`, for a relaunch its dispatcher stopped it
    /// for: `--replace-legacy`, in a new tab.
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

/// The app's `hello`, or `None` when it does not answer one within half a
/// second, connecting included.
pub fn hello(socket: &Path) -> Option<AppHello> {
    let deadline = Instant::now() + HELLO_TIMEOUT;
    let mut stream = connect(socket, deadline).ok()?;
    let answer = exchange(
        &mut stream,
        &serde_json::json!({ "cmd": "hello" }),
        deadline,
    )
    .ok()?;
    serde_json::from_value(answer.data?).ok()
}

/// Hands `launch` to the app and waits for its answer, [`ACK_TIMEOUT`] at
/// most from the start of the connect. Never retried: an unanswered launch
/// may have started.
pub fn launch(socket: &Path, launch: &LaunchRequest) -> Handoff {
    let deadline = Instant::now() + ACK_TIMEOUT;
    // Nothing was sent until the connection is made.
    let Ok(mut stream) = connect(socket, deadline) else {
        return Handoff::Absent;
    };
    let mut request = match serde_json::to_value(launch) {
        Ok(request) => request,
        Err(error) => return Handoff::Unanswered(error.to_string()),
    };
    request["cmd"] = serde_json::json!("launch");
    match exchange(&mut stream, &request, deadline) {
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

/// Connects without blocking past `deadline`: a suspended app whose backlog
/// is full would otherwise hold a plain connect forever.
fn connect(socket: &Path, deadline: Instant) -> io::Result<UnixStream> {
    let address = SockAddr::unix(socket)?;
    let raw = Socket::new(Domain::UNIX, Type::STREAM, None)?;
    raw.connect_timeout(&address, left(deadline)?)?;
    raw.set_nonblocking(false)?;
    Ok(UnixStream::from(OwnedFd::from(raw)))
}

/// What is left until `deadline`, or a timeout once it passed.
fn left(deadline: Instant) -> io::Result<Duration> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "The app did not answer in time.",
        ));
    }
    Ok(left)
}

/// Sends one request line and reads one answer line, both before
/// `deadline`: each read and write waits only for what is left of it, so an
/// app that answers a byte at a time cannot stretch it, and an answer longer
/// than [`MAX_ANSWER_BYTES`] is refused.
fn exchange(
    stream: &mut UnixStream,
    request: &serde_json::Value,
    deadline: Instant,
) -> io::Result<BridgeAnswer> {
    let request = format!("{request}\n").into_bytes();
    let mut sent = 0;
    while sent < request.len() {
        stream.set_write_timeout(Some(left(deadline)?))?;
        match stream.write(&request[sent..]) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(written) => sent += written,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    let line = read_line(stream, deadline)?;
    serde_json::from_slice(&line).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// One answer line, without its newline.
fn read_line(stream: &mut UnixStream, deadline: Instant) -> io::Result<Vec<u8>> {
    let mut line = Vec::new();
    let mut chunk = [0; 1024];
    loop {
        stream.set_read_timeout(Some(left(deadline)?))?;
        let read = match stream.read(&mut chunk) {
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if read == 0 {
            break;
        }
        line.extend_from_slice(&chunk[..read]);
        let end = line.iter().position(|&byte| byte == b'\n');
        if let Some(end) = end {
            line.truncate(end);
        }
        if line.len() > MAX_ANSWER_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("The app's answer is longer than {MAX_ANSWER_BYTES} bytes."),
            ));
        }
        if end.is_some() {
            return Ok(line);
        }
    }
    if line.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "The app closed the connection without answering.",
        ));
    }
    Ok(line)
}

#[cfg(test)]
mod tests {
    use std::io::BufRead;
    use std::os::unix::net::UnixListener;

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
        serve_with(move |mut stream| match answer {
            Some(answer) => stream.write_all(answer.as_bytes()).unwrap(),
            // Held open, unanswered, until the test ends.
            None => thread::sleep(Duration::from_secs(60)),
        })
    }

    /// A server that reads each connection's request line, then hands the
    /// connection to `then` on a thread of its own.
    fn serve_with(
        then: impl Fn(UnixStream) + Send + Sync + Copy + 'static,
    ) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(bridge_dir(dir.path())).unwrap();
        let socket = socket_path(dir.path());
        let listener = UnixListener::bind(&socket).unwrap();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let stream = stream.unwrap();
                let mut line = String::new();
                std::io::BufReader::new(&stream)
                    .read_line(&mut line)
                    .unwrap();
                thread::spawn(move || then(stream));
            }
        });
        (dir, socket)
    }

    /// Runs `call` on a thread, and fails the test when it has not returned
    /// within `bound`, instead of hanging it.
    fn within<T: Send + 'static>(bound: Duration, call: impl FnOnce() -> T + Send + 'static) -> T {
        let (sender, receiver) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let _ = sender.send(call());
        });
        receiver
            .recv_timeout(bound)
            .unwrap_or_else(|_| panic!("still waiting after {bound:?}"))
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
        // Bounded, not exact: a loaded test run delays the thread that gives
        // up, so only a wait far past the timeout means it never gave up.
        assert!(waited < ACK_TIMEOUT + Duration::from_secs(10), "{waited:?}");
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

    #[test]
    fn an_answer_trickled_a_byte_at_a_time_ends_at_the_deadline() {
        // Each byte comes before a per-read timeout would fire, so only an
        // overall deadline ends it.
        let (_dir, socket) = serve_with(|mut stream| {
            while stream.write_all(b" ").is_ok() {
                thread::sleep(Duration::from_millis(50));
            }
        });
        let started = Instant::now();

        let presence = within(Duration::from_secs(3), move || probe(&socket));

        assert_eq!(presence, AppPresence::Absent);
        assert!(started.elapsed() < HELLO_TIMEOUT + Duration::from_millis(500));
    }

    #[test]
    fn an_endless_answer_is_cut_at_its_cap() {
        let (_dir, socket) = serve_with(|mut stream| {
            let chunk = [b'x'; 4096];
            while stream.write_all(&chunk).is_ok() {}
        });
        let started = Instant::now();

        let handoff = within(Duration::from_secs(4), move || launch(&socket, &request()));

        let Handoff::Unanswered(reason) = handoff else {
            panic!("{handoff:?}");
        };
        assert!(reason.contains("longer than"), "{reason}");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "read past the cap"
        );
    }

    #[test]
    fn a_listener_that_never_accepts_is_absent_in_time() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(bridge_dir(dir.path())).unwrap();
        let socket = socket_path(dir.path());
        // Bound with the smallest backlog and never accepted: a suspended app.
        let listener = Socket::new(Domain::UNIX, Type::STREAM, None).unwrap();
        listener.bind(&SockAddr::unix(&socket).unwrap()).unwrap();
        listener.listen(0).unwrap();
        let mut queued = Vec::new();
        for _ in 0..64 {
            let client = Socket::new(Domain::UNIX, Type::STREAM, None).unwrap();
            client.set_nonblocking(true).unwrap();
            if client.connect(&SockAddr::unix(&socket).unwrap()).is_err() {
                break;
            }
            queued.push(client);
        }
        let started = Instant::now();

        let presence = within(Duration::from_secs(3), move || probe(&socket));

        assert_eq!(presence, AppPresence::Absent);
        assert!(started.elapsed() < HELLO_TIMEOUT + Duration::from_millis(500));
        drop((listener, queued));
    }

    #[test]
    fn the_bridge_is_available_only_where_this_user_can_make_it() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        assert!(!available(&data), "no data dir yet");
        fs::create_dir(&data).unwrap();
        assert!(available(&data), "a data dir of this user's");
        fs::create_dir(bridge_dir(&data)).unwrap();
        fs::set_permissions(&data, fs::Permissions::from_mode(0o555)).unwrap();
        assert!(available(&data), "the folder is made already");
        fs::set_permissions(&data, fs::Permissions::from_mode(0o755)).unwrap();
        fs::remove_dir(bridge_dir(&data)).unwrap();

        // As `/etc/localify` is to a desktop user: not writable.
        fs::set_permissions(&data, fs::Permissions::from_mode(0o555)).unwrap();
        let read_only = available(&data);
        fs::set_permissions(&data, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!read_only);

        let elsewhere = dir.path().join("elsewhere");
        fs::create_dir(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, bridge_dir(&data)).unwrap();
        assert!(!available(&data), "a link is no folder of the bridge's");
    }
}
