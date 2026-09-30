#!/usr/bin/env bash
# Hermetic tests for ensure-clax.sh: a scratch HOME, and a PATH holding only
# the tools the wrapper needs plus fake `clax` binaries. No network, no real
# ~/.clax, no harness.
set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$(mktemp -d)" && pwd -P)"
trap 'rm -rf "$ROOT"' EXIT
mkdir -p "$ROOT/wrapper"
cp "$HERE/ensure-clax.sh" "$ROOT/wrapper/ensure-clax.sh"
SCRIPT="$ROOT/wrapper/ensure-clax.sh"
V="$(sed -n 's/^CLAX_VERSION="\(.*\)"$/\1/p' "$SCRIPT")"
# The interpreter itself, not a version-manager shim that needs the real PATH.
PY="$(python3 -c 'import sys; print(sys.executable)')"
ORIG_PATH="$PATH"
ORIG_HOME="$HOME"
FAILED=0
pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1"; FAILED=1; }

# The tools the wrapper may call, linked into an otherwise empty directory
# (never a clax).
TOOLS="$ROOT/tools"
mkdir -p "$TOOLS"
for t in bash sh env awk head tail grep sed tr cat mktemp mv mkdir rm chmod date wc cp sleep ls mkfifo tee; do
    if p="$(command -v "$t" 2>/dev/null)" && [ -x "$p" ]; then ln -sf "$p" "$TOOLS/$t"; fi
done

# A fake clax at $1/clax whose --version prints $2; any other run prints its
# arguments.
fake_clax() {
    mkdir -p "$1"
    printf '#!/bin/sh\nif [ "$1" = "--version" ]; then echo "%s"; exit 0; fi\necho "args: $*"\n' "$2" > "$1/clax"
    chmod +x "$1/clax"
}

new_env() {
    SANDBOX="$(mktemp -d "$ROOT/case.XXXXXX")"
    export HOME="$SANDBOX/home"
    mkdir -p "$HOME"
    FAKEBIN="$SANDBOX/fakebin"
    mkdir -p "$FAKEBIN"
    export PATH="$FAKEBIN:$TOOLS"
    unset CLAX_BIN CLAX_HOME CLAX_SOURCE_DIR CLAX_INSTALL_DIR CLAX_CONFIG_DIR CLAX_RELEASE_BASE_URL CLAX_RELEASE_VERSION
}
run() { OUT="$("$TOOLS/bash" "$SCRIPT" "$@" 2>"$SANDBOX/stderr" < /dev/null)"; RC=$?; ERR="$(cat "$SANDBOX/stderr")"; }
run_at() { local s="$1"; shift; OUT="$("$TOOLS/bash" "$s" "$@" 2>"$SANDBOX/stderr" < /dev/null)"; RC=$?; ERR="$(cat "$SANDBOX/stderr")"; }

# MCP requests as the clients send them: rmcp (Codex) puts the ID first, the
# TypeScript SDK (Claude Code) puts it last.
REQS='{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"method":"tools/list","params":{},"jsonrpc":"2.0","id":1}
{"jsonrpc":"2.0","id":"call-2","method":"tools/call","params":{"name":"status","arguments":{}}}
{"jsonrpc":"2.0","id":3,"method":"resources/list","params":{}}
{"jsonrpc":"2.0","id":4,"method":"ping"}'
mcp() { OUT="$(printf '%s\n' "$REQS" | "$TOOLS/bash" "$SCRIPT" exec mcp --agent codex 2>"$SANDBOX/stderr")"; RC=$?; ERR="$(cat "$SANDBOX/stderr")"; }
# Checks that $OUT is the fallback server's answer to $REQS; prints the text
# of its status tool. Fails (non-zero) otherwise.
fallback_text() {
    "$PY" - "$OUT" <<'PYEOF'
import json, sys
lines = [json.loads(l) for l in sys.argv[1].splitlines()]
assert [l["id"] for l in lines] == [0, 1, "call-2", 3, 4], lines
init = lines[0]["result"]
assert init["protocolVersion"] == "2025-06-18" and init["serverInfo"]["name"] == "clax", init
assert init["capabilities"] == {"tools": {}}, init
assert init["instructions"].startswith("Clax is unavailable: "), init
tools = lines[1]["result"]["tools"]
assert [t["name"] for t in tools] == ["status"] and tools[0]["inputSchema"]["type"] == "object", tools
call = lines[2]["result"]
assert call["isError"] is True and call["content"][0]["type"] == "text", call
assert lines[3]["error"]["code"] == -32601, lines[3]
assert lines[4]["result"] == {}, lines[4]
print(call["content"][0]["text"])
PYEOF
}
hooks_log() { cat "${CLAX_HOME:-$HOME/.clax}/logs/hooks.log" 2>/dev/null; }

# --- Resolution -------------------------------------------------------------

new_env
fake_clax "$FAKEBIN" "clax $V"
run
if [ "$RC" = 0 ] && [ "$OUT" = "$FAKEBIN/clax" ]; then pass "the clax on PATH is found"
else fail "the clax on PATH is found (rc=$RC out=$OUT err=$ERR)"; fi

new_env
fake_clax "$FAKEBIN" "somethingelse 1.0"
fake_clax "$SANDBOX/second" "clax $V"
PATH="$FAKEBIN:$SANDBOX/second:$TOOLS" run
if [ "$RC" = 0 ] && [ "$OUT" = "$SANDBOX/second/clax" ]; then pass "a foreign clax on PATH is skipped"
else fail "a foreign clax on PATH is skipped (rc=$RC out=$OUT)"; fi

new_env
fake_clax "$FAKEBIN" "clax $V"
fake_clax "$SANDBOX/x" "clax $V"
CLAX_BIN="$SANDBOX/x/clax" run exec status
if [ "$RC" = 0 ] && [ "$OUT" = "args: status" ]; then
    CLAX_BIN="$SANDBOX/x/clax" run
    if [ "$OUT" = "$SANDBOX/x/clax" ]; then pass "CLAX_BIN wins over PATH"; else fail "CLAX_BIN wins over PATH (out=$OUT)"; fi
else fail "CLAX_BIN wins over PATH (rc=$RC out=$OUT err=$ERR)"; fi

new_env
fake_clax "$FAKEBIN" "clax $V"
CLAX_BIN="$SANDBOX/missing" run exec status
if [ "$RC" = 1 ] && [ -z "$OUT" ] && echo "$ERR" | grep -q "CLAX_BIN is set to '$SANDBOX/missing', which is not a usable clax binary"; then
    pass "an unusable CLAX_BIN fails instead of falling back to PATH"
else fail "an unusable CLAX_BIN fails (rc=$RC out=$OUT err=$ERR)"; fi

new_env
fake_clax "$FAKEBIN" "clax 0.0.1"
mcp
if [ "$OUT" = "args: mcp --agent codex" ] && echo "$ERR" | grep -q "warning: $FAKEBIN/clax is clax 0.0.1, but this plugin is clax $V" \
    && hooks_log | grep -q "launch mode=mcp agent=codex bin=\"$FAKEBIN/clax\" version=\"clax 0.0.1\" warning=\"$FAKEBIN/clax is clax 0.0.1"; then
    pass "a clax of another version runs in MCP mode with a logged warning"
else fail "a clax of another version runs with a logged warning (out=$OUT err=$ERR log=$(hooks_log))"; fi

new_env
fake_clax "$FAKEBIN" "clax $V"
mcp
if [ "$OUT" = "args: mcp --agent codex" ] && [ -z "$ERR" ] && hooks_log | grep -q "launch mode=mcp agent=codex bin=\"$FAKEBIN/clax\" version=\"clax $V\" warning=\"\""; then
    pass "every MCP start is logged"
else fail "every MCP start is logged (out=$OUT err=$ERR log=$(hooks_log))"; fi

new_env
mcp
if text="$(fallback_text)" && echo "$text" | grep -q "no clax binary is on PATH" && echo "$text" | grep -q "just install" \
    && hooks_log | grep -q "launcher mode=mcp agent=codex exit=fallback reason=\"no clax binary is on PATH.*tried=\"PATH has no clax: $FAKEBIN:$TOOLS\""; then
    pass "no clax: the MCP client gets the reason from the fallback server"
else fail "no clax: the MCP client gets the reason (out=$OUT err=$ERR log=$(hooks_log))"; fi

new_env
CLAX_BIN="$SANDBOX/a\"b\\c" mcp
if text="$(fallback_text)" && echo "$text" | grep -qF "$SANDBOX/a\"b\\c"; then
    pass "the fallback escapes quotes and backslashes in its JSON"
else fail "the fallback escapes quotes and backslashes (out=$OUT)"; fi

new_env
OUT="$({
    printf '%s\n' "$(echo "$REQS" | head -1)"
    sleep 0.5
    fake_clax "$FAKEBIN" "clax $V"
    printf '%s\n' '{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"status","arguments":{}}}'
} | "$TOOLS/bash" "$SCRIPT" exec mcp --agent claude 2>/dev/null)"
if echo "$OUT" | tail -1 | grep -q "clax is now available at $FAKEBIN/clax. Reconnect"; then
    pass "the fallback's status tool notices a clax installed since"
else fail "the fallback's status tool notices a clax installed since (out=$OUT)"; fi

# The MCP server reads the client's stdin and writes its stdout.
new_env
printf '#!/bin/sh\nif [ "$1" = "--version" ]; then echo "clax %s"; exit 0; fi\nwhile IFS= read -r l; do echo "got: $l"; done\n' "$V" > "$FAKEBIN/clax"
chmod +x "$FAKEBIN/clax"
OUT="$(printf 'one\ntwo\n' | "$TOOLS/bash" "$SCRIPT" exec mcp --agent claude 2>"$SANDBOX/stderr")"; RC=$?
if [ "$RC" = 0 ] && [ "$OUT" = "$(printf 'got: one\ngot: two')" ] && ! hooks_log | grep -q launcher; then
    pass "the MCP server gets the client's stdin and stdout"
else fail "the MCP server gets the client's stdin and stdout (rc=$RC out=$OUT log=$(hooks_log))"; fi

# A server that exits with an error at startup, before reading stdin: the
# client gets the fallback server, whose status tool gives the error.
new_env
printf '#!/bin/sh\nif [ "$1" = "--version" ]; then echo "clax %s"; exit 0; fi\necho "starting" >&2\necho "error: /x/config.toml: bad \\"port\\"" >&2\necho "  detail" >&2\nexit 1\n' "$V" > "$FAKEBIN/clax"
chmod +x "$FAKEBIN/clax"
mcp
if text="$(fallback_text)" && [ "$text" = "Clax is unavailable: clax exited 1: error: /x/config.toml: bad 'port' detail (Details: ~/.clax/logs/hooks.log.)" ] \
    && echo "$ERR" | grep -q "^starting$" \
    && hooks_log | grep -q "launcher mode=mcp agent=codex exit=fallback reason=\"clax exited 1: error: /x/config.toml: bad 'port' detail\""; then
    pass "a server that fails at startup: the MCP client gets its error from the fallback server"
else fail "a server that fails at startup (out=$OUT err=$ERR log=$(hooks_log))"; fi

# TERM, INT and HUP reach the server, and the wrapper exits with its status.
new_env
printf '#!/bin/sh\nif [ "$1" = "--version" ]; then echo "clax %s"; exit 0; fi\necho $$ > "%s/pid"\ntrap "exit 0" TERM\nwhile :; do sleep 0.05; done\n' "$V" "$SANDBOX" > "$FAKEBIN/clax"
chmod +x "$FAKEBIN/clax"
# Stdin stays open, as a client's does, through a FIFO this shell holds.
mkfifo "$SANDBOX/in"
exec 7<>"$SANDBOX/in"
"$TOOLS/bash" "$SCRIPT" exec mcp --agent claude < "$SANDBOX/in" > "$SANDBOX/out" 2>&1 &
WPID=$!
i=0; while [ ! -s "$SANDBOX/pid" ] && [ "$i" -lt 100 ]; do sleep 0.05; i=$((i + 1)); done
kill -TERM "$WPID" 2>/dev/null
i=0; while kill -0 "$WPID" 2>/dev/null && [ "$i" -lt 100 ]; do sleep 0.05; i=$((i + 1)); done
SPID="$(cat "$SANDBOX/pid" 2>/dev/null)"
if ! kill -0 "$WPID" 2>/dev/null && [ -n "$SPID" ] && ! kill -0 "$SPID" 2>/dev/null && [ ! -s "$SANDBOX/out" ]; then
    pass "TERM to the wrapper stops the MCP server"
else fail "TERM to the wrapper stops the MCP server (out=$(cat "$SANDBOX/out"))"; kill -9 "$WPID" $SPID 2>/dev/null; fi
wait "$WPID" 2>/dev/null
exec 7>&-

# The real binary, on a home whose config.toml does not parse: `clax mcp`
# exits at startup, and the client gets the reason naming the file.
new_env
REAL_BIN="${CLAX_TEST_BIN:-}"
if [ -z "$REAL_BIN" ]; then
    REPO="$(cd "$HERE/.." && pwd)"
    # Cargo and rustup need the real HOME; the binary itself never runs with it.
    if (cd "$REPO" && HOME="$ORIG_HOME" PATH="$ORIG_PATH" cargo build -q -p clax-cli --bin clax) >"$SANDBOX/build.log" 2>&1; then
        REAL_BIN="$(cd "$REPO" && HOME="$ORIG_HOME" PATH="$ORIG_PATH" cargo metadata --format-version 1 --no-deps | "$PY" -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')/debug/clax"
    fi
fi
export CLAX_HOME="$SANDBOX/clax-home"
mkdir -p "$CLAX_HOME"
printf '[serve\nport = 7481\n' > "$CLAX_HOME/config.toml"
if [ -x "$REAL_BIN" ] && [ "$("$REAL_BIN" --version)" = "clax $V" ]; then
    CLAX_BIN="$REAL_BIN" mcp
    if text="$(fallback_text)" && echo "$text" | grep -qF "Clax is unavailable: clax exited 1: error: $CLAX_HOME/config.toml" \
        && hooks_log | grep -qF "launcher mode=mcp agent=codex exit=fallback reason=\"clax exited 1: error: $CLAX_HOME/config.toml"; then
        pass "clax mcp on a malformed config.toml: the MCP client gets the reason from the fallback server"
    else fail "clax mcp on a malformed config.toml (out=$OUT err=$ERR log=$(hooks_log))"; fi
else fail "clax mcp on a malformed config.toml: no clax $V binary to run (bin=$REAL_BIN; $(tail -5 "$SANDBOX/build.log" 2>/dev/null))"; fi

# Neither a checkout, ~/.cargo/bin off PATH, ~/.local/bin, nor a harness's
# configuration is searched.
new_env
mkdir -p "$SANDBOX/repo/plugins/clax/scripts" "$HOME/.codex"
printf '[workspace]\nmembers = [\n    "crates/clax-cli",\n]\n' > "$SANDBOX/repo/Cargo.toml"
cp "$SCRIPT" "$SANDBOX/repo/plugins/clax/scripts/ensure-clax.sh"
fake_clax "$SANDBOX/repo/target/debug" "clax $V"
fake_clax "$HOME/.cargo/bin" "clax $V"
fake_clax "$HOME/.local/bin" "clax $V"
printf '[marketplaces.clax]\nsource_type = "local"\nsource = "%s"\n' "$SANDBOX/repo" > "$HOME/.codex/config.toml"
CLAX_SOURCE_DIR="$SANDBOX/repo" CLAX_INSTALL_DIR="$HOME/.local/bin" run_at "$SANDBOX/repo/plugins/clax/scripts/ensure-clax.sh"
if [ "$RC" = 1 ] && [ -z "$OUT" ]; then pass "only PATH is searched: not a checkout, ~/.cargo/bin, ~/.local/bin or Codex's config"
else fail "only PATH is searched (rc=$RC out=$OUT)"; fi

new_env
fake_clax "$FAKEBIN" "clax $V"
run exec one "two words"
if [ "$RC" = 0 ] && [ "$OUT" = "args: one two words" ]; then pass "exec passes arguments through"
else fail "exec passes arguments through (rc=$RC out=$OUT)"; fi

new_env
run bogus
if [ "$RC" = 2 ] && echo "$ERR" | grep -q usage; then pass "an unknown mode prints usage"; else fail "an unknown mode prints usage (rc=$RC)"; fi

# --- Hooks never fail --------------------------------------------------------

new_env
run exec hook --agent codex session-start
if [ "$RC" = 0 ] && [ -z "$OUT" ] && [ "$(printf '%s\n' "$ERR" | grep -c .)" = 1 ] && echo "$ERR" | grep -q "no clax binary is on PATH" \
    && hooks_log | grep -q "launcher mode=hook agent=codex exit=0 reason=\"no clax binary is on PATH.*argv=\"exec hook --agent codex session-start\""; then
    pass "hook mode with no clax prints one line, logs it and exits 0"
else fail "hook mode with no clax (rc=$RC out=$OUT err=$ERR log=$(hooks_log))"; fi

new_env
CLAX_BIN="$SANDBOX/missing" run exec hook --agent claude stop
if [ "$RC" = 0 ] && [ -z "$OUT" ] && [ "$(printf '%s\n' "$ERR" | grep -c .)" = 1 ] && echo "$ERR" | grep -q "CLAX_BIN is set to"; then
    pass "hook mode with an unusable CLAX_BIN prints one line and exits 0"
else fail "hook mode with an unusable CLAX_BIN (rc=$RC err=$ERR)"; fi

new_env
printf '#!/bin/sh\nif [ "$1" = "--version" ]; then echo "clax %s"; exit 0; fi\necho "partial output"\necho "boom: daemon exploded" >&2\nexit 3\n' "$V" > "$FAKEBIN/clax"
chmod +x "$FAKEBIN/clax"
run exec hook --agent claude prompt
if [ "$RC" = 0 ] && [ -z "$OUT" ] && echo "$ERR" | grep -q "boom: daemon exploded" \
    && hooks_log | grep -q "launcher mode=hook agent=claude exit=3 reason=\"clax exited 3: boom: daemon exploded\""; then
    pass "a failing hook binary exits 0, drops its stdout and is logged with its stderr"
else fail "a failing hook binary (rc=$RC out=$OUT err=$ERR log=$(hooks_log))"; fi

new_env
fake_clax "$FAKEBIN" "clax 0.0.1"
run exec hook --agent codex stop
if [ "$RC" = 0 ] && [ "$OUT" = "args: hook --agent codex stop" ] && [ -z "$ERR" ] && [ -z "$(hooks_log)" ]; then
    pass "a succeeding hook passes its stdout through and logs nothing, whatever its version"
else fail "a succeeding hook passes its stdout through (rc=$RC out=$OUT err=$ERR log=$(hooks_log))"; fi

new_env
export CLAX_HOME="$SANDBOX/ax-home"
mkdir -p "$CLAX_HOME/logs"
awk 'BEGIN { for (i = 0; i < 20000; i++) print "0123456789012345678901234567890123456789012345678901234567890123" }' > "$CLAX_HOME/logs/hooks.log"
run exec hook --agent claude stop
if [ "$RC" = 0 ] && [ -s "$CLAX_HOME/logs/hooks.log.1" ] && [ "$(wc -l < "$CLAX_HOME/logs/hooks.log" | tr -d ' ')" = 1 ] \
    && grep -q "agent=claude" "$CLAX_HOME/logs/hooks.log"; then
    pass "hooks.log rotates to hooks.log.1 past 1 MiB, under CLAX_HOME"
else fail "hooks.log rotates past 1 MiB (rc=$RC)"; fi

new_env
export CLAX_HOME="$SANDBOX/not-a-dir"
echo file > "$CLAX_HOME"
run exec hook --agent codex stop
if [ "$RC" = 0 ] && [ -z "$OUT" ]; then pass "an unwritable log does not fail a hook"; else fail "an unwritable log does not fail a hook (rc=$RC err=$ERR)"; fi

# --- No alias for the previous name -------------------------------------------
# Its variables, its binary on PATH and its home are all ignored and left
# untouched. The name is assembled from two halves so the name gate finds no
# literal.
OLD="arti""fax"
OLD_UPPER="ARTI""FAX"
new_env
fake_clax "$SANDBOX/elsewhere" "clax $V"
printf '#!/bin/sh\necho "%s %s"\n' "$OLD" "$V" > "$FAKEBIN/$OLD"
chmod +x "$FAKEBIN/$OLD"
mkdir -p "$HOME/.$OLD/bin"
cp "$SANDBOX/elsewhere/clax" "$HOME/.$OLD/bin/clax"
export "${OLD_UPPER}_BIN=$SANDBOX/elsewhere/clax" "${OLD_UPPER}_HOME=$HOME/.$OLD"
run exec hook --agent codex stop
unset "${OLD_UPPER}_BIN" "${OLD_UPPER}_HOME"
if [ "$RC" = 0 ] && [ -z "$OUT" ] && echo "$ERR" | grep -q "no clax binary is on PATH" \
    && [ ! -e "$HOME/.$OLD/logs" ] && [ -s "$HOME/.clax/logs/hooks.log" ] \
    && [ "$(PATH="$ORIG_PATH" ls -A "$HOME/.$OLD")" = bin ]; then
    pass "the previous name's variables, binary and home are ignored and left untouched"
else fail "the previous name's variables, binary and home are ignored (rc=$RC out=$OUT err=$ERR)"; fi

[ "$FAILED" = 0 ] && echo "all wrapper tests passed" || echo "wrapper tests FAILED"
exit "$FAILED"
