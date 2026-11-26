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
# shellcheck source=scripts/fake-exe.sh
. scripts/fake-exe.sh
fake_exe "$T/bin/tool-hook.sh" < scripts/tool-hook.sh
fake_exe "$T/bin/ensure-clax.sh" <<'SH'
#!/bin/sh
{ printf '%s|' "$*"; cat; echo; } >> "$CALLS"
echo '{"noise": true}'
echo 'noise' >&2
exit 3
SH
export HOME="$T/fakehome" CLAX_HOME="$T/home" CALLS="$T/calls"
STAMPS="$CLAX_HOME/run/tool-hook"
calls() { if [ -f "$CALLS" ]; then wc -l < "$CALLS" | tr -d ' '; else echo 0; fi; }
# The input comes from a process substitution, not a pipe: a gate that exits
# without reading it (an unknown harness) would leave a pipe's writer to die
# of SIGPIPE, and pipefail would report that as the gate's status.
# gate <harness> <input> [mode]
gate() { local agent="$1" input="$2"; shift 2; "$T/bin/tool-hook.sh" "$agent" "$@" < <(printf '%s' "$input") 2>"$T/err"; }
# Sets a file's mtime $2 seconds from now.
age() { python3 -c 'import os, sys, time; t = time.time() + int(sys.argv[2]); os.utime(sys.argv[1], (t, t))' "$1" "$2"; }
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

AG='"conversationId":"c0ffee00-0000-4000-8000-000000000001","hookEventName":"PreInvocation"'
n="$(calls)"
out="$(gate antigravity "{$AG,\"invocationNum\":0}" prompt)"; code=$?
if [ "$code" = 0 ] && [ "$out" = '{"noise": true}' ] && [ ! -s "$T/err" ] && [ "$(calls)" = $((n + 1)) ] \
  && tail -n 1 "$CALLS" | grep -qF "exec hook --agent antigravity prompt|{$AG,\"invocationNum\":0}"; then
  pass "antigravity prompt at a turn's first model call runs clax hook ... prompt and passes its stdout through"
else fail "prompt, invocationNum 0: exit $code, stdout '$out', $(calls) runs, $(tail -n 1 "$CALLS")"; fi
out="$(gate antigravity "{$AG,\"invocationNum\":0}" prompt)"
if [ "$out" = '{"noise": true}' ] && [ "$(calls)" = $((n + 2)) ] && [ ! -e "$STAMPS/antigravity-c0ffee00-0000-4000-8000-000000000001" ]; then
  pass "prompt mode keeps no stamp and runs again within 60 s"
else fail "prompt again: stdout '$out', $(calls) runs, $(ls "$STAMPS")"; fi
out="$(gate antigravity "{$AG,\"invocationNum\":2}" prompt)"; code=$?
if [ "$code" = 0 ] && [ -z "$out" ] && [ "$(calls)" = $((n + 2)) ]; then pass "a later model call of the turn starts no clax"
else fail "prompt, invocationNum 2: exit $code, stdout '$out', $(calls) runs"; fi
out="$(gate antigravity "{$AG}" prompt)"
gate antigravity "{$AG, \"invocationNum\": 0}" prompt >/dev/null
if [ "$out" = '{"noise": true}' ] && [ "$(calls)" = $((n + 4)) ]; then pass "without invocationNum, or with it spaced, prompt mode runs clax"
else fail "prompt, no invocationNum: stdout '$out', $(calls) runs"; fi

n="$(calls)"
TIN='{"conversationId":"c0ffee00-0000-4000-8000-000000000001","hookEventName":"PostToolUse"}'
out="$(gate antigravity "$TIN")"
gate antigravity "$TIN" >/dev/null
if [ -z "$out" ] && [ "$(calls)" = $((n + 1)) ] && [ -f "$STAMPS/antigravity-c0ffee00-0000-4000-8000-000000000001" ] \
  && tail -n 1 "$CALLS" | grep -qF "exec hook --agent antigravity tool|$TIN"; then
  pass "antigravity's tool hook stamps by conversationId and runs at most once a minute"
else fail "antigravity tool: stdout '$out', $(calls) runs, $(ls "$STAMPS")"; fi
gate antigravity '{"conversationId":"../x"}' >/dev/null
if [ "$(calls)" = $((n + 2)) ] && [ ! -e "$STAMPS/antigravity-../x" ] && [ ! -e "$CLAX_HOME/run/x" ] \
  && [ "$(find "$STAMPS" -name 'antigravity-*' | wc -l | tr -d ' ')" = 1 ]; then
  pass "an unusable conversationId stamps nothing"
else fail "antigravity ../x: $(calls) runs, $(ls -R "$CLAX_HOME")"; fi

if [ "$FAILED" -ne 0 ]; then echo "tool hook gate checks failed"; exit 1; fi
echo "tool hook gate checks passed"
