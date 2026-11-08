//! Gates of docs/remote-machines.md, PR 1, on the app's side: the run PTY.
//! Capture isolation, the shell environment, the end of a run in a run PTY,
//! interrupted capture when the app dies, Ctrl-Z, stop through the session,
//! and migration from shell-tab runs. Each test gets its own home, config and
//! state, and runs the real `localify` binary in a real PTY, as the desktop
//! app does through the same library code.
//!
//! Where the app itself has to be a separate process (started with a minimal
//! environment, or killed), this test binary runs again as that app: see
//! `app_helper`.

use std::ffi::OsString;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use localify::branding::BRANDING;
use localify::desktop::dto::ServiceRunDto;
use localify::desktop::pty::{self, WindowSize, suspend_disabled};
use localify::desktop::run_pty::{
    AppInstallation, RunPty, RunPtyEnd, RunPtyLaunch, run_environment,
};
use localify::desktop::shell_environment::ShellEnvironment;
use localify::infrastructure::process::{self, Signal};
use localify::infrastructure::running_services::{
    OutputAvailability, RunState, RunningService, RunningServiceStore,
};
use localify::infrastructure::runs::finish::Finish;
use localify::infrastructure::runs::output::{self, EndReason};
use localify::infrastructure::shell_env;
use localify::infrastructure::test_pause::PAUSE_DIR_ENV;

const HELPER_ENV: &str = "LOCALIFY_RUN_PTY_HELPER";

/// Children that ignore SIGINT, SIGTERM and the hangup of their terminal, and
/// never listen on their port.
const STUBBORN: &str = r#"echo started
sh -c 'trap "" INT TERM HUP; touch ../trapped-1; exec sleep 300' &
sh -c 'trap "" INT TERM HUP; touch ../trapped-2; exec sleep 300' &
echo ready
wait"#;

struct Sandbox {
    _dir: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    config: PathBuf,
    app: PathBuf,
}

impl Sandbox {
    /// A project `app` whose service `web` runs `script` with `sh -c`.
    fn new(script: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let home = root.join("home");
        let app = root.join("app");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&app).unwrap();
        let config = root.join("config.toml");
        fs::write(&config, config_toml(&root, &app, script)).unwrap();
        Self {
            _dir: dir,
            root,
            home,
            config,
            app,
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

    fn record(&self) -> Option<RunningService> {
        self.store().list().unwrap().into_iter().next()
    }

    /// What the sandbox's home and state are for every process it starts.
    fn sandbox_env(&self) -> [(OsString, OsString); 2] {
        [
            ("HOME".into(), self.home.clone().into()),
            ("XDG_STATE_HOME".into(), self.root.join("state").into()),
        ]
    }

    /// The app's launch of `web`, as `DesktopApi::service_run_command` gives it.
    fn launch(&self, environment: Vec<(OsString, OsString)>) -> RunPtyLaunch {
        RunPtyLaunch {
            executable: env!("CARGO_BIN_EXE_localify").into(),
            args: [
                "--config",
                self.config.to_str().unwrap(),
                "dev",
                "--project",
                "app",
                "web",
            ]
            .map(OsString::from)
            .to_vec(),
            working_directory: self.app.clone(),
            environment,
            size: WindowSize::new(100, 30).unwrap(),
        }
    }

    /// The app's installation, as the sandbox's.
    fn installation(&self) -> AppInstallation {
        AppInstallation {
            home: Some(self.home.clone().into()),
            xdg_state_home: Some(self.root.join("state").into()),
            profile: None,
        }
    }

    /// The test process's own environment, as a run gets it, in the sandbox.
    fn run_env(&self) -> Vec<(OsString, OsString)> {
        let mut environment = run_environment(std::env::vars_os(), &self.app, &self.installation());
        // macOS keeps state under HOME, Linux under XDG_STATE_HOME: both the sandbox's.
        environment.retain(|(key, _)| key != "XDG_STATE_HOME");
        environment.push(("XDG_STATE_HOME".into(), self.root.join("state").into()));
        environment
    }

    /// A service tab of `web`.
    fn open_tab(&self) -> Tab {
        Tab::spawn(self, self.run_env())
    }

    /// `localify --config <sandbox> <args>`, as a person's terminal runs it.
    fn localify(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_localify"));
        command
            .arg("--config")
            .arg(&self.config)
            .args(args)
            .current_dir(&self.app)
            .envs(self.sandbox_env())
            .env_remove("LOCALIFY_PROFILE")
            .env_remove("LOCALIFY_SERVICE_ORIGIN")
            .stdin(Stdio::null());
        command
    }

    /// A launch from a terminal with a command that exits at once.
    fn launch_from_terminal(&self) -> std::process::Output {
        self.localify(&["dev", "web", "--project", "app", "--", "sh", "-c", "exit 0"])
            .output()
            .unwrap()
    }

    /// Waits until the record's heartbeat is later than every member's start,
    /// so a finisher can prove they belong to the run, and returns the record.
    fn wait_for_proof(&self, executor: u32) -> RunningService {
        wait_until(Duration::from_secs(30), || {
            let record = self.record()?;
            let alive = record.run.as_ref()?.last_alive_at;
            let members: Vec<_> = process::all_processes()
                .into_iter()
                .filter(|process| process.sid == Some(executor))
                .collect();
            (record.pid == executor
                && members.len() >= 4
                && members.iter().all(|process| process.start <= alive))
            .then_some(record)
        })
        .unwrap_or_else(|| {
            panic!(
                "no heartbeat after the members started: {:?}",
                self.record()
            )
        })
    }

    /// Waits until both of `STUBBORN`'s children have set their traps.
    fn wait_until_trapped(&self) {
        let trapped = wait_until(Duration::from_secs(30), || {
            (self.root.join("trapped-1").exists() && self.root.join("trapped-2").exists())
                .then_some(())
        });
        assert!(
            trapped.is_some(),
            "the service's children never set their traps"
        );
    }

    /// Every byte in every run's stream.
    fn all_streams(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        let Ok(runs) = fs::read_dir(self.services_dir().join("output")) else {
            return bytes;
        };
        for run in runs.flatten() {
            for segment in fs::read_dir(run.path()).unwrap().flatten() {
                if segment.path().extension().is_some_and(|ext| ext == "log") {
                    bytes.extend(fs::read(segment.path()).unwrap());
                }
            }
        }
        bytes
    }
}

fn config_toml(root: &Path, app: &Path, script: &str) -> String {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let script = script
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n");
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

/// A service tab: its run PTY, what it showed, and how it ended.
struct Tab {
    pty: RunPty,
    shown: Arc<Mutex<Vec<u8>>>,
    /// While true, the app's reader holds back after the chunk it has.
    held: Arc<(Mutex<bool>, Condvar)>,
    /// How the run ended, and whether its stream was finalized by then.
    ended: Receiver<(RunPtyEnd, Option<EndReason>)>,
    dir: PathBuf,
}

impl Tab {
    fn spawn(sandbox: &Sandbox, environment: Vec<(OsString, OsString)>) -> Self {
        let shown = Arc::new(Mutex::new(Vec::new()));
        let held = Arc::new((Mutex::new(false), Condvar::new()));
        let (sender, ended) = mpsc::channel();
        let dir_cell: Arc<OnceLock<PathBuf>> = Arc::new(OnceLock::new());
        let (output_shown, output_held) = (Arc::clone(&shown), Arc::clone(&held));
        let end_dir = Arc::clone(&dir_cell);
        let pty = RunPty::spawn(
            Arc::new(sandbox.store()),
            sandbox.launch(environment),
            move |bytes| {
                output_shown.lock().unwrap().extend_from_slice(bytes);
                let (lock, released) = &*output_held;
                let mut holding = lock.lock().unwrap();
                while *holding {
                    holding = released.wait(holding).unwrap();
                }
            },
            move |end| {
                let dir = wait_until(Duration::from_secs(5), || end_dir.get().cloned()).unwrap();
                let finalized = output::read_end(&dir).ok().flatten().map(|end| end.reason);
                let _ = sender.send((end, finalized));
            },
        )
        .unwrap();
        let dir = sandbox.store().run_dir(pty.run_id());
        dir_cell.set(dir.clone()).unwrap();
        Self {
            pty,
            shown,
            held,
            ended,
            dir,
        }
    }

    fn shown(&self) -> String {
        String::from_utf8_lossy(&self.shown.lock().unwrap()).into_owned()
    }

    fn wait_for(&self, text: &str) {
        let found = wait_until(Duration::from_secs(30), || {
            self.shown().contains(text).then_some(())
        });
        let executor = self.pty.executor_pid();
        assert!(
            found.is_some(),
            "never showed {text:?} (executor {executor} alive: {}, ended: {}):\n{}",
            process::process_exists(executor),
            self.pty.has_ended(),
            self.shown()
        );
    }

    fn hold(&self, holding: bool) {
        let (lock, released) = &*self.held;
        *lock.lock().unwrap() = holding;
        released.notify_all();
    }

    fn type_keys(&self, keys: &str) {
        self.pty.write(keys.as_bytes()).unwrap();
    }

    fn wait_end(&self, timeout: Duration) -> (RunPtyEnd, Option<EndReason>) {
        self.ended
            .recv_timeout(timeout)
            .unwrap_or_else(|_| panic!("the run never ended:\n{}", self.shown()))
    }

    /// The run's stream, raw.
    fn stream(&self) -> Vec<u8> {
        fs::read(self.dir.join("0.log")).unwrap_or_default()
    }

    fn session_members(&self) -> Vec<u32> {
        let sid = self.pty.executor_pid();
        process::all_processes()
            .into_iter()
            .filter(|process| process.sid == Some(sid))
            .map(|process| process.pid)
            .collect()
    }
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

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

fn send(pid: u32, signal: libc::c_int) {
    // SAFETY: kill only sends a signal to a process this test started.
    assert_eq!(unsafe { libc::kill(pid as libc::pid_t, signal) }, 0);
}

fn read_pid(path: &Path) -> u32 {
    wait_until(Duration::from_secs(30), || {
        fs::read_to_string(path).ok()?.trim().parse().ok()
    })
    .unwrap_or_else(|| panic!("{} never held a PID", path.display()))
}

/// An ordinary shell tab, in its own PTY.
struct ShellTab {
    master: pty::PtyMaster,
    child: Child,
    shown: Arc<Mutex<Vec<u8>>>,
}

impl ShellTab {
    fn open(sandbox: &Sandbox) -> Self {
        let mut command = Command::new("/bin/sh");
        command
            .current_dir(&sandbox.app)
            .envs(sandbox.sandbox_env());
        let (master, child) = pty::spawn(command, WindowSize::new(80, 24).unwrap(), false).unwrap();
        let shown = Arc::new(Mutex::new(Vec::new()));
        let mut reader = master.try_clone().unwrap();
        let reading = Arc::clone(&shown);
        thread::spawn(move || {
            let mut buffer = [0_u8; 4096];
            while let Ok(count) = reader.read(&mut buffer) {
                if count == 0 {
                    return;
                }
                reading.lock().unwrap().extend_from_slice(&buffer[..count]);
            }
        });
        Self {
            master,
            child,
            shown,
        }
    }

    /// Types a command and waits for what it prints.
    fn run(&self, command: &str, prints: &str) {
        self.master
            .try_clone()
            .unwrap()
            .write_all(format!("{command}\n").as_bytes())
            .unwrap();
        let printed = wait_until(Duration::from_secs(10), || {
            let shown = String::from_utf8_lossy(&self.shown.lock().unwrap()).into_owned();
            // The command's output, not only the terminal's echo of what was typed.
            (shown.matches(prints).count() > command.matches(prints).count()).then_some(())
        });
        assert!(printed.is_some(), "the shell never printed {prints}");
    }
}

impl Drop for ShellTab {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// --- Capture isolation ----------------------------------------------------

#[test]
fn a_suspended_then_resumed_service_with_a_delayed_reader_exports_no_shell_bytes() {
    let sandbox = Sandbox::new(
        "echo $$ > ../service.pid; echo phase-one; \
         while [ ! -f ../go ]; do sleep 0.05; done; echo phase-two; exit 3",
    );
    let tab = sandbox.open_tab();
    tab.wait_for("phase-one");
    // The app's reader stops after what it has: the rest stays in the PTY.
    tab.hold(true);
    let service = read_pid(&sandbox.root.join("service.pid"));

    // A suspended service. SIGSTOP, since the run's group has no shell to
    // resume it and SIGTSTP is discarded in such an orphaned group on Linux.
    send(service, libc::SIGSTOP);
    let shell = ShellTab::open(&sandbox);
    shell.run("echo shell-$((40+2))", "shell-42");
    send(service, libc::SIGCONT);
    fs::write(sandbox.root.join("go"), "").unwrap();
    thread::sleep(Duration::from_millis(300));
    tab.hold(false);

    let (end, finalized) = tab.wait_end(Duration::from_secs(30));
    assert_eq!(end.code, Some(3));
    assert!(!end.stopped());
    assert_eq!(finalized, Some(EndReason::Complete));
    let stream = tab.stream();
    assert_eq!(
        stream,
        *tab.shown.lock().unwrap(),
        "the stream is every byte shown"
    );
    assert!(contains(&stream, "phase-one") && contains(&stream, "phase-two"));
    assert!(!contains(&sandbox.all_streams(), "shell-"));
}

#[test]
fn a_shell_tab_opened_after_the_run_is_never_exported() {
    let sandbox = Sandbox::new("echo service-line-one; echo service-line-two");
    let tab = sandbox.open_tab();
    let (_, finalized) = tab.wait_end(Duration::from_secs(30));
    assert_eq!(finalized, Some(EndReason::Complete));

    // `Open a terminal here`: an ordinary shell tab in the service's folder.
    let shell = ShellTab::open(&sandbox);
    shell.run("echo secret-after-exit", "secret-after-exit");

    let streams = sandbox.all_streams();
    assert!(contains(&streams, "service-line-two"));
    assert!(!contains(&streams, "secret-after-exit"));
    let logs = sandbox
        .localify(&["logs", "web", "--project", "app"])
        .output()
        .unwrap();
    let logs = String::from_utf8_lossy(&logs.stdout).into_owned();
    assert!(logs.contains("service-line-one"), "{logs}");
    assert!(!logs.contains("secret-after-exit"), "{logs}");
}

#[test]
fn keystrokes_typed_into_a_running_service_tab_appear_in_its_output() {
    let sandbox = Sandbox::new("echo ready; read line; echo \"got:$line\"");
    let tab = sandbox.open_tab();
    tab.wait_for("ready");

    tab.type_keys("typed-keys\r");

    tab.wait_end(Duration::from_secs(30));
    let stream = tab.stream();
    assert!(
        contains(&stream, "typed-keys"),
        "the terminal echoes what is typed"
    );
    assert!(contains(&stream, "got:typed-keys"));
}

// --- Ctrl-Z and stopping --------------------------------------------------

#[test]
fn ctrl_z_typed_into_a_service_tab_suspends_nothing() {
    let sandbox = Sandbox::new(
        "echo $$ > ../service.pid; i=0; while [ $i -lt 600 ]; do echo tick-$i; i=$((i+1)); sleep 0.05; done",
    );
    let tab = sandbox.open_tab();
    assert!(suspend_disabled(&tab.pty.termios().unwrap()));
    tab.wait_for("tick-3");
    let service = read_pid(&sandbox.root.join("service.pid"));

    tab.type_keys("\x1a");
    thread::sleep(Duration::from_millis(500));
    let ticks_after = tab.shown().matches("tick-").count();
    thread::sleep(Duration::from_millis(500));

    assert!(!is_stopped(service), "Ctrl-Z stopped the service");
    assert!(
        tab.shown().matches("tick-").count() > ticks_after,
        "the service stopped printing"
    );
    let record = sandbox.record().unwrap();
    assert!(sandbox.store().stop(&record).unwrap());
    tab.wait_end(Duration::from_secs(30));
}

fn is_stopped(pid: u32) -> bool {
    let state = Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    String::from_utf8_lossy(&state.stdout)
        .trim_start()
        .starts_with('T')
}

#[test]
fn stop_ends_the_whole_run_through_its_session_and_types_nothing() {
    let sandbox = Sandbox::new(
        "trap 'echo int >> ../interrupts' INT; \
         perl -e 'setpgrp(0, 0); $SIG{INT} = q(IGNORE); sleep 300' & \
         echo ready; wait",
    );
    let tab = sandbox.open_tab();
    tab.wait_for("ready");
    let record = wait_until(Duration::from_secs(30), || {
        sandbox.record().filter(|record| record.run_id().is_some())
    })
    .unwrap();
    wait_until(Duration::from_secs(10), || {
        (tab.session_members().len() >= 3).then_some(())
    })
    .expect("the service's child never started");

    assert!(sandbox.store().stop(&record).unwrap());

    let (end, finalized) = tab.wait_end(Duration::from_secs(30));
    assert!(end.stopped(), "{end:?}");
    assert_eq!(finalized, Some(EndReason::Complete));
    assert!(
        tab.session_members().is_empty(),
        "{:?}",
        tab.session_members()
    );
    assert!(
        !sandbox.root.join("interrupts").exists(),
        "a Ctrl-C reached the service"
    );
    assert!(!contains(&tab.stream(), "^C"));
}

// --- The end of a run in a run PTY ----------------------------------------

#[test]
fn ctrl_c_typed_into_the_tab_ends_every_process_before_the_tab_shows_exited() {
    let sandbox = Sandbox::new(STUBBORN);
    let tab = sandbox.open_tab();
    tab.wait_for("ready");
    sandbox.wait_until_trapped();

    tab.type_keys("\x03");
    let between = sandbox.launch_from_terminal();
    assert!(!between.status.success(), "a launch was admitted mid-stop");
    // A second Ctrl-C kills at once.
    tab.type_keys("\x03");

    let (end, finalized) = tab.wait_end(Duration::from_secs(60));
    assert_eq!(end.code, Some(130));
    assert_eq!(
        finalized,
        Some(EndReason::Complete),
        "Exited showed before finalization"
    );
    assert!(
        tab.session_members().is_empty(),
        "{:?}",
        tab.session_members()
    );
    let after = sandbox.launch_from_terminal();
    assert!(
        after.status.success(),
        "{}",
        String::from_utf8_lossy(&after.stderr)
    );
}

#[test]
fn a_killed_executor_s_run_is_ended_and_finalized_complete_by_the_app() {
    let sandbox = Sandbox::new(STUBBORN);
    let tab = sandbox.open_tab();
    tab.wait_for("ready");
    sandbox.wait_until_trapped();
    let executor = tab.pty.executor_pid();
    // Its children start before a heartbeat, so they are proven the run's.
    sandbox.wait_for_proof(executor);

    assert!(process::send_signal(executor, Signal::Kill));
    // A launch meanwhile waits for the run to end, the app's reading of the
    // PTY included, then is admitted.
    let between = sandbox.launch_from_terminal();
    assert!(
        between.status.success(),
        "{}",
        String::from_utf8_lossy(&between.stderr)
    );

    let (end, finalized) = tab.wait_end(Duration::from_secs(60));
    assert_eq!(end.signal, Some(libc::SIGKILL));
    assert!(end.error.is_none(), "{:?}", end.error);
    assert_eq!(finalized, Some(EndReason::Complete));
    assert!(
        tab.session_members().is_empty(),
        "{:?}",
        tab.session_members()
    );
    let after = sandbox.launch_from_terminal();
    assert!(
        after.status.success(),
        "{}",
        String::from_utf8_lossy(&after.stderr)
    );
}

#[test]
fn a_tab_whose_launch_is_refused_registers_nothing_and_leaves_no_folder() {
    let sandbox = Sandbox::new("printf 'first-%s\\n' up; sleep 300");
    let first = sandbox.open_tab();
    first.wait_for("first-up");

    // In a terminal, `localify dev` asks before replacing a running service.
    let second = sandbox.open_tab();
    second.wait_for("Stop it and start again?");
    second.type_keys("n\r");
    let (end, _) = second.wait_end(Duration::from_secs(30));

    assert!(!end.registered);
    assert!(
        second.shown().contains("already running"),
        "{}",
        second.shown()
    );
    assert!(!second.dir.exists());
    assert!(second.pty.write(b"x").is_err(), "an ended tab took input");
    let record = sandbox.record().unwrap();
    assert!(sandbox.store().stop(&record).unwrap());
    first.wait_end(Duration::from_secs(30));
}

// --- Interrupted capture: the app dies ------------------------------------

#[test]
fn killing_the_app_finalizes_its_run_interrupted_at_what_was_written() {
    let sandbox = Sandbox::new(STUBBORN);
    let mut app = sandbox.app_helper("hold", &[]);
    let line = app.line("run ");
    let run_id = line.trim();
    let dir = sandbox
        .store()
        .run_dir(&localify::infrastructure::runs::RunId::parse(run_id).unwrap());
    assert!(output::reader_alive(&dir).unwrap());
    sandbox.wait_until_trapped();
    let executor = sandbox.record().expect("the run never registered").pid;
    // Its children start before a heartbeat, so the next finisher can prove
    // they are the run's once the app and the executor are gone.
    let record = sandbox.wait_for_proof(executor);
    assert_eq!(record.run_id().map(|id| id.as_str()), Some(run_id));

    app.kill();
    let written = fs::metadata(dir.join("0.log")).unwrap().len();
    assert!(!output::reader_alive(&dir).unwrap());

    // Nobody calls stop: `localify dev` sees its terminal go, ends the run
    // and finalizes what the app wrote.
    let end = wait_until(Duration::from_secs(45), || {
        let gone = !process::process_exists(executor);
        output::read_end(&dir).unwrap().filter(|_| gone)
    })
    .expect("the run was never ended after the app died");
    assert_eq!(end.reason, EndReason::Interrupted);
    assert_eq!(end.offset, written);
    let left: Vec<_> = process::all_processes()
        .into_iter()
        .filter(|process| process.sid == Some(executor))
        .collect();
    assert!(left.is_empty(), "{left:?}");
    assert_eq!(record.run_id().map(|id| id.as_str()), Some(run_id));
    let after = sandbox.launch_from_terminal();
    assert!(
        after.status.success(),
        "{}",
        String::from_utf8_lossy(&after.stderr)
    );
}

#[test]
fn a_pty_allocated_while_another_child_spawns_leaks_no_descriptor() {
    let sandbox = Sandbox::new("printf 'service-%s\\n' up; exec sleep 300");
    let pause = sandbox.root.join("pause");
    fs::create_dir_all(&pause).unwrap();
    fs::write(pause.join("pty-allocated"), "").unwrap();

    let mut app = sandbox.app_helper("fd-race", &[(PAUSE_DIR_ENV, pause.to_str().unwrap())]);
    let resolver = app.line("resolver ");
    let terminal = app.line("terminal ");
    let run_id = app.line("run ");

    assert!(resolver.contains("checked"), "{resolver}");
    assert!(
        !resolver.contains("leaked"),
        "a spawn inherited the PTY: {resolver}"
    );
    assert!(terminal.contains("checked"), "{terminal}");
    assert!(
        !terminal.contains("leaked"),
        "a terminal inherited the PTY: {terminal}"
    );

    // And the app's death still ends its run.
    let dir = sandbox
        .store()
        .run_dir(&localify::infrastructure::runs::RunId::parse(run_id.trim()).unwrap());
    let executor = sandbox.record().expect("the run never registered").pid;
    app.kill();
    let end = wait_until(Duration::from_secs(45), || {
        let gone = process::process_info(executor).is_none();
        output::read_end(&dir).unwrap().filter(|_| gone)
    })
    .expect("the run was never ended after the app died");
    assert_eq!(end.reason, EndReason::Interrupted);
    assert!(
        session_alive(executor).is_empty(),
        "{:?}",
        session_alive(executor)
    );
}

#[test]
fn killing_the_app_while_the_profile_resolves_leaves_nothing_alive() {
    let sandbox = Sandbox::new("touch ../service-started; exec sleep 300");
    let marker = sandbox.hanging_infisical();
    let path = format!("{}:/usr/bin:/bin", sandbox.root.join("bin").display());
    let mut app = sandbox.app_helper("resolving", &[("HELPER_PROFILE", "slow"), ("PATH", &path)]);
    app.line("started ");
    let (helper, executor) = read_helper_pids(&marker);

    app.kill();

    let gone = wait_until(Duration::from_secs(45), || {
        (process::process_info(helper).is_none() && process::process_info(executor).is_none())
            .then_some(())
    });
    assert!(
        gone.is_some(),
        "the profile's CLI or localify dev outlived the app"
    );
    assert!(
        session_alive(executor).is_empty(),
        "{:?}",
        session_alive(executor)
    );
    assert!(
        !sandbox.root.join("service-started").exists(),
        "the service started"
    );
    assert!(
        sandbox.store().list().unwrap().is_empty(),
        "the run registered"
    );
}

/// The PIDs a hanging Infisical CLI wrote: its own, and its parent's,
/// `localify dev`.
fn read_helper_pids(marker: &Path) -> (u32, u32) {
    wait_until(Duration::from_secs(30), || {
        let content = fs::read_to_string(marker).ok()?;
        let mut pids = content.split_whitespace().map(|pid| pid.parse().ok());
        Some((pids.next()??, pids.next()??))
    })
    .expect("the profile never reached its Infisical source")
}

impl Sandbox {
    /// A `slow` profile whose Infisical source never answers, as a CLI
    /// waiting for a login does, and ignores SIGTERM and SIGHUP. Returns the
    /// file it writes its PID and its parent's to.
    fn hanging_infisical(&self) -> PathBuf {
        fs::write(
            self.app.join("localify.toml"),
            "[profiles.slow]\nsources = [{ infisical = {} }]\n",
        )
        .unwrap();
        fs::write(self.app.join(".infisical.yaml"), "workspaceId: \"w\"\n").unwrap();
        let bin = self.root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let marker = self.root.join("infisical.pids");
        let cli = bin.join("infisical");
        fs::write(
            &cli,
            format!(
                "#!/bin/sh\ntrap '' TERM HUP\necho $$ $PPID > \"{}\"\nexec sleep 300\n",
                marker.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&cli, fs::Permissions::from_mode(0o755)).unwrap();
        marker
    }
}

#[test]
fn the_daemon_finishes_a_run_pty_whose_app_and_executor_both_died() {
    let sandbox = Sandbox::new(STUBBORN);
    let mut app = sandbox.app_helper("hold", &[]);
    let run_id = app.line("run ");
    let dir = sandbox
        .store()
        .run_dir(&localify::infrastructure::runs::RunId::parse(run_id.trim()).unwrap());
    sandbox.wait_until_trapped();
    let executor = sandbox.record().expect("the run never registered").pid;
    sandbox.wait_for_proof(executor);

    // Both readers of the run gone at once: nothing of the app is left to
    // finish it, and `localify dev` had no time to.
    assert!(process::send_signal(executor, Signal::Kill));
    app.kill();
    assert!(!output::reader_alive(&dir).unwrap());
    // A SIGKILL takes effect asynchronously: the pass skips an executor still
    // exiting, as the daemon would until its next pass.
    wait_until(Duration::from_secs(10), || {
        process::process_info(executor).is_none().then_some(())
    })
    .expect("the executor outlived SIGKILL");
    assert!(
        !session_alive(executor).is_empty(),
        "the children died with their terminal"
    );

    let finished = sandbox.store().finish_unread_runs().unwrap();

    assert_eq!(finished.len(), 1, "{finished:?}");
    assert_eq!(finished[0].1, Finish::Finalized(EndReason::Interrupted));
    assert!(
        session_alive(executor).is_empty(),
        "{:?}",
        session_alive(executor)
    );
    let after = sandbox.launch_from_terminal();
    assert!(
        after.status.success(),
        "{}",
        String::from_utf8_lossy(&after.stderr)
    );
}

#[test]
fn a_run_pty_whose_app_still_reads_is_left_to_the_app() {
    // Printed as two words, so only the service's output, not its echoed command, reads so.
    let sandbox = Sandbox::new("printf 'service-%s\\n' up; sleep 300");
    let tab = sandbox.open_tab();
    tab.wait_for("service-up");

    let finished = sandbox.store().finish_unread_runs().unwrap();

    assert!(finished.is_empty(), "{finished:?}");
    let record = sandbox.record().unwrap();
    assert!(sandbox.store().stop(&record).unwrap());
    tab.wait_end(Duration::from_secs(30));
}

#[test]
fn a_reader_held_past_the_executor_s_end_finalizes_interrupted() {
    // The reader holds after the service's own first line: what is left fits in the PTY.
    let sandbox = Sandbox::new("printf 'first-%s\\n' line; sleep 0.5; printf 'second-%s\\n' line");
    let tab = sandbox.open_tab();
    tab.wait_for("first-line");
    tab.hold(true);
    let executor = tab.pty.executor_pid();
    wait_until(Duration::from_secs(30), || {
        (!process::process_exists(executor)).then_some(())
    })
    .expect("the service never ended");

    let (end, finalized) = tab.wait_end(Duration::from_secs(30));
    tab.hold(false);

    assert_eq!(finalized, Some(EndReason::Interrupted));
    assert_eq!(end.output, Some(EndReason::Interrupted));
    assert!(
        end.error
            .as_deref()
            .unwrap_or_default()
            .contains("still open"),
        "{end:?}"
    );
    // The reader wrote what it last read, then held: what it never read is the PTY's end.
    assert!(contains(&tab.stream(), "first-line"));
}

#[test]
fn concurrent_launches_from_the_app_and_a_terminal_leave_one_run() {
    if Command::new("python3").arg("--version").output().is_err() {
        let _ = writeln!(std::io::stderr(), "skipped: python3 is not installed");
        return;
    }
    // It listens, so a detached launch that wins returns at once.
    let sandbox =
        Sandbox::new(r#"echo ready; exec python3 -u -m http.server "$PORT" --bind 127.0.0.1"#);
    // The app's button pressed twice, and a terminal's `dev --detach`, at once.
    let tabs: Vec<Tab> = (0..2).map(|_| sandbox.open_tab()).collect();
    let detached = sandbox
        .localify(&["dev", "web", "--project", "app", "--detach"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();

    let record = wait_until(Duration::from_secs(30), || {
        sandbox.record().filter(|record| record.run_id().is_some())
    })
    .unwrap();
    let admitted = tabs
        .iter()
        .position(|tab| Some(tab.pty.run_id()) == record.run_id());
    for (index, tab) in tabs.iter().enumerate() {
        if Some(index) != admitted {
            // A tab that finds the service running asks before replacing it,
            // as any terminal does: it is told no.
            let mut answered = false;
            let end = wait_until(Duration::from_secs(30), || {
                if let Ok((end, _)) = tab.ended.try_recv() {
                    return Some(end);
                }
                if !answered && tab.shown().contains("Stop it and start again?") {
                    answered = tab.pty.write(b"n\r").is_ok();
                }
                None
            })
            .unwrap_or_else(|| panic!("a losing tab never ended:\n{}", tab.shown()));
            assert!(!end.registered, "a second tab ran the service");
        }
    }
    match admitted {
        Some(_) => assert!(!detached.success(), "the detached launch ran too"),
        None => assert!(detached.success(), "no launch ran"),
    }
    assert!(sandbox.store().stop(&record).unwrap());
}

/// Every living process of the session `sid` leads.
fn session_alive(sid: u32) -> Vec<u32> {
    process::all_processes()
        .into_iter()
        .filter(|process| process.sid == Some(sid))
        .map(|process| process.pid)
        .collect()
}

// --- The shell environment -------------------------------------------------

/// The app, started again as a separate process with a minimal environment,
/// as `env -i HOME=… PATH=/usr/bin:/bin` would.
struct AppProcess {
    child: Child,
    lines: Receiver<String>,
}

impl Sandbox {
    fn app_helper(&self, mode: &str, extra: &[(&str, &str)]) -> AppProcess {
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
            .envs(self.sandbox_env())
            .env(HELPER_ENV, mode)
            .env("HELPER_CONFIG", &self.config)
            .env("HELPER_APP", &self.app)
            .envs(extra.iter().copied())
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
        AppProcess { child, lines }
    }
}

impl AppProcess {
    /// The value of the helper's next line that starts with `key`.
    fn line(&mut self, key: &str) -> String {
        loop {
            let line = self
                .lines
                .recv_timeout(Duration::from_secs(60))
                .unwrap_or_else(|_| panic!("the app never said {key:?}"));
            if let Some(value) = line.strip_prefix(key) {
                return value.to_string();
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

/// Not a test: the app, for the tests above that need it in a process of its
/// own. `hold` starts `web` in a run PTY and waits to be killed; `resolve`
/// reads the shell environment; `service` reads it, then runs `web` with it.
#[test]
#[ignore = "run by the tests above as the app"]
fn app_helper() {
    let Ok(mode) = std::env::var(HELPER_ENV) else {
        return;
    };
    let config = std::env::var("HELPER_CONFIG").unwrap();
    let app = PathBuf::from(std::env::var("HELPER_APP").unwrap());
    let localify = PathBuf::from(
        std::env::var("HELPER_LOCALIFY").unwrap_or_else(|_| env!("CARGO_BIN_EXE_localify").into()),
    );
    match mode.as_str() {
        "resolve" => {
            let deadline = Duration::from_millis(
                std::env::var("HELPER_DEADLINE_MS")
                    .unwrap()
                    .parse()
                    .unwrap(),
            );
            let started = Instant::now();
            let resolved = shell_env::resolve_within(&localify, &shell_env::user_shell(), deadline);
            println!("HELPER elapsed {}", started.elapsed().as_millis());
            match resolved {
                Ok(environment) => {
                    let path = environment.iter().find(|(key, _)| key == "PATH").unwrap();
                    println!("HELPER ok {}", path.1.to_string_lossy());
                }
                Err(error) => println!("HELPER failed {error:#}"),
            }
        }
        "hold" | "service" => {
            let started = Instant::now();
            let environment = if mode == "service" {
                let shell = ShellEnvironment::new(localify.clone(), shell_env::user_shell());
                let environment = shell.environment();
                let status = shell.status();
                println!("HELPER elapsed {}", started.elapsed().as_millis());
                println!(
                    "HELPER shell {:?} {}",
                    status.state,
                    status.reason.unwrap_or_default()
                );
                environment
            } else {
                std::env::vars_os().collect()
            };
            let (run, shown, ended) = helper_run(&config, &app, environment);
            if mode == "hold" {
                wait_until(Duration::from_secs(30), || {
                    contains(&shown.lock().unwrap(), "ready").then_some(())
                })
                .unwrap();
                println!("HELPER run {}", run.run_id());
                thread::sleep(Duration::from_secs(600));
            }
            let end = ended.recv_timeout(Duration::from_secs(60)).unwrap();
            println!("HELPER run {}", run.run_id());
            println!("HELPER end {:?}", end.code);
        }
        "resolving" => {
            // Started, and killed by the test while its profile resolves.
            let (run, _, _) = helper_run(&config, &app, std::env::vars_os().collect());
            println!("HELPER started {}", run.run_id());
            thread::sleep(Duration::from_secs(600));
        }
        "fd-race" => fd_race(&config, &app),
        _ => panic!("unknown helper mode {mode}"),
    }
}

/// `web` in a run PTY of the helper app, on `HELPER_PROFILE` when set: the
/// run, what it showed, and its end.
fn helper_run(
    config: &str,
    app: &Path,
    environment: Vec<(OsString, OsString)>,
) -> (RunPty, Arc<Mutex<Vec<u8>>>, Receiver<RunPtyEnd>) {
    let mut args: Vec<OsString> = ["--config", config, "dev", "--project", "app", "web"]
        .map(OsString::from)
        .to_vec();
    if let Ok(profile) = std::env::var("HELPER_PROFILE") {
        args.extend(["--profile".into(), profile.into()]);
    }
    let (sender, ended) = mpsc::channel();
    let shown = Arc::new(Mutex::new(Vec::new()));
    let output_shown = Arc::clone(&shown);
    let run = RunPty::spawn(
        Arc::new(RunningServiceStore::new_default().unwrap()),
        RunPtyLaunch {
            executable: env!("CARGO_BIN_EXE_localify").into(),
            args,
            working_directory: app.to_path_buf(),
            environment: run_environment(environment, app, &AppInstallation::of_this_process()),
            size: WindowSize::new(100, 30).unwrap(),
        },
        move |bytes| output_shown.lock().unwrap().extend_from_slice(bytes),
        move |end| {
            let _ = sender.send(end);
        },
    )
    .unwrap();
    (run, shown, ended)
}

/// Opens a run PTY that stops, by the test's barrier, right after its
/// descriptors are allocated; meanwhile starts a child as the shell resolver
/// does and, once the PTY is done, another terminal. Says which descriptors
/// of theirs are terminals, then holds the run until killed.
fn fd_race(config: &str, app: &Path) {
    let pause = PathBuf::from(std::env::var("LOCALIFY_TEST_PAUSE_DIR").unwrap());
    let (config, app_dir) = (config.to_string(), app.to_path_buf());
    let opening =
        thread::spawn(move || helper_run(&config, &app_dir, std::env::vars_os().collect()));
    wait_until(Duration::from_secs(30), || {
        pause.join("pty-allocated.reached").exists().then_some(())
    })
    .expect("the PTY never reached its barrier");
    // A spawn while the first PTY is allocated, as the shell resolver spawns.
    let resolver = thread::spawn(|| {
        let _spawning = process::spawning();
        Command::new("/bin/sh")
            .args(["-c", TTY_CHECK])
            .stdin(Stdio::null())
            .output()
            .unwrap()
    });
    fs::remove_file(pause.join("pty-allocated")).unwrap();
    let resolver = resolver.join().unwrap();
    let (run, shown, _ended) = opening.join().unwrap();
    // Another terminal, opened next to the first.
    let mut command = Command::new("/bin/sh");
    command.args(["-c", &format!("{TTY_CHECK}; echo checked-done")]);
    let (master, mut child) = pty::spawn(command, WindowSize::new(80, 24).unwrap(), false).unwrap();
    let mut terminal = Vec::new();
    let mut reader = master.try_clone().unwrap();
    let mut buffer = [0_u8; 4096];
    while !contains(&terminal, "checked-done") {
        match reader.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(count) => terminal.extend_from_slice(&buffer[..count]),
        }
    }
    child.wait().unwrap();
    println!(
        "HELPER resolver {}",
        String::from_utf8_lossy(&resolver.stdout).replace('\n', " ")
    );
    println!(
        "HELPER terminal {}",
        String::from_utf8_lossy(&terminal).replace(['\r', '\n'], " ")
    );
    wait_until(Duration::from_secs(30), || {
        contains(&shown.lock().unwrap(), "service-up").then_some(())
    })
    .expect("the run never started");
    println!("HELPER run {}", run.run_id());
    thread::sleep(Duration::from_secs(600));
}

/// Prints `leaked-N` for each descriptor above standard error that is a
/// terminal: one inherited from another PTY.
const TTY_CHECK: &str = "n=3; while [ $n -lt 64 ]; do if [ -t $n ]; then echo leaked-$n; fi; n=$((n+1)); done; echo checked";

/// A shell's startup file, where the test user's shell init lives.
fn init_file(home: &Path, shell: &str) -> PathBuf {
    match shell {
        "zsh" => home.join(".zshrc"),
        "bash" => home.join(".bash_profile"),
        "fish" => home.join(".config/fish/config.fish"),
        "nu" if cfg!(target_os = "macos") => {
            home.join("Library/Application Support/nushell/config.nu")
        }
        "nu" => home.join(".config/nushell/config.nu"),
        _ => unreachable!("no init file for {shell}"),
    }
}

fn write_init(home: &Path, shell: &str, content: &str) {
    let path = init_file(home, shell);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn find_shell(name: &str) -> Option<PathBuf> {
    [
        "/bin",
        "/usr/bin",
        "/usr/local/bin",
        "/opt/homebrew/bin",
        "/home/linuxbrew/.linuxbrew/bin",
    ]
    .iter()
    .map(|dir| Path::new(dir).join(name))
    .find(|path| path.is_file())
}

#[test]
fn a_service_finds_a_tool_that_only_its_shell_init_puts_on_path() {
    for shell in ["zsh", "bash", "fish", "nu"] {
        let Some(path) = find_shell(shell) else {
            // Straight to stderr, past libtest's capture, so CI shows what it skipped.
            let _ = writeln!(
                std::io::stderr(),
                "skipped the {shell} case: {shell} is not installed on this machine"
            );
            continue;
        };
        let sandbox = Sandbox::new("only-from-shell-init");
        let tools = sandbox.home.join("tool-bin");
        fs::create_dir_all(&tools).unwrap();
        let tool = tools.join("only-from-shell-init");
        fs::write(&tool, "#!/bin/sh\necho tool-ran-for-the-service\n").unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
        let init = match shell {
            "fish" => "set -gx PATH $HOME/tool-bin $PATH\n".to_string(),
            "nu" => "$env.PATH = ($env.PATH | prepend $\"($env.HOME)/tool-bin\")\n".to_string(),
            _ => "export PATH=\"$HOME/tool-bin:$PATH\"\necho init-says-hello\n".to_string(),
        };
        write_init(&sandbox.home, shell, &init);

        let mut app = sandbox.app_helper("service", &[("SHELL", path.to_str().unwrap())]);
        let status = app.line("shell ");
        let run_id = app.line("run ");
        app.line("end ");

        assert!(status.starts_with("Ready"), "{shell}: {status}");
        let dir = sandbox
            .store()
            .run_dir(&localify::infrastructure::runs::RunId::parse(&run_id).unwrap());
        let stream = fs::read(dir.join("0.log")).unwrap();
        assert!(
            contains(&stream, "tool-ran-for-the-service"),
            "{shell}: {}",
            String::from_utf8_lossy(&stream)
        );
        assert!(
            !contains(&stream, "init-says-hello"),
            "{shell}: shell init reached the stream"
        );
    }
}

/// A deadline short enough for failing cases to keep the tests quick.
const TEST_DEADLINE: Duration = Duration::from_secs(3);

/// Runs the resolver in the app process with bash as the user's shell and
/// `init` as its startup file, within `deadline`. Returns what it said, how
/// long it took, and the PID the init wrote, if it got that far.
fn resolve_with_bash_init(
    init: &str,
    localify: Option<&Path>,
    deadline: Duration,
) -> (String, Duration, Option<u32>) {
    let bash = find_shell("bash").expect("bash is on every machine CI runs");
    let sandbox = Sandbox::new("exit 0");
    let pid_file = sandbox.home.join("init.pid");
    write_init(
        &sandbox.home,
        "bash",
        &format!("echo $$ > \"$HOME/init.pid\"\n{init}"),
    );
    let mut extra = vec![
        ("SHELL", bash.to_str().unwrap().to_string()),
        ("HELPER_DEADLINE_MS", deadline.as_millis().to_string()),
    ];
    if let Some(localify) = localify {
        extra.push(("HELPER_LOCALIFY", localify.display().to_string()));
    }
    let extra: Vec<(&str, &str)> = extra
        .iter()
        .map(|(key, value)| (*key, value.as_str()))
        .collect();
    let started = Instant::now();
    let mut app = sandbox.app_helper("resolve", &extra);
    let elapsed: u64 = app.line("elapsed ").parse().unwrap();
    let outcome = app
        .lines
        .recv_timeout(Duration::from_secs(10))
        .expect("the resolver said nothing");
    assert!(started.elapsed() < Duration::from_secs(30));
    let pid = fs::read_to_string(pid_file)
        .ok()
        .and_then(|pid| pid.trim().parse().ok());
    (outcome, Duration::from_millis(elapsed), pid)
}

fn assert_failed_and_gone(case: &str, (outcome, elapsed, pid): (String, Duration, Option<u32>)) {
    assert!(outcome.starts_with("failed "), "{case}: {outcome}");
    assert!(
        elapsed < TEST_DEADLINE + Duration::from_millis(500),
        "{case}: took {elapsed:?}"
    );
    if let Some(pid) = pid {
        assert!(
            !process::process_exists(pid),
            "{case}: the shell outlived the resolver"
        );
    }
}

#[test]
fn a_shell_init_that_hangs_waits_on_dev_tty_or_floods_fails_within_the_deadline() {
    assert_failed_and_gone(
        "hang",
        resolve_with_bash_init("sleep 60\n", None, TEST_DEADLINE),
    );
    assert_failed_and_gone(
        "/dev/tty",
        resolve_with_bash_init("read answer < /dev/tty || sleep 60\n", None, TEST_DEADLINE),
    );
    assert_failed_and_gone(
        "more than 1 MiB",
        resolve_with_bash_init(
            "head -c 2000000 /dev/zero | tr '\\0' x\n",
            None,
            TEST_DEADLINE,
        ),
    );
}

#[test]
fn a_forged_frame_or_text_inside_it_fails() {
    let scripts = tempfile::tempdir().unwrap();
    let cases = [
        (
            "forged",
            "printf '%s\\0PATH=/evil\\0%s' \"$2\" \"$2\"; printf '%s\\0PATH=/bin\\0%s' \"$2\" \"$2\"",
        ),
        (
            "inside",
            "printf '%s\\0PATH=/bin\\0injected text\\0%s' \"$2\" \"$2\"",
        ),
    ];
    for (case, body) in cases {
        let fake = scripts.path().join(format!("localify-{case}"));
        fs::write(&fake, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
        assert_failed_and_gone(case, resolve_with_bash_init("", Some(&fake), TEST_DEADLINE));
    }
    // The real command, framed once, is accepted, within the real deadline.
    let (outcome, _, _) = resolve_with_bash_init("", None, shell_env::DEADLINE);
    assert!(outcome.starts_with("ok "), "{outcome}");
}

#[test]
fn a_hanging_shell_init_leaves_runs_on_the_app_s_environment_within_10_seconds() {
    let Some(bash) = find_shell("bash") else {
        panic!("bash is on every machine CI runs");
    };
    let sandbox = Sandbox::new("echo \"service-ran PATH=$PATH\"");
    write_init(
        &sandbox.home,
        "bash",
        "echo init-before-hanging\nsleep 60\n",
    );

    let mut app = sandbox.app_helper("service", &[("SHELL", bash.to_str().unwrap())]);
    let elapsed: u64 = app.line("elapsed ").parse().unwrap();
    let status = app.line("shell ");
    let run_id = app.line("run ");

    assert!(elapsed <= 10_000, "took {elapsed} ms");
    assert!(status.starts_with("Failed"), "{status}");
    let dir = sandbox
        .store()
        .run_dir(&localify::infrastructure::runs::RunId::parse(&run_id).unwrap());
    let stream = fs::read(dir.join("0.log")).unwrap();
    assert!(
        contains(&stream, "service-ran PATH=/usr/bin:/bin"),
        "{}",
        String::from_utf8_lossy(&stream)
    );
    assert!(!contains(&sandbox.all_streams(), "init-before-hanging"));
}

// --- Migration --------------------------------------------------------------

#[test]
fn a_service_started_from_a_shell_tab_before_the_update_is_a_legacy_run_left_alone() {
    let sandbox = Sandbox::new("echo new-run");
    // The record's key, as `localify dev` names this service.
    assert!(sandbox.launch_from_terminal().status.success());
    let key = sandbox.record().unwrap().key;
    let record_path = fs::read_dir(sandbox.services_dir())
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|ext| ext == "json"))
        .unwrap();
    // What the previous app left: a service in a shell tab, its whole shell in a file.
    let mut legacy = Command::new("sleep").arg("300").spawn().unwrap();
    let started = process::process_info(legacy.id()).unwrap().started_wall;
    let whole_shell = sandbox.services_dir().join("logs/old-shell.log");
    fs::create_dir_all(whole_shell.parent().unwrap()).unwrap();
    fs::write(
        &whole_shell,
        "me@mac app % pnpm dev\nold-whole-shell-bytes\n",
    )
    .unwrap();
    let json = serde_json::json!({
        "projectId": key.project_id, "environment": key.environment, "serviceId": key.service_id,
        "pid": legacy.id(), "processStartedAt": process::lstart_label(started).unwrap(),
        "url": "http://web.localify", "port": 3000, "origin": "app", "profile": null,
        "startedAt": localify::infrastructure::launch_sessions::unix_now(),
        "logFile": whole_shell, "exit": null,
    });
    fs::write(&record_path, json.to_string()).unwrap();

    let store = sandbox.store();
    let record = sandbox.record().unwrap();
    assert_eq!(store.state(&record), RunState::Running);
    assert_eq!(record.output_availability(), OutputAvailability::LegacyRun);
    let shown = ServiceRunDto::new(&record, store.state(&record), None);
    assert!(shown.legacy);
    assert_eq!(shown.output_run_id, None);

    // The new app's launch is refused, and the legacy run keeps running.
    let tab = sandbox.open_tab();
    let (end, _) = tab.wait_end(Duration::from_secs(30));
    assert!(!end.registered);
    assert!(process::process_exists(legacy.id()));
    assert!(!contains(&sandbox.all_streams(), "old-whole-shell-bytes"));

    legacy.kill().unwrap();
    legacy.wait().unwrap();
}
