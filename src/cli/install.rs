use std::env;

use anyhow::Result;
use localify::desktop::DesktopApi;

use super::backend::WriteBackend;
use super::context::AppContext;
use crate::application::StepOutcome;
use crate::application::install::Install;
#[cfg(target_os = "macos")]
use crate::application::ports::IosSimulatorTrust;
use crate::branding::BRANDING;
use crate::infrastructure::config::Config;
use crate::infrastructure::dns::get_dns_service;
use crate::infrastructure::filesystem::FileSystemSetup;
#[cfg(target_os = "macos")]
use crate::infrastructure::ios_simulator_trust::trust_booted_ios_simulators;
use crate::infrastructure::network::get_network_info;
use crate::infrastructure::paths::LocalifyPaths;

pub fn execute(ctx: &AppContext, paths: &LocalifyPaths, config: &Config) -> Result<()> {
    println!("Setting up Localify...\n");

    if BRANDING.is_dev_profile() {
        return install_dev_profile(ctx);
    }

    if let WriteBackend::Helper(client) = WriteBackend::resolve(&ctx.config_path)? {
        let result = client.install_localify()?;
        println!("  Using IP address: {}", result.lan_ip);
        for step in &result.steps {
            let label = step.label.as_deref().unwrap_or("");
            if step.status == "warning" {
                eprintln!("  {} {}", label, step.message);
            } else {
                println!("  {} {}", label, step.message);
            }
        }
        print_ios_simulator_trust_step(paths);
        println!("\nLocalify installation complete!");
        println!();
        println!("Register domains with: localify register <domain> --route \"/=3000\"");
        return Ok(());
    }

    let dns_service = get_dns_service()?;
    let network_info = get_network_info();
    let system = FileSystemSetup::new(paths);

    let use_case = Install::new(
        &ctx.cert_service,
        &ctx.config_store,
        &dns_service,
        &network_info,
        &system,
        config.daemon.dns_port,
    );
    let result = use_case.execute()?;

    println!("  Using IP address: {}", result.lan_ip);
    if result.lan_ip.is_loopback() {
        println!("  Warning: No network detected, using localhost.");
    }

    for (label, outcome) in &result.steps {
        match outcome {
            StepOutcome::Success(msg) => println!("  {} {}", label, msg),
            StepOutcome::Warning(msg) => eprintln!("  {} {}", label, msg),
            StepOutcome::Skipped(msg) => println!("  {} {}", label, msg),
        }
    }

    println!("\nLocalify installation complete!");
    println!();
    println!("Register domains with: localify register <domain> --route \"/=3000\"");

    Ok(())
}

/// The dev profile runs as the user. The API asks for the administrator prompt
/// itself for the resolver file and the terminal command.
fn install_dev_profile(ctx: &AppContext) -> Result<()> {
    let result = DesktopApi::new(ctx.config_path.clone(), env::current_exe()?).install()?;
    println!("  Using IP address: {}", result.lan_ip);
    for step in &result.steps {
        let label = step.label.as_deref().unwrap_or("");
        if step.status == "warning" {
            eprintln!("  {} {}", label, step.message);
        } else {
            println!("  {} {}", label, step.message);
        }
    }
    println!("\nLocalify dev setup complete!");
    Ok(())
}

#[cfg(target_os = "macos")]
fn print_ios_simulator_trust_step(paths: &LocalifyPaths) {
    let Some(outcome) = ios_simulator_trust_outcome(&paths.data_dir.join("ca.crt")) else {
        return;
    };

    match outcome {
        StepOutcome::Success(message) | StepOutcome::Skipped(message) => {
            println!("  iOS Simulator trust {}", message);
        }
        StepOutcome::Warning(message) => {
            eprintln!("  iOS Simulator trust {}", message);
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn print_ios_simulator_trust_step(_paths: &LocalifyPaths) {}

#[cfg(target_os = "macos")]
fn ios_simulator_trust_outcome(cert_path: &std::path::Path) -> Option<StepOutcome> {
    match trust_booted_ios_simulators(cert_path) {
        Ok(IosSimulatorTrust::Trusted { device_names }) => Some(StepOutcome::Success(format!(
            "Root CA trusted in {}.",
            format_ios_simulator_names(&device_names)
        ))),
        Ok(IosSimulatorTrust::NoBootedDevices) => Some(StepOutcome::Skipped(
            "No booted iOS Simulator found. Boot one and rerun 'localify install' before testing .localify HTTPS there.".into(),
        )),
        Ok(IosSimulatorTrust::CertificateMissing { path }) => Some(StepOutcome::Warning(format!(
            "Root CA file was not found at {}; iOS Simulator HTTPS trust was not configured.",
            path.display()
        ))),
        Ok(IosSimulatorTrust::ToolUnavailable { reason }) => Some(StepOutcome::Skipped(format!(
            "Xcode simctl is not available; iOS Simulator HTTPS trust was not configured. {}",
            reason
        ))),
        Ok(IosSimulatorTrust::CurrentUserUnavailable) => None,
        Err(error) => Some(StepOutcome::Warning(format!(
            "Failed to trust the Root CA in booted iOS Simulators: {error}"
        ))),
    }
}

#[cfg(target_os = "macos")]
fn format_ios_simulator_names(device_names: &[String]) -> String {
    match device_names {
        [] => "booted iOS Simulator".into(),
        [name] => format!("booted iOS Simulator '{name}'"),
        names => format!(
            "{} booted iOS Simulators ({})",
            names.len(),
            names.join(", ")
        ),
    }
}
