//! Launch fencing and launch records end to end (docs/remote-machines.md,
//! "The run lock and launch fencing" and "Launch records"): a real
//! `localify dev` admits or refuses a dispatched launch, a test hook holds it
//! between its checks and its admission, or between its admission and its run
//! record, and the test plays the reconciler through the library. Everything
//! lives in a temporary home.

use std::fs::{self, File, FileTimes};
use std::io;
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
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
        let mut command = self.dev_own("web", flags);
        command.args(["--", "sh", "-c", script]);
        command
    }

    /// `localify dev` of `service` with `flags`, running its own command.
    fn dev_own(&self, service: &str, flags: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_localify"));
        command
            .arg("--config")
            .arg(&self.config)
            .args(["dev", service, "--project", "app"])
            .args(flags)
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
        self.arm_in("pause", point);
    }

    fn release(&self, point: &str) {
        self.release_in("pause", point);
    }

    fn wait_reached(&self, point: &str) {
        self.wait_reached_in("pause", point);
    }

    /// A pause folder of its own, for one executor among several.
    fn pause_dir(&self, name: &str) -> PathBuf {
        let dir = self.root.join(name);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn arm_in(&self, dir: &str, point: &str) {
        fs::write(self.pause_dir(dir).join(point), "").unwrap();
    }

    fn release_in(&self, dir: &str, point: &str) {
        fs::remove_file(self.root.join(dir).join(point)).unwrap();
    }

    fn wait_reached_in(&self, dir: &str, point: &str) {
        let reached = self.root.join(dir).join(format!("{point}.reached"));
        wait_until(Duration::from_secs(30), || reached.exists().then_some(()))
            .unwrap_or_else(|| panic!("never reached {point}:\n{}", self.executor_log()));
    }

    /// Rewrites the config, `from` replaced by `to`.
    fn edit_config(&self, from: &str, to: &str) {
        let config = fs::read_to_string(&self.config).unwrap();
        assert!(config.contains(from), "{from} is not in the config");
        fs::write(&self.config, config.replace(from, to)).unwrap();
    }

    /// A `slow` profile whose Infisical CLI never answers, as one waiting
    /// for a login does, and leaves a descendant behind. Returns the PATH that
    /// finds it, and the file it writes its PID and its descendant's to.
    fn slow_infisical(&self) -> (String, PathBuf) {
        let marker = self.root.join("infisical.pids");
        let path = self.infisical(&format!(
            "sleep 300 &\necho $$ $! > \"{}.tmp\"\nmv \"{0}.tmp\" \"{0}\"\nexec sleep 300\n",
            marker.display()
        ));
        (path, marker)
    }

    /// A `slow` profile whose Infisical CLI runs `script`. Returns the PATH
    /// that finds it.
    fn infisical(&self, script: &str) -> String {
        fs::write(
            self.app.join("localify.toml"),
            "[profiles.slow]\nsources = [{ infisical = {} }]\n",
        )
        .unwrap();
        fs::write(self.app.join(".infisical.yaml"), "workspaceId: \"w\"\n").unwrap();
        let bin = self.root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let cli = bin.join("infisical");
        fs::write(&cli, format!("#!/bin/sh\n{script}")).unwrap();
        fs::set_permissions(&cli, fs::Permissions::from_mode(0o755)).unwrap();
        format!("{}:{}", bin.display(), self.path)
    }

    /// A dispatched launch of the service on the `slow` profile, found on
    /// `path`.
    fn spawn_slow(&self, id: &RunId, path: &str) -> Child {
        let mut command = self.dev(
            &[&launch_id_flag(id), "--profile", "slow"],
            &self.touch_marker(),
        );
        command.env("PATH", path);
        self.spawn_executor(command)
    }

    /// The PIDs a helper wrote to `marker`, once it did.
    fn helper_pids(&self, marker: &Path) -> (u32, u32) {
        wait_until(Duration::from_secs(30), || {
            let content = fs::read_to_string(marker).ok()?;
            let mut pids = content
                .split_whitespace()
                .map(|pid| pid.parse::<u32>().ok());
            Some((pids.next()??, pids.next()??))
        })
        .unwrap_or_else(|| panic!("the profile never ran its helper:\n{}", self.executor_log()))
    }

    fn running(&self, id: &RunId) {
        wait_until(Duration::from_secs(30), || {
            (self.report(id, None) == LaunchReport::Running).then_some(())
        })
        .unwrap_or_else(|| panic!("{id} never ran:\n{}", self.executor_log()));
    }

    /// Stops what still runs of the service.
    fn stop(&self) {
        if let Some(record) = self.store().list().unwrap().into_iter().next() {
            let _ = self.store().stop(&record);
        }
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

fn refused_with(report: &LaunchReport) -> Option<RefusalCode> {
    match report {
        LaunchReport::Refused { code, .. } => Some(*code),
        _ => None,
    }
}

#[test]
fn a_launch_refused_before_its_lock_never_stops_the_running_service() {
    let sandbox = Sandbox::new();
    let first = RunId::generate().unwrap();
    let mut running = sandbox.spawn_executor(sandbox.dev(&[&launch_id_flag(&first)], "sleep 30"));
    sandbox.running(&first);
    let touch = sandbox.touch_marker();

    // Expired, with --replace.
    let expired = RunId::generate().unwrap();
    let past = (unix_ms_now() - 1).to_string();
    let output = sandbox
        .dev(
            &[
                &launch_id_flag(&expired),
                "--launch-expires",
                &past,
                "--replace",
            ],
            &touch,
        )
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(
        refused_with(&sandbox.report(&expired, None)),
        Some(RefusalCode::Expired)
    );

    // Already decided: the reconciler declared it expired.
    let decided = RunId::generate().unwrap();
    sandbox.store().launches().expire(&decided).unwrap();
    let output = sandbox
        .dev(&[&launch_id_flag(&decided), "--replace"], &touch)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(sandbox.report(&decided, None), LaunchReport::Expired);

    // Checked against another revision, with its own command.
    let stale = RunId::generate().unwrap();
    let output = sandbox
        .dev_own(
            "web",
            &[
                &launch_id_flag(&stale),
                "--expect-rev",
                "0123456789abcdef",
                "--replace",
            ],
        )
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(
        refused_with(&sandbox.report(&stale, None)),
        Some(RefusalCode::Conflict)
    );

    // A person's own launch of a target off loopback.
    sandbox.edit_config("target = \"127.0.0.1:", "target = \"192.0.2.1:");
    let output = sandbox.dev(&["--replace"], &touch).output().unwrap();
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("only supports route targets"),
        "{}",
        stderr(&output)
    );
    sandbox.edit_config("target = \"192.0.2.1:", "target = \"127.0.0.1:");

    // None of them stopped the run, nor started anything.
    assert!(running.try_wait().unwrap().is_none(), "the run was stopped");
    assert_eq!(sandbox.report(&first, None), LaunchReport::Running);
    assert!(!sandbox.marker().exists());
    sandbox.stop();
    running.wait().unwrap();
}

#[test]
fn a_launch_that_fails_before_its_admission_publishes_its_refusal() {
    let sandbox = Sandbox::new();

    // A service the config no longer has.
    let removed = RunId::generate().unwrap();
    let output = sandbox
        .dev_own("gone", &[&launch_id_flag(&removed)])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(
        refused_with(&sandbox.report(&removed, None)),
        Some(RefusalCode::Conflict)
    );

    // A checkout that no longer exists.
    let moved = RunId::generate().unwrap();
    fs::rename(&sandbox.app, sandbox.root.join("moved")).unwrap();
    let output = sandbox
        .dev(&[&launch_id_flag(&moved)], "exit 0")
        .current_dir(&sandbox.root)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let LaunchReport::Refused { code, message } = sandbox.report(&moved, None) else {
        panic!("not refused: {}", stderr(&output));
    };
    assert_eq!(code, RefusalCode::Conflict);
    assert!(message.contains("no longer exists"), "{message}");
    fs::rename(sandbox.root.join("moved"), &sandbox.app).unwrap();

    // A config that does not parse.
    let invalid = RunId::generate().unwrap();
    fs::write(&sandbox.config, "[projects.app\n").unwrap();
    let output = sandbox
        .dev(&[&launch_id_flag(&invalid)], "exit 0")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(
        refused_with(&sandbox.report(&invalid, None)),
        Some(RefusalCode::Conflict),
        "{}",
        stderr(&output)
    );
    assert!(sandbox.store().list().unwrap().is_empty());
}

#[test]
fn killing_an_executor_while_its_profile_helper_runs_ends_the_helper_and_what_it_started() {
    let sandbox = Sandbox::new();
    let (path, marker) = sandbox.slow_infisical();
    let id = RunId::generate().unwrap();
    let mut executor = sandbox.spawn_slow(&id, &path);
    let (helper, descendant) = sandbox.helper_pids(&marker);
    assert_eq!(sandbox.report(&id, None), LaunchReport::Starting);

    assert!(process::send_signal(executor.id(), Signal::Kill));
    executor.wait().unwrap();
    assert!(
        process::process_info(helper).is_some(),
        "the helper died with its executor"
    );

    let interrupted = LaunchReport::Exited {
        code: None,
        signal: None,
        reason: Some(ExitReason::InterruptedBeforeRegistration),
    };
    assert_eq!(sandbox.report(&id, None), interrupted);
    for pid in [helper, descendant] {
        assert!(
            wait_until(Duration::from_secs(30), || process::process_info(pid)
                .is_none()
                .then_some(()))
            .is_some(),
            "process {pid} of the launch outlived its cleanup"
        );
    }
    let sid = executor.id();
    assert!(
        process::all_processes()
            .iter()
            .all(|process| process.sid != Some(sid)),
        "the launch's session is not empty"
    );
    assert_eq!(sandbox.report(&id, None), interrupted);
    assert!(!sandbox.store().run_dir(&id).exists());
    assert!(!sandbox.marker().exists());
    let next = sandbox.dev(&[], "exit 0").output().unwrap();
    assert!(next.status.success(), "{}", stderr(&next));
}

#[test]
fn an_executor_killed_between_its_run_s_end_and_its_exit_never_reads_running() {
    let sandbox = Sandbox::new();
    let lost = LaunchReport::Exited {
        code: None,
        signal: None,
        reason: Some(ExitReason::ExecutorLost),
    };
    sandbox.arm("run-end-published");

    // Completed by a reconciler.
    let id = RunId::generate().unwrap();
    let mut executor = sandbox.spawn_executor(sandbox.dev(&[&launch_id_flag(&id)], "exit 3"));
    sandbox.wait_reached("run-end-published");
    assert!(process::send_signal(executor.id(), Signal::Kill));
    executor.wait().unwrap();
    assert_eq!(sandbox.report(&id, None), lost);
    let record = sandbox.store().list().unwrap().remove(0);
    assert_eq!(record.exit.map(|exit| exit.code), Some(None));

    // Completed by the daemon's pass, before anyone asked.
    fs::remove_file(sandbox.root.join("pause/run-end-published.reached")).unwrap();
    let id = RunId::generate().unwrap();
    let mut executor = sandbox.spawn_executor(sandbox.dev(&[&launch_id_flag(&id)], "exit 3"));
    sandbox.wait_reached("run-end-published");
    assert!(process::send_signal(executor.id(), Signal::Kill));
    executor.wait().unwrap();
    sandbox.release("run-end-published");
    let store = sandbox.store();
    store.sweep_leftovers().unwrap();
    store.finish_unread_runs().unwrap();
    let Some(LaunchRecord::Admitted(admitted)) = sandbox.launch(&id) else {
        panic!("not admitted");
    };
    assert_eq!(
        admitted.exit.and_then(|exit| exit.reason),
        Some(ExitReason::ExecutorLost)
    );
    assert!(store.list().unwrap()[0].exit.is_some());
    assert_eq!(sandbox.report(&id, None), lost);
}

#[test]
fn of_two_replacing_launches_only_the_one_that_takes_the_service_over_is_named() {
    let sandbox = Sandbox::new();
    let first = RunId::generate().unwrap();
    let mut running = sandbox.spawn_executor(sandbox.dev(&[&launch_id_flag(&first)], "sleep 30"));
    sandbox.running(&first);
    let (winner, loser) = (RunId::generate().unwrap(), RunId::generate().unwrap());
    let replacer = |id: &RunId, pauses: &str| {
        for point in ["replace-chosen", "replace-stopped"] {
            sandbox.arm_in(pauses, point);
        }
        let mut command = sandbox.dev(&[&launch_id_flag(id), "--replace"], "sleep 30");
        command.env(PAUSE_DIR_ENV, sandbox.pause_dir(pauses));
        sandbox.spawn_executor(command)
    };
    let mut lost = replacer(&loser, "pause-loser");
    let mut won = replacer(&winner, "pause-winner");
    sandbox.wait_reached_in("pause-loser", "replace-chosen");
    sandbox.wait_reached_in("pause-winner", "replace-chosen");

    // The loser stops the run first, then waits before taking the lock.
    sandbox.release_in("pause-loser", "replace-chosen");
    sandbox.wait_reached_in("pause-loser", "replace-stopped");
    running.wait().unwrap();
    sandbox.release_in("pause-winner", "replace-chosen");
    sandbox.wait_reached_in("pause-winner", "replace-stopped");
    // The other one takes the lock first.
    sandbox.release_in("pause-winner", "replace-stopped");
    sandbox.running(&winner);
    sandbox.release_in("pause-loser", "replace-stopped");
    assert!(!lost.wait().unwrap().success());

    assert_eq!(
        sandbox.report(&first, None),
        LaunchReport::Replaced { by: winner.clone() }
    );
    assert_eq!(
        refused_with(&sandbox.report(&loser, None)),
        Some(RefusalCode::Conflict)
    );
    sandbox.stop();
    won.wait().unwrap();
}

#[test]
fn an_admitted_launch_runs_from_the_config_it_was_checked_against() {
    let sandbox = Sandbox::new();
    let id = RunId::generate().unwrap();
    sandbox.arm("launch-admitted");
    let mut executor = sandbox.spawn_executor(sandbox.dev(&[&launch_id_flag(&id)], "exit 0"));
    sandbox.wait_reached("launch-admitted");

    // Edited after the checks: the run never sees it.
    sandbox.edit_config("https_enabled = false", "https_enabled = true");
    sandbox.release("launch-admitted");
    assert!(
        executor.wait().unwrap().success(),
        "{}",
        sandbox.executor_log()
    );

    let record = sandbox.store().list().unwrap().remove(0);
    assert_eq!(record.url, "http://web.localify");
    assert_eq!(
        sandbox.report(&id, None),
        LaunchReport::Exited {
            code: Some(0),
            signal: None,
            reason: None
        }
    );
}

/// Kills `pid` and waits until it is gone.
fn kill_and_wait(pid: u32) {
    process::send_signal(pid, Signal::Kill);
    assert!(
        wait_until(Duration::from_secs(30), || process::process_info(pid)
            .is_none()
            .then_some(()))
        .is_some(),
        "process {pid} survived SIGKILL"
    );
}

#[test]
fn a_killed_launch_whose_helpers_nothing_proves_stays_open_and_signals_nothing() {
    let sandbox = Sandbox::new();
    let interrupted = LaunchReport::Exited {
        code: None,
        signal: None,
        reason: Some(ExitReason::InterruptedBeforeRegistration),
    };
    // Reported, with its folder and without an exit, while they live.
    let still_open = |id: &RunId, alive: &[u32]| {
        let error = reconcile(&sandbox.store(), &NativeProcesses, id, None).unwrap_err();
        assert!(
            format!("{error:#}").contains("could not be confirmed"),
            "{error:#}"
        );
        assert!(sandbox.store().run_dir(id).exists());
        let Some(LaunchRecord::Admitted(admitted)) = sandbox.launch(id) else {
            panic!("not admitted");
        };
        assert_eq!(admitted.exit, None);
        for pid in alive {
            assert!(process::process_info(*pid).is_some(), "{pid} was signalled");
        }
    };

    // Killed right after it started its helper, before it witnessed it.
    let (path, marker) = sandbox.slow_infisical();
    sandbox.arm("helper-spawned");
    let id = RunId::generate().unwrap();
    let mut executor = sandbox.spawn_slow(&id, &path);
    sandbox.wait_reached("helper-spawned");
    let (helper, descendant) = sandbox.helper_pids(&marker);
    assert!(process::send_signal(executor.id(), Signal::Kill));
    executor.wait().unwrap();
    sandbox.release("helper-spawned");
    still_open(&id, &[helper, descendant]);
    kill_and_wait(helper);
    kill_and_wait(descendant);
    assert_eq!(sandbox.report(&id, None), interrupted);
    assert!(!sandbox.store().run_dir(&id).exists());

    // A descendant its witnessed helper started after the executor died,
    // once that helper exited.
    let (go, late) = (sandbox.root.join("go"), sandbox.root.join("late.pid"));
    let path = sandbox.infisical(&format!(
        "while [ ! -e \"{go}\" ]; do sleep 0.05; done\nsleep 300 &\necho $! > \"{late}.tmp\"\nmv \"{late}.tmp\" \"{late}\"\n",
        go = go.display(),
        late = late.display()
    ));
    let id = RunId::generate().unwrap();
    let mut executor = sandbox.spawn_slow(&id, &path);
    let alive = sandbox.store().run_dir(&id).join("executor-alive");
    wait_until(Duration::from_secs(30), || alive.exists().then_some(()))
        .unwrap_or_else(|| panic!("never witnessed:\n{}", sandbox.executor_log()));
    assert!(process::send_signal(executor.id(), Signal::Kill));
    executor.wait().unwrap();
    fs::write(&go, "").unwrap();
    let born_late = wait_until(Duration::from_secs(30), || {
        fs::read_to_string(&late).ok()?.trim().parse::<u32>().ok()
    })
    .unwrap();
    let sid = executor.id();
    wait_until(Duration::from_secs(30), || {
        let members: Vec<u32> = process::all_processes()
            .iter()
            .filter(|process| process.sid == Some(sid))
            .map(|process| process.pid)
            .collect();
        (members == [born_late]).then_some(())
    })
    .expect("the helper never exited");
    still_open(&id, &[born_late]);
    kill_and_wait(born_late);
    assert_eq!(sandbox.report(&id, None), interrupted);
    assert!(!sandbox.store().run_dir(&id).exists());
    assert!(!sandbox.marker().exists());
}

#[test]
fn a_replacement_its_checks_refuse_never_stops_the_running_service() {
    let sandbox = Sandbox::new();
    let first = RunId::generate().unwrap();
    let mut running = sandbox.spawn_executor(sandbox.dev(&[&launch_id_flag(&first)], "sleep 30"));
    sandbox.running(&first);
    let touch = sandbox.touch_marker();
    let profiles =
        "[profiles.default]\nsources = []\n\n[profiles.prod]\ndanger = true\nsources = []\n";
    let refused = |flags: &[&str], says: &str| {
        let output = sandbox.dev(flags, &touch).output().unwrap();
        assert!(!output.status.success(), "{flags:?} started");
        assert!(
            stderr(&output).contains(says),
            "{flags:?}: {}",
            stderr(&output)
        );
    };

    // In the foreground and detached: a profile that does not exist, and a
    // dangerous one nobody confirmed, with no terminal.
    fs::write(sandbox.app.join("localify.toml"), profiles).unwrap();
    for detach in [&[][..], &["--detach"][..]] {
        let with = |flags: &[&'static str]| [detach, flags].concat();
        refused(&with(&["--replace", "--profile", "typo"]), "typo");
        refused(
            &with(&["--replace", "--profile", "prod"]),
            "--confirm-profile prod",
        );
    }
    // Detached: `--domain` with a named service, which only the executor
    // used to refuse, after the dispatcher had stopped the run.
    refused(
        &["--detach", "--replace", "--domain", "other.localify"],
        "--domain only applies",
    );
    // Detached: a `localify.toml` that does not load.
    fs::write(sandbox.app.join("localify.toml"), "[profiles\n").unwrap();
    refused(&["--detach", "--replace"], "localify.toml");

    assert!(running.try_wait().unwrap().is_none(), "the run was stopped");
    assert_eq!(sandbox.report(&first, None), LaunchReport::Running);
    assert!(!sandbox.marker().exists());
    sandbox.stop();
    running.wait().unwrap();
}

#[test]
fn of_two_executors_of_one_launch_the_one_that_waited_never_stops_its_twin_s_run() {
    let sandbox = Sandbox::new();
    let first = RunId::generate().unwrap();
    let mut running = sandbox.spawn_executor(sandbox.dev(&[&launch_id_flag(&first)], "sleep 30"));
    sandbox.running(&first);
    let id = RunId::generate().unwrap();
    let twin = |pauses: &str, points: &[&str]| {
        for point in points {
            sandbox.arm_in(pauses, point);
        }
        let mut command = sandbox.dev(&[&launch_id_flag(&id), "--replace"], "sleep 30");
        command.env(PAUSE_DIR_ENV, sandbox.pause_dir(pauses));
        sandbox.spawn_executor(command)
    };

    // One is held once its checks passed, while the other replaces the run
    // and is held before it publishes its decision.
    let mut late = twin("pause-late", &["launch-preflight-passed", "replace-chosen"]);
    sandbox.wait_reached_in("pause-late", "launch-preflight-passed");
    let mut early = twin("pause-early", &["launch-admission"]);
    sandbox.wait_reached_in("pause-early", "launch-admission");
    running.wait().unwrap();
    sandbox.release_in("pause-late", "launch-preflight-passed");
    thread::sleep(Duration::from_secs(1));
    assert!(
        !sandbox
            .root
            .join("pause-late/replace-chosen.reached")
            .exists(),
        "both executors of the launch reached a replacement"
    );

    // Admitted, it runs, and its twin stops nothing.
    sandbox.release_in("pause-early", "launch-admission");
    sandbox.running(&id);
    sandbox.release_in("pause-late", "replace-chosen");
    assert!(!late.wait().unwrap().success());
    assert!(
        early.try_wait().unwrap().is_none(),
        "its twin stopped the run"
    );
    assert_eq!(sandbox.report(&id, None), LaunchReport::Running);
    assert_eq!(
        sandbox.report(&first, None),
        LaunchReport::Replaced { by: id.clone() }
    );
    sandbox.stop();
    early.wait().unwrap();
}
