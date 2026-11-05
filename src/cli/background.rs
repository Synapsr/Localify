//! Services that run without the caller's terminal, and the commands that act
//! on any service `localify dev` started: `status`, `logs`, `stop`, `restart`.
//! `dev --in-app` starts a service in the desktop app's terminal, as its launch
//! button does; `dev --detach` starts one in the background. Both write the
//! service's output to a file that `logs` reads, so a person in the app and an
//! agent read the same lines.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::IsTerminal;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::Serialize;

use super::context::AppContext;
use super::services::{RunTarget, resolve_launch_target, resolve_service_key};
use crate::branding::BRANDING;
use crate::infrastructure::launch_sessions::unix_now;
use crate::infrastructure::running_services::{
    self, OutputChunk, RunningService, RunningServiceStore, SERVICE_ORIGIN_ENV, SERVICE_OUTPUT_ENV,
    ServiceKey, ServiceOrigin, service_selector,
};

/// A first `next dev` can take this long before it listens.
const START_TIMEOUT: Duration = Duration::from_secs(90);
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const FAILURE_OUTPUT_LINES: usize = 40;

/// Runs the same `dev` command again in the background, with its output in a
/// file, and returns once the service listens, stopped, or is still starting.
pub fn start_detached() -> Result<()> {
    let store = RunningServiceStore::new_default()?;
    let log_path = store.new_log_file(&format!("background-{}", std::process::id()))?;
    let log = File::options()
        .append(true)
        .open(&log_path)
        .with_context(|| format!("Failed to open '{}'", log_path.display()))?;

    let mut command = Command::new(current_exe()?);
    command
        .args(foreground_args(std::env::args_os().skip(1)))
        .env(SERVICE_OUTPUT_ENV, &log_path)
        .env(SERVICE_ORIGIN_ENV, ServiceOrigin::Background.as_env())
        .stdin(Stdio::null())
        .stdout(log.try_clone().context("Failed to share the output file")?)
        .stderr(log)
        // Its own process group: closing the terminal or the agent's shell leaves it running.
        .process_group(0);
    let mut child = command.spawn().context("Failed to start the service")?;
    wait_for_child(&mut child, &store, &log_path)
}

/// The arguments of this command without `--detach`. What follows `--` is the
/// service's own command and stays as it is.
fn foreground_args(args: impl Iterator<Item = OsString>) -> Vec<OsString> {
    let mut in_service_command = false;
    args.filter(|arg| {
        if in_service_command {
            return true;
        }
        if arg == "--" {
            in_service_command = true;
        }
        arg != "--detach"
    })
    .collect()
}

fn wait_for_child(child: &mut Child, store: &RunningServiceStore, log_path: &Path) -> Result<()> {
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        let run = store.list()?.into_iter().find(|run| run.pid == child.id());
        if let Some(status) = child.try_wait().context("Failed to watch the service")? {
            let output = last_lines(log_path);
            if run.is_none() {
                let _ = fs::remove_file(log_path);
            }
            bail!("The service stopped while starting ({status}). Its last output:\n\n{output}");
        }
        if let Some(run) = &run
            && listens(run.port)
        {
            print_started(run, "Started");
            return Ok(());
        }
        if Instant::now() >= deadline {
            match run {
                Some(run) => print_started(&run, "Still starting"),
                None => println!(
                    "{} is still preparing the service in the background (PID {}).\n  output: {}",
                    BRANDING.app_name,
                    child.id(),
                    log_path.display()
                ),
            }
            return Ok(());
        }
        thread::sleep(POLL_INTERVAL);
    }
}

/// Asks the desktop app to start the service in its terminal, as its launch
/// button does, and returns once the service listens.
pub fn start_in_app(
    target: RunTarget<'_>,
    profile: Option<&str>,
    config_path: &Path,
    ctx: &AppContext,
) -> Result<()> {
    let launch = resolve_launch_target(target, config_path, ctx)?;
    let Some(environment) = launch.key.environment.clone() else {
        bail!(
            "'{}' has no project environment for the app to start it in. Run '{} project env list' to check the project.",
            launch.key.service_id,
            BRANDING.cli_command()
        );
    };
    let store = RunningServiceStore::new_default()?;
    let previous = store.get(&launch.key)?;
    if let Some(run) = previous.as_ref().filter(|run| run.is_running()) {
        println!(
            "'{}' is already running (PID {}, {}), not started again.\n  url: {}",
            run.key.service_id,
            run.pid,
            describe_origin(run.origin),
            run.url
        );
        return Ok(());
    }
    if listens(launch.port) {
        println!(
            "Something already answers on port {} for '{}', not started again.\n  url: {}",
            launch.port, launch.key.service_id, launch.url
        );
        return Ok(());
    }

    let since = unix_now();
    open_url(&launch_service_link(&launch.key, &environment, profile))?;
    println!(
        "Asked the {} app to start '{}' in its terminal.",
        BRANDING.app_name, launch.key.service_id
    );
    wait_for_app_run(&store, &launch.key, since)
}

/// `localify://action/launch-service?project=…&env=…&service=…[&profile=…]`
fn launch_service_link(key: &ServiceKey, environment: &str, profile: Option<&str>) -> String {
    let mut link = format!(
        "localify://action/launch-service?project={}&env={}&service={}",
        percent_encode(&key.project_id),
        percent_encode(environment),
        percent_encode(&key.service_id)
    );
    if let Some(profile) = profile {
        link.push_str(&format!("&profile={}", percent_encode(profile)));
    }
    link
}

fn percent_encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

fn open_url(url: &str) -> Result<()> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let output = Command::new(opener)
        .arg(url)
        .output()
        .with_context(|| format!("Failed to run {opener}"))?;
    if !output.status.success() {
        bail!(
            "The {} app did not open ({}). Install it from https://localify.dev, or start the service without it: {} dev --detach",
            BRANDING.app_name,
            String::from_utf8_lossy(&output.stderr).trim(),
            BRANDING.cli_command()
        );
    }
    Ok(())
}

/// Waits for the run the app starts: a record newer than `since` whose process
/// listens, or ended.
fn wait_for_app_run(store: &RunningServiceStore, key: &ServiceKey, since: u64) -> Result<()> {
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        let run = store.get(key)?.filter(|run| run.started_at >= since);
        if let Some(run) = &run {
            if let Some(exit) = &run.exit {
                bail!(
                    "'{}' stopped while starting ({}). Its last output:\n\n{}",
                    key.service_id,
                    describe_exit_code(exit.code),
                    run.log_file.as_deref().map(last_lines).unwrap_or_default()
                );
            }
            if listens(run.port) {
                print_started(run, "Started");
                return Ok(());
            }
        }
        if Instant::now() >= deadline {
            match run {
                Some(run) => print_started(&run, "Still starting"),
                None => println!(
                    "The app has not started '{}' yet. Look at its terminal panel, or check with: {} services status",
                    key.service_id,
                    BRANDING.cli_command()
                ),
            }
            return Ok(());
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn listens(port: u16) -> bool {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    TcpStream::connect_timeout(&address, Duration::from_millis(200)).is_ok()
}

fn print_started(run: &RunningService, state: &str) {
    let selector = service_selector(&run.key);
    println!(
        "{state} '{}' {} (PID {}).",
        run.key.service_id,
        describe_origin(run.origin),
        run.pid
    );
    println!("  url: {}", run.url);
    println!("  logs: {} logs {selector}", BRANDING.cli_command());
    println!("  stop: {} stop {selector}", BRANDING.cli_command());
}

fn describe_origin(origin: ServiceOrigin) -> &'static str {
    match origin {
        ServiceOrigin::App => "in the app's terminal",
        ServiceOrigin::Background => "in the background",
        ServiceOrigin::Terminal => "in a terminal",
    }
}

fn describe_exit_code(code: Option<i32>) -> String {
    match code {
        Some(code) => format!("exit code {code}"),
        None => "ended by a signal".into(),
    }
}

fn last_lines(path: &Path) -> String {
    running_services::tail(path, FAILURE_OUTPUT_LINES)
        .map(|chunk| chunk.lines.join("\n"))
        .unwrap_or_default()
}

pub fn stop(
    service: &str,
    project: Option<&str>,
    environment: Option<&str>,
    ctx: &AppContext,
) -> Result<()> {
    let key = resolve_service_key(service, project, environment, ctx)?;
    let run = RunningServiceStore::new_default()?.get(&key)?;
    match run {
        Some(run) if running_services::stop(&run)? => {
            println!("Stopped '{}' (PID {}).", key.service_id, run.pid);
        }
        _ => println!("'{}' is not running.", key.service_id),
    }
    Ok(())
}

/// Stops the service and starts it again where it ran, on the same launch
/// profile. One started in a terminal Localify does not own restarts in the app.
pub fn restart(
    service: &str,
    project: Option<&str>,
    environment: Option<&str>,
    config_path: &Path,
    ctx: &AppContext,
) -> Result<()> {
    let key = resolve_service_key(service, project, environment, ctx)?;
    let Some(run) = RunningServiceStore::new_default()?.get(&key)? else {
        bail!(
            "'{}' was never started by {}. Start it with: {} dev {} --in-app",
            key.service_id,
            BRANDING.app_name,
            BRANDING.cli_command(),
            service_selector(&key)
        );
    };
    if running_services::stop(&run)? {
        println!("Stopped '{}' (PID {}).", key.service_id, run.pid);
    }

    let mut command = Command::new(current_exe()?);
    command.arg("--config").arg(config_path).args([
        "dev",
        &key.service_id,
        "--project",
        &key.project_id,
    ]);
    if let Some(environment) = &key.environment {
        command.args(["--env", environment]);
    }
    if let Some(profile) = &run.profile {
        command.args(["--profile", profile]);
    }
    command.arg(match run.origin {
        ServiceOrigin::Background => "--detach",
        ServiceOrigin::App | ServiceOrigin::Terminal => "--in-app",
    });
    let status = command.status().context("Failed to start the service")?;
    if !status.success() {
        bail!("'{}' did not start again.", key.service_id);
    }
    Ok(())
}

pub struct LogsArgs<'a> {
    pub service: &'a str,
    pub project: Option<&'a str>,
    pub environment: Option<&'a str>,
    pub lines: usize,
    pub follow: bool,
    pub cursor: Option<u64>,
}

/// Prints a service's output as the app's terminal shows it, without colors.
/// With a cursor, only what came after it. A reader that is not a terminal also
/// gets the cursor to pass next time.
pub fn logs(args: LogsArgs<'_>, ctx: &AppContext) -> Result<()> {
    let key = resolve_service_key(args.service, args.project, args.environment, ctx)?;
    let store = RunningServiceStore::new_default()?;
    let Some(run) = store.get(&key)? else {
        bail!(
            "'{}' has no recorded run. Start it with: {} dev {} --in-app",
            key.service_id,
            BRANDING.cli_command(),
            service_selector(&key)
        );
    };
    let Some(log_file) = run.log_file.clone() else {
        bail!(
            "'{}' was started in a terminal {} does not own, and its output is only there. Start it with --in-app or --detach to read it here.",
            key.service_id,
            BRANDING.app_name
        );
    };
    eprintln!("{}\n", describe_run(&run));

    let mut chunk = match args.cursor {
        Some(cursor) => running_services::read_from(&log_file, cursor)?,
        None => running_services::tail(&log_file, args.lines)?,
    };
    print_lines(&chunk);
    if args.follow {
        follow(&store, &key, &log_file, &mut chunk)?;
    }
    if args.cursor.is_some() || !std::io::stdout().is_terminal() {
        eprintln!("\nNext cursor: {}", chunk.cursor);
    }
    Ok(())
}

/// Prints new lines as they come, until the run ends.
fn follow(
    store: &RunningServiceStore,
    key: &ServiceKey,
    log_file: &Path,
    chunk: &mut OutputChunk,
) -> Result<()> {
    loop {
        thread::sleep(Duration::from_millis(500));
        let running = store.get(key)?.is_some_and(|run| run.is_running());
        *chunk = running_services::read_from(log_file, chunk.cursor)?;
        print_lines(chunk);
        if !running {
            return Ok(());
        }
    }
}

fn print_lines(chunk: &OutputChunk) {
    for line in &chunk.lines {
        println!("{line}");
    }
}

fn describe_run(run: &RunningService) -> String {
    let started = elapsed(run.started_at);
    if run.is_running() {
        return format!(
            "'{}' is running {} (PID {}), started {started} ago.",
            run.key.service_id,
            describe_origin(run.origin),
            run.pid
        );
    }
    match &run.exit {
        Some(exit) => format!(
            "'{}' stopped {} ago ({}).",
            run.key.service_id,
            elapsed(exit.at),
            describe_exit_code(exit.code)
        ),
        None => format!("'{}' has stopped.", run.key.service_id),
    }
}

fn elapsed(since: u64) -> String {
    let seconds = unix_now().saturating_sub(since);
    // Minutes are enough once past one: "3m 12s" reads worse than "3m".
    let seconds = if seconds >= 60 {
        seconds - seconds % 60
    } else {
        seconds
    };
    humantime::format_duration(Duration::from_secs(seconds)).to_string()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StatusOutput<'a> {
    #[serde(flatten)]
    service: &'a RunningService,
    running: bool,
}

/// Every service Localify started, running or stopped, in every environment.
pub fn status(json: bool) -> Result<()> {
    let services = RunningServiceStore::new_default()?.list()?;
    if json {
        let output = services
            .iter()
            .map(|service| StatusOutput {
                service,
                running: service.is_running(),
            })
            .collect::<Vec<_>>();
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }
    if services.is_empty() {
        println!("{} has not started any service yet.", BRANDING.app_name);
        return Ok(());
    }
    for service in services {
        println!("{}", service_selector(&service.key));
        println!("  {}", describe_run(&service));
        println!("  url: {}  port: {}", service.url, service.port);
    }
    Ok(())
}

fn current_exe() -> Result<std::path::PathBuf> {
    std::env::current_exe().with_context(|| {
        format!(
            "Failed to resolve the {} executable path",
            BRANDING.cli_command()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn the_background_run_drops_only_localify_s_detach_flag() {
        let result = foreground_args(
            args(&[
                "dev", "web", "--detach", "--env", "feat", "--", "pnpm", "dev", "--detach",
            ])
            .into_iter(),
        );

        assert_eq!(
            result,
            args(&[
                "dev", "web", "--env", "feat", "--", "pnpm", "dev", "--detach"
            ])
        );
    }

    #[test]
    fn the_launch_link_names_the_service_and_escapes_values() {
        let key = ServiceKey {
            project_id: "my app".into(),
            environment: Some("feat-auth".into()),
            service_id: "web".into(),
        };

        assert_eq!(
            launch_service_link(&key, "feat-auth", Some("staging")),
            "localify://action/launch-service?project=my%20app&env=feat-auth&service=web&profile=staging"
        );
    }
}
