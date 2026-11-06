//! The tools `localify mcp` offers, each one CLI command. A tool turns its JSON
//! arguments into that command's arguments; the CLI does the rest.

use anyhow::{Result, bail};
use serde_json::{Map, Value, json};

pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    pub params: &'static [Param],
    /// The command depends on the folder it runs in (project detection, worktrees).
    pub uses_cwd: bool,
    pub read_only: bool,
    pub destructive: bool,
    pub build: fn(&ToolArgs) -> Result<Vec<String>>,
}

pub struct Param {
    pub name: &'static str,
    pub kind: Kind,
    pub required: bool,
    pub description: &'static str,
}

pub enum Kind {
    String,
    Boolean,
    Integer,
    StringList,
    OneOf(&'static [&'static str]),
}

const fn param(name: &'static str, kind: Kind, description: &'static str) -> Param {
    Param {
        name,
        kind,
        required: false,
        description,
    }
}

const fn required(name: &'static str, kind: Kind, description: &'static str) -> Param {
    Param {
        name,
        kind,
        required: true,
        description,
    }
}

const PROJECT: Param = param(
    "project",
    Kind::String,
    "Project ID or name. Needed only when the folder does not tell which project.",
);
const ENV: Param = param(
    "env",
    Kind::String,
    "Project environment (a worktree checkout with its own domains and ports), by slug or name. Inside a worktree, defaults to that worktree's environment; elsewhere, to the primary checkout.",
);
const SERVICE: Param = required(
    "service",
    Kind::String,
    "Service name or ID, as list_services shows it.",
);
const WILDCARD: Param = param(
    "wildcard",
    Kind::Boolean,
    "The wildcard registration (*.domain) instead of the plain domain.",
);
const DOMAIN: Param = required("domain", Kind::String, "Domain name ending in .localify.");
const WRITE_SCRIPTS: Param = param(
    "write_scripts",
    Kind::Boolean,
    "Also write a dev-localify script in package.json. Only when the user wants it checked in.",
);
const PROFILE: Param = param(
    "profile",
    Kind::String,
    "Launch profile from localify.toml: changes only the variables the service starts with.",
);
const ENV_FILES: Param = param(
    "env_files",
    Kind::StringList,
    "Extra dotenv files, relative to cwd, loaded before the profile's sources.",
);
const CONFIRM_PROFILE: Param = param(
    "confirm_profile",
    Kind::String,
    "Repeats the profile name to start a danger = true profile. Pass it only when the user asked for that profile in this task.",
);
const LINES: Param = param("lines", Kind::Integer, "Number of lines to return.");
const PUBLIC_URL: Param = required(
    "url",
    Kind::String,
    "The public URL, its slug or its id, as public_url_list shows them.",
);

/// `stop` or `restart` of one service.
fn service_command(verb: &str, args: &ToolArgs) -> Result<Vec<String>> {
    Args::new(&[verb])
        .positional(args.required("service")?)?
        .option("--project", args.string("project")?)
        .option("--env", args.string("env")?)
        .done()
}

pub const TOOLS: &[Tool] = &[
    Tool {
        name: "status",
        description: "Shows whether Localify is installed and its daemon runs, DNS and HTTPS trust state, and the registered domains. Call it first when a .localify URL does not load.",
        params: &[],
        uses_cwd: false,
        read_only: true,
        destructive: false,
        build: |_| Ok(argv(&["status"])),
    },
    Tool {
        name: "list_projects",
        description: "Lists Localify projects with their IDs, folders and domains.",
        params: &[],
        uses_cwd: false,
        read_only: true,
        destructive: false,
        build: |_| Ok(argv(&["list-projects"])),
    },
    Tool {
        name: "list_domains",
        description: "Lists every registered .localify domain and where it routes.",
        params: &[],
        uses_cwd: false,
        read_only: true,
        destructive: false,
        build: |_| Ok(argv(&["list"])),
    },
    Tool {
        name: "list_services",
        description: "Lists the services of every project (JSON): ID, project, URL, domains, folder, dev command and local target.",
        params: &[],
        uses_cwd: false,
        read_only: true,
        destructive: false,
        build: |_| Ok(argv(&["services", "list", "--json"])),
    },
    Tool {
        name: "service_status",
        description: "Shows every service Localify started, in every environment (JSON): running or not, PID, URL, port, origin (app, background, terminal), start time, and exit code once it stopped.",
        params: &[],
        uses_cwd: false,
        read_only: true,
        destructive: false,
        build: |_| Ok(argv(&["services", "status", "--json"])),
    },
    Tool {
        name: "get_service_url",
        description: "Returns a service's .localify URL and details (JSON). Use it instead of guessing localhost ports.",
        params: &[SERVICE, PROJECT, ENV],
        uses_cwd: true,
        read_only: true,
        destructive: false,
        build: |args| {
            Args::new(&["get"])
                .positional(args.required("service")?)?
                .flag("--json", true)
                .option("--project", args.string("project")?)
                .option("--env", args.string("env")?)
                .done()
        },
    },
    Tool {
        name: "service_start",
        description: "Starts a service in the Localify app's terminal, as its launch button does, so the user sees it there, and returns once it listens, with its URL. If it already runs, says so and starts nothing. Without a service, uses the one in cwd; in a folder that is not a project yet, creates the project and its domains first. Set background when the app is not installed. Read its output with service_logs.",
        params: &[
            param(
                "service",
                Kind::String,
                "Service name or ID. Defaults to the service of cwd.",
            ),
            PROJECT,
            ENV,
            param(
                "domain",
                Kind::String,
                "Primary domain to use when this call creates the project. Not for existing services: use set_service_domain.",
            ),
            PROFILE,
            param(
                "background",
                Kind::Boolean,
                "Run it in the background instead of the app's terminal. Needed for env_files, confirm_profile and command.",
            ),
            ENV_FILES,
            CONFIRM_PROFILE,
            param(
                "command",
                Kind::StringList,
                "With background: command to run instead of the service's dev command, one argument per item.",
            ),
        ],
        uses_cwd: true,
        read_only: false,
        destructive: false,
        build: |args| {
            let mut command = Args::new(&["dev"]);
            if let Some(service) = args.string("service")? {
                command = command.positional(service)?;
            }
            let background = args.flag("background")?;
            command
                .flag("--in-app", !background)
                .flag("--detach", background)
                .option("--project", args.string("project")?)
                .option("--env", args.string("env")?)
                .option("--domain", args.string("domain")?)
                .option("--profile", args.string("profile")?)
                .options("--env-file", args.list("env_files")?)
                .option("--confirm-profile", args.string("confirm_profile")?)
                .trailing(args.list("command")?)
                .done()
        },
    },
    Tool {
        name: "service_stop",
        description: "Stops a service Localify started (in the app, the background or a terminal), with everything it launched.",
        params: &[SERVICE, PROJECT, ENV],
        uses_cwd: true,
        read_only: false,
        destructive: false,
        build: |args| service_command("stop", args),
    },
    Tool {
        name: "service_restart",
        description: "Stops a service and starts it again where it ran, on the same launch profile, and returns once it listens.",
        params: &[SERVICE, PROJECT, ENV],
        uses_cwd: true,
        read_only: false,
        destructive: false,
        build: |args| service_command("restart", args),
    },
    Tool {
        name: "service_logs",
        description: "Returns a service's output, the same lines the app's terminal shows, without colors: the last `lines` lines, or with `cursor` only what came after it. The result ends with the next cursor: pass it back to read what follows.",
        params: &[
            SERVICE,
            PROJECT,
            ENV,
            LINES,
            param(
                "cursor",
                Kind::Integer,
                "Cursor from the previous service_logs call, to read only newer lines.",
            ),
        ],
        uses_cwd: true,
        read_only: true,
        destructive: false,
        build: |args| {
            Args::new(&["logs"])
                .positional(args.required("service")?)?
                .option("--project", args.string("project")?)
                .option("--env", args.string("env")?)
                .option("--lines", args.integer("lines")?)
                .option("--cursor", args.integer("cursor")?)
                .done()
        },
    },
    Tool {
        name: "run_command",
        description: "Runs a command that is not a service (migration, database studio, script) in cwd with the variables of a launch profile, and returns its output when it ends.",
        params: &[
            required(
                "command",
                Kind::StringList,
                "Command to run, one argument per item.",
            ),
            PROFILE,
            ENV_FILES,
            CONFIRM_PROFILE,
        ],
        uses_cwd: true,
        read_only: false,
        destructive: false,
        build: |args| {
            let command = args.list("command")?;
            if command.is_empty() {
                bail!("command needs at least one item.");
            }
            Args::new(&["exec"])
                .option("--profile", args.string("profile")?)
                .options("--env-file", args.list("env_files")?)
                .option("--confirm-profile", args.string("confirm_profile")?)
                .trailing(command)
                .done()
        },
    },
    Tool {
        name: "init_project",
        description: "Creates a Localify project for the app in cwd without starting it: reserves free ports, registers .localify domains and patches supported framework config (Next.js, Vite). For a first run, service_start does this too. Do not rerun it to repair a project: use configure_dev or add_service.",
        params: &[
            param(
                "name",
                Kind::String,
                "Project name. Defaults to the folder name.",
            ),
            param(
                "domain",
                Kind::String,
                "Primary domain, e.g. my-app (.localify is added).",
            ),
            param(
                "target",
                Kind::String,
                "Fixed upstream (port, host:port, URL or folder). Only when the user asked for one: Localify picks a free port otherwise.",
            ),
            param("with_api", Kind::Boolean, "Also create an API domain."),
            param(
                "api_domain",
                Kind::String,
                "API domain when with_api is set.",
            ),
            param(
                "api_target",
                Kind::String,
                "API target when with_api is set.",
            ),
            param(
                "project_only",
                Kind::Boolean,
                "Create the project without domains.",
            ),
            WRITE_SCRIPTS,
        ],
        uses_cwd: true,
        read_only: false,
        destructive: false,
        build: |args| {
            let mut command = Args::new(&["init-project"]);
            if let Some(name) = args.string("name")? {
                command = command.positional(name)?;
            }
            command
                .flag("--yes", true)
                .option("--domain", args.string("domain")?)
                .option("--target", args.string("target")?)
                .flag("--with-api", args.flag("with_api")?)
                .option("--api-domain", args.string("api_domain")?)
                .option("--api-target", args.string("api_target")?)
                .flag("--project-only", args.flag("project_only")?)
                .flag("--write-scripts", args.flag("write_scripts")?)
                .done()
        },
    },
    Tool {
        name: "add_service",
        description: "Adds an app folder of the project in cwd as a new service (for a monorepo app the first setup skipped), with a free port and a domain.",
        params: &[
            required(
                "path",
                Kind::String,
                "App folder relative to the project root, e.g. apps/admin.",
            ),
            PROJECT,
            param(
                "domain",
                Kind::String,
                "Domain. Defaults to the app name and project.",
            ),
            param(
                "target",
                Kind::String,
                "Fixed upstream. Defaults to a free port.",
            ),
            WRITE_SCRIPTS,
        ],
        uses_cwd: true,
        read_only: false,
        destructive: false,
        build: |args| {
            Args::new(&["project", "add-service"])
                .option("--path", Some(args.required("path")?))
                .option("--project", args.string("project")?)
                .option("--domain", args.string("domain")?)
                .option("--target", args.string("target")?)
                .flag("--write-scripts", args.flag("write_scripts")?)
                .done()
        },
    },
    Tool {
        name: "configure_dev",
        description: "Reapplies Localify's framework config (allowed dev origins and hosts) to the project in cwd, after it drifted.",
        params: &[PROJECT, WRITE_SCRIPTS],
        uses_cwd: true,
        read_only: false,
        destructive: false,
        build: |args| {
            Args::new(&["project", "configure-dev"])
                .option("--project", args.string("project")?)
                .flag("--write-scripts", args.flag("write_scripts")?)
                .done()
        },
    },
    Tool {
        name: "set_service_domain",
        description: "Changes a service's domain: updates the project, replaces the route, creates the certificate and reloads routing.",
        params: &[SERVICE, DOMAIN, PROJECT, ENV],
        uses_cwd: true,
        read_only: false,
        destructive: false,
        build: |args| {
            Args::new(&["project", "domain", "set"])
                .positional(args.required("service")?)?
                .positional(args.required("domain")?)?
                .option("--project", args.string("project")?)
                .option("--env", args.string("env")?)
                .done()
        },
    },
    Tool {
        name: "update_project",
        description: "Renames a project, enables or disables it (a disabled project's domains stop routing), or points it at the folder it moved to.",
        params: &[
            required("project", Kind::String, "Project ID or name."),
            param("name", Kind::String, "New name."),
            param(
                "enabled",
                Kind::Boolean,
                "true to enable, false to disable.",
            ),
            param(
                "root",
                Kind::String,
                "Absolute path of the folder the project moved to.",
            ),
        ],
        uses_cwd: false,
        read_only: false,
        destructive: false,
        build: |args| {
            let enabled = args.optional_flag("enabled")?;
            Args::new(&["project", "update"])
                .positional(args.required("project")?)?
                .option("--name", args.string("name")?)
                .flag("--enable", enabled == Some(true))
                .flag("--disable", enabled == Some(false))
                .option("--root", args.string("root")?)
                .done()
        },
    },
    Tool {
        name: "remove_project",
        description: "Removes a project from Localify. The project's folder is not touched.",
        params: &[
            required(
                "project",
                Kind::String,
                "Project ID, as list_projects shows it.",
            ),
            param(
                "delete_domains",
                Kind::Boolean,
                "Also unregister its domains. Required when it still has domains.",
            ),
        ],
        uses_cwd: false,
        read_only: false,
        destructive: true,
        build: |args| {
            Args::new(&["remove-project"])
                .positional(args.required("project")?)?
                .flag("--force", true)
                .flag("--delete-domains", args.flag("delete_domains")?)
                .done()
        },
    },
    Tool {
        name: "list_environments",
        description: "Lists project environments (JSON): the primary checkout and each attached worktree, with folder, branch and service domains.",
        params: &[PROJECT],
        uses_cwd: false,
        read_only: true,
        destructive: false,
        build: |args| {
            Args::new(&["project", "env", "list", "--json"])
                .option("--project", args.string("project")?)
                .done()
        },
    },
    Tool {
        name: "discover_worktrees",
        description: "Lists a project's Git worktrees (JSON) and whether each one is attached as an environment.",
        params: &[required("project", Kind::String, "Project ID or name.")],
        uses_cwd: false,
        read_only: true,
        destructive: false,
        build: |args| {
            Args::new(&["project", "env", "discover", "--json"])
                .option("--project", Some(args.required("project")?))
                .done()
        },
    },
    Tool {
        name: "ensure_environment",
        description: "Attaches a worktree as a project environment with its own domains and ports. service_start does it by itself when run inside a worktree.",
        params: &[
            PROJECT,
            param(
                "name",
                Kind::String,
                "Environment name. Defaults to the branch or folder name.",
            ),
            param("root", Kind::String, "Worktree folder. Defaults to cwd."),
            param(
                "primary",
                Kind::Boolean,
                "Update the primary environment instead.",
            ),
        ],
        uses_cwd: true,
        read_only: false,
        destructive: false,
        build: |args| {
            Args::new(&["project", "env", "ensure"])
                .option("--project", args.string("project")?)
                .option("--name", args.string("name")?)
                .option("--root", args.string("root")?)
                .flag("--primary", args.flag("primary")?)
                .done()
        },
    },
    Tool {
        name: "copy_environment_files",
        description: "Copies .env files from one environment to another, such as from the primary checkout into a new worktree. Existing files are kept unless overwrite is set.",
        params: &[
            required("project", Kind::String, "Project ID or name."),
            required("env", Kind::String, "Target environment slug."),
            param(
                "from",
                Kind::String,
                "Source environment slug. Defaults to primary.",
            ),
            param(
                "paths",
                Kind::StringList,
                "Env file paths relative to the root.",
            ),
            param(
                "all",
                Kind::Boolean,
                "Copy every env file of the source instead of paths.",
            ),
            param(
                "overwrite",
                Kind::Boolean,
                "Replace files that already exist.",
            ),
        ],
        uses_cwd: false,
        read_only: false,
        destructive: false,
        build: |args| {
            Args::new(&["project", "env", "copy-env", "--json"])
                .option("--project", Some(args.required("project")?))
                .option("--env", Some(args.required("env")?))
                .option("--from", args.string("from")?)
                .options("--path", args.list("paths")?)
                .flag("--all", args.flag("all")?)
                .flag("--overwrite", args.flag("overwrite")?)
                .done()
        },
    },
    Tool {
        name: "remove_environment",
        description: "Cleans up after a worktree: stops its services, unregisters its domains and certificates, and with delete_worktree deletes the Git worktree folder (Git refuses if it has uncommitted changes; the branch is kept). Call it when the work in a worktree is done, from outside that worktree.",
        params: &[
            required("env", Kind::String, "Environment slug or name."),
            PROJECT,
            param(
                "delete_worktree",
                Kind::Boolean,
                "Also delete the worktree folder with git worktree remove.",
            ),
            param(
                "force",
                Kind::Boolean,
                "With delete_worktree, discard uncommitted changes. Only when the user agreed to lose them.",
            ),
        ],
        uses_cwd: true,
        read_only: false,
        destructive: true,
        build: |args| {
            Args::new(&["project", "env", "remove"])
                .positional(args.required("env")?)?
                .option("--project", args.string("project")?)
                .flag("--delete-worktree", args.flag("delete_worktree")?)
                .flag("--force", args.flag("force")?)
                .done()
        },
    },
    Tool {
        name: "prune_environments",
        description: "Removes the environments whose folder no longer exists, such as worktrees deleted with git. Call it after deleting a worktree any other way than remove_environment.",
        params: &[
            param("project", Kind::String, "Only this project (ID or name)."),
            param("dry_run", Kind::Boolean, "List them without removing."),
        ],
        uses_cwd: false,
        read_only: false,
        destructive: true,
        build: |args| {
            Args::new(&["project", "env", "prune"])
                .option("--project", args.string("project")?)
                .flag("--dry-run", args.flag("dry_run")?)
                .done()
        },
    },
    Tool {
        name: "register_domain",
        description: "Registers a domain outside any project, routing to targets the user gave. For a project's app, prefer init_project or add_service.",
        params: &[
            DOMAIN,
            required(
                "routes",
                Kind::StringList,
                "Routes as PATH=TARGET, e.g. \"/=3000\" or \"/api=127.0.0.1:3001\". TARGET is a port, host:port or folder.",
            ),
            param("wildcard", Kind::Boolean, "Also match every subdomain."),
        ],
        uses_cwd: false,
        read_only: false,
        destructive: false,
        build: |args| {
            let routes = args.list("routes")?;
            if routes.is_empty() {
                bail!("routes needs at least one PATH=TARGET.");
            }
            Args::new(&["register"])
                .positional(args.required("domain")?)?
                .flag("--wildcard", args.flag("wildcard")?)
                .options("--route", routes)
                .done()
        },
    },
    Tool {
        name: "unregister_domain",
        description: "Unregisters a domain and deletes its certificate.",
        params: &[DOMAIN, WILDCARD],
        uses_cwd: false,
        read_only: false,
        destructive: true,
        build: |args| {
            Args::new(&["unregister"])
                .positional(args.required("domain")?)?
                .flag("--wildcard", args.flag("wildcard")?)
                .flag("--force", true)
                .done()
        },
    },
    Tool {
        name: "attach_domain",
        description: "Moves a registered domain into a project.",
        params: &[
            DOMAIN,
            required("project", Kind::String, "Project ID or name."),
            WILDCARD,
        ],
        uses_cwd: false,
        read_only: false,
        destructive: false,
        build: |args| {
            Args::new(&["project", "domain", "attach"])
                .positional(args.required("domain")?)?
                .option("--project", Some(args.required("project")?))
                .flag("--wildcard", args.flag("wildcard")?)
                .done()
        },
    },
    Tool {
        name: "detach_domain",
        description: "Takes a domain out of its project. The domain stays registered.",
        params: &[DOMAIN, WILDCARD],
        uses_cwd: false,
        read_only: false,
        destructive: false,
        build: |args| {
            Args::new(&["project", "domain", "detach"])
                .positional(args.required("domain")?)?
                .flag("--wildcard", args.flag("wildcard")?)
                .done()
        },
    },
    Tool {
        name: "list_routes",
        description: "Lists a domain's routes.",
        params: &[DOMAIN, WILDCARD],
        uses_cwd: false,
        read_only: true,
        destructive: false,
        build: |args| {
            Args::new(&["route", "list"])
                .flag("--wildcard", args.flag("wildcard")?)
                .positional(args.required("domain")?)?
                .done()
        },
    },
    Tool {
        name: "add_route",
        description: "Adds a path route to a registered domain.",
        params: &[
            DOMAIN,
            required("path", Kind::String, "Path prefix, e.g. /api."),
            required("target", Kind::String, "Port, host:port or folder."),
            WILDCARD,
        ],
        uses_cwd: false,
        read_only: false,
        destructive: false,
        build: |args| {
            Args::new(&["route", "add"])
                .flag("--wildcard", args.flag("wildcard")?)
                .positional(args.required("domain")?)?
                .positional(args.required("path")?)?
                .positional(args.required("target")?)?
                .done()
        },
    },
    Tool {
        name: "remove_route",
        description: "Removes a path route from a domain.",
        params: &[
            DOMAIN,
            required("path", Kind::String, "Path prefix to remove."),
            WILDCARD,
        ],
        uses_cwd: false,
        read_only: false,
        destructive: true,
        build: |args| {
            Args::new(&["route", "remove"])
                .flag("--wildcard", args.flag("wildcard")?)
                .positional(args.required("domain")?)?
                .positional(args.required("path")?)?
                .done()
        },
    },
    Tool {
        name: "daemon",
        description: "Starts, stops, restarts or reloads the Localify daemon that serves DNS and the proxy. stop keeps it stopped until start. reload rereads the config.",
        params: &[required(
            "action",
            Kind::OneOf(&["start", "stop", "restart", "reload"]),
            "What to do.",
        )],
        uses_cwd: false,
        read_only: false,
        destructive: false,
        build: |args| {
            Ok(vec![args.one_of(
                "action",
                &["start", "stop", "restart", "reload"],
            )?])
        },
    },
    Tool {
        name: "read_daemon_logs",
        description: "Returns the last lines of the Localify daemon log: proxy, DNS and certificate errors.",
        params: &[LINES],
        uses_cwd: false,
        read_only: true,
        destructive: false,
        build: |args| {
            Args::new(&["logs"])
                .option("--lines", args.integer("lines")?)
                .done()
        },
    },
    Tool {
        name: "install_localify",
        description: "Sets Localify up on this machine (DNS resolver, certificate authority, daemon). macOS asks the user for one approval. Only when status says Localify is not installed.",
        params: &[],
        uses_cwd: false,
        read_only: false,
        destructive: false,
        build: |_| Ok(argv(&["install"])),
    },
    Tool {
        name: "trust_https",
        description: "Makes browsers on this Mac trust Localify's HTTPS certificates. Opens a macOS window that needs the user's password: only when the user asked.",
        params: &[],
        uses_cwd: false,
        read_only: false,
        destructive: false,
        build: |_| Ok(argv(&["trust"])),
    },
    Tool {
        name: "lan_access",
        description: "Shows or changes whether other devices on the network can reach Localify. enable exposes the user's dev servers to the network: only when the user asked.",
        params: &[required(
            "action",
            Kind::OneOf(&["status", "enable", "disable"]),
            "status, enable or disable.",
        )],
        uses_cwd: false,
        read_only: false,
        destructive: false,
        build: |args| {
            Ok(vec![
                "lan".to_string(),
                args.one_of("action", &["status", "enable", "disable"])?,
            ])
        },
    },
    Tool {
        name: "public_url_list",
        description: "Lists the user's public URLs on Localify Cloud (JSON): https://<slug>.localify.fyi, the local domain each one serves, password or open, and which machine serves it (servedHere).",
        params: &[],
        uses_cwd: false,
        read_only: true,
        destructive: false,
        build: |_| Ok(argv(&["public-url", "list", "--json"])),
    },
    Tool {
        name: "public_url_create",
        description: "Gives a project domain of this machine a public https://<slug>.localify.fyi URL, served from here and protected by a password. Returns the URL and the generated password (JSON), shown this once: pass both on to the user. Needs the machine signed in (a person runs `localify login`).",
        params: &[
            required(
                "local_domain",
                Kind::String,
                "A project's domain as list_services shows it, or the project's namespace <project-id>.localify, which wildcard opens to every service and worktree under it.",
            ),
            param(
                "wildcard",
                Kind::Boolean,
                "Also reach every environment (worktree) of the project, at https://<slug>--<label>.localify.fyi.",
            ),
            param(
                "slug",
                Kind::String,
                "Slug of the URL, 3 to 32 characters of a-z, 0-9 and -. Generated from the domain by default.",
            ),
            param(
                "password",
                Kind::String,
                "Only when the user chose the password. Otherwise Localify generates one and returns it.",
            ),
        ],
        uses_cwd: false,
        read_only: false,
        destructive: false,
        build: |args| {
            Args::new(&["public-url", "create"])
                .positional(args.required("local_domain")?)?
                .flag("--json", true)
                .flag("--wildcard", args.flag("wildcard")?)
                .option("--slug", args.string("slug")?)
                .option("--password", args.string("password")?)
                .done()
        },
    },
    Tool {
        name: "public_url_remove",
        description: "Removes a public URL: it stops working for everyone at once. Its slug stays reserved for the user for 30 days.",
        params: &[PUBLIC_URL],
        uses_cwd: false,
        read_only: false,
        destructive: true,
        build: |args| {
            Args::new(&["public-url", "remove"])
                .positional(args.required("url")?)?
                .flag("--json", true)
                .flag("--yes", true)
                .done()
        },
    },
    Tool {
        name: "public_url_protect",
        description: "Puts a public URL behind a password, or replaces its password, which signs every browser out. Returns the generated password (JSON), shown this once: pass it on to the user.",
        params: &[
            PUBLIC_URL,
            param(
                "password",
                Kind::String,
                "Only when the user chose the password. Otherwise Localify generates one and returns it.",
            ),
        ],
        uses_cwd: false,
        read_only: false,
        destructive: false,
        build: |args| {
            Args::new(&["public-url", "protect"])
                .positional(args.required("url")?)?
                .flag("--json", true)
                .option("--password", args.string("password")?)
                .done()
        },
    },
    Tool {
        name: "public_url_claim",
        description: "Serves a public URL from this machine instead of the machine that served it, such as one that was reinstalled or is gone.",
        params: &[PUBLIC_URL],
        uses_cwd: false,
        read_only: false,
        destructive: false,
        build: |args| {
            Args::new(&["public-url", "claim"])
                .positional(args.required("url")?)?
                .flag("--json", true)
                .done()
        },
    },
];

impl Tool {
    /// The tool as `tools/list` describes it.
    pub fn definition(&self) -> Value {
        let mut properties = Map::new();
        let mut required = Vec::new();
        for param in self.params {
            properties.insert(param.name.to_string(), param.schema());
            if param.required {
                required.push(param.name);
            }
        }
        if self.uses_cwd {
            properties.insert(
                "cwd".to_string(),
                json!({
                    "type": "string",
                    "description": "Absolute path of the folder to run in: the project, app or worktree. Defaults to the folder the agent started Localify's server in.",
                }),
            );
        }
        json!({
            "name": self.name,
            "description": self.description,
            "inputSchema": {
                "type": "object",
                "properties": properties,
                "required": required,
                "additionalProperties": false,
            },
            "annotations": {
                "readOnlyHint": self.read_only,
                "destructiveHint": self.destructive,
                "openWorldHint": false,
            },
        })
    }
}

impl Param {
    fn schema(&self) -> Value {
        let mut schema = match self.kind {
            Kind::String => json!({ "type": "string" }),
            Kind::Boolean => json!({ "type": "boolean" }),
            Kind::Integer => json!({ "type": "integer", "minimum": 0 }),
            Kind::StringList => json!({ "type": "array", "items": { "type": "string" } }),
            Kind::OneOf(values) => json!({ "type": "string", "enum": values }),
        };
        schema["description"] = json!(self.description);
        schema
    }
}

/// A tool call's arguments, read with the types its schema promises.
pub struct ToolArgs {
    values: Map<String, Value>,
}

impl ToolArgs {
    pub fn new(value: &Value) -> Result<Self> {
        match value {
            Value::Null => Ok(Self { values: Map::new() }),
            Value::Object(values) => Ok(Self {
                values: values.clone(),
            }),
            _ => bail!("Tool arguments must be a JSON object."),
        }
    }

    pub fn string(&self, name: &str) -> Result<Option<String>> {
        match self.values.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(value)) if value.trim().is_empty() => Ok(None),
            Some(Value::String(value)) => Ok(Some(value.clone())),
            Some(_) => bail!("'{name}' must be a string."),
        }
    }

    pub fn required(&self, name: &str) -> Result<String> {
        match self.string(name)? {
            Some(value) => Ok(value),
            None => bail!("'{name}' is required."),
        }
    }

    pub fn flag(&self, name: &str) -> Result<bool> {
        Ok(self.optional_flag(name)?.unwrap_or(false))
    }

    pub fn optional_flag(&self, name: &str) -> Result<Option<bool>> {
        match self.values.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Bool(value)) => Ok(Some(*value)),
            Some(_) => bail!("'{name}' must be true or false."),
        }
    }

    pub fn integer(&self, name: &str) -> Result<Option<String>> {
        match self.values.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Number(value)) if value.as_u64().is_some() => Ok(Some(value.to_string())),
            Some(_) => bail!("'{name}' must be a whole number, 0 or more."),
        }
    }

    pub fn list(&self, name: &str) -> Result<Vec<String>> {
        match self.values.get(name) {
            None | Some(Value::Null) => Ok(Vec::new()),
            Some(Value::Array(items)) => items
                .iter()
                .map(|item| match item {
                    Value::String(value) => Ok(value.clone()),
                    _ => bail!("'{name}' must be a list of strings."),
                })
                .collect(),
            Some(_) => bail!("'{name}' must be a list of strings."),
        }
    }

    pub fn one_of(&self, name: &str, allowed: &[&str]) -> Result<String> {
        let value = self.required(name)?;
        if !allowed.contains(&value.as_str()) {
            bail!("'{name}' must be one of: {}.", allowed.join(", "));
        }
        Ok(value)
    }
}

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| part.to_string()).collect()
}

/// Builds CLI arguments so that no value can be read as a flag: options are
/// passed as `--name=value`, and positionals may not start with `-`.
struct Args(Vec<String>);

impl Args {
    fn new(parts: &[&str]) -> Self {
        Self(argv(parts))
    }

    fn positional(mut self, value: String) -> Result<Self> {
        if value.starts_with('-') {
            bail!("'{value}' cannot start with '-'.");
        }
        self.0.push(value);
        Ok(self)
    }

    fn option(mut self, flag: &str, value: Option<String>) -> Self {
        if let Some(value) = value {
            self.0.push(format!("{flag}={value}"));
        }
        self
    }

    fn options(mut self, flag: &str, values: Vec<String>) -> Self {
        for value in values {
            self.0.push(format!("{flag}={value}"));
        }
        self
    }

    fn flag(mut self, flag: &str, on: bool) -> Self {
        if on {
            self.0.push(flag.to_string());
        }
        self
    }

    /// A command for the service or `exec`, after `--`.
    fn trailing(mut self, command: Vec<String>) -> Self {
        if !command.is_empty() {
            self.0.push("--".to_string());
            self.0.extend(command);
        }
        self
    }

    fn done(self) -> Result<Vec<String>> {
        Ok(self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(name: &str, arguments: Value) -> Result<Vec<String>> {
        let tool = TOOLS.iter().find(|tool| tool.name == name).unwrap();
        (tool.build)(&ToolArgs::new(&arguments)?)
    }

    #[test]
    fn tool_names_are_unique() {
        let mut names = TOOLS.iter().map(|tool| tool.name).collect::<Vec<_>>();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), TOOLS.len());
    }

    #[test]
    fn service_start_runs_in_the_app_unless_asked_for_the_background() {
        assert_eq!(
            build(
                "service_start",
                json!({ "service": "web", "env": "feat-auth" })
            )
            .unwrap(),
            ["dev", "web", "--in-app", "--env=feat-auth"]
        );
        assert_eq!(
            build(
                "service_start",
                json!({ "service": "web", "background": true, "command": ["pnpm", "dev"] }),
            )
            .unwrap(),
            ["dev", "web", "--detach", "--", "pnpm", "dev"]
        );
    }

    #[test]
    fn service_logs_reads_on_from_a_cursor() {
        assert_eq!(
            build("service_logs", json!({ "service": "web", "cursor": 0 })).unwrap(),
            ["logs", "web", "--cursor=0"]
        );
    }

    #[test]
    fn remove_environment_can_delete_the_worktree() {
        let args = build(
            "remove_environment",
            json!({ "env": "feat-auth", "delete_worktree": true }),
        )
        .unwrap();

        assert_eq!(
            args,
            ["project", "env", "remove", "feat-auth", "--delete-worktree"]
        );
    }

    #[test]
    fn a_value_cannot_become_a_flag() {
        assert!(build("service_stop", json!({ "service": "--all" })).is_err());

        let args = build(
            "service_stop",
            json!({ "service": "web", "project": "--force" }),
        )
        .unwrap();
        assert_eq!(args, ["stop", "web", "--project=--force"]);
    }

    #[test]
    fn update_project_maps_enabled_to_one_flag() {
        assert_eq!(
            build(
                "update_project",
                json!({ "project": "app", "enabled": false })
            )
            .unwrap(),
            ["project", "update", "app", "--disable"]
        );
    }

    #[test]
    fn an_agent_cannot_sign_in_or_open_a_url_to_everyone() {
        for name in ["login", "logout", "public_url_unprotect"] {
            assert!(TOOLS.iter().all(|tool| tool.name != name), "{name}");
        }
        let create = TOOLS
            .iter()
            .find(|tool| tool.name == "public_url_create")
            .unwrap();
        assert!(create.params.iter().all(|param| param.name != "public"));
        assert!(
            build(
                "public_url_create",
                json!({ "local_domain": "journal.localify", "public": true })
            )
            .unwrap()
            .iter()
            .all(|arg| arg != "--public")
        );
    }

    #[test]
    fn public_url_tools_print_json_and_remove_without_a_prompt() {
        assert_eq!(
            build(
                "public_url_create",
                json!({ "local_domain": "journal.localify", "wildcard": true })
            )
            .unwrap(),
            [
                "public-url",
                "create",
                "journal.localify",
                "--json",
                "--wildcard"
            ]
        );
        assert_eq!(
            build("public_url_remove", json!({ "url": "journal-a8f3" })).unwrap(),
            ["public-url", "remove", "journal-a8f3", "--json", "--yes"]
        );
        assert!(
            TOOLS
                .iter()
                .find(|tool| tool.name == "public_url_remove")
                .unwrap()
                .destructive
        );
        assert_eq!(
            build(
                "public_url_protect",
                json!({ "url": "https://journal-a8f3.localify.fyi", "password": "--x" })
            )
            .unwrap(),
            [
                "public-url",
                "protect",
                "https://journal-a8f3.localify.fyi",
                "--json",
                "--password=--x"
            ]
        );
        assert!(build("public_url_claim", json!({ "url": "--all" })).is_err());
    }

    #[test]
    fn wrong_types_are_refused() {
        assert!(build("service_logs", json!({ "service": "web", "lines": "ten" })).is_err());
        assert!(build("daemon", json!({ "action": "explode" })).is_err());
        assert!(build("run_command", json!({ "command": [] })).is_err());
    }
}
