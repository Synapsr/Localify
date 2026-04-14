use anyhow::Result;

use super::context::AppContext;

pub fn execute(ctx: &AppContext) -> Result<()> {
    let projects = ctx.config_store.list_projects()?;
    let configured_domains = ctx.config_store.list_configured_domains()?;

    if projects.is_empty() {
        println!("No projects configured.");
        println!("\nCreate one with:");
        println!(
            "  {} init-project my-app",
            crate::branding::BRANDING.cli_command()
        );
        return Ok(());
    }

    println!("Projects:\n");

    for project in projects {
        let domains = configured_domains
            .iter()
            .filter(|domain| domain.project_id.as_deref() == Some(project.id.as_str()))
            .collect::<Vec<_>>();

        println!(
            "  {} [{}] {}",
            project.name,
            project.id,
            if project.enabled {
                "(enabled)"
            } else {
                "(disabled)"
            }
        );

        if domains.is_empty() {
            println!("    No domains");
            println!();
            continue;
        }

        for domain in domains {
            println!("    {}", domain.registration.display_pattern());
            for route in domain.registration.routes() {
                println!("      {} -> {}", route.path(), route.target());
            }
        }
        println!();
    }

    let unassigned_domains = configured_domains
        .iter()
        .filter(|domain| domain.project_id.is_none())
        .collect::<Vec<_>>();

    if !unassigned_domains.is_empty() {
        println!("Unassigned domains:\n");
        for domain in unassigned_domains {
            println!("  {}", domain.registration.display_pattern());
        }
    }

    Ok(())
}
