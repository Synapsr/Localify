//! The terminal panel's sessions, through the `TerminalManager` the desktop
//! app uses, with its events recorded instead of sent to a window
//! (docs/remote-machines.md, PR 1): tabs that end at once, a launch closed
//! while it still prepares, and two launches of one service.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use localify::branding::BRANDING;
use localify::desktop::DesktopApi;
use localify::desktop::run_pty::AppInstallation;
use localify::desktop::shell_environment::ShellEnvironmentDto;
use localify::desktop::terminal::{
    TerminalEvents, TerminalExitPayload, TerminalLaunchServiceRequest, TerminalManager,
    TerminalOutputPayload, TerminalSessionDto, TerminalSessionRequest, TerminalSessionStatus,
    TerminalTargetKind, TerminalTargetRequest,
};
use localify::infrastructure::process;
use localify::infrastructure::running_services::RunningServiceStore;

/// What the manager told the window.
#[derive(Default)]
struct Recorded {
    sessions: Mutex<Vec<TerminalSessionDto>>,
    output: Mutex<Vec<u8>>,
}

impl TerminalEvents for Recorded {
    fn output(&self, payload: TerminalOutputPayload) {
        self.output.lock().unwrap().extend(payload.bytes);
    }

    fn exit(&self, _payload: TerminalExitPayload) {}

    fn session(&self, session: TerminalSessionDto) {
        self.sessions.lock().unwrap().push(session);
    }

    fn shell_environment(&self, _status: ShellEnvironmentDto) {}
}

struct Sandbox {
    _dir: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    config: PathBuf,
    app: PathBuf,
    /// On the run's PATH, through the test's shell.
    bin: PathBuf,
}

impl Sandbox {
    fn new(script: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let (home, app, bin) = (root.join("home"), root.join("app"), root.join("bin"));
        for folder in [&home, &app, &bin] {
            fs::create_dir_all(folder).unwrap();
        }
        let config = root.join("config.toml");
        fs::write(&config, config_toml(&root, &app, script)).unwrap();
        Self {
            _dir: dir,
            root,
            home,
            config,
            app,
            bin,
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

    /// A shell that puts the sandbox's `bin` first on PATH, then runs what
    /// the resolver asks it to, as a person's shell init would.
    fn shell(&self) -> PathBuf {
        let shell = self.root.join("zsh");
        executable(
            &shell,
            &format!(
                "#!/bin/sh\nshift\nexport PATH=\"{}:/usr/bin:/bin\"\nexec /bin/sh -c \"$1\"\n",
                self.bin.display()
            ),
        );
        shell
    }

    /// The app's terminal panel, running `executable` as its `localify`.
    fn manager(&self, executable: &Path) -> (TerminalManager, Arc<Recorded>, DesktopApi) {
        let events = Arc::new(Recorded::default());
        let manager = TerminalManager::new(
            executable.to_path_buf(),
            self.shell(),
            self.store(),
            AppInstallation {
                home: Some(self.home.clone().into()),
                xdg_state_home: Some(self.root.join("state").into()),
                profile: None,
            },
            Arc::clone(&events) as Arc<dyn TerminalEvents>,
        );
        let api = DesktopApi::new(self.config.clone(), executable.to_path_buf());
        (manager, events, api)
    }

    fn launch(&self, api: &DesktopApi, profile: Option<&str>) -> TerminalLaunchServiceRequest {
        let environment = api
            .bootstrap()
            .unwrap()
            .project_environments
            .into_iter()
            .find(|environment| environment.project_id == "app" && environment.primary)
            .expect("the project has no primary checkout");
        TerminalLaunchServiceRequest {
            target: TerminalTargetRequest {
                kind: TerminalTargetKind::Service,
                project_id: "app".into(),
                environment_slug: environment.slug,
                service_id: Some("web".into()),
            },
            cols: 100,
            rows: 30,
            profile: profile.map(str::to_string),
        }
    }
}

fn config_toml(root: &Path, app: &Path, script: &str) -> String {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
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

/// The tab as the manager lists it, once it has exited.
fn wait_exited(manager: &TerminalManager, id: &str) -> TerminalSessionDto {
    wait_until(Duration::from_secs(30), || {
        manager
            .list_sessions()
            .into_iter()
            .find(|session| session.id == id && session.status == TerminalSessionStatus::Exited)
    })
    .unwrap_or_else(|| {
        panic!(
            "tab {id} never showed its exit: {:?}",
            manager.list_sessions()
        )
    })
}

#[test]
fn a_launch_refused_at_once_shows_its_end_in_its_tab() {
    let sandbox = Sandbox::new("exit 0");
    // A `localify dev` that refuses before registering anything.
    let refusing = sandbox.root.join("refusing-localify");
    executable(
        &refusing,
        "#!/bin/sh\necho \"'web' is already running\"\nexit 1\n",
    );
    let (manager, events, api) = sandbox.manager(&refusing);

    let launched = manager
        .launch_service(&api, sandbox.launch(&api, None))
        .unwrap();

    let tab = wait_exited(&manager, &launched.session.id);
    let end = tab.run.unwrap().ended.expect("the tab has no end");
    assert!(!end.registered);
    assert_eq!(end.code, Some(1));
    let told = events.sessions.lock().unwrap().clone();
    assert!(told.iter().any(|session| session.id == launched.session.id
        && session.status == TerminalSessionStatus::Exited));
}

#[test]
fn a_service_that_exits_at_once_shows_its_exit_in_its_tab() {
    let sandbox = Sandbox::new("exit 3");
    let (manager, events, api) = sandbox.manager(Path::new(env!("CARGO_BIN_EXE_localify")));

    let launched = manager
        .launch_service(&api, sandbox.launch(&api, None))
        .unwrap();

    let tab = wait_exited(&manager, &launched.session.id);
    let end = tab.run.unwrap().ended.expect("the tab has no end");
    assert!(end.registered);
    assert_eq!(end.code, Some(3));
    let told = events.sessions.lock().unwrap().clone();
    assert!(told.iter().any(|session| session.id == launched.session.id
        && session.status == TerminalSessionStatus::Exited));
}

#[test]
fn closing_a_tab_while_its_profile_resolves_starts_no_service() {
    let sandbox = Sandbox::new("touch ../service-started; sleep 300");
    fs::write(
        sandbox.app.join("localify.toml"),
        "[profiles.slow]\nsources = [{ infisical = {} }]\n",
    )
    .unwrap();
    fs::write(sandbox.app.join(".infisical.yaml"), "workspaceId: \"w\"\n").unwrap();
    // An Infisical CLI that never answers, as one waiting for a login does,
    // and ignores SIGTERM and SIGHUP.
    let marker = sandbox.root.join("infisical.pids");
    executable(
        &sandbox.bin.join("infisical"),
        &format!(
            "#!/bin/sh\ntrap '' TERM HUP\necho $$ $PPID > \"{}\"\nexec sleep 300\n",
            marker.display()
        ),
    );
    let (manager, _events, api) = sandbox.manager(Path::new(env!("CARGO_BIN_EXE_localify")));

    let launched = manager
        .launch_service(&api, sandbox.launch(&api, Some("slow")))
        .unwrap();
    let (infisical, executor): (u32, u32) = wait_until(Duration::from_secs(30), || {
        let content = fs::read_to_string(&marker).ok()?;
        let mut pids = content.split_whitespace().map(|pid| pid.parse().ok());
        Some((pids.next()??, pids.next()??))
    })
    .expect("the profile never reached its Infisical source");
    assert!(
        sandbox.store().list().unwrap().is_empty(),
        "registered before its sources"
    );

    manager
        .close(TerminalSessionRequest {
            session_id: launched.session.id.clone(),
        })
        .unwrap();

    // Closed only once the launch and everything it started are gone.
    assert!(manager.list_sessions().is_empty());
    let run_id = launched.session.run.as_ref().map(|run| run.run_id.clone());
    assert!(
        process::process_info(infisical).is_none(),
        "the profile's CLI outlived the launch"
    );
    assert!(
        process::process_info(executor).is_none(),
        "localify dev outlived the launch"
    );
    assert!(
        !sandbox.root.join("service-started").exists(),
        "the service started"
    );
    assert!(
        sandbox.store().list().unwrap().is_empty(),
        "{run_id:?} registered"
    );
}

#[test]
fn launching_a_running_service_again_returns_its_tab() {
    let sandbox = Sandbox::new("echo up; sleep 300");
    let (manager, _events, api) = sandbox.manager(Path::new(env!("CARGO_BIN_EXE_localify")));

    let first = manager
        .launch_service(&api, sandbox.launch(&api, None))
        .unwrap();
    let second = manager
        .launch_service(&api, sandbox.launch(&api, None))
        .unwrap();

    assert_eq!(first.session.id, second.session.id);
    manager
        .stop_service(TerminalSessionRequest {
            session_id: first.session.id.clone(),
        })
        .unwrap();
    let tab = wait_exited(&manager, &first.session.id);
    assert!(tab.run.unwrap().ended.unwrap().stopped);
}
