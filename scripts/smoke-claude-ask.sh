#!/usr/bin/env bash
# Manual check of the AskUserQuestion mirror (spec
# 2026-10-06-agent-questions-and-inbox-design §4.4) against a real, interactive
# `claude`. Not a quality gate. No model is called: Claude Code talks to
# scripts/fake-anthropic.py, a scripted Messages API whose first answer calls
# AskUserQuestion ("Which layout?": "Two columns" or "One column") and whose
# second echoes the tool result it was sent, which requests.jsonl records.
#
# Needs `claude`, `tmux` and `python3` on PATH; no login. Everything lives in a
# scratch directory: a CLAX_HOME with a daemon on a free port (never the
# owner's daemon or ~/.clax), and a CLAUDE_CONFIG_DIR holding the first-run
# settings Claude Code reads (onboarding done, the fake API key approved, the
# scratch working directory trusted), so the owner's ~/.claude is never read
# or written. Each scenario runs a fresh `claude` with this checkout's plugin
# (--plugin-dir plugins/claude-code) in a detached tmux session on a private
# socket, and types one prompt:
#
#   4. No surface: no owner stream holds `questions`/`inbox`; the terminal
#      dialog appears within 3 s of the tool call and is answered there.
#   1. Answered in Clax: an owner stream holds `inbox`; the question is
#      answered "One column" through the owner route; the tool result names
#      it, the terminal never drew the dialog, and the question's inbox item
#      is read.
#   2. Skipped: as 1, declined; the tool result is an error saying the person
#      chose not to answer.
#   3. Moved to the terminal: as 1, released; the dialog appears within 10 s,
#      is answered "2" there, and within 5 s the question is answered via
#      `terminal`.
#
# Then once with --dangerously-load-development-channels plugin:clax@clax,
# reporting whether the fake model's AskUserQuestion call reaches the hook.
#
# Every wait polls a condition with a bound. The EXIT trap kills the tmux
# server (by its socket), the stream reader and the fake API (by recorded
# PID), and stops the daemon.
#
# Usage: scripts/smoke-claude-ask.sh [scratch-dir]
set -euo pipefail
cd "$(dirname "$0")/.."
REPO="$PWD"
SCRATCH="${1:-${TMPDIR:-/tmp}/clax-smoke-claude-ask}"
SCRATCH="$(mkdir -p "$SCRATCH" && cd "$SCRATCH" && pwd -P)"
export CLAX_HOME="$SCRATCH/home"
unset CLAX_PORT
export CLAX_NO_OPEN=1
BIN="$REPO/target/debug/clax"
CWD="$SCRATCH/cwd"
CONFIG="$SCRATCH/claude"
LOG="$SCRATCH/requests.jsonl"
PANES="$SCRATCH/panes"
SOCK="clax-smoke-ask-$$"
KEY="sk-ant-fake"
FAKE_PID=""
STREAM_PID=""
DAEMON_PID=""

die() { echo "smoke: FAIL: $1" >&2; exit 1; }
# shellcheck disable=SC2329 # run by the EXIT trap
cleanup() {
    tmux -L "$SOCK" kill-server >/dev/null 2>&1 || true
    local pid
    for pid in $STREAM_PID $FAKE_PID; do
        kill "$pid" >/dev/null 2>&1 || true
        wait "$pid" 2>/dev/null || true
    done
    "$BIN" stop >/dev/null 2>&1 || true
    [ -n "$DAEMON_PID" ] && kill "$DAEMON_PID" >/dev/null 2>&1 || true
}
trap cleanup EXIT

for c in claude tmux python3 curl; do command -v "$c" >/dev/null || die "$c is not on PATH"; done
REAL_HOME="$(cd "$HOME/.clax" 2>/dev/null && pwd -P || true)"
[ -n "$REAL_HOME" ] && [ "$CLAX_HOME" = "$REAL_HOME" ] && die "the scratch CLAX_HOME resolves to ~/.clax"
REAL_CLAUDE="$(cd "$HOME/.claude" 2>/dev/null && pwd -P || true)"
[ -n "$REAL_CLAUDE" ] && [ "$CONFIG" = "$REAL_CLAUDE" ] && die "the scratch CLAUDE_CONFIG_DIR resolves to ~/.claude"

echo "smoke: building clax"
cargo build -q -p clax-cli
rm -rf "$CLAX_HOME" "$CWD" "$CONFIG" "$PANES" "$LOG"
mkdir -p "$CLAX_HOME" "$CWD" "$CONFIG" "$PANES"
: > "$LOG"
echo "smoke: $(claude --version), $("$BIN" --version)"

# Polls `cmd` every 200 ms until it succeeds or `secs` pass.
wait_for() {
    local secs="$1"; shift
    local end=$(( $(date +%s) + secs ))
    until "$@"; do
        [ "$(date +%s)" -lt "$end" ] || return 1
        sleep 0.2
    done
}

# A free port, outside the ports the owner's daemons use.
PORT=""
for _ in 1 2 3 4 5; do
    P="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')"
    case "$P" in 7480|7481|7490) ;; *) PORT="$P"; break ;; esac
done
[ -n "$PORT" ] || die "no free port found"
printf '[serve]\nport = %s\n' "$PORT" > "$CLAX_HOME/config.toml"
"$BIN" serve >/dev/null || die "the scratch daemon did not start"
[ -f "$CLAX_HOME/daemon.json" ] || die "the scratch daemon wrote no daemon.json"
json_field() { python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))[sys.argv[2]])' "$1" "$2"; }
TOKEN="$(json_field "$CLAX_HOME/daemon.json" token)"
DAEMON_PID="$(json_field "$CLAX_HOME/daemon.json" pid)"
# The shell's origin: owner routes refuse another Origin.
BASE="http://localhost:$(json_field "$CLAX_HOME/daemon.json" port)"
echo "smoke: scratch daemon at $BASE (CLAX_HOME=$CLAX_HOME)"
api() { curl -fsS --noproxy '*' -H "Authorization: Bearer $TOKEN" "$@"; }
owner_post() { api -X POST -H "Origin: $BASE" -H 'content-type: application/json' "$@"; }

python3 "$REPO/scripts/fake-anthropic.py" "$SCRATCH" > "$SCRATCH/fake.port" 2> "$SCRATCH/fake.err" &
FAKE_PID=$!
wait_for 10 test -s "$SCRATCH/fake.port" || die "the fake Messages API did not start: $(cat "$SCRATCH/fake.err")"
FAKE="http://127.0.0.1:$(head -1 "$SCRATCH/fake.port")"
echo "smoke: fake Messages API at $FAKE"

# The first-run settings Claude Code reads from its config directory.
python3 - "$CONFIG" "$CWD" "$KEY" <<'EOF'
import json, sys
config, cwd, key = sys.argv[1:4]
json.dump({
    "hasCompletedOnboarding": True, "theme": "dark", "numStartups": 5,
    "customApiKeyResponses": {"approved": [key[-20:]], "rejected": []},
    "projects": {cwd: {"hasTrustDialogAccepted": True, "hasCompletedProjectOnboarding": True, "allowedTools": []}},
}, open(config + "/.claude.json", "w"))
json.dump({}, open(config + "/settings.json", "w"))
EOF

pane() { tmux -L "$SOCK" capture-pane -p -t "$1" 2>/dev/null || true; }
pane_has() { pane "$1" | grep -F -- "$2" >/dev/null; }
# The terminal's AskUserQuestion dialog is on screen.
dialog_shown() { pane_has "$1" "Enter to select"; }

# Starts `claude` in tmux session $1 with extra flags $2..., waits for its
# prompt (answering a first-run prompt the settings did not cover with Enter),
# and types the prompt.
start_claude() {
    local name="$1"; shift
    tmux -L "$SOCK" new-session -d -s "$name" -x 160 -y 50 -c "$CWD" \
        env -u CLAUDECODE -u CLAUDE_CODE_ENTRYPOINT -u CLAUDE_CODE_SSE_PORT \
        CLAUDE_CONFIG_DIR="$CONFIG" ANTHROPIC_BASE_URL="$FAKE" ANTHROPIC_API_KEY="$KEY" \
        CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 DISABLE_AUTOUPDATER=1 \
        CLAX_HOME="$CLAX_HOME" CLAX_BIN="$BIN" CLAX_NO_OPEN=1 \
        claude --plugin-dir "$REPO/plugins/claude-code" "$@"
    local end=$(( $(date +%s) + 30 ))
    until pane_has "$name" "shift+tab to cycle" || pane_has "$name" "? for shortcuts"; do
        if pane_has "$name" "trust" || pane_has "$name" "Press Enter" || pane_has "$name" "development channels"; then
            tmux -L "$SOCK" send-keys -t "$name" Enter
        fi
        [ "$(date +%s)" -lt "$end" ] || { pane "$name" > "$PANES/$name.txt"; die "$name: claude showed no prompt within 30 s (pane in $PANES/$name.txt)"; }
        sleep 0.2
    done
    tmux -L "$SOCK" send-keys -t "$name" -l "Ask me which layout to use."
    tmux -L "$SOCK" send-keys -t "$name" Enter
}

stop_claude() {
    pane "$1" > "$PANES/$1.txt"
    tmux -L "$SOCK" kill-session -t "$1" >/dev/null 2>&1 || true
}

# Prints `<is_error> <text>` of the tool result for the fake's call in the
# requests logged after line $1, or nothing.
result_after() {
    python3 - "$LOG" "$1" <<'EOF'
import json, sys
log, skip = sys.argv[1], int(sys.argv[2])
for line in open(log).read().splitlines()[skip:]:
    body = json.loads(line).get("body") or {}
    for m in (body.get("messages") or []) if isinstance(body, dict) else []:
        for b in m.get("content") if isinstance(m.get("content"), list) else []:
            if b.get("type") == "tool_result" and b.get("tool_use_id") == "toolu_smoke1":
                c = b.get("content")
                text = c if isinstance(c, str) else " ".join(x.get("text", "") for x in c or [])
                print("error" if b.get("is_error") else "ok", text.replace("\n", " "))
                sys.exit(0)
EOF
}
has_result() { [ -n "$(result_after "$1")" ]; }
# The Unix time the fake answered with the AskUserQuestion call, after line $1.
tool_use_time() {
    python3 - "$LOG" "$1" <<'EOF'
import json, sys
for line in open(sys.argv[1]).read().splitlines()[int(sys.argv[2]):]:
    r = json.loads(line)
    if r.get("stop_reason") == "tool_use":
        print(r["t"]); break
EOF
}
has_tool_use() { [ -n "$(tool_use_time "$1")" ]; }

QUESTIONS_SEEN="$SCRATCH/seen.txt"
: > "$QUESTIONS_SEEN"
# The ID of an open question not seen by an earlier scenario, or nothing.
new_open_question() {
    api "$BASE/api/questions" | python3 -c '
import json, sys
seen = set(open(sys.argv[1]).read().split())
for q in json.load(sys.stdin)["questions"]:
    if q["id"] not in seen and q["source"] == "hook":
        print(q["id"]); break' "$QUESTIONS_SEEN"
}
question_field() { api "$BASE/api/questions/$1" | python3 -c 'import json,sys; q=json.load(sys.stdin)["question"]; print(q[sys.argv[1]])' "$2"; }

OPEN_Q=""
# Waits up to 30 s for the scenario's mirrored question, checking on each
# poll that the terminal has not drawn the dialog.
await_question() {
    local name="$1" end=$(( $(date +%s) + 30 ))
    OPEN_Q=""
    while [ -z "$OPEN_Q" ]; do
        dialog_shown "$name" && { stop_claude "$name"; die "$name: the terminal drew the dialog while a Clax surface was open"; }
        OPEN_Q="$(new_open_question)"
        [ -n "$OPEN_Q" ] && break
        [ "$(date +%s)" -lt "$end" ] || { stop_claude "$name"; die "$name: no open question in Clax within 30 s"; }
        sleep 0.2
    done
    echo "$OPEN_Q" >> "$QUESTIONS_SEEN"
}

# Waits up to 15 s for the tool result, failing if the dialog shows meanwhile.
await_result_no_dialog() {
    local name="$1" from="$2" end=$(( $(date +%s) + 15 ))
    until has_result "$from"; do
        dialog_shown "$name" && { stop_claude "$name"; die "$name: the terminal drew the dialog"; }
        [ "$(date +%s)" -lt "$end" ] || { stop_claude "$name"; die "$name: no tool result within 15 s"; }
        sleep 0.2
    done
}

# Opens an owner stream and subscribes it to `inbox`: a Clax surface is open.
hold_surface() {
    curl -sN --noproxy '*' -H "Authorization: Bearer $TOKEN" "$BASE/api/stream" > "$SCRATCH/stream.out" 2>&1 &
    STREAM_PID=$!
    wait_for 10 grep -q '^data:' "$SCRATCH/stream.out" || die "the owner stream did not open"
    local sid
    sid="$(python3 -c '
import json, sys
for l in open(sys.argv[1]):
    if l.startswith("data:"):
        print(json.loads(l[5:])["stream"]); break' "$SCRATCH/stream.out")"
    owner_post -d '{"subscribe":["inbox"]}' "$BASE/api/stream/$sid" >/dev/null || die "could not subscribe the owner stream to inbox"
    echo "smoke: owner stream $sid holds inbox (curl PID $STREAM_PID)"
}

RESULTS=()
pass() { echo "smoke: PASS: $1"; RESULTS+=("PASS: $1"); }

# --- 4. No surface -----------------------------------------------------------
FROM="$(wc -l < "$LOG")"
start_claude s4
wait_for 30 has_tool_use "$FROM" || { stop_claude s4; die "s4: the fake model was never asked"; }
T_TOOL="$(tool_use_time "$FROM")"
wait_for 10 dialog_shown s4 || { stop_claude s4; die "s4: the terminal dialog did not appear"; }
T_DIALOG="$(python3 -c 'import time; print(time.time())')"
LAG="$(python3 -c 'import sys; print("%.1f" % (float(sys.argv[2]) - float(sys.argv[1])))' "$T_TOOL" "$T_DIALOG")"
python3 -c 'import sys; sys.exit(0 if float(sys.argv[1]) <= 3.0 else 1)' "$LAG" || { stop_claude s4; die "s4: the dialog appeared ${LAG} s after the tool call (over 3 s)"; }
Q4="$(api "$BASE/api/questions?status=all" | python3 -c 'import json,sys; q=[x for x in json.load(sys.stdin)["questions"] if x["source"]=="hook"]; print(q[0]["id"] if q else "")')"
[ -n "$Q4" ] || { stop_claude s4; die "s4: the call was not mirrored"; }
echo "$Q4" >> "$QUESTIONS_SEEN"
tmux -L "$SOCK" send-keys -t s4 2 Enter
wait_for 15 has_result "$FROM" || { stop_claude s4; die "s4: no tool result after answering in the terminal"; }
R="$(result_after "$FROM")"
stop_claude s4
case "$R" in "ok "*"One column"*) ;; *) die "s4: the tool result does not name One column: $R" ;; esac
pass "4 no surface: dialog ${LAG} s after the tool call; question $Q4 $(question_field "$Q4" status) via $(question_field "$Q4" answered_via); tool result: ${R#ok }"

hold_surface

# --- 1. Answered in Clax -----------------------------------------------------
FROM="$(wc -l < "$LOG")"
start_claude s1
await_question s1
Q1="$OPEN_Q"
owner_post -d '{"answers":[{"selected":["One column"],"text":null}]}' "$BASE/api/questions/$Q1/answer" >/dev/null || die "s1: answering $Q1 failed"
await_result_no_dialog s1 "$FROM"
R="$(result_after "$FROM")"
stop_claude s1
case "$R" in "ok "*"One column"*) ;; *) die "s1: the tool result does not name One column: $R" ;; esac
READ="$(api "$BASE/api/inbox?kind=question" | python3 -c '
import json, sys
print(next((str(i["read"]).lower() for i in json.load(sys.stdin)["items"] if (i.get("question") or {}).get("id") == sys.argv[1]), "missing"))' "$Q1")"
[ "$READ" = true ] || die "s1: the question's inbox item is not read ($READ)"
pass "1 answered in Clax: question $Q1 answered via $(question_field "$Q1" answered_via), no dialog, inbox item read; tool result: ${R#ok }"

# --- 2. Skipped --------------------------------------------------------------
FROM="$(wc -l < "$LOG")"
start_claude s2
await_question s2
Q2="$OPEN_Q"
owner_post -d '{}' "$BASE/api/questions/$Q2/decline" >/dev/null || die "s2: declining $Q2 failed"
await_result_no_dialog s2 "$FROM"
R="$(result_after "$FROM")"
stop_claude s2
case "$R" in "error "*"chose not to answer"*) ;; *) die "s2: the tool result is not the skip error: $R" ;; esac
pass "2 skipped: question $Q2 $(question_field "$Q2" status), no dialog; tool result (is_error): ${R#error }"

# --- 3. Moved to the terminal ------------------------------------------------
FROM="$(wc -l < "$LOG")"
start_claude s3
await_question s3
Q3="$OPEN_Q"
owner_post -d '{}' "$BASE/api/questions/$Q3/release" >/dev/null || die "s3: releasing $Q3 failed"
wait_for 10 dialog_shown s3 || { stop_claude s3; die "s3: the dialog did not appear within 10 s of the release"; }
tmux -L "$SOCK" send-keys -t s3 2 Enter
wait_for 15 has_result "$FROM" || { stop_claude s3; die "s3: no tool result after answering in the terminal"; }
R="$(result_after "$FROM")"
case "$R" in "ok "*"One column"*) ;; *) stop_claude s3; die "s3: the tool result does not name One column: $R" ;; esac
terminal_answered() { [ "$(question_field "$Q3" status) $(question_field "$Q3" answered_via)" = "answered terminal" ]; }
wait_for 5 terminal_answered || { stop_claude s3; die "s3: question $Q3 is not answered via terminal within 5 s"; }
stop_claude s3
pass "3 moved to the terminal: dialog after release, question $Q3 answered via terminal; tool result: ${R#ok }"

# --- The development channel -------------------------------------------------
FROM="$(wc -l < "$LOG")"
BEFORE="$(api "$BASE/api/questions?status=all&limit=200" | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["questions"]))')"
start_claude ch --dangerously-load-development-channels plugin:clax@clax
CH_NOTE=""
pane_has ch "Channels are not currently available" && CH_NOTE="; Claude Code said: --dangerously-load-development-channels ignored (plugin:clax@clax), Channels are not currently available"
wait_for 30 has_tool_use "$FROM" || { stop_claude ch; die "ch: the fake model was never asked"; }
OFFERED="$(python3 - "$LOG" "$FROM" <<'EOF'
import json, sys
for line in open(sys.argv[1]).read().splitlines()[int(sys.argv[2]):]:
    r = json.loads(line)
    if r.get("stop_reason") == "tool_use":
        print("yes" if any(t.get("name") == "AskUserQuestion" for t in r["body"].get("tools") or []) else "no"); break
EOF
)"
mirrored() { [ "$(api "$BASE/api/questions?status=all&limit=200" | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["questions"]))')" -gt "$BEFORE" ]; }
if wait_for 10 mirrored; then REACHED="reached the hook (mirrored into Clax)"
elif has_result "$FROM"; then REACHED="did not reach the hook; tool result: $(result_after "$FROM")"
else REACHED="did not reach the hook within 10 s"; fi
stop_claude ch
CHANNEL="channel: AskUserQuestion offered: $OFFERED; the call $REACHED$CH_NOTE"
echo "smoke: $CHANNEL"

echo
for r in "${RESULTS[@]}"; do echo "smoke: $r"; done
echo "smoke: $CHANNEL"
echo "smoke: PASS (panes in $PANES, requests in $LOG)"
