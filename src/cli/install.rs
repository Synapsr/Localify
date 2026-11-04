use anyhow::Result;

use super::context::{AppContext, direct_api};
use crate::application::StepOutcome;
use crate::application::install::Install;
use crate::branding::BRANDING;
use crate::infrastructure::config::Config;
use crate::infrastructure::dns::get_dns_service;
use crate::infrastructure::filesystem::FileSystemSetup;
use crate::infrastructure::network::get_network_info;
use crate::infrastructure::paths::LocalifyPaths;

pub fn execute(ctx: &AppContext, paths: &LocalifyPaths, config: &Config) -> Result<()> {
    println!("Setting up Localify...\n");

    if BRANDING.installs_with_one_approval() {
        return install_with_one_approval(ctx, paths);
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
            StepOutcome::Skipped(msg) | StepOutcome::Pending(msg) => {
                println!("  {} {}", label, msg)
            }
        }
    }

    println!("\nLocalify installation complete!");
    println!();
    println!("Register domains with: localify register <domain> --route \"/=3000\"");

    Ok(())
}

/// The shared API does the work, in one approval, as this user. Node trust
/// and iOS Simulators are set up here too: over SSH they may be all that works.
fn install_with_one_approval(ctx: &AppContext, paths: &LocalifyPaths) -> Result<()> {
    let result = direct_api(&ctx.config_path)?.install()?;

    println!("  Using IP address: {}", result.lan_ip);
    let mut pending = false;
    for step in &result.steps {
        let label = step.label.as_deref().unwrap_or("");
        match step.status.as_str() {
            "warning" => eprintln!("  {} {}", label, step.message),
            "pending" => {
                pending = true;
                eprintln!("  {} {}", label, step.message);
            }
            _ => println!("  {} {}", label, step.message),
        }
    }

    println!("\nLocalify installation complete!");
    println!("  Data and logs: {}", paths.data_dir.display());
    if pending {
        println!("  HTTP works now. HTTPS needs the step above to be trusted by browsers.");
    }
    println!();
    println!("Register domains with: localify register <domain> --route \"/=3000\"");
    Ok(())
}
