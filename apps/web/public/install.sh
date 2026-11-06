#!/bin/sh
# Installs or updates the Localify command line, then sets Localify up.
#
#   curl -fsSL https://localify.dev/install.sh | sh
#
# Run it again to update: the background service restarts on the new binary.
#
# Setting up asks for an administrator's approval once, through sudo, for DNS
# and the background service. Over SSH, keep a terminal so sudo can ask:
#
#   ssh -t <host> 'curl -fsSL https://localify.dev/install.sh | sh'
#
# LOCALIFY_VERSION=1.3.2       installs this version instead of the latest one
# LOCALIFY_INSTALL_DIR=<dir>   installs the command there, not in ~/.local/bin
# LOCALIFY_NO_SETUP=1          installs the command only, without `localify install`
#
# Source: https://github.com/Synapsr/Localify/blob/main/apps/web/public/install.sh

set -eu

releases=${LOCALIFY_RELEASES_URL:-https://github.com/Synapsr/Localify/releases}
dir=${LOCALIFY_INSTALL_DIR:-$HOME/.local/bin}
app=/Applications/Localify.app

fail() {
  printf 'Localify: %s\n' "$1" >&2
  exit 1
}

system=$(uname -s)
machine=$(uname -m)
case "$system $machine" in
  "Darwin arm64") target=macos-arm64 ;;
  "Linux x86_64") target=linux-amd64 ;;
  *) fail "there is no build for $system on $machine yet. Build it from source: https://github.com/Synapsr/Localify" ;;
esac

if [ "$system" = Darwin ] && [ -d "$app" ]; then
  echo "The Localify app is installed and updates itself."
  echo "Its command is localify, or $app/Contents/MacOS/localify."
  echo "If it is not set up yet, open it, or run: $app/Contents/MacOS/localify install"
  exit 0
fi

command -v curl >/dev/null 2>&1 || fail "curl is needed to download Localify."

version=${LOCALIFY_VERSION:-}
if [ -z "$version" ]; then
  # GitHub redirects the latest release to .../releases/tag/v<version>.
  latest=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "$releases/latest") ||
    fail "could not reach $releases."
  version=${latest##*/tag/}
fi
version=${version#v}
case $version in
  "" | *[!0-9A-Za-z.+-]*) fail "could not tell which version to install (got '$version')." ;;
esac

archive="localify-$version-$target.tar.gz"
url="$releases/download/v$version/$archive"
work=$(mktemp -d)
partial="$dir/.localify.$$"
trap 'rm -rf "$work"; rm -f "$partial"' EXIT
trap 'exit 1' INT TERM

echo "Downloading Localify $version for $target..."
curl -fsSL "$url" -o "$work/$archive" || fail "could not download $url."
curl -fsSL "$url.sha256" -o "$work/$archive.sha256" || fail "could not download $url.sha256."

expected=$(cut -d ' ' -f 1 <"$work/$archive.sha256")
if command -v sha256sum >/dev/null 2>&1; then
  actual=$(sha256sum "$work/$archive" | cut -d ' ' -f 1)
else
  actual=$(shasum -a 256 "$work/$archive" | cut -d ' ' -f 1)
fi
if [ -z "$expected" ] || [ "$actual" != "$expected" ]; then
  fail "the download does not match its SHA-256 checksum, so nothing was installed."
fi

tar -xzf "$work/$archive" -C "$work" localify || fail "$archive holds no localify command."
mkdir -p "$dir"
# A copy next to the old command, then a rename: a running daemon never sees
# half a file, and notices the new one to restart on it.
cp "$work/localify" "$partial"
chmod 755 "$partial"
mv -f "$partial" "$dir/localify"
echo "Installed $("$dir/localify" --version) at $dir/localify."

case ":$PATH:" in
  *":$dir:"*) ;;
  *) echo "$dir is not on your PATH. Add this line to your shell profile: export PATH=\"$dir:\$PATH\"" ;;
esac

if [ "${LOCALIFY_NO_SETUP:-}" = 1 ]; then
  echo "Set it up with: $dir/localify install"
  exit 0
fi

# Piped from curl, this script is sudo's standard input: let it ask on the terminal.
if (: </dev/tty) 2>/dev/null; then
  "$dir/localify" install </dev/tty
else
  "$dir/localify" install
fi
