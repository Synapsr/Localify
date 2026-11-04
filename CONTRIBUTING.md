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

## Contributor license agreement

Every contribution requires accepting the [contributor license agreement](CLA.md).
You keep your copyright and give Téo Goulois a license to use your contribution,
including under terms other than the AGPL. Accept it by ticking the box in the
pull request template.

## Code of conduct

Everyone taking part in this project follows the
[code of conduct](CODE_OF_CONDUCT.md).

## Reporting bugs

Open an issue with your OS version, Localify version (`localify --version`),
the steps to reproduce, and the relevant output of `localify logs`.

Security issues should **not** be reported in public issues. See
[SECURITY.md](SECURITY.md).

## License

Localify is licensed under [AGPL-3.0-only](LICENSE). Code from Roxy, the project
it derives from, remains under the [MIT license](LICENSE-MIT). See
[NOTICE](NOTICE).
