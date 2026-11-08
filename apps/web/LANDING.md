# Landing page reference

Source of truth for the copy, features, and product facts on the website (`apps/web/`). Read the relevant parts before you write a section. If something here is wrong, fix it here first, then update the section that depends on it.

For the full Rust and desktop picture (architecture, DNS, TLS, proxy behavior, control plane), see [TECHNICAL-REFERENCE.md](TECHNICAL-REFERENCE.md).

---

## 1. Positioning

- **One-liner**: a clean HTTPS URL for every project, worktree, and machine. Your agents run it, you open the URL.
- **Angle**: agent first, human in the loop. Coding agents set Localify up and drive it over MCP; the person watches and steers from the desktop app.
- **Strongest point**: it scales with the way people code with agents now: several agents in parallel, one Git worktree each, on more than one computer. Every worktree gets its own URL, and one public URL per project reaches all of them from another device.
- **Category**: local development proxy, HTTPS for localhost.
- **Audience**: developers who let AI coding tools (Claude Code, Codex, Cursor) work in parallel, often on several machines.
- **Tone**: pragmatic, technical, understated. Close to Linear, Raycast, or Arc in voice.
- **Actions**: downloading the app is the primary action. Viewing the source at <https://github.com/Synapsr/Localify> is the secondary one.

## 2. The problem we solve

Working with AI agents means running several dev servers in parallel, often one per Git worktree, and sometimes on a machine you're not sitting at. Each one wants port 3000 by default, and each one points its env (`BASE_URL`, OAuth callbacks) at `http://localhost:3000`.

- **Port collisions**: opening a second project bumps it to `:3001`, which silently breaks anything its env hard-coded to `:3000`.
- **No mental map**: four tabs all say "localhost".
- **Demos feel unfinished**: `http://localhost:3000` doesn't look like a product.
- **Side effects compound**: webhooks and OAuth callbacks can't tolerate moving ports.
- **You lose sight of what agents run**: servers start in terminals you never see, on a machine you may not be at.

## 3. How the app works

1. **Download the app.**
2. **Onboarding**: on first launch it asks for the administrator password once, then sets up HTTPS, DNS, and the daemon. macOS asks to confirm the certificate once, the first time HTTPS is needed in a browser.
3. **Add projects**, two ways:
   - **Pick a folder**: Localify scans it and fills in what it can (name, apps, ports, likely domains). The user reviews and confirms.
   - **CLI**: `localify init-project` in the project directory does the same from the editor's terminal.
4. **Use the domains**: each project exposes one or more `.localify` hostnames that open directly in the browser.

## 4. Features

The product is organized around **projects**. Everything else is either a way to create a project or something a project exposes.

### 4.1 Projects as the primary abstraction

A project groups several apps under one name. Each app gets its own `.localify` hostname, and they share the project's dashboard, logs, and settings. Developers think in projects, not domains.

### 4.2 Two ways in: folder or CLI

Pick a folder in the app (Localify scans it, detects apps, proposes fields, and the user confirms), or run `localify init-project` from the editor's terminal.

### 4.3 Open in editor, launch server

Each project card can open the folder in the user's editor and start the dev server. The main view doubles as a status dashboard across all projects.

### 4.4 Request logs

A feed of incoming requests per project, with method, path, and status.

### 4.5 Customizable hotkeys

Users can remap the app's hotkeys in settings.

### 4.6 Setup is handled

DNS, TLS, CA install, Node trust, and CLI install happen during onboarding. There's no `/etc/hosts` editing, no `mkcert`, and no `NODE_EXTRA_CA_CERTS` setup. This is the headline promise.

### 4.7 Set up by your AI agent

Localify includes an MCP server for Claude Code, Codex, and Cursor (`localify agent install`, or **Connect agents** in the desktop **Settings → AI agents**). Every action of the app and the CLI is a tool. The agent sets the project up, starts and stops services behind their `.localify` URLs, and when its work in a Git worktree is done, removes that environment and the worktree folder. The site leads with this (use case 01).

### 4.8 One URL per worktree

A service started inside a Git worktree attaches the worktree to the project as an environment, with its own ports and domains: `environment.project.localify` for a single-service project, `service--environment.project.localify` otherwise (ADR 0009). The main checkout keeps its URLs. `remove_environment` with `delete_worktree` stops its services, unregisters its domains, and deletes the folder.

### 4.9 Public URLs across machines (Localify Cloud Pro)

`localify public-url create <project>.localify --wildcard` gives a project one `https://<slug>.localify.fyi` URL, and every service and worktree under it opens at `https://<slug>--<label>.localify.fyi`. It is protected by a password by default, with a link that signs a browser in by itself. One machine serves each URL; `claim` moves it to another of the user's machines. Agents can create, protect, and remove URLs; signing in and opening a URL to everyone stay the person's ([docs/public-urls.md](../../docs/public-urls.md)).

- **Plan**: Localify Cloud Pro, $7.99 a month or $79 a year, after a 7-day free trial: 2 serving machines, 10 public URLs.
- **Access**: sign-in is invite-only for now (`ALLOWED_GITHUB_IDS` in `apps/cloud`). Say so wherever the page sells Pro, and drop it when sign-up opens.

## 5. Technical snapshot

High-level anchor for copy decisions. Full detail in [TECHNICAL-REFERENCE.md](TECHNICAL-REFERENCE.md).

- Localify is a **Rust daemon** combining four pieces: a **local DNS server**, an **HTTP server**, an **HTTPS server**, and a **Unix management socket** for the CLI and desktop app ([src/daemon/server.rs](../../src/daemon/server.rs), [src/daemon/mgmt_socket.rs](../../src/daemon/mgmt_socket.rs)).
- Install creates the folders and config, generates a **local Root CA**, installs it in the trust store, and configures the OS to route `.localify` DNS queries to the embedded DNS ([src/application/install.rs](../../src/application/install.rs), [src/infrastructure/dns/macos.rs](../../src/infrastructure/dns/macos.rs)).
- The embedded DNS only resolves `.localify` and refuses anything else ([src/daemon/dns_server.rs](../../src/daemon/dns_server.rs)).
- Routing runs on **domain registrations**: exact or wildcard patterns with per-path-prefix routes. Matching uses **host specificity**, then **longest path prefix** ([src/domain/value_objects/domain_pattern.rs](../../src/domain/value_objects/domain_pattern.rs), [src/domain/registration.rs](../../src/domain/registration.rs), [src/daemon/router.rs](../../src/daemon/router.rs)).
- The HTTP proxy rewrites the URI toward the target, injects `X-Forwarded-*`, strips hop-by-hop headers, and forwards the response. WebSockets get an upgrade handshake, then a bidirectional stream copy ([src/daemon/proxy.rs](../../src/daemon/proxy.rs)).
- TLS uses **rustls + SNI**. For registered domains Localify loads or generates the cert. For a valid but unknown `.localify` hostname it can mint a **cert on the fly**, signed by the local CA, so the browser shows a proper app error instead of a raw TLS failure ([src/daemon/tls.rs](../../src/daemon/tls.rs)).
- When the domain is unknown, the route is missing, or the upstream is down, the daemon renders **diagnostic HTML pages**. If the domain is bound to a local project, those pages can offer **Open in editor** and **Run dev**, locked to same-origin and loopback ([src/daemon/router.rs](../../src/daemon/router.rs)).

**Shipping features** (what copy can claim today):

- Custom `.localify` domains with trusted local HTTPS.
- Wildcard subdomains (one level).
- Path-based routing to a port, `host:port`, or static folder.
- WebSocket support.
- Live request logs and a daemon log viewer.
- Project mode with grouped domains and `init-project` bootstrap ([src/cli/init_project.rs](../../src/cli/init_project.rs)).
- Docker and Compose auto-discovery ([docs/docker.md](../../docs/docker.md)).
- Tauri desktop app with **Projects / Logs / Settings** ([shared-navigation.constants.ts](../../desktop-ui/src/components/shared/shared-navigation.constants.ts)).
- Node.js trust onboarding to prevent server-side TLS errors in Node and Next ([src/desktop/node_trust.rs](../../src/desktop/node_trust.rs)).
- Desktop packaging, CLI sidecar, and auto-update ([docs/desktop.md](../../docs/desktop.md)).

### Facts copy can use

- **License**: everything that runs on the user's machines is free and open source under AGPL-3.0-only. No account, and no limit on projects, domains, or worktrees. Public URLs are the only paid part (4.9).
- **Platforms**: the desktop app runs on macOS on Apple Silicon for now. The CLI and daemon install on macOS on Apple Silicon and Linux x86_64 with `curl -fsSL https://localify.dev/install.sh | sh` (`public/install.sh`, which downloads the GitHub release and checks its SHA-256), and build from source elsewhere. Windows and a Linux desktop app are planned for later, with no date to promise.
- **For agents**: `public/llms.txt` sums up install, first steps, MCP and public URLs. Keep it in step with this file and `docs/README.md`.
- **Architecture**: Rust daemon plus Tauri desktop UI.
- **Offline**: the proxy, DNS, and TLS need no internet after install.
- **Telemetry**: official desktop builds send product analytics and error reports to PostHog. A filter removes URLs, file paths, `.localify` domains, and quoted names from error reports before sending, but don't promise it catches everything. The CLI and daemon send nothing. On the website, Umami runs for every visitor and PostHog only after consent in the cookie banner. The privacy policy has the details.

## 6. Differentiators

- **vs. `/etc/hosts` + `mkcert`**: Localify sets up DNS, the local CA, and Node trust in one permission prompt, with no manual `mkcert -install`, `NODE_EXTRA_CA_CERTS` exports, or per-project scripts.
- **vs. ngrok, Cloudflare Tunnel, zrok, Pinggy**: a tunnel puts one port on the internet. Localify starts on the machine: a named HTTPS URL per project and worktree, set up by agents and visible in one app. When the person needs them elsewhere, one public URL covers a whole project, every worktree included.
- **vs. LocalCan**: the closest product (local domains, HTTPS, public URLs). Localify is open source, free for everything local, and built around agents and worktrees rather than sharing previews.
- **vs. Portless**: named `.localhost` URLs for agents, CLI only. Localify adds trusted HTTPS, the desktop app, and public URLs across machines.
- **vs. Caddy, nginx**: a different problem. They optimize for running production-shaped configs on a dev box. Localify optimizes for DX: a simple UI, no config files, and a natural flow for juggling many projects.

## 7. Proof

There are no users yet, so the site carries no testimonials, ratings, customer logos, or usage numbers. Add one only when it's real and we can name the source.

## 8. Page sections

Keep in sync with [`app/[locale]/page.tsx`](app/[locale]/page.tsx).

| Section | File | Notes |
|---|---|---|
| Hero | `app/_sections/hero.tsx` | Eyebrow, download and source actions, CLI install command, interactive mockup of the Projects view with a download dialog, then the strip of supported agents |
| Use cases | `app/_sections/use-cases.tsx` | Four numbered rows: agent setup (4.7), one URL per worktree (4.8), watching from the app, every machine (4.9). Visuals in `app/_components/use-case-visuals.tsx` |
| Not just URLs | `app/_sections/features.tsx` | Three cards: HTTPS setup, launch profiles, any stack |
| How it works | `app/_sections/how-it-works.tsx` | Three steps and a diagram of the pieces |
| Pricing | `app/_sections/pricing.tsx` | Free and Localify Cloud Pro (4.9); the Pro action asks for an invite by email while sign-in is invite-only |
| FAQ | `app/_sections/faq.tsx` | |
| Final CTA | `app/_sections/final-cta.tsx` | Download and source actions, requirements |

## 9. Visuals

The use-case visuals are coded mockups, not videos, so they follow the product without re-recording. They reuse the sky backgrounds in `public/sky/` and the example project `journal` from the docs. Keep what they show true: tool names from `localify mcp`, domain shapes from ADR 0009 and 0022.

## 10. Copy conventions

- Sentence case for headings.
- Inline code style (`studio.localify`, `127.0.0.1:5173`) for hostnames and ports.
- Use a number only if it was measured.
- Describe what the product does today. Roadmap items and features that don't exist yet stay off the page.
- American English spelling.
- Every string exists in all nine locales, with a real translation.
