#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
PRODUCT_NAME="$(node -e "console.log(JSON.parse(require('node:fs').readFileSync('$ROOT_DIR/src-tauri/tauri.conf.json', 'utf8')).productName)")"
APP_PATH="$ROOT_DIR/src-tauri/target/release/bundle/macos/$PRODUCT_NAME.app"
DMG_PATH="$(find "$ROOT_DIR/src-tauri/target/release/bundle/dmg" -maxdepth 1 -name "${PRODUCT_NAME}_*.dmg" | head -n 1)"

if [ "$(uname -s)" != "Darwin" ]; then
  echo "Skipping macOS post-processing on non-macOS host"
  exit 0
fi

if [ ! -d "$APP_PATH" ]; then
  echo "Skipping macOS post-processing because app bundle is missing"
  exit 0
fi

if [ -z "$DMG_PATH" ] || [ ! -f "$DMG_PATH" ]; then
  echo "Skipping macOS post-processing because dmg is missing"
  exit 0
fi

has_notary_credentials=1
for var in APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID; do
  if [ -z "${!var:-}" ]; then
    has_notary_credentials=0
  fi
done

if [ "$has_notary_credentials" -eq 1 ]; then
  if xcrun stapler validate "$DMG_PATH" >/dev/null 2>&1; then
    echo "DMG already has a stapled notarization ticket"
  else
    echo "Submitting DMG for notarization"
    xcrun notarytool submit \
      "$DMG_PATH" \
      --apple-id "$APPLE_ID" \
      --password "$APPLE_PASSWORD" \
      --team-id "$APPLE_TEAM_ID" \
      --wait

    echo "Stapling DMG notarization ticket"
    xcrun stapler staple "$DMG_PATH"
  fi
else
  echo "Skipping DMG notarization because APPLE_ID / APPLE_PASSWORD / APPLE_TEAM_ID are not all set"
fi

if [ -n "${APPLE_SIGNING_IDENTITY:-}" ] && [ "$has_notary_credentials" -eq 1 ]; then
  bash "$ROOT_DIR/scripts/verify-macos-signing.sh" "$APP_PATH" "$DMG_PATH"
else
  echo "Skipping signing verification because signing or notarization env vars are incomplete"
fi
