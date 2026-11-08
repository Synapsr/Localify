//! Gates of docs/remote-machines.md, PR 1, on the executor side: concurrent
//! launches, the end of a run, interrupted capture, session reuse,
//! finalization, and native process identity. Each test gets its own home,
//! config and state, and runs the real `localify` binary.

use std::cell::RefCell;
use std::fs;
use std::io;
use std::net::TcpListener;
use std::os::fd::FromRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::Mutex;
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
        let mut command = self.in_sandbox(env!("CARGO_BIN_EXE_localify"));
        command.arg("--config").arg(&self.config).args(args);
        command
    }

    /// Any program, in the sandbox's home, state and app folder.
    fn in_sandbox(&self, program: &str) -> Command {
        let mut command = Command::new(program);
        command
            .current_dir(&self.app)
            .env("HOME", &self.root)
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("PATH", &self.path)
            .env_remove("LOCALIFY_PROFILE")
            .env_remove("LOCALIFY_SERVICE_ORIGIN")
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

    let finished = sandbox.store().finish_unread_runs().unwrap();

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
            .finish_unread_runs()
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

    // In one argument: a run id may start with `-`.
    let cursor_arg = format!("--cursor={cursor}");
    let again = sandbox.run(&["logs", "web", "--project", "app", &cursor_arg]);
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

/// Now, in a fake table: later than every start these tests make up.
const FAKE_NOW: u64 = 1_000_000;

/// A process table a test controls: PIDs and sessions reused at will, and
/// processes that die when signalled, unless they ignore SIGTERM.
struct FakeTable {
    boot_id: String,
    processes: RefCell<Vec<(ProcessInfo, bool)>>,
    signals: RefCell<Vec<(u32, Signal)>>,
    /// What the first lookup of one process sees, before the session was
    /// replaced by `processes`.
    first_lookup: RefCell<Option<Vec<ProcessInfo>>>,
    /// What the table holds once the next enumeration is over: a scan that
    /// outlives its own witness.
    after_scan: RefCell<Option<Vec<(ProcessInfo, bool)>>>,
    /// A process `pid` starts when it gets SIGTERM.
    fork_on_term: RefCell<Option<(u32, (ProcessInfo, bool))>>,
    /// A process `pid` started, which no scan saw until `pid` got SIGKILL.
    fork_on_kill: RefCell<Option<(u32, (ProcessInfo, bool))>>,
    /// The finisher dies as it sends this signal to this process.
    die_on_signal: std::cell::Cell<Option<(u32, Signal)>>,
    /// The finisher dies right after this signal reached this process.
    die_after_signal: std::cell::Cell<Option<(u32, Signal)>>,
    /// Processes every scan shows, but that are gone when looked up.
    vanish_on_lookup: RefCell<Vec<u32>>,
}

impl FakeTable {
    fn new(boot_id: &str, processes: Vec<(ProcessInfo, bool)>) -> Self {
        Self {
            boot_id: boot_id.into(),
            processes: processes.into(),
            signals: Vec::new().into(),
            first_lookup: None.into(),
            after_scan: None.into(),
            fork_on_term: None.into(),
            fork_on_kill: None.into(),
            die_on_signal: None.into(),
            die_after_signal: None.into(),
            vanish_on_lookup: Vec::new().into(),
        }
    }

    fn current(&self) -> Vec<ProcessInfo> {
        self.processes
            .borrow()
            .iter()
            .map(|(process, _)| *process)
            .collect()
    }
}

impl ProcessTable for FakeTable {
    fn processes(&self) -> Vec<ProcessInfo> {
        let scan = self.current();
        if let Some(after) = self.after_scan.borrow_mut().take() {
            *self.processes.borrow_mut() = after;
        }
        scan
    }

    fn process(&self, pid: u32) -> Option<ProcessInfo> {
        if self.vanish_on_lookup.borrow().contains(&pid) {
            return None;
        }
        if let Some(before) = self.first_lookup.borrow_mut().take() {
            return before.into_iter().find(|process| process.pid == pid);
        }
        self.current()
            .into_iter()
            .find(|process| process.pid == pid)
    }

    fn signal(&self, pid: u32, signal: Signal) {
        if self.die_on_signal.get() == Some((pid, signal)) {
            self.die_on_signal.set(None);
            panic!("the finisher died");
        }
        self.signals.borrow_mut().push((pid, signal));
        let forks = self
            .fork_on_term
            .borrow()
            .filter(|(parent, _)| *parent == pid && signal == Signal::Terminate);
        if let Some((_, child)) = forks {
            self.fork_on_term.borrow_mut().take();
            self.processes.borrow_mut().push(child);
        }
        let missed = self
            .fork_on_kill
            .borrow()
            .filter(|(parent, _)| *parent == pid && signal == Signal::Kill);
        if let Some((_, child)) = missed {
            self.fork_on_kill.borrow_mut().take();
            self.processes.borrow_mut().push(child);
        }
        self.processes
            .borrow_mut()
            .retain(|(process, ignores_term)| {
                process.pid != pid || (*ignores_term && signal == Signal::Terminate)
            });
        if self.die_after_signal.get() == Some((pid, signal)) {
            self.die_after_signal.set(None);
            panic!("the finisher died");
        }
    }

    fn boot_id(&self) -> Option<String> {
        Some(self.boot_id.clone())
    }

    fn now(&self) -> u64 {
        FAKE_NOW
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

#[test]
fn a_witness_that_vanishes_during_the_scan_proves_nothing() {
    let dir = run_folder();
    // The scan sees the old member, then a process of a replacement session...
    let table = FakeTable::new(
        "boot-a",
        vec![
            (fake_process(4243, 4242, 900), true),
            (fake_process(4244, 4242, 6_000), false),
        ],
    );
    // ...because the old member died while the scan went on.
    *table.after_scan.borrow_mut() = Some(vec![
        (fake_process(4244, 4242, 6_000), false),
        (fake_process(4245, 4242, 6_100), false),
    ]);

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
}

#[test]
fn a_worker_forked_by_an_escaped_descendant_during_shutdown_is_ended_too() {
    let dir = run_folder();
    // The executor dies of its SIGTERM; its descendant that left the session
    // ignores it, and forks a worker as it gets it.
    let table = FakeTable::new(
        "boot-a",
        vec![
            (fake_process(4242, 4242, 100), false),
            (escaped(5000, 4242, 2_000), true),
        ],
    );
    *table.fork_on_term.borrow_mut() = Some((5000, (escaped(5001, 5000, 2_100), true)));

    let finish = finisher(&table)
        .stop(dir.path(), &recorded_session("boot-a"), true)
        .unwrap();

    assert_eq!(finish, Finish::Finalized(EndReason::Interrupted));
    assert!(table.current().is_empty(), "{:?}", table.current());
    assert!(table.signals.borrow().contains(&(5001, Signal::Kill)));
}

#[test]
fn a_successor_ends_what_a_finisher_that_died_mid_way_had_proven() {
    let dir = run_folder();
    // Heartbeat 1000; the witness started at 900 and leaves on SIGTERM; the
    // worker started at 1500, after the heartbeat, and ignores it.
    let table = FakeTable::new(
        "boot-a",
        vec![
            (fake_process(4243, 4242, 900), false),
            (fake_process(4247, 4242, 1_500), true),
        ],
    );
    // The first finisher dies as it sends its first SIGTERM to the worker.
    table.die_on_signal.set(Some((4247, Signal::Terminate)));
    let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        finisher(&table).finish(
            dir.path(),
            &recorded_session("boot-a"),
            true,
            FinishMode::End,
        )
    }));
    assert!(crashed.is_err(), "the first finisher was meant to die");
    assert_eq!(table.current().len(), 1, "{:?}", table.current());

    let next = finisher(&table)
        .finish(
            dir.path(),
            &recorded_session("boot-a"),
            true,
            FinishMode::End,
        )
        .unwrap();

    assert_eq!(next, Finish::Finalized(EndReason::Interrupted));
    assert!(table.current().is_empty(), "{:?}", table.current());
    assert!(table.signals.borrow().contains(&(4247, Signal::Kill)));
}

#[test]
fn a_stable_witness_proves_the_run_when_earlier_ones_keep_vanishing() {
    let dir = run_folder();
    // 4243 and 4244 show in every scan but are gone when read again; 4245,
    // started before the heartbeat too, is stable.
    let table = FakeTable::new(
        "boot-a",
        vec![
            (fake_process(4243, 4242, 800), true),
            (fake_process(4244, 4242, 850), true),
            (fake_process(4245, 4242, 950), true),
        ],
    );
    *table.vanish_on_lookup.borrow_mut() = vec![4243, 4244];

    let admit = finisher(&table)
        .finish(
            dir.path(),
            &recorded_session("boot-a"),
            true,
            FinishMode::Admit,
        )
        .unwrap();

    assert_eq!(admit, Finish::LiveMembers(vec![4243, 4244, 4245]));
    assert!(output::read_end(dir.path()).unwrap().is_none());
}

#[test]
fn witnesses_that_always_vanish_leave_the_run_open() {
    let dir = run_folder();
    let table = FakeTable::new("boot-a", vec![(fake_process(4243, 4242, 800), true)]);
    *table.vanish_on_lookup.borrow_mut() = vec![4243];

    let error = finisher(&table)
        .finish(
            dir.path(),
            &recorded_session("boot-a"),
            true,
            FinishMode::Admit,
        )
        .unwrap_err();

    assert!(error.to_string().contains("Try again"), "{error}");
    assert!(output::read_end(dir.path()).unwrap().is_none());
}

#[test]
fn a_start_in_the_tick_of_the_last_proof_proves_nothing() {
    let dir = run_folder();
    // Started exactly when the session was last proven, and first seen once
    // its parent was killed: nothing ties it to the run.
    let table = FakeTable::new("boot-a", vec![(fake_process(4243, 4242, 900), true)]);
    *table.fork_on_kill.borrow_mut() = Some((4243, (fake_process(4246, 4242, FAKE_NOW), true)));

    finisher(&table)
        .finish(
            dir.path(),
            &recorded_session("boot-a"),
            true,
            FinishMode::End,
        )
        .unwrap();

    assert!(!table.signals.borrow().iter().any(|(pid, _)| *pid == 4246));
}

#[test]
fn a_process_in_the_run_s_group_but_another_session_is_not_a_member() {
    let dir = run_folder();
    let stranger = ProcessInfo {
        pgid: 4242,
        ..fake_process(4243, 9999, 900)
    };
    let table = FakeTable::new("boot-a", vec![(stranger, false)]);

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
}

#[test]
fn a_child_first_seen_after_its_parent_died_is_still_ended() {
    let dir = run_folder();
    // A proven member that ignores SIGTERM, with a child it started while it
    // was proven alive, which every scan missed until the member was killed.
    let table = FakeTable::new("boot-a", vec![(fake_process(4243, 4242, 900), true)]);
    *table.fork_on_kill.borrow_mut() = Some((4243, (fake_process(4246, 4242, 1_500), true)));

    let finish = finisher(&table)
        .finish(
            dir.path(),
            &recorded_session("boot-a"),
            true,
            FinishMode::End,
        )
        .unwrap();

    assert_eq!(finish, Finish::Finalized(EndReason::Interrupted));
    assert!(table.current().is_empty(), "{:?}", table.current());
    assert!(table.signals.borrow().contains(&(4246, Signal::Kill)));
}

/// A process of the run's executor that left its session.
fn escaped(pid: u32, ppid: u32, start: u64) -> ProcessInfo {
    ProcessInfo {
        ppid,
        ..fake_process(pid, pid, start)
    }
}

/// A process table shared between finishers on two threads.
struct SharedTable {
    /// Each process, and whether SIGTERM ends it.
    processes: Mutex<Vec<(ProcessInfo, bool)>>,
    signals: Mutex<Vec<(u32, Signal, Instant)>>,
}

impl ProcessTable for SharedTable {
    fn processes(&self) -> Vec<ProcessInfo> {
        let processes = self.processes.lock().unwrap();
        processes.iter().map(|(process, _)| *process).collect()
    }

    fn process(&self, pid: u32) -> Option<ProcessInfo> {
        self.processes()
            .into_iter()
            .find(|process| process.pid == pid)
    }

    fn signal(&self, pid: u32, signal: Signal) {
        self.signals
            .lock()
            .unwrap()
            .push((pid, signal, Instant::now()));
        self.processes
            .lock()
            .unwrap()
            .retain(|(process, dies_on_term)| {
                process.pid != pid || !(signal == Signal::Kill || *dies_on_term)
            });
    }

    fn boot_id(&self) -> Option<String> {
        Some("boot-a".into())
    }

    fn now(&self) -> u64 {
        1_000_000
    }
}

#[test]
fn a_stop_whose_executor_dies_goes_on_only_under_the_lock() {
    let dir = run_folder();
    let table = SharedTable {
        processes: Mutex::new(vec![
            (fake_process(4242, 4242, 100), true),
            (fake_process(4243, 4242, 900), false),
        ]),
        signals: Mutex::new(Vec::new()),
    };

    thread::scope(|scope| {
        let stopping = scope.spawn(|| {
            let finisher = Finisher {
                table: &table,
                grace: Duration::from_millis(300),
            };
            finisher.stop(dir.path(), &recorded_session("boot-a"), true)
        });
        // The executor dies of its SIGTERM; another finisher then takes the lock.
        let died = wait_until(Duration::from_secs(5), || {
            let signals = table.signals.lock().unwrap();
            signals
                .iter()
                .any(|(pid, signal, _)| *pid == 4242 && *signal == Signal::Terminate)
                .then_some(())
        });
        assert!(died.is_some(), "the executor was never signalled");
        let other = FinalizeLock::acquire(dir.path()).unwrap().unwrap();
        let from = Instant::now();
        thread::sleep(Duration::from_secs(6));
        let to = Instant::now();
        drop(other);

        let finish = stopping.join().unwrap().unwrap();

        assert_eq!(finish, Finish::Finalized(EndReason::Interrupted));
        let signals = table.signals.lock().unwrap();
        let meanwhile: Vec<_> = signals
            .iter()
            .filter(|(_, _, at)| *at >= from && *at <= to)
            .collect();
        assert!(
            meanwhile.is_empty(),
            "signalled while another finisher held the lock: {meanwhile:?}"
        );
        assert!(table.processes().is_empty());
    });
}

#[test]
fn a_detached_start_exits_with_the_executor_s_status() {
    let sandbox = Sandbox::new();
    let detached = |script: &str| {
        sandbox
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
            .unwrap()
    };

    let failed = detached("exit 3");
    assert_eq!(failed.status.code(), Some(3), "{}", stderr(&failed));
    let killed = detached("kill -TERM $$");
    assert_eq!(killed.status.code(), Some(143), "{}", stderr(&killed));
}

#[test]
fn a_launch_id_that_starts_with_a_dash_reaches_the_run() {
    let sandbox = Sandbox::new();
    let launch_id = "-qAAAAAAAAAAAAAAAAAAAA";

    let output = sandbox
        .localify(&[
            "dev",
            "web",
            "--project",
            "app",
            &format!("--launch-id={launch_id}"),
            "--",
            "sh",
            "-c",
            "exit 0",
        ])
        .output()
        .unwrap();

    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        sandbox.record().unwrap().run_id().map(RunId::as_str),
        Some(launch_id)
    );
}

#[test]
fn the_horizon_is_saved_before_the_signal_that_ends_its_witness() {
    let dir = run_folder();
    // The witness leaves on SIGTERM, forking a worker as it goes; the worker
    // started after the heartbeat, before the witness was read again.
    let table = FakeTable::new("boot-a", vec![(fake_process(4243, 4242, 900), false)]);
    *table.fork_on_term.borrow_mut() = Some((4243, (fake_process(4247, 4242, 1_500), true)));
    // The finisher dies right after that SIGTERM.
    table.die_after_signal.set(Some((4243, Signal::Terminate)));
    let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        finisher(&table).finish(
            dir.path(),
            &recorded_session("boot-a"),
            true,
            FinishMode::End,
        )
    }));
    assert!(crashed.is_err(), "the first finisher was meant to die");

    finisher(&table)
        .finish(
            dir.path(),
            &recorded_session("boot-a"),
            true,
            FinishMode::End,
        )
        .unwrap();

    assert!(table.current().is_empty(), "{:?}", table.current());
    assert!(table.signals.borrow().contains(&(4247, Signal::Kill)));
}

/// What a finisher of an earlier boot saved: an escaped descendant, whose PID
/// and start an unrelated process of this boot now has.
fn saved_by_another_boot(dir: &Path) {
    fs::write(
        dir.join("finalize-state.json"),
        r#"{"provenThrough":1000,"targets":[{"pid":5000,"start":2000,"member":false}]}"#,
    )
    .unwrap();
}

#[test]
fn a_finisher_of_another_boot_s_run_trusts_nothing_it_saved() {
    let dir = run_folder();
    saved_by_another_boot(dir.path());
    let table = FakeTable::new("boot-b", vec![(escaped(5000, 1, 2_000), false)]);

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
    assert!(!dir.path().join("finalize-state.json").exists());
}

#[test]
fn an_already_finalized_run_of_another_boot_signals_nothing() {
    let dir = run_folder();
    saved_by_another_boot(dir.path());
    output::publish_end(dir.path(), &output::RunEnd::now(EndReason::Complete, 0)).unwrap();
    let table = FakeTable::new("boot-b", vec![(escaped(5000, 1, 2_000), false)]);

    let finish = finisher(&table)
        .finish(
            dir.path(),
            &recorded_session("boot-a"),
            true,
            FinishMode::End,
        )
        .unwrap();

    assert_eq!(finish, Finish::AlreadyFinalized);
    assert!(table.signals.borrow().is_empty(), "{:?}", table.signals);
}

#[test]
fn an_already_finalized_run_ends_only_the_exact_processes_saved() {
    let dir = run_folder();
    saved_by_another_boot(dir.path());
    output::publish_end(dir.path(), &output::RunEnd::now(EndReason::Complete, 0)).unwrap();
    // Same boot: the saved escaped descendant, and a process in the session
    // that nothing saved names.
    let table = FakeTable::new(
        "boot-a",
        vec![
            (escaped(5000, 1, 2_000), false),
            (fake_process(4243, 4242, 500), false),
        ],
    );

    finisher(&table)
        .finish(
            dir.path(),
            &recorded_session("boot-a"),
            true,
            FinishMode::End,
        )
        .unwrap();

    let signals = table.signals.borrow();
    assert!(signals.contains(&(5000, Signal::Terminate)));
    assert!(!signals.iter().any(|(pid, _)| *pid == 4243), "{signals:?}");
}

/// A pseudo-terminal: its master, and its slave for a child to use.
fn open_pty() -> (fs::File, fs::File) {
    let (mut master, mut slave) = (0, 0);
    // SAFETY: openpty writes two descriptors into the integers; a null name,
    // termios and window size mean the defaults.
    let opened = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(opened, 0, "{}", io::Error::last_os_error());
    // SAFETY: both descriptors were just opened and are owned by nothing else.
    unsafe { (fs::File::from_raw_fd(master), fs::File::from_raw_fd(slave)) }
}

/// A service that writes `ready` once it traps SIGINT, a line to `count` for
/// each SIGINT it gets, and ends by itself after about 3 seconds.
struct CountingService {
    ready: PathBuf,
    count: PathBuf,
}

impl CountingService {
    fn new(sandbox: &Sandbox) -> Self {
        Self {
            ready: sandbox.root.join("ready"),
            count: sandbox.root.join("interrupts"),
        }
    }

    fn script(&self) -> String {
        format!(
            "trap \"echo int >> {}\" INT; touch {}; i=0; while [ $i -lt 30 ]; do sleep 0.1; i=$((i+1)); done",
            self.count.display(),
            self.ready.display()
        )
    }

    fn wait_until_ready(&self) {
        let ready = wait_until(Duration::from_secs(30), || {
            self.ready.exists().then_some(())
        });
        assert!(ready.is_some(), "the service never trapped SIGINT");
    }

    fn interrupts(&self) -> usize {
        fs::read_to_string(&self.count)
            .unwrap_or_default()
            .lines()
            .count()
    }
}

/// Runs `command` as the leader of a new session whose controlling terminal
/// is a fresh PTY, types Ctrl+C there once `service` is ready, and returns how
/// the command exited.
fn ctrl_c_on_a_terminal(mut command: Command, service: &CountingService) -> Option<i32> {
    let (mut master, slave) = open_pty();
    command
        .stdin(slave.try_clone().unwrap())
        .stdout(slave.try_clone().unwrap())
        .stderr(slave);
    // SAFETY: only setsid and ioctl run between fork and exec; both are
    // async-signal-safe and touch no memory of the parent.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().unwrap();
    // What the terminal shows is read and dropped, so no writer ever blocks.
    let mut output = master.try_clone().unwrap();
    thread::spawn(move || io::copy(&mut output, &mut io::sink()));
    service.wait_until_ready();

    io::Write::write_all(&mut master, b"\x03").unwrap();

    child.wait().unwrap().code()
}

#[test]
fn a_ctrl_c_typed_in_a_shell_s_terminal_reaches_the_service_once() {
    let sandbox = Sandbox::new();
    let service = CountingService::new(&sandbox);
    // A shell leads the terminal's session and runs `localify dev` in its
    // foreground group, as a person's shell would. It traps Ctrl+C rather than
    // ignore it: an ignored signal stays ignored in what it runs.
    let mut shell = sandbox.in_sandbox("/bin/sh");
    shell.arg("-c").arg(format!(
        "trap : INT; '{}' --config '{}' dev web --project app -- sh -c '{}'",
        env!("CARGO_BIN_EXE_localify"),
        sandbox.config.display(),
        service.script()
    ));

    assert_eq!(ctrl_c_on_a_terminal(shell, &service), Some(130));
    assert_eq!(service.interrupts(), 1);
}

#[test]
fn a_ctrl_c_typed_in_the_terminal_localify_leads_reaches_the_service_once() {
    let sandbox = Sandbox::new();
    let service = CountingService::new(&sandbox);
    // `localify dev` itself leads the session, with the PTY as its terminal.
    let dev = sandbox.localify(&[
        "dev",
        "web",
        "--project",
        "app",
        "--",
        "sh",
        "-c",
        &service.script(),
    ]);

    assert_eq!(ctrl_c_on_a_terminal(dev, &service), Some(130));
    assert_eq!(service.interrupts(), 1);
}

#[test]
fn a_ctrl_c_sent_to_a_detached_run_is_forwarded_to_its_service() {
    let sandbox = Sandbox::new();
    let service = CountingService::new(&sandbox);
    let mut executor = sandbox.spawn_background(&service.script());
    service.wait_until_ready();

    assert!(process::send_signal(executor.id(), Signal::Interrupt));

    executor.wait().unwrap();
    assert_eq!(service.interrupts(), 1);
}
