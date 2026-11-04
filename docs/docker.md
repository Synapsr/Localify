# Docker Integration

Localify can automatically discover Docker Compose services and
register them as `.localify` domains. When enabled, starting a
compose stack instantly makes services available at
`https://<service>.<project>.localify`.

## Enabling Docker Integration

Add the `[docker]` section to `config.toml`
(`~/Library/Application Support/Localify Desktop/` on macOS, `/etc/localify/` on Linux):

```toml
[docker]
enabled = true
```

Then restart the daemon:

```bash
localify restart
```

On Linux, use `sudo`. Localify connects to the Docker socket and watches for container
lifecycle events. When a container starts or stops, Localify
automatically updates its routing table.

## How Auto-Discovery Works

When a container starts, Localify evaluates it using these rules
(in order):

1. **`localify.enable=false`** label -- skip (explicit opt-out)
2. **`localify.enable=true`** label -- register (explicit opt-in)
3. **Compose labels + exposed port** -- register automatically
4. **Otherwise** -- skip

For compose services, the domain is derived from the project
and service names: `<service>.<project>.localify`. For example,
a service named `web` in project `myapp` becomes
`web.myapp.localify`.

### Requirements for Auto-Discovery

- The container must have at least one **published port**
  (`ports:` mapping in `docker-compose.yml`)
- Localify proxies to the **host port**, not the container port
- If a container exposes multiple ports, set `localify.port` to
  pick one (see [Labels Reference](#labels-reference))

## Quick Start

Given this `docker-compose.yml`:

```yaml
services:
  web:
    build: .
    ports:
      - "3000:3000"
```

```bash
# Enable Docker integration in Localify config
# (add [docker] enabled = true, then restart)

# Start your compose stack
docker compose up -d

# Localify auto-discovers and registers web.myproject.localify
# (project name comes from directory name by default)
open https://web.myproject.localify
```

No `localify register` needed -- it happens automatically.

## Labels Reference

Control Localify behavior with container labels, either in
`docker-compose.yml` or via `docker run --label`.

| Label | Values | Description |
| ----- | ------ | ----------- |
| `localify.enable` | `true` / `false` | Force opt-in or opt-out |
| `localify.domain` | e.g. `app.localify` | Override the auto-generated domain |
| `localify.port` | e.g. `8080` | Pick which container port to proxy |
| `localify.wildcard` | `true` | Register as wildcard (`*.domain`) |

### Examples

**Custom domain:**

```yaml
services:
  web:
    build: .
    ports:
      - "3000:3000"
    labels:
      localify.domain: "myapp.localify"
```

**Explicit opt-in (non-compose container):**

```yaml
services:
  standalone:
    image: nginx
    ports:
      - "8080:80"
    labels:
      localify.enable: "true"
      localify.domain: "nginx.localify"
```

**Multiple ports -- pick one:**

```yaml
services:
  api:
    build: .
    ports:
      - "3000:3000"
      - "9090:9090"
    labels:
      localify.port: "3000"
```

**Wildcard subdomains:**

```yaml
services:
  web:
    build: .
    ports:
      - "3000:3000"
    labels:
      localify.wildcard: "true"
```

This registers `*.web.myproject.localify`, so
`anything.web.myproject.localify` routes to the container.

**Opt out a service:**

```yaml
services:
  db:
    image: postgres
    ports:
      - "5432:5432"
    labels:
      localify.enable: "false"
```

## Domain Name Resolution

Localify determines the domain in this order:

1. **`localify.domain` label** -- used as-is (must end with `.localify`)
2. **Compose labels** -- `<service>.<project>.localify`

The compose project name defaults to the directory name. You
can set it explicitly with `COMPOSE_PROJECT_NAME` or the
`name:` key in `docker-compose.yml`:

```yaml
name: myapp

services:
  web:
    build: .
    ports:
      - "3000:3000"
# Domain: web.myapp.localify
```

## Container-to-Localify Communication

When one container needs to reach another container's
`.localify` domain (or any `.localify` domain served by the host),
Docker's default DNS won't resolve `.localify` names. Use
`extra_hosts` to point the domain at the host:

```yaml
services:
  web:
    build: .
    ports:
      - "3000:3000"

  worker:
    build: .
    extra_hosts:
      - "web.myproject.localify:host-gateway"
```

`host-gateway` resolves to the host machine's IP (typically
`host.docker.internal` on Docker Desktop). The `worker`
container can now reach `http://web.myproject.localify` through
Localify on the host.

Add one entry per `.localify` domain the container needs to
access.

> **Note:** Wildcard `extra_hosts` (e.g., `*.localify`) are not
> supported -- `/etc/hosts` does not allow wildcards. You must
> list each domain explicitly.

### Linux vs macOS

On **Linux**, Docker containers share the host network
namespace (or use a bridge with direct host access). DNS
resolution and HTTP connectivity work without `extra_hosts`
in most setups.

On **macOS**, Docker Desktop runs containers inside a Linux
VM. Containers cannot reach the host via its LAN IP, but can
reach it via `host-gateway` / `host.docker.internal`. The
`extra_hosts` approach above is required for container-to-Localify
communication. Localify listens on loopback by default there. If a
container cannot reach it, run `localify lan enable`, which lets
Localify listen on every interface.

## Monitoring

Check which Docker containers Localify has discovered:

```bash
localify list
```

Docker-discovered domains show their source as "external" in
the output. They appear alongside manually registered domains
but cannot be edited with `localify register` or `localify route` --
they are managed entirely by the Docker watcher.

View discovery logs:

```bash
# See container add/remove events
LOCALIFY_LOG=debug localify start --foreground

# Or check the log file
localify logs -f
```

Example log output:

```text
INFO Docker integration enabled
INFO Docker domain added domain=web.myapp.localify target=127.0.0.1:3000
INFO Docker reconciliation complete added=1 removed=0 total=1
```

## Troubleshooting

### Container Not Discovered

Check that:

1. Docker integration is enabled (`[docker] enabled = true`)
2. The container has published ports (`ports:` in compose)
3. The container is not opted out (`localify.enable=false`)
4. The daemon is running (`localify status`)

Run with debug logging to see why a container was skipped:

```bash
LOCALIFY_LOG=debug localify start --foreground
```

Look for `Docker container skipped` messages with a reason.

### "No published host port" Error

Localify needs a host port mapping to proxy traffic. Make sure
your service has a `ports:` entry:

```yaml
services:
  web:
    build: .
    # This is required:
    ports:
      - "3000:3000"
    # EXPOSE alone is not enough
```

### Multiple Ports Without Label

If a container exposes more than one port and no `localify.port`
label is set, Localify skips it (ambiguous). Add the label:

```yaml
labels:
  localify.port: "3000"
```

### Docker Socket Permission

Localify connects to the Docker socket
(`/var/run/docker.sock` by default). On macOS the daemon runs as you, so you need
access to that socket, which Docker Desktop gives you. On Linux a daemon started with
`sudo` has it. If you see connection errors, verify the socket exists and is accessible.
