# Architecture Reference

**Localify** — Local development proxy written in Rust. Routes custom `.localify` domains to local services with automatic HTTPS support. Ships as a CLI plus a Tauri desktop app that drives the same core.

This document is the entry point for all architecture and convention decisions for the desktop UI layer. For Rust conventions (DDD layers, YAGNI, error handling, module boundaries), see [../CLAUDE.md](../CLAUDE.md).

## Core Principles

1. **Rust-first desktop core** — The core is a Rust binary (`src/`). The desktop UI is a thin Tauri shell that calls Rust commands via `invoke`. No business logic lives in the desktop frontend.
2. **Desktop UI stays local-only** — The desktop UI talks to the Rust core through Tauri commands only. There is no HTTP client, no SQL driver, no ORM, no auth in `desktop-ui/`.
3. **Type-safe end-to-end** — TypeScript strict mode in the desktop UI. DTOs in [desktop-ui/src/types/roxy.ts](../desktop-ui/src/types/roxy.ts) mirror the serializable types returned by `DesktopApi` (see [src/desktop/](../src/desktop/)).
4. **Minimal abstraction** — No premature helpers or wrappers. Three similar lines beats one clever abstraction.
5. **Minimal workspace, separate apps** — The repo can host multiple app entry points when needed. Today that means the Tauri desktop shell plus a standalone marketing web app under `apps/web`. There are still no shared packages or internal frontend libraries.
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

### Marketing web app (`apps/web`)

| Concern | Technology | Non-negotiable |
|---------|-----------|----------------|
| Framework | Next.js App Router | Yes |
| Language | TypeScript (strict) | Yes |
| Styling | App-local CSS / CSS Modules | Yes |
| Runtime | Static-first marketing pages | Yes |

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

### Code Review — Pre-commit quality gate

Applied contextually: the agent (or reviewer) checks only the rules relevant to the files changed in the diff.

| File | Applies when... |
|------|----------------|
| [00-general.md](code-review/00-general.md) | Any change |
| [01-structure.md](code-review/01-structure.md) | Any file created, moved, or renamed |
| [02-typescript.md](code-review/02-typescript.md) | Any `.ts` / `.tsx` file changed |
| [03-react-patterns.md](code-review/03-react-patterns.md) | React components or hooks changed |
| [04-data-layer.md](code-review/04-data-layer.md) | Tauri client wrappers or React Query factories changed |
| [05-forms.md](code-review/05-forms.md) | TanStack Form code changed |
| [06-styling.md](code-review/06-styling.md) | Tailwind classes or style files changed |
| [07-security.md](code-review/07-security.md) | Tauri command inputs or user input handling changed |
| [08-checklist.md](code-review/08-checklist.md) | **Always** — condensed pass/fail checklist |

For Rust file changes, follow [../CLAUDE.md](../CLAUDE.md) instead.

## How to Use (for AI agents)

When asked to commit or review code:

1. Run `git diff --staged` (or `git diff` if nothing staged) to see what changed.
2. Categorize changed files:
   - `desktop-ui/**/*.{ts,tsx,css}` → load the relevant `code-review/` files.
   - `apps/web/**/*.{ts,tsx,css}` → treat as standalone Next.js marketing code; keep it isolated from desktop concerns.
   - `src/**/*.rs`, `src-tauri/**/*.rs`, `Cargo.toml` → follow [../CLAUDE.md](../CLAUDE.md).
3. Load **only** the `code-review/` files relevant to the touched domains.
4. Always load `code-review/08-checklist.md` as a final pass for UI changes.
5. Flag violations with the rule reference (e.g., `[TS-03]`, `[DL-01]`). Don't flag what's clean.
6. If everything passes, proceed with the commit. No summary needed.
