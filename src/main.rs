use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::Result;
use branding::BRANDING;
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::Shell;

mod application;
mod branding;
mod cli;
mod config;
mod daemon;
mod domain;
mod infrastructure;

use cli::context::AppContext;
use infrastructure::config::{Config, ConfigStore};
use infrastructure::paths::RoxyPaths;

#[derive(Parser)]
#[command(name = "localify")]
#[command(about = "Local development proxy with custom .localify domains and HTTPS")]
#[command(version)]
struct Cli {
    /// Path to the config file
    #[arg(short, long, global = true, default_value_os_t = BRANDING.config_path())]
    config: PathBuf,

    /// Enable verbose output
    #[arg(short, long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initial setup - configures DNS and prepares Localify for use
    Install,

    /// Remove all Localify configuration from the system
    Uninstall {
        /// Skip confirmation prompt
        #[arg(long)]
        force: bool,
    },

    /// Register a new domain with routes
    Register {
        /// Domain name (must end with .localify)
        domain: String,

        /// Register wildcard subdomains for this domain (matches myapp.localify and *.myapp.localify)
        #[arg(long)]
        wildcard: bool,

        /// Route in format PATH=TARGET (e.g., "/=3000" or "/api=3001")
        /// TARGET can be: port (3000), host:port (192.168.1.50:3000), or path (/var/www)
        #[arg(long, short = 'r', value_name = "PATH=TARGET", required = true)]
        route: Vec<String>,
    },

    /// Create a project and bootstrap one or more local domains
    InitProject {
        /// Project name shown in the desktop app. Defaults to the current folder name.
        name: Option<String>,

        /// Primary domain for the project
        #[arg(long)]
        domain: Option<String>,

        /// Primary target: local port, host:port, URL, or directory path
        #[arg(long)]
        target: Option<String>,

        /// Also create an API domain for the project
        #[arg(long)]
        with_api: bool,

        /// API domain to create when --with-api is set
        #[arg(long)]
        api_domain: Option<String>,

        /// API target when --with-api is set
        #[arg(long)]
        api_target: Option<String>,

        /// Create only the project metadata without any domains
        #[arg(long)]
        project_only: bool,

        /// Accept the detected project bootstrap plan without interactive prompts
        #[arg(long)]
        yes: bool,
    },

    /// Unregister a domain
    Unregister {
        /// Domain name to unregister
        domain: String,

        /// Unregister wildcard subdomains for this domain (removes *.myapp.localify registration)
        #[arg(long)]
        wildcard: bool,

        /// Skip confirmation prompt
        #[arg(long)]
        force: bool,
    },

    /// Manage routes for a domain
    Route {
        #[command(subcommand)]
        command: RouteCommands,
    },

    /// List all registered domains
    List,

    /// List all configured projects and their domains
    ListProjects,

    /// Remove a project
    RemoveProject {
        /// Project ID to remove
        project_id: String,

        /// Also unregister all domains attached to the project
        #[arg(long)]
        delete_domains: bool,

        /// Skip the preview and execute immediately
        #[arg(long)]
        force: bool,
    },

    /// Manage project-scoped local development setup
    Project {
        #[command(subcommand)]
        command: ProjectCommands,
    },

    /// Manage coding-agent integration
    Agent {
        #[command(subcommand)]
        command: AgentCommands,
    },

    /// Start the Localify daemon
    Start {
        /// Run in foreground (don't daemonize)
        #[arg(long)]
        foreground: bool,
    },

    /// Stop the Localify daemon
    Stop,

    /// Restart the Localify daemon
    Restart,

    /// Show daemon and domain status
    Status,

    /// View daemon logs
    Logs {
        /// Number of lines to show
        #[arg(short = 'n', long, default_value = "50")]
        lines: usize,

        /// Clear all logs
        #[arg(long)]
        clear: bool,

        /// Follow log output (like tail -f)
        #[arg(short = 'f', long)]
        follow: bool,
    },

    /// Manage the stored Keygen license
    License {
        #[command(subcommand)]
        command: LicenseCommands,
    },

    /// Reload daemon configuration
    Reload,

    /// Generate shell completions
    Completions {
        /// Shell to generate completions for
        shell: Shell,
    },

    /// Internal privileged helper used by the desktop app.
    #[command(hide = true)]
    Helper {
        #[command(subcommand)]
        command: HelperCommands,
    },
}

#[derive(Subcommand)]
enum HelperCommands {
    EnsureRunning {
        #[arg(long)]
        socket: PathBuf,

        #[arg(long)]
        uid: u32,
    },

    Serve {
        #[arg(long)]
        socket: PathBuf,

        #[arg(long)]
        uid: u32,
    },

    UninstallService {
        #[arg(long)]
        uid: u32,
    },
}

#[derive(Subcommand)]
enum RouteCommands {
    /// Add a route to an existing domain
    Add {
        /// Manage wildcard registration for this domain (matches myapp.localify and *.myapp.localify)
        #[arg(long)]
        wildcard: bool,

        /// Domain name
        domain: String,

        /// Path prefix (e.g., "/api")
        path: String,

        /// Target: port, host:port, or filesystem path
        target: String,
    },

    /// Remove a route from a domain
    Remove {
        /// Manage wildcard registration for this domain (matches myapp.localify and *.myapp.localify)
        #[arg(long)]
        wildcard: bool,

        /// Domain name
        domain: String,

        /// Path prefix to remove
        path: String,
    },

    /// List routes for a domain
    List {
        /// Manage wildcard registration for this domain (matches myapp.localify and *.myapp.localify)
        #[arg(long)]
        wildcard: bool,

        /// Domain name
        domain: String,
    },
}

#[derive(Subcommand)]
enum ProjectCommands {
    /// Reapply local dev scripts and framework config for the current project
    ConfigureDev {
        /// Project ID when the current folder is ambiguous
        #[arg(long)]
        project: Option<String>,
    },

    /// Add a missing app/service to an existing project
    AddService {
        /// App folder relative to the current project root
        #[arg(long)]
        path: PathBuf,

        /// Project ID when the current folder is ambiguous
        #[arg(long)]
        project: Option<String>,

        /// Local domain for this service. Defaults to the detected app label and project name.
        #[arg(long)]
        domain: Option<String>,

        /// Local target. Defaults to an automatically selected free port.
        #[arg(long)]
        target: Option<String>,
    },
}

#[derive(Subcommand)]
enum AgentCommands {
    /// Install the bundled Localify skill for detected coding agents
    InstallSkill,

    /// Show Localify skill installation status
    SkillStatus,
}

#[derive(Subcommand)]
enum LicenseCommands {
    /// Show the current licensing status and limits
    Status,

    /// Activate a license key for this machine
    Activate {
        /// Keygen license key
        key: String,
    },

    /// Revalidate the stored license for the current machine
    Refresh,

    /// Import a Keygen offline license certificate from disk
    ImportOffline {
        /// Path to the certificate file
        path: PathBuf,

        /// License key used to decrypt the file (defaults to the stored key)
        #[arg(long)]
        key: Option<String>,
    },

    /// Export the cached offline license certificate to disk
    ExportOffline {
        /// Destination path for the certificate file
        path: PathBuf,
    },

    /// Remove the stored license from local config
    Clear,
}

/// Load config from file, or return defaults if the file doesn't exist.
/// For `install`, the config file may not exist yet, so defaults are fine.
fn load_config_and_paths(config_path: &Path) -> Result<(Config, RoxyPaths)> {
    let config_store = ConfigStore::new(config_path.to_path_buf());
    let config = config_store.load()?;
    let paths = config.paths.clone();
    Ok((config, paths))
}

fn ensure_rustls_crypto_provider() -> Result<()> {
    static INIT_RESULT: OnceLock<Result<(), String>> = OnceLock::new();

    let result = INIT_RESULT.get_or_init(|| {
        rustls::crypto::aws_lc_rs::default_provider()
            .install_default()
            .map_err(|_| {
                "Failed to initialize the rustls crypto provider. \
                 Localify requires the aws-lc-rs provider before starting TLS."
                    .to_string()
            })
    });

    match result {
        Ok(()) => Ok(()),
        Err(message) => Err(anyhow::anyhow!(message.clone())),
    }
}

fn main() -> Result<()> {
    ensure_rustls_crypto_provider()?;

    let cli = Cli::parse();
    let config_path = &cli.config;

    // Handle completions before loading config (works even with malformed config)
    if let Commands::Completions { shell } = &cli.command {
        clap_complete::generate(
            *shell,
            &mut Cli::command(),
            BRANDING.cli_command(),
            &mut std::io::stdout(),
        );
        return Ok(());
    }

    let (config, paths) = load_config_and_paths(config_path)?;
    let ctx = AppContext::new(config_path, &paths);

    match cli.command {
        Commands::Install => cli::install::execute(&ctx, &paths, &config),
        Commands::Uninstall { force } => cli::uninstall::execute(force, &ctx, &paths),
        Commands::Register {
            domain,
            wildcard,
            route,
        } => cli::register::execute(domain, wildcard, route, &ctx),
        Commands::InitProject {
            name,
            domain,
            target,
            with_api,
            api_domain,
            api_target,
            project_only,
            yes,
        } => cli::init_project::execute(
            cli::init_project::InitProjectArgs {
                name,
                domain,
                target,
                with_api,
                api_domain,
                api_target,
                project_only,
                yes,
            },
            config_path,
            roxy::project_init::LocalAppUpdatePolicy {
                promote_dev_script: config.agent.promote_dev_script,
            },
        ),
        Commands::Unregister {
            domain,
            wildcard,
            force,
        } => cli::unregister::execute(domain, wildcard, force, &ctx),
        Commands::Route { command } => match command {
            RouteCommands::Add {
                wildcard,
                domain,
                path,
                target,
            } => cli::route::add(domain, wildcard, path, target, &ctx),
            RouteCommands::Remove {
                wildcard,
                domain,
                path,
            } => cli::route::remove(domain, wildcard, path, &ctx),
            RouteCommands::List { wildcard, domain } => cli::route::list(domain, wildcard, &ctx),
        },
        Commands::List => cli::list::execute(&ctx),
        Commands::ListProjects => cli::list_projects::execute(&ctx),
        Commands::RemoveProject {
            project_id,
            delete_domains,
            force,
        } => cli::remove_project::execute(project_id, delete_domains, force, &ctx),
        Commands::Project { command } => {
            let policy = roxy::project_init::LocalAppUpdatePolicy {
                promote_dev_script: config.agent.promote_dev_script,
            };
            match command {
                ProjectCommands::ConfigureDev { project } => cli::project::configure_dev(
                    cli::project::ConfigureDevArgs { project },
                    config_path,
                    policy,
                ),
                ProjectCommands::AddService {
                    path,
                    project,
                    domain,
                    target,
                } => cli::project::add_service(
                    cli::project::AddServiceArgs {
                        path,
                        project,
                        domain,
                        target,
                    },
                    config_path,
                    policy,
                ),
            }
        }
        Commands::Agent { command } => match command {
            AgentCommands::InstallSkill => {
                let result = roxy::agent_skill::install_agent_skill()?;
                println!("Installed Localify skill for:");
                for agent in result.installed_agents {
                    println!("  {agent}");
                }
                Ok(())
            }
            AgentCommands::SkillStatus => {
                let status = roxy::agent_skill::agent_skill_status()?;
                println!("Bundled skill: {}", status.skill_dir.display());
                println!(
                    "npx available: {}",
                    if status.npx_available { "yes" } else { "no" }
                );
                println!("Detected agents:");
                for agent in status.detected_agents {
                    if agent.detected {
                        println!("  {} ({})", agent.label, agent.id);
                    }
                }
                println!("Install command:");
                println!("  {}", status.install_command);
                Ok(())
            }
        },
        Commands::Start { foreground } => {
            cli::start::execute(foreground, cli.verbose, config_path, &paths, &config.daemon)
        }
        Commands::Stop => cli::stop::execute(&ctx),
        Commands::Restart => cli::restart::execute(cli.verbose, config_path, &ctx),
        Commands::Status => cli::status::execute(&ctx, &config.daemon),
        Commands::Logs {
            lines,
            clear,
            follow,
        } => cli::logs::execute(lines, clear, follow, &paths),
        Commands::License { command } => match command {
            LicenseCommands::Status => cli::license::status(&ctx),
            LicenseCommands::Activate { key } => cli::license::activate(key, &ctx),
            LicenseCommands::Refresh => cli::license::refresh(&ctx),
            LicenseCommands::ImportOffline { path, key } => {
                cli::license::import_offline(path, key, &ctx)
            }
            LicenseCommands::ExportOffline { path } => cli::license::export_offline(path, &ctx),
            LicenseCommands::Clear => cli::license::clear(&ctx),
        },
        Commands::Reload => cli::reload::execute(cli.verbose, config_path, &ctx),
        Commands::Helper { command } => match command {
            HelperCommands::EnsureRunning { socket, uid } => {
                cli::helper::ensure_running(config_path, socket, uid)
            }
            HelperCommands::Serve { socket, uid } => {
                cli::helper::serve_forever(config_path, uid, socket)
            }
            HelperCommands::UninstallService { uid } => cli::helper::uninstall_service(uid),
        },
        Commands::Completions { .. } => unreachable!(),
    }
}
