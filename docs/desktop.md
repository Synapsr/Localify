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

## Sign And Notarize

For external macOS distribution, the desktop app should be signed with a
`Developer ID Application` certificate and notarized by Apple. Tauri supports
that flow directly during `tauri build`.

Local build prerequisites:

- a paid Apple Developer account
- a `Developer ID Application` certificate installed in your login keychain
- the signing identity available via `security find-identity -v -p codesigning`
- notarization credentials configured through Tauri env vars

Recommended notarization auth for CI is the App Store Connect API:

- `APPLE_SIGNING_IDENTITY`
- `APPLE_API_ISSUER`
- `APPLE_API_KEY`
- `APPLE_API_KEY_PATH`

For local notarized builds on your Mac:

```bash
export APPLE_SIGNING_IDENTITY="Developer ID Application: Your Name (TEAMID)"
export APPLE_API_ISSUER="..."
export APPLE_API_KEY="..."
export APPLE_API_KEY_PATH="$HOME/private_keys/AuthKey_XXXXXX.p8"

pnpm tauri:build
bash scripts/verify-macos-signing.sh \
  "src-tauri/target/release/bundle/macos/Localify Desktop.app" \
  "$(find src-tauri/target/release/bundle/dmg -maxdepth 1 -name 'Localify Desktop_*.dmg' | head -n 1)"
```

The verification script checks:

- `codesign --verify` on the `.app`
- `spctl --assess` on the `.app`
- `xcrun stapler validate` on the `.app`
- `codesign --verify` on the `.dmg`
- `xcrun stapler validate` on the `.dmg`

## GitHub Actions

A manual workflow is available at `.github/workflows/desktop-notarize.yml`.
It builds a notarized desktop artifact on `macos-latest` without changing the
existing CLI release pipeline.

Required GitHub secrets:

- `APPLE_SIGNING_IDENTITY`
- `APPLE_CERTIFICATE`
- `APPLE_CERTIFICATE_PASSWORD`
- `KEYCHAIN_PASSWORD`
- `APPLE_API_ISSUER`
- `APPLE_API_KEY`
- `APPLE_API_KEY_BASE64`

Notes:

- `APPLE_CERTIFICATE` should be the base64-encoded exported `.p12`
- `APPLE_API_KEY_BASE64` should be the base64-encoded contents of the
  downloaded App Store Connect `.p8` key
- the workflow uploads a notarized `.app` and `.dmg` as build artifacts

Useful helper commands:

```bash
# Export your signing certificate from a local .p12 for GitHub Secrets
openssl base64 -A -in /path/to/certificate.p12 -out apple-certificate-base64.txt

# Convert the App Store Connect private key for GitHub Secrets
base64 -i /path/to/AuthKey_XXXXXX.p8 | tr -d '\n' > apple-api-key-base64.txt
```

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
