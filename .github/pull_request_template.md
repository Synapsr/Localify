## Summary

<!-- What does this change, and why? Link the issue it fixes. -->

## Checks

<!-- Tick what applies to the files you changed. CONTRIBUTING.md lists every check. -->

- [ ] Rust: `cargo fmt`, `cargo clippy -- -D warnings` and `cargo test` pass
- [ ] `desktop-ui/`: `pnpm run typecheck` and `pnpm desktop:test` pass
- [ ] `apps/web/`: `pnpm web:typecheck` and `pnpm web:build` pass, with the same keys in every locale
- [ ] `apps/cloud/`: `pnpm cloud:typecheck` and `pnpm cloud:test` pass
- [ ] Users will notice this change, and I added a note in `changes/`
- [ ] Docs are updated where behaviour changed

## Contributor license agreement

- [ ] I have read and accept the [contributor license agreement](https://github.com/Synapsr/Localify/blob/main/CLA.md)
