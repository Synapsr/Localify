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

### Automatic updates

A Mac can install new versions by itself (ADR 0034).

- **Setting:** `[updates] automatic` in the config. It starts on only for a new install: no config, no old `/etc/localify` folder, and nothing an earlier install leaves behind (its root CA and certificates, the daemon's pid file or log, the launchd service, the DNS resolver). A place that can't be checked counts as there. On a new install the app writes the config, with it on, as it starts, before its welcome can save a setting; `localify install` writes it on too when it finds no config and nothing else. Anything else starts off. A config that is missing or can't be read counts as off. Moving an old `/etc/localify` config into the user's folder keeps that config's setting, off when it has none, even when the welcome already wrote a config there. Settings has `Install updates automatically`, right after Startup, and the CLI has `localify update --auto on|off`. The management socket's `update_settings {automatic}` changes it. An unattended install requires `automatic` to be on just before it replaces anything; an install a person confirms works whether it is on or off.
- **Checks:** 2 minutes after the daemon starts, then every 6 hours plus or minus 30 minutes. Also when `remote_machines` shows a paired machine on a newer version: such a version comes from the relay's presence and is only a hint. Every check, scheduled, triggered or manual, goes through one coalesced operation with a machine-wide cooldown of 10 minutes, doubled after each failure up to 6 hours. The last check's time is persisted, so a restart doesn't reset it. The daemon also downloads at most once every 6 hours until an install succeeds: the time of its last download is persisted before the download starts, so neither a restart nor a hint repeats it. A person's `localify update install` doesn't wait for that cooldown. A check reads the manifest the app's updater reads (`latest.json`, from the build's updater endpoint) over HTTPS. That manifest is discovery, not authority.
- **Trust:** the archive's minisign signature is checked against the updater public key built into Localify before anything is extracted. Only a version strictly newer than both the installed bundle's `CFBundleShortVersionString` and the highest version this machine ever installed is accepted, compared by semver precedence, so build metadata (`+...`) never makes a version newer. A build that failed here before is skipped until the next release: it is recorded by its archive's SHA-256 and by its release identity (the manifest's version and signature), which is checked before anything is downloaded. Only failures of the archive are recorded: a bad signature, one it can't be read from, one that breaks the extraction rules, or a bundle whose version isn't the manifest's. Nothing else is, since it can be this machine's fault: a full disk, a write error, or a `codesign` or `plutil` that refuses or couldn't run, as they also do for local reasons such as too many open files. Those are retried at the next check. The tools are given the updates folder's resolved path, the one whose every folder was checked, never a spelling through a symlink. After extraction, the bundle's version must equal the manifest's. `codesign --verify --deep --strict` must pass with a requirement anchored to Apple and pinned to Localify's team ID and bundle identifier, covering the daemon's executable too. Freshness is not guaranteed: a compromised feed could withhold an update or offer an older valid one to an older client, and this version accepts that.
- **Extraction:** into a new private directory on the destination's volume, made by the installer. No other account may be able to change any folder above it: from `/` down to the updates folder, each must be owned by this account or root, have no group or other write in its mode, and have no ACL entry that lets another account add, delete or rename what it holds, or change its ACL or owner (`add_file`, `add_subdirectory`, `delete_child`, `delete`, `writesecurity`, `chown`). Denials, entries for this account, and entries only passed on to new folders don't count. Otherwise the installer stops before extracting, and the status's `reason` names the folder: whoever can change it could rename a folder below and put their own tree where the staged bundle is. Each folder is opened from the one above it, never through a symlink, and the new directory stays open until the swap. The updates folder, `staging/` and the new directory must be owned by this account; the installer removes write access for anyone else from their mode, clears their ACLs (as `chmod -N` does, so an inherited ACL can't let another account in), and checks the new directory is empty. The archive may hold exactly one `Localify.app`, with no absolute or `..` paths, no hard links, no device or FIFO entries, no setuid or setgid bits, no data in entries that aren't files, and only relative symlinks that stay inside the bundle. Every path is created one component at a time from a directory descriptor, never following a symlink, and symlinks are made only after every file and folder, so a name that differs only in case on a case-insensitive volume can't lead outside. The archive is read in tar's raw mode: every header counts toward 20,000 entries, including GNU long-name and PAX headers, whose contents are capped at 1 MiB together. Of PAX records, only `path` and `linkpath` are used; `size`, sparse records and global headers are refused. The files are capped at 1 GiB, and the decompressed stream at that plus the headers. Quarantine is not added and never cleared; the archive is notarized and stapled.
- **Where:** only the production bundle whose executable is the daemon's, at `/Applications/Localify.app` or `~/Applications/Localify.app`, when the user owns it and can replace it. A dev build, a translocated or read-only bundle, a mounted DMG, or a location that would need authorization stays manual, and Settings says why. The app applies the same rules to its own bundle, since `Restart to update` hands off to that bundle's CLI: a copy opened from a DMG or Downloads offers no `Restart to update` and says why, even when the daemon's bundle could be replaced. It checks again right before it hands off and quits.
- **One installer:** the `localify` binary's own installer, in the core, enforces every rule here. The daemon runs it unattended, and `localify update install` runs it when a person asks. The app has no updater of its own: it reads `update_status`, runs the same coalesced check for `Check now` and the `Check for Updates…` menu, and installs through this installer. Tauri's updater plugin is not used; the bundler still signs the updater archive with the key in `tauri.conf.json`.
- **Is the app running:** the app takes an exclusive `flock` on `~/Library/Application Support/Localify/app.lock` as its first step and holds it for its lifetime. An app that finds `app.lock` held while `update.lock` is held too waits for the install, saying "Updating Localify…" after a second. When `app.lock` stays held without `update.lock`, another copy of the app holds it, and this one hands its launch over without waiting. It tries `app.lock` again for a tenth of a second first, since an installer lets go of `update.lock` an instant before `app.lock`, and the daemon's check takes `app.lock` alone for an instant. If no other copy takes the launch, this one is the copy that runs, and it takes the lock then. Whatever it found, before its window opens the app checks whether its bundle now holds a newer version than its own, and runs that executable instead. The installer runs only when it can take `app.lock`, and also finds no process whose executable lies inside the bundle (`proc_listallpids` and `proc_pidpath`). That second check also covers apps from 1.6 and earlier, which take no lock, and other accounts using the same bundle. If it can't read a process's path, it doesn't install. Two cases are not "in use": the bundle's own `localify`, which the daemon and the CLI run from and which outlive a replacement, and a process whose executable was deleted, which can't be running the bundle's files on disk.
- **When the daemon installs:** only when both checks pass. Otherwise it reports the update as available and tries again at the next check.
- **Swap:** under an exclusive `flock` on `~/Library/Application Support/Localify/update.lock`, a file that is never deleted, in the bundle owner's account, at a path that doesn't depend on the configurable data dir. Only the owner's account installs. The installer checks again that the destination is the same inode and version it verified, and that the staged bundle still has the version and passes the same `codesign` check, and fails without touching it when the volume lacks `RENAME_SWAP`. `codesign` and `plutil` need a path, so right before and right after each of them the installer checks that the path still leads to the bundle in the staging directory it holds open. It then exchanges the staged bundle and the installed one with `renameatx_np`, naming the staged one from that open directory and the installed one from its open folder, so nothing that renames a folder above them changes what is swapped. The old bundle moves to `~/Library/Application Support/Localify/previous/<version>.app`, which keeps one previous version; the copy kept before is removed only once the new one is in `previous/`. If the move fails, the old bundle stays in its staging directory, and the install's output and the status's `reason` say where, until it is deleted. The daemon restarts on the new binary, as it already does after any replacement. A destination that changed before that check makes the installer stop and leave it as it is. A person replacing the bundle at the very same moment, from a DMG for example, is not supported.
- **With the app open:** the app shows an available update in Settings → `Install updates automatically` and as a banner, with `Restart to update` when this copy may be replaced. Restarting lists what it will close by name: the running service tabs and the open shell tabs. On the person's click, the app starts `localify update install --relaunch` from its bundle's own CLI, detached in its own session (`setsid`) with stdin on `/dev/null`, then quits, which releases `app.lock`. If that process can't start, the app says why and stays open. The installer downloads after the app quits. That process waits for `app.lock`, installs, and opens the new app in the same login session. The app never relaunches by itself.
- **Status:** `update_status` on the management socket answers `{installed, available, state, reason, automatic, lastCheckAt, canInstall}`, and so does `localify update --status` (JSON when stdout is not a terminal; read locally when the daemon isn't running). `state` is `unknown` (never checked), `upToDate`, `available`, `checking`, `installing` or `failed` (the last check failed, or `updates.json` can't be trusted). `reason` says why an available update waits, why the check failed, or where a previous bundle was left. `lastCheckAt` is the last check's time in Unix seconds. `canInstall` is false when this copy can only be updated by hand (a dev build, a bundle in another place or owned by another account, Linux) or `updates.json` can't be trusted; `reason` then says why, and the app offers no `Restart to update`. The app also sets it false when its own bundle fails those rules, with that reason when an update waits. With the daemon down, the app asks the bundle's CLI for the same status. That is what the app and the CLI read, so nothing depends on a notification arriving.
- **Files:** `~/Library/Application Support/Localify/` holds `app.lock`, `update.lock`, `check.lock` (held during a check, so checks coalesce), `updates.json` (the highest version installed, the digests and releases that failed, the last check, its failures and what it read, the last unattended download), `staging/`, `previous/` and `updater.log`, the one updater log. A missing `updates.json` is a first use. One that can't be read or holds something invalid, such as a highest version that isn't a version, stops every check and install, with a `failed` status that names the file: reading it as new would forget the oldest version allowed and the releases that failed. To start again, delete it; the installed version is then the oldest one allowed. `localify::infrastructure::updates::app_lock::take_app_lock` is how the app takes its lock.
- **Recovery:** no automatic rollback. If a new version misbehaves, `~/Library/Application Support/Localify/previous/` holds the last one (or the staging directory the status names, when it couldn't be moved there): quit Localify, put that bundle back in place of the installed one, and turn `automatic` off until a fixed release.
- **Linux:** no unattended install, because the CLI there is checked by SHA-256 only. A Linux daemon that sees a newer version says "Update available: run the install command again" in `localify status` and in the app. It reads only the manifest's `version`; release builds get the manifest's address from `LOCALIFY_UPDATER_BASE_URL`, as the app does.

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
