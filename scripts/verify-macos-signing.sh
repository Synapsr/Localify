#!/usr/bin/env bash

set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: $0 /path/to/app.app /path/to/app.dmg" >&2
  exit 1
fi

APP_PATH="$1"
DMG_PATH="$2"

if [ ! -d "$APP_PATH" ]; then
  echo "missing app bundle: $APP_PATH" >&2
  exit 1
fi

if [ ! -f "$DMG_PATH" ]; then
  echo "missing dmg: $DMG_PATH" >&2
  exit 1
fi

echo "Verifying codesign for app bundle"
codesign --verify --deep --strict --verbose=2 "$APP_PATH"

echo "Assessing app bundle with Gatekeeper"
spctl --assess --type execute --verbose=4 "$APP_PATH"

echo "Validating stapled notarization ticket on app bundle"
xcrun stapler validate "$APP_PATH"

echo "Verifying codesign for dmg"
codesign --verify --verbose=2 "$DMG_PATH"

echo "Validating stapled notarization ticket on dmg"
xcrun stapler validate "$DMG_PATH"

echo "macOS signing verification passed"
