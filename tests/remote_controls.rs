//! The control executor against real processes (docs/remote-machines.md,
//! "Controls", and the PR 4 gates): the daemon's `LocalHost`, driven
//! directly, reads a real config and real run records, stops real runs and
//! starts the real `localify dev`, detached or through a stand-in for the
//! app's bridge. Everything lives in a temporary home, so an installed
//! Localify is never touched. The session around a control is tested in the
//! daemon (`src/daemon/remote/tests/control.rs`).

use std::ffi::OsString;
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::net::UnixListener;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use localify::application::inventory::{AppPresence, Switches, TargetSettings};
use localify::branding::BRANDING;
use localify::infrastructure::app_bridge::{self, LaunchRequest};
use localify::infrastructure::process::{self, NativeProcesses};
use localify::infrastructure::remote::control::{
    ControlRunner, ControlTimings, Host, LocalHost, Look, Order,
};
use localify::infrastructure::remote::requests::{Report, RequestEntries};
use localify::infrastructure::remote::wire::{
    Control, ControlAction, ControlExpect, ControlOutcome, ControlService,
};
use localify::infrastructure::running_services::{RunOutput, RunningServiceStore, ServiceKey};
use localify::infrastructure::runs::RunId;
use localify::infrastructure::runs::launches::{LaunchReport, reconcile};

struct Sandbox {
    _dir: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    config: PathBuf,
    port: u16,
}

impl Sandbox {
    /// `web` runs `script`.
    fn new(script: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let (home, app) = (root.join("home"), root.join("app"));
        for folder in [&home, &app] {
            fs::create_dir_all(folder).unwrap();
        }
        let port = free_port();
        let config = root.join("config.toml");
        fs::write(&config, config_toml(&root, &app, port, script)).unwrap();
        Self {
            _dir: dir,
            root,
            home,
            config,
            port,
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

    fn environment(&self) -> Vec<(OsString, OsString)> {
        vec![
            ("HOME".into(), self.home.clone().into()),
            ("XDG_STATE_HOME".into(), self.root.join("state").into()),
            ("PATH".into(), "/usr/bin:/bin".into()),
        ]
    }

    fn host(&self) -> LocalHost {
        let environment = self.environment();
        LocalHost {
            config_path: self.config.clone(),
            data_dir: self.root.join("data"),
            services_dir: self.services_dir(),
            executable: Ok(PathBuf::from(env!("CARGO_BIN_EXE_localify"))),
            environment: Arc::new(move || environment.clone()),
            installation: vec![
                ("HOME".into(), Some(self.home.clone().into())),
                (BRANDING.profile_env().into(), None),
                (
                    "XDG_STATE_HOME".into(),
                    Some(self.root.join("state").into()),
                ),
            ],
            project_roots: Arc::new(Default::default),
        }
    }

    fn runner(&self, timings: ControlTimings) -> (Arc<LocalHost>, ControlRunner) {
        let host = Arc::new(self.host());
        let runner = ControlRunner::new(
            host.clone(),
            RequestEntries::in_services_dir(&self.services_dir()),
            timings,
        );
        (host, runner)
    }

    /// `localify dev` for `web`, with the sandbox's environment.
    fn localify(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_localify"));
        command
            .arg("--config")
            .arg(&self.config)
            .args(args)
            .current_dir(&self.home)
            .env_clear()
            .envs(self.environment())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    }

    /// A background executor, in its own session, as `dev --detach` makes.
    fn background(&self, args: &[&str]) -> Child {
        let mut command = self.localify(args);
        command.env("LOCALIFY_SERVICE_ORIGIN", "background");
        // SAFETY: only setsid runs between fork and exec; it is async-signal-safe.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        command.spawn().unwrap()
    }

    fn look(&self, host: &LocalHost, legacy_id: Option<&str>) -> Look {
        host.look(&primary(), None, legacy_id).unwrap().unwrap()
    }

    fn report(&self, id: &RunId) -> LaunchReport {
        reconcile(&self.store(), &NativeProcesses, id, None).unwrap()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        for record in self.store().list().unwrap_or_default() {
            let _ = self.store().stop(&record);
        }
    }
}

fn primary() -> ServiceKey {
    ServiceKey {
        project_id: "app".into(),
        environment: None,
        service_id: "web".into(),
    }
}

fn order(action: ControlAction, look: &Look, request: &str) -> Order {
    let run = look.facts.run.as_ref();
    let expires = SystemTime::now() + Duration::from_secs(30);
    let expires = time::OffsetDateTime::from(expires)
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
    Order {
        peer: "machine-a".into(),
        peer_name: "Machine A".into(),
        control: Control {
            id: "c1".into(),
            action,
            service: ControlService {
                project_id: "app".into(),
                environment: None,
                service_id: "web".into(),
            },
            profile: None,
            request_id: format!("{request:q>22}"),
            expect: ControlExpect {
                run_id: run.and_then(|run| run.run_id.clone()),
                config_rev: look.facts.service.config_rev.clone(),
                legacy_id: run.and_then(|run| run.legacy_id.clone()),
            },
            expires_at: expires,
        },
    }
}

fn switches(background_starts: bool, app: AppPresence) -> impl Fn() -> Switches {
    move || Switches {
        settings: TargetSettings {
            share: true,
            control: true,
            background_starts,
        },
        app,
    }
}

fn fast(budget: Duration) -> ControlTimings {
    ControlTimings {
        dispatch_budget: budget,
        settle: Duration::from_secs(3),
        poll: Duration::from_millis(50),
        ahead: Duration::from_secs(60),
    }
}

fn wait_until<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(found) = check() {
            return found;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(50));
    }
}

/// The gate: a restart of a background run with `backgroundStarts` off is
/// refused, and the run keeps running.
#[test]
fn a_background_restart_without_background_starts_is_refused_and_it_keeps_running() {
    let sandbox = Sandbox::new("exec sleep 300");
    let mut executor = sandbox.background(&["dev", "web", "--project", "app"]);
    let (host, runner) = sandbox.runner(fast(Duration::from_secs(5)));
    let look = wait_until("the background run", || {
        Some(sandbox.look(&host, None)).filter(|look| look.facts.run.is_some())
    });

    let report = runner.run(
        &order(ControlAction::Restart, &look, "r1"),
        &switches(false, AppPresence::Absent),
        &mut |_| {},
    );

    assert_eq!(report.outcome, ControlOutcome::Refused, "{report:?}");
    assert_eq!(report.code.as_deref(), Some("background_disabled"));
    assert!(executor.try_wait().unwrap().is_none(), "it keeps running");
    let record = sandbox.store().get(&look.key).unwrap().unwrap();
    assert!(sandbox.store().is_running(&record));
    assert_eq!(
        record.run_id().map(ToString::to_string),
        look.facts.run.unwrap().run_id
    );
}

/// The gate: a proven legacy run restarted with `expect.legacyId` ends and
/// is replaced by a run with a run id and a stream, started in the
/// background with `--replace-legacy`.
#[test]
fn a_proven_legacy_run_is_relaunched_in_the_background() {
    let sandbox = Sandbox::new("exec sleep 300");
    let (host, runner) = sandbox.runner(fast(Duration::from_secs(10)));
    let key = sandbox.look(&host, None).key;
    let mut legacy = Command::new("sleep").arg("300").spawn().unwrap();
    write_legacy_record(&sandbox, &key, legacy.id());
    let look = sandbox.look(&host, None);
    let legacy_id = look
        .facts
        .run
        .as_ref()
        .and_then(|run| run.legacy_id.clone());
    assert!(
        legacy_id.is_some(),
        "a proven legacy run: {:?}",
        look.facts.run
    );

    let report = runner.run(
        &order(ControlAction::Restart, &look, "r2"),
        &switches(true, AppPresence::Absent),
        &mut |_| {},
    );

    assert!(
        matches!(
            report.outcome,
            ControlOutcome::Done | ControlOutcome::Starting
        ),
        "{report:?}"
    );
    assert!(legacy.wait().is_ok(), "the legacy run ended");
    let record = sandbox.store().get(&key).unwrap().unwrap();
    assert_eq!(
        record.run_id().map(ToString::to_string),
        report.run_id,
        "replaced by the relaunch"
    );
    assert_eq!(record.run.as_ref().unwrap().output, Some(RunOutput::Pipe));
}

/// The executor refuses a relaunch once the record names another run: a
/// local launch that came in between wins.
#[test]
fn a_relaunch_starts_nothing_once_another_launch_took_the_service_over() {
    let sandbox = Sandbox::new("exec sleep 300");
    let (host, _) = sandbox.runner(fast(Duration::from_secs(10)));
    let look = sandbox.look(&host, None);
    let status = sandbox
        .localify(&[
            "dev",
            "web",
            "--project",
            "app",
            "--remote-launch=AAAAAAAAAAAAAAAAAAAAAA",
            &format!(
                "--launch-expires={}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_millis()
                    + 15_000
            ),
            &format!("--expect-rev={}", look.facts.service.config_rev),
            "--request=machine-a:req-1",
            "--replace-legacy=boot.1.2",
        ])
        .status()
        .unwrap();

    assert!(!status.success());
    let id = RunId::parse("AAAAAAAAAAAAAAAAAAAAAA").unwrap();
    let LaunchReport::Refused { message, .. } = sandbox.report(&id) else {
        panic!("not refused: {:?}", sandbox.report(&id));
    };
    assert!(message.contains("someone else"), "{message}");
    assert!(
        sandbox.store().get(&look.key).unwrap().is_none(),
        "nothing ran"
    );
}

/// The gate: launch A starts and exits, a local launch B then replaces the
/// service's record, and the bridge's answer about A was lost. The target
/// still reports A as admitted and exited, with its code.
#[test]
fn a_lost_acknowledgement_is_reported_exactly() {
    let sandbox = Sandbox::new("echo a-ran; sleep 1; exit 3");
    let (host, runner) = sandbox.runner(fast(Duration::from_secs(12)));
    let look = sandbox.look(&host, None);
    let bridge = unanswering_app(&sandbox);

    let control = order(ControlAction::Start, &look, "r3");
    let ran = thread::scope(|scope| {
        let running = scope.spawn(|| {
            runner.run(
                &control,
                &switches(false, AppPresence::Running),
                &mut |_| {},
            )
        });
        // The app spawned A, then closed the connection without a word.
        let launch_a = bridge.join().unwrap();
        wait_until("A exited", || {
            matches!(sandbox.report(&launch_a), LaunchReport::Exited { .. }).then_some(())
        });
        // A local launch B takes the service over.
        let mut b = sandbox.background(&["dev", "web", "--project", "app", "--", "sleep", "300"]);
        wait_until("B registered", || {
            let record = sandbox.store().get(&look.key).ok()??;
            (record.run_id() != Some(&launch_a)).then_some(())
        });
        let report = running.join().unwrap();
        let _ = b.kill();
        let _ = b.wait();
        (launch_a, report)
    });
    let (launch_a, report): (RunId, Report) = ran;

    assert_eq!(report.outcome, ControlOutcome::Failed, "{report:?}");
    assert!(
        report.message.contains("exited with code 3"),
        "{}",
        report.message
    );
    assert_eq!(report.run_id.as_deref(), Some(launch_a.as_str()));
    // And `control.status` says the same.
    let status = runner.status("machine-a", &format!("{:q>22}", "r3"));
    assert_eq!(status, report);
}

/// The app's bridge, as an app that spawns the launch's `localify dev` and
/// closes the connection before it answers. Returns the launch's id.
fn unanswering_app(sandbox: &Sandbox) -> thread::JoinHandle<RunId> {
    let socket = app_bridge::socket_path(&sandbox.root.join("data"));
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(socket.parent().unwrap())
        .unwrap();
    let listener = UnixListener::bind(&socket).unwrap();
    let config = sandbox.config.clone();
    let environment = sandbox.environment();
    let home = sandbox.home.clone();
    thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line).unwrap();
        let mut request: serde_json::Value = serde_json::from_str(&line).unwrap();
        request.as_object_mut().unwrap().remove("cmd");
        let request: LaunchRequest = serde_json::from_value(request).unwrap();
        let requester = request.requested_by.clone().unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_localify"));
        command
            .arg("--config")
            .arg(&config)
            .args(["dev", "web", "--project", "app"])
            .arg(format!("--remote-launch={}", request.launch_id))
            .arg(format!("--launch-expires={}", request.expires_at))
            .arg(format!("--expect-rev={}", request.expect_rev.unwrap()))
            .arg(format!(
                "--request={}:{}",
                requester.machine_id, requester.request_id
            ))
            .current_dir(&home)
            .env_clear()
            .envs(environment)
            .env("LOCALIFY_SERVICE_ORIGIN", "background")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // SAFETY: only setsid runs between fork and exec; it is async-signal-safe.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut executor = command.spawn().unwrap();
        thread::spawn(move || executor.wait());
        drop(stream);
        request.launch_id
    })
}

/// A record of `web` as a version without run ids left it for `pid`, run
/// by `dev --detach`.
fn write_legacy_record(sandbox: &Sandbox, key: &ServiceKey, pid: u32) {
    let info = process::process_info(pid).unwrap();
    let record = serde_json::json!({
        "projectId": key.project_id,
        "environment": key.environment,
        "serviceId": key.service_id,
        "pid": pid,
        "processStartedAt": process::lstart_label(info.started_wall).unwrap(),
        "url": "http://web.localify",
        "port": sandbox.port,
        "origin": "background",
        "profile": null,
        "startedAt": SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
        "logFile": null,
        "exit": null,
    });
    let dir = sandbox.services_dir();
    fs::create_dir_all(&dir).unwrap();
    let name = format!(
        "{}--{}--{}.json",
        key.project_id,
        key.environment.as_deref().unwrap_or("default"),
        key.service_id
    );
    let mut file = fs::File::create(dir.join(name)).unwrap();
    file.write_all(record.to_string().as_bytes()).unwrap();
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
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
