#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
DEFAULT_UPDATER_KEY_PATH="${HOME}/.tauri/localify-updater.key"

if [ -f "$ROOT_DIR/.env" ]; then
  set -a
  # shellcheck disable=SC1091
  . "$ROOT_DIR/.env"
  set +a
fi

if [ -z "${TAURI_SIGNING_PRIVATE_KEY:-}" ] && [ -f "$DEFAULT_UPDATER_KEY_PATH" ]; then
  export TAURI_SIGNING_PRIVATE_KEY="$(cat "$DEFAULT_UPDATER_KEY_PATH")"
elif [ -n "${TAURI_SIGNING_PRIVATE_KEY:-}" ] && [ -f "${TAURI_SIGNING_PRIVATE_KEY}" ]; then
  export TAURI_SIGNING_PRIVATE_KEY="$(cat "${TAURI_SIGNING_PRIVATE_KEY}")"
fi

export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}"

# A keychain kept only for releases stays locked outside builds. Machines that sign
# from their login keychain don't have one.
RELEASE_KEYCHAIN="$HOME/Library/Keychains/localify-release.keychain-db"
if [ -n "${LOCALIFY_RELEASE_KEYCHAIN_PASSWORD:-}" ] && [ -f "$RELEASE_KEYCHAIN" ]; then
  security unlock-keychain -p "$LOCALIFY_RELEASE_KEYCHAIN_PASSWORD" "$RELEASE_KEYCHAIN"
  trap 'security lock-keychain "$RELEASE_KEYCHAIN"' EXIT
fi
unset LOCALIFY_RELEASE_KEYCHAIN_PASSWORD

cd "$ROOT_DIR"

pnpm check-version-sync
pnpm desktop:prepare-sidecar

if [ "$(uname -s)" = "Darwin" ]; then
  rm -rf "$ROOT_DIR/src-tauri/target/release/bundle/macos"
  rm -rf "$ROOT_DIR/src-tauri/target/release/bundle/dmg"
fi

# Tauri only signs here. The post-processing notarizes the final app and DMG.
env -u APPLE_ID -u APPLE_PASSWORD -u APPLE_TEAM_ID \
  -u APPLE_API_KEY -u APPLE_API_ISSUER -u APPLE_API_KEY_PATH \
  pnpm tauri build --bundles app,dmg
pnpm desktop:postprocess-macos-bundles

if [ -n "${LOCALIFY_UPDATER_BASE_URL:-}" ]; then
  pnpm desktop:publish-updater
fi
