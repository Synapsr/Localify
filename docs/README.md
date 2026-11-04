# Localify

Localify is a local development proxy for macOS and Linux that
gives your projects custom `.localify` domains with automatic
HTTPS. It ships as a single binary with no external
dependencies. Register a domain, point it at a port or
directory, and open `https://myapp.localify` in your browser.

> **Linux users:** See [linux.md](linux.md) for
> platform-specific setup details and troubleshooting.

## Quick Start

```bash
# Initial setup: creates the Root CA, configures DNS, trusts the
# certificate in the system trust store, and trusts booted iOS
# Simulators when available. On macOS it also starts the daemon.
localify install

# Register a domain that proxies to localhost:3000
localify register myapp.localify --route "/=3000"

# Open in browser
open https://myapp.localify        # macOS
xdg-open https://myapp.localify   # Linux
```

On Linux, prefix `install`, `register`, `start` and the other commands that change
the setup with `sudo`, and run `sudo localify start` after registering.

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
| `localify stop`                    | Stop daemon            |
| `localify restart`                 | Restart daemon         |
| `localify reload`                  | Reload configuration   |
| `localify status`                  | Show daemon status     |
| `localify logs [-n N] [-f]`        | View or follow logs    |
| `localify lan enable\|disable\|status` | Let other devices connect, or not |
| `localify dev [service] --profile <name>` | Run a service on a [launch profile](launch-profiles.md) |
| `localify profile init`            | Set up launch profiles interactively |
| `localify exec --profile <name> -- <cmd>` | Run a command (not a service) on a launch profile |
| `localify completions <shell>`     | Generate completions   |

On macOS none of these need `sudo`, and Localify refuses to run as root.
`install`, `uninstall` and `lan` ask for an administrator's approval when they have
to change the system. On Linux, commands that change the system setup or control
the daemon need `sudo`.

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

On Linux:

```text
/etc/localify/
├── config.toml          # Main configuration
├── ca.key               # Root CA private key
├── ca.crt               # Root CA certificate
└── certs/
    ├── <domain>.key     # Per-domain private key
    └── <domain>.crt     # Per-domain certificate

/var/run/localify.pid        # PID file (when daemon runs)

/var/log/localify/
└── localify.log             # Daemon log file
```

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

## Auto-Start on Boot

### macOS

`localify install` sets this up. launchd starts the daemon at boot, before anyone
signs in, and restarts it if it crashes. `localify stop` keeps it stopped, including
across a reboot, until `localify start`.

### Linux (systemd)

Create a systemd service file:

```bash
sudo tee /etc/systemd/system/localify.service > /dev/null <<'EOF'
[Unit]
Description=Localify local development proxy
After=network.target

[Service]
Type=simple
ExecStart=/usr/local/bin/localify start --foreground
Restart=on-failure

[Install]
WantedBy=multi-user.target
EOF

sudo systemctl daemon-reload
sudo systemctl enable --now localify
```

## Daemon: Foreground vs Background

On macOS launchd runs the daemon, and these commands steer it without any prompt:

- `localify stop` asks it to exit and leaves a marker file. launchd still holds the
  ports and wakes the job on the next connection, so while the marker exists the job
  closes that connection and exits again. Names stop resolving, and no process stays
  behind.
- `localify start` removes the marker and opens a connection, which wakes the job.
- `localify restart` asks it to exit with a code launchd restarts on. An update needs
  no command: the daemon notices its binary was replaced and does the same.

launchd holds the ports, so outside it a daemon can only use ports from 1024 up on
loopback. macOS lets a normal user listen on a low port on all interfaces but not on
loopback.

**Background** (Linux, and the macOS dev profile): forks to the background,
writes a PID file, logs to `/var/log/localify/localify.log`:

```bash
sudo localify start
```

**Foreground**: stays in the terminal, logs to stdout,
stop with Ctrl+C. Useful for debugging:

```bash
sudo localify start --foreground
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
`/etc/localify/config.toml` on Linux (override with `--config`).

### Daemon Section

```toml
[daemon]
http_port = 80
https_port = 443
dns_port = 1053
log_level = "info"
lan_access = false
```

All three ports must be different. On macOS launchd opens the HTTP and HTTPS ports
for the daemon, so changing them needs one approval: use the desktop app's settings
or run `localify install` after editing the file. On Linux the daemon needs `sudo`
to bind to ports below 1024.

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
data_dir = "/etc/localify"
pid_file = "/var/run/localify.pid"
log_file = "/var/log/localify/localify.log"
certs_dir = "/etc/localify/certs"
```

The values above are the Linux defaults. You only need this
section if you want different locations.

## Access From Other Devices

On macOS only your Mac can reach Localify by default: the proxy and its DNS listen on
loopback, and the DNS server answers `127.0.0.1` and `::1`.

To test from a phone or another computer, run `localify lan enable`, which rewrites
the launchd plist with one approval. Localify then listens on every interface and its
DNS answers other devices with this Mac's LAN address. `localify lan disable` takes
that back, and `localify lan status` tells you where you are. Do this on networks you
trust: anyone on the network can then reach your dev servers.

The opt-in fails before asking for anything if another program already listens on
every interface on that port, as Tailscale can on 443.

On Linux the daemon listens on every interface as before, because Docker containers
reach the host through its bridge address.

## HTTPS Trust That Is Still Pending

macOS needs a confirmation on screen to trust a certificate authority, even with
`sudo`, so on a machine with no screen `localify install` finishes everything else and
reports trust as pending. HTTP works, and so does HTTPS from Node.js through
`NODE_EXTRA_CA_CERTS`. Browsers warn until the CA is trusted. `localify status` prints
the exact command to run from a session with a screen.

The CA only signs for the `.localify` suffix, and clients enforce that through a name
constraint in the certificate, so its key cannot be used to impersonate another site.

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
