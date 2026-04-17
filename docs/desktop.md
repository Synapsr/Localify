# Desktop App

This repo now includes a minimal Tauri shell in `src-tauri/` backed by a
structured Rust API in `src/desktop/`, plus a small Vite + TypeScript
frontend shell in `desktop-ui/`.

## What Exists

- A separate Tauri crate in `src-tauri/`
- A Vite + TypeScript frontend scaffold in `desktop-ui/`
- A `roxy::desktop::DesktopApi` facade that exposes the current backend
  actions as serializable DTOs for Tauri commands
- A typed Tauri client and lightweight app store for future frontend work

The goal is to keep future frontend work isolated from the core Rust
application code.

## Exposed Commands

- `get_bootstrap`
- `get_config`
- `update_config`
- `get_daemon_status`
- `list_domains`
- `install_roxy`
- `get_uninstall_preview`
- `uninstall_roxy`
- `register_domain`
- `unregister_domain`
- `add_route`
- `remove_route`
- `list_routes`
- `start_daemon`
- `stop_daemon`
- `restart_daemon`
- `reload_daemon`
- `read_logs`

## Development Notes

The desktop shell assumes the existing CLI binary is still the process used to
launch the daemon. By default, the Tauri backend looks for:

- config at `/etc/localify/config.toml`
- CLI binary at `target/debug/localify` in debug builds

You can override both when running the desktop app:

```bash
LOCALIFY_CONFIG_PATH=/custom/config.toml \
LOCALIFY_CLI_PATH=/custom/path/to/localify \
npm run tauri:dev
```

## Run It

Build the CLI binary first so the desktop backend can launch the daemon:

```bash
cargo build --bin localify
pnpm i
pnpm tauri:dev
```

## Package It

The packaged desktop app needs the `localify` CLI inside the bundle. The
packaging script now builds that CLI as a Tauri sidecar automatically, then
produces a macOS app bundle and DMG.

```bash
pnpm tauri:build
```

Artifacts are written under:

- `src-tauri/target/release/bundle/macos/`
- `src-tauri/target/release/bundle/dmg/`

If you need to override the CLI path while testing a packaged build, set
`LOCALIFY_CLI_PATH`.

## Frontend Files

- `desktop-ui/src/lib/tauri-client.ts`
  typed wrappers around Tauri `invoke`
- `desktop-ui/src/state/app-store.ts`
  lightweight state container for bootstrap/log data
- `desktop-ui/src/types/roxy.ts`
  frontend DTO contracts mirroring the Rust desktop API
- `desktop-ui/src/main.ts`
  shell rendering and event wiring

The current UI is still intentionally thin. The point of this scaffold is to
make future frontend implementation a straight screen-building exercise
instead of more infrastructure work.
