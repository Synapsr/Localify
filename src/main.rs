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
use infrastructure::paths::LocalifyPaths;

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

/// A service for `stop`, `restart` and `logs`, which act on the daemon without one.
#[derive(clap::Args, Debug, Default)]
struct ServiceSelector {
    /// Service name or ID
    service: Option<String>,

    /// Project ID/name when service resolution is ambiguous
    #[arg(long, requires = "service")]
    project: Option<String>,

    /// Project environment (worktree) name or slug. Defaults to the current worktree's.
    #[arg(long, requires = "service")]
    env: Option<String>,
}

/// Launch profile options shared by `dev` and `run`.
#[derive(clap::Args, Debug, Default)]
struct LaunchProfileFlags {
    /// Launch profile from the project's localify.toml, or default/staging/prod for a project linked to Infisical without one. Changes only the service's variables: same service, domain and port. Defaults to the profile named `default`, if any.
    #[arg(short = 'p', long, value_name = "NAME")]
    profile: Option<String>,

    /// Extra dotenv file, loaded before the profile's sources (repeatable; relative to the current directory). `pass://` values are read from Proton Pass.
    #[arg(long = "env-file", value_name = "PATH")]
    env_file: Vec<PathBuf>,

    /// Required to start a `danger = true` profile when no terminal is attached (agents, scripts, CI). Must repeat the profile name.
    #[arg(long, value_name = "NAME")]
    confirm_profile: Option<String>,

    /// Print Proton Pass and Infisical values in the service output instead of masking them
    #[arg(long)]
    no_masking: bool,
}

impl LaunchProfileFlags {
    fn into_args(self) -> cli::launch_profile::LaunchProfileArgs {
        cli::launch_profile::LaunchProfileArgs {
            profile: self.profile,
            env_files: self.env_file,
            confirm_profile: self.confirm_profile,
            no_masking: self.no_masking,
        }
    }
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

        /// Create or update package.json dev-localify scripts during bootstrap
        #[arg(long)]
        write_scripts: bool,
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

    /// Manage project services
    Services {
        #[command(subcommand)]
        command: ServicesCommands,
    },

    /// Print the primary Localify URL for a service
    Get {
        /// Service name or ID
        service: String,

        /// Project ID/name when service resolution is ambiguous
        #[arg(long)]
        project: Option<String>,

        /// Project environment name/slug. Defaults to the primary environment.
        #[arg(long)]
        env: Option<String>,

        /// Print structured JSON
        #[arg(long)]
        json: bool,
    },

    /// Run a project service with Localify environment variables
    Run {
        /// Service name or ID. If omitted, Localify infers it from the current directory or prompts.
        service: Option<String>,

        /// Project ID/name when service resolution is ambiguous
        #[arg(long)]
        project: Option<String>,

        /// Project environment (a worktree or branch checkout with its own domains and ports). Defaults to the primary environment. To change only variables, use --profile.
        #[arg(long)]
        env: Option<String>,

        #[command(flatten)]
        launch_profile: LaunchProfileFlags,

        /// Temporary command override, passed after `--`
        #[arg(last = true, value_name = "COMMAND")]
        command: Vec<String>,
    },

    /// Start a local dev service through Localify without requiring package.json script rewrites
    Dev {
        /// Service name or ID. If omitted, Localify infers it from the current directory or prompts.
        service: Option<String>,

        /// Primary domain to pre-fill/use if dev initializes the project first
        #[arg(long)]
        domain: Option<String>,

        /// Project ID/name when service resolution is ambiguous
        #[arg(long)]
        project: Option<String>,

        /// Project environment (a worktree or branch checkout with its own domains and ports). Defaults to the primary environment. To change only variables, use --profile.
        #[arg(long)]
        env: Option<String>,

        #[command(flatten)]
        launch_profile: LaunchProfileFlags,

        /// Start the service in the desktop app's terminal, as its launch button does, and return once it listens. Read its output with `logs <service>`, stop it with `stop <service>`.
        #[arg(long, conflicts_with = "detach")]
        in_app: bool,

        /// Run in the background, without the app, and return once the service listens. Read its output with `logs <service>`, stop it with `stop <service>`.
        #[arg(long)]
        detach: bool,

        /// If the service is already running, stop it first, then start it again; with a run id, only if that run is the one running. Without it, a terminal asks, and anything else (an agent, a script) leaves it running.
        #[arg(long, num_args = 0..=1, require_equals = true, value_name = "RUN_ID")]
        replace: Option<Option<String>>,

        /// For dispatchers: the run id this launch gets, 16 random bytes in base64url (docs/remote-machines.md).
        #[arg(long, hide = true, value_name = "ID")]
        launch_id: Option<String>,

        /// Temporary command override, passed after `--`
        #[arg(last = true, value_name = "COMMAND")]
        command: Vec<String>,
    },

    /// Run a command that is not a service (database studio, migration, worker) with the variables of a launch profile, from the current folder
    Exec {
        #[command(flatten)]
        launch_profile: LaunchProfileFlags,

        /// Command to run, passed after `--`
        #[arg(last = true, value_name = "COMMAND", required = true)]
        command: Vec<String>,
    },

    /// Set up launch profiles (localify.toml) for the current project
    Profile {
        #[command(subcommand)]
        command: ProfileCommands,
    },

    /// Manage coding-agent integration
    Agent {
        #[command(subcommand)]
        command: AgentCommands,
    },

    /// Serve every Localify action to a coding agent over MCP (stdio). Agents start it themselves; see `agent install`.
    Mcp,

    /// Start the Localify daemon
    Start {
        /// Run in foreground (don't daemonize)
        #[arg(long)]
        foreground: bool,
    },

    /// Stop a service Localify started, or the Localify daemon when no service is given
    Stop {
        #[command(flatten)]
        service: ServiceSelector,
    },

    /// Restart a service Localify started where it ran, or the Localify daemon when no service is given
    Restart {
        #[command(flatten)]
        service: ServiceSelector,
    },

    /// Show daemon and domain status
    Status,

    /// This machine's inventory, as JSON, from the daemon (docs/remote-machines.md)
    #[command(hide = true)]
    Inventory {
        /// The revision already held, `<epoch>:<rev>`: an unchanged snapshot is not sent again
        #[arg(long, value_name = "EPOCH:REV")]
        have: Option<String>,
    },

    /// Print a service's output as the app's terminal shows it, or the daemon logs when no service is given
    Logs {
        #[command(flatten)]
        service: ServiceSelector,

        /// Number of lines to show
        #[arg(short = 'n', long, default_value = "50")]
        lines: usize,

        /// Print only the output after this cursor, as the previous `logs` call gave it
        #[arg(long, requires = "service", conflicts_with = "follow")]
        cursor: Option<String>,

        /// Clear the daemon logs
        #[arg(long, conflicts_with = "service")]
        clear: bool,

        /// Follow log output (like tail -f). For a service, until it stops.
        #[arg(short = 'f', long)]
        follow: bool,
    },

    /// Reload daemon configuration
    Reload,

    /// Trust HTTPS in browsers on this Mac, with one macOS confirmation
    Trust,

    /// Let other devices reach Localify, or keep it on this machine (the default)
    Lan {
        #[command(subcommand)]
        command: LanCommands,
    },

    /// Sign this machine in to Localify Cloud with GitHub, to give project domains public URLs
    Login {
        /// Print JSON (the default when stdout is not a terminal)
        #[arg(long)]
        json: bool,
    },

    /// Sign this machine out of Localify Cloud. The public URLs it serves go offline.
    Logout {
        /// Print JSON (the default when stdout is not a terminal)
        #[arg(long)]
        json: bool,
    },

    /// Show who is signed in to Localify Cloud on this machine, the plan, and how its tunnel is doing
    Whoami {
        /// Print JSON (the default when stdout is not a terminal)
        #[arg(long)]
        json: bool,
    },

    /// Start Localify Cloud Pro, after a 7-day trial: opens the checkout and waits for the plan
    Subscribe {
        /// Pay yearly ($79) instead of monthly ($7.99)
        #[arg(long)]
        yearly: bool,

        /// Print JSON (the default when stdout is not a terminal)
        #[arg(long)]
        json: bool,
    },

    /// Manage the Localify Cloud subscription: card, invoices, interval, cancelling
    Billing {
        /// Print JSON (the default when stdout is not a terminal)
        #[arg(long)]
        json: bool,
    },

    /// See this account's other machines, pair with them, and choose what they may see and do here
    Machines {
        /// Print JSON (the default when stdout is not a terminal)
        #[arg(long, global = true)]
        json: bool,

        #[command(subcommand)]
        command: Option<MachinesCommands>,
    },

    /// Share a project domain at https://<slug>.localify.fyi, behind a password unless opened
    PublicUrl {
        /// Print JSON (the default when stdout is not a terminal)
        #[arg(long, global = true)]
        json: bool,

        #[command(subcommand)]
        command: PublicUrlCommands,
    },

    /// Generate shell completions
    Completions {
        /// Shell to generate completions for
        shell: Shell,
    },

    /// For the desktop app: prints this environment framed by the nonce, NUL-separated, so the app can read what the user's shell init sets up (docs/remote-machines.md).
    #[command(hide = true)]
    ShellEnv {
        /// 32 hex digits the app chose for this read
        nonce: String,
    },
}

#[derive(Subcommand)]
enum MachinesCommands {
    /// Pair with another machine: both show a code, and a person confirms it on each
    Pair {
        /// The machine's name or id
        #[arg(required_unless_present = "accept")]
        machine: Option<String>,

        /// Wait on this machine for another one's pairing request
        #[arg(long, conflicts_with = "machine")]
        accept: bool,
    },

    /// Forget a paired machine here, and on it when a session is open
    Unpair {
        /// The machine's name or id
        machine: String,
    },

    /// Sign a machine out of the account: it can never connect again
    Remove {
        /// The machine's name or id
        machine: String,

        /// Do not ask for confirmation
        #[arg(long)]
        yes: bool,
    },

    /// Show or change what paired machines may see and do on this one (all off by default)
    Settings {
        /// Share projects, services and output with paired machines; off turns the others off too
        #[arg(long, value_enum)]
        share: Option<cli::machines::Switch>,

        /// Allow paired machines to start, stop and restart services here (needs --share on)
        #[arg(long, value_enum)]
        control: Option<cli::machines::Switch>,

        /// Allow remote starts in the background when the app is closed (needs --control on)
        #[arg(long = "background-starts", value_enum)]
        background_starts: Option<cli::machines::Switch>,
    },
}

#[derive(Subcommand)]
enum LanCommands {
    /// Listen on every network interface. macOS asks for one approval.
    Enable,

    /// Listen on this machine only. macOS asks for one approval.
    Disable,

    /// Show who can reach Localify
    Status,
}

#[derive(Subcommand)]
enum PublicUrlCommands {
    /// List your public URLs, with the machine that serves each
    List,

    /// Give a project domain of this machine a public URL, served from here. It asks for a generated password unless --password or --public.
    Create {
        /// A project's domain, such as web.journal.localify, or its namespace <project-id>.localify, which --wildcard opens to every service and environment under it
        local_domain: String,

        /// Also reach every environment of the project, at https://<slug>--<label>.localify.fyi
        #[arg(long)]
        wildcard: bool,

        /// Slug of the URL: 3 to 32 characters of a-z, 0-9 and -. Generated from the domain by default.
        #[arg(long)]
        slug: Option<String>,

        /// Password to ask for, instead of a generated one
        #[arg(long, conflicts_with = "public")]
        password: Option<String>,

        /// Open the URL to everyone, without a password
        #[arg(long)]
        public: bool,
    },

    /// Remove a public URL. Its slug stays reserved for you for 30 days.
    Remove {
        /// The public URL, its slug or its id
        #[arg(value_name = "URL_OR_SLUG_OR_ID")]
        reference: String,

        /// Remove without asking (required when no terminal is attached)
        #[arg(long)]
        yes: bool,
    },

    /// Ask for a password, or change it. Generates one unless --password is given. Browsers signed in before must sign in again.
    Protect {
        /// The public URL, its slug or its id
        #[arg(value_name = "URL_OR_SLUG_OR_ID")]
        reference: String,

        /// Password to ask for, instead of a generated one
        #[arg(long)]
        password: Option<String>,
    },

    /// Open a public URL to everyone, without a password
    Unprotect {
        /// The public URL, its slug or its id
        #[arg(value_name = "URL_OR_SLUG_OR_ID")]
        reference: String,

        /// Open it without asking (required when no terminal is attached)
        #[arg(long)]
        yes: bool,
    },

    /// Serve a public URL from this machine instead of the machine that serves it now
    Claim {
        /// The public URL, its slug or its id
        #[arg(value_name = "URL_OR_SLUG_OR_ID")]
        reference: String,
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
    /// Manage project environments
    Env {
        #[command(subcommand)]
        command: ProjectEnvCommands,
    },

    /// Manage project service domains
    Domain {
        #[command(subcommand)]
        command: ProjectDomainCommands,
    },

    /// Rename, enable or disable a project, or point it at the folder it moved to
    Update {
        /// Project ID or name
        project: String,

        /// New display name
        #[arg(long)]
        name: Option<String>,

        /// Route the project's domains again
        #[arg(long, conflicts_with = "disable")]
        enable: bool,

        /// Stop routing the project's domains without removing them
        #[arg(long)]
        disable: bool,

        /// Folder the project moved to
        #[arg(long)]
        root: Option<PathBuf>,
    },

    /// Reapply local dev scripts and framework config for the current project
    ConfigureDev {
        /// Project ID when the current folder is ambiguous
        #[arg(long)]
        project: Option<String>,

        /// Create or update package.json dev-localify scripts while repairing config
        #[arg(long)]
        write_scripts: bool,
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

        /// Create or update package.json dev-localify scripts for this service
        #[arg(long)]
        write_scripts: bool,
    },
}

#[derive(Subcommand)]
enum ProjectDomainCommands {
    /// Set the primary local domain for a service
    Set {
        /// Service name or ID
        service: String,

        /// New local domain
        domain: String,

        /// Project ID/name when the current folder is ambiguous
        #[arg(long)]
        project: Option<String>,

        /// Project environment name/slug. Defaults to the current worktree environment when detected.
        #[arg(long)]
        env: Option<String>,
    },

    /// Move a registered domain into a project
    Attach {
        /// Domain name
        domain: String,

        /// Project ID or name
        #[arg(long)]
        project: String,

        /// The wildcard registration of this domain
        #[arg(long)]
        wildcard: bool,
    },

    /// Take a registered domain out of its project, keeping the domain
    Detach {
        /// Domain name
        domain: String,

        /// The wildcard registration of this domain
        #[arg(long)]
        wildcard: bool,
    },
}

#[derive(Subcommand)]
enum ProjectEnvCommands {
    /// List project environments
    List {
        /// Project ID/name to filter
        #[arg(long)]
        project: Option<String>,

        /// Print structured JSON
        #[arg(long)]
        json: bool,
    },

    /// Discover Git worktrees for a project
    Discover {
        /// Project ID/name to inspect
        #[arg(long)]
        project: String,

        /// Print structured JSON
        #[arg(long)]
        json: bool,
    },

    /// Ensure a project environment for the current root/worktree
    Ensure {
        /// Project ID/name when the current folder is ambiguous
        #[arg(long)]
        project: Option<String>,

        /// Environment label. Defaults to the git branch or folder name.
        #[arg(long)]
        name: Option<String>,

        /// Environment root. Defaults to the current directory or git worktree root.
        #[arg(long)]
        root: Option<PathBuf>,

        /// Mark/update the primary environment metadata instead of creating variant domains.
        #[arg(long)]
        primary: bool,
    },

    /// Remove a project environment: stop its services and unregister its domains. Run it when a worktree is done.
    Remove {
        /// Environment name or slug
        env: String,

        /// Project ID/name when the current folder is ambiguous
        #[arg(long)]
        project: Option<String>,

        /// Also delete the environment's Git worktree folder. Git refuses when it has uncommitted changes. The branch is kept.
        #[arg(long)]
        delete_worktree: bool,

        /// With --delete-worktree, delete the worktree even with uncommitted changes
        #[arg(long, requires = "delete_worktree")]
        force: bool,
    },

    /// Remove the environments whose folder no longer exists, such as a worktree deleted with git
    Prune {
        /// Only this project (ID or name)
        #[arg(long)]
        project: Option<String>,

        /// List what would be removed without removing it
        #[arg(long)]
        dry_run: bool,
    },

    /// Copy environment files between project environments
    CopyEnv {
        /// Project ID/name
        #[arg(long)]
        project: String,

        /// Target project environment slug
        #[arg(long = "env")]
        environment: String,

        /// Source project environment slug
        #[arg(long = "from", default_value = "primary")]
        from: String,

        /// Relative environment file path to copy. Omit with --all to copy every source env file.
        #[arg(long = "path")]
        paths: Vec<String>,

        /// Copy every existing env file from the source environment
        #[arg(long)]
        all: bool,

        /// Overwrite existing files in the target environment
        #[arg(long)]
        overwrite: bool,

        /// Print structured JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum ServicesCommands {
    /// List explicit and inferred services
    List {
        /// Print structured JSON
        #[arg(long)]
        json: bool,
    },

    /// Show every service Localify started, in any environment: running or not, PID, URL, port, since when, exit code
    Status {
        /// Print structured JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum ProfileCommands {
    /// Create or update localify.toml interactively: pick sources (Infisical, Proton Pass notes, pass:// references or local files), check every service, preview, then write
    Init,
}

#[derive(Subcommand)]
enum AgentCommands {
    /// Connect the Localify MCP server to the coding agents installed here (Claude Code, Codex, Cursor)
    Install,

    /// Show which coding agents have the Localify MCP server
    Status,
}

fn cloud_adapters(
    config_path: &Path,
    paths: &LocalifyPaths,
) -> infrastructure::cloud_adapters::CloudAdapters {
    infrastructure::cloud_adapters::CloudAdapters::new(config_path, paths)
}

/// Load config from file, or return defaults if the file doesn't exist.
/// For `install`, the config file may not exist yet, so defaults are fine.
fn load_config_and_paths(config_path: &Path) -> Result<(Config, LocalifyPaths)> {
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

/// Ends this process with a service's exit code, so `localify dev web && next`
/// stops where the service failed or was interrupted (130).
fn exit_with(code: i32) -> Result<()> {
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

/// Files created as root would land in the user's folder, and nothing needs it: the
/// commands that do ask for their own approval. Root on Linux stays allowed without
/// sudo, and with no systemd, because a container may have no other user.
fn refuse_sudo() -> Result<()> {
    if !infrastructure::process::is_root() {
        return Ok(());
    }
    let through_sudo_with_systemd = std::env::var_os("SUDO_USER").is_some()
        && BRANDING.has_systemd()
        && !BRANDING.is_dev_profile();
    if cfg!(target_os = "macos") || (cfg!(target_os = "linux") && through_sudo_with_systemd) {
        anyhow::bail!(
            "{0} does not run as root. Run it without sudo: it asks for an administrator's approval when it needs one.",
            BRANDING.app_name
        );
    }
    Ok(())
}

/// The status a Unix tool ends with when its output is closed: 128 + SIGPIPE.
const CLOSED_STDOUT_EXIT_CODE: i32 = 141;

/// `println!` panics when stdout is closed, and `panic = "abort"` turns that into a
/// crash report. `localify status | head -1` is normal use, so it ends quietly instead.
fn exit_quietly_when_stdout_closes() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let message = info
            .payload()
            .downcast_ref::<String>()
            .map(String::as_str)
            .unwrap_or_default();
        if is_closed_stdout(message) {
            std::process::exit(CLOSED_STDOUT_EXIT_CODE);
        }
        default_hook(info);
    }));
}

fn is_closed_stdout(message: &str) -> bool {
    message.starts_with("failed printing to stdout") && message.contains("Broken pipe")
}

fn main() -> Result<()> {
    exit_quietly_when_stdout_closes();
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
    // Before any config is read, so nothing but the frame reaches stdout.
    if let Commands::ShellEnv { nonce } = &cli.command {
        return infrastructure::shell_env::print_environment(nonce);
    }

    refuse_sudo()?;

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
            write_scripts,
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
                write_scripts,
                detected_primary_domain: None,
            },
            config_path,
            localify::project_init::LocalAppUpdatePolicy::from_dev_script_preference(
                config.agent.promote_dev_script,
            ),
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
            let policy = localify::project_init::LocalAppUpdatePolicy::from_dev_script_preference(
                config.agent.promote_dev_script,
            );
            match command {
                ProjectCommands::Env { command } => match command {
                    ProjectEnvCommands::List { project, json } => cli::project_env::list(
                        cli::project_env::ListEnvArgs { project, json },
                        &ctx,
                    ),
                    ProjectEnvCommands::Discover { project, json } => cli::project_env::discover(
                        cli::project_env::DiscoverEnvArgs { project, json },
                        config_path,
                        &ctx,
                    ),
                    ProjectEnvCommands::Ensure {
                        project,
                        name,
                        root,
                        primary,
                    } => cli::project_env::ensure(
                        cli::project_env::EnsureEnvArgs {
                            project,
                            name,
                            root,
                            primary,
                        },
                        &ctx,
                    ),
                    ProjectEnvCommands::Remove {
                        env,
                        project,
                        delete_worktree,
                        force,
                    } => cli::project_env::remove(
                        cli::project_env::RemoveEnvArgs {
                            environment: env,
                            project,
                            delete_worktree,
                            force,
                        },
                        config_path,
                        &ctx,
                    ),
                    ProjectEnvCommands::Prune { project, dry_run } => cli::project_env::prune(
                        cli::project_env::PruneEnvArgs { project, dry_run },
                        config_path,
                        &ctx,
                    ),
                    ProjectEnvCommands::CopyEnv {
                        project,
                        environment,
                        from,
                        paths,
                        all,
                        overwrite,
                        json,
                    } => cli::project_env::copy_env(
                        cli::project_env::CopyEnvArgs {
                            project,
                            environment,
                            from,
                            paths,
                            all,
                            overwrite,
                            json,
                        },
                        config_path,
                    ),
                },
                ProjectCommands::Domain { command } => match command {
                    ProjectDomainCommands::Set {
                        service,
                        domain,
                        project,
                        env,
                    } => cli::project::set_domain(
                        cli::project::SetDomainArgs {
                            service,
                            domain,
                            project,
                            environment: env,
                        },
                        &ctx,
                    ),
                    ProjectDomainCommands::Attach {
                        domain,
                        project,
                        wildcard,
                    } => cli::project::assign_domain(
                        domain,
                        wildcard,
                        Some(project),
                        config_path,
                        &ctx,
                    ),
                    ProjectDomainCommands::Detach { domain, wildcard } => {
                        cli::project::assign_domain(domain, wildcard, None, config_path, &ctx)
                    }
                },
                ProjectCommands::Update {
                    project,
                    name,
                    enable,
                    disable,
                    root,
                } => cli::project::update(
                    cli::project::UpdateProjectArgs {
                        project,
                        name,
                        enabled: (enable || disable).then_some(enable),
                        root,
                    },
                    config_path,
                    &ctx,
                ),
                ProjectCommands::ConfigureDev {
                    project,
                    write_scripts,
                } => cli::project::configure_dev(
                    cli::project::ConfigureDevArgs {
                        project,
                        write_scripts,
                    },
                    config_path,
                    policy,
                ),
                ProjectCommands::AddService {
                    path,
                    project,
                    domain,
                    target,
                    write_scripts,
                } => cli::project::add_service(
                    cli::project::AddServiceArgs {
                        path,
                        project,
                        domain,
                        target,
                        write_scripts,
                    },
                    config_path,
                    policy,
                ),
            }
        }
        Commands::Services { command } => match command {
            ServicesCommands::List { json } => cli::services::list(&ctx, json),
            ServicesCommands::Status { json } => cli::background::status(json),
        },
        Commands::Get {
            service,
            project,
            env,
            json,
        } => cli::services::get(service, project, env, json, &ctx),
        Commands::Run {
            service,
            project,
            env,
            launch_profile,
            command,
        } => exit_with(cli::services::run(
            cli::services::RunArgs {
                mode: cli::services::RunMode::Run,
                service_name: service,
                project,
                environment: env,
                init_domain: None,
                override_command: command,
                launch_profile: launch_profile.into_args(),
                launch_id: None,
                replace: cli::services::Replace::for_launch(None)?,
            },
            config_path,
            localify::project_init::LocalAppUpdatePolicy::from_dev_script_preference(
                config.agent.promote_dev_script,
            ),
            &ctx,
            &paths,
        )?),
        Commands::Dev {
            detach: true,
            service,
            project,
            env,
            launch_id,
            replace,
            ..
        } => exit_with(cli::background::start_detached(
            launch_id.as_deref(),
            cli::background::Detached {
                target: cli::services::RunTarget {
                    service: service.as_deref(),
                    project: project.as_deref(),
                    environment: env.as_deref(),
                    init_domain: None,
                },
                replace: cli::services::Replace::for_launch(
                    replace.as_ref().map(|id| id.as_deref()),
                )?,
            },
            config_path,
            &ctx,
        )?),
        Commands::Dev {
            in_app: true,
            service,
            domain,
            project,
            env,
            launch_profile,
            command,
            launch_id,
            replace,
            ..
        } => {
            if !command.is_empty()
                || launch_id.is_some()
                || !launch_profile.env_file.is_empty()
                || launch_profile.confirm_profile.is_some()
                || launch_profile.no_masking
            {
                anyhow::bail!(
                    "--in-app starts the service as the app's launch button does: it takes a service, --project, --env and --profile only. Use --detach for the other options."
                );
            }
            cli::background::start_in_app(
                cli::services::RunTarget {
                    service: service.as_deref(),
                    project: project.as_deref(),
                    environment: env.as_deref(),
                    init_domain: domain.as_deref(),
                },
                launch_profile.profile.as_deref(),
                cli::services::Replace::for_launch(replace.as_ref().map(|id| id.as_deref()))?,
                config_path,
                &ctx,
            )
        }
        Commands::Dev {
            service,
            domain,
            project,
            env,
            launch_profile,
            command,
            launch_id,
            replace,
            ..
        } => exit_with(cli::services::run(
            cli::services::RunArgs {
                mode: cli::services::RunMode::Dev,
                service_name: service,
                project,
                environment: env,
                init_domain: domain,
                override_command: command,
                launch_profile: launch_profile.into_args(),
                launch_id,
                replace: cli::services::Replace::for_launch(
                    replace.as_ref().map(|id| id.as_deref()),
                )?,
            },
            config_path,
            localify::project_init::LocalAppUpdatePolicy::runtime_only(),
            &ctx,
            &paths,
        )?),
        Commands::Exec {
            launch_profile,
            command,
        } => {
            let code = cli::exec::run(
                cli::exec::ExecArgs {
                    launch_profile: launch_profile.into_args(),
                    command,
                },
                &paths,
            )?;
            if code != 0 {
                std::process::exit(code);
            }
            Ok(())
        }
        Commands::Profile { command } => match command {
            ProfileCommands::Init => cli::profile_init::run(&ctx),
        },
        Commands::Agent { command } => match command {
            AgentCommands::Install => cli::agent::install(),
            AgentCommands::Status => cli::agent::status(),
        },
        Commands::Mcp => cli::mcp::serve(config_path),
        Commands::Start { foreground } => {
            cli::start::execute(foreground, cli.verbose, config_path, &paths, &config.daemon)
        }
        Commands::Stop { service } => match service.service {
            Some(name) => cli::background::stop(
                &name,
                service.project.as_deref(),
                service.env.as_deref(),
                &ctx,
            ),
            None => cli::stop::execute(&ctx, &paths, &config.daemon),
        },
        Commands::Restart { service } => match service.service {
            Some(name) => cli::background::restart(
                &name,
                service.project.as_deref(),
                service.env.as_deref(),
                config_path,
                &ctx,
            ),
            None => cli::restart::execute(cli.verbose, config_path, &ctx, &paths, &config.daemon),
        },
        Commands::Status => cli::status::execute(&ctx, &config.daemon),
        Commands::Inventory { have } => cli::inventory::show(have.as_deref(), &ctx),
        Commands::Logs {
            service,
            lines,
            cursor,
            clear,
            follow,
        } => match service.service {
            Some(name) => cli::background::logs(
                cli::background::LogsArgs {
                    service: &name,
                    project: service.project.as_deref(),
                    environment: service.env.as_deref(),
                    lines,
                    follow,
                    cursor: cursor.as_deref(),
                },
                &ctx,
            ),
            None => cli::logs::execute(lines, clear, follow, &paths),
        },
        Commands::Reload => {
            cli::reload::execute(cli.verbose, config_path, &ctx, &paths, &config.daemon)
        }
        Commands::Trust => cli::trust::execute(config_path),
        Commands::Lan { command } => match command {
            LanCommands::Enable => cli::lan::set(true, config_path, &paths, &config.daemon),
            LanCommands::Disable => cli::lan::set(false, config_path, &paths, &config.daemon),
            LanCommands::Status => {
                cli::lan::print_status(&config.daemon);
                Ok(())
            }
        },
        Commands::Login { json } => cli::cloud::login(json, &cloud_adapters(config_path, &paths)),
        Commands::Logout { json } => cli::cloud::logout(json, &cloud_adapters(config_path, &paths)),
        Commands::Whoami { json } => cli::cloud::whoami(json, &cloud_adapters(config_path, &paths)),
        Commands::Subscribe { yearly, json } => {
            cli::cloud::subscribe(yearly, json, &cloud_adapters(config_path, &paths))
        }
        Commands::Billing { json } => {
            cli::cloud::billing(json, &cloud_adapters(config_path, &paths))
        }
        Commands::Machines { json, command } => match command {
            None => cli::machines::list(json, &ctx),
            Some(MachinesCommands::Pair { accept: true, .. }) => cli::machines::accept(&ctx),
            Some(MachinesCommands::Pair { machine, .. }) => {
                cli::machines::pair(machine.as_deref().unwrap_or_default(), &ctx)
            }
            Some(MachinesCommands::Unpair { machine }) => {
                cli::machines::unpair(&machine, json, &ctx)
            }
            Some(MachinesCommands::Remove { machine, yes }) => {
                cli::machines::remove(&machine, yes, json, &ctx, &paths)
            }
            Some(MachinesCommands::Settings {
                share,
                control,
                background_starts,
            }) => cli::machines::settings(share, control, background_starts, json, &ctx),
        },
        Commands::PublicUrl { json, command } => {
            let cloud = cloud_adapters(config_path, &paths);
            match command {
                PublicUrlCommands::List => cli::public_url::list(json, &cloud),
                PublicUrlCommands::Create {
                    local_domain,
                    wildcard,
                    slug,
                    password,
                    public,
                } => cli::public_url::create(
                    cli::public_url::CreateArgs {
                        local_domain,
                        wildcard,
                        slug,
                        password,
                        public,
                    },
                    json,
                    &cloud,
                ),
                PublicUrlCommands::Remove { reference, yes } => {
                    cli::public_url::remove(&reference, yes, json, &cloud)
                }
                PublicUrlCommands::Protect {
                    reference,
                    password,
                } => cli::public_url::protect(&reference, password, json, &cloud),
                PublicUrlCommands::Unprotect { reference, yes } => {
                    cli::public_url::unprotect(&reference, yes, json, &cloud)
                }
                PublicUrlCommands::Claim { reference } => {
                    cli::public_url::claim(&reference, json, &cloud)
                }
            }
        }
        Commands::Completions { .. } | Commands::ShellEnv { .. } => unreachable!(),
    }
}
