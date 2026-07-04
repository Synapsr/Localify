use anyhow::Result;

use super::backend::WriteBackend;
use super::context::AppContext;
use crate::application::StepOutcome;
use crate::application::install::Install;
use crate::infrastructure::config::Config;
use crate::infrastructure::dns::get_dns_service;
use crate::infrastructure::filesystem::FileSystemSetup;
use crate::infrastructure::network::get_network_info;
use crate::infrastructure::paths::RoxyPaths;

pub fn execute(ctx: &AppContext, paths: &RoxyPaths, config: &Config) -> Result<()> {
    println!("Setting up Localify...\n");

    if let WriteBackend::Helper(client) = WriteBackend::resolve(&ctx.config_path)? {
        let result = client.install_roxy()?;
        println!("  Using IP address: {}", result.lan_ip);
        for step in &result.steps {
            let label = step.label.as_deref().unwrap_or("");
            if step.status == "warning" {
                eprintln!("  {} {}", label, step.message);
            } else {
                println!("  {} {}", label, step.message);
            }
        }
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
