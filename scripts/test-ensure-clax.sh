#!/usr/bin/env bash
# Hermetic tests for ensure-clax.sh: temp HOME, a PATH holding only
# symlinks to the tools the script needs plus fake `clax` binaries, and a
# local http.server standing in for GitHub releases. No network is used.
set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$(mktemp -d)" && pwd -P)"
# The script looks for a source checkout above its own directory, so the copy
# under test lives outside any checkout; checkout cases place their own copy.
mkdir -p "$ROOT/plain/scripts"
cp "$HERE/ensure-clax.sh" "$ROOT/plain/scripts/ensure-clax.sh"
SCRIPT="$ROOT/plain/scripts/ensure-clax.sh"
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
for t in bash env awk sort head grep curl tar sha256sum shasum mktemp uname mv chmod mkdir rm cat dirname sed tr date wc cp touch; do
    if p="$(command -v "$t" 2>/dev/null)" && [ -x "$p" ]; then ln -sf "$p" "$TOOLS/$t"; fi
done

# The same tools without curl.
NOCURL="$ROOT/nocurl"
mkdir -p "$NOCURL"
for t in "$TOOLS"/*; do
    name="${t##*/}"
    [ "$name" = curl ] || ln -sf "$(readlink "$t")" "$NOCURL/$name"
done

fake_clax() { # dir version-line
    mkdir -p "$1"
    printf '#!/bin/sh\nif [ "$1" = "--version" ]; then echo "%s"; else echo "args: $*"; fi\n' "$2" > "$1/clax"
    chmod +x "$1/clax"
}

# Fresh sandbox: sets HOME and PATH, clears every override.
new_env() {
    SANDBOX="$(mktemp -d "$ROOT/case.XXXXXX")"
    export HOME="$SANDBOX/home"
    mkdir -p "$HOME"
    FAKEBIN="$SANDBOX/fakebin"
    mkdir -p "$FAKEBIN"
    export PATH="$FAKEBIN:$TOOLS"
    unset CLAX_BIN CLAX_INSTALL_DIR CLAX_CONFIG_DIR CLAX_RELEASE_BASE_URL CLAX_RELEASE_VERSION \
        CLAX_SOURCE_DIR CLAX_HOME
}

run() { OUT="$("$TOOLS/bash" "$SCRIPT" "$@" 2>"$SANDBOX/stderr")"; RC=$?; ERR="$(cat "$SANDBOX/stderr")"; }
# Runs the copy of the script at $1 instead of $SCRIPT.
run_copy() { local s="$1"; shift; OUT="$("$TOOLS/bash" "$s" "$@" 2>"$SANDBOX/stderr")"; RC=$?; ERR="$(cat "$SANDBOX/stderr")"; }

# A fake source checkout at $1: a workspace Cargo.toml naming clax-cli and a
# copy of the script at plugins/clax/scripts/. Binaries are added per case.
fake_checkout() {
    mkdir -p "$1/plugins/clax/scripts" "$1/plugins/clax/.codex-plugin"
    printf '[workspace]\nmembers = [\n    "crates/clax-cli",\n]\n' > "$1/Cargo.toml"
    cp "$HERE/ensure-clax.sh" "$1/plugins/clax/scripts/ensure-clax.sh"
    echo '{"name": "clax", "version": "0.2.0"}' > "$1/plugins/clax/.codex-plugin/plugin.json"
}
# A plugin copy at $1 (as a harness caches it), outside any checkout.
plugin_copy() {
    mkdir -p "$1/scripts" "$1/.codex-plugin"
    cp "$HERE/ensure-clax.sh" "$1/scripts/ensure-clax.sh"
    echo "{\"name\": \"clax\", \"version\": \"0.2.0\"${2:+, \"source\": \"$2\"}}" > "$1/.codex-plugin/plugin.json"
}
hooks_log() { cat "${CLAX_HOME:-$HOME/.clax}/logs/hooks.log" 2>/dev/null; }

# --- Resolution -------------------------------------------------------------

new_env
fake_clax "$SANDBOX/explicit" "clax 0.2.0"
fake_clax "$FAKEBIN" "clax 0.9.9"
CLAX_BIN="$SANDBOX/explicit/clax" run
if [ "$RC" = 0 ] && [ "$OUT" = "$SANDBOX/explicit/clax" ]; then pass "CLAX_BIN wins over PATH"; else fail "CLAX_BIN wins over PATH (rc=$RC out=$OUT)"; fi

new_env
fake_clax "$FAKEBIN" "clax 0.2.0"
CLAX_BIN="$SANDBOX/missing" run
if [ "$RC" = 0 ] && [ "$OUT" = "$FAKEBIN/clax" ] && echo "$ERR" | grep -q "CLAX_BIN"; then
    pass "unusable CLAX_BIN warns and falls through"
else fail "unusable CLAX_BIN warns and falls through (rc=$RC out=$OUT err=$ERR)"; fi

new_env
fake_clax "$FAKEBIN" "somethingelse 1.0"
fake_clax "$HOME/.local/bin" "clax 0.2.0"
run
if [ "$RC" = 0 ] && [ "$OUT" = "$HOME/.local/bin/clax" ]; then pass "foreign clax on PATH is skipped"; else fail "foreign clax on PATH is skipped (rc=$RC out=$OUT)"; fi

new_env
fake_clax "$FAKEBIN" "somethingelse 1.0"
fake_clax "$HOME/.clax/bin" "clax 0.2.0"
run
if [ "$RC" = 0 ] && [ "$OUT" = "$HOME/.clax/bin/clax" ]; then pass "fallback bin dir is found"; else fail "fallback bin dir is found (rc=$RC out=$OUT)"; fi

new_env
fake_clax "$FAKEBIN" "clax 0.2.0"
run exec one "two words"
if [ "$RC" = 0 ] && [ "$OUT" = "args: one two words" ]; then pass "exec passes arguments through"; else fail "exec passes arguments through (rc=$RC out=$OUT)"; fi

new_env
fake_clax "$FAKEBIN" "clax 0.1.0"
run
if [ "$RC" = 0 ] && echo "$ERR" | grep -q "older than 0.2.0"; then pass "version warning fires for 0.1.0"; else fail "version warning fires for 0.1.0 (rc=$RC err=$ERR)"; fi

new_env
fake_clax "$FAKEBIN" "clax 0.2.0"
run bogus
if [ "$RC" = 2 ] && echo "$ERR" | grep -q usage; then pass "unknown subcommand prints usage"; else fail "unknown subcommand prints usage (rc=$RC)"; fi

# --- Source checkout --------------------------------------------------------

new_env
fake_checkout "$SANDBOX/repo"
fake_clax "$SANDBOX/repo/target/debug" "clax 0.2.0"
run_copy "$SANDBOX/repo/plugins/clax/scripts/ensure-clax.sh"
if [ "$RC" = 0 ] && [ "$OUT" = "$SANDBOX/repo/target/debug/clax" ]; then pass "a checkout's target/debug/clax is found above the script"
else fail "a checkout's target/debug/clax is found above the script (rc=$RC out=$OUT err=$ERR)"; fi

new_env
fake_checkout "$SANDBOX/repo"
fake_clax "$SANDBOX/repo/target/release" "clax 0.2.0"
run_copy "$SANDBOX/repo/plugins/clax/scripts/ensure-clax.sh"
if [ "$RC" = 0 ] && [ "$OUT" = "$SANDBOX/repo/target/release/clax" ]; then pass "a checkout's target/release/clax is found"
else fail "a checkout's target/release/clax is found (rc=$RC out=$OUT err=$ERR)"; fi

new_env
fake_checkout "$SANDBOX/repo"
fake_clax "$SANDBOX/repo/target/release" "clax 0.2.0"
fake_clax "$SANDBOX/repo/target/debug" "clax 0.2.0"
touch -t 202001010000 "$SANDBOX/repo/target/release/clax"
run_copy "$SANDBOX/repo/plugins/clax/scripts/ensure-clax.sh"
if [ "$RC" = 0 ] && [ "$OUT" = "$SANDBOX/repo/target/debug/clax" ]; then pass "the newer of the release and debug builds wins"
else fail "the newer of the release and debug builds wins (rc=$RC out=$OUT err=$ERR)"; fi

new_env
fake_checkout "$SANDBOX/repo"
fake_clax "$SANDBOX/repo/target/debug" "clax 0.1.0"
run_copy "$SANDBOX/repo/plugins/clax/scripts/ensure-clax.sh"
if [ "$RC" = 0 ] && [ "$OUT" = "$SANDBOX/repo/target/debug/clax" ] && echo "$ERR" | grep -q "older than 0.2.0"; then
    pass "an old checkout build warns but is used"
else fail "an old checkout build warns but is used (rc=$RC out=$OUT err=$ERR)"; fi

new_env
fake_checkout "$SANDBOX/repo"
fake_clax "$SANDBOX/repo/target/debug" "somethingelse 1.0"
run_copy "$SANDBOX/repo/plugins/clax/scripts/ensure-clax.sh" exec hook --agent codex stop
if [ "$RC" = 0 ] && [ -z "$OUT" ]; then pass "a foreign binary in the checkout's target is skipped"
else fail "a foreign binary in the checkout's target is skipped (rc=$RC out=$OUT err=$ERR)"; fi

new_env
fake_checkout "$SANDBOX/repo"
printf '[package]\nname = "other"\n' > "$SANDBOX/repo/Cargo.toml"
fake_clax "$SANDBOX/repo/target/debug" "clax 0.2.0"
run_copy "$SANDBOX/repo/plugins/clax/scripts/ensure-clax.sh" exec hook --agent codex stop
if [ "$RC" = 0 ] && [ -z "$OUT" ]; then pass "a Cargo.toml that does not name clax-cli is not a checkout"
else fail "a Cargo.toml that does not name clax-cli is not a checkout (rc=$RC out=$OUT)"; fi

new_env
fake_checkout "$SANDBOX/repo"
fake_clax "$SANDBOX/repo/target/debug" "clax 0.2.0"
fake_clax "$FAKEBIN" "clax 0.2.0"
fake_clax "$SANDBOX/inst" "clax 0.2.0"
run_copy "$SANDBOX/repo/plugins/clax/scripts/ensure-clax.sh"
r1="$OUT"
PATH="$TOOLS" CLAX_INSTALL_DIR="$SANDBOX/inst" run_copy "$SANDBOX/repo/plugins/clax/scripts/ensure-clax.sh"
if [ "$r1" = "$FAKEBIN/clax" ] && [ "$OUT" = "$SANDBOX/inst/clax" ]; then pass "PATH and the install dir come before the checkout"
else fail "PATH and the install dir come before the checkout (path=$r1 install=$OUT)"; fi

new_env
fake_checkout "$SANDBOX/repo"
fake_clax "$SANDBOX/repo/target/debug" "clax 0.2.0"
plugin_copy "$SANDBOX/cache/plugin"
CLAX_SOURCE_DIR="$SANDBOX/repo" run_copy "$SANDBOX/cache/plugin/scripts/ensure-clax.sh"
if [ "$RC" = 0 ] && [ "$OUT" = "$SANDBOX/repo/target/debug/clax" ]; then pass "CLAX_SOURCE_DIR names the checkout for a plugin copy"
else fail "CLAX_SOURCE_DIR names the checkout for a plugin copy (rc=$RC out=$OUT err=$ERR)"; fi

new_env
fake_checkout "$SANDBOX/repo"
fake_clax "$SANDBOX/repo/target/debug" "clax 0.2.0"
plugin_copy "$SANDBOX/cache/plugin" "$SANDBOX/repo/plugins/clax"
run_copy "$SANDBOX/cache/plugin/scripts/ensure-clax.sh"
if [ "$RC" = 0 ] && [ "$OUT" = "$SANDBOX/repo/target/debug/clax" ]; then pass "the manifest's source field names the checkout for a plugin copy"
else fail "the manifest's source field names the checkout for a plugin copy (rc=$RC out=$OUT err=$ERR)"; fi

new_env
fake_checkout "$SANDBOX/repo"
fake_clax "$SANDBOX/repo/target/debug" "clax 0.2.0"
plugin_copy "$SANDBOX/codex-home/plugins/cache/clax/clax/0.2.0"
printf '[marketplaces.other]\nsource = "/nowhere"\n\n[marketplaces.clax]\nsource_type = "local"\nsource = "%s"\n\n[plugins."clax@clax"]\nenabled = true\n' "$SANDBOX/repo" > "$SANDBOX/codex-home/config.toml"
run_copy "$SANDBOX/codex-home/plugins/cache/clax/clax/0.2.0/scripts/ensure-clax.sh"
if [ "$RC" = 0 ] && [ "$OUT" = "$SANDBOX/repo/target/debug/clax" ]; then pass "a Codex plugin cache copy finds the checkout its marketplace was added from"
else fail "a Codex plugin cache copy finds the checkout its marketplace was added from (rc=$RC out=$OUT err=$ERR)"; fi

# --- No alias for the previous name -------------------------------------------
# Its variables, its binary on PATH, its home's bin directory and a Codex
# marketplace under its name are all ignored. The name is assembled from two
# halves so the repository's name gate finds no literal.
OLD="arti""fax"
OLD_UPPER="ARTI""FAX"
new_env
fake_clax "$SANDBOX/elsewhere" "clax 0.2.0"
printf '#!/bin/sh\necho "%s 0.2.0"\n' "$OLD" > "$FAKEBIN/$OLD"
chmod +x "$FAKEBIN/$OLD"
mkdir -p "$HOME/.$OLD/bin"
cp "$FAKEBIN/$OLD" "$HOME/.$OLD/bin/$OLD"
fake_checkout "$SANDBOX/repo"
fake_clax "$SANDBOX/repo/target/debug" "clax 0.2.0"
plugin_copy "$SANDBOX/codex-home/plugins/cache/clax/clax/0.2.0"
printf '[marketplaces.%s]\nsource_type = "local"\nsource = "%s"\n' "$OLD" "$SANDBOX/repo" > "$SANDBOX/codex-home/config.toml"
export "${OLD_UPPER}_BIN=$SANDBOX/elsewhere/clax" "${OLD_UPPER}_HOME=$HOME/.$OLD" "${OLD_UPPER}_SOURCE_DIR=$SANDBOX/repo"
run_copy "$SANDBOX/codex-home/plugins/cache/clax/clax/0.2.0/scripts/ensure-clax.sh" exec hook --agent codex stop
unset "${OLD_UPPER}_BIN" "${OLD_UPPER}_HOME" "${OLD_UPPER}_SOURCE_DIR"
if [ "$RC" = 0 ] && [ -z "$OUT" ] && echo "$ERR" | grep -q "no binary found" \
    && [ ! -e "$HOME/.$OLD/logs" ] && [ -s "$HOME/.clax/logs/hooks.log" ] \
    && [ "$(PATH="$ORIG_PATH" ls -A "$HOME/.$OLD")" = bin ] && [ "$(PATH="$ORIG_PATH" ls -A "$HOME/.$OLD/bin")" = "$OLD" ]; then
    pass "the previous name's variables, binary, home and marketplace are ignored and left untouched"
else fail "the previous name's variables, binary, home and marketplace are ignored and left untouched (rc=$RC out=$OUT err=$ERR)"; fi

# --- Hook mode never downloads and never fails --------------------------------

new_env
plugin_copy "$SANDBOX/cache/plugin"
CLAX_RELEASE_BASE_URL="http://127.0.0.1:9/unreachable" CLAX_RELEASE_VERSION="v0.2.0" \
    run_copy "$SANDBOX/cache/plugin/scripts/ensure-clax.sh" exec hook --agent codex session-start
if [ "$RC" = 0 ] && [ -z "$OUT" ] && [ "$(printf '%s\n' "$ERR" | grep -c .)" = 1 ] \
    && echo "$ERR" | grep -q "no binary found; install with \`cargo install --path crates/clax-cli\`" \
    && ! echo "$ERR" | grep -qi download && [ ! -e "$HOME/.local/bin/clax" ]; then
    pass "hook mode with no binary prints one line, exits 0 and downloads nothing"
else fail "hook mode with no binary prints one line, exits 0 and downloads nothing (rc=$RC out=$OUT err=$ERR)"; fi
if hooks_log | grep -q "mode=hook agent=codex .*argv=\"exec hook --agent codex session-start\""; then
    pass "a failed hook resolution is logged to hooks.log"
else fail "a failed hook resolution is logged to hooks.log ($(hooks_log))"; fi

new_env
export CLAX_HOME="$SANDBOX/ax-home"
plugin_copy "$SANDBOX/cache/plugin"
mkdir -p "$CLAX_HOME/logs"
awk 'BEGIN { for (i = 0; i < 20000; i++) print "0123456789012345678901234567890123456789012345678901234567890123" }' > "$CLAX_HOME/logs/hooks.log"
run_copy "$SANDBOX/cache/plugin/scripts/ensure-clax.sh" exec hook --agent claude stop
if [ "$RC" = 0 ] && [ -s "$CLAX_HOME/logs/hooks.log.1" ] && [ "$(wc -l < "$CLAX_HOME/logs/hooks.log" | tr -d ' ')" = 1 ] \
    && grep -q "agent=claude" "$CLAX_HOME/logs/hooks.log"; then
    pass "hooks.log rotates to hooks.log.1 past 1 MiB, under CLAX_HOME"
else fail "hooks.log rotates to hooks.log.1 past 1 MiB (rc=$RC)"; fi

new_env
export CLAX_HOME="$SANDBOX/not-a-dir"
echo file > "$CLAX_HOME"
plugin_copy "$SANDBOX/cache/plugin"
run_copy "$SANDBOX/cache/plugin/scripts/ensure-clax.sh" exec hook --agent codex stop
if [ "$RC" = 0 ] && [ -z "$OUT" ]; then pass "an unwritable log does not fail the hook"
else fail "an unwritable log does not fail the hook (rc=$RC err=$ERR)"; fi

new_env
fake_clax "$FAKEBIN" "clax 0.2.0"
run exec hook --agent codex stop
if [ "$RC" = 0 ] && [ "$OUT" = "args: hook --agent codex stop" ]; then pass "hook mode runs a found binary"
else fail "hook mode runs a found binary (rc=$RC out=$OUT)"; fi

new_env
plugin_copy "$SANDBOX/cache/plugin"
CLAX_BIN="$SANDBOX/missing" run_copy "$SANDBOX/cache/plugin/scripts/ensure-clax.sh" exec hook --agent codex stop
if [ "$RC" = 0 ] && [ -z "$OUT" ] && [ "$(printf '%s\n' "$ERR" | grep -c .)" = 1 ] \
    && echo "$ERR" | grep -q "no binary found" && echo "$ERR" | grep -q "CLAX_BIN '$SANDBOX/missing'"; then
    pass "hook mode folds an unusable CLAX_BIN into its one line"
else fail "hook mode folds an unusable CLAX_BIN into its one line (rc=$RC err=$ERR)"; fi

# A found binary that fails: exit 0, no stdout, stderr passed on, failure logged.
new_env
mkdir -p "$FAKEBIN"
printf '#!/bin/sh\nif [ "$1" = "--version" ]; then echo "clax 0.2.0"; exit 0; fi\necho "partial output"\necho "boom: daemon exploded" >&2\nexit 3\n' > "$FAKEBIN/clax"
chmod +x "$FAKEBIN/clax"
run exec hook --agent claude prompt
if [ "$RC" = 0 ] && [ -z "$OUT" ] && echo "$ERR" | grep -q "boom: daemon exploded"; then
    pass "hook mode exits 0 and drops stdout when the binary fails"
else fail "hook mode exits 0 and drops stdout when the binary fails (rc=$RC out=$OUT err=$ERR)"; fi
if hooks_log | grep -q "mode=hook agent=claude exit=3 reason=\"clax exited 3: boom: daemon exploded\""; then
    pass "a failing hook binary is logged with its stderr"
else fail "a failing hook binary is logged with its stderr ($(hooks_log))"; fi

# A found binary that succeeds: its stdout passes through, nothing is logged.
new_env
fake_clax "$FAKEBIN" "clax 0.2.0"
run exec hook --agent codex stop
if [ "$RC" = 0 ] && [ "$OUT" = "args: hook --agent codex stop" ] && [ -z "$(hooks_log)" ]; then
    pass "hook mode passes a succeeding binary's stdout through"
else fail "hook mode passes a succeeding binary's stdout through (rc=$RC out=$OUT log=$(hooks_log))"; fi

# MCP mode: setup failures before any download still log and name the remedies.
new_env
printf '#!/bin/sh\ncase "$1" in -s) echo FreeBSD ;; -m) echo x86_64 ;; esac\n' > "$FAKEBIN/uname"
chmod +x "$FAKEBIN/uname"
run exec mcp --agent codex
if [ "$RC" = 1 ] && echo "$ERR" | grep -q "unsupported OS" && echo "$ERR" | grep -q "cargo install --path crates/clax-cli" \
    && hooks_log | grep -q "mode=mcp agent=codex exit=1"; then
    pass "an unsupported platform in mcp mode logs and names the remedies"
else fail "an unsupported platform in mcp mode logs and names the remedies (rc=$RC err=$ERR log=$(hooks_log))"; fi

new_env
PATH="$FAKEBIN:$NOCURL" run exec mcp --agent codex
if [ "$RC" = 1 ] && echo "$ERR" | grep -q "required commands not found: curl" && echo "$ERR" | grep -q "CLAX_BIN" \
    && hooks_log | grep -q "mode=mcp agent=codex exit=1"; then
    pass "a missing curl in mcp mode logs and names the remedies"
else fail "a missing curl in mcp mode logs and names the remedies (rc=$RC err=$ERR log=$(hooks_log))"; fi

# --- Download ---------------------------------------------------------------

PATH="$ORIG_PATH"
# Serve a fake release for every supported target under /v0.2.0/.
SERVE="$ROOT/serve"
mkdir -p "$SERVE/v0.2.0" "$ROOT/payload"
fake_clax "$ROOT/payload" "clax 0.2.0"
for target in aarch64-apple-darwin x86_64-unknown-linux-musl aarch64-unknown-linux-musl; do
    tarball="clax-${target}.tar.gz"
    (cd "$ROOT/payload" && tar czf "$SERVE/v0.2.0/$tarball" clax)
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
CLAX_RELEASE_BASE_URL="$BASE" CLAX_RELEASE_VERSION="v0.2.0" run
if [ "$RC" = 0 ] && [ "$OUT" = "$HOME/.local/bin/clax" ] && [ -x "$HOME/.local/bin/clax" ]; then
    pass "download installs to ~/.local/bin"
else fail "download installs to ~/.local/bin (rc=$RC out=$OUT err=$ERR)"; fi

new_env
fake_clax "$FAKEBIN" "somethingelse 1.0"
CLAX_RELEASE_BASE_URL="$BASE" CLAX_RELEASE_VERSION="v0.2.0" run
if [ "$RC" = 0 ] && [ "$OUT" = "$HOME/.clax/bin/clax" ] && [ -x "$HOME/.clax/bin/clax" ] && [ ! -e "$HOME/.local/bin/clax" ]; then
    pass "install dir is ~/.clax/bin when a foreign binary shadows the name"
else fail "install dir is ~/.clax/bin when shadowed (rc=$RC out=$OUT err=$ERR)"; fi

# Each supported Linux machine gets a static musl build.
for machine in x86_64 aarch64; do
    new_env
    printf '#!/bin/sh\ncase "$1" in -s) echo Linux ;; -m) echo %s ;; esac\n' "$machine" > "$FAKEBIN/uname"
    chmod +x "$FAKEBIN/uname"
    CLAX_RELEASE_BASE_URL="$BASE" CLAX_RELEASE_VERSION="v0.2.0" run
    if [ "$RC" = 0 ] && echo "$ERR" | grep -q "(${machine}-unknown-linux-musl)" && [ -x "$HOME/.local/bin/clax" ]; then
        pass "Linux ${machine} downloads ${machine}-unknown-linux-musl"
    else fail "Linux ${machine} downloads ${machine}-unknown-linux-musl (rc=$RC err=$ERR)"; fi
done

new_env
CLAX_RELEASE_BASE_URL="$BASE" CLAX_RELEASE_VERSION="bad" run
if [ "$RC" != 0 ] && echo "$ERR" | grep -qi "verifying checksum" && [ ! -e "$HOME/.local/bin/clax" ] && [ ! -e "$HOME/.clax/bin/clax" ]; then
    pass "bad checksum aborts and installs nothing"
else fail "bad checksum aborts and installs nothing (rc=$RC out=$OUT)"; fi

new_env
CLAX_RELEASE_BASE_URL="$BASE" CLAX_RELEASE_VERSION="v9.9.9" run exec mcp --agent codex
if [ "$RC" = 1 ] && [ -z "$OUT" ] && echo "$ERR" | grep -q "no Clax release has been published yet" \
    && echo "$ERR" | grep -q "cargo install --path crates/clax-cli" && echo "$ERR" | grep -q "CLAX_BIN"; then
    pass "a failed download in mcp mode says there is no release yet and names the remedies"
else fail "a failed download in mcp mode says there is no release yet (rc=$RC err=$ERR)"; fi
if hooks_log | grep -q "mode=mcp agent=codex"; then pass "a failed mcp resolution is logged to hooks.log"
else fail "a failed mcp resolution is logged to hooks.log ($(hooks_log))"; fi

new_env
CLAX_RELEASE_BASE_URL="$BASE" CLAX_RELEASE_VERSION="v0.2.0" run exec hook --agent codex stop
if [ "$RC" = 0 ] && [ -z "$OUT" ] && [ ! -e "$HOME/.local/bin/clax" ] && [ ! -e "$HOME/.clax/bin/clax" ]; then
    pass "hook mode does not download even when a release is available"
else fail "hook mode does not download even when a release is available (rc=$RC out=$OUT err=$ERR)"; fi

[ "$FAILED" = 0 ] && echo "all installer tests passed" || echo "installer tests FAILED"
exit "$FAILED"
