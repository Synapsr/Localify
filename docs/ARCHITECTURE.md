# Architecture Reference

**Localify** — Local development proxy written in Rust. Routes custom `.localify` domains to local services with automatic HTTPS support. Ships as a CLI plus a Tauri desktop app that drives the same core.

This document is the entry point for all architecture and convention decisions for the desktop UI layer. For Rust conventions (DDD layers, YAGNI, error handling, module boundaries), see [../CLAUDE.md](../CLAUDE.md).

## Core Principles

1. **Rust-first desktop core** — The core is a Rust binary (`src/`). The desktop UI is a thin Tauri shell that calls Rust commands via `invoke`. No business logic lives in the desktop frontend.
2. **Desktop UI stays local-only** — The desktop UI talks to the Rust core through Tauri commands only. There is no HTTP client, no SQL driver, no ORM, no auth in `desktop-ui/`.
3. **Type-safe end-to-end** — TypeScript strict mode in the desktop UI. DTOs in [desktop-ui/src/types/localify.ts](../desktop-ui/src/types/localify.ts) mirror the serializable types returned by `DesktopApi` (see [src/desktop/](../src/desktop/)).
4. **Minimal abstraction** — No premature helpers or wrappers. Three similar lines beats one clever abstraction.
5. **Minimal workspace** — The repo holds one app: the Tauri desktop shell and its React UI. The website lives in its own repository.
6. **Convention over configuration** — File naming, folder structure, and patterns are standardized so any contributor (or agent) can navigate the code instantly.

## Imposed Stack

### Rust layer (core)

| Concern | Technology |
|---------|-----------|
| CLI parsing | clap |
| Async runtime | tokio |
| HTTP server | axum + hyper |
| TLS / certificates | rustls + rcgen |
| Serialization | serde + serde_json |
| Error handling | anyhow (app) / thiserror (lib) |
| Desktop shell | Tauri v2 |

Rust conventions (DDD layer rules, module layout, YAGNI, testing strategy) live in [../CLAUDE.md](../CLAUDE.md).

### Desktop UI layer (`desktop-ui/`)

| Concern | Technology | Non-negotiable |
|---------|-----------|----------------|
| Build | Vite | Yes |
| Language | TypeScript (strict) | Yes |
| UI framework | React | Yes |
| Routing | TanStack Router (file-based routes) | Yes |
| Server state | TanStack React Query | Yes |
| Client state | Zustand | Yes |
| Forms | TanStack Form + Zod | Yes |
| Styling | Tailwind CSS | Yes |
| UI primitives | shadcn/ui on top of Base UI (`@base-ui-components/react`) | Yes |
| Native bridge | `@tauri-apps/api` via typed wrappers in `lib/tauri-client.ts` | Yes |

### Explicitly excluded

- No database, no ORM, no SQL driver in the UI — persistence belongs to Rust
- No shared internal packages or component libraries — keep the workspace shallow and app boundaries explicit
- No auth, no network fetches, no HTTP client — the UI only speaks Tauri `invoke`
- No i18n in v1
- No shared component library package — shadcn primitives (Base UI under the hood) are copied into `desktop-ui/src/components/ui/` and live in the repo

## Documents

### Localify operational guides

| File | Topic |
|------|-------|
| [README.md](README.md) | Project overview and usage |
| [desktop.md](desktop.md) | Tauri shell layout and exposed commands |
| [docker.md](docker.md) | Docker integration |
| [linux.md](linux.md) | Linux platform notes |

For Rust file changes, follow [../CLAUDE.md](../CLAUDE.md) instead.

## How to Use (for AI agents)

When asked to commit or review code:

1. Run `git diff --staged` (or `git diff` if nothing staged) to see what changed.
2. Categorize changed files:
   - `desktop-ui/**/*.{ts,tsx,css}` → follow the desktop UI rules in this document.
   - `src/**/*.rs`, `src-tauri/**/*.rs`, `Cargo.toml` → follow [../CLAUDE.md](../CLAUDE.md).
3. Flag violations of the rules in this document. Don't flag what's clean.
4. If everything passes, proceed with the commit. No summary needed.
