#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
DEFAULT_UPDATER_KEY_PATH="${HOME}/.tauri/localify-updater.key"
PRODUCT_NAME="$(node -e "console.log(JSON.parse(require('node:fs').readFileSync('$ROOT_DIR/src-tauri/tauri.conf.json', 'utf8')).productName)")"
VERSION="$(node -e "console.log(JSON.parse(require('node:fs').readFileSync('$ROOT_DIR/src-tauri/tauri.conf.json', 'utf8')).version)")"
APP_PATH="$ROOT_DIR/src-tauri/target/release/bundle/macos/$PRODUCT_NAME.app"
MACOS_BUNDLE_DIR="$ROOT_DIR/src-tauri/target/release/bundle/macos"
DMG_BUNDLE_DIR="$ROOT_DIR/src-tauri/target/release/bundle/dmg"
DMG_ARCH="$(uname -m)"
if [ "$DMG_ARCH" = "arm64" ]; then
  DMG_ARCH="aarch64"
fi
DMG_PATH="$DMG_BUNDLE_DIR/${PRODUCT_NAME}_${VERSION}_${DMG_ARCH}.dmg"
ARCHIVE_PATH="$MACOS_BUNDLE_DIR/$PRODUCT_NAME.app.tar.gz"
ARCHIVE_SIGNATURE_PATH="$ARCHIVE_PATH.sig"
NOTARY_ATTEMPTS="${LOCALIFY_NOTARY_ATTEMPTS:-3}"
NOTARY_EXTRA_ARGS=()

if [ "${LOCALIFY_NOTARY_S3_ACCELERATION:-0}" != "1" ]; then
  NOTARY_EXTRA_ARGS+=(--no-s3-acceleration)
fi

if [ -z "${TAURI_SIGNING_PRIVATE_KEY:-}" ] && [ -f "$DEFAULT_UPDATER_KEY_PATH" ]; then
  export TAURI_SIGNING_PRIVATE_KEY="$(cat "$DEFAULT_UPDATER_KEY_PATH")"
elif [ -n "${TAURI_SIGNING_PRIVATE_KEY:-}" ] && [ -f "${TAURI_SIGNING_PRIVATE_KEY}" ]; then
  export TAURI_SIGNING_PRIVATE_KEY="$(cat "${TAURI_SIGNING_PRIVATE_KEY}")"
fi
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}"

if [ "$(uname -s)" != "Darwin" ]; then
  echo "Skipping macOS post-processing on non-macOS host"
  exit 0
fi

if [ ! -d "$APP_PATH" ]; then
  echo "Skipping macOS post-processing because app bundle is missing"
  exit 0
fi

NOTARY_AUTH=()
if [ -n "${APPLE_API_KEY:-}" ] && [ -n "${APPLE_API_ISSUER:-}" ] && [ -n "${APPLE_API_KEY_PATH:-}" ]; then
  NOTARY_AUTH=(--key "$APPLE_API_KEY_PATH" --key-id "$APPLE_API_KEY" --issuer "$APPLE_API_ISSUER")
elif [ -n "${APPLE_ID:-}" ] && [ -n "${APPLE_PASSWORD:-}" ] && [ -n "${APPLE_TEAM_ID:-}" ]; then
  NOTARY_AUTH=(--apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" --team-id "$APPLE_TEAM_ID")
fi

has_notary_credentials=0
if [ "${#NOTARY_AUTH[@]}" -gt 0 ]; then
  has_notary_credentials=1
fi

submit_for_notarization() {
  local artifact_path="$1"
  local attempt=1

  while [ "$attempt" -le "$NOTARY_ATTEMPTS" ]; do
    echo "Submitting $(basename "$artifact_path") for notarization (attempt $attempt/$NOTARY_ATTEMPTS)"
    if xcrun notarytool submit \
      "$artifact_path" \
      "${NOTARY_AUTH[@]}" \
      "${NOTARY_EXTRA_ARGS[@]}" \
      --wait; then
      return 0
    fi

    if [ "$attempt" -eq "$NOTARY_ATTEMPTS" ]; then
      return 1
    fi

    attempt=$((attempt + 1))
    sleep 10
  done
}

if [ "$has_notary_credentials" -eq 1 ]; then
  if xcrun stapler validate "$APP_PATH" >/dev/null 2>&1; then
    echo "App bundle already has a stapled notarization ticket"
  else
    echo "Submitting app bundle for notarization"
    app_zip_path="$MACOS_BUNDLE_DIR/$PRODUCT_NAME.app.zip"
    rm -f "$app_zip_path"
    ditto -c -k --keepParent "$APP_PATH" "$app_zip_path"
    submit_for_notarization "$app_zip_path"
    rm -f "$app_zip_path"

    echo "Stapling app bundle notarization ticket"
    xcrun stapler staple "$APP_PATH"
  fi

  echo "Rebuilding updater archive from notarized app bundle"
  rm -f "$ARCHIVE_PATH" "$ARCHIVE_SIGNATURE_PATH"
  COPYFILE_DISABLE=1 tar -czf "$ARCHIVE_PATH" -C "$MACOS_BUNDLE_DIR" "$PRODUCT_NAME.app"
  pnpm --silent tauri signer sign "$ARCHIVE_PATH" >/dev/null

  echo "Rebuilding DMG from notarized app bundle"
  rm -f "$DMG_BUNDLE_DIR"/"${PRODUCT_NAME}"_*.dmg
  staging_dir="$(mktemp -d)"
  cp -R "$APP_PATH" "$staging_dir/"
  ln -s /Applications "$staging_dir/Applications"
  hdiutil create \
    -volname "$PRODUCT_NAME" \
    -srcfolder "$staging_dir" \
    -ov \
    -format UDZO \
    "$DMG_PATH"
  rm -rf "$staging_dir"

  if [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
    echo "Signing DMG"
    codesign -s "$APPLE_SIGNING_IDENTITY" "$DMG_PATH"
  fi

  if xcrun stapler validate "$DMG_PATH" >/dev/null 2>&1; then
    echo "DMG already has a stapled notarization ticket"
  else
    echo "Submitting DMG for notarization"
    submit_for_notarization "$DMG_PATH"

    echo "Stapling DMG notarization ticket"
    xcrun stapler staple "$DMG_PATH"
  fi
else
  echo "Skipping notarization because neither the APPLE_API_* nor the APPLE_ID variables are all set"
fi

if [ -n "${APPLE_SIGNING_IDENTITY:-}" ] && [ "$has_notary_credentials" -eq 1 ]; then
  bash "$ROOT_DIR/scripts/verify-macos-signing.sh" "$APP_PATH" "$DMG_PATH"
else
  echo "Skipping signing verification because signing or notarization env vars are incomplete"
fi
