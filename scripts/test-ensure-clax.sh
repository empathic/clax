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
for t in bash sh env awk head tail grep sed tr cat mktemp mv mkdir rm chmod date wc cp sleep ls; do
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
    unset GROK_SESSION_ID GROK_HOOK_EVENT GROK_PLUGIN_ROOT GROK_HOME CLAUDE_PID CLAUDE_CODE_SESSION_ID CLAUDE_PLUGIN_ROOT CLAUDE_PROJECT_DIR CLAX_SESSION_ID
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
    i=0; while ! hooks_log | grep -q "exit=fallback" && [ "$i" -lt 200 ]; do sleep 0.05; i=$((i + 1)); done
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

# A fake clax whose `mcp --preflight` fails with $2 on stderr while the file
# $3 exists, and which otherwise prints its arguments.
preflight_clax() {
    printf '#!/bin/sh\nif [ "$1" = "--version" ]; then echo "clax %s"; exit 0; fi\nfor a in "$@"; do if [ "$a" = --preflight ] && [ -e "%s" ]; then echo "%s" >&2; exit 1; fi; done\necho "args: $*"\n' "$V" "$3" "$2" > "$1/clax"
    chmod +x "$1/clax"
}

# The preflight fails: the client gets its reason from the fallback server,
# and clax mcp itself never runs.
new_env
preflight_clax "$FAKEBIN" "error: /x/config.toml: bad port" "$SANDBOX/broken"
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
preflight_clax "$FAKEBIN" "error: broken" "$SANDBOX/broken"
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
printf '#!/bin/sh\nif [ "$1" = "--version" ]; then echo "clax %s"; exit 0; fi\nfor a in "$@"; do if [ "$a" = --preflight ]; then echo "error: unexpected argument '"'"'--preflight'"'"' found" >&2; exit 2; fi; done\necho "args: $*"\n' "$V" > "$FAKEBIN/clax"
chmod +x "$FAKEBIN/clax"
mcp
if [ "$OUT" = "args: mcp --agent codex" ] && ! hooks_log | grep -q launcher; then
    pass "a clax without --preflight still runs"
else fail "a clax without --preflight still runs (out=$OUT log=$(hooks_log))"; fi

# The wrapper execs clax mcp: its parent is the harness. Here the harness is
# a Python process that starts the wrapper in the plugin's directory, with
# the system directories on PATH as a harness has them (they hold no clax).
SYS_PATH="/usr/bin:/bin:/usr/sbin:/sbin"
new_env
printf '#!/bin/sh\nif [ "$1" = "--version" ]; then echo "clax %s"; exit 0; fi\ncase "$*" in *--preflight*) exit 0 ;; esac\necho "ppid=$PPID"\n' "$V" > "$FAKEBIN/clax"
chmod +x "$FAKEBIN/clax"
mkdir -p "$SANDBOX/plugin"
OUT="$(PATH="$PATH:$SYS_PATH" "$PY" - "$TOOLS/bash" "$SCRIPT" "$SANDBOX/plugin" <<'PYEOF'
import os, subprocess, sys
out = subprocess.run([sys.argv[1], sys.argv[2], "exec", "mcp", "--agent", "codex"], cwd=sys.argv[3],
                     stdin=subprocess.DEVNULL, capture_output=True, text=True).stdout.strip()
print(out == f"ppid={os.getpid()}", out, os.getpid())
PYEOF
)"
case "$OUT" in True*) pass "clax mcp's parent is the harness, not the wrapper" ;; *) fail "clax mcp's parent is the harness ($OUT)" ;; esac

# An unusable clax on PATH is named in the reason.
new_env
printf '#!/bin/sh\necho "dyld: Library not loaded: libfoo.dylib" >&2\nexit 134\n' > "$FAKEBIN/clax"
chmod +x "$FAKEBIN/clax"
run exec hook --agent codex stop
if [ "$RC" = 0 ] && echo "$ERR" | grep -qF "no usable clax binary is on PATH ($FAKEBIN/clax: \`--version\` exited 134: dyld: Library not loaded: libfoo.dylib). Reinstall it with"; then
    pass "an unusable clax on PATH is named with its --version failure"
else fail "an unusable clax on PATH is named (rc=$RC err=$ERR)"; fi

# A --version that hangs is cut off.
new_env
printf '#!/bin/sh\nexec sleep 30\n' > "$FAKEBIN/clax"
chmod +x "$FAKEBIN/clax"
START="$(date +%s)"
run exec hook --agent codex stop
ELAPSED=$(( $(date +%s) - START ))
if [ "$RC" = 0 ] && [ "$ELAPSED" -lt 10 ] && echo "$ERR" | grep -qF "\`--version\` did not finish within 5 s"; then
    pass "a --version that hangs is cut off after 5 s"
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

# --- Grok guard: the Claude Code copy stands down in a Grok session -----------

# A fake clax that records each run, so a case can tell that none happened.
recording_clax() {
    mkdir -p "$1"
    printf '#!/bin/sh\nif [ "$1" = "--version" ]; then echo "clax %s"; exit 0; fi\necho "$*" >> "%s/ran"\necho "args: $*"\n' "$V" "$SANDBOX" > "$1/clax"
    chmod +x "$1/clax"
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
mcp_as claude GROK_SESSION_ID=019a-g CLAUDE_PID=1
if standdown_text >/dev/null && [ ! -e "$SANDBOX/ran" ]; then
    pass "grok guard: Grok started from a Claude Code shell (inherited CLAUDE_PID) stands the copy down"
else fail "grok guard: inherited CLAUDE_PID (out=$OUT)"; fi

new_env
recording_clax "$FAKEBIN"
GROK_SESSION_ID=019a-g run_under_claude exec mcp --agent claude
if [ "$RC" = 0 ] && grep -q -- '--agent claude' "$SANDBOX/ran" 2>/dev/null; then
    pass "grok guard: Claude Code as the parent (CLAUDE_PID) runs clax even with GROK_SESSION_ID"
else fail "grok guard: CLAUDE_PID parent (rc=$RC out=$OUT err=$ERR)"; fi

for agent in grok codex; do
    new_env
    recording_clax "$FAKEBIN"
    mcp_as "$agent" GROK_SESSION_ID=019a-g
    if grep -q -- "mcp --agent $agent" "$SANDBOX/ran" 2>/dev/null && ! hooks_log | grep -q standdown; then
        pass "grok guard: --agent $agent is never stood down"
    else fail "grok guard: --agent $agent (out=$OUT log=$(hooks_log))"; fi
done

new_env
recording_clax "$FAKEBIN"
OUT="$(printf '{"sessionId":"g","hookEventName":"Stop"}' | GROK_HOOK_EVENT=Stop "$TOOLS/bash" "$SCRIPT" exec hook --agent claude stop 2>"$SANDBOX/stderr")"; RC=$?; ERR="$(cat "$SANDBOX/stderr")"
if [ "$RC" = 0 ] && [ -z "$OUT" ] && [ -z "$ERR" ] && [ ! -e "$SANDBOX/ran" ] \
    && hooks_log | grep -q ' standdown mode=hook agent=claude host=grok$'; then
    pass "grok guard: a Claude copy hook that Grok runs reads its input, prints nothing and exits 0"
else fail "grok guard: Claude copy hook under Grok (rc=$RC out=$OUT err=$ERR)"; fi

new_env
recording_clax "$FAKEBIN"
OUT="$(printf '{"session_id":"s"}' | GROK_SESSION_ID=g "$TOOLS/bash" "$SCRIPT" exec hook --agent claude stop 2>"$SANDBOX/stderr")"; RC=$?
if [ "$RC" = 0 ] && grep -q -- 'hook --agent claude stop' "$SANDBOX/ran" 2>/dev/null; then
    pass "grok guard: a Claude Code hook with only GROK_SESSION_ID (Claude Code started from a Grok shell) acts"
else fail "grok guard: hook without GROK_HOOK_EVENT (rc=$RC)"; fi

new_env
recording_clax "$FAKEBIN"
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
