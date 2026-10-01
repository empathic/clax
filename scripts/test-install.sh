#!/usr/bin/env bash
# Tests install.sh against scripts/fake-release-server.py on 127.0.0.1 (a
# port the kernel picks), with a scratch HOME. No network.
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
INSTALL="$(cd "$HERE/.." && pwd)/install.sh"
ROOT="$(cd "$(mktemp -d)" && pwd -P)"
SERVER_PID=""
# shellcheck disable=SC2329 # run by the EXIT trap
cleanup() {
    if [ -n "$SERVER_PID" ]; then kill "$SERVER_PID" 2>/dev/null; wait "$SERVER_PID" 2>/dev/null; fi
    rm -rf "$ROOT"
    return 0
}
trap cleanup EXIT
# The interpreter itself, not a version-manager shim that needs the real PATH.
PY="$(python3 -c 'import sys; print(sys.executable)')"
ORIG_PATH="$PATH"
FAILED=0
pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1"; FAILED=1; }
V=0.3.0
TARGETS="aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-musl aarch64-unknown-linux-musl"

# Release trees: good (clax $V) and wrong (an archive holding clax 0.0.9).
mkdir -p "$ROOT/payload" "$ROOT/release/good/v$V" "$ROOT/release/wrong/v$V" "$ROOT/stage"
printf '#!/bin/sh\necho "clax %s"\n' "$V" > "$ROOT/payload/clax"
chmod +x "$ROOT/payload/clax"
for t in $TARGETS; do
    "$HERE/package-release.sh" archive "$V" "$t" "$ROOT/payload/clax" "$ROOT/release/good/v$V" >/dev/null
    mkdir -p "$ROOT/stage/clax-$V-$t"
    printf '#!/bin/sh\necho "clax 0.0.9"\n' > "$ROOT/stage/clax-$V-$t/clax"
    chmod +x "$ROOT/stage/clax-$V-$t/clax"
    (cd "$ROOT/stage" && tar -czf "$ROOT/release/wrong/v$V/clax-$V-$t.tar.gz" "clax-$V-$t")
done
"$HERE/package-release.sh" sums "$ROOT/release/good/v$V" >/dev/null
"$HERE/package-release.sh" sums "$ROOT/release/wrong/v$V" >/dev/null
REQLOG="$ROOT/requests.log"
: > "$REQLOG"
(FAKE_LATEST="$V" exec "$PY" "$HERE/fake-release-server.py" "$ROOT/release" "$REQLOG" "$ROOT/port") &
SERVER_PID=$!
for _ in $(seq 50); do [ -s "$ROOT/port" ] && break; sleep 0.1; done
BASE="http://127.0.0.1:$(cat "$ROOT/port")"

new_env() {
    SANDBOX="$(mktemp -d "$ROOT/case.XXXXXX")"
    export HOME="$SANDBOX/home"
    mkdir -p "$HOME" "$SANDBOX/bin"
    export PATH="$SANDBOX/bin:$ORIG_PATH"
    unset CLAX_INSTALL_DIR CLAX_DOWNLOAD_TIMEOUT
    : > "$REQLOG"
}
fake_uname() {
    # shellcheck disable=SC2016 # $1 belongs to the generated script
    printf '#!/bin/sh\ncase "$1" in -s) echo %s ;; -m) echo %s ;; esac\n' "$1" "$2" > "$SANDBOX/bin/uname"
    chmod +x "$SANDBOX/bin/uname"
}
inst() { # mode [args...]
    local mode="$1"; shift
    OUT="$(CLAX_RELEASE_BASE_URL="$BASE/$mode" CLAX_RELEASE_LATEST_URL="$BASE/$mode/latest" bash "$INSTALL" "$@" 2>&1)"; RC=$?
}

new_env
fake_uname Linux x86_64
inst ok
if [ "$RC" = 0 ] && [ "$("$HOME/.local/bin/clax" --version)" = "clax $V" ] && grep -qx "/ok/latest" "$REQLOG" \
    && grep -qx "/ok/v$V/clax-$V-x86_64-unknown-linux-musl.tar.gz" "$REQLOG" && echo "$OUT" | grep -q "clax init"; then
    pass "the latest release is found, checked and installed into ~/.local/bin"
else fail "the latest release is installed (rc=$RC out=$OUT)"; fi

for pair in Darwin/arm64/aarch64-apple-darwin Darwin/x86_64/x86_64-apple-darwin Linux/aarch64/aarch64-unknown-linux-musl Linux/arm64/aarch64-unknown-linux-musl; do
    new_env
    fake_uname "${pair%%/*}" "$(echo "$pair" | cut -d/ -f2)"
    CLAX_INSTALL_DIR="$SANDBOX/dest" inst ok "v$V"
    if [ "$RC" = 0 ] && [ -x "$SANDBOX/dest/clax" ] && grep -qx "/ok/v$V/clax-$V-${pair##*/}.tar.gz" "$REQLOG" && ! grep -q latest "$REQLOG"; then
        pass "a named version on ${pair%/*} fetches ${pair##*/}"
    else fail "a named version on ${pair%/*} (rc=$RC out=$OUT)"; fi
done

new_env
fake_uname Linux x86_64
inst none "$V"
if [ "$RC" = 1 ] && echo "$OUT" | grep -q "answered HTTP 404 (the release may not exist, or the repository may not be public yet)" && [ ! -e "$HOME/.local/bin/clax" ]; then
    pass "a missing release (or a private repository) is named"
else fail "a missing release (rc=$RC out=$OUT)"; fi

new_env
fake_uname Linux x86_64
inst badsum "$V"
if [ "$RC" = 1 ] && echo "$OUT" | grep -q "checksum mismatch" && [ ! -e "$HOME/.local/bin/clax" ]; then
    pass "a checksum mismatch installs nothing"
else fail "a checksum mismatch (rc=$RC out=$OUT)"; fi

new_env
fake_uname Linux x86_64
inst partial "$V"
if [ "$RC" = 1 ] && echo "$OUT" | grep -q "was cut short" && [ ! -e "$HOME/.local/bin/clax" ]; then
    pass "a partial download installs nothing"
else fail "a partial download (rc=$RC out=$OUT)"; fi

new_env
fake_uname Linux x86_64
start=$(date +%s)
CLAX_DOWNLOAD_TIMEOUT=2 inst slow "$V"
took=$(( $(date +%s) - start ))
if [ "$RC" = 1 ] && echo "$OUT" | grep -q "timed out after 2s" && [ "$took" -lt 10 ] && [ ! -e "$HOME/.local/bin/clax" ]; then
    pass "a stalled download times out within its bound"
else fail "a stalled download times out (took=${took}s rc=$RC out=$OUT)"; fi

new_env
fake_uname Linux x86_64
inst wrong "$V"
if [ "$RC" = 1 ] && echo "$OUT" | grep -q "does not hold clax $V" && [ ! -e "$HOME/.local/bin/clax" ]; then
    pass "an archive holding another version is refused"
else fail "an archive holding another version (rc=$RC out=$OUT)"; fi

new_env
fake_uname FreeBSD x86_64
inst ok "$V"
if [ "$RC" = 1 ] && echo "$OUT" | grep -q "no prebuilt clax for FreeBSD/x86_64" && [ ! -s "$REQLOG" ]; then
    pass "an unsupported platform says so without downloading"
else fail "an unsupported platform (rc=$RC out=$OUT)"; fi

new_env
fake_uname Linux x86_64
mkdir -p "$HOME/.local/bin"
printf '#!/bin/sh\necho "clax 0.0.1"\n' > "$HOME/.local/bin/clax"
chmod +x "$HOME/.local/bin/clax"
exec 3< "$HOME/.local/bin/clax"
inst ok "$V"
if [ "$RC" = 0 ] && [ "$("$HOME/.local/bin/clax" --version)" = "clax $V" ] && grep -q "0.0.1" <&3 \
    && [ -z "$(find "$HOME/.local/bin" -name '.clax.*')" ]; then
    pass "an existing clax is replaced by one rename; an open copy keeps the old file"
else fail "an existing clax is replaced by rename (rc=$RC out=$OUT)"; fi
exec 3<&-

[ "$FAILED" = 0 ] && echo "installer tests passed" || echo "installer tests FAILED"
exit "$FAILED"
