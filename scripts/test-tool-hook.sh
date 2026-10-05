#!/usr/bin/env bash
# Tests the PostToolUse gate (scripts/tool-hook.sh) against a fake
# ensure-clax.sh that records each run. Uses a scratch HOME and CLAX_HOME.
set -uo pipefail
cd "$(dirname "$0")/.."
FAILED=0
fail() { echo "FAIL: $1"; FAILED=1; }
pass() { echo "PASS: $1"; }
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
mkdir -p "$T/bin" "$T/fakehome"
cp scripts/tool-hook.sh "$T/bin/tool-hook.sh"
cat > "$T/bin/ensure-clax.sh" <<'SH'
#!/bin/sh
{ printf '%s|' "$*"; cat; echo; } >> "$CALLS"
echo '{"noise": true}'
echo 'noise' >&2
exit 3
SH
chmod +x "$T/bin/tool-hook.sh" "$T/bin/ensure-clax.sh"
export HOME="$T/fakehome" CLAX_HOME="$T/home" CALLS="$T/calls"
STAMPS="$CLAX_HOME/run/tool-hook"
calls() { if [ -f "$CALLS" ]; then wc -l < "$CALLS" | tr -d ' '; else echo 0; fi; }
gate() { printf '%s' "$2" | "$T/bin/tool-hook.sh" "$1" 2>"$T/err"; }
# Sets a file's mtime $2 seconds from now (perl: no interpreter shim to start).
age() { perl -e '$t = time + $ARGV[1]; utime $t, $t, $ARGV[0] or die' "$1" "$2"; }
IN='{"session_id":"cc-1","hook_event_name":"PostToolUse","tool_name":"Edit"}'

out="$(gate claude "$IN")"; code=$?
if [ "$code" = 0 ] && [ -z "$out" ] && [ ! -s "$T/err" ]; then pass "exits 0 and prints nothing, even when clax fails and writes output"
else fail "exit $code, stdout '$out', stderr '$(cat "$T/err")'"; fi
if [ "$(calls)" = 1 ] && grep -qF "exec hook --agent claude tool|$IN" "$CALLS"; then pass "the first call runs clax hook ... tool with the hook input"
else fail "first call: $(cat "$CALLS" 2>/dev/null)"; fi
if [ -f "$STAMPS/claude-cc-1" ]; then pass "the stamp is under CLAX_HOME/run/tool-hook"; else fail "no stamp at $STAMPS/claude-cc-1"; fi
if [ -z "$(ls -A "$HOME")" ]; then pass "nothing is written under HOME (no ~/.claude, no ~/.codex)"; else fail "HOME holds: $(ls -A "$HOME")"; fi

gate claude "$IN" >/dev/null
if [ "$(calls)" = 1 ]; then pass "a call within 60 s starts no clax"; else fail "throttle: $(calls) runs"; fi
# 55 s, not 59: the gate reads the clock later, in whole seconds.
age "$STAMPS/claude-cc-1" -55
gate claude "$IN" >/dev/null
if [ "$(calls)" = 1 ]; then pass "a stamp 55 s old still skips"; else fail "55 s: $(calls) runs"; fi
age "$STAMPS/claude-cc-1" -61
gate claude "$IN" >/dev/null
if [ "$(calls)" = 2 ]; then pass "a stamp 61 s old renews again"; else fail "61 s: $(calls) runs"; fi
age "$STAMPS/claude-cc-1" 3600
gate claude "$IN" >/dev/null
if [ "$(calls)" = 3 ]; then pass "a stamp dated in the future counts as old"; else fail "future: $(calls) runs"; fi

gate claude '{"session_id": "cc-2"}' >/dev/null
gate codex '{"session_id":"cc-1"}' >/dev/null
if [ "$(calls)" = 5 ] && [ -f "$STAMPS/claude-cc-2" ] && [ -f "$STAMPS/codex-cc-1" ]; then pass "stamps are per harness and per session (spaced JSON too)"
else fail "per session: $(calls) runs, $(ls "$STAMPS")"; fi

gate claude '{"hook_event_name":"PostToolUse"}' >/dev/null
gate claude '{"hook_event_name":"PostToolUse"}' >/dev/null
gate claude '{"session_id":"../../escape"}' >/dev/null
if [ "$(calls)" = 8 ] && [ "$(ls "$STAMPS" | sort | tr '\n' ' ')" = "claude-cc-1 claude-cc-2 codex-cc-1 " ] && [ ! -e "$CLAX_HOME/escape" ]; then
  pass "without a usable session_id every call runs clax and no stamp is written"
else fail "no session: $(calls) runs, $(ls -R "$CLAX_HOME")"; fi

out="$(gate nope "$IN")"; code=$?
if [ "$code" = 0 ] && [ -z "$out" ] && [ "$(calls)" = 8 ]; then pass "an unknown harness does nothing and exits 0"; else fail "unknown harness: $code '$out' $(calls)"; fi

chmod 000 "$STAMPS"
out="$(gate claude '{"session_id":"cc-3"}')"; code=$?
chmod 755 "$STAMPS"
if [ "$code" = 0 ] && [ -z "$out" ] && [ "$(calls)" = 9 ]; then pass "an unwritable stamp directory still renews and exits 0"; else fail "unwritable: $code '$out' $(calls)"; fi

if [ "$FAILED" -ne 0 ]; then echo "tool hook gate checks failed"; exit 1; fi
echo "tool hook gate checks passed"
