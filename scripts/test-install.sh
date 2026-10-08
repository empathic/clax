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
# shellcheck source=scripts/fake-exe.sh
. "$HERE/fake-exe.sh"
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
    unset CLAX_INSTALL_DIR CLAX_DOWNLOAD_TIMEOUT CLAX_ALLOW_ROOT
    : > "$REQLOG"
    fake_uid 1000
}
fake_uid() { # what `id -u` prints; the tests never run as root
    printf '#!/bin/sh\necho %s\n' "$1" | fake_exe "$SANDBOX/bin/id"
}
fake_uname() { # os arch [hw.optional.arm64, default 0]
    # shellcheck disable=SC2016 # $1 belongs to the generated script
    printf '#!/bin/sh\ncase "$1" in -s) echo %s ;; -m) echo %s ;; esac\n' "$1" "$2" | fake_exe "$SANDBOX/bin/uname"
    printf '#!/bin/sh\necho %s\n' "${3:-0}" | fake_exe "$SANDBOX/bin/sysctl"
}
# Prints a directory holding links to the tools install.sh uses, minus those
# named, for a PATH of "$SANDBOX/bin:<that directory>".
only_tools() {
    local dir="$SANDBOX/tools" c skip
    mkdir -p "$dir"
    for c in bash curl tar gzip awk mktemp rm mkdir cp chmod mv sha256sum shasum perl; do
        for skip in "$@"; do [ "$c" = "$skip" ] && continue 2; done
        if command -v "$c" >/dev/null 2>&1; then ln -sf "$(command -v "$c")" "$dir/$c"; fi
    done
    echo "$dir"
}
inst() { # mode [args...]
    local mode="$1"; shift
    OUT="$(CLAX_RELEASE_BASE_URL="$BASE/$mode" CLAX_RELEASE_LATEST_URL="$BASE/$mode/latest" bash "$INSTALL" "$@" 2>&1)"; RC=$?
}

new_env
fake_uname Linux x86_64
inst ok
if [ "$RC" = 0 ] && [ "$("$HOME/.local/bin/clax" --version)" = "clax $V" ] && grep -qx "/ok/latest" "$REQLOG" \
    && grep -qx "/ok/v$V/clax-$V-x86_64-unknown-linux-musl.tar.gz" "$REQLOG" && echo "$OUT" | grep "clax init" >/dev/null; then
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
if [ "$RC" = 1 ] && echo "$OUT" | grep "answered HTTP 404 (the release may not exist, or the repository may not be public yet)" >/dev/null && [ ! -e "$HOME/.local/bin/clax" ]; then
    pass "a missing release (or a private repository) is named"
else fail "a missing release (rc=$RC out=$OUT)"; fi

new_env
fake_uname Linux x86_64
inst badsum "$V"
if [ "$RC" = 1 ] && echo "$OUT" | grep "checksum mismatch" >/dev/null && [ ! -e "$HOME/.local/bin/clax" ]; then
    pass "a checksum mismatch installs nothing"
else fail "a checksum mismatch (rc=$RC out=$OUT)"; fi

new_env
fake_uname Linux x86_64
inst partial "$V"
if [ "$RC" = 1 ] && echo "$OUT" | grep "was cut short" >/dev/null && [ ! -e "$HOME/.local/bin/clax" ]; then
    pass "a partial download installs nothing"
else fail "a partial download (rc=$RC out=$OUT)"; fi

new_env
fake_uname Linux x86_64
start=$(date +%s)
CLAX_DOWNLOAD_TIMEOUT=2 inst slow "$V"
took=$(( $(date +%s) - start ))
if [ "$RC" = 1 ] && echo "$OUT" | grep "timed out after 2s" >/dev/null && [ "$took" -lt 10 ] && [ ! -e "$HOME/.local/bin/clax" ]; then
    pass "a stalled download times out within its bound"
else fail "a stalled download times out (took=${took}s rc=$RC out=$OUT)"; fi

new_env
fake_uname Linux x86_64
inst wrong "$V"
if [ "$RC" = 1 ] && echo "$OUT" | grep "does not hold clax $V" >/dev/null && [ ! -e "$HOME/.local/bin/clax" ]; then
    pass "an archive holding another version is refused"
else fail "an archive holding another version (rc=$RC out=$OUT)"; fi

new_env
fake_uname FreeBSD x86_64
inst ok "$V"
if [ "$RC" = 1 ] && echo "$OUT" | grep "no prebuilt clax for FreeBSD/x86_64" >/dev/null && [ ! -s "$REQLOG" ]; then
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

new_env
fake_uname Darwin x86_64 1
CLAX_INSTALL_DIR="$SANDBOX/dest" inst ok "$V"
if [ "$RC" = 0 ] && grep -qx "/ok/v$V/clax-$V-aarch64-apple-darwin.tar.gz" "$REQLOG"; then
    pass "a shell under Rosetta on Apple silicon still fetches aarch64-apple-darwin"
else fail "a shell under Rosetta (rc=$RC out=$OUT)"; fi

new_env
fake_uname Linux x86_64
fake_uid 0
inst ok "$V"
if [ "$RC" = 1 ] && echo "$OUT" | grep "will not run as root" >/dev/null && [ ! -s "$REQLOG" ] && [ ! -e "$HOME/.local" ]; then
    pass "root is refused before anything is downloaded or created"
else fail "root is refused (rc=$RC out=$OUT)"; fi

new_env
fake_uname Linux x86_64
fake_uid 0
CLAX_INSTALL_DIR="$SANDBOX/sys" inst ok "$V"
RC1=$RC
CLAX_ALLOW_ROOT=1 inst ok "$V"
if [ "$RC1" = 1 ] && [ "$RC" = 1 ] && [ ! -s "$REQLOG" ] && [ ! -e "$SANDBOX/sys" ] && [ ! -e "$HOME/.local" ]; then
    pass "root with only one of CLAX_INSTALL_DIR and CLAX_ALLOW_ROOT is refused"
else fail "root with one opt-in variable (rc=$RC1,$RC out=$OUT)"; fi

new_env
fake_uname Linux x86_64
fake_uid 0
CLAX_ALLOW_ROOT=1 CLAX_INSTALL_DIR="$SANDBOX/sys" inst ok "$V"
if [ "$RC" = 0 ] && [ "$("$SANDBOX/sys/clax" --version)" = "clax $V" ] && [ ! -e "$HOME/.local" ]; then
    pass "root installs into an explicit CLAX_INSTALL_DIR with CLAX_ALLOW_ROOT=1"
else fail "root opt-in (rc=$RC out=$OUT)"; fi

new_env
fake_uname Linux x86_64
inst none
if [ "$RC" = 1 ] && echo "$OUT" | grep "could not find the latest release at .*(the repository may not be public yet)" >/dev/null && [ ! -e "$HOME/.local" ]; then
    pass "no latest release (or a private repository) is named"
else fail "no latest release (rc=$RC out=$OUT)"; fi

new_env
fake_uname Linux x86_64
PATH="$SANDBOX/bin:$(only_tools curl)" inst ok "$V"
if [ "$RC" = 1 ] && echo "$OUT" | grep "curl is required" >/dev/null && [ ! -s "$REQLOG" ]; then
    pass "a missing curl is named"
else fail "a missing curl (rc=$RC out=$OUT)"; fi

new_env
fake_uname Linux x86_64
PATH="$SANDBOX/bin:$(only_tools sha256sum shasum)" inst ok "$V"
if [ "$RC" = 1 ] && echo "$OUT" | grep "sha256sum or shasum is required" >/dev/null && [ ! -s "$REQLOG" ]; then
    pass "a missing checksum tool is named"
else fail "a missing checksum tool (rc=$RC out=$OUT)"; fi

new_env
fake_uname Linux x86_64
if command -v shasum >/dev/null 2>&1; then
    PATH="$SANDBOX/bin:$(only_tools sha256sum)" inst ok "$V"
    if [ "$RC" = 0 ] && [ "$("$HOME/.local/bin/clax" --version)" = "clax $V" ]; then
        pass "shasum checks the archive when sha256sum is missing"
    else fail "the shasum fallback (rc=$RC out=$OUT)"; fi
else echo "SKIP: the shasum fallback (no shasum here)"; fi

new_env
fake_uname Linux x86_64
for f in .bashrc .zshrc .profile; do echo "# mine" > "$HOME/$f"; done
inst ok "$V"
files="$(cd "$HOME" && find . -type f | sort | tr '\n' ' ')"
if [ "$RC" = 0 ] && echo "$OUT" | grep 'export PATH=' >/dev/null && [ "$files" = "./.bashrc ./.local/bin/clax ./.profile ./.zshrc " ] \
    && [ "$(cat "$HOME/.bashrc" "$HOME/.zshrc" "$HOME/.profile")" = "$(printf '# mine\n# mine\n# mine')" ]; then
    pass "an install directory off PATH is only reported; no rc file is touched"
else fail "rc files untouched (rc=$RC files=$files out=$OUT)"; fi

new_env
fake_uname Linux x86_64
mkdir -p "$SANDBOX/ro"
chmod 555 "$SANDBOX/ro"
CLAX_INSTALL_DIR="$SANDBOX/ro" inst ok "$V"
chmod 755 "$SANDBOX/ro"
if [ "$RC" != 0 ] && [ -z "$(ls -A "$SANDBOX/ro")" ]; then
    pass "a failed copy leaves nothing in the install directory"
else fail "a failed copy (rc=$RC out=$OUT)"; fi

new_env
fake_uname Linux x86_64
mkdir -p "$SANDBOX/dest"
printf '#!/bin/sh\nexit 1\n' | fake_exe "$SANDBOX/bin/mv"
CLAX_INSTALL_DIR="$SANDBOX/dest" inst ok "$V"
rm -f "$SANDBOX/bin/mv"
if [ "$RC" != 0 ] && [ -z "$(ls -A "$SANDBOX/dest")" ]; then
    pass "a staged binary whose rename fails is removed on exit"
else fail "a staged binary is removed on exit (rc=$RC out=$OUT)"; fi

[ "$FAILED" = 0 ] && echo "installer tests passed" || echo "installer tests FAILED"
exit "$FAILED"
