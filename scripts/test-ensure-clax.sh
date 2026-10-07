#!/usr/bin/env bash
# Hermetic tests for ensure-clax.sh: a scratch HOME, a PATH holding only the
# tools the wrapper needs, fake `clax` binaries named by CLAX_BIN or the
# `bin` setting, and fake releases served by scripts/fake-release-server.py
# on 127.0.0.1. No network, no real ~/.clax, no harness.
set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$(mktemp -d)" && pwd -P)"
SERVER_PID=""
# shellcheck disable=SC2329 # run by the EXIT trap
cleanup() {
    if [ -n "$SERVER_PID" ]; then kill "$SERVER_PID" 2>/dev/null; wait "$SERVER_PID" 2>/dev/null; fi
    rm -rf "$ROOT"
    return 0
}
trap cleanup EXIT
# with_limits SRC DEST PROBE WAIT: a copy of the wrapper SRC at DEST whose
# --version and preflight limit (PROBE_SECS, 5 s) is PROBE seconds and whose
# first-run download wait (MCP_INSTALL_WAIT_SECS, 8 s) is WAIT seconds. The
# copies under test get long limits, so no case that is not about a limit
# races one: a managed install runs a freshly extracted binary, whose first
# run macOS assesses, which can take seconds on a loaded machine. The cases
# about the limits get short ones.
with_limits() {
    mkdir -p "${2%/*}"
    sed -e "s/^PROBE_SECS=[0-9]*$/PROBE_SECS=$3/" -e "s/^MCP_INSTALL_WAIT_SECS=[0-9]*$/MCP_INSTALL_WAIT_SECS=$4/" "$1" > "$2"
    if ! grep -qx "PROBE_SECS=$3" "$2" || ! grep -qx "MCP_INSTALL_WAIT_SECS=$4" "$2"; then
        echo "FAIL: $1 sets no PROBE_SECS or MCP_INSTALL_WAIT_SECS line; update with_limits" >&2
        exit 1
    fi
}
# with_pin SRC DEST VERSION SUM...: a copy of the wrapper SRC at DEST that
# pins VERSION with the four checksums SUM (aarch64-apple-darwin,
# x86_64-apple-darwin, x86_64-unknown-linux-musl, aarch64-unknown-linux-musl),
# whatever SRC pins: "" and four "" pin nothing. Every case runs such a copy,
# so the suite is the same whether the checked-in wrapper pins a release.
with_pin() {
    mkdir -p "${2%/*}"
    sed -e "s/^PINNED_VERSION=\".*\"$/PINNED_VERSION=\"$3\"/" \
        -e "s/^SHA256_AARCH64_APPLE_DARWIN=\".*\"$/SHA256_AARCH64_APPLE_DARWIN=\"$4\"/" \
        -e "s/^SHA256_X86_64_APPLE_DARWIN=\".*\"$/SHA256_X86_64_APPLE_DARWIN=\"$5\"/" \
        -e "s/^SHA256_X86_64_UNKNOWN_LINUX_MUSL=\".*\"$/SHA256_X86_64_UNKNOWN_LINUX_MUSL=\"$6\"/" \
        -e "s/^SHA256_AARCH64_UNKNOWN_LINUX_MUSL=\".*\"$/SHA256_AARCH64_UNKNOWN_LINUX_MUSL=\"$7\"/" "$1" > "$2"
    if ! grep -qx "PINNED_VERSION=\"$3\"" "$2" || ! grep -qx "SHA256_AARCH64_APPLE_DARWIN=\"$4\"" "$2" \
        || ! grep -qx "SHA256_X86_64_APPLE_DARWIN=\"$5\"" "$2" || ! grep -qx "SHA256_X86_64_UNKNOWN_LINUX_MUSL=\"$6\"" "$2" \
        || ! grep -qx "SHA256_AARCH64_UNKNOWN_LINUX_MUSL=\"$7\"" "$2"; then
        echo "FAIL: $1 lacks a PINNED_VERSION or SHA256_* line; update with_pin" >&2
        exit 1
    fi
}
with_limits "$HERE/ensure-clax.sh" "$ROOT/wrapper/limits/ensure-clax.sh" 120 120
# The wrapper with no pin, for every case before the managed-install ones.
SCRIPT="$ROOT/wrapper/ensure-clax.sh"
with_pin "$ROOT/wrapper/limits/ensure-clax.sh" "$SCRIPT" "" "" "" "" ""
V="$(sed -n 's/^CLAX_VERSION="\(.*\)"$/\1/p' "$SCRIPT")"
# The interpreter itself, not a version-manager shim that needs the real PATH.
PY="$(python3 -c 'import sys; print(sys.executable)')"
ORIG_PATH="$PATH"
ORIG_HOME="$HOME"
FAILED=0
# shellcheck source=scripts/fake-exe.sh
. "$HERE/fake-exe.sh"
pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1"; FAILED=1; }

# The tools the wrapper may call, linked into an otherwise empty directory
# (never a clax).
TOOLS="$ROOT/tools"
mkdir -p "$TOOLS"
for t in bash sh env awk head tail grep sed tr cat mktemp mv mkdir rm chmod date wc cp sleep ls \
    curl tar gzip shasum sha256sum perl find uname sysctl; do
    if p="$(command -v "$t" 2>/dev/null)" && [ -x "$p" ]; then ln -sf "$p" "$TOOLS/$t"; fi
done
# curl runs through a guard that refuses, and records in $TOOLS/offhost.log,
# any URL not on 127.0.0.1, where the fake releases are served: no case may
# reach the network, whatever the wrapper under test pins.
mv "$TOOLS/curl" "$TOOLS/real-curl"
fake_exe "$TOOLS/curl" <<'SH'
#!/bin/sh
for a in "$@"; do
    case "$a" in
        http://127.0.0.1:*) ;;
        *://*) echo "$a" >> "${0%/*}/offhost.log"; echo "curl: $a is not on 127.0.0.1" >&2; exit 7 ;;
    esac
done
exec "${0%/*}/real-curl" "$@"
SH

# --- Fake releases ------------------------------------------------------------
# Fake releases of clax $P for every target, served by fake-release-server.py:
# /ok (the release), /none (404), /wrong (archives whose checksums differ from
# the pinned ones) and /gate (held until $REL/gate-open exists). Every case's
# CLAX_RELEASE_BASE_URL names this server; the cases that pin nothing must
# make no request to it at all.
P=9.1.0
REL="$ROOT/release"
TARGETS="aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-musl aarch64-unknown-linux-musl"
mkdir -p "$ROOT/payload" "$REL/good/v$P" "$REL/wrong/v$P" "$ROOT/wstage"
printf '#!/bin/sh\nif [ "$1" = "--version" ]; then echo "clax %s"; exit 0; fi\necho "managed: $*"\n' "$P" > "$ROOT/payload/clax"
chmod +x "$ROOT/payload/clax"
for t in $TARGETS; do
    "$HERE/package-release.sh" archive "$P" "$t" "$ROOT/payload/clax" "$REL/good/v$P" > /dev/null
    mkdir -p "$ROOT/wstage/clax-$P-$t"
    printf '#!/bin/sh\necho "clax %s"\n# tampered\n' "$P" > "$ROOT/wstage/clax-$P-$t/clax"
    chmod +x "$ROOT/wstage/clax-$P-$t/clax"
    (cd "$ROOT/wstage" && tar -czf "$REL/wrong/v$P/clax-$P-$t.tar.gz" "clax-$P-$t")
done
"$HERE/package-release.sh" sums "$REL/good/v$P" > /dev/null
REQLOG="$ROOT/requests.log"
: > "$REQLOG"
"$PY" "$HERE/fake-release-server.py" "$REL" "$REQLOG" "$ROOT/port" &
SERVER_PID=$!
i=0
while [ ! -s "$ROOT/port" ] && [ "$i" -lt 100 ]; do sleep 0.05; i=$((i + 1)); done
BASE="http://127.0.0.1:$(cat "$ROOT/port" 2>/dev/null)"

# A fake clax at $1/clax whose --version prints $2; any other run prints its
# arguments.
fake_clax() {
    printf '#!/bin/sh\nif [ "$1" = "--version" ]; then echo "%s"; exit 0; fi\necho "args: $*"\n' "$2" | fake_exe "$1/clax"
}

# Writes the `bin` setting, as `clax bin set` does, naming $1.
set_bin() {
    local home="${CLAX_HOME:-$HOME/.clax}"
    mkdir -p "$home"
    printf 'bin = "%s"\n' "$1" > "$home/config.toml"
}

new_env() {
    SANDBOX="$(mktemp -d "$ROOT/case.XXXXXX")"
    export HOME="$SANDBOX/home"
    mkdir -p "$HOME"
    FAKEBIN="$SANDBOX/fakebin"
    mkdir -p "$FAKEBIN"
    export PATH="$FAKEBIN:$TOOLS"
    unset CLAX_BIN CLAX_HOME CLAX_SOURCE_DIR CLAX_INSTALL_DIR CLAX_CONFIG_DIR CLAX_RELEASE_BASE_URL CLAX_RELEASE_VERSION
    unset GROK_SESSION_ID GROK_HOOK_EVENT GROK_PLUGIN_ROOT GROK_HOME CLAUDE_PID CLAUDE_CODE_SESSION_ID CLAUDE_PLUGIN_ROOT CLAUDE_PROJECT_DIR CLAX_SESSION_ID
    # The fake server, never GitHub; the managed-install cases name a mode.
    export CLAX_RELEASE_BASE_URL="$BASE/none"
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
set_bin "$FAKEBIN/clax"
run
if [ "$RC" = 0 ] && [ "$OUT" = "$FAKEBIN/clax" ]; then pass "the bin setting in config.toml names the clax"
else fail "the bin setting in config.toml names the clax (rc=$RC out=$OUT err=$ERR)"; fi

new_env
fake_clax "$FAKEBIN" "clax $V"
run exec status
if [ "$RC" = 1 ] && [ -z "$OUT" ] && echo "$ERR" | grep -q "pins no Clax release yet"; then pass "a clax on PATH is never run"
else fail "a clax on PATH is never run (rc=$RC out=$OUT err=$ERR)"; fi

new_env
fake_clax "$FAKEBIN" "clax $V"
mkdir -p "$HOME/.clax"
printf '[serve]\nbin = "%s"\n' "$FAKEBIN/clax" > "$HOME/.clax/config.toml"
run
if [ "$RC" = 1 ] && echo "$ERR" | grep -q "pins no Clax release yet"; then pass "a bin key inside a table is not the bin setting"
else fail "a bin key inside a table is not the bin setting (rc=$RC out=$OUT err=$ERR)"; fi

for line in "bin = '/abs/clax'" 'bin="/abs/clax"' 'bin = "relative/clax"' 'bin = "/a\\b/clax"' '"bin" = "/abs/clax"' 'bin = "/abs/clax" # note'; do
    new_env
    mkdir -p "$HOME/.clax"
    printf '# settings\n%s\n\n[serve]\nport = 7481\n' "$line" > "$HOME/.clax/config.toml"
    run exec status
    if [ "$RC" = 1 ] && echo "$ERR" | grep -qF "sets bin in a form the plugins do not read" && echo "$ERR" | grep -qF "clax bin set"; then
        pass "a bin line not in the form clax writes is an error: $line"
    else fail "a bin line not in the form clax writes is an error: $line (rc=$RC err=$ERR)"; fi
done

new_env
fake_clax "$FAKEBIN" "clax $V"
mkdir -p "$HOME/.clax"
printf 'bin = "%s"\nbin = "%s"\n' "$FAKEBIN/clax" "$FAKEBIN/clax" > "$HOME/.clax/config.toml"
run
if [ "$RC" = 1 ] && echo "$ERR" | grep -qF "(and 1 more)"; then pass "two bin lines are an error"
else fail "two bin lines are an error (rc=$RC err=$ERR)"; fi

new_env
set_bin "$SANDBOX/gone/clax"
run exec status
if [ "$RC" = 1 ] && echo "$ERR" | grep -qF "sets bin = \"$SANDBOX/gone/clax\", which is not a usable clax binary (not an executable file)"; then
    pass "a bin setting naming no usable clax is an error, not a fall-through"
else fail "a bin setting naming no usable clax is an error (rc=$RC err=$ERR)"; fi

new_env
export CLAX_HOME="$SANDBOX/ch"
fake_clax "$FAKEBIN" "clax $V"
set_bin "$FAKEBIN/clax"
run
if [ "$RC" = 0 ] && [ "$OUT" = "$FAKEBIN/clax" ]; then pass "the bin setting is read from \$CLAX_HOME/config.toml"
else fail "the bin setting is read from \$CLAX_HOME/config.toml (rc=$RC out=$OUT err=$ERR)"; fi

new_env
fake_clax "$FAKEBIN" "clax $V"
set_bin "$FAKEBIN/clax"
fake_clax "$SANDBOX/x" "clax $V"
CLAX_BIN="$SANDBOX/x/clax" run exec status
if [ "$RC" = 0 ] && [ "$OUT" = "args: status" ]; then
    CLAX_BIN="$SANDBOX/x/clax" run
    if [ "$OUT" = "$SANDBOX/x/clax" ]; then pass "CLAX_BIN wins over the bin setting"; else fail "CLAX_BIN wins over the bin setting (out=$OUT)"; fi
else fail "CLAX_BIN wins over the bin setting (rc=$RC out=$OUT err=$ERR)"; fi

new_env
fake_clax "$FAKEBIN" "clax $V"
set_bin "$FAKEBIN/clax"
CLAX_BIN="$SANDBOX/missing" run exec status
if [ "$RC" = 1 ] && [ -z "$OUT" ] && echo "$ERR" | grep -q "CLAX_BIN is set to '$SANDBOX/missing', which is not a usable clax binary"; then
    pass "an unusable CLAX_BIN fails instead of falling back to the bin setting"
else fail "an unusable CLAX_BIN fails (rc=$RC out=$OUT err=$ERR)"; fi

new_env
fake_clax "$FAKEBIN" "clax 0.0.1"
set_bin "$FAKEBIN/clax"
mcp
if [ "$OUT" = "args: mcp --agent codex" ] && echo "$ERR" | grep -q "warning: $FAKEBIN/clax is clax 0.0.1, but this plugin is clax $V" \
    && hooks_log | grep -q "launch mode=mcp agent=codex bin=\"$FAKEBIN/clax\" version=\"clax 0.0.1\" warning=\"$FAKEBIN/clax is clax 0.0.1"; then
    pass "a clax of another version runs in MCP mode with a logged warning"
else fail "a clax of another version runs with a logged warning (out=$OUT err=$ERR log=$(hooks_log))"; fi

new_env
fake_clax "$FAKEBIN" "clax $V"
set_bin "$FAKEBIN/clax"
mcp
if [ "$OUT" = "args: mcp --agent codex" ] && [ -z "$ERR" ] && hooks_log | grep -q "launch mode=mcp agent=codex bin=\"$FAKEBIN/clax\" version=\"clax $V\" warning=\"\""; then
    pass "every MCP start is logged"
else fail "every MCP start is logged (out=$OUT err=$ERR log=$(hooks_log))"; fi

new_env
mcp
if text="$(fallback_text)" && echo "$text" | grep -q "pins no Clax release yet" && echo "$text" | grep -q "just install" \
    && echo "$text" | grep -q "clax bin set" && echo "$text" | grep -q "CLAX_BIN" \
    && hooks_log | grep -q "launcher mode=mcp agent=codex exit=fallback reason=\"this plugin pins no Clax release yet.*tried=\"no CLAX_BIN; no bin setting in $HOME/.clax/config.toml; no pinned release\""; then
    pass "no pin and no binary named: the MCP client gets the reason from the fallback server"
else fail "no clax: the MCP client gets the reason (out=$OUT err=$ERR log=$(hooks_log))"; fi

new_env
CLAX_BIN="$SANDBOX/a\"b\\c" mcp
if text="$(fallback_text)" && echo "$text" | grep -qF "$SANDBOX/a\"b\\c"; then
    pass "the fallback escapes quotes and backslashes in its JSON"
else fail "the fallback escapes quotes and backslashes (out=$OUT)"; fi

new_env
OUT="$({
    printf '%s\n' "$(echo "$REQS" | head -1)"
    i=0; while ! hooks_log | grep -q "exit=fallback" && [ "$i" -lt 200 ]; do sleep 0.05; i=$((i + 1)); done
    fake_clax "$FAKEBIN" "clax $V"
    set_bin "$FAKEBIN/clax"
    printf '%s\n' '{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"status","arguments":{}}}'
} | "$TOOLS/bash" "$SCRIPT" exec mcp --agent claude 2>/dev/null)"
if echo "$OUT" | tail -1 | grep -q "clax is now available at $FAKEBIN/clax. Reconnect"; then
    pass "the fallback's status tool notices a clax installed since"
else fail "the fallback's status tool notices a clax installed since (out=$OUT)"; fi

# The MCP server reads the client's stdin and writes its stdout.
new_env
printf '#!/bin/sh\nif [ "$1" = "--version" ]; then echo "clax %s"; exit 0; fi\nwhile IFS= read -r l; do echo "got: $l"; done\n' "$V" | fake_exe "$FAKEBIN/clax"
set_bin "$FAKEBIN/clax"
OUT="$(printf 'one\ntwo\n' | "$TOOLS/bash" "$SCRIPT" exec mcp --agent claude 2>"$SANDBOX/stderr")"; RC=$?
if [ "$RC" = 0 ] && [ "$OUT" = "$(printf 'got: one\ngot: two')" ] && ! hooks_log | grep -q launcher; then
    pass "the MCP server gets the client's stdin and stdout"
else fail "the MCP server gets the client's stdin and stdout (rc=$RC out=$OUT log=$(hooks_log))"; fi

# A fake clax in $1 whose `mcp --preflight` fails with $2 on stderr while the
# file `broken` beside $1 exists, and which otherwise prints its arguments.
preflight_clax() {
    printf '#!/bin/sh\nif [ "$1" = "--version" ]; then echo "clax %s"; exit 0; fi\nfor a in "$@"; do if [ "$a" = --preflight ] && [ -e "${0%%/*}/../broken" ]; then echo "%s" >&2; exit 1; fi; done\necho "args: $*"\n' "$V" "$2" | fake_exe "$1/clax"
}

# The preflight fails: the client gets its reason from the fallback server,
# and clax mcp itself never runs.
new_env
preflight_clax "$FAKEBIN" "error: /x/config.toml: bad port"
set_bin "$FAKEBIN/clax"
: > "$SANDBOX/broken"
mcp
if text="$(fallback_text)" \
    && [ "$text" = "Clax is unavailable: clax cannot start its MCP server: /x/config.toml: bad port. Fix that, then reconnect the clax MCP server (/mcp in Claude Code) or start a new session. (Details: ~/.clax/logs/hooks.log.)" ] \
    && ! echo "$OUT" | grep -q "args:" \
    && hooks_log | grep -q "launcher mode=mcp agent=codex exit=fallback reason=\"clax cannot start its MCP server: /x/config.toml: bad port."; then
    pass "a failing preflight: the MCP client gets its reason from the fallback server"
else fail "a failing preflight (out=$OUT err=$ERR log=$(hooks_log))"; fi

# Once the cause is fixed, the status tool says so.
new_env
preflight_clax "$FAKEBIN" "error: broken"
set_bin "$FAKEBIN/clax"
: > "$SANDBOX/broken"
OUT="$({
    printf '%s\n' "$(echo "$REQS" | head -1)"
    # Fix the cause once the fallback is serving.
    i=0; while ! hooks_log | grep -q "exit=fallback" && [ "$i" -lt 200 ]; do sleep 0.05; i=$((i + 1)); done
    rm -f "$SANDBOX/broken"
    printf '%s\n' '{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"status","arguments":{}}}'
} | "$TOOLS/bash" "$SCRIPT" exec mcp --agent claude 2>/dev/null)"
if echo "$OUT" | tail -1 | grep -q "clax can start now. Reconnect"; then
    pass "the fallback's status tool notices a preflight that passes since"
else fail "the fallback's status tool notices a preflight that passes since (out=$OUT)"; fi

# A clax that predates --preflight runs as before.
new_env
printf '#!/bin/sh\nif [ "$1" = "--version" ]; then echo "clax %s"; exit 0; fi\nfor a in "$@"; do if [ "$a" = --preflight ]; then echo "error: unexpected argument '"'"'--preflight'"'"' found" >&2; exit 2; fi; done\necho "args: $*"\n' "$V" | fake_exe "$FAKEBIN/clax"
set_bin "$FAKEBIN/clax"
mcp
if [ "$OUT" = "args: mcp --agent codex" ] && ! hooks_log | grep -q launcher; then
    pass "a clax without --preflight still runs"
else fail "a clax without --preflight still runs (out=$OUT log=$(hooks_log))"; fi

# The wrapper execs clax mcp: its parent is the harness. Here the harness is
# a Python process that starts the wrapper in the plugin's directory, with
# the system directories on PATH as a harness has them (they hold no clax).
SYS_PATH="/usr/bin:/bin:/usr/sbin:/sbin"
new_env
printf '#!/bin/sh\nif [ "$1" = "--version" ]; then echo "clax %s"; exit 0; fi\ncase "$*" in *--preflight*) exit 0 ;; esac\necho "ppid=$PPID"\n' "$V" | fake_exe "$FAKEBIN/clax"
set_bin "$FAKEBIN/clax"
mkdir -p "$SANDBOX/plugin"
OUT="$(PATH="$PATH:$SYS_PATH" "$PY" - "$TOOLS/bash" "$SCRIPT" "$SANDBOX/plugin" <<'PYEOF'
import os, subprocess, sys
out = subprocess.run([sys.argv[1], sys.argv[2], "exec", "mcp", "--agent", "codex"], cwd=sys.argv[3],
                     stdin=subprocess.DEVNULL, capture_output=True, text=True).stdout.strip()
print(out == f"ppid={os.getpid()}", out, os.getpid())
PYEOF
)"
case "$OUT" in True*) pass "clax mcp's parent is the harness, not the wrapper" ;; *) fail "clax mcp's parent is the harness ($OUT)" ;; esac

# An unusable clax named by the bin setting is named in the reason.
new_env
printf '#!/bin/sh\necho "dyld: Library not loaded: libfoo.dylib" >&2\nexit 134\n' | fake_exe "$FAKEBIN/clax"
set_bin "$FAKEBIN/clax"
run exec hook --agent codex stop
if [ "$RC" = 0 ] && echo "$ERR" | grep -qF "sets bin = \"$FAKEBIN/clax\", which is not a usable clax binary (\`--version\` exited 134: dyld: Library not loaded: libfoo.dylib)"; then
    pass "an unusable clax is named with its --version failure"
else fail "an unusable clax is named (rc=$RC err=$ERR)"; fi

# A --version that hangs is cut off, here after 1 s.
new_env
printf '#!/bin/sh\nexec sleep 30\n' | fake_exe "$FAKEBIN/clax"
set_bin "$FAKEBIN/clax"
with_limits "$HERE/ensure-clax.sh" "$ROOT/wrapper/short/ensure-clax.sh" 1 1
START="$(date +%s)"
run_at "$ROOT/wrapper/short/ensure-clax.sh" exec hook --agent codex stop
ELAPSED=$(( $(date +%s) - START ))
if [ "$RC" = 0 ] && [ "$ELAPSED" -lt 20 ] && echo "$ERR" | grep -qF "\`--version\` did not finish within 1 s"; then
    pass "a --version that hangs is cut off at the limit"
else fail "a --version that hangs is cut off (rc=$RC elapsed=$ELAPSED err=$ERR)"; fi

# Nested "id" and "method" keys, as in a tool call's arguments, are not
# mistaken for the request's own, in either key order.
new_env
OUT="$(printf '%s\n' \
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"status","arguments":{"id":42}}}' \
    '{"method":"tools/call","params":{"name":"status","arguments":{"method":"GET","id":"x"}},"jsonrpc":"2.0","id":"a\"b"}' \
    '{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1,"id":7}}' \
    | "$TOOLS/bash" "$SCRIPT" exec mcp --agent codex 2>/dev/null)"
if "$PY" - "$OUT" <<'PYEOF'
import json, sys
lines = [json.loads(l) for l in sys.argv[1].splitlines()]
assert [l["id"] for l in lines] == [2, 'a"b'], lines
assert all(l["result"]["isError"] is True for l in lines), lines
PYEOF
then pass "the fallback reads only top-level id and method"
else fail "the fallback reads only top-level id and method (out=$OUT)"; fi

# The real binary ($CLAX_TEST_BIN, else built here). Cargo and rustup get
# their own homes, the real ones unless set; everything else, the build
# included, sees a scratch HOME.
REAL_BIN="${CLAX_TEST_BIN:-}"
if [ -z "$REAL_BIN" ]; then
    REPO="$(cd "$HERE/.." && pwd)"
    export CARGO_HOME="${CARGO_HOME:-$ORIG_HOME/.cargo}" RUSTUP_HOME="${RUSTUP_HOME:-$ORIG_HOME/.rustup}"
    if (cd "$REPO" && HOME="$ROOT" PATH="$ORIG_PATH" cargo build -q -p clax-cli --bin clax) >"$ROOT/build.log" 2>&1; then
        REAL_BIN="$(cd "$REPO" && HOME="$ROOT" PATH="$ORIG_PATH" cargo metadata --format-version 1 --no-deps | "$PY" -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')/debug/clax"
    fi
fi
if [ ! -x "$REAL_BIN" ] || [ "$("$REAL_BIN" --version)" != "clax $V" ]; then
    fail "no clax $V binary to test with (bin=$REAL_BIN; $(tail -5 "$ROOT/build.log" 2>/dev/null))"
    REAL_BIN=""
fi

# On a home whose config.toml does not parse, the client gets the reason,
# naming the file, and no daemon starts.
if [ -n "$REAL_BIN" ]; then
    new_env
    export CLAX_HOME="$SANDBOX/clax-home"
    mkdir -p "$CLAX_HOME"
    printf '[serve\nport = 7481\n' > "$CLAX_HOME/config.toml"
    CLAX_BIN="$REAL_BIN" mcp
    if text="$(fallback_text)" && echo "$text" | grep -qF "Clax is unavailable: clax cannot start its MCP server: $CLAX_HOME/config.toml: TOML parse error" \
        && hooks_log | grep -qF "launcher mode=mcp agent=codex exit=fallback reason=\"clax cannot start its MCP server: $CLAX_HOME/config.toml" \
        && [ ! -e "$CLAX_HOME/daemon.json" ]; then
        pass "clax mcp on a malformed config.toml: the MCP client gets the reason from the fallback server"
    else fail "clax mcp on a malformed config.toml (out=$OUT err=$ERR log=$(hooks_log))"; fi
fi

# A Codex session through the wrapper registers the harness as its parent and
# the harness's directory as its cwd, not the wrapper's (spec §11). The fake
# harness runs in a project
# directory and starts the wrapper in the plugin's, as Codex does. The daemon
# listens on a free port of the scratch home.
if [ -n "$REAL_BIN" ]; then
    new_env
    export CLAX_HOME="$SANDBOX/clax-home" CLAX_NO_OPEN=1
    mkdir -p "$CLAX_HOME" "$SANDBOX/project" "$SANDBOX/plugin"
    PORT="$("$PY" -c '
import socket
while True:
    s = socket.socket(); s.bind(("127.0.0.1", 0)); p = s.getsockname()[1]; s.close()
    if p not in (7480, 7481): print(p); break')"
    printf '[serve]\nport = %s\n' "$PORT" > "$CLAX_HOME/config.toml"
    # The system directories hold lsof, which reads the harness's cwd on macOS.
    OUT="$(cd "$SANDBOX/project" && PATH="$PATH:$SYS_PATH" CLAX_BIN="$REAL_BIN" "$PY" - "$TOOLS/bash" "$SCRIPT" "$SANDBOX/plugin" "$CLAX_HOME" <<'PYEOF' 2>"$SANDBOX/stderr"
import json, os, subprocess, sys, time, urllib.request
bash, script, plugin, home = sys.argv[1:5]
p = subprocess.Popen([bash, script, "exec", "mcp", "--agent", "codex"], cwd=plugin,
                     stdin=subprocess.PIPE, stdout=subprocess.PIPE)
p.stdin.write(b'{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}\n')
p.stdin.flush()
init = json.loads(p.stdout.readline())
sessions = []
deadline = time.time() + 30
while time.time() < deadline and not sessions:
    try:
        info = json.load(open(os.path.join(home, "daemon.json")))
        req = urllib.request.Request(f"http://127.0.0.1:{info['port']}/api/sessions",
                                     headers={"Authorization": f"Bearer {info['token']}"})
        sessions = [s for s in json.load(urllib.request.urlopen(req, timeout=2))["sessions"] if s["harness"] == "codex"]
    except Exception:
        pass
    if not sessions:
        time.sleep(0.2)
p.stdin.close()
p.wait(timeout=15)
ok = ("result" in init and len(sessions) == 1 and sessions[0]["parent_pid"] == os.getpid()
      and os.path.realpath(sessions[0]["cwd"]) == os.path.realpath(os.getcwd()))
print(ok, json.dumps(sessions), os.getpid(), os.getcwd())
PYEOF
)"
    CLAX_BIN="$REAL_BIN" "$REAL_BIN" stop > /dev/null 2>&1
    case "$OUT" in True*) pass "a Codex session through the wrapper registers the harness's PID and cwd" ;;
        *) fail "a Codex session through the wrapper registers the harness's PID and cwd ($OUT; $(tail -5 "$SANDBOX/stderr"))" ;; esac
    unset CLAX_NO_OPEN
fi

# Neither PATH, a checkout, ~/.cargo/bin, ~/.local/bin, nor a harness's
# configuration is searched.
new_env
mkdir -p "$SANDBOX/repo/plugins/clax/scripts" "$HOME/.codex"
printf '[workspace]\nmembers = [\n    "crates/clax-cli",\n]\n' > "$SANDBOX/repo/Cargo.toml"
cp "$SCRIPT" "$SANDBOX/repo/plugins/clax/scripts/ensure-clax.sh"
fake_clax "$SANDBOX/repo/target/debug" "clax $V"
fake_clax "$HOME/.cargo/bin" "clax $V"
fake_clax "$HOME/.local/bin" "clax $V"
printf '[marketplaces.clax]\nsource_type = "local"\nsource = "%s"\n' "$SANDBOX/repo" > "$HOME/.codex/config.toml"
PATH="$HOME/.cargo/bin:$HOME/.local/bin:$TOOLS" CLAX_SOURCE_DIR="$SANDBOX/repo" CLAX_INSTALL_DIR="$HOME/.local/bin" run_at "$SANDBOX/repo/plugins/clax/scripts/ensure-clax.sh"
if [ "$RC" = 1 ] && [ -z "$OUT" ]; then pass "nothing is searched: not PATH, a checkout, ~/.cargo/bin, ~/.local/bin or Codex's config"
else fail "nothing is searched (rc=$RC out=$OUT)"; fi

new_env
fake_clax "$FAKEBIN" "clax $V"
set_bin "$FAKEBIN/clax"
run exec one "two words"
if [ "$RC" = 0 ] && [ "$OUT" = "args: one two words" ]; then pass "exec passes arguments through"
else fail "exec passes arguments through (rc=$RC out=$OUT)"; fi

new_env
run bogus
if [ "$RC" = 2 ] && echo "$ERR" | grep -q usage; then pass "an unknown mode prints usage"; else fail "an unknown mode prints usage (rc=$RC)"; fi

# --- Hooks never fail --------------------------------------------------------

new_env
run exec hook --agent codex session-start
if [ "$RC" = 0 ] && [ -z "$OUT" ] && [ "$(printf '%s\n' "$ERR" | grep -c .)" = 1 ] && echo "$ERR" | grep -q "pins no Clax release yet" \
    && hooks_log | grep -q "launcher mode=hook agent=codex exit=0 reason=\"this plugin pins no Clax release yet.*argv=\"exec hook --agent codex session-start\""; then
    pass "hook mode with no clax prints one line, logs it and exits 0"
else fail "hook mode with no clax (rc=$RC out=$OUT err=$ERR log=$(hooks_log))"; fi

new_env
CLAX_BIN="$SANDBOX/missing" run exec hook --agent claude stop
if [ "$RC" = 0 ] && [ -z "$OUT" ] && [ "$(printf '%s\n' "$ERR" | grep -c .)" = 1 ] && echo "$ERR" | grep -q "CLAX_BIN is set to"; then
    pass "hook mode with an unusable CLAX_BIN prints one line and exits 0"
else fail "hook mode with an unusable CLAX_BIN (rc=$RC err=$ERR)"; fi

new_env
printf '#!/bin/sh\nif [ "$1" = "--version" ]; then echo "clax %s"; exit 0; fi\necho "partial output"\necho "boom: daemon exploded" >&2\nexit 3\n' "$V" | fake_exe "$FAKEBIN/clax"
set_bin "$FAKEBIN/clax"
run exec hook --agent claude prompt
if [ "$RC" = 0 ] && [ -z "$OUT" ] && echo "$ERR" | grep -q "boom: daemon exploded" \
    && hooks_log | grep -q "launcher mode=hook agent=claude exit=3 reason=\"clax exited 3: boom: daemon exploded\""; then
    pass "a failing hook binary exits 0, drops its stdout and is logged with its stderr"
else fail "a failing hook binary (rc=$RC out=$OUT err=$ERR log=$(hooks_log))"; fi

new_env
fake_clax "$FAKEBIN" "clax 0.0.1"
set_bin "$FAKEBIN/clax"
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

# --- Grok guard: the Claude Code copy stands down in a Grok session -----------

# A fake clax in $1 that records each run in `ran` beside $1, so a case can
# tell that none happened.
recording_clax() {
    printf '#!/bin/sh\nif [ "$1" = "--version" ]; then echo "clax %s"; exit 0; fi\necho "$*" >> "${0%%/*}/../ran"\necho "args: $*"\n' "$V" | fake_exe "$1/clax"
}
# Runs the wrapper as a child of a shell whose PID is exported as CLAUDE_PID,
# as Claude Code does for the MCP servers it starts.
run_under_claude() {
    OUT="$("$TOOLS/bash" -c 'export CLAUDE_PID=$$; "$1" "$2" "${@:3}"; exit $?' _ "$TOOLS/bash" "$SCRIPT" "$@" 2>"$SANDBOX/stderr" < /dev/null)"
    RC=$?; ERR="$(cat "$SANDBOX/stderr")"
}
standdown_text() {
    "$PY" - "$OUT" <<'PYEOF'
import json, sys
lines = [json.loads(l) for l in sys.argv[1].splitlines()]
assert [l["id"] for l in lines] == [0, 1, "call-2", 3, 4], lines
init = lines[0]["result"]
assert init["serverInfo"]["name"] == "clax" and "clax-grok" in init["instructions"], init
tools = lines[1]["result"]["tools"]
assert [t["name"] for t in tools] == ["status"], tools
call = lines[2]["result"]
assert call["isError"] is False, call
assert lines[3]["error"]["code"] == -32601 and lines[4]["result"] == {}, lines
print(call["content"][0]["text"])
PYEOF
}
mcp_as() { local agent="$1"; shift; OUT="$(printf '%s\n' "$REQS" | env "$@" "$TOOLS/bash" "$SCRIPT" exec mcp --agent "$agent" 2>"$SANDBOX/stderr")"; RC=$?; ERR="$(cat "$SANDBOX/stderr")"; }

new_env
recording_clax "$FAKEBIN"
set_bin "$FAKEBIN/clax"
mcp_as claude GROK_SESSION_ID=019a-g
if [ "$RC" = 0 ] && text="$(standdown_text)" && echo "$text" | grep -qF 'clax_grok__publish' \
    && [ ! -e "$SANDBOX/ran" ] && hooks_log | grep -q ' standdown mode=mcp agent=claude host=grok$'; then
    pass "grok guard: the Claude copy's MCP server under Grok serves status only and runs no clax"
else fail "grok guard: Claude copy MCP under Grok (rc=$RC out=$OUT err=$ERR ran=$(cat "$SANDBOX/ran" 2>/dev/null))"; fi

new_env
mcp_as claude GROK_SESSION_ID=019a-g
if [ "$RC" = 0 ] && standdown_text >/dev/null && ! hooks_log | grep -q launcher; then
    pass "grok guard: standing down needs no clax binary"
else fail "grok guard: standing down without clax (out=$OUT log=$(hooks_log))"; fi

new_env
recording_clax "$FAKEBIN"
set_bin "$FAKEBIN/clax"
mcp_as claude GROK_SESSION_ID=019a-g CLAUDE_PID=1
if standdown_text >/dev/null && [ ! -e "$SANDBOX/ran" ]; then
    pass "grok guard: Grok started from a Claude Code shell (inherited CLAUDE_PID) stands the copy down"
else fail "grok guard: inherited CLAUDE_PID (out=$OUT)"; fi

new_env
recording_clax "$FAKEBIN"
set_bin "$FAKEBIN/clax"
GROK_SESSION_ID=019a-g run_under_claude exec mcp --agent claude
if [ "$RC" = 0 ] && grep -q -- '--agent claude' "$SANDBOX/ran" 2>/dev/null; then
    pass "grok guard: Claude Code as the parent (CLAUDE_PID) runs clax even with GROK_SESSION_ID"
else fail "grok guard: CLAUDE_PID parent (rc=$RC out=$OUT err=$ERR)"; fi

for agent in grok codex; do
    new_env
    recording_clax "$FAKEBIN"
    set_bin "$FAKEBIN/clax"
    mcp_as "$agent" GROK_SESSION_ID=019a-g
    if grep -q -- "mcp --agent $agent" "$SANDBOX/ran" 2>/dev/null && ! hooks_log | grep -q standdown; then
        pass "grok guard: --agent $agent is never stood down"
    else fail "grok guard: --agent $agent (out=$OUT log=$(hooks_log))"; fi
done

new_env
recording_clax "$FAKEBIN"
set_bin "$FAKEBIN/clax"
OUT="$(printf '{"sessionId":"g","hookEventName":"Stop"}' | GROK_HOOK_EVENT=Stop "$TOOLS/bash" "$SCRIPT" exec hook --agent claude stop 2>"$SANDBOX/stderr")"; RC=$?; ERR="$(cat "$SANDBOX/stderr")"
if [ "$RC" = 0 ] && [ -z "$OUT" ] && [ -z "$ERR" ] && [ ! -e "$SANDBOX/ran" ] \
    && hooks_log | grep -q ' standdown mode=hook agent=claude host=grok$'; then
    pass "grok guard: a Claude copy hook that Grok runs reads its input, prints nothing and exits 0"
else fail "grok guard: Claude copy hook under Grok (rc=$RC out=$OUT err=$ERR)"; fi

new_env
recording_clax "$FAKEBIN"
set_bin "$FAKEBIN/clax"
OUT="$(printf '{"session_id":"s"}' | GROK_SESSION_ID=g "$TOOLS/bash" "$SCRIPT" exec hook --agent claude stop 2>"$SANDBOX/stderr")"; RC=$?
if [ "$RC" = 0 ] && grep -q -- 'hook --agent claude stop' "$SANDBOX/ran" 2>/dev/null; then
    pass "grok guard: a Claude Code hook with only GROK_SESSION_ID (Claude Code started from a Grok shell) acts"
else fail "grok guard: hook without GROK_HOOK_EVENT (rc=$RC)"; fi

new_env
recording_clax "$FAKEBIN"
set_bin "$FAKEBIN/clax"
GROK_HOOK_EVENT=Stop GROK_SESSION_ID=g run exec status
if [ "$RC" = 0 ] && [ "$OUT" = "args: status" ]; then
    pass "grok guard: CLI mode is never stood down"
else fail "grok guard: CLI mode (rc=$RC out=$OUT)"; fi

# The wrapper and the binary say the same thing.
want="$(sed -n 's/^pub const GROK_STANDDOWN: &str = "\(.*\)";$/\1/p' "$HERE/../crates/clax-mcp/src/standdown.rs")"
new_env
mcp_as claude GROK_SESSION_ID=019a-g
if [ -n "$want" ] && [ "$(standdown_text)" = "$want" ]; then pass "grok guard: the wrapper's text is the binary's"
else fail "grok guard: the stand-down text differs from crates/clax-mcp/src/standdown.rs"; fi

# --- No alias for the previous name -------------------------------------------
# Its variables, its binary on PATH and its home are all ignored and left
# untouched. The name is assembled from two halves so the name gate finds no
# literal.
OLD="arti""fax"
OLD_UPPER="ARTI""FAX"
new_env
fake_clax "$SANDBOX/elsewhere" "clax $V"
printf '#!/bin/sh\necho "%s %s"\n' "$OLD" "$V" | fake_exe "$FAKEBIN/$OLD"
mkdir -p "$HOME/.$OLD/bin"
cp "$SANDBOX/elsewhere/clax" "$HOME/.$OLD/bin/clax"
export "${OLD_UPPER}_BIN=$SANDBOX/elsewhere/clax" "${OLD_UPPER}_HOME=$HOME/.$OLD"
run exec hook --agent codex stop
unset "${OLD_UPPER}_BIN" "${OLD_UPPER}_HOME"
if [ "$RC" = 0 ] && [ -z "$OUT" ] && echo "$ERR" | grep -q "pins no Clax release yet" \
    && [ ! -e "$HOME/.$OLD/logs" ] && [ -s "$HOME/.clax/logs/hooks.log" ] \
    && [ "$(PATH="$ORIG_PATH" ls -A "$HOME/.$OLD")" = bin ]; then
    pass "the previous name's variables, binary and home are ignored and left untouched"
else fail "the previous name's variables, binary and home are ignored (rc=$RC out=$OUT err=$ERR)"; fi

if [ ! -s "$REQLOG" ]; then pass "the cases that pin nothing make no download request"
else fail "the cases that pin nothing make no download request (reqs=$(cat "$REQLOG"))"; fi

# --- The managed install of the pinned release ----------------------------------
# A copy of the wrapper pins $P with the good archives' checksums.
PINNED="$ROOT/wrapper/pinned/ensure-clax.sh"
sum_of() { awk -v f="clax-$P-$1.tar.gz" '$2 == f { print $1 }' "$REL/good/v$P/SHA256SUMS"; }
with_pin "$SCRIPT" "$PINNED" "$P" "$(sum_of aarch64-apple-darwin)" "$(sum_of x86_64-apple-darwin)" \
    "$(sum_of x86_64-unknown-linux-musl)" "$(sum_of aarch64-unknown-linux-musl)"
# The pinned wrapper with a 1 s first-run download wait, for the case about it.
PINNED_SHORT="$ROOT/wrapper/pinned-short/ensure-clax.sh"
with_limits "$PINNED" "$PINNED_SHORT" 120 1
if [ "$("$TOOLS/bash" "$PINNED" pinned-version)" = "$P" ] && [ -z "$("$TOOLS/bash" "$SCRIPT" pinned-version)" ] \
    && ! grep -q '^SHA256_[A-Z0-9_]*=""$' "$PINNED"; then
    pass "pinned-version prints the pin (empty in a wrapper that pins nothing)"
else fail "pinned-version prints the pin"; fi

# Runs the pinned wrapper against server mode $1.
prun() { local mode="$1"; shift; OUT="$(CLAX_RELEASE_BASE_URL="$BASE/$mode" "$TOOLS/bash" "$PINNED" "$@" 2>"$SANDBOX/stderr" < /dev/null)"; RC=$?; ERR="$(cat "$SANDBOX/stderr")"; }
pmcp() { local mode="$1"; shift; OUT="$(printf '%s\n' "$REQS" | CLAX_RELEASE_BASE_URL="$BASE/$mode" "$TOOLS/bash" "$PINNED" exec mcp --agent codex 2>"$SANDBOX/stderr")"; RC=$?; ERR="$(cat "$SANDBOX/stderr")"; }
MANAGED() { echo "${CLAX_HOME:-$HOME/.clax}/bin/$P"; }
requests() { grep -c . "$REQLOG" | tr -d ' '; }
listing() { PATH="$ORIG_PATH" ls -A "$1" 2>/dev/null | tr '\n' ' '; }
leftovers() { PATH="$ORIG_PATH" ls -A "$HOME/.clax/bin" 2>/dev/null | grep '^\.' || true; }
sha_of() { shasum -a 256 "$1" | awk '{ print $1 }'; }

new_env; : > "$REQLOG"
prun ok
if [ "$RC" = 0 ] && [ "$OUT" = "$(MANAGED)/clax" ] && [ -f "$(MANAGED)/clax.sha256" ] \
    && [ "$(cat "$(MANAGED)/clax.sha256")" = "$(sha_of "$ROOT/payload/clax")" ] \
    && grep -q "^/ok/v$P/clax-$P-.*\.tar\.gz$" "$REQLOG" && [ -z "$(leftovers)" ] \
    && echo "$ERR" | grep -q "installed clax $P at $(MANAGED)/clax" \
    && hooks_log | grep -q "install mode=print agent=- version=$P exit=0"; then
    pass "the pinned release is downloaded, checked and installed under \$CLAX_HOME/bin/<version>"
else fail "the pinned release is installed (rc=$RC out=$OUT err=$ERR reqs=$(cat "$REQLOG"))"; fi
: > "$REQLOG"
prun none exec status
if [ "$RC" = 0 ] && [ "$OUT" = "managed: status" ] && [ "$(requests)" = 0 ] && [ -z "$ERR" ]; then
    pass "a valid managed install runs without a download"
else fail "a valid managed install runs without a download (rc=$RC out=$OUT err=$ERR reqs=$(requests))"; fi

new_env
export CLAX_HOME="$SANDBOX/ch"
prun ok
if [ "$RC" = 0 ] && [ "$OUT" = "$CLAX_HOME/bin/$P/clax" ]; then pass "the managed install lives under \$CLAX_HOME/bin"
else fail "the managed install lives under \$CLAX_HOME/bin (rc=$RC out=$OUT err=$ERR)"; fi

new_env; : > "$REQLOG"
fake_clax "$FAKEBIN" "clax $V"
set_bin "$FAKEBIN/clax"
prun ok
if [ "$RC" = 0 ] && [ "$OUT" = "$FAKEBIN/clax" ] && [ "$(requests)" = 0 ] && [ ! -e "$HOME/.clax/bin" ]; then
    pass "the bin setting wins over the pinned release, which is not downloaded"
else fail "the bin setting wins over the pinned release (rc=$RC out=$OUT reqs=$(requests))"; fi
CLAX_BIN="$FAKEBIN/clax" prun ok
if [ "$RC" = 0 ] && [ "$OUT" = "$FAKEBIN/clax" ] && [ "$(requests)" = 0 ]; then pass "CLAX_BIN wins over the pinned release"
else fail "CLAX_BIN wins over the pinned release (rc=$RC out=$OUT)"; fi

new_env; : > "$REQLOG"
prun wrong exec status
if [ "$RC" = 1 ] && [ -z "$OUT" ] && echo "$ERR" | grep -q "does not match the checksum this plugin pins" \
    && [ ! -e "$(MANAGED)" ] && [ -z "$(leftovers)" ]; then
    pass "a download whose checksum differs from the pinned one installs nothing"
else fail "a checksum mismatch installs nothing (rc=$RC out=$OUT err=$ERR bin=$(listing "$HOME/.clax/bin"))"; fi

new_env
prun none exec status
if [ "$RC" = 1 ] && echo "$ERR" | grep -q "answered HTTP 404" && [ ! -e "$(MANAGED)" ] && [ -z "$(leftovers)" ]; then
    pass "a release that is not there installs nothing and says so"
else fail "a missing release (rc=$RC err=$ERR)"; fi

new_env
prun ok
printf 'damage\n' >> "$(MANAGED)/clax"
: > "$REQLOG"
prun ok exec status
if [ "$RC" = 0 ] && [ "$OUT" = "managed: status" ] && [ "$(requests)" = 1 ] \
    && [ "$(sha_of "$(MANAGED)/clax")" = "$(cat "$(MANAGED)/clax.sha256")" ] && [ -z "$(leftovers)" ]; then
    pass "a damaged managed binary is never run and is replaced"
else fail "a damaged managed binary is replaced (rc=$RC out=$OUT err=$ERR reqs=$(requests))"; fi
rm -f "$(MANAGED)/clax.sha256"
: > "$REQLOG"
prun ok
if [ "$RC" = 0 ] && [ "$(requests)" = 1 ] && [ -f "$(MANAGED)/clax.sha256" ]; then pass "a managed install without its sha256 record is replaced"
else fail "a managed install without its sha256 record is replaced (rc=$RC reqs=$(requests))"; fi

new_env
pids=""
for n in 1 2 3 4 5; do
    (CLAX_RELEASE_BASE_URL="$BASE/ok" "$TOOLS/bash" "$PINNED" > "$SANDBOX/out.$n" 2> "$SANDBOX/err.$n" < /dev/null; echo $? > "$SANDBOX/rc.$n") &
    pids="$pids $!"
done
for p in $pids; do wait "$p"; done
ok=1
for n in 1 2 3 4 5; do
    { [ "$(cat "$SANDBOX/rc.$n")" = 0 ] && [ "$(cat "$SANDBOX/out.$n")" = "$(MANAGED)/clax" ]; } || ok=0
done
if [ "$ok" = 1 ] && [ "$(listing "$HOME/.clax/bin")" = "$P " ] && [ "$(listing "$(MANAGED)")" = "clax clax.sha256 " ]; then
    pass "five concurrent first runs all get one valid install and leave nothing behind"
else fail "concurrent installs ($(for n in 1 2 3 4 5; do echo "[$(cat "$SANDBOX/rc.$n") $(cat "$SANDBOX/out.$n") $(cat "$SANDBOX/err.$n")]"; done); bin=$(listing "$HOME/.clax/bin"))"; fi

new_env
for d in 8.0.0 9.0.0 9.0.10 10.0.0 9.2.0 notaversion .staging.old; do mkdir -p "$HOME/.clax/bin/$d"; echo x > "$HOME/.clax/bin/$d/clax"; done
PATH="$ORIG_PATH" touch -t 202001010000 "$HOME/.clax/bin/.staging.old"
prun wrong
before="$(listing "$HOME/.clax/bin")"
prun ok
after="$(listing "$HOME/.clax/bin")"
if [ "$before" = ".staging.old 10.0.0 8.0.0 9.0.0 9.0.10 9.2.0 notaversion " ] && [ "$after" = "10.0.0 9.0.10 $P 9.2.0 notaversion " ]; then
    pass "after an install, older versions go except the newest of them; newer versions and other names stay; a failed install removes nothing"
else fail "old-version cleanup (before=$before after=$after)"; fi

new_env; : > "$REQLOG"
START="$(date +%s)"
prun ok exec hook --agent claude session-start
ELAPSED=$(( $(date +%s) - START ))
if [ "$RC" = 0 ] && [ -z "$OUT" ] && [ -z "$ERR" ] && [ "$(requests)" = 0 ] && [ ! -e "$(MANAGED)" ] && [ "$ELAPSED" -lt 3 ] \
    && hooks_log | grep -q "install-pending mode=hook agent=claude version=$P reason=\"not installed\"" && ! hooks_log | grep -q launcher; then
    pass "a hook never downloads: with the release not installed yet it exits 0 at once, silently"
else fail "a hook never downloads (rc=$RC out=$OUT err=$ERR reqs=$(requests) elapsed=$ELAPSED log=$(hooks_log))"; fi
prun ok
prun ok exec hook --agent claude stop
if [ "$RC" = 0 ] && [ "$OUT" = "managed: hook --agent claude stop" ]; then pass "once installed, hooks run the managed clax"
else fail "once installed, hooks run the managed clax (rc=$RC out=$OUT)"; fi

new_env; : > "$REQLOG"
pmcp ok
if [ "$OUT" = "managed: mcp --agent codex" ] && [ -x "$(MANAGED)/clax" ] && [ -z "$(leftovers)" ] \
    && hooks_log | grep -q "install mode=mcp agent=codex version=$P exit=0" \
    && hooks_log | grep -q "launch mode=mcp agent=codex bin=\"$(MANAGED)/clax\" version=\"clax $P\" warning=\"\""; then
    pass "a first MCP start downloads the pinned release and runs it, without a version warning"
else fail "a first MCP start downloads and runs (out=$OUT err=$ERR log=$(hooks_log))"; fi

new_env; : > "$REQLOG"
pmcp wrong
if text="$(fallback_text)" && echo "$text" | grep -q "could not install clax $P: the download of .* does not match the checksum" && [ ! -e "$(MANAGED)" ]; then
    pass "a failed first-run install: the MCP client gets the reason"
else fail "a failed first-run install (out=$OUT err=$ERR)"; fi

# A download slower than the MCP server's wait: the fallback answers, the
# download goes on in the background, and status says once it is done.
new_env
rm -f "$REL/gate-open"
OUT="$({
    printf '%s\n' "$(echo "$REQS" | head -1)"
    i=0; while ! hooks_log | grep -q "exit=fallback" && [ "$i" -lt 400 ]; do sleep 0.05; i=$((i + 1)); done
    printf '%s\n' '{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"status","arguments":{}}}'
    sleep 0.3
    : > "$REL/gate-open"
    i=0; while ! hooks_log | grep -q "install mode=mcp agent=claude version=$P exit=0" && [ "$i" -lt 400 ]; do sleep 0.05; i=$((i + 1)); done
    printf '%s\n' '{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"status","arguments":{}}}'
} | CLAX_RELEASE_BASE_URL="$BASE/gate" "$TOOLS/bash" "$PINNED_SHORT" exec mcp --agent claude 2>/dev/null)"
if echo "$OUT" | head -1 | grep -q "is still downloading" && echo "$OUT" | sed -n 2p | grep -q "is still downloading" \
    && echo "$OUT" | tail -1 | grep -q "clax is now available at $(MANAGED)/clax. Reconnect"; then
    pass "a download slower than the MCP wait: the fallback says so, and status reports it done"
else fail "a slow first-run download (out=$OUT log=$(hooks_log))"; fi
rm -f "$REL/gate-open"

new_env
fake_clax "$FAKEBIN" "clax 0.0.1"
set_bin "$FAKEBIN/clax"
prun ok exec status
if [ "$RC" = 0 ] && echo "$ERR" | grep -q "warning: $FAKEBIN/clax is clax 0.0.1, but this plugin is clax $V"; then
    pass "a bin setting of another version warns"
else fail "a bin setting of another version warns (err=$ERR)"; fi

if [ ! -e "$TOOLS/offhost.log" ]; then pass "no case asked for a URL off 127.0.0.1"
else fail "no case asked for a URL off 127.0.0.1 ($(cat "$TOOLS/offhost.log"))"; fi

[ "$FAILED" = 0 ] && echo "all wrapper tests passed" || echo "wrapper tests FAILED"
exit "$FAILED"
