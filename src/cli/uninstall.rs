use anyhow::Result;

use super::context::{AppContext, direct_api, require_config_writable};
use crate::application::StepOutcome;
use crate::application::uninstall::Uninstall;
use crate::infrastructure::dns::get_dns_service;
use crate::infrastructure::filesystem::FileSystemSetup;
use crate::infrastructure::paths::LocalifyPaths;

pub fn execute(force: bool, ctx: &AppContext, paths: &LocalifyPaths) -> Result<()> {
    let dns_service = get_dns_service()?;
    let system = FileSystemSetup::new(paths);

    let use_case = Uninstall::new(
        &ctx.config_store,
        &ctx.cert_service,
        &ctx.daemon,
        &dns_service,
        &system,
        paths.data_dir.display().to_string(),
    );

    if !force {
        let preview = use_case.preview()?;
        println!("This will remove all Localify configuration including:");
        println!("  - Stop the running daemon");
        println!("  - DNS configuration for *.localify domains");
        println!("  - All registered domains ({})", preview.domain_count);
        println!("  - All SSL certificates from system trust store");
        if cfg!(target_os = "macos") {
            println!("  - The background service, and the terminal command it installed");
        }
        println!("  - All data in {}/", preview.data_dir);
        println!("\nRun with --force to confirm, or press Ctrl+C to cancel.");
        return Ok(());
    }

    if cfg!(target_os = "macos") {
        println!("Uninstalling Localify...\n");
        let result = direct_api(&ctx.config_path)?.uninstall()?;
        for step in &result.steps {
            let label = step.label.as_deref().unwrap_or("");
            if step.status == "warning" {
                eprintln!("  {}: {}", label, step.message);
            } else {
                println!("  {}: {}", label, step.message);
            }
        }
        println!("\nLocalify uninstallation complete!");
        println!("All configuration and certificates have been removed.");
        return Ok(());
    }

    require_config_writable(&ctx.config_path)?;

    println!("Uninstalling Localify...\n");

    let result = use_case.execute()?;

    for (label, outcome) in &result.steps {
        match outcome {
            StepOutcome::Success(msg) => println!("  {}: {}", label, msg),
            StepOutcome::Warning(msg) => eprintln!("  {}: {}", label, msg),
            StepOutcome::Skipped(msg) | StepOutcome::Pending(msg) => {
                println!("  {}: {}", label, msg)
            }
        }
    }

    println!("\nLocalify uninstallation complete!");
    println!("All configuration and certificates have been removed.");

    Ok(())
}
