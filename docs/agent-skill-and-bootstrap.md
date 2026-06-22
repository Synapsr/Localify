# Agent Skill and Bootstrap Plan

Localify should help coding agents use Localify the way a human would, and go further when they create new projects.

## Decisions

- Localify ships the `localify` Agent Skill with the app and keeps a user-local canonical copy in sync with the installed Localify version.
- Localify delegates global multi-agent installation to the `skills` CLI instead of maintaining Claude Code, Codex, Cursor, and other agent paths directly.
- The Agent Skill stays lightweight. It tells agents when and how to call Localify CLI commands; it does not duplicate bootstrap logic in helper scripts.
- Agent skill installation is best-effort: missing Node.js or `npx` is an actionable settings state, not a Localify installation failure.
- The default install targets detected agents only. Installing to every `skills`-supported agent is an explicit advanced option.
- The skill covers both existing projects and Agent-Initialized Projects.
- For new Next.js projects, Localify should configure the app aggressively enough that agents can use the `.localify` URL immediately:
  - create or update `dev-localify`;
  - add the Local Domain to `allowedDevOrigins`;
  - add Localify-compatible `images.remotePatterns` for the Local Domain.
- Localify should not create framework config files by default. It patches existing supported config files. Creating a missing Next.js config is only acceptable as a rare fallback when Next is clearly detected as a dependency and the workflow explicitly chooses full framework bootstrap.
- Localify may create or update the `dev-localify` package script even when no supported framework config file exists. In that case, framework-specific config updates are skipped with an actionable message.
- Localify has a user setting for promoting Localify to the default package `dev` script. This setting is enabled by default. When enabled, project bootstrap silently rewrites `dev` to the Localify-compatible command and keeps `dev-localify` aligned. When disabled, bootstrap only creates or updates `dev-localify`.
- When Localify promotes the package `dev` script, it preserves the previous command as `dev-localhost` when that script name is available. If `dev-localhost` already exists, Localify does not create another backup script.
- The promotion setting applies to future project bootstrap by default. Existing projects are only changed by an explicit per-project action.
- Localify supports both bootstrap-time updates through `init-project` and an explicit migration command/action for existing projects. Both paths should share the same local app update planner.
- The existing-project migration command is `localify project configure-dev`.
- Agent/dev bootstrap is project-scoped. It should not introduce standalone-domain flows.
- When `project configure-dev` runs in a folder that is not linked to a Localify project, it can create/link a project through the same planning flow as `init-project`. Interactive runs ask for confirmation; non-interactive runs require explicit flags or `--yes`.
- The primary command for agents working in a repository is `localify init-project --yes`. It must be able to create/link the project, create Local Domains, and apply the local dev scripts/framework updates needed for the project to run correctly under Localify. `localify project configure-dev` is reserved for existing-project migration, repair, or reapplying dev-script policy.
- Agents should treat `init-project` as the initial repository bootstrap, not a command to rerun blindly for repairs. After bootstrap, agents should use targeted status, listing, route, log, and configure-dev commands to diagnose or repair specific issues.
- On first bootstrap, Localify chooses available local ports and persists them in generated package scripts instead of forcing framework defaults like `3000` or `5173`.
- In monorepos, non-interactive `init-project --yes` should configure likely primary apps by heuristic rather than every package with a `dev` script. If a service is missing, agents can add or configure it later with targeted commands.
- Agents should verify whether the heuristic covered the relevant repo apps after bootstrap. Missing apps are added through a dedicated project service command rather than rerunning full initialization.
- The command for adding a missing app/service to an existing project is `localify project add-service`.
- `project add-service` takes an explicit `--path` to the app folder. `--target` defaults to `auto`, which reserves a free local port and persists it in generated scripts. `--domain` can be inferred from the app path and project slug when the project context is unambiguous.
- `project add-service` applies the same local app update planner as `init-project`: package scripts, optional `dev` promotion, backup to `dev-localhost` when available, and supported existing framework config patches.
- Vite projects are first-class in v1. Localify should create or update `dev-localify` and add an explicit `server.allowedHosts` entry for the Local Domain instead of setting `allowedHosts: true`, but only when a Vite config file already exists. Localify should not create `vite.config.*` for projects that do not already have one.

## Open Questions

- Exact v1 parser/update coverage for existing Vite config shapes.
