use anyhow::{Result, anyhow, bail};
use roxy::desktop::dto::RemoveProjectRequest;

use super::backend::WriteBackend;
use super::context::AppContext;
use crate::application::ports::DaemonConnection;
use crate::application::unregister_domain::UnregisterDomain;
use crate::branding::BRANDING;

pub fn execute(
    project_id: String,
    delete_domains: bool,
    force: bool,
    ctx: &AppContext,
) -> Result<()> {
    let project = ctx
        .config_store
        .list_projects()?
        .into_iter()
        .find(|project| project.id == project_id)
        .ok_or_else(|| anyhow!("Project '{}' not found.", project_id))?;

    let attached_domains = ctx
        .config_store
        .list_configured_domains()?
        .into_iter()
        .filter(|domain| domain.project_id.as_deref() == Some(project.id.as_str()))
        .collect::<Vec<_>>();

    if !force {
        println!("This will remove the project:");
        println!("  Project: {} [{}]", project.name, project.id);
        println!(
            "  Status: {}",
            if project.enabled {
                "enabled"
            } else {
                "disabled"
            }
        );

        if attached_domains.is_empty() {
            println!("  Attached domains: none");
        } else {
            println!("  Attached domains:");
            for domain in &attached_domains {
                println!("    {}", domain.registration.display_pattern());
            }
        }

        if !attached_domains.is_empty() && !delete_domains {
            println!(
                "\nThis project still has domains. Reassign or remove them first, or rerun with --delete-domains --force."
            );
            return Ok(());
        }

        println!("\nRun with --force to confirm.");
        return Ok(());
    }

    if !attached_domains.is_empty() && !delete_domains {
        bail!(
            "Project '{}' still has domains. Use --delete-domains --force, or reassign them first.",
            project.id
        );
    }

    if let WriteBackend::Helper(client) = WriteBackend::resolve(&ctx.config_path)? {
        let result = client.remove_project(RemoveProjectRequest {
            id: project.id.clone(),
            delete_domains,
        })?;
        if result.daemon_reloaded {
            println!("Reloaded the running daemon.");
        }
        println!("Removed project: {} [{}]", project.name, project.id);
        if !result.removed_domains.is_empty() {
            println!(
                "Removed {} attached domain(s).",
                result.removed_domains.len()
            );
        }
        println!(
            "Create a new one with: {} init-project my-app",
            BRANDING.cli_command()
        );
        return Ok(());
    }

    if delete_domains {
        let unregister_domain = UnregisterDomain::new(&ctx.config_store, &ctx.cert_service);
        for domain in &attached_domains {
            unregister_domain.execute(domain.registration.pattern())?;
        }
    }

    ctx.config_store.remove_project(&project.id)?;

    match ctx.mgmt_client.reload() {
        Ok(()) => println!("Reloaded the running daemon."),
        Err(crate::application::ports::DaemonConnectionError::NotRunning) => {}
        Err(error) => eprintln!("Warning: failed to reload daemon: {error}"),
    }

    println!("Removed project: {} [{}]", project.name, project.id);

    if delete_domains && !attached_domains.is_empty() {
        println!("Removed {} attached domain(s).", attached_domains.len());
    } else if !attached_domains.is_empty() {
        println!(
            "Project could not be removed while domains were still attached. Use --delete-domains."
        );
    }

    println!(
        "Create a new one with: {} init-project my-app",
        BRANDING.cli_command()
    );

    Ok(())
}
