# Localify

Localify gives local development projects stable `.localify` domains with
trusted HTTPS. It ships as a Rust CLI and daemon, a Tauri desktop app, and a
small Next.js website.

## What it does

- Routes memorable `.localify` domains to local services.
- Generates and trusts local TLS certificates.
- Supports HTTP, HTTPS, WebSockets, static directories, and wildcard domains.
- Groups domains and services into projects with worktree-aware development
  commands.
- Exposes request logs, project controls, diagnostics, and licensing through
  the desktop app.

## Repository layout

- `src/` — Rust library, CLI, daemon, DNS, TLS, routing, and project logic.
- `src-tauri/` — Tauri desktop shell and commands.
- `desktop-ui/` — React, Vite, and TanStack desktop interface.
- `apps/web/` — Next.js website.
- `skills/localify/` — bundled agent skill for working with Localify projects.

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

## Local CLI example

```bash
localify init-project my-app --yes
localify status
localify logs -f
```

The default project domain is `https://my-app.localify`. Localify selects a
free target port unless one is explicitly provided.

## Documentation

- [Architecture](docs/ARCHITECTURE.md)
- [Desktop development](docs/desktop.md)
- [Docker integration](docs/docker.md)
- [Linux setup](docs/linux.md)
- [Technical overview](docs/localify-technical-overview.md)
- [Public URLs](docs/public-urls.md)

## Contributing

Contributions are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md) to get
started, and [SECURITY.md](SECURITY.md) to report a vulnerability.

## Acknowledgements

Localify started as a fork of [Roxy](https://github.com/rbas/roxy) by
Martin Voldřich, which provides the original DNS, TLS, and reverse proxy
foundations.

## License

[MIT](LICENSE.md)
