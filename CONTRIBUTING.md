# Contributing to Localify

Thanks for taking the time to contribute. Bug reports, fixes, documentation
improvements, and feature proposals are all welcome.

## Getting started

Requirements:

- Rust stable
- Node.js and pnpm
- macOS for the primary desktop development flow (see [docs/linux.md](docs/linux.md) for Linux)

```bash
pnpm install
cargo build --bin localify
pnpm tauri:dev
```

The repository layout is described in the [README](README.md), and the
architecture documents live in [docs/](docs/).

## Before opening a pull request

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

Please also read:

- [docs/rust/conventions.md](docs/rust/conventions.md) for Rust changes
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for `desktop-ui/` changes

## Pull requests

- Keep pull requests focused: one fix or feature per PR.
- Use [Conventional Commits](https://www.conventionalcommits.org/) for commit
  messages (`feat:`, `fix:`, `docs:`, `chore:`…). The changelog is generated
  from them.
- For larger changes, open an issue first so the approach can be discussed
  before you invest time in it.
- Never commit secrets. `.env` files are ignored; keep it that way.

## Reporting bugs

Open an issue with your OS version, Localify version (`localify --version`),
the steps to reproduce, and the relevant output of `localify logs`.

Security issues should **not** be reported in public issues. See
[SECURITY.md](SECURITY.md).

## License

By contributing, you agree that your contributions are licensed under the
[MIT License](LICENSE.md).
