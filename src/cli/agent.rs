//! `localify agent`: connects coding agents to the `localify mcp` server.

use std::path::PathBuf;

use anyhow::{Context, Result};
use localify::agent_mcp::{self, AgentMcpStatus};

use crate::branding::BRANDING;

pub fn install() -> Result<()> {
    let result = agent_mcp::install_agent_mcp(&current_binary()?)?;
    let labels = result
        .installed_agents
        .iter()
        .map(|agent| agent.label())
        .collect::<Vec<_>>();
    println!(
        "Connected the {} MCP server to {}.",
        BRANDING.app_name,
        labels.join(", ")
    );
    for path in &result.removed_skills {
        println!("Removed the old Localify skill: {}", path.display());
    }
    println!("Restart running agent sessions so they load it.");
    Ok(())
}

pub fn status() -> Result<()> {
    let status = agent_mcp::agent_mcp_status(&current_binary()?)?;
    print_status(&status);
    Ok(())
}

fn print_status(status: &AgentMcpStatus) {
    println!(
        "MCP server '{}': {} mcp\n",
        status.server.name,
        status.server.binary.display()
    );
    for agent in &status.agents {
        let state = match (agent.detected, agent.installed) {
            (_, true) => "connected",
            (true, false) => "not connected",
            (false, false) => "not installed",
        };
        println!("  {}: {state}", agent.agent.label());
        if agent.detected && !agent.installed {
            println!("    {}", agent.manual_command);
        }
    }
    if status
        .agents
        .iter()
        .any(|agent| agent.detected && !agent.installed)
    {
        println!(
            "\nConnect them all with: {} agent install",
            BRANDING.cli_command()
        );
    }
}

fn current_binary() -> Result<PathBuf> {
    std::env::current_exe().with_context(|| {
        format!(
            "Failed to resolve the {} executable path",
            BRANDING.cli_command()
        )
    })
}
