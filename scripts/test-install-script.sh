#!/bin/sh
# Runs apps/web/public/install.sh against a local server shaped like GitHub
# releases, with a fake `localify` and a fake `uname`.
#
#   pnpm test:install-script

set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
script="$root/apps/web/public/install.sh"
work=$(mktemp -d)
server_pid=
cleanup() {
  if [ -n "$server_pid" ]; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
  rm -rf "$work"
}
trap cleanup EXIT

failures=0
pass() { printf 'ok   %s\n' "$1"; }
fail() {
  printf 'FAIL %s\n' "$1" >&2
  failures=$((failures + 1))
}

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d ' ' -f 1
  else
    shasum -a 256 "$1" | cut -d ' ' -f 1
  fi
}

# A release: the archive of a fake `localify`, and its checksum file.
release() {
  version=$1
  checksum=$2
  dir="$work/site/releases/download/v$version"
  mkdir -p "$dir" "$work/build"
  cat >"$work/build/localify" <<EOF
#!/bin/sh
case "\$1" in
  --version) echo "localify $version" ;;
  install) echo "install ran" >"$work/install.log" ;;
esac
EOF
  chmod 755 "$work/build/localify"
  archive="localify-$version-linux-amd64.tar.gz"
  tar -czf "$dir/$archive" -C "$work/build" localify
  if [ "$checksum" = good ]; then
    printf '%s  %s\n' "$(sha256 "$dir/$archive")" "$archive" >"$dir/$archive.sha256"
  else
    printf '%s  %s\n' "0000000000000000000000000000000000000000000000000000000000000000" "$archive" >"$dir/$archive.sha256"
  fi
}

release 9.9.9 good
release 9.9.8 bad
mkdir -p "$work/site/releases/tag"
echo "release page" >"$work/site/releases/tag/v9.9.9"

mkdir -p "$work/bin" "$work/home"
cat >"$work/bin/uname" <<'EOF'
#!/bin/sh
case "$1" in
  -s) echo "${FAKE_UNAME_S:-Linux}" ;;
  -m) echo "${FAKE_UNAME_M:-x86_64}" ;;
esac
EOF
chmod 755 "$work/bin/uname"

cat >"$work/server.py" <<'EOF'
import http.server, sys

class Releases(http.server.SimpleHTTPRequestHandler):
    def redirect_latest(self):
        if self.path != "/releases/latest":
            return False
        self.send_response(302)
        self.send_header("Location", "/releases/tag/v9.9.9")
        self.end_headers()
        return True

    def do_HEAD(self):
        if not self.redirect_latest():
            super().do_HEAD()

    def do_GET(self):
        if not self.redirect_latest():
            super().do_GET()

    def log_message(self, *args):
        pass

server = http.server.ThreadingHTTPServer(
    ("127.0.0.1", 0),
    lambda *args: Releases(*args, directory=sys.argv[1]),
)
with open(sys.argv[2], "w") as port:
    port.write(str(server.server_port))
server.serve_forever()
EOF
python3 -I "$work/server.py" "$work/site" "$work/port" &
server_pid=$!
i=0
while [ ! -s "$work/port" ]; do
  i=$((i + 1))
  [ "$i" -lt 50 ] || { echo "The test server did not start." >&2; exit 1; }
  sleep 0.1
done
releases="http://127.0.0.1:$(cat "$work/port")/releases"

# Runs the script as `curl | sh` would, with the fake `uname` first on PATH.
run() {
  env HOME="$work/home" PATH="$work/bin:$PATH" LOCALIFY_RELEASES_URL="$releases" "$@" \
    sh -c "sh <'$script'" >"$work/out" 2>&1
}
installed="$work/home/.local/bin/localify"

if run LOCALIFY_NO_SETUP=1 && [ "$("$installed" --version)" = "localify 9.9.9" ] &&
  [ ! -e "$work/install.log" ] && grep -q "is not on your PATH" "$work/out"; then
  pass "installs the latest release, checked, without setting up"
else
  fail "installs the latest release: $(cat "$work/out")"
fi

if run LOCALIFY_VERSION=v9.9.9 </dev/null && grep -q "install ran" "$work/install.log"; then
  pass "installs a chosen version, then runs localify install"
else
  fail "runs localify install: $(cat "$work/out")"
fi

if ! run LOCALIFY_VERSION=9.9.8 && grep -q "does not match its SHA-256 checksum" "$work/out" &&
  [ "$("$installed" --version)" = "localify 9.9.9" ] &&
  [ -z "$(find "$work/home/.local/bin" -name '.localify.*')" ]; then
  pass "refuses a download whose checksum does not match, and keeps the installed one"
else
  fail "refuses a bad checksum: $(cat "$work/out")"
fi

if ! run FAKE_UNAME_S=FreeBSD FAKE_UNAME_M=amd64 && grep -q "no build for FreeBSD on amd64" "$work/out"; then
  pass "says when the platform has no build"
else
  fail "refuses an unsupported platform: $(cat "$work/out")"
fi

if ! run LOCALIFY_VERSION='1.0;rm' && grep -q "could not tell which version" "$work/out"; then
  pass "refuses a version that is not one"
else
  fail "refuses a malformed version: $(cat "$work/out")"
fi

[ "$failures" -eq 0 ] || exit 1
