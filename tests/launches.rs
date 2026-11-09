//! Launch fencing and launch records end to end (docs/remote-machines.md,
//! "The run lock and launch fencing" and "Launch records"): a real
//! `localify dev` admits or refuses a dispatched launch, a test hook holds it
//! between its checks and its admission, or between its admission and its run
//! record, and the test plays the reconciler through the library. Everything
//! lives in a temporary home.

use std::fs::{self, File, FileTimes};
use std::io;
use std::net::TcpListener;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use localify::branding::BRANDING;
use localify::infrastructure::process::{self, NativeProcesses, Signal};
use localify::infrastructure::running_services::RunningServiceStore;
use localify::infrastructure::runs::RunId;
use localify::infrastructure::runs::launches::{
    ExitReason, LaunchRecord, LaunchReport, RefusalCode, reconcile, unix_ms_now,
};
use localify::infrastructure::test_pause::PAUSE_DIR_ENV;

struct Sandbox {
    _dir: tempfile::TempDir,
    root: PathBuf,
    config: PathBuf,
    app: PathBuf,
    path: String,
}

impl Sandbox {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let app = root.join("app");
        fs::create_dir_all(&app).unwrap();
        fs::create_dir_all(root.join("pause")).unwrap();
        let config = root.join("config.toml");
        fs::write(&config, config_toml(&root, &app, free_port())).unwrap();
        Self {
            _dir: dir,
            root,
            config,
            app,
            path: std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into()),
        }
    }

    /// `localify dev` of the service with `flags`, running `script`, in the
    /// sandbox's home and state, with its test pauses.
    fn dev(&self, flags: &[&str], script: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_localify"));
        command
            .arg("--config")
            .arg(&self.config)
            .args(["dev", "web", "--project", "app"])
            .args(flags)
            .args(["--", "sh", "-c", script])
            .current_dir(&self.app)
            .env("HOME", &self.root)
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("PATH", &self.path)
            .env(PAUSE_DIR_ENV, self.root.join("pause"))
            .env_remove("LOCALIFY_PROFILE")
            .env_remove("LOCALIFY_SERVICE_ORIGIN")
            .stdin(Stdio::null());
        command
    }

    /// An executor started as a dispatcher starts one: in its own session,
    /// as a background run, its own messages in a file.
    fn spawn_executor(&self, mut command: Command) -> Child {
        let log = File::create(self.root.join("executor.log")).unwrap();
        command
            .env("LOCALIFY_SERVICE_ORIGIN", "background")
            .stdout(log.try_clone().unwrap())
            .stderr(log);
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

    fn executor_log(&self) -> String {
        fs::read_to_string(self.root.join("executor.log")).unwrap_or_default()
    }

    /// Holds every executor at `point` until [`Sandbox::release`].
    fn arm(&self, point: &str) {
        fs::write(self.root.join("pause").join(point), "").unwrap();
    }

    fn release(&self, point: &str) {
        fs::remove_file(self.root.join("pause").join(point)).unwrap();
    }

    fn wait_reached(&self, point: &str) {
        let reached = self.root.join("pause").join(format!("{point}.reached"));
        wait_until(Duration::from_secs(30), || reached.exists().then_some(()))
            .unwrap_or_else(|| panic!("never reached {point}:\n{}", self.executor_log()));
    }

    fn services_dir(&self) -> PathBuf {
        if cfg!(target_os = "macos") {
            self.root
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

    fn launch(&self, id: &RunId) -> Option<LaunchRecord> {
        self.store().launches().read(id).unwrap()
    }

    fn report(&self, id: &RunId, expires_at: Option<u64>) -> LaunchReport {
        reconcile(&self.store(), &NativeProcesses, id, expires_at).unwrap()
    }

    fn marker(&self) -> PathBuf {
        self.root.join("started")
    }

    /// A script that leaves a mark once the service runs.
    fn touch_marker(&self) -> String {
        format!("touch '{}'", self.marker().display())
    }
}

fn config_toml(root: &Path, app: &Path, port: u16) -> String {
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
command = ["sh", "-c", "exit 0"]
"#,
        root = root.display(),
        app = app.display(),
    )
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
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

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn launch_id_flag(id: &RunId) -> String {
    format!("--launch-id={id}")
}

/// Sends a signal Localify itself never sends, such as SIGSTOP.
fn signal_raw(child: &Child, signal: libc::c_int) {
    // SAFETY: kill only signals; the PID is a child this test has not reaped.
    assert_eq!(unsafe { libc::kill(child.id() as libc::pid_t, signal) }, 0);
}

#[test]
fn a_launch_delivered_after_its_expiry_starts_nothing() {
    let sandbox = Sandbox::new();
    let id = RunId::generate().unwrap();
    let expired = (unix_ms_now() - 1).to_string();

    let output = sandbox
        .dev(
            &[&launch_id_flag(&id), "--launch-expires", &expired],
            &sandbox.touch_marker(),
        )
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(stderr(&output).contains("expired"), "{}", stderr(&output));
    assert!(!sandbox.marker().exists());
    assert!(matches!(
        sandbox.launch(&id),
        Some(LaunchRecord::Refused {
            code: RefusalCode::Expired,
            ..
        })
    ));
    assert!(sandbox.store().list().unwrap().is_empty());
}

#[test]
fn the_reconciler_s_expired_record_wins_over_a_launch_held_before_its_admission() {
    let sandbox = Sandbox::new();
    let id = RunId::generate().unwrap();
    let expires_at = unix_ms_now() + 5_000;
    sandbox.arm("launch-admission");

    let executor = sandbox
        .dev(
            &[
                &launch_id_flag(&id),
                "--launch-expires",
                &expires_at.to_string(),
            ],
            &sandbox.touch_marker(),
        )
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Its expiry check passed; it waits before publishing its admission.
    sandbox.wait_reached("launch-admission");
    assert_eq!(sandbox.report(&id, Some(expires_at)), LaunchReport::Unknown);
    while unix_ms_now() < expires_at {
        thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(sandbox.report(&id, Some(expires_at)), LaunchReport::Expired);
    sandbox.release("launch-admission");
    let output = executor.wait_with_output().unwrap();

    assert!(!output.status.success());
    assert!(stderr(&output).contains("expired"), "{}", stderr(&output));
    assert!(!sandbox.marker().exists());
    assert!(matches!(
        sandbox.launch(&id),
        Some(LaunchRecord::Expired { .. })
    ));
    assert!(sandbox.store().list().unwrap().is_empty());
    assert_eq!(sandbox.report(&id, Some(expires_at)), LaunchReport::Expired);
}

#[test]
fn a_temporary_launch_file_left_by_a_crash_before_its_link_is_ignored_and_swept() {
    let sandbox = Sandbox::new();
    let id = RunId::generate().unwrap();
    let expires_at = unix_ms_now() + 15_000;
    sandbox.arm("launch-record-written");

    let mut executor = sandbox.spawn_executor(sandbox.dev(
        &[
            &launch_id_flag(&id),
            "--launch-expires",
            &expires_at.to_string(),
        ],
        &sandbox.touch_marker(),
    ));
    sandbox.wait_reached("launch-record-written");
    assert!(process::send_signal(executor.id(), Signal::Kill));
    executor.wait().unwrap();
    sandbox.release("launch-record-written");

    let launches = sandbox.store().launches().dir().to_path_buf();
    let temporary: Vec<PathBuf> = fs::read_dir(&launches)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "tmp"))
        .collect();
    assert_eq!(temporary.len(), 1, "{temporary:?}");
    // Readers never take it for a decision: the launch is undecided, then
    // expired, and nothing started.
    assert_eq!(sandbox.launch(&id), None);
    assert_eq!(sandbox.report(&id, Some(expires_at)), LaunchReport::Unknown);
    assert_eq!(
        sandbox.report(&id, Some(unix_ms_now())),
        LaunchReport::Expired
    );
    assert!(!sandbox.marker().exists());

    // Younger than an hour, it stays; older, the daemon's sweep removes it.
    let store = sandbox.store();
    store.sweep_leftovers().unwrap();
    assert!(temporary[0].exists());
    let two_hours_ago = SystemTime::now() - Duration::from_secs(2 * 60 * 60);
    File::options()
        .write(true)
        .open(&temporary[0])
        .unwrap()
        .set_times(FileTimes::new().set_modified(two_hours_ago))
        .unwrap();
    assert_eq!(store.sweep_leftovers().unwrap(), 1);
    assert!(!temporary[0].exists());
    assert!(matches!(
        sandbox.launch(&id),
        Some(LaunchRecord::Expired { .. })
    ));
}

#[test]
fn an_executor_stopped_between_admission_and_registration_is_starting_and_killed_there_is_interrupted()
 {
    let sandbox = Sandbox::new();
    let id = RunId::generate().unwrap();
    sandbox.arm("launch-admitted");

    let mut executor = sandbox.spawn_executor(sandbox.dev(
        &[
            &launch_id_flag(&id),
            "--launch-expires",
            &(unix_ms_now() + 15_000).to_string(),
        ],
        &sandbox.touch_marker(),
    ));
    sandbox.wait_reached("launch-admitted");
    signal_raw(&executor, libc::SIGSTOP);
    sandbox.release("launch-admitted");

    let Some(LaunchRecord::Admitted(admitted)) = sandbox.launch(&id) else {
        panic!("not admitted:\n{}", sandbox.executor_log());
    };
    assert_eq!(admitted.executor.pid, executor.id());
    assert_eq!(admitted.executor.sid, Some(executor.id()));
    assert_eq!(admitted.requested_by, None);
    assert_eq!(sandbox.report(&id, None), LaunchReport::Starting);
    assert!(sandbox.store().list().unwrap().is_empty());

    assert!(process::send_signal(executor.id(), Signal::Kill));
    executor.wait().unwrap();
    let interrupted = LaunchReport::Exited {
        code: None,
        signal: None,
        reason: Some(ExitReason::InterruptedBeforeRegistration),
    };
    assert_eq!(sandbox.report(&id, None), interrupted);
    assert_eq!(sandbox.report(&id, None), interrupted);
    let sid = executor.id();
    assert!(
        process::all_processes()
            .iter()
            .all(|process| process.sid != Some(sid)),
        "the launch's session is not empty"
    );
    assert!(!sandbox.marker().exists());

    // The service is free: the next launch starts.
    let next = sandbox.dev(&[], "exit 0").output().unwrap();
    assert!(next.status.success(), "{}", stderr(&next));
}

#[test]
fn a_dispatched_launch_records_its_exit_and_the_launch_that_replaced_it() {
    let sandbox = Sandbox::new();
    let first = RunId::generate().unwrap();
    let second = RunId::generate().unwrap();

    let mut executor = sandbox.spawn_executor(sandbox.dev(&[&launch_id_flag(&first)], "sleep 30"));
    wait_until(Duration::from_secs(30), || {
        (sandbox.report(&first, None) == LaunchReport::Running).then_some(())
    })
    .unwrap_or_else(|| panic!("never running:\n{}", sandbox.executor_log()));

    let replacing = sandbox
        .dev(&[&launch_id_flag(&second), "--replace"], "exit 3")
        .output()
        .unwrap();
    executor.wait().unwrap();

    assert_eq!(replacing.status.code(), Some(3), "{}", stderr(&replacing));
    assert_eq!(
        sandbox.report(&first, None),
        LaunchReport::Replaced { by: second.clone() }
    );
    assert_eq!(
        sandbox.report(&second, None),
        LaunchReport::Exited {
            code: Some(3),
            signal: None,
            reason: None
        }
    );
    let Some(LaunchRecord::Admitted(admitted)) = sandbox.launch(&second) else {
        panic!("not admitted");
    };
    assert_eq!(admitted.run_id, second);
    assert_eq!(admitted.service.service_id, "web");

    // A third launch while nothing runs but the port is taken is refused.
    let third = RunId::generate().unwrap();
    let port = sandbox.store().list().unwrap()[0].port;
    let _taken = TcpListener::bind(("127.0.0.1", port)).unwrap();
    let refused = sandbox
        .dev(&[&launch_id_flag(&third)], &sandbox.touch_marker())
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert!(matches!(
        sandbox.report(&third, None),
        LaunchReport::Refused {
            code: RefusalCode::Conflict,
            ..
        }
    ));
    assert!(!sandbox.marker().exists());
}

#[test]
fn a_person_s_own_launch_leaves_no_launch_record() {
    let sandbox = Sandbox::new();

    let output = sandbox.dev(&[], "exit 0").output().unwrap();

    assert!(output.status.success(), "{}", stderr(&output));
    let records = fs::read_dir(sandbox.store().launches().dir())
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
                .count()
        })
        .unwrap_or(0);
    assert_eq!(records, 0);
}
