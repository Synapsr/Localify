//! Gates of docs/remote-machines.md, PR 1, on the executor side: concurrent
//! launches, the end of a run, interrupted capture, session reuse,
//! finalization, and native process identity. Each test gets its own home,
//! config and state, and runs the real `localify` binary.

use std::cell::RefCell;
use std::fs;
use std::io;
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use localify::branding::BRANDING;
use localify::infrastructure::process::{self, ProcessInfo, ProcessTable, Signal};
use localify::infrastructure::running_services::{RunningService, RunningServiceStore};
use localify::infrastructure::runs::RunId;
use localify::infrastructure::runs::finish::{Finish, FinishMode, Finisher, RunSession};
use localify::infrastructure::runs::output::{self, EndReason, FinalizeLock, TryFinalizeLock};

/// Children that ignore SIGINT and SIGTERM, and never listen on their port.
const STUBBORN: &str = r#"echo started
sh -c 'trap "" INT TERM; while :; do sleep 1; done' &
sh -c 'trap "" INT TERM; sleep 300' &
wait"#;

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

    /// `localify --config <sandbox> <args>`, in the sandbox's home and state.
    fn localify(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_localify"));
        command
            .arg("--config")
            .arg(&self.config)
            .args(args)
            .current_dir(&self.app)
            .env("HOME", &self.root)
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("PATH", &self.path)
            .env_remove("LOCALIFY_PROFILE")
            .env_remove("LOCALIFY_SERVICE_ORIGIN")
            .env_remove("LOCALIFY_SERVICE_OUTPUT")
            .stdin(Stdio::null());
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.localify(args).output().unwrap()
    }

    /// A foreground `localify dev` of the service, running `script`.
    fn dev(&self, script: &str) -> Command {
        self.localify(&["dev", "web", "--project", "app", "--", "sh", "-c", script])
    }

    /// An executor started as a dispatcher starts it: in its own session, as
    /// a background run, its own messages in a file.
    fn spawn_background(&self, script: &str) -> Child {
        let log = fs::File::create(self.root.join("executor.log")).unwrap();
        let mut command = self.dev(script);
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

    fn record(&self) -> Option<RunningService> {
        self.store().list().unwrap().into_iter().next()
    }

    fn wait_for_record(&self, ready: impl Fn(&RunningService) -> bool) -> RunningService {
        wait_until(Duration::from_secs(30), || {
            self.record().filter(|run| ready(run))
        })
        .unwrap_or_else(|| panic!("no record; executor said:\n{}", self.executor_log()))
    }

    fn executor_log(&self) -> String {
        fs::read_to_string(self.root.join("executor.log")).unwrap_or_default()
    }

    fn run_dir(&self, run: &RunningService) -> PathBuf {
        self.store().run_dir(run.run_id().unwrap())
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
        thread::sleep(Duration::from_millis(100));
    }
}

/// Every living process of the session, the executor's PID.
fn session_members(sid: u32) -> Vec<ProcessInfo> {
    process::all_processes()
        .into_iter()
        .filter(|process| process.sid == Some(sid))
        .collect()
}

/// Waits until the record's heartbeat is later than every member's start, so
/// the members are proven to belong to the run.
fn wait_for_proof(sandbox: &Sandbox, sid: u32, members: usize) -> RunningService {
    wait_until(Duration::from_secs(30), || {
        let record = sandbox.record()?;
        let alive = record.run.as_ref()?.last_alive_at;
        let found = session_members(sid);
        (found.len() >= members && found.iter().all(|process| process.start <= alive))
            .then_some(record)
    })
    .unwrap_or_else(|| {
        panic!(
            "no heartbeat after the members started:\n{}",
            sandbox.executor_log()
        )
    })
}

fn kill_executor(executor: &mut Child) {
    assert!(process::send_signal(executor.id(), Signal::Kill));
    executor.wait().unwrap();
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn concurrent_launches_admit_exactly_one() {
    let sandbox = Sandbox::new();
    let mut launches: Vec<Child> = (0..4)
        .map(|_| {
            sandbox
                .dev("sleep 30")
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();

    let count = launches.len();
    let refused = wait_until(Duration::from_secs(30), || {
        let exited = launches
            .iter_mut()
            .filter_map(|child| child.try_wait().unwrap())
            .count();
        (exited == count - 1).then_some(exited)
    });
    thread::sleep(Duration::from_millis(500));

    assert_eq!(refused, Some(3));
    let (running, ended): (Vec<_>, Vec<_>) = launches
        .into_iter()
        .partition(|child| child.id() == sandbox.record().unwrap().pid);
    assert_eq!(running.len(), 1);
    for child in ended {
        let output = child.wait_with_output().unwrap();
        assert!(!output.status.success());
        assert!(
            stderr(&output).contains("already running"),
            "{}",
            stderr(&output)
        );
    }
    let stop = sandbox.run(&["stop", "web", "--project", "app"]);
    assert!(stop.status.success(), "{}", stderr(&stop));
    for mut child in running {
        child.wait().unwrap();
    }
}

#[test]
fn a_run_record_that_cannot_be_written_starts_nothing() {
    if process::is_root() {
        return;
    }
    let sandbox = Sandbox::new();
    let first = sandbox.dev("exit 0").output().unwrap();
    assert!(first.status.success(), "{}", stderr(&first));
    let services = sandbox.services_dir();
    let marker = sandbox.root.join("started");

    fs::set_permissions(&services, fs::Permissions::from_mode(0o555)).unwrap();
    let output = sandbox
        .dev(&format!("touch '{}'", marker.display()))
        .output()
        .unwrap();
    fs::set_permissions(&services, fs::Permissions::from_mode(0o755)).unwrap();

    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("Failed to write"),
        "{}",
        stderr(&output)
    );
    assert!(!marker.exists(), "the service started: {}", stderr(&output));
}

#[test]
fn the_end_of_a_run_whose_executor_was_killed() {
    let sandbox = Sandbox::new();
    let mut executor = sandbox.spawn_background(STUBBORN);
    let sid = executor.id();
    let record = wait_for_proof(&sandbox, sid, 5);
    assert_eq!(record.run.as_ref().unwrap().sid, Some(sid));

    kill_executor(&mut executor);
    let left = session_members(sid);
    assert!(left.len() >= 4, "{left:?}");

    let between = sandbox.dev("exit 0").output().unwrap();
    assert!(!between.status.success());
    assert!(
        stderr(&between).contains("still has processes"),
        "{}",
        stderr(&between)
    );

    let finished = sandbox.store().finish_background_runs().unwrap();

    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0].1, Finish::Finalized(EndReason::Interrupted));
    assert!(
        session_members(sid).is_empty(),
        "{:?}",
        session_members(sid)
    );
    let end = output::read_end(&sandbox.run_dir(&record))
        .unwrap()
        .unwrap();
    assert_eq!(end.reason, EndReason::Interrupted);

    let after = sandbox.dev("exit 0").output().unwrap();
    assert!(after.status.success(), "{}", stderr(&after));
    assert_ne!(sandbox.record().unwrap().run_id(), record.run_id());
}

#[test]
fn a_killed_background_executor_is_finalized_interrupted_at_what_it_wrote() {
    let sandbox = Sandbox::new();
    let script =
        r#"i=0; while [ $i -lt 200 ]; do echo "line $i"; i=$((i+1)); done; echo ready; sleep 300"#;
    let mut executor = sandbox.spawn_background(script);
    let record = sandbox.wait_for_record(|run| run.has_stream());
    let dir = sandbox.run_dir(&record);
    let written = wait_until(Duration::from_secs(30), || {
        let read = output::read(&dir, 0, output::MAX_READ).ok()?;
        let ready = read.lines.last().is_some_and(|line| line.text == "ready");
        ready.then_some(read.next)
    })
    .expect("the service's output never reached the stream");

    kill_executor(&mut executor);
    let refused = sandbox.dev("exit 0").output().unwrap();
    assert!(!refused.status.success(), "admitted before finalization");

    let started = Instant::now();
    let record = sandbox.record().unwrap();
    assert!(sandbox.store().stop(&record).unwrap());

    assert!(started.elapsed() < Duration::from_secs(8));
    let end = output::read_end(&dir).unwrap().unwrap();
    assert_eq!(end.reason, EndReason::Interrupted);
    assert_eq!(end.offset, written);
    let read = output::read(&dir, 0, output::MAX_READ).unwrap();
    assert_eq!(read.lines.len(), 201);
    assert!(read.ended && read.interrupted);
    let after = sandbox.dev("exit 0").output().unwrap();
    assert!(after.status.success(), "{}", stderr(&after));
}

#[test]
fn a_finisher_killed_mid_way_lets_the_next_one_complete_and_never_two_at_once() {
    let sandbox = Sandbox::new();
    let mut executor = sandbox.spawn_background(STUBBORN);
    let sid = executor.id();
    let record = wait_for_proof(&sandbox, sid, 5);
    let dir = sandbox.run_dir(&record);
    kill_executor(&mut executor);

    // The first finisher holds the run's lock through its 10-second grace period.
    let mut first = sandbox
        .localify(&["stop", "web", "--project", "app"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let held = wait_until(Duration::from_secs(10), || {
        matches!(
            FinalizeLock::try_acquire(&dir).unwrap(),
            TryFinalizeLock::Busy
        )
        .then_some(())
    });
    assert!(held.is_some(), "the first finisher never took the lock");

    let services = sandbox.services_dir();
    let second = thread::spawn(move || {
        RunningServiceStore::new(services)
            .finish_background_runs()
            .unwrap()
    });
    thread::sleep(Duration::from_secs(1));
    assert!(!second.is_finished(), "two finishers ran at once");
    assert!(output::read_end(&dir).unwrap().is_none());

    kill_executor(&mut first);
    let finished = second.join().unwrap();

    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0].1, Finish::Finalized(EndReason::Interrupted));
    assert!(
        session_members(sid).is_empty(),
        "{:?}",
        session_members(sid)
    );
    assert!(output::read_end(&dir).unwrap().is_some());
}

#[test]
fn a_background_run_reads_by_run_id_and_offset_and_stops_complete() {
    if Command::new("python3").arg("--version").output().is_err() {
        return;
    }
    let sandbox = Sandbox::new();
    let script = r#"echo hello-from-web; exec python3 -u -m http.server "$PORT" --bind 127.0.0.1"#;

    let started = sandbox
        .localify(&[
            "dev",
            "web",
            "--project",
            "app",
            "--detach",
            "--",
            "sh",
            "-c",
            script,
        ])
        .output()
        .unwrap();
    assert!(started.status.success(), "{}", stderr(&started));
    assert!(String::from_utf8_lossy(&started.stdout).contains("Started 'web'"));

    let logs = sandbox.run(&["logs", "web", "--project", "app"]);
    assert!(logs.status.success(), "{}", stderr(&logs));
    assert!(String::from_utf8_lossy(&logs.stdout).contains("hello-from-web"));
    let cursor = stderr(&logs)
        .lines()
        .find_map(|line| line.strip_prefix("Next cursor: ").map(str::to_string))
        .expect("logs printed no cursor");
    let run_id = cursor.split(':').next().unwrap();
    assert_eq!(
        Some(run_id),
        sandbox.record().unwrap().run_id().map(RunId::as_str)
    );

    let again = sandbox.run(&["logs", "web", "--project", "app", "--cursor", &cursor]);
    assert!(again.status.success(), "{}", stderr(&again));
    assert!(!String::from_utf8_lossy(&again.stdout).contains("hello-from-web"));

    let stop = sandbox.run(&["stop", "web", "--project", "app"]);
    assert!(stop.status.success(), "{}", stderr(&stop));
    let record = sandbox.record().unwrap();
    let end = output::read_end(&sandbox.run_dir(&record))
        .unwrap()
        .unwrap();
    assert_eq!(end.reason, EndReason::Complete);
    assert!(record.exit.is_some());
}

#[test]
fn identity_and_heartbeats_are_native_and_spawn_no_ps() {
    let mut sandbox = Sandbox::new();
    let fake_bin = sandbox.root.join("fake-bin");
    let marker = sandbox.root.join("spawned");
    fs::create_dir_all(&fake_bin).unwrap();
    for tool in ["ps", "pgrep", "kill", "lsof"] {
        let path = fake_bin.join(tool);
        fs::write(
            &path,
            format!("#!/bin/sh\necho {tool} >> '{}'\nexit 1\n", marker.display()),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    sandbox.path = format!("{}:/usr/bin:/bin", fake_bin.display());

    let mut executor = sandbox.spawn_background("sleep 30");
    let record = sandbox.wait_for_record(|run| run.run_id().is_some());
    let identity = record.run.clone().unwrap();
    assert_eq!(
        Some(identity.start),
        process::process_info(executor.id()).map(|process| process.start)
    );
    assert_eq!(Some(identity.boot_id.clone()), process::boot_id());
    assert!(identity.last_alive_at <= process::monotonic_now());

    let status = sandbox.run(&["services", "status", "--json"]);
    assert!(String::from_utf8_lossy(&status.stdout).contains("\"running\": true"));
    let stop = sandbox.run(&["stop", "web", "--project", "app"]);
    assert!(stop.status.success(), "{}", stderr(&stop));
    executor.wait().unwrap();

    assert!(
        !marker.exists(),
        "spawned: {}",
        fs::read_to_string(&marker).unwrap_or_default()
    );
}

/// A process table a test controls: PIDs and sessions reused at will, and
/// processes that die when signalled, unless they ignore SIGTERM.
struct FakeTable {
    boot_id: String,
    processes: RefCell<Vec<(ProcessInfo, bool)>>,
    signals: RefCell<Vec<(u32, Signal)>>,
    /// What the first lookup of one process sees, before the session was
    /// replaced by `processes`.
    first_lookup: RefCell<Option<Vec<ProcessInfo>>>,
}

impl FakeTable {
    fn new(boot_id: &str, processes: Vec<(ProcessInfo, bool)>) -> Self {
        Self {
            boot_id: boot_id.into(),
            processes: processes.into(),
            signals: Vec::new().into(),
            first_lookup: None.into(),
        }
    }
}

impl ProcessTable for FakeTable {
    fn processes(&self) -> Vec<ProcessInfo> {
        self.processes
            .borrow()
            .iter()
            .map(|(process, _)| *process)
            .collect()
    }

    fn process(&self, pid: u32) -> Option<ProcessInfo> {
        if let Some(before) = self.first_lookup.borrow_mut().take() {
            return before.into_iter().find(|process| process.pid == pid);
        }
        self.processes()
            .into_iter()
            .find(|process| process.pid == pid)
    }

    fn signal(&self, pid: u32, signal: Signal) {
        self.signals.borrow_mut().push((pid, signal));
        self.processes
            .borrow_mut()
            .retain(|(process, ignores_term)| {
                process.pid != pid || (*ignores_term && signal == Signal::Terminate)
            });
    }

    fn boot_id(&self) -> Option<String> {
        Some(self.boot_id.clone())
    }
}

fn fake_process(pid: u32, sid: u32, start: u64) -> ProcessInfo {
    ProcessInfo {
        pid,
        ppid: 1,
        pgid: pid,
        sid: Some(sid),
        start,
        // A wall clock moved backwards: these look older than anything recorded.
        started_wall: 0,
    }
}

fn recorded_session(boot_id: &str) -> RunSession {
    RunSession {
        executor_pid: 4242,
        executor_start: 100,
        sid: Some(4242),
        pgid: Some(4242),
        boot_id: boot_id.into(),
        last_alive_at: 1_000,
    }
}

fn finisher(table: &FakeTable) -> Finisher<'_> {
    Finisher {
        table,
        grace: Duration::from_millis(200),
    }
}

fn run_folder() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    output::create_run_dir(dir.path()).unwrap();
    dir
}

#[test]
fn a_reused_session_id_is_never_signalled() {
    let dir = run_folder();
    // The executor's PID now leads an unrelated session, every member of it
    // started after the run's last heartbeat.
    let table = FakeTable::new(
        "boot-a",
        vec![
            (fake_process(4242, 4242, 5_000), false),
            (fake_process(4243, 4242, 5_100), false),
        ],
    );

    let finish = finisher(&table)
        .finish(
            dir.path(),
            &recorded_session("boot-a"),
            true,
            FinishMode::End,
        )
        .unwrap();

    assert_eq!(finish, Finish::Finalized(EndReason::Interrupted));
    assert!(table.signals.borrow().is_empty(), "{:?}", table.signals);
    assert_eq!(table.processes().len(), 2);
}

#[test]
fn a_record_from_another_boot_signals_nothing() {
    let dir = run_folder();
    // Started "before" the heartbeat, but in another boot's time.
    let table = FakeTable::new("boot-b", vec![(fake_process(4243, 4242, 500), true)]);

    let finish = finisher(&table)
        .finish(
            dir.path(),
            &recorded_session("boot-a"),
            false,
            FinishMode::End,
        )
        .unwrap();

    assert_eq!(finish, Finish::Finalized(EndReason::Complete));
    assert!(table.signals.borrow().is_empty());
}

#[test]
fn proven_members_are_ended_but_the_next_launcher_only_refuses() {
    let dir = run_folder();
    let table = FakeTable::new(
        "boot-a",
        vec![
            (fake_process(4243, 4242, 900), true),
            (fake_process(4250, 4242, 2_000), false),
        ],
    );
    let session = recorded_session("boot-a");

    let admit = finisher(&table)
        .finish(dir.path(), &session, true, FinishMode::Admit)
        .unwrap();
    assert_eq!(admit, Finish::LiveMembers(vec![4243, 4250]));
    assert!(table.signals.borrow().is_empty());
    assert!(output::read_end(dir.path()).unwrap().is_none());

    let end = finisher(&table)
        .finish(dir.path(), &session, true, FinishMode::End)
        .unwrap();

    assert_eq!(end, Finish::Finalized(EndReason::Interrupted));
    assert!(table.processes().is_empty());
    let signals = table.signals.borrow();
    assert!(signals.contains(&(4243, Signal::Terminate)));
    assert!(signals.contains(&(4243, Signal::Kill)));
    assert!(signals.contains(&(4250, Signal::Terminate)));
}

#[test]
fn a_session_replaced_right_after_the_executor_was_seen_is_never_signalled() {
    let dir = run_folder();
    // Alive when first looked up...
    let executor = fake_process(4242, 4242, 100);
    // ...then gone, its PID and session id carried by unrelated processes.
    let table = FakeTable::new(
        "boot-a",
        vec![
            (fake_process(4242, 4242, 5_000), false),
            (fake_process(4243, 4242, 5_100), false),
        ],
    );
    *table.first_lookup.borrow_mut() = Some(vec![executor]);

    let _ = finisher(&table).stop(dir.path(), &recorded_session("boot-a"), true);

    assert!(table.signals.borrow().is_empty(), "{:?}", table.signals);
    assert_eq!(table.processes().len(), 2);
}

#[test]
fn stopping_a_run_ends_a_descendant_that_left_its_session_and_ignores_sigterm() {
    let sandbox = Sandbox::new();
    let pid_file = sandbox.root.join("escaped.pid");
    let script = format!(
        r#"perl -MPOSIX -e 'POSIX::setsid(); $SIG{{TERM}} = "IGNORE"; open(my $f, ">", "{}"); print $f $$; close($f); sleep 300' &
wait"#,
        pid_file.display()
    );
    let mut executor = sandbox.spawn_background(&script);
    let escaped: u32 = wait_until(Duration::from_secs(20), || {
        fs::read_to_string(&pid_file).ok()?.trim().parse().ok()
    })
    .expect("the descendant never started");
    let left_session = process::process_info(escaped).unwrap();
    assert_eq!(left_session.sid, Some(escaped));
    let record = sandbox.wait_for_record(|run| run.run_id().is_some());

    assert!(sandbox.store().stop(&record).unwrap());

    executor.wait().unwrap();
    let survivor = process::process_info(escaped).filter(|now| now.start == left_session.start);
    assert!(survivor.is_none(), "{survivor:?} survived");
}

#[test]
fn a_foreground_run_exits_with_its_service_s_code_or_130_after_ctrl_c() {
    let sandbox = Sandbox::new();

    let failed = sandbox.dev("exit 3").output().unwrap();
    assert_eq!(failed.status.code(), Some(3), "{}", stderr(&failed));
    let killed = sandbox.dev("kill -TERM $$").output().unwrap();
    assert_eq!(killed.status.code(), Some(143), "{}", stderr(&killed));

    // Ctrl+C in a terminal reaches the whole foreground job.
    let mut job = sandbox
        .dev("sleep 30")
        .process_group(0)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    sandbox.wait_for_record(|run| run.pid == job.id());
    thread::sleep(Duration::from_millis(300));
    let group = libc::pid_t::try_from(job.id()).unwrap();
    // SAFETY: a negative PID names the job's own process group, made above.
    assert_eq!(unsafe { libc::kill(-group, libc::SIGINT) }, 0);

    assert_eq!(job.wait().unwrap().code(), Some(130));
}

#[test]
fn last_alive_at_is_refreshed_while_the_run_lives() {
    let sandbox = Sandbox::new();
    let mut executor = sandbox.spawn_background("sleep 30");
    let first = sandbox.wait_for_record(|run| run.run_id().is_some());
    let first_alive = first.run.as_ref().unwrap().last_alive_at;

    let refreshed = wait_until(Duration::from_secs(15), || {
        let alive = sandbox.record()?.run?.last_alive_at;
        (alive > first_alive).then_some(alive)
    });

    assert!(refreshed.is_some(), "lastAliveAt never moved");
    assert!(refreshed.unwrap() <= process::monotonic_now());
    sandbox.store().stop(&first).unwrap();
    executor.wait().unwrap();
}
