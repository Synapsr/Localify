//! The app's side of the app bridge (docs/remote-machines.md, "The app
//! bridge"): while the desktop app runs, it listens on
//! `<data dir>/app-bridge/app-bridge.sock`, and runs each launch handed to it
//! in a service tab, through the same run-PTY path as its launch button. The
//! protocol and the client are in `crate::infrastructure::app_bridge`.
//!
//! Only the app's own user can open the socket: it is `0600`, in a folder of
//! the bridge's own that the app makes `0700`. The data dir around it is
//! never changed, since a layout may share it, such as `/etc/localify`; where
//! the app cannot make that folder, it has no bridge. A lock file in the
//! folder, held while the app runs, makes one app the bridge's owner. One
//! JSON request per connection, one JSON answer; a launch is answered once
//! its run PTY is spawned, so `localify dev` then decides, under the
//! service's lock and its launch record, whether it starts.

use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::desktop::DesktopApi;
use crate::desktop::run_pty::{RemoteLaunch, RunLaunch};
use crate::desktop::terminal::{
    DispatchFailure, DispatchedLaunch, TerminalManager, TerminalTargetKind, TerminalTargetRequest,
};
use crate::domain::launch_profile::LaunchProfileName;
use crate::infrastructure::app_bridge::{
    AppHello, BridgeAnswer, BridgeError, LaunchRequest, bridge_dir, socket_path,
};
use crate::infrastructure::process::effective_uid;
use crate::infrastructure::runs::launches::{RequestedBy, unix_ms_now};
use crate::infrastructure::test_pause::pause_at;

/// What `hello` answers in `bridge`.
pub const BRIDGE_VERSION: u32 = 1;
/// In the bridge's folder: held by the app that owns the bridge.
const LOCK_NAME: &str = "app-bridge.lock";
/// The longest request line the app reads.
const MAX_REQUEST_BYTES: usize = 16 * 1024;
/// Connections served at once; more are closed as they come.
const MAX_HANDLERS: usize = 16;
/// How long the app waits for a client's request line.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// A launch whose expiry is further away than this is out of bounds.
const MAX_EXPIRY_AHEAD_MS: u64 = 60_000;
/// The longest machine name the tab shows.
const MAX_NAME_CHARS: usize = 64;

/// Runs one launch handed to the app, and returns once its run PTY is
/// spawned.
pub type Launcher = dyn Fn(LaunchRequest) -> Result<(), BridgeError> + Send + Sync;

/// The bridge's socket, listened on until this is dropped, which removes it.
pub struct AppBridge {
    path: PathBuf,
    /// The socket's inode, so only the socket this bridge bound is removed.
    inode: (u64, u64),
    closed: Arc<AtomicBool>,
    /// Held while the bridge is this app's; released after the socket is
    /// removed.
    _lock: File,
}

impl AppBridge {
    /// Listens on `<data_dir>/app-bridge/app-bridge.sock`, making the
    /// bridge's folder first; fails where it cannot, as on a data dir of
    /// root's. Under the folder's lock, a socket a crashed app left is
    /// replaced; another app holding the lock owns the bridge, and is left
    /// alone, and so is anything that is not a socket.
    pub fn listen(data_dir: &Path, launcher: Arc<Launcher>) -> Result<Self> {
        let dir = bridge_dir(data_dir);
        make_bridge_dir(data_dir, &dir)?;
        let lock = take_lock(&dir.join(LOCK_NAME))?;
        let path = socket_path(data_dir);
        remove_stale_socket(&path)?;
        let listener = UnixListener::bind(&path)
            .with_context(|| format!("Failed to listen on '{}'", path.display()))?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("Failed to make '{}' private", path.display()))?;
        let metadata = fs::symlink_metadata(&path)
            .with_context(|| format!("Failed to read '{}'", path.display()))?;
        let closed = Arc::new(AtomicBool::new(false));
        let accepting = Arc::clone(&closed);
        let handlers = Arc::new(AtomicUsize::new(0));
        thread::Builder::new()
            .name("app-bridge-accept".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    if accepting.load(Ordering::SeqCst) {
                        return;
                    }
                    let Ok(stream) = stream else {
                        continue;
                    };
                    // A bounded number of handlers, each on a thread that may fail
                    // to start: one more connection is then closed, and the
                    // listener goes on.
                    if handlers.fetch_add(1, Ordering::SeqCst) >= MAX_HANDLERS {
                        handlers.fetch_sub(1, Ordering::SeqCst);
                        continue;
                    }
                    let launcher = Arc::clone(&launcher);
                    let running = Arc::clone(&handlers);
                    let spawned =
                        thread::Builder::new()
                            .name("app-bridge".into())
                            .spawn(move || {
                                let _slot = HandlerSlot(running);
                                serve(stream, launcher.as_ref());
                            });
                    if spawned.is_err() {
                        handlers.fetch_sub(1, Ordering::SeqCst);
                    }
                }
            })
            .context("Failed to start the app bridge")?;
        Ok(Self {
            path,
            inode: (metadata.dev(), metadata.ino()),
            closed,
            _lock: lock,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Frees a handler's place when its thread ends, a panic included.
struct HandlerSlot(Arc<AtomicUsize>);

impl Drop for HandlerSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Drop for AppBridge {
    /// Stops accepting and removes the socket, unless another app replaced
    /// it meanwhile. The lock goes after, once the socket is gone.
    fn drop(&mut self) {
        self.closed.store(true, Ordering::SeqCst);
        // Wakes the accepting thread, which then sees it is closed.
        let _ = UnixStream::connect(&self.path);
        let ours = fs::symlink_metadata(&self.path)
            .is_ok_and(|metadata| (metadata.dev(), metadata.ino()) == self.inode);
        if ours {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// The launcher the app runs: each launch in a service tab of `manager`.
pub fn terminal_launcher(manager: TerminalManager, api: DesktopApi) -> Arc<Launcher> {
    Arc::new(move |request| launch_in_tab(&manager, &api, request))
}

fn launch_in_tab(
    manager: &TerminalManager,
    api: &DesktopApi,
    request: LaunchRequest,
) -> Result<(), BridgeError> {
    let environment_slug = match request.service.environment {
        Some(slug) => slug,
        None => primary_slug(api, &request.service.project_id)?,
    };
    let launch = RunLaunch {
        id: request.launch_id,
        expires_at: request.expires_at,
        replaces: request.replaces,
        replaces_legacy: request.replaces_legacy,
        remote: match (&request.requested_by, request.expect_rev) {
            (Some(requester), Some(expect_rev)) => Some(RemoteLaunch {
                expect_rev,
                requested_by: RequestedBy {
                    machine_id: requester.machine_id.clone(),
                    request_id: requester.request_id.clone(),
                },
            }),
            _ => None,
        },
    };
    let dispatched = DispatchedLaunch {
        target: TerminalTargetRequest {
            kind: TerminalTargetKind::Service,
            project_id: request.service.project_id,
            environment_slug,
            service_id: Some(request.service.service_id),
        },
        profile: request.profile,
        launch,
        started_by: request.requested_by.map(|requester| requester.name),
    };
    match manager.launch_dispatched(api, dispatched) {
        Ok(_) => Ok(()),
        Err(DispatchFailure::UnknownService(message)) => Err(error("unknown_service", message)),
        // Whatever stops a launch before its admission is a conflict.
        Err(DispatchFailure::Failed(message)) => Err(error("conflict", message)),
    }
}

/// The slug of the project's primary checkout.
fn primary_slug(api: &DesktopApi, project_id: &str) -> Result<String, BridgeError> {
    let bootstrap = api
        .bootstrap()
        .map_err(|failure| error("conflict", format!("{failure:#}")))?;
    bootstrap
        .project_environments
        .into_iter()
        .find(|environment| environment.project_id == project_id && environment.primary)
        .map(|environment| environment.slug)
        .ok_or_else(|| {
            error(
                "unknown_service",
                format!("Project '{project_id}' has no primary checkout on this machine."),
            )
        })
}

/// Answers one connection: one request line, one answer line.
fn serve(stream: UnixStream, launcher: &Launcher) {
    let _ = stream.set_read_timeout(Some(REQUEST_TIMEOUT));
    let mut line = String::new();
    let read = BufReader::new((&stream).take(MAX_REQUEST_BYTES as u64 + 1)).read_line(&mut line);
    let answer = match read {
        Ok(_) if line.len() > MAX_REQUEST_BYTES => Err(error(
            "bad_request",
            format!("The request is longer than {MAX_REQUEST_BYTES} bytes."),
        )),
        Ok(_) => answer(&line, launcher),
        Err(failure) => Err(error(
            "bad_request",
            format!("Failed to read the request: {failure}"),
        )),
    };
    let answer = match answer {
        Ok(data) => BridgeAnswer {
            ok: true,
            data: Some(data),
            error: None,
        },
        Err(error) => BridgeAnswer {
            ok: false,
            data: None,
            error: Some(error),
        },
    };
    if let Ok(text) = serde_json::to_string(&answer) {
        let _ = (&stream).write_all(format!("{text}\n").as_bytes());
    }
}

fn answer(line: &str, launcher: &Launcher) -> Result<Value, BridgeError> {
    match parse(line)? {
        Request::Hello => serde_json::to_value(AppHello {
            version: env!("CARGO_PKG_VERSION").to_string(),
            bridge: BRIDGE_VERSION,
        })
        .map_err(|failure| error("bad_request", failure.to_string())),
        Request::Launch(launch) => {
            validate(&launch, unix_ms_now())?;
            pause_at("app-bridge-launch");
            launcher(*launch)?;
            pause_at("app-bridge-spawned");
            Ok(json!({ "accepted": true }))
        }
    }
}

#[derive(Debug)]
enum Request {
    Hello,
    Launch(Box<LaunchRequest>),
}

fn parse(line: &str) -> Result<Request, BridgeError> {
    let value: Value = serde_json::from_str(line)
        .map_err(|failure| error("bad_request", format!("The request is not JSON: {failure}")))?;
    let Value::Object(mut fields) = value else {
        return Err(error("bad_request", "The request is not a JSON object."));
    };
    let command = match fields.remove("cmd") {
        Some(Value::String(command)) => command,
        _ => return Err(error("bad_request", "The request has no \"cmd\".")),
    };
    match command.as_str() {
        "hello" => Ok(Request::Hello),
        "launch" => serde_json::from_value(Value::Object(fields))
            .map(|launch| Request::Launch(Box::new(launch)))
            .map_err(|failure| error("bad_request", format!("Malformed launch: {failure}"))),
        other => Err(error(
            "unsupported",
            format!("This version of the Localify app does not know '{other}'."),
        )),
    }
}

/// Refuses what this app cannot run, before anything is spawned.
fn validate(launch: &LaunchRequest, now: u64) -> Result<(), BridgeError> {
    if let Some(legacy_id) = &launch.replaces_legacy {
        let shaped = (1..=128).contains(&legacy_id.len())
            && legacy_id.split('.').count() >= 3
            && legacy_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-.".contains(&byte));
        if !shaped || launch.replaces.is_some() || launch.requested_by.is_none() {
            return Err(error(
                "bad_request",
                "replacesLegacy names a legacy run (<boot id>.<pid>.<start>) for a remote relaunch, without replaces.",
            ));
        }
    }
    if launch.expires_at <= now {
        return Err(error(
            "expired",
            "The launch expired before it reached the app; nothing was started.",
        ));
    }
    if launch.expires_at > now + MAX_EXPIRY_AHEAD_MS {
        return Err(error(
            "bad_request",
            "The launch expires more than 60 seconds from now.",
        ));
    }
    if let Some(profile) = &launch.profile {
        LaunchProfileName::new(profile)
            .map_err(|failure| error("bad_request", failure.to_string()))?;
    }
    match (&launch.requested_by, &launch.expect_rev) {
        (None, None) => Ok(()),
        (Some(requester), Some(expect_rev)) => {
            RequestedBy::parse(&format!(
                "{}:{}",
                requester.machine_id, requester.request_id
            ))
            .map_err(|failure| error("bad_request", failure.to_string()))?;
            let name_chars = requester.name.chars().count();
            if name_chars == 0
                || name_chars > MAX_NAME_CHARS
                || requester.name.chars().any(char::is_control)
            {
                return Err(error(
                    "bad_request",
                    "The requesting machine's name is empty, too long, or has control characters.",
                ));
            }
            if expect_rev.len() != 16 || !expect_rev.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(error(
                    "bad_request",
                    "expectRev is not a configRev (16 hex digits).",
                ));
            }
            Ok(())
        }
        _ => Err(error(
            "bad_request",
            "A remote launch has both requestedBy and expectRev; a local one has neither.",
        )),
    }
}

fn error(code: &str, message: impl Into<String>) -> BridgeError {
    BridgeError {
        code: code.to_string(),
        message: message.into(),
    }
}

/// Makes the bridge's folder `dir`, in `data_dir`, enterable by this user
/// only. The data dir is made when missing, and its permissions are never
/// changed. A folder that is a symbolic link, or another user's, is refused.
fn make_bridge_dir(data_dir: &Path, dir: &Path) -> Result<()> {
    // Its permissions are whatever the layout gave it, or the umask's.
    let _ = fs::create_dir_all(data_dir);
    match DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(failure) if failure.kind() == io::ErrorKind::AlreadyExists => {}
        Err(failure) => {
            return Err(failure).with_context(|| {
                format!(
                    "Failed to make '{}', so the app bridge is unavailable on this layout: is '{}' another user's?",
                    dir.display(),
                    data_dir.display()
                )
            });
        }
    }
    let metadata =
        fs::symlink_metadata(dir).with_context(|| format!("Failed to read '{}'", dir.display()))?;
    if !metadata.is_dir() || metadata.uid() != effective_uid() {
        bail!(
            "'{}' is not a folder of this user, so the app bridge cannot listen in it.",
            dir.display()
        );
    }
    if metadata.mode() & 0o077 != 0 {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("Failed to make '{}' private", dir.display()))?;
    }
    Ok(())
}

/// Takes the bridge's lock for as long as the returned file is open. The
/// file stays: only the lock on it comes and goes.
fn take_lock(path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("Failed to open '{}'", path.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(fs::TryLockError::WouldBlock) => bail!(
            "Another Localify app owns the app bridge ('{}' is locked).",
            path.display()
        ),
        Err(fs::TryLockError::Error(failure)) => {
            Err(failure).with_context(|| format!("Failed to lock '{}'", path.display()))
        }
    }
}

/// Removes a socket nobody listens on, which only a refused connection
/// proves. Fails when an app still answers on it, when connecting fails in
/// any other way, or when something other than a socket is there. Called
/// with the bridge's lock held, so no other app is between this and its
/// bind.
fn remove_stale_socket(path: &Path) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(failure) if failure.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(failure) => {
            return Err(failure).with_context(|| format!("Failed to read '{}'", path.display()));
        }
    };
    if !metadata.file_type().is_socket() {
        bail!(
            "'{}' is not a socket. Remove it, then open Localify again.",
            path.display()
        );
    }
    match UnixStream::connect(path) {
        Ok(_) => bail!(
            "Another Localify app already listens on '{}'.",
            path.display()
        ),
        Err(failure) if failure.kind() == io::ErrorKind::ConnectionRefused => {
            fs::remove_file(path).with_context(|| format!("Failed to remove '{}'", path.display()))
        }
        Err(failure) => Err(failure).with_context(|| {
            format!(
                "Failed to tell whether an app still listens on '{}'",
                path.display()
            )
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::infrastructure::app_bridge::{
        self as client, Handoff, Requester, ServiceRef, hello,
    };
    use crate::infrastructure::runs::RunId;

    fn launch() -> LaunchRequest {
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

    /// A bridge whose launcher records what it was handed.
    fn bridge() -> (tempfile::TempDir, AppBridge, Arc<Mutex<Vec<LaunchRequest>>>) {
        let dir = tempfile::tempdir().unwrap();
        let launched = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&launched);
        let bridge = AppBridge::listen(
            &dir.path().join("data"),
            Arc::new(move |request| {
                recorded.lock().unwrap().push(request);
                Ok(())
            }),
        )
        .unwrap();
        (dir, bridge, launched)
    }

    fn raw(path: &Path, line: &str) -> BridgeAnswer {
        let mut stream = UnixStream::connect(path).unwrap();
        stream.write_all(line.as_bytes()).unwrap();
        let mut answer = String::new();
        BufReader::new(stream).read_line(&mut answer).unwrap();
        serde_json::from_str(&answer).unwrap()
    }

    fn code(answer: &BridgeAnswer) -> &str {
        assert!(!answer.ok, "{answer:?}");
        &answer.error.as_ref().unwrap().code
    }

    #[test]
    fn only_its_owner_can_reach_the_socket() {
        let (dir, bridge, _) = bridge();

        let socket = fs::symlink_metadata(bridge.path()).unwrap();
        assert!(socket.file_type().is_socket());
        assert_eq!(socket.mode() & 0o777, 0o600);
        assert_eq!(bridge.path(), socket_path(&dir.path().join("data")));
        let folder = fs::metadata(bridge_dir(&dir.path().join("data"))).unwrap();
        assert_eq!(folder.mode() & 0o777, 0o700);
    }

    #[test]
    fn the_data_dir_is_never_changed() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        fs::create_dir(&data).unwrap();
        fs::set_permissions(&data, fs::Permissions::from_mode(0o755)).unwrap();

        let _bridge = AppBridge::listen(&data, Arc::new(|_| Ok(()))).unwrap();

        assert_eq!(fs::metadata(&data).unwrap().mode() & 0o777, 0o755);
    }

    #[test]
    fn a_data_dir_this_user_cannot_write_has_no_bridge() {
        // As `/etc/localify` is to a desktop user: readable, not writable.
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        fs::create_dir(&data).unwrap();
        fs::set_permissions(&data, fs::Permissions::from_mode(0o555)).unwrap();

        let listening = AppBridge::listen(&data, Arc::new(|_| Ok(())));

        let mode = fs::metadata(&data).unwrap().mode() & 0o777;
        fs::set_permissions(&data, fs::Permissions::from_mode(0o755)).unwrap();
        let error = format!("{:#}", listening.err().expect("listened anyway"));
        assert!(error.contains("unavailable on this layout"), "{error}");
        assert_eq!(mode, 0o555, "the data dir was changed");
        assert!(!bridge_dir(&data).exists());
    }

    #[test]
    fn an_open_bridge_folder_is_made_private() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        fs::create_dir_all(bridge_dir(&data)).unwrap();
        fs::set_permissions(bridge_dir(&data), fs::Permissions::from_mode(0o755)).unwrap();

        let _bridge = AppBridge::listen(&data, Arc::new(|_| Ok(()))).unwrap();

        let mode = fs::metadata(bridge_dir(&data)).unwrap().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[test]
    fn a_bridge_folder_that_is_a_link_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let elsewhere = dir.path().join("elsewhere");
        fs::create_dir_all(&data).unwrap();
        fs::create_dir(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, bridge_dir(&data)).unwrap();

        assert!(AppBridge::listen(&data, Arc::new(|_| Ok(()))).is_err());
        assert!(fs::read_dir(&elsewhere).unwrap().next().is_none());
    }

    #[test]
    fn hello_says_the_app_runs_and_which_bridge() {
        let (_dir, bridge, _) = bridge();

        let hello = hello(bridge.path()).unwrap();

        assert_eq!(hello.bridge, BRIDGE_VERSION);
        assert_eq!(hello.version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn connections_past_the_limit_are_closed_and_the_bridge_goes_on() {
        let (_dir, bridge, _) = bridge();
        // Silent clients hold every handler.
        let silent: Vec<UnixStream> = (0..MAX_HANDLERS)
            .map(|_| UnixStream::connect(bridge.path()).unwrap())
            .collect();
        wait_for(|| {
            // One more is closed at once: it reads the end, not an answer.
            let mut extra = UnixStream::connect(bridge.path()).unwrap();
            extra
                .set_read_timeout(Some(Duration::from_millis(500)))
                .unwrap();
            let _ = extra.write_all(b"{\"cmd\":\"hello\"}\n");
            let mut answer = String::new();
            // Closed unread: macOS reads the end, Linux may report a reset.
            match extra.read_to_string(&mut answer) {
                Ok(_) => answer.is_empty(),
                Err(error) => matches!(
                    error.kind(),
                    io::ErrorKind::ConnectionReset | io::ErrorKind::BrokenPipe
                ),
            }
        });

        drop(silent);
        // Their handlers end, and the bridge answers again.
        wait_for(|| hello(bridge.path()).is_some());
    }

    fn wait_for(mut done: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(std::time::Instant::now() < deadline, "timed out");
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// A data dir holding what a crashed app leaves: its bridge's folder,
    /// and a socket nobody listens on.
    fn crashed_app_left() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(bridge_dir(&data))
            .unwrap();
        drop(UnixListener::bind(socket_path(&data)).unwrap());
        // A child another test is forking holds a copy of every descriptor
        // until it execs, the listener's included: stale once refused.
        let stale = || {
            UnixStream::connect(socket_path(&data))
                .is_err_and(|failure| failure.kind() == io::ErrorKind::ConnectionRefused)
        };
        eventually(stale);
        (dir, data)
    }

    /// Waits up to 2 seconds for `done`, for what a forking child of another
    /// test holds for a moment.
    fn eventually(mut done: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !done() {
            assert!(std::time::Instant::now() < deadline, "still not done");
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_stale_socket_is_replaced_and_a_live_one_kept() {
        let (_dir, data) = crashed_app_left();

        let bridge = AppBridge::listen(&data, Arc::new(|_| Ok(()))).unwrap();
        assert!(hello(bridge.path()).is_some());

        let second = AppBridge::listen(&data, Arc::new(|_| Ok(())));
        let error = format!(
            "{:#}",
            second.err().expect("took over a bridge another app owns")
        );
        assert!(error.contains("owns the app bridge"), "{error}");
        assert!(hello(bridge.path()).is_some());

        drop(bridge);
        assert!(!socket_path(&data).exists(), "the socket outlived its app");
        assert!(
            bridge_dir(&data).join(LOCK_NAME).exists(),
            "the lock file went"
        );
        // The lock is free again once a forking child let go of its copy.
        let mut next = None;
        eventually(|| {
            next = AppBridge::listen(&data, Arc::new(|_| Ok(()))).ok();
            next.is_some()
        });
        assert!(hello(&socket_path(&data)).is_some());
    }

    #[test]
    fn apps_starting_together_leave_one_bridge_that_answers() {
        for _ in 0..20 {
            let (_dir, data) = crashed_app_left();
            let start = Arc::new(std::sync::Barrier::new(8));
            let starts: Vec<_> = (0..8)
                .map(|_| {
                    let data = data.clone();
                    let start = Arc::clone(&start);
                    thread::spawn(move || {
                        start.wait();
                        AppBridge::listen(&data, Arc::new(|_| Ok(())))
                    })
                })
                .collect();
            let bridges: Vec<_> = starts
                .into_iter()
                .filter_map(|start| start.join().unwrap().ok())
                .collect();

            assert_eq!(
                bridges.len(),
                1,
                "{} apps think they own the bridge",
                bridges.len()
            );
            let socket = fs::symlink_metadata(socket_path(&data)).unwrap();
            assert_eq!(
                (socket.dev(), socket.ino()),
                bridges[0].inode,
                "the owner's socket was removed or replaced"
            );
        }
    }

    #[test]
    fn a_socket_that_cannot_be_probed_is_never_removed() {
        let (_dir, data) = crashed_app_left();
        // Connecting fails, but not with a refusal: nothing proves it stale.
        fs::set_permissions(socket_path(&data), fs::Permissions::from_mode(0o000)).unwrap();

        let listening = AppBridge::listen(&data, Arc::new(|_| Ok(())));

        let kept = fs::symlink_metadata(socket_path(&data)).is_ok();
        // Root connects whatever the mode says, and then sees it stale.
        if effective_uid() != 0 {
            assert!(listening.is_err());
            assert!(kept, "removed a socket it could not probe");
        }
    }

    #[test]
    fn what_is_not_a_socket_is_never_removed() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        fs::create_dir_all(bridge_dir(&data)).unwrap();
        fs::write(socket_path(&data), "mine").unwrap();

        assert!(AppBridge::listen(&data, Arc::new(|_| Ok(()))).is_err());
        assert_eq!(fs::read_to_string(socket_path(&data)).unwrap(), "mine");
    }

    #[test]
    fn a_launch_is_accepted_once_handed_to_the_launcher() {
        let (_dir, bridge, launched) = bridge();
        let request = launch();

        assert_eq!(client::launch(bridge.path(), &request), Handoff::Accepted);

        assert_eq!(*launched.lock().unwrap(), vec![request]);
    }

    #[test]
    fn malformed_requests_are_refused_and_start_nothing() {
        let (_dir, bridge, launched) = bridge();
        let mut unknown_field = serde_json::to_value(launch()).unwrap();
        unknown_field["cmd"] = json!("launch");
        unknown_field["command"] = json!(["rm", "-rf", "/"]);
        let mut bad_id = serde_json::to_value(launch()).unwrap();
        bad_id["cmd"] = json!("launch");
        bad_id["launchId"] = json!("../../etc");

        for line in [
            "not json\n".to_string(),
            "[1, 2]\n".to_string(),
            "{\"launchId\": \"x\"}\n".to_string(),
            format!("{unknown_field}\n"),
            format!("{bad_id}\n"),
            format!("{}\n", "x".repeat(MAX_REQUEST_BYTES + 10)),
        ] {
            let answer = raw(bridge.path(), &line);
            assert_eq!(code(&answer), "bad_request", "{line:.80}");
        }
        let answer = raw(bridge.path(), "{\"cmd\": \"format_disk\"}\n");
        assert_eq!(code(&answer), "unsupported");
        assert!(launched.lock().unwrap().is_empty());
    }

    #[test]
    fn launches_this_app_cannot_run_are_refused_with_their_code() {
        let now = unix_ms_now();
        let refused = |change: fn(&mut LaunchRequest)| {
            let mut request = launch();
            change(&mut request);
            validate(&request, now).unwrap_err().code
        };

        assert!(validate(&launch(), now).is_ok());
        let mut local = launch();
        local.requested_by = None;
        local.expect_rev = None;
        assert!(validate(&local, now).is_ok());
        let mut relaunch = launch();
        relaunch.replaces_legacy = Some("6E0C1F4A-0B6D-4C5E-9F3A-1D2B3C4D5E6F.42.7".into());
        assert!(validate(&relaunch, now).is_ok());
        for legacy_id in ["boot", "../x.1.2", ""] {
            let mut request = launch();
            request.replaces_legacy = Some(legacy_id.into());
            assert_eq!(validate(&request, now).unwrap_err().code, "bad_request");
        }
        let mut both = relaunch.clone();
        both.replaces = Some(RunId::generate().unwrap());
        assert_eq!(validate(&both, now).unwrap_err().code, "bad_request");
        assert_eq!(refused(|request| request.expires_at = 1), "expired");
        assert_eq!(
            refused(|request| request.expires_at = u64::MAX),
            "bad_request"
        );
        assert_eq!(refused(|request| request.expect_rev = None), "bad_request");
        assert_eq!(
            refused(|request| request.requested_by = None),
            "bad_request"
        );
        assert_eq!(
            refused(|request| request.expect_rev = Some("not-a-rev".into())),
            "bad_request"
        );
        assert_eq!(
            refused(|request| request.profile = Some("../prod".into())),
            "bad_request"
        );
        assert_eq!(
            refused(|request| {
                if let Some(requester) = request.requested_by.as_mut() {
                    requester.name = "A\u{1b}[2J".into();
                }
            }),
            "bad_request"
        );
        assert_eq!(
            refused(|request| {
                if let Some(requester) = request.requested_by.as_mut() {
                    requester.request_id = "a:b".into();
                }
            }),
            "bad_request"
        );
    }

    #[test]
    fn a_legacy_relaunch_reaches_the_launcher() {
        let (_dir, bridge, launched) = bridge();
        let mut request = launch();
        request.replaces_legacy = Some("boot.1.2".into());

        assert_eq!(client::launch(bridge.path(), &request), Handoff::Accepted);
        assert_eq!(*launched.lock().unwrap(), vec![request]);
    }
}
