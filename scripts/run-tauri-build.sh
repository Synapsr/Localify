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

cd "$ROOT_DIR"

pnpm desktop:check-version-sync
pnpm desktop:prepare-sidecar
pnpm tauri build --bundles app,dmg
pnpm desktop:postprocess-macos-bundles

if [ -n "${LOCALIFY_UPDATER_BASE_URL:-}" ]; then
  pnpm desktop:publish-updater
fi
