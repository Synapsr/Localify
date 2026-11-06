# Localify

Localify is a local development proxy for macOS and Linux that
gives your projects custom `.localify` domains with automatic
HTTPS. It ships as a single binary with no external
dependencies. Register a domain, point it at a port or
directory, and open `https://myapp.localify` in your browser.

> **Linux users:** See [linux.md](linux.md) for
> platform-specific setup details and troubleshooting.

## Quick Start

Install the command line on macOS (Apple Silicon) or Linux (x86_64), then set it up:

```bash
curl -fsSL https://localify.dev/install.sh | sh
```

The script downloads the latest release from GitHub, checks its SHA-256, puts `localify`
in `~/.local/bin`, then runs `localify install`, which asks for your administrator
password once. Run it again to update: the background service restarts on the new
binary. Over SSH, keep a terminal so `sudo` can ask:
`ssh -t <host> 'curl -fsSL https://localify.dev/install.sh | sh'`.
`LOCALIFY_VERSION`, `LOCALIFY_INSTALL_DIR` and `LOCALIFY_NO_SETUP=1` change what it does;
the [script](../apps/web/public/install.sh) says how. On a Mac you can also use the app,
<https://downloads.localify.dev/latest/localify.dmg>, which updates itself; the script
leaves a Mac with the app alone.

```bash
# Initial setup, which the script runs for you: creates the Root CA, configures
# DNS, trusts the certificate in the system trust store, and trusts booted iOS
# Simulators when available. On macOS it also starts the daemon.
localify install

# Register a domain that proxies to localhost:3000
localify register myapp.localify --route "/=3000"

# Open in browser
open https://myapp.localify        # macOS
xdg-open https://myapp.localify   # Linux
```

These work the same on Linux with systemd. With no systemd, for example in a container,
run them as root, and start the daemon yourself with `localify start`.

## Commands Reference

| Command                            | Description            |
| ---------------------------------- | ---------------------- |
| `localify install`                 | Initial setup          |
| `localify uninstall [--force]`     | Full cleanup           |
| `localify register <domain> ...`   | Register domain        |
| `localify register --wildcard ..`  | Register wildcard      |
| `localify unregister <domain>`     | Remove domain          |
| `localify list`                    | Show all domains       |
| `localify route add ...`           | Add route to domain    |
| `localify route remove ...`        | Remove route           |
| `localify route list <domain>`     | List routes for domain |
| `localify start [--foreground]`    | Start daemon           |
| `localify stop`                    | Stop daemon (without a service) |
| `localify restart`                 | Restart daemon (without a service) |
| `localify reload`                  | Reload configuration   |
| `localify status`                  | Show daemon status     |
| `localify logs [-n N] [-f]`        | View or follow daemon logs |
| `localify lan enable\|disable\|status` | Let other devices connect, or not |
| `localify dev [service] --profile <name>` | Run a service on a [launch profile](launch-profiles.md) |
| `localify profile init`            | Set up launch profiles interactively |
| `localify exec --profile <name> -- <cmd>` | Run a command (not a service) on a launch profile |
| `localify dev [service] --in-app`  | Run a service in the app's terminal |
| `localify dev [service] --detach`  | Run a service in the background |
| `localify services status`         | Show the services Localify started |
| `localify logs <service> [-n N] [-f] [--cursor C]` | Print a service's output |
| `localify stop <service>`          | Stop a service Localify started |
| `localify restart <service>`       | Restart a service where it ran |
| `localify project update <project> ...` | Rename, enable, disable, or re-link a project |
| `localify project domain attach\|detach <domain>` | Move a domain into or out of a project |
| `localify project env remove <env> [--delete-worktree]` | Remove a worktree environment, and the worktree |
| `localify project env prune`       | Remove environments whose folder is gone |
| `localify login` / `logout` / `whoami` | Sign this machine in to Localify Cloud with GitHub, or out |
| `localify public-url create <domain> ...` | Give a project domain a [public URL](#public-urls) |
| `localify public-url list\|remove\|protect\|unprotect\|claim` | Manage your public URLs |
| `localify agent install`           | Connect coding agents to Localify's MCP server |
| `localify mcp`                     | The MCP server agents start |
| `localify completions <shell>`     | Generate completions   |

None of these need `sudo`, and Localify refuses to run under it. `install`, `uninstall`
and `lan` ask for an administrator's approval when they have to change the system. The
one exception is Linux with no systemd, where the system setup needs root.

## Coding Agents

`localify mcp` is an MCP server that gives Claude Code, Codex, and Cursor every Localify
action as a tool: starting and stopping services, reading their output, projects,
domains, routes, worktree environments, and the daemon. Each tool runs the CLI command of
the same name, so an agent gets exactly what you get in a terminal. Because the server is
the installed binary, agents always see the actions of the installed version.

```bash
localify agent install   # connect every agent installed here
localify agent status    # see which ones are connected
```

`agent install` uses `claude mcp add` and `codex mcp add`, and edits
`~/.cursor/mcp.json` for Cursor. It also removes the Localify skill earlier versions
installed: the copies Localify made and the links to them, never a skill or link of your
own. Restart open agent sessions afterwards.

You rarely need to run it. `localify install` and the desktop app's setup connect the
agents installed at that time, and after an update the app connects them once, with a
message saying so. **Settings → AI agents** has the switch to turn this off and the
**Connect agents** button for an agent installed later.

### Services in the app, from the command line

`localify dev --in-app` starts a service in the desktop app's terminal, as its launch
button does: the app opens if needed, and the command prints the URL and returns once the
service listens. If the service already runs, it says so and starts nothing. Without the
app, `--detach` runs it in the background instead.

The app's terminal also writes what it shows to a file, so everyone reads the same lines:

```bash
localify dev web --in-app          # add --env feat-login for a worktree
localify logs web -n 100           # the app terminal's lines, without colors
localify logs web --follow         # until the service stops
localify services status           # running or not, PID, URL, port, since, exit code
localify restart web               # stops it and starts it again where it ran
localify stop web
```

`stop`, `restart` and `logs` work on any `localify dev`, in the app, in the background, or
in a terminal; only the output of a service run in your own terminal stays in that
terminal. Without a service, `stop`, `restart` and `logs` act on the daemon, as before.
When `logs` writes to something other than a terminal, it ends with `Next cursor: N`;
`--cursor N` then prints only the lines that came after. Agents use this to follow a
service.

The link `localify://action/launch-service?project=<id>&env=<slug>&service=<id>`, with an
optional `&profile=<name>`, does the same as `--in-app`.

### Cleaning up worktrees

Running `localify dev` inside a Git worktree gives it its own environment: domains,
certificates, and ports. When the work there is done, remove it from another folder:

```bash
localify project env remove feat-login --delete-worktree
```

This stops the environment's services, deletes the worktree with `git worktree remove`,
and unregisters its domains. Git refuses when the worktree has uncommitted changes, and
nothing is removed; `--force` discards them. The branch is kept. Without
`--delete-worktree`, the folder stays and only Localify's side goes. For a worktree
deleted another way, `localify project env prune` removes the environments whose folder
is gone.

## Route Targets

Routes map a URL path prefix to a target. The format
is `PATH=TARGET`.

**Port** — proxy to `127.0.0.1` on the given port:

```bash
localify register app.localify --route "/=3000"
```

**Host and port** — proxy to a specific address:

```bash
localify register app.localify --route "/=192.168.1.50:3000"
```

**Directory** — serve static files from disk:

```bash
localify register app.localify --route "/=/var/www/html"
```

**Multiple routes** — combine targets on one domain.
The longest matching prefix wins:

```bash
localify register app.localify \
  --route "/=3000" \
  --route "/api=3001"
```

You can also manage routes after registration:

```bash
localify route add app.localify /webhooks 9000
localify route remove app.localify /webhooks
localify route list app.localify
```

## Reverse Proxy Behavior

When forwarding requests to a backend service, Localify
sets standard proxy headers and strips hop-by-hop headers
per [RFC 7230 §6.1][rfc7230].

[rfc7230]: https://www.rfc-editor.org/rfc/rfc7230#section-6.1

### Forwarding Headers

Localify adds these headers to every proxied request so your
backend knows the original client details:

| Header | Value |
| ------ | ----- |
| `X-Forwarded-Host` | Original `Host` header from the client |
| `X-Forwarded-Proto` | `http` or `https` |
| `X-Forwarded-For` | Client IP (appended to existing chain) |

Most frameworks use these automatically. For example,
Django reads `X-Forwarded-Proto` to decide whether to
generate `https://` URLs, and Rails uses
`X-Forwarded-Host` for routing.

### Launch Profile Header

When a service was started with a
[launch profile](launch-profiles.md)
(`localify dev --profile <name>`), Localify adds
`X-Localify-Profile: <name>` to its responses, so browser
dev tools and API clients show which variables the
service runs on. For dangerous profiles it also adds a
same-origin `<script src="/.localify/launch-profile.js">`
to HTML pages, which prefixes the tab title and shows a
draggable indicator.

### Hop-by-Hop Header Stripping

Localify removes the following hop-by-hop headers from both
the forwarded request and the backend response:

`Connection`, `Keep-Alive`, `Proxy-Authenticate`,
`Proxy-Authorization`, `TE`, `Trailer`,
`Transfer-Encoding`, `Upgrade`, plus any headers
listed in the `Connection` header value.

These headers describe the connection between two
adjacent nodes (client↔proxy or proxy↔backend) and
must not leak across hops.

### Debugging Proxy Headers

Enable debug logging to see the forwarding headers
Localify sets on each request
(see [Logging and Verbosity](#logging-and-verbosity)):

```bash
LOCALIFY_LOG=debug localify start --foreground
```

```text
DEBUG Forwarding headers set x_forwarded_host=myapp.localify
  x_forwarded_proto=https x_forwarded_for=127.0.0.1
DEBUG Proxying HTTP request target=127.0.0.1:3000
```

## Wildcard Subdomains

Register a domain with `--wildcard` to match the base
domain **and** any single-level subdomain. Localify generates
a wildcard TLS certificate so every subdomain gets
trusted HTTPS automatically.

```bash
localify register myapp.localify --wildcard --route "/=3000"
```

This single registration handles all of these:

| URL | Matches? |
| --- | -------- |
| `https://myapp.localify` | yes |
| `https://blog.myapp.localify` | yes |
| `https://api.myapp.localify` | yes |
| `https://a.b.myapp.localify` | no (multi-level) |
| `https://other.localify` | no (different domain) |

### Combining Exact and Wildcard

You can register both an exact domain and a wildcard
for the same base domain. The exact registration takes
priority when both match:

```bash
# Exact: dedicated routes for the base domain
localify register myapp.localify --route "/=3000"

# Wildcard: catch-all for subdomains
localify register myapp.localify --wildcard --route "/=4000"

# myapp.localify        → port 3000 (exact wins)
# blog.myapp.localify   → port 4000 (wildcard)
# api.myapp.localify    → port 4000 (wildcard)
```

### Managing Wildcard Routes

Use `--wildcard` with `route` subcommands to manage
routes on a wildcard registration:

```bash
localify route add --wildcard myapp.localify /api 3001
localify route remove --wildcard myapp.localify /api
localify route list --wildcard myapp.localify
```

### Unregistering a Wildcard

```bash
localify unregister --wildcard myapp.localify
```

This removes the wildcard registration and its
certificate. Any exact registration for the same domain
is left untouched.

### Configuration

Wildcard registrations are stored in `config.toml`
with a `*.` prefix on the domain:

```toml
[domains.wildcard-myapp-localify]
domain = "*.myapp.localify"
https_enabled = true

[[domains.wildcard-myapp-localify.routes]]
path = "/"
target = "127.0.0.1:3000"
```

## Static File Serving

When serving static files from a directory, Localify provides:

**Index file support** — if a directory contains `index.html`,
it's served automatically:

```bash
# Directory structure:
# /var/www/mysite/
# ├── index.html
# ├── about/
# │   └── index.html
# └── assets/
#     └── style.css

localify register site.localify --route "/=/var/www/mysite"

# Behavior:
# https://site.localify          → serves /var/www/mysite/index.html
# https://site.localify/about/   → serves /var/www/mysite/about/index.html
# https://site.localify/assets/  → shows file browser (no index.html)
```

**File browser** — directories without `index.html` display an
automatic directory listing, making it easy to browse files and
navigate subdirectories

## Files and Directories

On macOS, everything of yours lives in your user folder:

```text
~/Library/Application Support/Localify Desktop/
├── config.toml          # Main configuration
├── localify.pid         # PID file (when daemon runs)
├── localify.sock        # Management socket, yours only
├── localify.stopped     # Present while you have the daemon stopped
├── data/
│   ├── ca.key           # Root CA private key, 0600
│   ├── ca.crt           # Root CA certificate
│   └── certs/
│       ├── <domain>.key # Per-domain private key, 0600
│       └── <domain>.crt # Per-domain certificate
└── logs/
    └── localify.log     # Daemon log file
```

The one file outside it is `/Library/LaunchDaemons/dev.localify.daemon.plist`, which
tells launchd to open the ports and run the daemon as you.

On Linux with systemd:

```text
~/.config/localify/
├── config.toml          # Main configuration
├── localify.pid         # PID file (when daemon runs)
├── localify.sock        # Management socket
├── data/
│   ├── ca.key           # Root CA private key (mode 600)
│   ├── ca.crt           # Root CA certificate
│   └── certs/
│       ├── <domain>.key # Per-domain private key
│       └── <domain>.crt # Per-domain certificate
└── logs/
    └── localify.log     # Daemon log file
```

The files outside it are `localify.service`, `localify-http.socket` and
`localify-https.socket` in `/etc/systemd/system/`, which tell systemd to open the ports
and run the daemon as you.

Run as root, or on a machine with no systemd, Localify keeps its files in
`/etc/localify/`, `/var/run/localify.pid` and `/var/log/localify/`.

DNS configuration (created by `localify install`):

**macOS:**

```text
/etc/resolver/localify
```

**Linux (systemd-resolved):**

```text
/etc/systemd/resolved.conf.d/localify.conf
```

This tells the system to resolve all `*.localify` domains
through the local DNS server.

All paths are configurable via the `[paths]` section in
`config.toml` (see [Configuration](#configuration)).

Each project folder holds `.localify/project.json`, which names its project on this
machine, so Localify can find a project again after you move its folder. It belongs to
this machine only: Localify adds `**/.localify/project.json` to the repository's
`.git/info/exclude`, shared by its worktrees, so it is never committed. A marker committed
before that stays tracked; stop tracking it with
`git rm --cached .localify/project.json`.

## Auto-Start on Boot

### macOS

`localify install` sets this up. launchd starts the daemon at boot, before anyone
signs in, and restarts it if it crashes. `localify stop` keeps it stopped, including
across a reboot, until `localify start`.

### Linux (systemd)

The same. `localify install` writes the units, and systemd starts the daemon at boot as
you and restarts it if it crashes. `localify stop` stays stopped across a reboot.

## Daemon: Foreground vs Background

launchd on macOS and systemd on Linux run the daemon, and these commands steer it
without any prompt:

- `localify stop` asks it to exit and leaves a marker file. The service manager still
  holds the ports and wakes the daemon on the next connection, so while the marker
  exists the daemon closes that connection and exits again. Names stop resolving, and
  no process stays behind.
- `localify start` removes the marker and opens a connection, which wakes the daemon.
- `localify restart` asks it to exit with a code the manager restarts on. An update
  needs no command: the daemon notices its binary was replaced and does the same.

The manager holds the ports, so outside it a daemon can only use ports from 1024 up on
loopback.

**Background** (no systemd, and the dev profile): forks to the background and writes a
PID file. As root it logs to `/var/log/localify/localify.log`:

```bash
localify start
```

**Foreground**: stays in the terminal, logs to stdout,
stop with Ctrl+C. Useful for debugging:

```bash
localify start --foreground
```

## Logging and Verbosity

View logs:

```bash
localify logs              # last 50 lines
localify logs -n 100       # last 100 lines
localify logs -f           # follow (like tail -f)
localify logs --clear      # clear the log file
```

Change the log level (highest priority first):

1. **Environment variable**:
   `LOCALIFY_LOG=debug localify start --foreground`
2. **CLI flag**: `localify start --verbose`
   (sets debug level)
3. **Config file**: under launchd this is the way. Edit `config.toml`, then run
   `localify restart`:

   ```toml
   [daemon]
   log_level = "debug"
   ```

4. **Default** — `info`

Available levels: `error`, `warn`, `info`, `debug`.

## Shell Completions

Generate tab completions for your shell with
`localify completions <shell>`, then restart the shell.

**Fish:**

```bash
localify completions fish > ~/.config/fish/completions/localify.fish
```

**Zsh:**

```bash
localify completions zsh > ~/.zfunc/_localify
```

Make sure `~/.zfunc` is in your fpath. Add to `.zshrc`:

```bash
fpath=(~/.zfunc $fpath)
autoload -Uz compinit && compinit
```

**Bash:**

```bash
localify completions bash \
  > ~/.local/share/bash-completion/completions/localify
```

After setup, press `Tab` to complete commands, options,
and arguments.

## Global Options

All commands accept these global flags:

| Flag | Default | Description |
| ---- | ------- | ----------- |
| `-c`, `--config <PATH>` | [see Configuration](#configuration) | Config file |
| `-v`, `--verbose` | off | Enable debug output |

Example using a custom config:

```bash
localify -c /opt/localify/config.toml start
```

## Configuration

The configuration lives in
`~/Library/Application Support/Localify Desktop/config.toml` on macOS and in
`~/.config/localify/config.toml` on Linux (`/etc/localify/config.toml` as root). Override
it with `--config`.

### Daemon Section

```toml
[daemon]
http_port = 80
https_port = 443
dns_port = 1053
log_level = "info"
lan_access = false
```

All three ports must be different. launchd or systemd opens the HTTP and HTTPS ports for
the daemon, so changing them needs one approval: use the desktop app's settings or run
`localify install` after editing the file.

`lan_access` decides who can connect. See
[Access From Other Devices](#access-from-other-devices).

### Domain Sections

Each registered domain gets its own section:

```toml
[domains.myapp-localify]
domain = "myapp.localify"
https_enabled = true

[[domains.myapp-localify.routes]]
path = "/"
target = "127.0.0.1:3000"

[[domains.myapp-localify.routes]]
path = "/api"
target = "127.0.0.1:3001"
```

Domain names must end with `.localify` and can contain
letters, numbers, hyphens, and dots (for subdomains).
Wildcard registrations use a `*.` prefix
(see [Wildcard Subdomains](#wildcard-subdomains)).

### Paths Section

Override where Localify stores its data:

```toml
[paths]
data_dir = "/home/me/.config/localify/data"
pid_file = "/home/me/.config/localify/localify.pid"
log_file = "/home/me/.config/localify/logs/localify.log"
certs_dir = "/home/me/.config/localify/data/certs"
```

The values above are the defaults on Linux with systemd. You only need this section if
you want different locations.

## Access From Other Devices

On macOS only your Mac can reach Localify by default: the proxy and its DNS listen on
loopback, and the DNS server answers `127.0.0.1` and `::1`.

To test from a phone or another computer, run `localify lan enable`, which rewrites
the launchd plist or systemd units with one approval. Localify then listens on every interface and its
DNS answers other devices with this Mac's LAN address. `localify lan disable` takes
that back, and `localify lan status` tells you where you are. Do this on networks you
trust: anyone on the network can then reach your dev servers.

The opt-in fails before asking for anything if another program already listens on
every interface on that port, as Tailscale can on 443.

Run as root on Linux, or with no systemd, the daemon listens on every interface, as it
does in a container. Set `lan_access = false` to keep it on this machine.

## Public URLs

A public URL opens a project domain of this machine from anywhere, such as your
laptop or phone on another network: `https://<slug>.localify.fyi` reaches
`web.journal.localify` on the machine that serves it. It needs a Localify Cloud
account. Accounts are invite-only for now.

```bash
localify login                                   # shows a code to enter on github.com
localify public-url create web.journal.localify  # prints the URL, its password, and a link with it
localify public-url create journal.localify --wildcard
localify public-url list
```

- **Protected by default.** Each URL asks for a password, generated unless you pass
  `--password`. A browser that enters it stays signed in for 7 days on that host; each wildcard
  host asks once. The link with the password,
  `https://<slug>.localify.fyi/__localify/access#<password>`, signs a browser in
  without typing it; the machine that set the password keeps it to give that link
  again, in the app and in `public-url list --json`.
  `protect` sets a new password and signs every browser out. `unprotect`, or
  `--public` at creation, opens the URL to everyone.
- **Wildcard.** `--wildcard` on a project's namespace, such as `journal.localify`,
  reaches every service and environment under it: `<slug>--web.localify.fyi` opens
  `web.journal.localify`, and `<slug>--web--feature.localify.fyi` opens
  `web--feature.journal.localify`. Only domains of that project are reached.
- **One machine serves each URL.** `create` serves it from this machine. `claim`
  moves it here from another of your machines. The machine needs Localify running;
  it downloads Cloudflare's `cloudflared` on first use and runs it while it serves
  at least one URL.
- **What is never public:** domains outside a project, Docker domains, and
  Localify's own `/.localify/` pages.
- `remove` deletes a URL and keeps its slug for you for 30 days. `logout` takes
  this machine's URLs offline; after you sign in again, `claim` serves them again.

The desktop app does the same in Settings → Localify Cloud and in each domain's
Public URL section. Coding agents get the same tools over MCP, except signing in and
opening a URL to everyone, which stay yours: an agent always creates a protected URL
and hands you its password.

How it works, for contributors: [public-urls.md](public-urls.md).

### Apps That Check Origin

Through a public URL, your app gets the local host in `Host` and the public host in
`X-Forwarded-Host` ([ADR 0023](adr/0023-public-traffic-reaches-upstream-with-the-local-host.md)).
Writes such as a `POST` keep the browser's public `Origin`; only Next.js's own dev files
and hot reload get the local origin. An app that checks `Origin` against its own URL
therefore refuses them, as Better Auth does with `Invalid origin`. In development, trust
the public URLs too. With Better Auth 1.6:

```ts
trustedOrigins: [env.AUTH_URL, ...(isDev ? ["https://*.localify.fyi"] : [])],
```

or `BETTER_AUTH_TRUSTED_ORIGINS=https://*.localify.fyi`. For OAuth from the public URL,
give it a base URL that follows `X-Forwarded-Host`:
`baseURL: { allowedHosts: ["*.<project>.localify", "*.localify.fyi"], fallback: "https://<project>.localify" }`.

Trusting every `*.localify.fyi` origin is safe in development: Localify Cloud refuses
a write whose `Origin` belongs to another public URL before it reaches your machine
([public-urls.md](public-urls.md), "Public traffic", step 2).

## HTTPS Trust That Is Still Pending

On macOS the install leaves certificate trust for later, so that it asks for one password.
HTTP works right away, and so does HTTPS from Node.js through `NODE_EXTRA_CA_CERTS`.
Browsers warn until you click Trust HTTPS in the app or run `localify trust`, and macOS
asks you to confirm once. That confirmation needs a screen, so run it on the Mac itself.

On Linux with systemd the install trusts the CA, in the same `sudo`.

## Docker Integration

Localify can automatically discover Docker Compose services and
register them as `.localify` domains. Enable it in `config.toml`:

```toml
[docker]
enabled = true
```

Once enabled and the daemon is restarted, starting a compose
stack automatically registers domains like
`<service>.<project>.localify` with HTTPS. No manual
`localify register` needed.

See [docker.md](docker.md) for the full guide: labels
reference, container-to-Localify communication, troubleshooting,
and more.

## Troubleshooting

### Browser Shows "Not Secure" or Certificate Warnings

**If you installed Localify with your browser already open**, the browser won't
immediately pick up the newly trusted Root CA from the system trust store.

**Solution:** Restart your browser completely after running `localify install`.
Browsers cache the trusted certificate list at startup. If `localify status` says trust
is pending, run the command it prints.

**iOS Simulator:** On macOS, `localify install` and `localify dev` add the Root
CA to booted iOS Simulators via `xcrun simctl keychain`. If Safari still shows
a privacy warning, boot the simulator and use **Settings → Troubleshooting →
Trust booted simulators**, then fully quit and reopen Safari in the simulator.

**Linux with snap browsers:** Snap-packaged browsers (Firefox, Chromium)
are sandboxed and cannot access the system trust store. See the
[Linux guide](linux.md#snap-browsers-and-certificate-trust) for a one-time
fix using `certutil`.

### Certificates Show Wrong Domain Name

If accessing `myapp.localify` shows a certificate for a different domain, the daemon
needs to be restarted to pick up newly registered domains.

**Solution:** Run `localify restart` after registering new domains.

### "Connection Refused" or "This site can't be reached"

Check if the daemon is running:

```bash
localify status
```

If it's not running, start it:

```bash
localify start
```

Verify DNS is working:

```bash
# macOS
dig myapp.localify

# Linux
resolvectl query myapp.localify
```

### Port Already in Use

If Localify can't start because ports 80, 443, or 1053 are in use:

```bash
# Check what's using port 80 (HTTP)
sudo lsof -i :80

# Check what's using port 443 (HTTPS)
sudo lsof -i :443

# Check what's using port 1053 (DNS)
sudo lsof -i :1053
```

Stop the conflicting service or configure Localify to use
different ports in `config.toml`.

### Backend Service Not Responding

Make sure your backend service is actually running on the port you configured:

```bash
# Test if your service is listening
curl http://localhost:3000

# If that works but https://myapp.localify doesn't, check Localify's logs
localify logs -f
```
