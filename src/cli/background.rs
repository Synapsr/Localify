//! Services that run without a terminal: `dev --detach` starts one, `services
//! stop`, `services logs` and `services running` act on it. An agent has no
//! terminal to keep a dev server in, nor a Ctrl+C to stop it.

use std::ffi::OsString;
use std::fs::{self, File};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::Serialize;

use super::context::AppContext;
use super::services::resolve_service_key;
use crate::branding::BRANDING;
use crate::infrastructure::running_services::{
    self, DETACHED_LOG_ENV, RunningService, RunningServiceStore, ServiceKey,
};

/// A first `next dev` can take this long before it listens.
const START_TIMEOUT: Duration = Duration::from_secs(60);
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const FAILURE_OUTPUT_LINES: usize = 40;

/// Runs the same `dev` command again in the background, with its output in a
/// file, and returns once the service listens, stopped, or is still starting.
pub fn start_detached() -> Result<()> {
    let store = RunningServiceStore::new_default()?;
    let log_path = store.new_log_file()?;
    let log = File::create(&log_path)
        .with_context(|| format!("Failed to create '{}'", log_path.display()))?;
    let executable = std::env::current_exe().with_context(|| {
        format!(
            "Failed to resolve the {} executable path",
            BRANDING.cli_command()
        )
    })?;

    let mut command = Command::new(executable);
    command
        .args(foreground_args(std::env::args_os().skip(1)))
        .env(DETACHED_LOG_ENV, &log_path)
        .stdin(Stdio::null())
        .stdout(log.try_clone().context("Failed to share the output file")?)
        .stderr(log)
        // Its own process group: closing the terminal or the agent's shell leaves it running.
        .process_group(0);
    let mut child = command.spawn().context("Failed to start the service")?;
    wait_until_started(&mut child, &store, &log_path)
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

fn wait_until_started(
    child: &mut Child,
    store: &RunningServiceStore,
    log_path: &Path,
) -> Result<()> {
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        let service = service_run_by(store, child.id())?;
        if let Some(status) = child.try_wait().context("Failed to watch the service")? {
            let output = running_services::tail(log_path, FAILURE_OUTPUT_LINES).unwrap_or_default();
            if service.is_none() {
                let _ = fs::remove_file(log_path);
            }
            bail!("The service stopped while starting ({status}). Its last output:\n\n{output}");
        }
        if let Some(service) = &service
            && listens(service.port)
        {
            print_started(service, "Started");
            return Ok(());
        }
        if Instant::now() >= deadline {
            return match service {
                Some(service) => {
                    print_started(&service, "Still starting");
                    Ok(())
                }
                None => {
                    println!(
                        "{} is still preparing the service in the background (PID {}).\n  output: {}",
                        BRANDING.app_name,
                        child.id(),
                        log_path.display()
                    );
                    Ok(())
                }
            };
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn service_run_by(store: &RunningServiceStore, pid: u32) -> Result<Option<RunningService>> {
    Ok(store.list()?.into_iter().find(|service| service.pid == pid))
}

fn listens(port: u16) -> bool {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    TcpStream::connect_timeout(&address, Duration::from_millis(200)).is_ok()
}

fn print_started(service: &RunningService, state: &str) {
    let selector = service_selector(&service.key);
    println!(
        "{state} '{}' in the background (PID {}).",
        service.key.service_id, service.pid
    );
    println!("  url: {}", service.url);
    if let Some(log_file) = &service.log_file {
        println!("  output: {}", log_file.display());
    }
    println!(
        "  logs: {} services logs {selector}",
        BRANDING.cli_command()
    );
    println!(
        "  stop: {} services stop {selector}",
        BRANDING.cli_command()
    );
}

/// The arguments that name this service run exactly, from any folder.
fn service_selector(key: &ServiceKey) -> String {
    let mut selector = format!("{} --project {}", key.service_id, key.project_id);
    if let Some(environment) = &key.environment {
        selector.push_str(&format!(" --env {environment}"));
    }
    selector
}

pub fn stop(
    service: &str,
    project: Option<&str>,
    environment: Option<&str>,
    ctx: &AppContext,
) -> Result<()> {
    let key = resolve_service_key(service, project, environment, ctx)?;
    let store = RunningServiceStore::new_default()?;
    let Some(run) = store.get(&key)? else {
        println!("'{}' is not running.", key.service_id);
        return Ok(());
    };
    if running_services::stop(&run)? {
        println!("Stopped '{}' (PID {}).", key.service_id, run.pid);
    } else {
        println!("'{}' is not running.", key.service_id);
    }
    Ok(())
}

pub fn logs(
    service: &str,
    project: Option<&str>,
    environment: Option<&str>,
    lines: usize,
    ctx: &AppContext,
) -> Result<()> {
    let key = resolve_service_key(service, project, environment, ctx)?;
    let store = RunningServiceStore::new_default()?;
    let Some(run) = store.get(&key)? else {
        bail!(
            "'{}' has no recorded run. Start it in the background with: {} dev {} --detach",
            key.service_id,
            BRANDING.cli_command(),
            service_selector(&key)
        );
    };
    let Some(log_file) = &run.log_file else {
        bail!(
            "'{}' was started in a terminal, and its output is in that terminal. Only services started with --detach keep their output in a file.",
            key.service_id
        );
    };
    eprintln!(
        "'{}' {} (PID {}). Output: {}\n",
        key.service_id,
        if run.is_running() {
            "is running"
        } else {
            "has stopped"
        },
        run.pid,
        log_file.display()
    );
    println!("{}", running_services::tail(log_file, lines)?);
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RunningOutput<'a> {
    #[serde(flatten)]
    service: &'a RunningService,
    running: bool,
}

pub fn running(json: bool) -> Result<()> {
    let services = RunningServiceStore::new_default()?.list()?;
    let services = services
        .iter()
        .map(|service| RunningOutput {
            service,
            running: service.is_running(),
        })
        .filter(|output| output.running)
        .collect::<Vec<_>>();

    if json {
        println!("{}", serde_json::to_string_pretty(&services)?);
        return Ok(());
    }
    if services.is_empty() {
        println!("No service started by {} is running.", BRANDING.app_name);
        return Ok(());
    }
    for output in services {
        let service = output.service;
        println!("  {} (PID {})", service_selector(&service.key), service.pid);
        println!("    url: {}", service.url);
        if let Some(log_file) = &service.log_file {
            println!("    output: {}", log_file.display());
        }
    }
    Ok(())
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
    fn the_selector_names_project_and_environment() {
        let key = ServiceKey {
            project_id: "my-app".into(),
            environment: Some("feat-auth".into()),
            service_id: "web".into(),
        };

        assert_eq!(
            service_selector(&key),
            "web --project my-app --env feat-auth"
        );
    }
}
