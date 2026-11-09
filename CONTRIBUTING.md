# Contributing to Localify

Thanks for helping. Bug reports, fixes, docs and feature proposals are all welcome. This
page explains how to build Localify, run it while you work on it, test it, find your way
in the code, and send a pull request.

## What's where

| Path | What it is |
| --- | --- |
| `src/` | The Rust crate: the `localify` CLI, the daemon, and the library the desktop app calls |
| `src/main.rs`, `src/cli/` | Command parsing and one module per command. Thin: they call use cases |
| `src/cli/mcp/` | `localify mcp`, the MCP server for coding agents. Each tool runs one CLI command |
| `src/application/` | Use cases and the ports (traits) they need |
| `src/domain/` | Domain types and rules, with no I/O |
| `src/infrastructure/` | Config, certificates, DNS setup, system services, processes, runs, the cloud client, remote pairing |
| `src/daemon/` | The running daemon: DNS server, HTTP and HTTPS proxy, TLS, router, management socket, public ingress, tunnel, remote relay client |
| `src/desktop/` | `DesktopApi` and the DTOs the Tauri app uses, PTYs for terminal and service tabs, the app bridge |
| `src-tauri/` | The Tauri shell: `#[tauri::command]` wrappers around `DesktopApi`, terminal and remote events |
| `desktop-ui/` | The desktop app's interface: React, Vite, TanStack Router and Query |
| `tests/` | Rust integration tests |
| `apps/web/` | The website, [localify.dev](https://localify.dev): Next.js on Cloudflare Workers ([README](apps/web/README.md)) |
| `apps/cloud/` | Localify Cloud: the Cloudflare Worker behind public URLs, billing and the remote relay ([README](apps/cloud/README.md)) |
| `scripts/` | Release, packaging and version-sync scripts |
| `docs/` | The user guide, feature docs, contracts and ADRs (see [Docs to read](#docs-to-read)) |

Everything goes through the Rust core. The desktop UI calls Tauri commands, which call
`DesktopApi`, which calls the same use cases as the CLI; the UI does no networking of
its own, apart from analytics in official builds. [How Localify works](docs/localify-technical-overview.md) draws the whole
picture.

## Set up

You need:

- Rust stable, with `rustfmt` and `clippy`;
- Node.js 22 and pnpm 10, for the desktop UI, the website and the cloud Worker;
- macOS for the desktop app. The CLI and the daemon also build and run on Linux, see
  [docs/linux.md](docs/linux.md);
- optionally [`just`](https://github.com/casey/just), for shortcuts over cargo.

```bash
git clone https://github.com/Synapsr/Localify.git
cd Localify
pnpm install
cargo build --bin localify
```

To use the CLI you built as your real `localify`, install it somewhere on your `PATH`,
such as `~/.local/bin`, and set it up. Localify refuses to run under `sudo`;
`localify install` asks for an administrator's approval once by itself.

```bash
cargo build --release --bin localify
install -m 755 target/release/localify ~/.local/bin/localify
localify install
```

## Run the desktop app in development

```bash
cargo build --bin localify
pnpm tauri:dev
```

`pnpm tauri:dev` runs "Localify (Dev)" with `LOCALIFY_PROFILE=dev` and your debug
build of the CLI (`target/debug/localify`), so it doesn't touch an installed Localify:

- its config and data live in `~/Library/Application Support/Localify Dev/`;
- its domains end in `.localify.test` instead of `.localify`;
- its daemon uses ports 8080, 8443 and 1054, and the app starts it itself, with no
  launchd service;
- one approval writes `/etc/resolver/localify.test`.

When the dev app enables command line access, it installs `localify-dev`, which always
uses the dev profile: `localify-dev services list`.

[docs/desktop.md](docs/desktop.md) covers the shell, packaging, versioning and releases.
The website runs with `pnpm web:dev`, and the cloud Worker with `pnpm cloud:dev`; their
READMEs explain the setup each needs.

## Test and check

CI runs these on pull requests, depending on the paths they touch: the Rust checks on
macOS and Linux, the desktop UI, cloud and Tauri shell checks, and, in its own workflow,
the website's. Run the ones for what you changed:

| You changed | Run |
| --- | --- |
| Rust (`src/`, `tests/`) | `cargo fmt`, `cargo clippy -- -D warnings`, `cargo test` |
| `src-tauri/` | the Rust checks, then `pnpm desktop:build`, `node scripts/prepare-tauri-sidecar.mjs` and `cargo test --manifest-path src-tauri/Cargo.toml` |
| `desktop-ui/` | `pnpm run typecheck`, `pnpm desktop:test` |
| `apps/web/` | `pnpm web:typecheck`, `pnpm web:build`, with the same keys in all 9 `apps/web/messages/` locales |
| `apps/cloud/` | `pnpm cloud:typecheck`, `pnpm cloud:test` |
| `apps/web/public/install.sh` | `pnpm test:install-script` |

`just check` runs `cargo fmt --check` and clippy as CI does, `just test` runs
`cargo test`, and `just fix` formats and applies clippy's fixes. `just` alone lists the
rest. Some shell-environment tests use zsh and fish when they're installed. `pnpm lint`
and `pnpm format` run oxlint and oxfmt on the desktop UI.

## Docs to read

| Before you change | Read |
| --- | --- |
| Anything | [CONTEXT.md](CONTEXT.md), the words the code and the docs use |
| Rust code | [docs/rust/conventions.md](docs/rust/conventions.md) and [docs/rust/architecture.md](docs/rust/architecture.md) |
| `desktop-ui/` | [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) |
| Public URLs, accounts or billing | [docs/public-urls.md](docs/public-urls.md), the contract |
| Remote machines, runs, service tabs, the app bridge | [docs/remote-machines.md](docs/remote-machines.md), the contract |
| Launch profiles | [docs/launch-profiles.md](docs/launch-profiles.md) |
| How `localify dev` and agents set up a project and its framework config | [docs/agents-and-bootstrap.md](docs/agents-and-bootstrap.md) |
| Anything an ADR decided | [docs/adr/](docs/adr/), one decision per file |
| `apps/web/`, or a claim about what Localify does | [apps/web/README.md](apps/web/README.md) |

The contracts and the ADRs describe what is decided. A pull request that changes one of
those decisions changes the document with it, and says so. The user guide,
[docs/README.md](docs/README.md), describes what Localify does today from a user's side.

## Pull requests

- Keep each pull request to one fix or one feature. For larger changes, open an issue
  first so we can agree on the approach before you put time into it.
- Use [Conventional Commits](https://www.conventionalcommits.org/) for commit messages
  and pull request titles: `feat:`, `fix:`, `docs:`, `chore:`…
- When users will notice the change, add a short note in `changes/<slug>.md`, with a
  `type` (`new`, `improved` or `fixed`) and a title that says what they can do now.
  Releases gather these notes into [CHANGELOG.md](CHANGELOG.md).
- Update the docs your change makes wrong, and the contract or ADR it changes.
- Never commit secrets. `.env` files are ignored; keep it that way.

### Contributor license agreement

Every contribution needs the [contributor license agreement](CLA.md). You keep your
copyright and give Téo Goulois a license to use your contribution, including under terms
other than the AGPL. Accept it by ticking the box in the pull request template.

## Reporting bugs

Open an issue with your OS version, the Localify version (`localify --version`), the
steps to reproduce, and the relevant output of `localify status` and `localify logs`.

Security issues don't go in public issues: see [SECURITY.md](SECURITY.md).

## Code of conduct

Everyone taking part in this project follows the [code of conduct](CODE_OF_CONDUCT.md).

## License

Localify is licensed under [AGPL-3.0-only](LICENSE). Code from Roxy, the project it
derives from, remains under the [MIT license](LICENSE-MIT). See [NOTICE](NOTICE).
