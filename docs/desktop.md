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

`pnpm tauri:dev` also merges `src-tauri/tauri.dev.conf.json`, so the dev app
uses the explicit "Localify Desktop (Dev)" name and the dev-only icon asset.
The dev script also forces `LOCALIFY_CLI_PATH=$PWD/target/debug/localify`, so
daemon start/restart actions from the dev shell always prefer the repo's debug
CLI over any installed production binary or stale sibling build.

## Package It

The packaged desktop app needs the `localify` CLI inside the bundle. The
packaging script now builds that CLI as a Tauri sidecar automatically, then
produces a macOS app bundle, updater archive, and DMG.

```bash
pnpm tauri:build
```

Artifacts are written under:

- `src-tauri/target/release/bundle/macos/`
- `src-tauri/target/release/bundle/dmg/`
- `dist/updates/` when updater publishing is configured

If you need to override the CLI path while testing a packaged build, set
`LOCALIFY_CLI_PATH`.

## Desktop Versioning

The desktop release version is managed from the repo-root `package.json` and
then synced to the two Tauri/Rust files that also need an explicit version.

Use:

```bash
pnpm version:desktop -- 0.1.2
```

That updates:

- `package.json`
- `src-tauri/tauri.conf.json`
- `src-tauri/Cargo.toml`

`pnpm tauri:build` now validates that those three files stay aligned before it
starts packaging. If they drift, the build exits early and tells you to rerun
the sync command.

## Self-Hosted Desktop Updates

The desktop app can now check for a newer signed build during startup and
install it automatically. This does not require GitHub Releases or GitHub
Actions, but it does require Tauri updater artifacts and a separate updater
signing keypair.

Generate the updater key once on your Mac:

```bash
pnpm tauri signer generate -w ~/.tauri/localify-updater.key
```

Commit the generated public key to `src-tauri/updater.pub`, then keep the
private key safe. Existing installs can only trust future updates signed with
that same private key.

Add these updater values to `.env` for local release builds:

- `LOCALIFY_UPDATER_BASE_URL=https://downloads.example.com`
- `LOCALIFY_UPDATER_ENDPOINT=https://downloads.example.com/latest.json` optional
- `TAURI_SIGNING_PRIVATE_KEY=~/.tauri/localify-updater.key` optional when using the default path
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` optional if you password-protect the updater key

`pnpm tauri:build` now loads `.env`, defaults `TAURI_SIGNING_PRIVATE_KEY` to
`~/.tauri/localify-updater.key` when present, and does the full local release
flow. If `TAURI_SIGNING_PRIVATE_KEY` points at a local file, the wrapper reads
the key content before invoking Tauri:

- build the `.app`, `.app.tar.gz`, `.app.tar.gz.sig`, and `.dmg`
- sign and notarize the macOS app when Apple env vars are present
- publish `dist/updates/latest.json`
- copy the updater archive and signature to `dist/updates/<version>/`

`latest.json` is emitted in Tauri's static updater format, so you can upload
`dist/updates/` directly to any HTTPS host, object storage bucket, or CDN.
The packaged app reads the compiled-in update endpoint and checks it on startup
before showing the main UI.

If you need to republish the manifest from existing artifacts:

```bash
pnpm desktop:publish-updater
```

## Publish To Cloudflare R2

The simplest production hosting setup for Localify updates is:

- one public R2 bucket
- one custom domain such as `downloads.localify.app`
- the generated static updater files uploaded as plain objects

Tauri only needs HTTPS access to:

- `latest.json`
- `<version>/localify-desktop.app.tar.gz`
- `<version>/localify-desktop.app.tar.gz.sig`

Cloudflare setup:

1. Create an R2 bucket, for example `localify-updates`
2. Connect a custom domain to that bucket, for example `downloads.localify.app`
3. Authenticate Wrangler locally with `pnpm dlx wrangler login`

Add these values to `.env`:

- `LOCALIFY_UPDATER_BASE_URL=https://downloads.localify.app`
- `LOCALIFY_R2_BUCKET=localify-updates`
- `LOCALIFY_R2_JURISDICTION=` optional, only if your bucket uses a specific jurisdiction

Build the release locally:

```bash
pnpm tauri:build
```

That generates the updater payload in `dist/updates/`. Then upload it to R2:

```bash
pnpm publish:update:r2
```

The upload script pushes versioned artifacts first and `latest.json` last, so
clients never see a new manifest before the matching payload exists in R2.

If you want to inspect the exact Wrangler commands before a real upload:

```bash
pnpm desktop:publish-r2 -- --dry-run
```

The script uses `wrangler r2 object put` with explicit metadata:

- `latest.json` gets `Cache-Control: no-store, no-cache, must-revalidate`
- updater archives get `Cache-Control: public, max-age=31536000, immutable`

## Sign And Notarize

For external macOS distribution, the desktop app should be signed with a
`Developer ID Application` certificate and notarized by Apple. Tauri supports
that flow directly during `tauri build`.

Local build prerequisites:

- a paid Apple Developer account
- a `Developer ID Application` certificate installed in your login keychain
- the signing identity available via `security find-identity -v -p codesigning`
- notarization credentials configured through Tauri env vars

This repo now uses the Apple ID notarization flow for both local builds and CI:

- `APPLE_SIGNING_IDENTITY`
- `APPLE_ID`
- `APPLE_PASSWORD`
- `APPLE_TEAM_ID`

Optional when the Apple account is attached to multiple teams:

- `APPLE_PROVIDER_SHORT_NAME`

For local notarized builds on your Mac:

```bash
export APPLE_SIGNING_IDENTITY="Developer ID Application: Your Name (TEAMID)"
export APPLE_ID="you@example.com"
export APPLE_PASSWORD="xxxx-xxxx-xxxx-xxxx"
export APPLE_TEAM_ID="ABCDE12345"

pnpm tauri:build
```

`pnpm tauri:build` now does the full macOS post-processing automatically when
those env vars are present:

- build the `.app` and `.dmg`
- let Tauri sign + notarize the `.app`
- submit the `.dmg` to `notarytool` if it is not already stapled
- staple the `.dmg`
- verify both artifacts with `codesign`, `spctl`, and `stapler`

The verification script checks:

- `codesign --verify` on the `.app`
- `spctl --assess` on the `.app`
- `xcrun stapler validate` on the `.app`
- `codesign --verify` on the `.dmg`
- `xcrun stapler validate` on the `.dmg`

If you need to rerun only the post-processing on existing artifacts:

```bash
pnpm desktop:postprocess-macos-bundles
```

## GitHub Actions

A manual workflow is available at `.github/workflows/desktop-notarize.yml`.
It builds a notarized desktop artifact on `macos-latest` without changing the
existing CLI release pipeline.

Required GitHub secrets:

- `APPLE_SIGNING_IDENTITY`
- `APPLE_ID`
- `APPLE_PASSWORD`
- `APPLE_TEAM_ID`
- `APPLE_CERTIFICATE`
- `APPLE_CERTIFICATE_PASSWORD`
- `KEYCHAIN_PASSWORD`
- `APPLE_PROVIDER_SHORT_NAME` optional

Notes:

- `APPLE_CERTIFICATE` should be the base64-encoded exported `.p12`
- `APPLE_PASSWORD` must be an Apple app-specific password, not your main Apple
  account password
- `APPLE_TEAM_ID` is the Developer Team ID shown on your Apple Developer
  membership page
- `APPLE_PROVIDER_SHORT_NAME` is only needed if notarization complains about
  multiple teams/providers on the same Apple account
- the workflow uploads a notarized `.app` and `.dmg` as build artifacts

Useful helper commands:

```bash
# Export your signing certificate from a local .p12 for GitHub Secrets
openssl base64 -A -in /path/to/certificate.p12 -out apple-certificate-base64.txt
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
