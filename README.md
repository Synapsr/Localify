# Localify

Localify gives each local project a stable `.localify` domain with trusted
HTTPS, so you open `https://my-app.localify` instead of remembering which
`localhost` port a service landed on. OAuth callbacks, cookies, and CORS origins
get a fixed HTTPS address, and coding agents reach your app at a URL that stays
the same between runs.

It ships as a Rust CLI and daemon plus a Tauri desktop app. Everything that runs
on your machine is free, with no limit on projects or domains.

## What it does

- Routes memorable `.localify` domains to local services.
- Generates and trusts local TLS certificates.
- Supports HTTP, HTTPS, WebSockets, static directories, and wildcard domains.
- Groups domains and services into projects with worktree-aware development
  commands.
- Shows request logs, project controls, and diagnostics in the desktop app.

## Install

### Desktop app

Download the app from [localify.dev](https://localify.dev). On first launch it
asks for your administrator password once, then sets up HTTPS, DNS, and the
daemon.

### CLI from source

You need Rust stable, on macOS or Linux. On Linux, read
[docs/linux.md](docs/linux.md) first.

```bash
git clone https://github.com/Synapsr/Localify.git
cd Localify
cargo build --release --bin localify
sudo install -m 755 target/release/localify /usr/local/bin/localify
```

Set up the local certificate authority and DNS, then start the daemon:

```bash
sudo localify install
sudo localify start
```

`sudo localify uninstall` removes this setup.

## Local CLI example

With the setup above done, point a domain at a service on port 3000:

```bash
sudo localify register my-app.localify --route "/=3000"
localify status
localify logs -f
```

The app is now at `https://my-app.localify`. The
[CLI reference](docs/README.md) covers routes, wildcards, and static directories.

On macOS, `localify init-project my-app --yes` creates a project from the current
folder instead, with `my-app.localify` as its default domain. Localify picks a
free target port unless you provide one. The first run asks for your
administrator password to set up the system helper.

## Telemetry

Official desktop builds send product analytics and error reports to PostHog.
They send explicit events only, with no autocapture, and analytics events never
include project names, domains, request logs, or local paths. Error reports have
URLs and file paths removed, but a message can still name the project or domain
it's about. Session replay is off by default, and you can turn it on in Settings.
Builds without `VITE_PUBLIC_POSTHOG_TOKEN` send nothing, and the CLI and daemon
have no telemetry.

## Repository layout

- `src/`: Rust library, CLI, daemon, DNS, TLS, routing, and project logic.
- `src-tauri/`: Tauri desktop shell and commands.
- `desktop-ui/`: React, Vite, and TanStack desktop interface.
- `apps/web/`: Next.js website, [localify.dev](https://localify.dev). See
  [apps/web/README.md](apps/web/README.md).
- `skills/localify/`: bundled agent skill for working with Localify projects.

## Development

Requirements:

- Rust stable
- Node.js and pnpm
- macOS for the primary desktop development flow

Install JavaScript dependencies:

```bash
pnpm install
```

Build the CLI:

```bash
cargo build --bin localify
```

Run the desktop app with the development profile:

```bash
pnpm tauri:dev
```

Run the website:

```bash
pnpm web:dev
```

## Quality checks

Rust changes must pass:

```bash
cargo fmt
cargo clippy -- -D warnings
cargo test
```

Desktop UI changes must also pass:

```bash
pnpm run typecheck
```

Website changes must pass:

```bash
pnpm web:typecheck
pnpm web:build
```

## Documentation

- [CLI reference](docs/README.md)
- [Architecture](docs/ARCHITECTURE.md)
- [Desktop development](docs/desktop.md)
- [Docker integration](docs/docker.md)
- [Launch profiles](docs/launch-profiles.md)
- [Linux setup](docs/linux.md)
- [Technical overview](docs/localify-technical-overview.md)

Design notes for features that are not built yet:

- [Public URLs](docs/public-urls.md)

## Contributing

Contributions are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md) to get started,
including the [contributor license agreement](CLA.md) every contribution needs,
and the [code of conduct](CODE_OF_CONDUCT.md). To report a vulnerability, see
[SECURITY.md](SECURITY.md).

## Acknowledgements

Localify started as a fork of [Roxy](https://github.com/rbas/roxy) by
Martin Voldřich, which provides the original DNS, TLS, and reverse proxy
foundations.

## License

Localify is licensed under the [GNU Affero General Public License v3.0
only](LICENSE). It derives from Roxy, whose code remains under the
[MIT license](LICENSE-MIT).

The AGPL doesn't cover everything here. The Localify name and logo aren't
licensed, and neither is the media in `apps/web/public/` and
`desktop-ui/public/videos/`. `progresive-blur.tsx` is adapted from Skiper UI.
[NOTICE](NOTICE) has the details.
