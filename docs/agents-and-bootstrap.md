# Agents and Bootstrap Plan

Localify should help coding agents use Localify the way a human would, and go further when they create new projects.

## Decisions

- Agents reach Localify through its MCP server, `localify mcp`, not a skill. Every action of the desktop app and the CLI is a tool, and each tool runs one CLI command, so the CLI stays the only implementation (ADR 0018).
- The server is the installed binary, so agents always get the actions of the installed version and nothing has to be synced after an update.
- `localify agent install`, and the desktop's Connect agents button, register the server with detected agents through their own CLIs (`claude mcp add`, `codex mcp add`) or, for Cursor, `~/.cursor/mcp.json`. A missing agent CLI is reported with the command to run by hand, not a Localify installation failure.
- The default install targets detected agents only.
- The server's instructions cover both existing projects and Agent-Initialized Projects.
- An agent that is done with a Git worktree removes it: `project env remove <env> --delete-worktree` stops the environment's services, deletes the worktree with `git worktree remove`, and unregisters its domains. Git's refusal on uncommitted changes is checked before anything changes; `--force` discards them. The branch is kept. `project env prune` removes environments whose folder was deleted another way.
- Agents have no terminal to keep a dev server in: `dev --detach` runs it in the background with its output in a file, and `services stop` and `services logs` act on any run of `localify dev`.
- For new Next.js projects, Localify should configure the app aggressively enough that agents can use the `.localify` URL immediately:
  - launch through `localify dev` with runtime `PORT`, `HOST`, `LOCALIFY_URL`, URL envs, Node CA env, and framework CLI flags where needed;
  - add the Local Domain to `allowedDevOrigins`;
  - add Localify-compatible `images.remotePatterns` for the Local Domain.
- Localify should not create framework config files by default. It patches existing supported config files. Creating a missing Next.js config is only acceptable as a rare fallback when Next is clearly detected as a dependency and the workflow explicitly chooses full framework bootstrap.
- Localify may create or update the `dev-localify` package script only when the user explicitly requests package script updates. In that case, framework-specific config updates are still skipped with an actionable message when no supported config exists.
- Localify has a user setting for promoting Localify to the default package `dev` script. This setting is disabled by default. When enabled, project bootstrap rewrites `dev` to the Localify-compatible command and keeps `dev-localify` aligned.
- When Localify promotes the package `dev` script, it preserves the previous command as `dev-localhost` when that script name is available. If `dev-localhost` already exists, Localify does not create another backup script.
- The promotion setting applies to future project bootstrap by default. Existing projects are only changed by an explicit per-project action.
- Localify supports both bootstrap-time updates through `init-project` and an explicit migration command/action for existing projects. Both paths should share the same local app update planner.
- The existing-project migration command is `localify project configure-dev`.
- Agent/dev bootstrap is project-scoped. It should not introduce standalone-domain flows.
- When `project configure-dev` runs in a folder that is not linked to a Localify project, it can create/link a project through the same planning flow as `init-project`. Interactive runs ask for confirmation; non-interactive runs require explicit flags or `--yes`.
- The primary command for agents working in a repository is `localify dev`. It must be able to create/link the project when needed, create Local Domains, apply supported framework updates, inject runtime env/flags, and launch the service without requiring package script rewrites. In an interactive terminal, first-run auto-bootstrap prompts for project/service domains only; non-interactive runs keep automatic defaults, and `localify dev --domain <name>` pre-fills or overrides the primary Local Domain while preserving detected monorepo service selection. `localify init-project --yes` is reserved for setup without launching. `localify project configure-dev` is reserved for existing-project migration, repair, or explicitly reapplying framework/package-script policy.
- Agents should treat `init-project` as the initial repository bootstrap, not a command to rerun blindly for repairs. After bootstrap, agents should use targeted status, listing, route, log, and configure-dev commands to diagnose or repair specific issues.
- On first bootstrap, Localify chooses available local ports and persists them in Localify service metadata instead of forcing framework defaults like `3000` or `5173`. Package scripts are updated only when explicitly requested.
- In monorepos, non-interactive `init-project --yes` should configure likely primary apps by heuristic rather than every package with a `dev` script. If a service is missing, agents can add or configure it later with targeted commands.
- Agents should verify whether the heuristic covered the relevant repo apps after bootstrap. Missing apps are added through a dedicated project service command rather than rerunning full initialization.
- The command for adding a missing app/service to an existing project is `localify project add-service`.
- `project add-service` takes an explicit `--path` to the app folder. `--target` defaults to `auto`, which reserves a free local port and persists it in Localify service metadata. `--domain` can be inferred from the app path and project slug when the project context is unambiguous.
- `project add-service` applies the same local app update planner as `init-project`: supported existing framework config patches by default, and package scripts only with `--write-scripts` or the desktop checkbox.
- Vite projects are first-class in v1. Localify should add an explicit `server.allowedHosts` entry for the Local Domain instead of setting `allowedHosts: true`, but only when a Vite config file already exists. Localify should not create `vite.config.*` for projects that do not already have one.
- Agents must have a first-class command for service domain changes. The command is `localify project domain set <service> <domain>`, with `--project` and `--env` for non-interactive disambiguation. It updates Localify project metadata and the registered route together, attempts to generate the new certificate, and reloads the daemon.
- When an agent runs Localify from a recognized agent project cache path, Localify should resolve that cache folder back to the linked real checkout instead of treating the cache as a new project root. The first supported cache provider is Claude's `~/.claude/projects/-Users-name-Developer-projects-my-app` layout. If a cache cannot be mapped to an existing checkout, the CLI should fail with an actionable message rather than initialize a project inside the agent cache.

## Open Questions

- Exact v1 parser/update coverage for existing Vite config shapes.
