use crate::branding::BRANDING;
use anyhow::Result;

use super::context::{AppContext, require_config_writable};
use crate::application::StepOutcome;
use crate::application::register_domain::RegisterDomain;
use crate::domain::{DomainPattern, Route};

pub fn execute(
    domain: String,
    wildcard: bool,
    routes: Vec<String>,
    ctx: &AppContext,
) -> Result<()> {
    let pattern = DomainPattern::from_name(&domain, wildcard)?;

    require_config_writable(&ctx.config_path)?;

    let parsed_routes: Vec<Route> = routes
        .iter()
        .map(|s| Route::parse(s))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| anyhow::anyhow!("Invalid route: {}", e))?;

    let use_case = RegisterDomain::new(&ctx.config_store, &ctx.cert_service);

    println!(
        "Generating SSL certificate for {}...",
        pattern.display_pattern()
    );

    let result = use_case.execute(pattern, parsed_routes)?;

    match &result.cert_outcome {
        StepOutcome::Success(msg) => println!("  {}", msg),
        StepOutcome::Warning(msg) => {
            eprintln!("  {}", msg);
            eprintln!(
                "  Run '{}' to enable HTTPS, then register {}{} again.",
                BRANDING.admin_command("install"),
                result.registration.domain(),
                if wildcard { " --wildcard" } else { "" }
            );
        }
        StepOutcome::Skipped(msg) | StepOutcome::Pending(msg) => println!("  {}", msg),
    }

    println!(
        "\nRegistered domain: {}",
        result.registration.display_pattern()
    );
    println!("  Routes:");
    for route in result.registration.routes() {
        println!("    {} -> {}", route.path(), route.target());
    }
    println!(
        "  HTTPS: {}",
        if result.registration.is_https_enabled() {
            "enabled"
        } else {
            "disabled"
        }
    );
    println!("\nStart the proxy with: localify start");

    Ok(())
}
