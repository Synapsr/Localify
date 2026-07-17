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
# Initial setup — installs Root CA, configures DNS,
# trusts the certificate in the system trust store,
# and trusts booted iOS Simulators when available
sudo localify install

# Register a domain that proxies to localhost:3000
sudo localify register myapp.localify --route "/=3000"

# Start the daemon (requires sudo for ports 80/443)
sudo localify start

# Open in browser
open https://myapp.localify        # macOS
xdg-open https://myapp.localify   # Linux
```

## Commands Reference

| Command                            | Description            |
| ---------------------------------- | ---------------------- |
| `sudo localify install`                | Initial setup          |
| `sudo localify uninstall [--force]`    | Full cleanup           |
| `sudo localify register <domain> ...`  | Register domain        |
| `sudo localify register --wildcard ..` | Register wildcard      |
| `sudo localify unregister <domain>`    | Remove domain          |
| `localify list`                        | Show all domains       |
| `sudo localify route add ...`          | Add route to domain    |
| `localify route remove ...`            | Remove route           |
| `localify route list <domain>`         | List routes for domain |
| `sudo localify start [--foreground]`   | Start daemon           |
| `sudo localify stop`                   | Stop daemon            |
| `sudo localify restart`                | Restart daemon         |
| `sudo localify reload`                 | Reload configuration   |
| `localify status`                      | Show daemon status     |
| `localify logs [-n N] [-f]`            | View or follow logs    |
| `localify completions <shell>`         | Generate completions   |

**Note:** Commands that modify system configuration
(CA certs, DNS) or control the daemon (runs on ports
80/443) require `sudo`.

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
LOCALIFY_LOG=debug sudo localify start --foreground
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

### macOS (Homebrew)

If you installed Localify via Homebrew, use `brew services`
to start it automatically at boot:

```bash
# Start now and auto-start at boot
sudo brew services start localify

# Stop auto-start
sudo brew services stop localify
```

When managed by `brew services`, Localify runs in foreground
mode and launchd handles process supervision.

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

**Background** (default) — forks to the background,
writes a PID file, logs to `/var/log/localify/localify.log`:

```bash
sudo localify start
```

**Foreground** — stays in the terminal, logs to stdout,
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

1. **Environment variable** —
   `LOCALIFY_LOG=debug sudo localify start`
2. **CLI flag** — `sudo localify start --verbose`
   (sets debug level)
3. **Config file** — edit `/etc/localify/config.toml`:

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
| `-c`, `--config <PATH>` | `/etc/localify/config.toml` | Config file |
| `-v`, `--verbose` | off | Enable debug output |

Example using a custom config:

```bash
sudo localify -c /opt/localify/config.toml start
```

## Configuration

The configuration lives in `/etc/localify/config.toml`
(override with `--config`).

### Daemon Section

```toml
[daemon]
http_port = 80
https_port = 443
dns_port = 1053
log_level = "info"
```

All three ports must be different. The daemon needs
`sudo` to bind to ports below 1024.

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

The values above are the defaults. You only need this
section if you want different locations.

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

**Solution:** Restart your browser completely after running `sudo localify install`.
Browsers cache the trusted certificate list at startup.

**iOS Simulator:** On macOS, `localify install` also adds the Root CA to any
booted iOS Simulator via `xcrun simctl keychain`. If no iOS Simulator was
booted during installation, boot the simulator and run `sudo localify install`
again before testing HTTPS there.

**Linux with snap browsers:** Snap-packaged browsers (Firefox, Chromium)
are sandboxed and cannot access the system trust store. See the
[Linux guide](linux.md#snap-browsers-and-certificate-trust) for a one-time
fix using `certutil`.

### Certificates Show Wrong Domain Name

If accessing `myapp.localify` shows a certificate for a different domain, the daemon
needs to be restarted to pick up newly registered domains.

**Solution:** Run `sudo localify restart` after registering new domains.

### "Connection Refused" or "This site can't be reached"

Check if the daemon is running:

```bash
localify status
```

If it's not running, start it:

```bash
sudo localify start
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
different ports in `/etc/localify/config.toml`.

### Backend Service Not Responding

Make sure your backend service is actually running on the port you configured:

```bash
# Test if your service is listening
curl http://localhost:3000

# If that works but https://myapp.localify doesn't, check Localify's logs
localify logs -f
```
