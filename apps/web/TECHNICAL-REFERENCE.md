# Technical reference

The technical picture of Localify, kept in sync with the Rust code and the desktop app. Read this before writing a claim about how the product works. When the code and this doc diverge, fix the doc.

Companion to [LANDING.md](LANDING.md), which carries the positioning, audience, features and copy conventions.

The repository, runtime, desktop app, CLI, and domain suffix all use the
Localify name. Product naming is centralized in
[`src/branding.rs`](../../src/branding.rs).

---

## 1. Overall architecture

Localify is "Rust-first": the real product is the Rust binary. The desktop app is a thin Tauri shell that calls `DesktopApi`; business logic does not live in `desktop-ui` ([docs/ARCHITECTURE.md](../../docs/ARCHITECTURE.md)). In practice, the daemon owns runtime state, the proxy, the DNS, the TLS, the management socket, and the config read/write.

## 2. Data model

Persisted config holds three things: daemon settings, domains, and projects ([src/infrastructure/config/mod.rs](../../src/infrastructure/config/mod.rs)).

- A registered domain is a `DomainRegistration`: an exact or wildcard pattern, a list of routes, an HTTPS flag, and a source of either `config` or `external` ([src/domain/registration.rs](../../src/domain/registration.rs)).
- A wildcard covers the root domain and **one** subdomain level — not multi-level ([src/domain/value_objects/domain_pattern.rs](../../src/domain/value_objects/domain_pattern.rs)).

## 3. Boot and system integration

`localify install` prepares the machine: folders, default config file, Root CA, OS DNS config ([src/application/install.rs](../../src/application/install.rs)).

On macOS, Localify writes `/etc/resolver/localify` to route `.localify` queries to `127.0.0.1` and `::1` on the configured DNS port, then checks `scutil --dns` to confirm the resolver is picked up ([src/infrastructure/dns/macos.rs](../../src/infrastructure/dns/macos.rs)). When the daemon is running, status surfaces also run an effective system lookup for `localify-healthcheck.localify` so VPN or custom-DNS setups that bypass the macOS resolver do not look healthy by mistake ([src/infrastructure/dns/system_resolution.rs](../../src/infrastructure/dns/system_resolution.rs)). The embedded DNS daemon then handles `.localify` queries itself.

## 4. Full request lifecycle

On startup, the daemon loads registrations and project roots, builds an atomically swappable `RuntimeState` with `ArcSwap`, starts DNS, starts the management socket, then HTTP/HTTPS ([src/daemon/server.rs](../../src/daemon/server.rs)).

For each request:

- extract the `Host`;
- look up the most specific registration;
- pick the route with the longest path prefix;
- on miss, render a Localify error page;
- on match, serve a static folder or proxy upstream ([src/daemon/router.rs](../../src/daemon/router.rs), [src/daemon/router.rs](../../src/daemon/router.rs)).

## 5. Proxy behavior

For HTTP, Localify isn't a "magic" reverse proxy: it explicitly rebuilds the target URI as `http://host:port/path?query`, adds `X-Forwarded-Host`, `X-Forwarded-Proto`, `X-Forwarded-For`, strips hop-by-hop headers, then relays the response ([src/daemon/proxy.rs](../../src/daemon/proxy.rs)).

For WebSocket: detect the upgrade, open a `TcpStream` to the upstream, forward the upgrade request, check for `101 Switching Protocols`, then copy the client/backend streams both ways ([src/daemon/proxy.rs](../../src/daemon/proxy.rs), [src/daemon/proxy.rs](../../src/daemon/proxy.rs)).

## 6. TLS and local HTTPS

The daemon serves HTTPS directly with `rustls` — TLS isn't delegated to nginx/Caddy. The SNI resolver picks the right cert by hostname.

- If the cert for a registered domain doesn't exist yet, it's generated automatically.
- When a valid `.localify` hostname arrives **without** a registration, Localify can still mint a temporary cert on the fly using its local CA, so users see a proper app error page instead of a raw browser TLS failure ([src/daemon/tls.rs](../../src/daemon/tls.rs), [src/daemon/tls.rs](../../src/daemon/tls.rs), [src/daemon/tls.rs](../../src/daemon/tls.rs)).

## 7. Control plane and reload

The daemon exposes a JSON-over-lines Unix socket for `status`, `list` and `reload` ([src/daemon/mgmt_socket.rs](../../src/daemon/mgmt_socket.rs)). The CLI and the desktop app use it to drive the service without touching the proxy layer directly.

Reload rebuilds runtime state without a full restart, keeping a clean separation between persisted config and live service state ([src/daemon/server.rs](../../src/daemon/server.rs)).

## 8. Key product features

- **Projects.** Localify doesn't just manage isolated domains — it has a real project abstraction with `root_path`, associated domains, a reachability target, and project-level actions.
- **Local bootstrap.** `localify init-project` can create the project, its domains, store its root path, and apply useful local updates like dev scripts or certain `next.config` tweaks ([src/cli/init_project.rs](../../src/cli/init_project.rs), [src/project_init.rs](../../src/project_init.rs)).
- **Logs.** The desktop app has a dedicated logs/requests view — not a raw tail — with per-project filtering ([desktop-ui/src/components/shared/shared-navigation.constants.ts](../../desktop-ui/src/components/shared/shared-navigation.constants.ts)).
- **Built-in troubleshooting.** Trust setup, shell-wide CLI install, Node trust repair, port-conflict detection, etc. ([desktop-ui/src/components/app/settings/settings-screen.tsx](../../desktop-ui/src/components/app/settings/settings-screen.tsx)).
- **Node trust onboarding.** Localify isn't limited to browser trust: it writes a shell snippet and configures `launchctl` so Node picks up the local CA via `NODE_USE_SYSTEM_CA=1` and `NODE_EXTRA_CA_CERTS`, avoiding `UNABLE_TO_VERIFY_LEAF_SIGNATURE` errors in Next/Node ([src/desktop/node_trust.rs](../../src/desktop/node_trust.rs)).
- **Docker integration.** When enabled, Localify discovers Compose containers, derives domains, picks the host port, and injects those routes as `external` source — no manual `register` calls ([docs/docker.md](../../docs/docker.md), [src/domain/registration.rs](../../src/domain/registration.rs)).
- **Real desktop app.** Beyond a UI veneer, the desktop exposes install/uninstall, daemon status, domains/routes, logs, CLI access, and packaging/update flow ([docs/desktop.md](../../docs/desktop.md)).

## 9. In one sentence

Localify is essentially a **full-stack local reverse proxy**: DNS resolution, trusted local TLS, host + path routing, HTTP/WebSocket forwarding, and local project management — all exposed through a CLI and a desktop app.
