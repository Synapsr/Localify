# Desktop App

The desktop app is a Tauri shell in `src-tauri/` with a React interface in `desktop-ui/`.
It holds no business logic: the interface calls Tauri commands, the commands call
`localify::desktop::DesktopApi` (`src/desktop/`), and `DesktopApi` calls the same use cases
as the CLI. [ARCHITECTURE.md](ARCHITECTURE.md) has the interface's rules.

## Where Things Live

- `src-tauri/src/commands.rs`: the `#[tauri::command]` functions, one per action of
  `DesktopApi`. With the remote commands in `remote.rs`, this is the list of what the
  interface can ask for.
- `src-tauri/src/terminal.rs`: shell and service tabs, which stream PTY output to the
  interface as events.
- `src-tauri/src/remote.rs`: the remote machine commands, and the daemon's streams
  (output, controls, events) forwarded to the interface as events.
- `src/desktop/`: `DesktopApi`, its DTOs (`dto.rs`), PTYs (`pty.rs`, `run_pty.rs`),
  the shell environment, and the app bridge (`app_bridge.rs`).

## Development Notes

The desktop shell assumes the existing CLI binary is still the process used to
launch the daemon. Production defaults are:

- config at `~/Library/Application Support/Localify Desktop/config.toml`
- CLI binary at `target/debug/localify` in debug builds
- a launchd service, `dev.localify.daemon`, that runs that binary as the installing
  user and holds ports 80 and 443 for it
- while the app runs, the app bridge at `<data dir>/app-bridge/app-bridge.sock`,
  through which the daemon and `localify dev --in-app` hand it launches to run in
  service tabs; the app makes the `app-bridge` folder private (`0700`), never changes
  the data dir, and removes the socket when it quits. Where it cannot make that
  folder, as in a data dir of root's, it runs without the bridge
  (docs/remote-machines.md, "The app bridge")

The Root CA is trusted in your login keychain, for your account only. That needs no
administrator, and macOS confirms it in its own window. The install leaves this for
later so that it asks for one password: `localify trust` or the app's Trust HTTPS
button does it. Both profiles work this way.

Uninstalling also asks for one approval. It deletes the certificate and its key. macOS
keeps its record of having trusted that certificate, because removing the record takes
another confirmation. The record stays limited to the local suffix.

`pnpm tauri:dev` runs with `LOCALIFY_PROFILE=dev`. That profile is isolated
from the installed app:

- config at `~/Library/Application Support/Localify Dev/config.toml`
- data and certificates under `~/Library/Application Support/Localify Dev/data`
- pid, socket, logs and resolver names based on `localify-dev`
- domains under `.localify.test` instead of `.localify`
- ports 8080, 8443 and 1054, so no launchd service: the app starts the daemon itself
- an old root daemon from the installed app is left alone: this profile only stops its own

One approval writes `/etc/resolver/localify.test` and the `localify-dev` link.

You can override both when running the desktop app:

```bash
LOCALIFY_CONFIG_PATH=/custom/config.toml \
LOCALIFY_CLI_PATH=/custom/path/to/localify \
pnpm tauri:dev
```

## Run It

Build the CLI binary first so the desktop backend can launch the daemon:

```bash
cargo build --bin localify
pnpm i
pnpm tauri:dev
```

`pnpm tauri:dev` also merges `src-tauri/tauri.dev.conf.json`, so the dev app
uses the explicit "Localify (Dev)" name and the dev-only icon asset.
The dev script also forces `LOCALIFY_PROFILE=dev` and
`LOCALIFY_CLI_PATH=$PWD/target/debug/localify`, so daemon start/restart actions
from the dev shell use the repo's debug CLI and separate runtime files instead of
the installed production app.

When the dev app enables command line access, it installs `localify-dev`.
Invoking the CLI under that name automatically uses the dev profile, so test
commands can be run as:

```bash
localify-dev services list
localify-dev run web
```

## Updates

The daemon runs from the app's normal path under launchd, so replacing the binary needs
no prompt, whichever way it is replaced: in-app update, DMG, or a manual copy.

Every 2 seconds the daemon compares its binary with what is on disk. Once the file has
changed and stayed the same on two checks, the daemon exits with code 75, and launchd
starts the new build. `ThrottleInterval` is 1 second, so launchd adds at most that. A
daemon started another way, such as a dev build, is left alone.

## Launch at Login

Successful desktop activation registers Localify with the official Tauri
autostart plugin. The login item passes `--autostart`; that launch path keeps
the main window hidden and creates the tray icon. launchd starts the daemon, at
boot, whether or not the app ever opens.

The production Settings screen exposes the same login item as a "Launch at
login" toggle so users can verify, disable, or re-enable it without
reinstalling.

Uninstalling, from the app or with `localify uninstall`, removes the same login
item so the app does not keep a stale startup entry.

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

## Versioning

The command line and the desktop app share one version. It is managed from the
repo-root `package.json` and synced to the files that also need an explicit
version.

Use:

```bash
pnpm version:set -- 1.2.0
```

That updates:

- `package.json`
- `src-tauri/tauri.conf.json`
- `Cargo.toml` and `Cargo.lock`
- `src-tauri/Cargo.toml` and `src-tauri/Cargo.lock`

`pnpm tauri:build` validates that these stay aligned before it starts packaging.
If they drift, the build exits early and tells you to rerun the sync command.

Before a release, gather the notes in `changes/` into a new section of
`CHANGELOG.md` and delete the notes you used. The GitHub release takes its text
from that section.

For a production release, prefer the one-command publisher:

```bash
pnpm desktop:publish
```

By default it bumps the patch version, so `1.0.4` becomes `1.0.5`, then builds
the app, generates updater metadata, uploads the updater artifacts, and updates
the public DMG download alias in R2.

You can override the version explicitly:

```bash
pnpm desktop:publish -- --version 1.0.5
```

Or choose a different bump:

```bash
pnpm desktop:publish -- --bump minor
pnpm desktop:publish -- --bump major
```

To preview the resolved version and commands without changing files:

```bash
pnpm desktop:publish -- --dry-run
```

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

The public website download button should not point at the Tauri updater
archive. `pnpm desktop:publish-updater` also copies the notarized DMG into the
versioned publish directory as `localify.dmg`, and
`pnpm publish:update:r2` publishes a stable public alias:

- `latest/localify.dmg`

That alias is the clean URL for end users. The versioned DMG remains immutable,
while the `latest/` alias is replaced on each release.

If you need to republish the manifest from existing artifacts:

```bash
pnpm desktop:publish-updater
```

## Publish To Cloudflare R2

The simplest production hosting setup for Localify updates is:

- one public R2 bucket, shared by updater artifacts and public DMG downloads
- one custom domain such as `downloads.localify.app`
- the generated static updater files uploaded as plain objects

Tauri only needs HTTPS access to:

- `latest.json`
- `<version>/localify-desktop.app.tar.gz`
- `<version>/localify-desktop.app.tar.gz.sig`

The public website needs HTTPS access to:

- `latest/localify.dmg`
- `<version>/localify.dmg` for immutable release history

Cloudflare setup:

1. Create or reuse the public R2 bucket, for example `localify-updates-prod`
2. Connect a custom domain to that bucket, for example `downloads.localify.app`
3. Authenticate Wrangler locally with `pnpm dlx wrangler login`

Add these values to `.env`:

- `LOCALIFY_UPDATER_BASE_URL=https://downloads.localify.dev`
- `LOCALIFY_R2_BUCKET=localify-updates-prod`
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
It also publishes `latest/localify.dmg` for the marketing site after
the versioned DMG is uploaded.

If you want to inspect the exact Wrangler commands before a real upload:

```bash
pnpm desktop:publish-r2 -- --dry-run
```

The script uses `wrangler r2 object put` with explicit metadata:

- `latest.json` gets `Cache-Control: no-store, no-cache, must-revalidate`
- `latest/localify.dmg` gets `Cache-Control: no-store, no-cache, must-revalidate`
- updater archives get `Cache-Control: public, max-age=31536000, immutable`
- versioned DMGs get `Cache-Control: public, max-age=31536000, immutable`

## Sign And Notarize

For external macOS distribution, the desktop app is signed with a
`Developer ID Application` certificate and notarized by Apple. Releases are built
on a Mac, not in CI.

Prerequisites:

- a paid Apple Developer account
- a `Developer ID Application` certificate installed in your login keychain
- the signing identity available via `security find-identity -v -p codesigning`
- an App Store Connect API key, or an Apple ID with an
  [app-specific password](https://support.apple.com/102654)

The build reads these variables from the environment or from `.env`:

```bash
# The certificate's name or, safer when the name has accents, its SHA-1 hash
export APPLE_SIGNING_IDENTITY="Developer ID Application: Your Name (TEAMID)"

# With an API key, read from ~/.appstoreconnect/private_keys/AuthKey_<id>.p8
# unless APPLE_API_KEY_PATH says otherwise
export APPLE_API_KEY="ABCDE12345"
export APPLE_API_ISSUER="00000000-0000-0000-0000-000000000000"

# Or with an Apple ID
export APPLE_ID="you@example.com"
export APPLE_PASSWORD="xxxx-xxxx-xxxx-xxxx"
export APPLE_TEAM_ID="ABCDE12345"

pnpm tauri:build
```

Maintainers keep them in Infisical, project `localify`, folder `/release`, so a
release build needs no `.env`:

```bash
infisical run --env prod --path /release -- pnpm tauri:build
```

If `LOCALIFY_RELEASE_KEYCHAIN_PASSWORD` is set and the Mac has a
`localify-release` keychain, the build unlocks it and locks it again at the end.
The app sends analytics and error reports only when the build has
`VITE_PUBLIC_POSTHOG_TOKEN`.

With those set, `pnpm tauri:build`:

- builds and signs the `.app`
- notarizes the `.app` and staples the ticket
- rebuilds the updater archive and the `.dmg` from the stapled `.app`
- signs and notarizes the `.dmg`, and staples its ticket
- verifies both artifacts with `codesign`, `spctl`, and `stapler`

It uploads nothing. Only `pnpm publish:update:r2`, which `pnpm desktop:publish`
calls, changes what existing installs and the website see.

If you need to rerun only the post-processing on existing artifacts:

```bash
pnpm desktop:postprocess-macos-bundles
```

## Frontend Files

- `desktop-ui/src/lib/tauri-client.ts`: typed wrappers around Tauri `invoke`
- `desktop-ui/src/lib/queries/`: TanStack Query hooks over those wrappers
- `desktop-ui/src/types/localify.ts`: DTOs mirroring the Rust desktop API
- `desktop-ui/src/routes/`: the screens (TanStack Router, file-based)
- `desktop-ui/src/providers/`: app-wide state: terminal sessions, remote machines,
  updates, hotkeys
- `desktop-ui/src/main.tsx`: the entry point
