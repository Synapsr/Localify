//! Services that run without the caller's terminal, and the commands that act
//! on any service `localify dev` started: `status`, `logs`, `stop`, `restart`.
//! `dev --in-app` starts a service in the desktop app's terminal, as its launch
//! button does; `dev --detach` starts one in the background, where its output
//! goes to the run's stream. `logs` reads either, so a person in the app and an
//! agent read the same lines.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, IsTerminal};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::os::unix::process::{CommandExt, ExitStatusExt};
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
    self, RunState, RunningService, RunningServiceStore, SERVICE_ORIGIN_ENV, ServiceKey,
    ServiceOrigin, service_selector,
};
use crate::infrastructure::runs::RunId;
use crate::infrastructure::runs::output::{self, StreamLine, StreamRead};

/// A first `next dev` can take this long before it listens.
const START_TIMEOUT: Duration = Duration::from_secs(90);
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const FAILURE_OUTPUT_LINES: usize = 40;
/// How much of a run's stream one `logs` call reads at most.
const LOGS_WINDOW: u64 = 1024 * 1024;
/// How much one read of a stream takes.
const READ_CHUNK: usize = 64 * 1024;

/// Runs the same `dev` command again in the background, in its own session
/// and with its output in the run's stream, and returns once the service
/// listens, stopped, or is still starting.
pub fn start_detached(launch_id: Option<&str>) -> Result<i32> {
    let store = RunningServiceStore::new_default()?;
    let (launch_id, chosen) = match launch_id {
        Some(id) => (RunId::parse(id)?, false),
        None => (RunId::generate()?, true),
    };
    // The executor's own messages, such as why it refused to start.
    let launch_log = store.launch_log(&launch_id)?;
    let log = File::options()
        .create(true)
        .append(true)
        .open(&launch_log)
        .with_context(|| format!("Failed to open '{}'", launch_log.display()))?;

    let mut command = Command::new(current_exe()?);
    command
        .args(foreground_args(
            std::env::args_os().skip(1),
            chosen.then_some(&launch_id),
        ))
        .env(SERVICE_ORIGIN_ENV, ServiceOrigin::Background.as_env())
        .stdin(Stdio::null())
        .stdout(log.try_clone().context("Failed to share the launch log")?)
        .stderr(log);
    // Its own session: closing the terminal or the agent's shell leaves it
    // running, and the run's session and process group are the executor's PID.
    // SAFETY: the closure runs in the child between fork and exec and only calls
    // setsid, which is async-signal-safe and touches no memory.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().context("Failed to start the service")?;
    wait_for_child(&mut child, &store, &launch_id, &launch_log)
}

/// The arguments of this command without `--detach`, with the launch id this
/// dispatcher chose. What follows `--` is the service's own command and stays
/// as it is.
fn foreground_args(
    args: impl Iterator<Item = OsString>,
    launch_id: Option<&RunId>,
) -> Vec<OsString> {
    let mut foreground = Vec::new();
    let mut in_service_command = false;
    for arg in args {
        if !in_service_command && arg == "--" {
            in_service_command = true;
            push_launch_id(&mut foreground, launch_id);
        }
        if in_service_command || arg != "--detach" {
            foreground.push(arg);
        }
    }
    if !in_service_command {
        push_launch_id(&mut foreground, launch_id);
    }
    foreground
}

/// In one argument: an id may start with `-`, which would read as a flag.
fn push_launch_id(args: &mut Vec<OsString>, launch_id: Option<&RunId>) {
    if let Some(launch_id) = launch_id {
        args.push(format!("--launch-id={launch_id}").into());
    }
}

fn wait_for_child(
    child: &mut Child,
    store: &RunningServiceStore,
    launch_id: &RunId,
    launch_log: &Path,
) -> Result<i32> {
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        let run = store
            .list()?
            .into_iter()
            .find(|run| run.run_id() == Some(launch_id));
        if let Some(status) = child.try_wait().context("Failed to watch the service")? {
            let output = match &run {
                Some(run) => last_lines(store, run),
                None => String::new(),
            };
            let output = match output.is_empty() {
                true => last_file_lines(launch_log),
                false => output,
            };
            if run.is_none() {
                let _ = fs::remove_file(launch_log);
            }
            eprintln!(
                "The service stopped while starting ({status}). Its last output:\n\n{output}"
            );
            // As a shell reports a job: its code, or 128 + N for signal N.
            return Ok(status
                .code()
                .unwrap_or_else(|| 128 + status.signal().unwrap_or(0)));
        }
        if let Some(run) = &run
            && listens(run.port)
        {
            print_started(run, "Started");
            return Ok(0);
        }
        if Instant::now() >= deadline {
            match run {
                Some(run) => print_started(&run, "Still starting"),
                None => println!(
                    "{} is still preparing the service in the background (PID {}).\n  output: {}",
                    BRANDING.app_name,
                    child.id(),
                    launch_log.display()
                ),
            }
            return Ok(0);
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
    if let Some(run) = previous.as_ref().filter(|run| store.is_running(run)) {
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
                    last_lines(store, run)
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

/// The last lines of a run's output, from its stream or the app's file.
fn last_lines(store: &RunningServiceStore, run: &RunningService) -> String {
    if let Some(run_id) = run.run_id().filter(|_| run.has_stream()) {
        return stream_tail(&store.run_dir(run_id), FAILURE_OUTPUT_LINES)
            .map(|(lines, _)| join_lines(&lines))
            .unwrap_or_default();
    }
    run.log_file
        .as_deref()
        .map(last_file_lines)
        .unwrap_or_default()
}

fn last_file_lines(path: &Path) -> String {
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
    let store = RunningServiceStore::new_default()?;
    match store.get(&key)? {
        Some(run) if store.stop(&run)? => {
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
    let store = RunningServiceStore::new_default()?;
    let Some(run) = store.get(&key)? else {
        bail!(
            "'{}' was never started by {}. Start it with: {} dev {} --in-app",
            key.service_id,
            BRANDING.app_name,
            BRANDING.cli_command(),
            service_selector(&key)
        );
    };
    if store.stop(&run)? {
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
    pub cursor: Option<&'a str>,
}

/// Where `logs` reads on from: `<runId>:<offset>` in a run's stream, or a byte
/// offset in an app run's output file.
#[derive(Debug, PartialEq, Eq)]
enum Cursor {
    Stream { run_id: RunId, offset: u64 },
    File(u64),
}

impl Cursor {
    fn parse(value: &str) -> Result<Self> {
        let invalid = || {
            format!(
                "'{value}' is not a cursor. Pass back the one the previous `logs` call printed."
            )
        };
        match value.split_once(':') {
            Some((run_id, offset)) => Ok(Self::Stream {
                run_id: RunId::parse(run_id).with_context(invalid)?,
                offset: offset.parse().with_context(invalid)?,
            }),
            None => Ok(Self::File(value.parse().with_context(invalid)?)),
        }
    }
}

/// Prints a service's output without colors: a background run's stream, or
/// what the app's terminal shows. With a cursor, only what came after it. A
/// reader that is not a terminal also gets the cursor to pass next time.
pub fn logs(args: LogsArgs<'_>, ctx: &AppContext) -> Result<()> {
    let key = resolve_service_key(args.service, args.project, args.environment, ctx)?;
    let cursor = args.cursor.map(Cursor::parse).transpose()?;
    let store = RunningServiceStore::new_default()?;
    let Some(run) = store.get(&key)? else {
        bail!(
            "'{}' has no recorded run. Start it with: {} dev {} --in-app",
            key.service_id,
            BRANDING.cli_command(),
            service_selector(&key)
        );
    };
    eprintln!("{}\n", describe_run(&store, &run));
    if let Some(run_id) = run.run_id().filter(|_| run.has_stream()) {
        return stream_logs(&store, &run, run_id, cursor, &args);
    }
    let Some(log_file) = run.log_file.clone() else {
        bail!(
            "'{}' was started in a terminal {} does not own, and its output is only there. Start it with --in-app or --detach to read it here.",
            key.service_id,
            BRANDING.app_name
        );
    };
    let cursor = match cursor {
        Some(Cursor::File(offset)) => Some(offset),
        Some(Cursor::Stream { .. }) | None => None,
    };
    file_logs(&store, &key, &log_file, cursor, &args)
}

fn stream_logs(
    store: &RunningServiceStore,
    run: &RunningService,
    run_id: &RunId,
    cursor: Option<Cursor>,
    args: &LogsArgs<'_>,
) -> Result<()> {
    let dir = store.run_dir(run_id);
    let mut offset = match cursor {
        None => {
            let (lines, next) = stream_tail(&dir, args.lines)?;
            print_stream_lines(&lines);
            next
        }
        Some(Cursor::Stream {
            run_id: ref asked,
            offset,
        }) if asked == run_id => offset,
        Some(_) => {
            eprintln!(
                "'{}' restarted since that cursor: reading its current run from the start.",
                run.key.service_id
            );
            0
        }
    };
    if args.cursor.is_some() {
        offset = read_span(&dir, offset, LOGS_WINDOW, print_stream_chunk)?.next;
    }
    if args.follow {
        offset = follow_stream(store, run, &dir, offset)?;
    }
    if args.cursor.is_some() || !std::io::stdout().is_terminal() {
        eprintln!("\nNext cursor: {run_id}:{offset}");
    }
    Ok(())
}

/// The last `count` lines of a run's stream, from at most its last
/// [`LOGS_WINDOW`] bytes, and the offset after them.
fn stream_tail(dir: &Path, count: usize) -> Result<(Vec<StreamLine>, u64)> {
    let (first, total) = output::extent(dir)?;
    let start = first.max(total.saturating_sub(LOGS_WINDOW));
    // Reading from the middle of the stream starts inside a line: drop it.
    let mut skipping = start > 0;
    let mut lines = VecDeque::new();
    let last = read_span(dir, start, LOGS_WINDOW, |chunk| {
        for line in &chunk.lines {
            if skipping {
                skipping = line.cut;
                continue;
            }
            lines.push_back(line.clone());
            if lines.len() > count {
                lines.pop_front();
            }
        }
    })?;
    Ok((lines.into(), last.next))
}

/// Reads from `offset` until caught up or `budget` bytes were read, one
/// bounded read at a time, and returns the last read.
fn read_span(
    dir: &Path,
    mut offset: u64,
    budget: u64,
    mut each: impl FnMut(&StreamRead),
) -> Result<StreamRead> {
    let started = offset;
    loop {
        let chunk = output::read(dir, offset, READ_CHUNK)?;
        each(&chunk);
        if chunk.next == offset || chunk.next - started >= budget {
            return Ok(chunk);
        }
        offset = chunk.next;
    }
}

/// Prints new lines as they come, until the run ends, or until another run of
/// the service replaces it and its stream goes.
fn follow_stream(
    store: &RunningServiceStore,
    run: &RunningService,
    dir: &Path,
    mut offset: u64,
) -> Result<u64> {
    loop {
        let current = store.get(&run.key)?;
        let current_id = current.as_ref().and_then(RunningService::run_id);
        if current_id != run.run_id() || !dir.is_dir() {
            match current_id {
                Some(next) if Some(next) != run.run_id() => {
                    eprintln!("'{}' restarted as run {next}.", run.key.service_id)
                }
                _ => eprintln!("The run of '{}' ended.", run.key.service_id),
            }
            return Ok(offset);
        }
        let chunk = match read_span(dir, offset, LOGS_WINDOW, print_stream_chunk) {
            Ok(chunk) => chunk,
            // Removed while it was read: the next look says by what.
            Err(_) if !dir.is_dir() => continue,
            Err(error) => return Err(error),
        };
        let caught_up = chunk.next == offset;
        offset = chunk.next;
        if chunk.ended || (caught_up && !store.is_running(run)) {
            return Ok(offset);
        }
        if caught_up {
            thread::sleep(POLL_INTERVAL);
        }
    }
}

fn print_stream_chunk(chunk: &StreamRead) {
    if let Some(first) = chunk.rotated {
        eprintln!("Earlier output was dropped: it starts at offset {first}.");
    }
    print_stream_lines(&chunk.lines);
    if chunk.interrupted {
        match &chunk.capture_error {
            Some(error) => eprintln!("Output was cut off: writing it failed ({error})."),
            None => eprintln!("Output was cut off when {} stopped.", BRANDING.app_name),
        }
    }
}

fn print_stream_lines(lines: &[StreamLine]) {
    for line in lines {
        if line.cut {
            print!("{}", line.text);
        } else {
            println!("{}", line.text);
        }
    }
}

fn join_lines(lines: &[StreamLine]) -> String {
    let mut text = String::new();
    for line in lines {
        text.push_str(&line.text);
        if !line.cut {
            text.push('\n');
        }
    }
    text.trim_end().to_string()
}

/// An app run's output, from the file the app's terminal writes.
fn file_logs(
    store: &RunningServiceStore,
    key: &ServiceKey,
    log_file: &Path,
    cursor: Option<u64>,
    args: &LogsArgs<'_>,
) -> Result<()> {
    let mut chunk = match cursor {
        Some(cursor) => running_services::read_from(log_file, cursor, LOGS_WINDOW)?,
        None => running_services::tail(log_file, args.lines)?,
    };
    print_file_lines(&chunk.lines);
    if args.follow {
        loop {
            thread::sleep(Duration::from_millis(500));
            let running = store.get(key)?.is_some_and(|run| store.is_running(&run));
            chunk = running_services::read_from(log_file, chunk.cursor, LOGS_WINDOW)?;
            print_file_lines(&chunk.lines);
            if !running {
                break;
            }
        }
    }
    if cursor.is_some() || !std::io::stdout().is_terminal() {
        eprintln!("\nNext cursor: {}", chunk.cursor);
    }
    Ok(())
}

fn print_file_lines(lines: &[String]) {
    for line in lines {
        println!("{line}");
    }
}

fn describe_run(store: &RunningServiceStore, run: &RunningService) -> String {
    let started = elapsed(run.started_at);
    let state = store.state(run);
    if state == RunState::Unknown {
        return running_services::unverified_legacy_run(run);
    }
    if state == RunState::Running {
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
    /// `unknown` for a run an older version recorded that Localify cannot verify.
    state: RunState,
}

/// Every service Localify started, running or stopped, in every environment.
pub fn status(json: bool) -> Result<()> {
    let store = RunningServiceStore::new_default()?;
    let services = store.list()?;
    if json {
        let output = services
            .iter()
            .map(|service| StatusOutput {
                service,
                running: store.is_running(service),
                state: store.state(service),
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
        println!("  {}", describe_run(&store, &service));
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
    fn a_launch_id_that_starts_with_a_dash_is_passed_as_one_argument() {
        let launch_id = RunId::parse("-qAAAAAAAAAAAAAAAAAAAA").unwrap();

        let result = foreground_args(
            args(&["dev", "web", "--detach"]).into_iter(),
            Some(&launch_id),
        );

        assert_eq!(
            result,
            args(&["dev", "web", "--launch-id=-qAAAAAAAAAAAAAAAAAAAA"])
        );
    }

    #[test]
    fn the_background_run_drops_only_localify_s_detach_flag() {
        let launch_id = RunId::parse("AAAAAAAAAAAAAAAAAAAAAA").unwrap();
        let result = foreground_args(
            args(&[
                "dev", "web", "--detach", "--env", "feat", "--", "pnpm", "dev", "--detach",
            ])
            .into_iter(),
            Some(&launch_id),
        );

        assert_eq!(
            result,
            args(&[
                "dev",
                "web",
                "--env",
                "feat",
                "--launch-id=AAAAAAAAAAAAAAAAAAAAAA",
                "--",
                "pnpm",
                "dev",
                "--detach"
            ])
        );
    }

    #[test]
    fn a_launch_id_given_by_the_caller_is_not_added_twice() {
        let result = foreground_args(
            args(&[
                "dev",
                "web",
                "--launch-id",
                "AAAAAAAAAAAAAAAAAAAAAA",
                "--detach",
            ])
            .into_iter(),
            None,
        );

        assert_eq!(
            result,
            args(&["dev", "web", "--launch-id", "AAAAAAAAAAAAAAAAAAAAAA"])
        );
    }

    #[test]
    fn following_a_run_ends_cleanly_when_another_run_replaces_it() {
        use crate::infrastructure::running_services::NewRun;
        let dir = tempfile::tempdir().unwrap();
        let store = RunningServiceStore::new(dir.path().to_path_buf());
        let key = ServiceKey {
            project_id: "app".into(),
            environment: None,
            service_id: "web".into(),
        };
        let new_run = || NewRun {
            key: key.clone(),
            run_id: RunId::generate().unwrap(),
            url: "https://web.localify".into(),
            port: 3000,
            origin: ServiceOrigin::Background,
            profile: None,
            output: None,
        };
        let lock = store.lock(&key).unwrap();
        let first = store.register(&lock, new_run()).unwrap();
        let first_dir = store.run_dir(first.run_id().unwrap());
        let (done, finished) = std::sync::mpsc::channel();

        thread::scope(|scope| {
            scope.spawn(|| {
                let _ = done.send(follow_stream(&store, &first, &first_dir, 0));
            });
            thread::sleep(Duration::from_millis(500));
            store.register(&lock, new_run()).unwrap();

            let followed = finished
                .recv_timeout(Duration::from_secs(5))
                .expect("the follow went on after its run was replaced");
            assert!(followed.is_ok(), "{followed:?}");
        });
    }

    #[test]
    fn cursors_name_a_run_and_an_offset_or_a_file_offset() {
        assert_eq!(
            Cursor::parse("AAAAAAAAAAAAAAAAAAAAAA:42").unwrap(),
            Cursor::Stream {
                run_id: RunId::parse("AAAAAAAAAAAAAAAAAAAAAA").unwrap(),
                offset: 42
            }
        );
        assert_eq!(Cursor::parse("17").unwrap(), Cursor::File(17));
        assert!(Cursor::parse("../x:1").is_err());
        assert!(Cursor::parse("abc").is_err());
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
