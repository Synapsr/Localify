//! The inventory end to end (docs/remote-machines.md, "Inventory"): a real
//! daemon builds it, `localify inventory` reads it over the management
//! socket, and a real `localify dev` runs the service. Everything lives in a
//! temporary home on free ports, so an installed Localify is never touched.
//! Every port is reserved by a listener held until the moment it is used, so
//! no two ports of the test collide, and the daemon is started again on fresh
//! ports if another program took one in between.

use std::fs;
use std::io::{self, Write};
use std::net::TcpListener;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use localify::branding::BRANDING;
use localify::infrastructure::test_pause::PAUSE_DIR_ENV;
use serde_json::Value;

/// Tries for a daemon on fresh ports.
const DAEMON_ATTEMPTS: usize = 3;

struct Sandbox {
    _dir: tempfile::TempDir,
    root: PathBuf,
    app: PathBuf,
    port: u16,
    /// Holds the service's port until the test frees it.
    service: Option<TcpListener>,
    ingress: TcpListener,
    daemon: Option<Child>,
}

impl Sandbox {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let app = root.join("app");
        fs::create_dir_all(&app).unwrap();
        let service = reserve();
        Self {
            _dir: dir,
            root,
            app,
            port: service.local_addr().unwrap().port(),
            service: Some(service),
            ingress: reserve(),
            daemon: None,
        }
    }

    fn localify(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_localify"));
        command
            .arg("--config")
            .arg(self.root.join("config.toml"))
            .args(args)
            .current_dir(&self.app)
            .env("HOME", &self.root)
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env(
                "LOCALIFY_PUBLIC_INGRESS_PORT",
                self.ingress.local_addr().unwrap().port().to_string(),
            )
            .env_remove("LOCALIFY_PROFILE")
            .env_remove("LOCALIFY_SERVICE_ORIGIN")
            .env_remove("LOCALIFY_SERVICE_OUTPUT")
            .env_remove("LOCALIFY_CLOUD_API")
            .env(PAUSE_DIR_ENV, self.root.join("pause"))
            .stdin(Stdio::null());
        command
    }

    /// Holds every `localify dev` at `point` until [`Sandbox::release`].
    fn arm(&self, point: &str) {
        fs::create_dir_all(self.root.join("pause")).unwrap();
        fs::write(self.root.join("pause").join(point), "").unwrap();
    }

    fn release(&self, point: &str) {
        fs::remove_file(self.root.join("pause").join(point)).unwrap();
        let _ = fs::remove_file(self.root.join("pause").join(format!("{point}.reached")));
    }

    fn wait_reached(&self, point: &str) {
        let reached = self.root.join("pause").join(format!("{point}.reached"));
        wait_for(point, || reached.exists().then_some(()));
    }

    fn config_rev(&self) -> String {
        self.item()["service"]["configRev"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// The service's `configRev` once it is no longer `previous`.
    fn changed_rev(&self, previous: &str) -> String {
        wait_for("a new configRev", || {
            Some(self.config_rev()).filter(|rev| rev != previous)
        })
    }

    /// Writes the config with the daemon's ports reserved, frees them only
    /// to start the daemon, and starts it again on fresh ones when it exits,
    /// as it does when another program took one meanwhile.
    fn start_daemon(&mut self) {
        for _ in 0..DAEMON_ATTEMPTS {
            let ports = [reserve(), reserve(), reserve()];
            let [http, https, dns] = ports
                .each_ref()
                .map(|port| port.local_addr().unwrap().port());
            fs::write(
                self.root.join("config.toml"),
                config_toml(&self.root, self.port, http, https, dns),
            )
            .unwrap();
            drop(ports);
            // Owned by the sandbox at once, so its drop ends it on every path,
            // a panic included.
            self.daemon = Some(
                self.localify(&["start", "--foreground"])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .unwrap(),
            );
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                let exited = self
                    .daemon
                    .as_mut()
                    .is_some_and(|daemon| daemon.try_wait().unwrap().is_some());
                if exited {
                    self.daemon = None;
                    break;
                }
                if self.try_inventory(None).is_some() {
                    return;
                }
                assert!(
                    Instant::now() < deadline,
                    "timed out waiting for the daemon"
                );
                thread::sleep(Duration::from_millis(100));
            }
        }
        panic!("the daemon did not start in {DAEMON_ATTEMPTS} attempts");
    }

    fn try_inventory(&self, have: Option<&str>) -> Option<Value> {
        let mut args = vec!["inventory"];
        if let Some(have) = have {
            args.extend(["--have", have]);
        }
        let output = self.localify(&args).output().unwrap();
        output
            .status
            .success()
            .then(|| serde_json::from_slice(&output.stdout).unwrap())
    }

    fn inventory(&self) -> Value {
        self.try_inventory(None).expect("the inventory")
    }

    fn item(&self) -> Value {
        let snapshot = self.inventory();
        assert_eq!(snapshot["items"].as_array().unwrap().len(), 1, "{snapshot}");
        snapshot["items"][0].clone()
    }

    /// A background executor of the service, as `dev --detach` starts one.
    fn spawn_background(&self, script: &str) -> Child {
        let mut command =
            self.localify(&["dev", "web", "--project", "app", "--", "sh", "-c", script]);
        command
            .env("LOCALIFY_SERVICE_ORIGIN", "background")
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
        command.spawn().unwrap()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = self.localify(&["stop", "web", "--project", "app"]).output();
        if let Some(mut daemon) = self.daemon.take() {
            // Through the sandbox's own socket: never a system service.
            if let Ok(mut socket) = UnixStream::connect(self.root.join("localify.sock")) {
                let _ = socket.write_all(b"{\"cmd\":\"shutdown\"}\n");
            }
            let deadline = Instant::now() + Duration::from_secs(10);
            while Instant::now() < deadline && daemon.try_wait().ok().flatten().is_none() {
                thread::sleep(Duration::from_millis(50));
            }
            let _ = daemon.kill();
            let _ = daemon.wait();
        }
    }
}

fn config_toml(root: &Path, port: u16, http: u16, https: u16, dns: u16) -> String {
    format!(
        r#"[daemon]
http_port = {http}
https_port = {https}
dns_port = {dns}
lan_access = false

[paths]
data_dir = "{root}/data"
pid_file = "{root}/localify.pid"
log_file = "{root}/logs/localify.log"
certs_dir = "{root}/data/certs"
socket_path = "{root}/localify.sock"

[docker]
enabled = false

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
root_path = "{root}/app"

[[projects.app.services]]
name = "web"
primary_domain = "web.localify"
command = ["sh", "-c", "exit 0"]
"#,
        root = root.display(),
    )
}

/// A port of its own, held while the listener lives.
fn reserve() -> TcpListener {
    TcpListener::bind("127.0.0.1:0").unwrap()
}

fn wait_for<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(value) = check() {
            return value;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(100));
    }
}

const PROFILES: &str = r#"[profiles.default]
sources = [{ file = ".env" }]

[profiles.default.env]
DRY_RUN = "inline-secret-1"
"#;

#[test]
fn the_daemon_reports_each_state_and_revision_of_a_service() {
    let mut sandbox = Sandbox::new();
    fs::write(
        sandbox.root.join("config.toml"),
        config_toml(&sandbox.root, sandbox.port, 1, 1, 1),
    )
    .unwrap();
    assert!(
        sandbox.try_inventory(None).is_none(),
        "no daemon, no inventory"
    );
    sandbox.start_daemon();

    // The reserved port listens, and no record explains it.
    let snapshot = sandbox.inventory();
    let epoch = snapshot["epoch"].as_str().unwrap().to_string();
    let rev = snapshot["rev"].as_u64().unwrap();
    let item = &snapshot["items"][0];
    assert_eq!(snapshot["schema"], 1);
    assert_eq!(item["owner"], "unknown");
    assert_eq!(item["port"]["reachable"], true);
    assert_eq!(item["output"]["reason"], "unknown_owner");
    assert_eq!(item["controls"]["start"]["code"], "control_disabled");
    let config_rev = item["service"]["configRev"].as_str().unwrap().to_string();
    assert_eq!(config_rev.len(), 16);

    // Unchanged: the same revision, and nothing sent again.
    let unchanged = sandbox
        .try_inventory(Some(&format!("{epoch}:{rev}")))
        .unwrap();
    assert_eq!(unchanged["unchanged"], true);
    assert!(unchanged.get("items").is_none());

    // Nothing runs, the port is free.
    drop(sandbox.service.take());
    let snapshot = sandbox.inventory();
    assert!(snapshot["rev"].as_u64().unwrap() > rev);
    let item = &snapshot["items"][0];
    assert_eq!(item["owner"], Value::Null);
    assert_eq!(item["process"], "stopped");
    assert_eq!(item["port"]["reachable"], false);
    assert_eq!(item["output"]["reason"], "no_run");

    // A background run, not listening yet: running, owned, with its output.
    let mut executor = sandbox.spawn_background("echo started; sleep 300");
    let item = wait_for("the run", || {
        let item = sandbox.item();
        (item["process"] == "running").then_some(item)
    });
    assert_eq!(item["owner"], "background");
    assert_eq!(item["port"]["reachable"], false);
    assert_eq!(item["output"]["available"], true);
    assert_eq!(item["output"]["runId"], item["run"]["runId"]);
    assert_eq!(item["run"]["legacy"], false);

    // Stopped: the output stays, with the exit.
    let stop = sandbox
        .localify(&["stop", "web", "--project", "app"])
        .output()
        .unwrap();
    assert!(stop.status.success());
    executor.wait().unwrap();
    let stopped = sandbox.item();
    assert_eq!(stopped["process"], "stopped");
    assert_eq!(stopped["owner"], Value::Null);
    assert_eq!(stopped["output"]["runId"], item["run"]["runId"]);
    assert_ne!(stopped["run"]["exit"], Value::Null);

    // Another program on the port takes the owner, not the run's output.
    let other = TcpListener::bind(("127.0.0.1", sandbox.port)).unwrap();
    let taken = sandbox.item();
    assert_eq!(taken["owner"], "unknown");
    assert_eq!(taken["output"]["runId"], item["run"]["runId"]);
    drop(other);

    // An inline profile value is part of the revision, never of the output.
    let profiles = sandbox.app.join("localify.toml");
    fs::write(&profiles, PROFILES).unwrap();
    let with_profiles = sandbox.item()["service"]["configRev"].clone();
    fs::write(
        &profiles,
        PROFILES.replace("inline-secret-1", "inline-secret-2"),
    )
    .unwrap();
    let snapshot = sandbox.inventory();
    let edited = snapshot["items"][0]["service"]["configRev"].clone();
    assert_ne!(with_profiles, config_rev);
    assert_ne!(edited, with_profiles);
    assert!(!snapshot.to_string().contains("inline-secret"));
}

/// A remote launch of `profile` that expects revision `rev`, as the daemon
/// will dispatch one.
fn remote_launch(sandbox: &Sandbox, id: &str, rev: &str, profile: Option<&str>) -> Command {
    let expires = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis()
        + 15_000;
    let mut args = vec!["dev", "web", "--project", "app"];
    if let Some(profile) = profile {
        args.extend(["--profile", profile]);
    }
    let remote = format!("--remote-launch={id}");
    let expires = expires.to_string();
    args.extend([
        remote.as_str(),
        "--launch-expires",
        &expires,
        "--expect-rev",
        rev,
        "--request",
        "machine-a:request-1",
    ]);
    let mut command = sandbox.localify(&args);
    command.stdout(Stdio::null()).stderr(Stdio::piped());
    command
}

/// `services/launches/<id>.json`.
fn launch_record(sandbox: &Sandbox, id: &str) -> Value {
    let services = if cfg!(target_os = "macos") {
        sandbox
            .root
            .join("Library/Application Support")
            .join(BRANDING.desktop_state_dir_name)
            .join("services")
    } else {
        sandbox
            .root
            .join("state")
            .join(BRANDING.slug)
            .join("services")
    };
    let path = services.join("launches").join(format!("{id}.json"));
    serde_json::from_slice(&fs::read(&path).unwrap()).unwrap()
}

const STAGE: &str = r#"[profiles.default]
sources = []

[profiles.stage]
sources = []
"#;

/// A remote launch held after the checks it passed before its lock, while
/// the test changes `localify.toml` to `changed`: refused under the lock.
fn changed_during_launch(sandbox: &Sandbox, id: &str, rev: &str, changed: &str) {
    sandbox.arm("launch-preflight-passed");
    let launch = remote_launch(sandbox, id, rev, Some("stage"))
        .spawn()
        .unwrap();
    sandbox.wait_reached("launch-preflight-passed");
    fs::write(sandbox.app.join("localify.toml"), changed).unwrap();
    sandbox.release("launch-preflight-passed");
    let output = launch.wait_with_output().unwrap();
    assert!(!output.status.success());
    let record = launch_record(sandbox, id);
    assert_eq!(record["outcome"], "refused", "{record}");
    assert_eq!(record["code"], "conflict", "{record}");
}

#[test]
fn a_remote_launch_runs_only_the_definition_and_profile_it_was_checked_against() {
    let mut sandbox = Sandbox::new();
    sandbox.start_daemon();
    drop(sandbox.service.take());
    fs::write(sandbox.app.join("localify.toml"), STAGE).unwrap();
    let rev = sandbox.config_rev();

    // The executor computes the revision the inventory shows, and runs.
    let admitted = "AAAAAAAAAAAAAAAAAAAAAA";
    let output = remote_launch(&sandbox, admitted, &rev, Some("stage"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let record = launch_record(&sandbox, admitted);
    assert_eq!(record["outcome"], "admitted");
    assert_eq!(record["requestedBy"]["machineId"], "machine-a");
    assert_eq!(record["requestedBy"]["requestId"], "request-1");
    assert_eq!(record["exit"]["code"], 0);
    assert_eq!(sandbox.item()["run"]["runId"], admitted);

    // Changed while the executor runs, after the checks it passed first:
    // to other sources, then to dangerous.
    let sources = STAGE.replace(
        "[profiles.stage]\nsources = []",
        "[profiles.stage]\nsources = [{ file = \".env.stage\" }]",
    );
    changed_during_launch(&sandbox, "BAAAAAAAAAAAAAAAAAAAAA", &rev, &sources);
    let rev = sandbox.changed_rev(&rev);
    let dangerous = sources.replace("[profiles.stage]\n", "[profiles.stage]\ndanger = true\n");
    changed_during_launch(&sandbox, "CAAAAAAAAAAAAAAAAAAAAA", &rev, &dangerous);

    // Checked against the new revision, a dangerous profile is still refused.
    let fresh = sandbox.changed_rev(&rev);
    let refused = "DAAAAAAAAAAAAAAAAAAAAA";
    let output = remote_launch(&sandbox, refused, &fresh, Some("stage"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(launch_record(&sandbox, refused)["code"], "refused_profile");

    // None of them started a run.
    assert_eq!(sandbox.item()["run"]["runId"], admitted);
}

/// A `package.json` whose `dev` script ends at once.
const DEV_PACKAGE: &str = r#"{"name": "web", "scripts": {"dev": "node -e 0"}}"#;

#[test]
fn a_remote_launch_matches_the_inventory_s_revision_when_its_launch_is_detected() {
    let mut sandbox = Sandbox::new();
    sandbox.start_daemon();
    drop(sandbox.service.take());
    let configured = "command = [\"sh\", \"-c\", \"exit 0\"]\n";
    let config = sandbox.root.join("config.toml");
    let edit = |to: &str| {
        let content = fs::read_to_string(&config).unwrap();
        assert!(content.contains(configured));
        fs::write(&config, content.replace(configured, to)).unwrap();
    };
    let mut rev = sandbox.config_rev();
    let mut check = |id: &str, what: &str| {
        rev = sandbox.changed_rev(&rev);
        let output = remote_launch(&sandbox, id, &rev, None).output().unwrap();
        let record = launch_record(&sandbox, id);
        assert_eq!(
            record["outcome"],
            "admitted",
            "{what}: {record}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };

    // No command: the package's `dev` script is detected.
    edit("");
    fs::write(sandbox.app.join("package.json"), DEV_PACKAGE).unwrap();
    check("EAAAAAAAAAAAAAAAAAAAAA", "an inferred command");

    // No folder either: the package of the service's name is found.
    fs::remove_file(sandbox.app.join("package.json")).unwrap();
    fs::create_dir_all(sandbox.app.join("apps/web")).unwrap();
    fs::write(sandbox.app.join("apps/web/package.json"), DEV_PACKAGE).unwrap();
    check("FAAAAAAAAAAAAAAAAAAAAA", "an inferred folder");

    // A legacy `dev-localify` command, which detection drops.
    fs::remove_dir_all(sandbox.app.join("apps")).unwrap();
    fs::write(sandbox.app.join("package.json"), DEV_PACKAGE).unwrap();
    let content = fs::read_to_string(&config).unwrap();
    fs::write(
        &config,
        content.replace(
            "primary_domain = \"web.localify\"\n",
            "primary_domain = \"web.localify\"\ncommand = [\"pnpm\", \"dev-localify\"]\n",
        ),
    )
    .unwrap();
    check("GAAAAAAAAAAAAAAAAAAAAA", "a legacy command");
}
