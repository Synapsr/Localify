# AGENTS.md

## Task Completion Requirements

- **Rust**: `cargo fmt`, `cargo clippy -- -D warnings`, and `cargo test` must pass.
- **Desktop UI**: `pnpm run typecheck` must pass for any `desktop-ui/` change.
- **Website**: `pnpm web:typecheck` and `pnpm web:build` must pass for any `apps/web/` change, with identical keys in all 9 `apps/web/messages/` locales.

## Project Snapshot

Localify is a local development proxy written in Rust. It routes custom `.localify` domains to local services with automatic HTTPS. Ships as a CLI plus a Tauri desktop app (`desktop-ui/`, React + Vite + TanStack). The website is a separate Next.js app in `apps/web/`.

**Philosophy**: Pragmatic, idiomatic Rust. Build the simplest thing that works. YAGNI throughout.

## Core Priorities

1. **Correctness** — no panics in library code, errors handled explicitly, inputs validated at boundaries.
2. **Simplicity** — three similar lines beats one clever abstraction.
3. **YAGNI** — don't build what isn't needed yet.

If a tradeoff is required, choose correctness and simplicity over short-term convenience.

## Rust Skills (AI)

`.rust-skills/skills/` is a gitignored local install, so fresh clones lack it. When it exists and you're working on Rust:

- Invoke `rust-router` first for Rust questions, errors, or design.
- Follow the routed skill (`m01-*`..`m15-*`, `domain-*`).
- Use `unsafe-checker` for any unsafe/FFI review.

## Documentation map

- **Desktop UI architecture** — [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)
- **Rust conventions** — [docs/rust/conventions.md](docs/rust/conventions.md)
- **Rust architecture / DDD layers** — [docs/rust/architecture.md](docs/rust/architecture.md)
- **Operational guides** — [docs/desktop.md](docs/desktop.md), [docs/docker.md](docs/docker.md), [docs/linux.md](docs/linux.md)

## Load docs by context

| What you're changing | Read first |
|---------------------|------------|
| Any Rust file | [docs/rust/conventions.md](docs/rust/conventions.md) |
| Module structure, new layer, DDD boundary | [docs/rust/architecture.md](docs/rust/architecture.md) |
| Any `desktop-ui/` change | [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) |
| Any `apps/web/` change, or a claim about what Localify does | [apps/web/README.md](apps/web/README.md) |

## Pre-commit

- **Rust commits** — run the checklist at the bottom of [docs/rust/conventions.md](docs/rust/conventions.md).
- **`desktop-ui/` commits** — run `pnpm run typecheck` and re-read [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Commits and pull requests

- Use [Conventional Commits](https://www.conventionalcommits.org/) (`feat:`, `fix:`, `docs:`, `chore:`…).
- **Never add AI attribution anywhere**: no `🤖 Generated with [Claude Code](https://claude.com/claude-code)` footer, no `Co-Authored-By: Claude …` (or any other AI tool) trailer. This applies to commit messages, pull request titles and descriptions, issues, and comments.
