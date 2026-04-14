use std::path::{Path, PathBuf};

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
        /// Project name shown in the desktop app
        name: String,

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

    /// Start the Localify daemon
    Start {
        /// Run in foreground (don't daemonize)
        #[arg(long)]
        foreground: bool,
    },

    /// Stop the Roxy daemon
    Stop,

    /// Restart the Roxy daemon
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

/// Load config from file, or return defaults if the file doesn't exist.
/// For `install`, the config file may not exist yet, so defaults are fine.
fn load_config_and_paths(config_path: &Path) -> Result<(Config, RoxyPaths)> {
    let config_store = ConfigStore::new(config_path.to_path_buf());
    let config = config_store.load()?;
    let paths = config.paths.clone();
    Ok((config, paths))
}

fn main() -> Result<()> {
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
        } => cli::init_project::execute(
            cli::init_project::InitProjectArgs {
                name,
                domain,
                target,
                with_api,
                api_domain,
                api_target,
                project_only,
            },
            &ctx,
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
