//! `localify login`, `logout` and `whoami`: this machine's Localify Cloud
//! account (ADR 0020).

use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};

use super::output::{can_prompt, note, print_json, wants_json};
use crate::application::cloud_account::{AccountStatus, SignInPoll, TunnelRefresh};
use crate::application::ports::{DeviceAuthorization, TunnelState};
use crate::branding::BRANDING;
use crate::infrastructure::cloud_adapters::{CloudAdapters, computer_name};

/// Signs in with GitHub's device flow: shows the code, opens the browser for
/// a person at a terminal, and waits until the code is confirmed.
pub fn login(json_flag: bool, cloud: &CloudAdapters) -> Result<()> {
    let json = wants_json(json_flag);
    let accounts = cloud.accounts();
    let device = accounts.start_sign_in()?;
    print_code(&device, json);
    if can_prompt() {
        open_in_browser(&device.verification_uri);
    }

    let machine_name = computer_name();
    let deadline = Instant::now() + Duration::from_secs(device.expires_in);
    let mut interval = device.interval.max(1);
    let cli = BRANDING.cli_command();
    loop {
        thread::sleep(Duration::from_secs(interval));
        match accounts.poll_sign_in(&device.device_code, &machine_name)? {
            SignInPoll::Pending => {}
            // GitHub's rule for a missing interval: five seconds more.
            SignInPoll::SlowDown { interval: next } => interval = next.unwrap_or(interval + 5),
            SignInPoll::SignedIn { status, refresh } => {
                return print_signed_in(&status, &refresh, json);
            }
            SignInPoll::Denied(message) => {
                bail!("{message} To sign in after all, run `{cli} login` again.")
            }
            SignInPoll::NotAllowed(message) => bail!(
                "{message} Localify Cloud is invite-only for now: ask the Localify team to add this GitHub account."
            ),
            SignInPoll::Expired(message) => {
                bail!("{message} Run `{cli} login` again for a new code.")
            }
        }
        if Instant::now() >= deadline {
            bail!(
                "The code expired before it was confirmed. Run `{cli} login` again for a new code."
            );
        }
    }
}

pub fn logout(json_flag: bool, cloud: &CloudAdapters) -> Result<()> {
    let json = wants_json(json_flag);
    let accounts = cloud.accounts();
    let signed_out = accounts.sign_out()?;
    if let Some(warning) = &signed_out.warning {
        eprintln!("Warning: {warning}");
    }
    if let TunnelRefresh::Failed(reason) = &signed_out.refresh {
        eprintln!(
            "Warning: the daemon did not stop the tunnel ({reason}). Restart it with: {}",
            BRANDING.admin_command("restart")
        );
    }
    if json {
        return print_json(&accounts.status()?);
    }
    match signed_out.login {
        Some(login) => println!(
            "Signed out {login}. This machine's public URLs are offline until a machine claims them."
        ),
        None => println!("This machine was not signed in to Localify Cloud."),
    }
    Ok(())
}

pub fn whoami(json_flag: bool, cloud: &CloudAdapters) -> Result<()> {
    let status = cloud.accounts().status()?;
    if wants_json(json_flag) {
        return print_json(&status);
    }
    let (Some(login), Some(machine)) = (&status.login, &status.machine_name) else {
        println!(
            "Not signed in to Localify Cloud. Sign in with: {} login",
            BRANDING.cli_command()
        );
        return Ok(());
    };
    println!("Signed in to Localify Cloud as {login}, on the machine {machine}.");
    println!("{}", tunnel_line(&status));
    Ok(())
}

/// After a change: what the daemon did with it, when that needs saying.
pub fn print_refresh(refresh: &TunnelRefresh, json: bool) {
    match refresh {
        TunnelRefresh::Done => {}
        TunnelRefresh::DaemonStopped => note(
            json,
            &format!(
                "The daemon is not running: the tunnel starts with it. Start it with: {}",
                BRANDING.admin_command("start")
            ),
        ),
        TunnelRefresh::Failed(reason) => eprintln!(
            "Warning: the daemon did not take the change ({reason}). It checks again within 5 minutes, or restart it: {}",
            BRANDING.admin_command("restart")
        ),
    }
}

/// The code a person needs, even when stdout is for JSON.
fn print_code(device: &DeviceAuthorization, json: bool) {
    let minutes = device.expires_in.div_ceil(60);
    note(
        json,
        &format!(
            "To sign in, open {} and enter the code:\n\n    {}\n\nWaiting for GitHub (the code expires in {minutes} min)...",
            device.verification_uri, device.user_code
        ),
    );
}

fn print_signed_in(status: &AccountStatus, refresh: &TunnelRefresh, json: bool) -> Result<()> {
    if json {
        print_json(status)?;
    } else {
        println!(
            "Signed in as {}, on the machine {}.",
            status.login.as_deref().unwrap_or_default(),
            status.machine_name.as_deref().unwrap_or_default()
        );
        println!(
            "Give a project domain a public URL with: {} public-url create <local-domain>",
            BRANDING.cli_command()
        );
    }
    print_refresh(refresh, json);
    Ok(())
}

fn tunnel_line(status: &AccountStatus) -> String {
    let tunnel = &status.tunnel;
    match tunnel.state {
        TunnelState::Connected => format!(
            "Tunnel: connected to Cloudflare ({} connections).",
            tunnel.connections
        ),
        TunnelState::Starting => "Tunnel: starting.".to_string(),
        TunnelState::Stopped => {
            "Tunnel: stopped. It runs while this machine serves a public URL.".to_string()
        }
        TunnelState::Error => format!(
            "Tunnel: failed. {}",
            tunnel.error.as_deref().unwrap_or("See the daemon logs.")
        ),
        TunnelState::Unknown => match &tunnel.error {
            Some(error) => format!("Tunnel: unknown. {error}"),
            None => format!(
                "Tunnel: unknown, the daemon is not running. Start it with: {}",
                BRANDING.admin_command("start")
            ),
        },
    }
}

/// Best effort: the code and URL are printed anyway, for a machine reached
/// over SSH or without a browser.
fn open_in_browser(url: &str) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = Command::new(opener)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}
