use anyhow::Result;
use std::time::Duration;

use super::context::AppContext;
use crate::application::daemon_status::QueryDaemonStatus;
use crate::application::list_all_domains::ListAllDomains;
use crate::branding::BRANDING;
use crate::config::DaemonConfig;
use crate::domain::RegistrationSource;
use crate::infrastructure::dns::{SystemResolutionCheck, check_system_resolution};
use crate::infrastructure::macos_system::LegacyInstall;
use crate::infrastructure::network::get_network_info;

pub fn execute(ctx: &AppContext, daemon_config: &DaemonConfig) -> Result<()> {
    let network_info = get_network_info();

    let service = QueryDaemonStatus::new(&ctx.daemon, &ctx.cert_service, &network_info);
    let status = service.execute()?;

    let ca_label = if status.ca_installed {
        "trusted".to_string()
    } else if status.ca_trust_pending {
        format!(
            "not trusted yet. HTTP and Node.js work; browsers will warn about HTTPS.\n    Trust it with: {}",
            trust_command()
        )
    } else {
        format!("not installed. Run: {}", BRANDING.admin_command("install"))
    };

    if BRANDING.uses_launchd_service() && LegacyInstall::needs_migration() {
        println!(
            "An older {0} install is still on this Mac and runs as root.\nRun '{1}' once to replace it. It asks for one approval.\n",
            BRANDING.app_name,
            BRANDING.admin_command("install")
        );
    }

    match status.pid {
        Some(pid) => {
            println!("Localify daemon: running (PID: {})", pid);
            println!("  Root CA: {}", ca_label);
            println!("  HTTP:  http://localhost:{}", daemon_config.http_port);
            println!("  HTTPS: https://localhost:{}", daemon_config.https_port);
            print_dns_resolution_status(check_system_resolution(Duration::from_millis(1500)));
        }
        None => {
            println!("Localify daemon: stopped");
            println!("  Root CA: {}", ca_label);
            println!("\nStart with: {}", BRANDING.admin_command("start"));
        }
    }

    println!();
    super::lan::print_status(daemon_config);

    // List domains via ListAllDomains (daemon + Docker, or config fallback)
    let list_svc = ListAllDomains::new(&ctx.mgmt_client, &ctx.config_store, &ctx.cert_service);
    let result = list_svc.execute()?;

    if !result.domains.is_empty() {
        println!("\nRegistered domains: {}", result.domains.len());
        for info in result.domains {
            let scheme = if info.registration.is_https_enabled() {
                "https"
            } else {
                "http"
            };
            let source_label = if info.registration.source() == RegistrationSource::External {
                " [external]"
            } else {
                ""
            };
            println!(
                "  {}://{}{}",
                scheme,
                info.registration.display_pattern(),
                source_label,
            );
        }
    }

    Ok(())
}

fn print_dns_resolution_status(check: SystemResolutionCheck) {
    if check.healthy {
        let ips = check
            .resolved_ips
            .iter()
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        println!("  DNS: {} resolves locally ({})", check.hostname, ips);
        return;
    }

    println!(
        "  DNS: system lookup for {} is not reaching Localify",
        check.hostname
    );
    if let Some(message) = check.message {
        println!("       {}", message);
    }
    println!(
        "       If a VPN is active, its DNS or leak-protection settings may be bypassing macOS /etc/resolver."
    );
}

fn trust_command() -> String {
    if cfg!(target_os = "macos") {
        BRANDING.admin_command("trust")
    } else {
        BRANDING.admin_command("install")
    }
}
