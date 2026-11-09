//! The app bridge end to end (docs/remote-machines.md, "The app bridge", and
//! the PR 4 gate "A desktop disconnect cannot double-start").
//!
//! The app is this test binary started again as `app_helper`: it listens on
//! the bridge with the desktop app's own `AppBridge`, `TerminalManager` and
//! run-PTY spawn path, and runs the real `localify dev`. Only the Tauri
//! window around them is missing. The test is the dispatcher, through the
//! client the daemon uses, and kills the app where the gate says.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use localify::branding::BRANDING;
use localify::desktop::DesktopApi;
use localify::desktop::app_bridge::{AppBridge, terminal_launcher};
use localify::desktop::run_pty::AppInstallation;
use localify::desktop::shell_environment::ShellEnvironmentDto;
use localify::desktop::terminal::{
    TerminalEvents, TerminalExitPayload, TerminalManager, TerminalOpenedPayload,
    TerminalOutputPayload, TerminalSessionDto,
};
use localify::infrastructure::app_bridge::{
    self, Handoff, LaunchRequest, Requester, ServiceRef, settle_unanswered,
};
use localify::infrastructure::process::NativeProcesses;
use localify::infrastructure::running_services::{
    RunningService, RunningServiceStore, ServiceOrigin,
};
use localify::infrastructure::runs::RunId;
use localify::infrastructure::runs::launches::{
    AdmittedLaunch, LaunchRecord, LaunchReport, dispatch_expiry, reconcile, unix_ms_now,
};
use localify::infrastructure::test_pause::PAUSE_DIR_ENV;

const HELPER_ENV: &str = "APP_BRIDGE_HELPER_ROOT";

struct Sandbox {
    _dir: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    config: PathBuf,
}

impl Sandbox {
    /// `web` runs `script`, with `{port}` its port, once it appended a line
    /// to `starts`.
    fn new(script: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let (home, app, bin) = (root.join("home"), root.join("app"), root.join("bin"));
        for folder in [&home, &app, &bin, &root.join("pause")] {
            fs::create_dir_all(folder).unwrap();
        }
        let port = free_port();
        let config = root.join("config.toml");
        let script = format!(
            "echo started >> '{}'; {}",
            root.join("starts").display(),
            script.replace("{port}", &port.to_string())
        );
        fs::write(&config, config_toml(&root, &app, port, &script)).unwrap();
        // The user's shell, as the app resolves its environment through it.
        executable(
            &root.join("zsh"),
            &format!(
                "#!/bin/sh\nshift\nexport PATH=\"{}:/usr/bin:/bin\"\nexec /bin/sh -c \"$1\"\n",
                bin.display()
            ),
        );
        Self {
            _dir: dir,
            root,
            home,
            config,
        }
    }

    fn services_dir(&self) -> PathBuf {
        if cfg!(target_os = "macos") {
            self.home
                .join("Library/Application Support")
                .join(BRANDING.desktop_state_dir_name)
                .join("services")
        } else {
            self.root.join("state").join(BRANDING.slug).join("services")
        }
    }

    fn store(&self) -> RunningServiceStore {
        RunningServiceStore::new(self.services_dir())
    }

    fn socket(&self) -> PathBuf {
        app_bridge::socket_path(&self.root.join("data"))
    }

    /// The app, in a process of its own, once its bridge answers.
    fn start_app(&self) -> AppProcess {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "app_helper",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &self.home)
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env(HELPER_ENV, &self.root)
            .env(PAUSE_DIR_ENV, self.root.join("pause"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command.spawn().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, lines) = mpsc::channel();
        thread::spawn(move || {
            let mut text = String::new();
            let mut reader = std::io::BufReader::new(stdout);
            while std::io::BufRead::read_line(&mut reader, &mut text).unwrap_or(0) > 0 {
                // libtest prints "test app_helper ... " on the same line as the first one.
                if let Some(line) = text.find("HELPER ").map(|at| &text[at + 7..]) {
                    let _ = sender.send(line.trim_end().to_string());
                }
                text.clear();
            }
        });
        wait_until(Duration::from_secs(30), || {
            app_bridge::hello(&self.socket()).map(|_| ())
        })
        .expect("the app's bridge never answered");
        AppProcess { child, lines }
    }

    /// A launch of `web` as a paired machine asks for it, checked against
    /// revision `rev`.
    fn remote_launch(&self, rev: &str, replaces: Option<&RunId>) -> LaunchRequest {
        LaunchRequest {
            launch_id: RunId::generate().unwrap(),
            expires_at: dispatch_expiry(),
            service: ServiceRef {
                project_id: "app".into(),
                environment: None,
                service_id: "web".into(),
            },
            profile: None,
            expect_rev: Some(rev.to_string()),
            replaces: replaces.cloned(),
            replaces_legacy: None,
            requested_by: Some(Requester {
                machine_id: "machine-a".into(),
                name: "Machine A".into(),
                request_id: format!("req-{}", unix_ms_now()),
            }),
        }
    }

    /// `web`'s `configRev`, as the app's `localify dev` computes it: from
    /// its refusal of a launch checked against another one.
    fn config_rev(&self) -> String {
        let probe = self.remote_launch("0000000000000000", None);
        assert_eq!(
            app_bridge::launch(&self.socket(), &probe),
            Handoff::Accepted
        );
        let report = self.wait_report(&probe.launch_id, |report| {
            matches!(report, LaunchReport::Refused { .. })
        });
        let LaunchReport::Refused { message, .. } = report else {
            unreachable!()
        };
        message
            .split("(revision ")
            .nth(1)
            .and_then(|rest| rest.split(',').next())
            .unwrap_or_else(|| panic!("no revision in {message:?}"))
            .to_string()
    }

    fn arm(&self, point: &str) {
        fs::write(self.root.join("pause").join(point), "").unwrap();
    }

    fn wait_reached(&self, point: &str) {
        let reached = self.root.join("pause").join(format!("{point}.reached"));
        wait_until(Duration::from_secs(30), || reached.exists().then_some(()))
            .unwrap_or_else(|| panic!("the app never reached {point}"));
    }

    /// How many times `web` started.
    fn starts(&self) -> usize {
        fs::read_to_string(self.root.join("starts"))
            .map(|content| content.lines().count())
            .unwrap_or(0)
    }

    /// The run record of run `id`.
    fn record(&self, id: &RunId) -> RunningService {
        self.store()
            .list()
            .unwrap()
            .into_iter()
            .find(|record| record.run_id() == Some(id))
            .unwrap_or_else(|| panic!("run {id} has no record"))
    }

    fn report(&self, id: &RunId) -> LaunchReport {
        reconcile(&self.store(), &NativeProcesses, id, None).unwrap()
    }

    fn wait_report(&self, id: &RunId, wanted: fn(&LaunchReport) -> bool) -> LaunchReport {
        wait_until(Duration::from_secs(45), || {
            Some(self.report(id)).filter(wanted)
        })
        .unwrap_or_else(|| panic!("launch {id} never got there: {:?}", self.report(id)))
    }

    /// Asserts that nothing started in the background, and that at most one
    /// run of `web` exists, and returns that run's id.
    fn at_most_one_run(&self) -> Option<RunId> {
        let records = self.store().list().unwrap();
        assert!(records.len() <= 1, "{records:?}");
        assert!(
            records
                .iter()
                .all(|record| record.origin != ServiceOrigin::Background),
            "a background run was started: {records:?}"
        );
        assert!(self.starts() <= 1, "web started {} times", self.starts());
        let logs = fs::read_dir(self.services_dir().join("launches"))
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "log"))
                    .count()
            })
            .unwrap_or(0);
        assert_eq!(logs, 0, "a detached executor was started");
        records.first().and_then(|record| record.run_id().cloned())
    }

    fn stop_all(&self) {
        for record in self.store().list().unwrap() {
            let _ = self.store().stop(&record);
        }
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        self.stop_all();
    }
}

struct AppProcess {
    child: Child,
    lines: Receiver<String>,
}

impl AppProcess {
    /// What the app said when it opened the tab of launch `id`.
    fn opened(&self, id: &RunId) -> String {
        loop {
            let line = self
                .lines
                .recv_timeout(Duration::from_secs(30))
                .unwrap_or_else(|_| panic!("the app never opened a tab for {id}"));
            if line.starts_with("opened ") && line.contains(id.as_str()) {
                return line;
            }
        }
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for AppProcess {
    fn drop(&mut self) {
        self.kill();
    }
}

fn config_toml(root: &Path, app: &Path, port: u16, script: &str) -> String {
    let script = script.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        r#"[paths]
data_dir = "{root}/data"
pid_file = "{root}/localify.pid"
log_file = "{root}/logs/localify.log"
certs_dir = "{root}/data/certs"
socket_path = "{root}/localify.sock"

[domains."web.localify"]
pattern = "web.localify"
https_enabled = false
project = "app"

[[domains."web.localify".routes]]
path = "/"
target = "127.0.0.1:{port}"

[projects.app]
name = "app"
enabled = true
root_path = "{app}"

[[projects.app.services]]
name = "web"
primary_domain = "web.localify"
command = ["sh", "-c", "{script}"]
"#,
        root = root.display(),
        app = app.display(),
    )
}

fn executable(path: &Path, content: &str) {
    fs::write(path, content).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn wait_until<T>(timeout: Duration, mut check: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(value) = check() {
            return Some(value);
        }
        if Instant::now() >= deadline {
            return None;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn admitted(record: Option<LaunchRecord>) -> Box<AdmittedLaunch> {
    match record {
        Some(LaunchRecord::Admitted(admitted)) => admitted,
        other => panic!("not admitted: {other:?}"),
    }
}

/// Sends `request` from another thread, as the daemon would.
fn dispatch(sandbox: &Sandbox, request: &LaunchRequest) -> thread::JoinHandle<Handoff> {
    let (socket, request) = (sandbox.socket(), request.clone());
    thread::spawn(move || app_bridge::launch(&socket, &request))
}

#[test]
fn a_launch_handed_to_the_app_runs_in_a_tab_and_its_restart_takes_that_tab_over() {
    let sandbox = Sandbox::new("echo ready; exec sleep 300");
    let app = sandbox.start_app();
    let rev = sandbox.config_rev();

    let first = sandbox.remote_launch(&rev, None);
    assert_eq!(
        app_bridge::launch(&sandbox.socket(), &first),
        Handoff::Accepted
    );
    let opened = app.opened(&first.launch_id);
    assert!(
        opened.ends_with("replaced=None by=Some(\"Machine A\")"),
        "{opened}"
    );
    sandbox.wait_report(&first.launch_id, |report| *report == LaunchReport::Running);
    let record = sandbox.record(&first.launch_id);
    assert_eq!(record.origin, ServiceOrigin::App);
    let launch = admitted(sandbox.store().launches().read(&first.launch_id).unwrap());
    let requested_by = launch.requested_by.expect("no requestedBy");
    assert_eq!(requested_by.machine_id, "machine-a");
    assert_eq!(
        requested_by.request_id,
        first.requested_by.as_ref().unwrap().request_id
    );

    // A remote restart: the daemon stops the run, then hands the app the
    // launch that replaces it.
    assert!(sandbox.store().stop(&record).unwrap());
    let second = sandbox.remote_launch(&rev, Some(&first.launch_id));
    assert_eq!(
        app_bridge::launch(&sandbox.socket(), &second),
        Handoff::Accepted
    );
    let opened = app.opened(&second.launch_id);
    assert!(!opened.contains("replaced=None"), "{opened}");
    sandbox.wait_report(&second.launch_id, |report| *report == LaunchReport::Running);
    assert_eq!(
        sandbox.report(&first.launch_id),
        LaunchReport::Replaced {
            by: second.launch_id.clone()
        }
    );
    assert_eq!(sandbox.starts(), 2);
}

#[test]
fn a_remote_launch_checked_against_another_revision_is_refused_by_its_executor() {
    let sandbox = Sandbox::new("echo ready; exec sleep 300");
    let _app = sandbox.start_app();

    // Dispatches a launch expecting another revision, and reads the refusal.
    let rev = sandbox.config_rev();

    assert_eq!(rev.len(), 16, "{rev}");
    assert_eq!(sandbox.starts(), 0);
    assert_eq!(sandbox.at_most_one_run(), None);
}

#[test]
fn an_app_killed_before_it_spawns_the_run_starts_nothing() {
    let sandbox = Sandbox::new("echo ready; exec sleep 300");
    let mut app = sandbox.start_app();
    let launch = sandbox.remote_launch(&sandbox.config_rev(), None);
    sandbox.arm("app-bridge-launch");

    let sent = dispatch(&sandbox, &launch);
    sandbox.wait_reached("app-bridge-launch");
    app.kill();
    let handoff = sent.join().unwrap();
    assert!(matches!(handoff, Handoff::Unanswered(_)), "{handoff:?}");

    let report = settle_unanswered(&sandbox.store(), &launch.launch_id, launch.expires_at).unwrap();

    assert!(
        unix_ms_now() > launch.expires_at,
        "answered before the launch expired"
    );
    assert_eq!(report, LaunchReport::Expired);
    assert_eq!(sandbox.at_most_one_run(), None);
    assert_eq!(sandbox.starts(), 0);
}

#[test]
fn an_app_killed_after_it_spawned_the_run_starts_it_at_most_once() {
    let sandbox = Sandbox::new("echo ready; exec sleep 300");
    let mut app = sandbox.start_app();
    let launch = sandbox.remote_launch(&sandbox.config_rev(), None);
    sandbox.arm("app-bridge-spawned");

    let sent = dispatch(&sandbox, &launch);
    sandbox.wait_reached("app-bridge-spawned");
    // The run PTY is spawned, the answer not sent: the run starts, then the
    // app goes.
    wait_until(Duration::from_secs(30), || {
        (sandbox.starts() == 1).then_some(())
    })
    .expect("the app's run never started");
    app.kill();
    let handoff = sent.join().unwrap();
    assert!(matches!(handoff, Handoff::Unanswered(_)), "{handoff:?}");

    let report = settle_unanswered(&sandbox.store(), &launch.launch_id, launch.expires_at).unwrap();

    assert!(
        unix_ms_now() > launch.expires_at,
        "answered before the launch expired"
    );
    assert!(
        matches!(report, LaunchReport::Running | LaunchReport::Exited { .. }),
        "{report:?}"
    );
    assert_eq!(sandbox.at_most_one_run(), Some(launch.launch_id.clone()));
    // Its terminal gone, `localify dev` ends the run.
    sandbox.wait_report(&launch.launch_id, |report| {
        matches!(report, LaunchReport::Exited { .. })
    });
    assert_eq!(sandbox.at_most_one_run(), Some(launch.launch_id.clone()));
    assert_eq!(sandbox.starts(), 1);
}

#[test]
fn dev_in_app_starts_through_the_bridge_and_records_what_it_replaced() {
    let sandbox = Sandbox::new("exec python3 -m http.server {port} --bind 127.0.0.1");
    let _app = sandbox.start_app();
    let dev = |extra: &[&str]| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_localify"));
        command
            .arg("--config")
            .arg(&sandbox.config)
            .args(["dev", "web", "--project", "app", "--in-app"])
            .args(extra)
            .env("HOME", &sandbox.home)
            .env("XDG_STATE_HOME", sandbox.root.join("state"))
            .stdin(Stdio::null());
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    };

    let said = dev(&[]);
    assert!(said.contains("Started 'web'"), "{said}");
    let first = sandbox.store().list().unwrap().remove(0);
    assert_eq!(first.origin, ServiceOrigin::App);
    let first_id = first.run_id().cloned().unwrap();
    let launch = admitted(sandbox.store().launches().read(&first_id).unwrap());
    assert_eq!(launch.requested_by, None);

    let said = dev(&["--replace"]);
    assert!(said.contains("Started 'web'"), "{said}");
    let second = sandbox.store().list().unwrap().remove(0);
    let second_id = second.run_id().cloned().unwrap();
    assert_ne!(second_id, first_id);
    assert_eq!(
        sandbox.report(&first_id),
        LaunchReport::Replaced { by: second_id }
    );
    assert_eq!(sandbox.starts(), 2);
}

/// What the app's terminal panel tells the window: here, the tabs it opens.
struct Printed;

impl TerminalEvents for Printed {
    fn output(&self, _payload: TerminalOutputPayload) {}

    fn exit(&self, _payload: TerminalExitPayload) {}

    fn session(&self, _session: TerminalSessionDto) {}

    fn opened(&self, opened: TerminalOpenedPayload) {
        let run = opened.session.run.map(|run| run.run_id).unwrap_or_default();
        println!(
            "HELPER opened {run} replaced={:?} by={:?}",
            opened.replaced, opened.started_by
        );
    }

    fn shell_environment(&self, _status: ShellEnvironmentDto) {}
}

/// Not a test: the app, for the tests above, in a process of its own.
#[test]
#[ignore = "run by the tests above as the app"]
fn app_helper() {
    let Some(root) = std::env::var_os(HELPER_ENV).map(PathBuf::from) else {
        return;
    };
    let localify = PathBuf::from(env!("CARGO_BIN_EXE_localify"));
    let manager = TerminalManager::new(
        localify.clone(),
        root.join("zsh"),
        RunningServiceStore::new_default().unwrap(),
        AppInstallation::of_this_process(),
        Arc::new(Printed),
    );
    let api = DesktopApi::new(root.join("config.toml"), localify);
    let data_dir = api.data_dir().unwrap();
    let _bridge = AppBridge::listen(&data_dir, terminal_launcher(manager, api)).unwrap();
    println!("HELPER listening");
    thread::sleep(Duration::from_secs(600));
}
