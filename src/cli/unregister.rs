use anyhow::Result;
use localify::desktop::dto::DomainPatternRequest;

use super::backend::WriteBackend;
use super::context::AppContext;
use crate::application::StepOutcome;
use crate::application::unregister_domain::UnregisterDomain;
use crate::domain::DomainPattern;

pub fn execute(domain: String, wildcard: bool, force: bool, ctx: &AppContext) -> Result<()> {
    let pattern = DomainPattern::from_name(&domain, wildcard)?;

    let use_case = UnregisterDomain::new(&ctx.config_store, &ctx.cert_service);

    if !force {
        let registration = use_case.preview(&pattern)?;
        println!("This will unregister the domain:");
        println!("  Domain: {}", registration.display_pattern());
        println!("  Routes:");
        for route in registration.routes() {
            println!("    {} -> {}", route.path(), route.target());
        }
        if registration.is_https_enabled() {
            println!("  HTTPS certificate files will be removed");
        }
        println!("\nRun with --force to confirm.");
        return Ok(());
    }

    if let WriteBackend::Helper(client) = WriteBackend::resolve(&ctx.config_path)? {
        let result = client.unregister_domain(DomainPatternRequest { domain, wildcard })?;
        if result.cert_outcome.status == "warning" {
            eprintln!("{}", result.cert_outcome.message);
        } else if result.cert_outcome.status == "success" {
            println!("{}", result.cert_outcome.message);
        }
        println!("Unregistered domain: {}", result.registration.pattern);
        return Ok(());
    }

    let result = use_case.execute(&pattern)?;

    match &result.cert_outcome {
        StepOutcome::Success(msg) => println!("{}", msg),
        StepOutcome::Warning(msg) => eprintln!("{}", msg),
        StepOutcome::Skipped(_) => {}
    }

    println!(
        "Unregistered domain: {}",
        result.registration.display_pattern()
    );

    Ok(())
}
