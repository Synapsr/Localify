---
name: localify
description: Set up and use Localify for stable local development domains with trusted HTTPS, project routing, Docker discovery, and desktop-assisted debugging. Use when replacing localhost URLs, configuring .localify domains, testing OAuth or webhooks locally, wiring local apps for agents, or troubleshooting Localify DNS, HTTPS, daemon, or route issues.
---

# Localify

Localify gives local projects stable `.localify` domains with trusted HTTPS.
Use it instead of guessing or hardcoding `localhost:<port>` URLs.

## When to use this skill

Use Localify when an agent needs to:

- open or test a local app through a stable URL
- configure OAuth, webhook, callback, cookie, or CORS origins for local dev
- map one project to one or more local services
- debug a local domain that does not resolve, does not trust HTTPS, or reaches the wrong target
- inspect local proxy, daemon, DNS, certificate, Docker, or desktop app state

Do not choose framework default ports like `3000` yourself during project bootstrap. Let
`localify dev` or `localify init-project --yes` reserve an available port and wire the Localify
runtime config.

## Mental model

Localify is not a per-command Node wrapper. It is a local proxy system:

1. The setup installs a local CA and DNS resolver integration.
2. The daemon listens for HTTP, HTTPS, and DNS.
3. Projects register `.localify` domains that route to local targets.
4. The desktop app drives the same Rust core through Tauri commands.

The daemon is the source of truth for routing. The desktop app is a management surface.

## Quick start

If Localify is already installed:

```bash
localify status
localify list-projects
localify list
```

Create a project from the current folder:

```bash
localify init-project --yes
```

Start the current project through Localify:

```bash
localify dev
```

Pre-fill the primary domain during first-run auto-bootstrap:

```bash
localify dev --domain playground
```

Create an explicit route only when the user gives you the target:

```bash
localify register myapp.localify --route "/=3000"
```

Start the daemon:

```bash
localify start
```

Open the app at:

```text
https://myapp.localify
```

## Preferred agent workflow

When entering an existing repo or after creating a new app:

1. Check whether Localify is installed and running:

   ```bash
   localify status
   ```

2. Start the repo through Localify:

   ```bash
   localify dev
   ```

   This auto-bootstraps a missing Localify Project when the current folder exposes a runnable app,
   creates Local Domains, patches supported framework allowlists, injects `PORT`, `HOST`,
   `LOCALIFY_URL`, URL envs, and Node CA env, then launches the detected dev script.
   In an interactive terminal, first-run auto-bootstrap prompts for project/service domains with
   defaults that can be accepted by pressing Enter. Pass `--domain <name>` to pre-fill the primary
   domain or to choose it in non-interactive runs. It does not prompt for the reserved port or
   package script rewrites.

3. Bootstrap explicitly when you only want project metadata/routes without starting the app:

   ```bash
   localify init-project --yes
   ```

   This creates or links the Localify Project, creates the primary Local Domains, chooses available
   local ports, and patches supported framework config. It does not write `dev-localify` package
   scripts unless the user explicitly opts into package script updates.

4. List configured projects and domains:

   ```bash
   localify list-projects
   localify list
   localify services list
   ```

5. Resolve the service URL instead of guessing:

   ```bash
   localify get web
   ```

   For monorepos, inspect service names first:

   ```bash
   localify services list
   ```

6. If a relevant monorepo app was not selected by the bootstrap heuristic, add it explicitly:

   ```bash
   localify project add-service --path apps/admin
   ```

7. If a service domain should change, use the project-scoped domain command instead of editing
   Localify config directly:

   ```bash
   localify project domain set web my-app.localify
   ```

   For branch/worktree environments, pass the environment when it is not already implied by the
   current worktree:

   ```bash
   localify project domain set web feat-auth.my-app.localify --env feat-auth
   ```

8. If framework config drift after setup, repair the existing Project instead of rerunning bootstrap:

   ```bash
   localify project configure-dev
   ```

9. Before running a service from a branch/worktree environment, ensure that environment explicitly:

   ```bash
   localify project env ensure --name feat-auth
   localify project env list
   ```

10. Run a service through the Localify service metadata:

   ```bash
   localify dev web
   localify dev web --env feat-auth
   ```

   `dev` starts the configured or detected dev command, injects `PORT`, `HOST=127.0.0.1`,
   `LOCALIFY_URL`, URL envs, and framework runtime flags where needed. It does not write package
   scripts.

11. If the target does not respond, check that the underlying dev server is running on the mapped port.

When a coding agent is launched from a recognized agent project cache, Localify resolves the cache
path back to the linked real checkout when the project was already initialized. The first supported
cache format is Claude's `~/.claude/projects/-Users-name-Developer-projects-my-app` layout. If
Localify reports that the cache is not linked, run the command from the real project folder or
initialize/link the real folder first; do not create a Localify project inside the agent cache.

## Project setup

For a normal app folder, prefer:

```bash
localify init-project --yes
```

This lets Localify infer a project name, suggest domains, reserve local targets, and prepare supported frontend config updates where applicable.

For explicit setup:

```bash
localify init-project myapp --domain myapp --yes
```

For separate web and API domains:

```bash
localify init-project myapp --domain myapp --with-api --api-domain api.myapp --yes
```

Only pass `--target` or `--api-target` when the user explicitly requested a fixed upstream target
or an existing Localify `dev-localify` script already defines one. Do not copy a running `next dev`
or Vite default port such as `3000`, `5173`, or `4321` into the Localify project unless that fixed
port is intentional.

`init-project --yes` is an initial bootstrap command, not a generic repair loop. After it succeeds,
use targeted commands like `project configure-dev`, `project add-service`, `route list`, `logs`, and
`status` to investigate or repair specific issues.

Package script rewrites are optional. Prefer `localify dev` for runtime port/env injection. Use
`localify init-project --write-scripts`, `localify project configure-dev --write-scripts`, or the
desktop checkbox only when the repository should contain a checked-in `dev-localify` script.

For direct domain management without project metadata:

```bash
localify register myapp.localify --route "/=3000"
localify register api.myapp.localify --route "/=3001"
```

Use wildcard subdomains for tenant-style local testing:

```bash
localify register myapp.localify --wildcard --route "/=3000"
```

For project-owned service domains, prefer:

```bash
localify project domain set web myapp.localify
localify project domain set web preview.myapp.localify --env preview
```

This updates the service or environment metadata, replaces the registered route, attempts to generate
the certificate for the new domain, and reloads the daemon.

## Routes

Routes map a path prefix to a target.

```bash
localify register myapp.localify --route "/=3000" --route "/api=3001"
localify route add myapp.localify /admin 8080
localify route list myapp.localify
localify route remove myapp.localify /admin
```

Targets can be:

- a port: `3000`
- a host and port: `127.0.0.1:3000`
- an HTTPS target when explicitly needed: `https://127.0.0.1:3000`
- a directory for static file serving

Prefer explicit HTTPS targets only when the upstream app itself serves HTTPS.

## Desktop app

Use the desktop app when available to inspect:

- setup status
- daemon state
- project list
- domain target reachability
- logs
- certificate and Node.js trust diagnostics

If the desktop app and CLI disagree, trust the daemon/config state and refresh the desktop bootstrap.

## Docker

Localify can auto-discover Docker Compose services when Docker integration is enabled.

Expected pattern:

```text
https://<service>.<project>.localify
```

Use labels when the default is not enough:

```yaml
labels:
  localify.domain: "api.myapp.localify"
  localify.port: "8080"
  localify.wildcard: "true"
```

If Docker routes are missing, check Docker is running, restart the Localify daemon, then inspect `localify list`.

## Troubleshooting

Check daemon and setup state:

```bash
localify status
```

Check registered domains:

```bash
localify list
localify list-projects
```

Read logs:

```bash
localify logs
localify logs -f
```

Restart routing after config changes:

```bash
localify reload
localify restart
```

Common failures:

- Domain does not resolve: setup/DNS resolver may be missing, daemon may be stopped, or the wrong Localify profile is active.
- HTTPS warning: local CA may not be installed or trusted by the current browser/runtime.
- Browser works but Node.js fails: Node may need Localify's CA via its trust integration or `NODE_EXTRA_CA_CERTS`.
- Localify page shows target unavailable: the upstream dev server is not listening on the configured target.
- Wrong app responds: inspect `localify list`, route specificity, wildcard domains, and stale dev servers.
- WebSocket fails: confirm the upstream target and protocol match what the app serves.

## Agent rules

- Prefer `.localify` URLs over `localhost:<port>` once a route exists.
- Do not edit `/etc/hosts` manually for Localify domains.
- Do not run a second proxy to solve a Localify routing issue.
- Do not bypass certificate errors as a fix. Repair Localify trust instead.
- Keep project routes explicit and small.
- Use wildcard domains only when the app actually needs tenant or subdomain testing.
- Use `localify project domain set` for project service domain changes. Do not hand-edit the
  Localify TOML config for this.
- If the current directory is under a recognized agent project cache such as `.claude/projects`,
  rely on Localify's linked-project resolution or switch to the real checkout. Do not initialize
  projects inside agent cache folders.
- For app code changes, update local callback, OAuth, webhook, CORS, and cookie origins to the `.localify` URL when that is the route under test.
- After creating a Next.js or Vite project, run `localify dev` before handing the URL to the user.
- For Next.js, expect Localify to patch an existing `next.config.*` with `allowedDevOrigins` and Localify-compatible `images.remotePatterns`.
- For Vite, expect Localify to patch an existing `vite.config.*` with an explicit `server.allowedHosts` entry. Do not set `allowedHosts: true`.

## Run boundary

`localify run` is a compatibility launch command. Prefer `localify dev` for dev servers. Use
`localify init-project --yes`, `localify project add-service`, or `localify project env ensure` for
explicit config mutations; use `localify dev --env <name>` when working inside a project worktree or
branch environment.
