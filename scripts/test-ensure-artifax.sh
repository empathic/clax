#!/usr/bin/env bash
# Hermetic tests for ensure-artifax.sh: temp HOME, a PATH holding only
# symlinks to the tools the script needs plus fake `artifax` binaries, and a
# local http.server standing in for GitHub releases. No network is used.
set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
SCRIPT="$HERE/ensure-artifax.sh"
ROOT="$(mktemp -d)"
SERVER_PID=""
cleanup() {
    if [ -n "$SERVER_PID" ]; then kill "$SERVER_PID" 2>/dev/null; wait "$SERVER_PID" 2>/dev/null; fi
    rm -rf "$ROOT"
    return 0
}
trap cleanup EXIT

ORIG_PATH="$PATH"
FAILED=0
pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1"; FAILED=1; }

# Tools the script may call, linked into an otherwise empty directory.
TOOLS="$ROOT/tools"
mkdir -p "$TOOLS"
for t in bash env awk sort head grep curl tar sha256sum shasum mktemp uname mv chmod mkdir rm cat dirname sed tr; do
    if p="$(command -v "$t" 2>/dev/null)" && [ -x "$p" ]; then ln -sf "$p" "$TOOLS/$t"; fi
done

fake_artifax() { # dir version-line
    mkdir -p "$1"
    printf '#!/bin/sh\nif [ "$1" = "--version" ]; then echo "%s"; else echo "args: $*"; fi\n' "$2" > "$1/artifax"
    chmod +x "$1/artifax"
}

# Fresh sandbox: sets HOME and PATH, clears every override.
new_env() {
    SANDBOX="$(mktemp -d "$ROOT/case.XXXXXX")"
    export HOME="$SANDBOX/home"
    mkdir -p "$HOME"
    FAKEBIN="$SANDBOX/fakebin"
    mkdir -p "$FAKEBIN"
    export PATH="$FAKEBIN:$TOOLS"
    unset ARTIFAX_BIN ARTIFAX_INSTALL_DIR ARTIFAX_CONFIG_DIR ARTIFAX_RELEASE_BASE_URL ARTIFAX_RELEASE_VERSION
}

run() { OUT="$("$TOOLS/bash" "$SCRIPT" "$@" 2>"$SANDBOX/stderr")"; RC=$?; ERR="$(cat "$SANDBOX/stderr")"; }

# --- Resolution -------------------------------------------------------------

new_env
fake_artifax "$SANDBOX/explicit" "artifax 0.2.0"
fake_artifax "$FAKEBIN" "artifax 0.9.9"
ARTIFAX_BIN="$SANDBOX/explicit/artifax" run
if [ "$RC" = 0 ] && [ "$OUT" = "$SANDBOX/explicit/artifax" ]; then pass "ARTIFAX_BIN wins over PATH"; else fail "ARTIFAX_BIN wins over PATH (rc=$RC out=$OUT)"; fi

new_env
fake_artifax "$FAKEBIN" "artifax 0.2.0"
ARTIFAX_BIN="$SANDBOX/missing" run
if [ "$RC" = 0 ] && [ "$OUT" = "$FAKEBIN/artifax" ] && echo "$ERR" | grep -q "ARTIFAX_BIN"; then
    pass "unusable ARTIFAX_BIN warns and falls through"
else fail "unusable ARTIFAX_BIN warns and falls through (rc=$RC out=$OUT err=$ERR)"; fi

new_env
fake_artifax "$FAKEBIN" "somethingelse 1.0"
fake_artifax "$HOME/.local/bin" "artifax 0.2.0"
run
if [ "$RC" = 0 ] && [ "$OUT" = "$HOME/.local/bin/artifax" ]; then pass "foreign artifax on PATH is skipped"; else fail "foreign artifax on PATH is skipped (rc=$RC out=$OUT)"; fi

new_env
fake_artifax "$FAKEBIN" "somethingelse 1.0"
fake_artifax "$HOME/.artifax/bin" "artifax 0.2.0"
run
if [ "$RC" = 0 ] && [ "$OUT" = "$HOME/.artifax/bin/artifax" ]; then pass "fallback bin dir is found"; else fail "fallback bin dir is found (rc=$RC out=$OUT)"; fi

new_env
fake_artifax "$FAKEBIN" "artifax 0.2.0"
run exec one "two words"
if [ "$RC" = 0 ] && [ "$OUT" = "args: one two words" ]; then pass "exec passes arguments through"; else fail "exec passes arguments through (rc=$RC out=$OUT)"; fi

new_env
fake_artifax "$FAKEBIN" "artifax 0.1.0"
run
if [ "$RC" = 0 ] && echo "$ERR" | grep -q "older than 0.2.0"; then pass "version warning fires for 0.1.0"; else fail "version warning fires for 0.1.0 (rc=$RC err=$ERR)"; fi

new_env
fake_artifax "$FAKEBIN" "artifax 0.2.0"
run bogus
if [ "$RC" = 2 ] && echo "$ERR" | grep -q usage; then pass "unknown subcommand prints usage"; else fail "unknown subcommand prints usage (rc=$RC)"; fi

# --- Download ---------------------------------------------------------------

PATH="$ORIG_PATH"
# Serve a fake release for every supported target under /v0.2.0/.
SERVE="$ROOT/serve"
mkdir -p "$SERVE/v0.2.0" "$ROOT/payload"
fake_artifax "$ROOT/payload" "artifax 0.2.0"
for target in aarch64-apple-darwin x86_64-unknown-linux-musl aarch64-unknown-linux-musl; do
    tarball="artifax-${target}.tar.gz"
    (cd "$ROOT/payload" && tar czf "$SERVE/v0.2.0/$tarball" artifax)
    (cd "$SERVE/v0.2.0" && { sha256sum "$tarball" 2>/dev/null || shasum -a 256 "$tarball"; } > "$tarball.sha256")
done
mkdir -p "$SERVE/bad"
cp "$SERVE/v0.2.0"/*.tar.gz "$SERVE/bad/"
for f in "$SERVE"/v0.2.0/*.sha256; do
    sed 's/^[0-9a-f]\{64\}/0000000000000000000000000000000000000000000000000000000000000000/' "$f" > "$SERVE/bad/$(basename "$f")"
done

PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')"
(cd "$SERVE" && exec python3 -m http.server "$PORT" --bind 127.0.0.1 >/dev/null 2>&1) &
SERVER_PID=$!
for _ in {1..50}; do
    curl -fsS "http://127.0.0.1:$PORT/" >/dev/null 2>&1 && break
    sleep 0.1
done
BASE="http://127.0.0.1:$PORT"

new_env
ARTIFAX_RELEASE_BASE_URL="$BASE" ARTIFAX_RELEASE_VERSION="v0.2.0" run
if [ "$RC" = 0 ] && [ "$OUT" = "$HOME/.local/bin/artifax" ] && [ -x "$HOME/.local/bin/artifax" ]; then
    pass "download installs to ~/.local/bin"
else fail "download installs to ~/.local/bin (rc=$RC out=$OUT err=$ERR)"; fi

new_env
fake_artifax "$FAKEBIN" "somethingelse 1.0"
ARTIFAX_RELEASE_BASE_URL="$BASE" ARTIFAX_RELEASE_VERSION="v0.2.0" run
if [ "$RC" = 0 ] && [ "$OUT" = "$HOME/.artifax/bin/artifax" ] && [ -x "$HOME/.artifax/bin/artifax" ] && [ ! -e "$HOME/.local/bin/artifax" ]; then
    pass "install dir is ~/.artifax/bin when a foreign binary shadows the name"
else fail "install dir is ~/.artifax/bin when shadowed (rc=$RC out=$OUT err=$ERR)"; fi

# Each supported Linux machine gets a static musl build.
for machine in x86_64 aarch64; do
    new_env
    printf '#!/bin/sh\ncase "$1" in -s) echo Linux ;; -m) echo %s ;; esac\n' "$machine" > "$FAKEBIN/uname"
    chmod +x "$FAKEBIN/uname"
    ARTIFAX_RELEASE_BASE_URL="$BASE" ARTIFAX_RELEASE_VERSION="v0.2.0" run
    if [ "$RC" = 0 ] && echo "$ERR" | grep -q "(${machine}-unknown-linux-musl)" && [ -x "$HOME/.local/bin/artifax" ]; then
        pass "Linux ${machine} downloads ${machine}-unknown-linux-musl"
    else fail "Linux ${machine} downloads ${machine}-unknown-linux-musl (rc=$RC err=$ERR)"; fi
done

new_env
ARTIFAX_RELEASE_BASE_URL="$BASE" ARTIFAX_RELEASE_VERSION="bad" run
if [ "$RC" != 0 ] && echo "$ERR" | grep -qi "verifying checksum" && [ ! -e "$HOME/.local/bin/artifax" ] && [ ! -e "$HOME/.artifax/bin/artifax" ]; then
    pass "bad checksum aborts and installs nothing"
else fail "bad checksum aborts and installs nothing (rc=$RC out=$OUT)"; fi

[ "$FAILED" = 0 ] && echo "all installer tests passed" || echo "installer tests FAILED"
exit "$FAILED"
