<p align="center">
  <img src="desktop-ui/public/logo.png" alt="Localify" width="96" height="96">
</p>

<h1 align="center">Localify</h1>

<p align="center">
  Stable <code>https://&lt;name&gt;.localify</code> URLs for the apps you run on your machine.
</p>

<p align="center">
  <a href="https://localify.dev">Website</a> ·
  <a href="docs/README.md">User guide</a> ·
  <a href="CHANGELOG.md">Changelog</a> ·
  <a href="CONTRIBUTING.md">Contributing</a>
</p>

Localify is a local development proxy. A Rust daemon runs its own DNS server, issues
trusted certificates, and routes `https://journal.localify` to the port your dev server
listens on. You stop remembering ports, and OAuth callbacks, cookies, CORS origins and
webhooks get a fixed HTTPS address. Coding agents use the same URLs through Localify's
MCP server.

It ships as a command line tool for macOS and Linux and a desktop app for macOS.
Everything that runs on your machine is free, with no limit on projects or domains.

## A quick look

```bash
cd ~/code/journal
localify dev
```

The first run in a folder sets the project up. Localify picks a free port, registers a
domain such as `journal.localify`, allows it in Next.js `allowedDevOrigins` or Vite
`server.allowedHosts` when the project has that config file, and starts the `dev`
script with `PORT`, `HOST` and `LOCALIFY_URL` set. Then open `https://journal.localify`.

In a monorepo, each app it detects gets its own domain (`web.journal.localify`,
`api.journal.localify`). A Git worktree gets its own domains and ports, next to the main
checkout.

## Install

**macOS app.** Download it from [localify.dev](https://localify.dev), or take the DMG
directly: <https://downloads.localify.dev/latest/localify.dmg>. On first launch it asks
for your administrator password once, then sets up DNS, the certificate authority, the
background service and the `localify` command. The app updates itself.

**Command line, macOS (Apple Silicon) or Linux (x86_64):**

```bash
curl -fsSL https://localify.dev/install.sh | sh
```

The script checks the download's SHA-256, puts `localify` in `~/.local/bin`, then runs
`localify install`, which asks for your password once. Run it again to update. On Linux
you need systemd for the setup to run without root; read
[Localify on Linux](docs/linux.md) first.

With a normal install (macOS, or Linux with systemd), nothing of Localify's runs as root
afterwards, and only your machine can reach it until you run `localify lan enable`. On
macOS, browsers trust Localify's HTTPS once you click Trust HTTPS in the app or run
`localify trust`. `localify uninstall` removes everything.

To build from source, see [CONTRIBUTING.md](CONTRIBUTING.md).

## The commands you'll use

| Command | What it does |
| --- | --- |
| `localify dev [service]` | Start a service at its `.localify` URL. Add `--in-app` to run it in the desktop app's terminal, or `--detach` to run it in the background. |
| `localify services status` | Show what Localify started: running or not, PID, URL, port, since when, exit code. |
| `localify logs <service> [-f]` | Print a service's output, from any folder. |
| `localify stop <service>` / `restart <service>` | Stop a service, or restart it where it ran. |
| `localify register <domain> --route "/=3000"` | Point a domain at any port, address or folder, without a project. |

`localify status` checks the daemon, and `localify --help` lists everything else. The
[user guide](docs/README.md) covers projects, worktrees, routes, wildcards, [launch
profiles](docs/launch-profiles.md), [Docker](docs/docker.md) and troubleshooting.

## Coding agents

`localify mcp` is an MCP server that gives Claude Code, Codex and Cursor every Localify
action as a tool: start a service and get its URL, read its output, restart it, clean up
a finished worktree. Each tool runs one `localify` command. `localify install`
and the app connect the agents they find; `localify agent install` does it again later.
See [Coding agents](docs/README.md#coding-agents).

## Localify Cloud

The paid part. [Public URLs](docs/README.md#public-urls) open a project at
`https://<name>.localify.fyi`, behind a password, so you can reach it from your phone or
send it to someone. [Remote machines](docs/remote-machines.md) let you see, start and
stop the services of your other machines from the app. Both need Localify Cloud Pro: `localify
subscribe` starts it after a 7-day trial. Prices are on
[localify.dev/pricing](https://localify.dev/pricing).

## Privacy

Official desktop builds send product analytics and error reports to PostHog. They send
explicit events only, with no autocapture. Analytics events never include project names,
domains, request logs or local paths. Before an error report leaves your Mac, a filter
removes URLs, file paths, `.localify` domains and quoted names. It matches patterns, so
it can miss something in an unusual message. Session replay is off unless you turn it on
in Settings. Builds without `VITE_PUBLIC_POSTHOG_TOKEN` send nothing, and the CLI and the
daemon have no telemetry.

## Contributing

Bug reports, fixes and docs are welcome. [CONTRIBUTING.md](CONTRIBUTING.md) explains how
to build, run and test Localify, and how pull requests work. Every contribution needs the
[contributor license agreement](CLA.md), and everyone follows the
[code of conduct](CODE_OF_CONDUCT.md). Report vulnerabilities privately, as
[SECURITY.md](SECURITY.md) explains.

## License

Localify is licensed under the [GNU Affero General Public License v3.0 only](LICENSE).
It started as a fork of [Roxy](https://github.com/rbas/roxy) by Martin Voldřich, which
provided the original DNS, TLS and reverse proxy foundations, and whose code remains
under the [MIT license](LICENSE-MIT).

The AGPL doesn't cover everything here. The Localify name and logo aren't licensed, and
neither is the media in `apps/web/public/`. `progresive-blur.tsx` is adapted from Skiper
UI. [NOTICE](NOTICE) has the details.
