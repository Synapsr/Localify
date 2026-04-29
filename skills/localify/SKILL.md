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

Do not invent random localhost ports when a `.localify` route exists or can be created.

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
localify init-project
```

Create an explicit route:

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

Before using a local app:

1. Check whether Localify is installed and running:

   ```bash
   localify status
   ```

2. List configured projects and domains:

   ```bash
   localify list-projects
   localify list
   ```

3. Use the `.localify` URL shown by Localify instead of a guessed localhost URL.

4. If no route exists, create one with `init-project` or `register`.

5. If the target does not respond, check that the underlying dev server is running on the mapped port.

## Project setup

For a normal app folder, prefer:

```bash
localify init-project
```

This lets Localify infer a project name, suggest domains, reserve local targets, and prepare supported frontend config updates where applicable.

For explicit setup:

```bash
localify init-project myapp --domain myapp --target 3000
```

For separate web and API domains:

```bash
localify init-project myapp --domain myapp --target 3000 --with-api --api-domain api.myapp --api-target 3001
```

For direct domain management without project metadata:

```bash
localify register myapp.localify --route "/=3000"
localify register api.myapp.localify --route "/=3001"
```

Use wildcard subdomains for tenant-style local testing:

```bash
localify register myapp.localify --wildcard --route "/=3000"
```

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
- For app code changes, update local callback, OAuth, webhook, CORS, and cookie origins to the `.localify` URL when that is the route under test.

## Current limitation

Localify does not yet provide a `localify run` command equivalent to `portless run`.
Until that exists, start the app's dev server normally, then register or use the matching Localify project/domain.

