use anyhow::Result;
use roxy::desktop::dto::{RegisterDomainRequest, RegisterDomainResultDto, StepDto};

use super::backend::WriteBackend;
use super::context::AppContext;
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

    if let WriteBackend::Helper(client) = WriteBackend::resolve(&ctx.config_path)? {
        println!(
            "Generating SSL certificate for {}...",
            pattern.display_pattern()
        );
        let result = client.register_domain(RegisterDomainRequest {
            domain,
            wildcard,
            routes,
            project_id: None,
        })?;
        print_registration_result(&result, wildcard);
        return Ok(());
    }

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
                "  Run 'sudo localify register {}{}' to enable HTTPS.",
                result.registration.domain(),
                if wildcard { " --wildcard" } else { "" }
            );
        }
        StepOutcome::Skipped(msg) => println!("  {}", msg),
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

fn print_registration_result(result: &RegisterDomainResultDto, wildcard: bool) {
    print_step(&result.cert_outcome);
    if result.cert_outcome.status == "warning" {
        eprintln!(
            "  Run 'sudo localify register {}{}' to enable HTTPS.",
            result.registration.domain,
            if wildcard { " --wildcard" } else { "" }
        );
    }

    println!("\nRegistered domain: {}", result.registration.pattern);
    println!("  Routes:");
    for route in &result.registration.routes {
        println!("    {} -> {}", route.path, route.target);
    }
    println!(
        "  HTTPS: {}",
        if result.registration.https_enabled {
            "enabled"
        } else {
            "disabled"
        }
    );
    println!("\nStart the proxy with: localify start");
}

pub(crate) fn print_step(step: &StepDto) {
    if step.status == "warning" {
        eprintln!("  {}", step.message);
    } else {
        println!("  {}", step.message);
    }
}
