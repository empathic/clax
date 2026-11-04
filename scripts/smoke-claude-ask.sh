#!/usr/bin/env bash
# Manual check of the AskUserQuestion mirror (spec
# 2026-10-06-agent-questions-and-inbox-design §4.4) against a real, interactive
# `claude`. Not a quality gate. No model is called: Claude Code talks to
# scripts/fake-anthropic.py, a scripted Messages API that first watches the
# page the script published (the Clax plugin's `watch` tool, replies off), so
# the session has a page; holds its next answer until the script releases it;
# then calls AskUserQuestion ("Which layout?": "Two columns" or "One
# column"); and last echoes the tool result it was sent, which
# requests.jsonl records. The hook waits in Clax only for a session with a
# page, and only when the daemon's suggestion of where the person is names
# Clax (crates/clax-hooks/src/ask.rs): the person's prompt in the terminal
# names the terminal unless they engage in Clax after it.
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
#   4. No surface: the session watches the page, but no owner stream holds
#      `questions`/`inbox`; the hook asks in the terminal at once (hooks.log:
#      `ask mode=terminal outcome=terminal`), so the terminal dialog
#      appears, and it is answered there. The time from the tool call to the
#      dialog is printed.
#   1. Answered in Clax: an owner stream holds `inbox`, the session watches
#      the page, and after the prompt, before the question, the person's
#      focused tab reports them on the page (an owner browser session, as
#      `clax open` opens one: PUT /api/viewers/me/presence); the question is
#      answered "One column" from that browser session; the tool result
#      names it, the terminal never drew the dialog, and the question's
#      inbox item is read.
#   2. Skipped: as 1, declined; the tool result is an error saying the person
#      chose not to answer.
#   3. Moved to the terminal: as 1, released; the dialog appears, is
#      answered "2" there, and the question is answered via `terminal`.
#
# Then once with --dangerously-load-development-channels plugin:clax@clax,
# reporting whether the fake model's AskUserQuestion call reaches the hook.
#
# The person's decisions in Clax (answer, skip, move to the terminal) come
# from the owner browser session: with an agent session live, the token
# alone would wait for a browser's confirmation.
#
# Every wait polls a condition, bounded by a hang guard
# (scripts/lib/wait.sh's wait_for), which only ends a run that is stuck. A
# wait for the question or its answer in Clax fails at once when the
# terminal draws the dialog meanwhile. The EXIT trap kills the tmux
# server (by its socket), the stream reader and the fake API (by recorded
# PID), and stops the daemon.
#
# Usage: scripts/smoke-claude-ask.sh [scratch-dir]
# CLAX_TEST_BIN=<path> uses that debug binary instead of building one.
set -euo pipefail
cd "$(dirname "$0")/.."
REPO="$PWD"
SCRATCH="${1:-${TMPDIR:-/tmp}/clax-smoke-claude-ask}"
SCRATCH="$(mkdir -p "$SCRATCH" && cd "$SCRATCH" && pwd -P)"
export CLAX_HOME="$SCRATCH/home"
unset CLAX_PORT
export CLAX_NO_OPEN=1
BIN="${CLAX_TEST_BIN:-$REPO/target/debug/clax}"
case "$BIN" in /*) ;; *) BIN="$REPO/$BIN" ;; esac
# shellcheck source=scripts/lib/wait.sh
. scripts/lib/wait.sh
CWD="$SCRATCH/cwd"
CONFIG="$SCRATCH/claude"
LOG="$SCRATCH/requests.jsonl"
# What scripts/fake-anthropic.py reads: the page its watch call names, the
# file it creates when it holds its question, and the FIFO that releases it.
WATCH_ID="$SCRATCH/watch.id"
HELD="$SCRATCH/ask.held"
UNWATCHED="$SCRATCH/ask.unwatched"
GO="$SCRATCH/ask.go"
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

if [ -z "${CLAX_TEST_BIN:-}" ]; then
    echo "smoke: building clax"
    cargo build -q -p clax-cli
fi
# A file a background process writes is removed first too: the wait for it
# must not read a previous run's.
rm -rf "$CLAX_HOME" "$CWD" "$CONFIG" "$PANES" "$LOG" "$WATCH_ID" "$HELD" "$UNWATCHED" "$GO" \
    "$SCRATCH/fake.port" "$SCRATCH/fake.err" "$SCRATCH/stream.out"
mkdir -p "$CLAX_HOME" "$CWD" "$CONFIG" "$PANES"
: > "$LOG"
mkfifo "$GO"
echo "smoke: $(claude --version), $("$BIN" --version)"

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
token_post() { api -X POST -H "Origin: $BASE" -H 'content-type: application/json' "$@"; }

# An owner browser session, opened as `clax open` opens one: a login code
# made with the token, redeemed at GET /login, whose page carries the
# session's proof.
login="$(api -X POST "$BASE/api/browser-login" | python3 -c 'import json,sys; print(json.load(sys.stdin)["path"])')" \
    || die "could not make a login code"
curl -fsS --noproxy '*' -D "$SCRATCH/login.headers" -o "$SCRATCH/login.html" "$BASE$login" || die "could not redeem the login code"
OWNER_COOKIE="$(sed -n "s/^[Ss]et-[Cc]ookie: \(clax_owner_$(json_field "$CLAX_HOME/daemon.json" port)=[^;]*\).*/\1/p" "$SCRATCH/login.headers" | tr -d '\r')"
OWNER_PROOF="$(sed -n 's/.*data-proof="\([^"]*\)".*/\1/p' "$SCRATCH/login.html")"
[ -n "$OWNER_COOKIE" ] && [ -n "$OWNER_PROOF" ] || die "the login set no owner cookie or proof"
# A request from that browser session.
owner() {
    curl -fsS --noproxy '*' -H "Origin: $BASE" -H "Cookie: $OWNER_COOKIE" -H "x-clax-owner-proof: $OWNER_PROOF" \
        -H 'content-type: application/json' "$@"
}
owner_post() { owner -X POST "$@"; }

# The page every scenario's session watches but the development channel's.
PAGE_AID="$(api -X POST -H 'content-type: application/json' \
    -d '{"title": "Layout", "files": {"index.html": {"content": "<!doctype html><title>Layout</title><p>Two columns or one?</p>", "encoding": "utf8"}}}' \
    "$BASE/api/artifacts" | python3 -c 'import json,sys; print(json.load(sys.stdin)["artifact"]["id"])')" \
    || die "could not publish the page"
echo "$PAGE_AID" > "$WATCH_ID"
echo "smoke: the sessions watch $PAGE_AID"

python3 "$REPO/scripts/fake-anthropic.py" "$SCRATCH" > "$SCRATCH/fake.port" 2> "$SCRATCH/fake.err" &
FAKE_PID=$!
wait_for "the fake Messages API's port" test -s "$SCRATCH/fake.port" || die "the fake Messages API did not start: $(cat "$SCRATCH/fake.err")"
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
# The fake model's watch call runs without a permission prompt.
json.dump({"permissions": {"allow": ["mcp__plugin_clax_clax__watch"]}}, open(config + "/settings.json", "w"))
EOF

pane() { tmux -L "$SOCK" capture-pane -p -t "$1" 2>/dev/null || true; }
pane_has() { pane "$1" | grep -F -- "$2" >/dev/null; }
# The terminal's AskUserQuestion dialog is on screen.
dialog_shown() { pane_has "$1" "Enter to select"; }

# Whether `claude` in tmux session $1 shows its prompt; while it shows a
# first-run prompt the settings did not cover instead, answers it with Enter.
claude_ready() {
    if pane_has "$1" "shift+tab to cycle" || pane_has "$1" "? for shortcuts"; then return 0; fi
    if pane_has "$1" "trust" || pane_has "$1" "Press Enter" || pane_has "$1" "development channels"; then
        tmux -L "$SOCK" send-keys -t "$1" Enter
    fi
    return 1
}

# Starts `claude` in tmux session $1 with extra flags $2..., waits for its
# prompt (claude_ready), and types the prompt.
start_claude() {
    local name="$1"; shift
    # ENABLE_TOOL_SEARCH=false: the plugin's MCP tools are offered to the
    # model as they are, not deferred behind ToolSearch, so the fake model
    # can call `watch`.
    tmux -L "$SOCK" new-session -d -s "$name" -x 160 -y 50 -c "$CWD" \
        env -u CLAUDECODE -u CLAUDE_CODE_ENTRYPOINT -u CLAUDE_CODE_SSE_PORT \
        CLAUDE_CONFIG_DIR="$CONFIG" ANTHROPIC_BASE_URL="$FAKE" ANTHROPIC_API_KEY="$KEY" \
        CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 DISABLE_AUTOUPDATER=1 ENABLE_TOOL_SEARCH=false \
        CLAX_HOME="$CLAX_HOME" CLAX_BIN="$BIN" CLAX_NO_OPEN=1 \
        claude --plugin-dir "$REPO/plugins/claude-code" "$@"
    wait_for "$name: claude's prompt" claude_ready "$name" \
        || { pane "$name" > "$PANES/$name.txt"; die "$name: claude showed no prompt (pane in $PANES/$name.txt)"; }
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
    if r.get("tool") == "AskUserQuestion":
        print(r["t"]); break
EOF
}
has_tool_use() { [ -n "$(tool_use_time "$1")" ]; }

held_or_unwatched() { [ -f "$HELD" ] || [ -f "$UNWATCHED" ]; }
# Waits until session $1's watch of the page has reached the fake model,
# which then holds its AskUserQuestion call; runs $2... (what happens after
# the watch and before the question); and releases the call.
release_ask() {
    local name="$1"; shift
    wait_for "$name: the session's watch of the page" held_or_unwatched \
        || { stop_claude "$name"; die "$name: the session never watched the page (pane in $PANES/$name.txt)"; }
    [ -f "$UNWATCHED" ] && { stop_claude "$name"; die "$name: the session did not watch the page: $(cat "$UNWATCHED")"; }
    "$@"
    rm -f "$HELD"
    printf 'go\n' > "$GO"
}
# The person looks at the page in Clax: their focused tab there reports them.
look_at_page() {
    owner -X PUT -d "{\"artifact_id\": \"$PAGE_AID\", \"state\": \"here\", \"tab\": \"smoke\", \"focused\": true}" \
        "$BASE/api/viewers/me/presence" >/dev/null || die "could not report the person on $PAGE_AID"
}

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
# Whether session $1's mirrored question is open in Clax (set in OPEN_Q);
# fails the run if the terminal has drawn the dialog instead.
question_in_clax() {
    dialog_shown "$1" && { stop_claude "$1"; die "$1: the terminal drew the dialog while a Clax surface was open"; }
    OPEN_Q="$(new_open_question)"
    [ -n "$OPEN_Q" ]
}
# Waits for the scenario's mirrored question.
await_question() {
    OPEN_Q=""
    wait_for "$1: an open question in Clax" question_in_clax "$1" || { stop_claude "$1"; die "$1: no open question in Clax"; }
    echo "$OPEN_Q" >> "$QUESTIONS_SEEN"
}

# Whether the tool result for the requests after line $2 is in; fails the
# run if session $1's terminal has drawn the dialog.
result_no_dialog() {
    dialog_shown "$1" && { stop_claude "$1"; die "$1: the terminal drew the dialog"; }
    has_result "$2"
}
# Waits for the tool result, failing if the dialog shows meanwhile.
await_result_no_dialog() {
    wait_for "$1: the tool result" result_no_dialog "$1" "$2" || { stop_claude "$1"; die "$1: no tool result"; }
}

# Opens an owner stream and subscribes it to `inbox`: a Clax surface is open.
hold_surface() {
    curl -sN --noproxy '*' -H "Authorization: Bearer $TOKEN" "$BASE/api/stream" > "$SCRATCH/stream.out" 2>&1 &
    STREAM_PID=$!
    wait_for "the owner stream" grep -q '^data:' "$SCRATCH/stream.out" || die "the owner stream did not open"
    local sid
    sid="$(python3 -c '
import json, sys
for l in open(sys.argv[1]):
    if l.startswith("data:"):
        print(json.loads(l[5:])["stream"]); break' "$SCRATCH/stream.out")"
    token_post -d '{"subscribe":["inbox"]}' "$BASE/api/stream/$sid" >/dev/null || die "could not subscribe the owner stream to inbox"
    echo "smoke: owner stream $sid holds inbox (curl PID $STREAM_PID)"
}

RESULTS=()
pass() { echo "smoke: PASS: $1"; RESULTS+=("PASS: $1"); }

# --- 4. No surface -----------------------------------------------------------
FROM="$(wc -l < "$LOG")"
start_claude s4
release_ask s4 true
wait_for "s4: the fake model's AskUserQuestion call" has_tool_use "$FROM" || { stop_claude s4; die "s4: the fake model was never asked"; }
T_TOOL="$(tool_use_time "$FROM")"
wait_for "s4: the terminal dialog" dialog_shown s4 || { stop_claude s4; die "s4: the terminal dialog did not appear"; }
# A measurement, printed: what decides the scenario is the hook's outcome.
T_DIALOG="$(python3 -c 'import time; print(time.time())')"
LAG="$(python3 -c 'import sys; print("%.1f" % (float(sys.argv[2]) - float(sys.argv[1])))' "$T_TOOL" "$T_DIALOG")"
HOOKS_LOG="$CLAX_HOME/logs/hooks.log"
first_ask() { sed -n '/ ask mode=/{s/.* \(ask mode=[^ ]* outcome=[^ ]*\).*/\1/p;q;}' "$HOOKS_LOG" 2>/dev/null; }
asked() { [ -n "$(first_ask)" ]; }
wait_for "s4: the hook's ask line in $HOOKS_LOG" asked || { stop_claude s4; die "s4: hooks.log records no ask run"; }
[ "$(first_ask)" = "ask mode=terminal outcome=terminal" ] \
    || { stop_claude s4; die "s4: the hook did not ask in the terminal at once: $(first_ask)"; }
Q4="$(api "$BASE/api/questions?status=all" | python3 -c 'import json,sys; q=[x for x in json.load(sys.stdin)["questions"] if x["source"]=="hook"]; print(q[0]["id"] if q else "")')"
[ -n "$Q4" ] || { stop_claude s4; die "s4: the call was not mirrored"; }
echo "$Q4" >> "$QUESTIONS_SEEN"
tmux -L "$SOCK" send-keys -t s4 2 Enter
wait_for "s4: the tool result" has_result "$FROM" || { stop_claude s4; die "s4: no tool result after answering in the terminal"; }
R="$(result_after "$FROM")"
stop_claude s4
case "$R" in "ok "*"One column"*) ;; *) die "s4: the tool result does not name One column: $R" ;; esac
pass "4 no surface: the hook asked in the terminal at once (dialog ${LAG} s after the tool call); question $Q4 $(question_field "$Q4" status) via $(question_field "$Q4" answered_via); tool result: ${R#ok }"

hold_surface

# --- 1. Answered in Clax -----------------------------------------------------
FROM="$(wc -l < "$LOG")"
start_claude s1
release_ask s1 look_at_page
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
release_ask s2 look_at_page
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
release_ask s3 look_at_page
await_question s3
Q3="$OPEN_Q"
owner_post -d '{}' "$BASE/api/questions/$Q3/release" >/dev/null || die "s3: releasing $Q3 failed"
wait_for "s3: the terminal dialog" dialog_shown s3 || { stop_claude s3; die "s3: the dialog did not appear after the release"; }
tmux -L "$SOCK" send-keys -t s3 2 Enter
wait_for "s3: the tool result" has_result "$FROM" || { stop_claude s3; die "s3: no tool result after answering in the terminal"; }
R="$(result_after "$FROM")"
case "$R" in "ok "*"One column"*) ;; *) stop_claude s3; die "s3: the tool result does not name One column: $R" ;; esac
terminal_answered() { [ "$(question_field "$Q3" status) $(question_field "$Q3" answered_via)" = "answered terminal" ]; }
wait_for "s3: question $Q3 answered via terminal" terminal_answered || { stop_claude s3; die "s3: question $Q3 is not answered via terminal"; }
stop_claude s3
pass "3 moved to the terminal: dialog after release, question $Q3 answered via terminal; tool result: ${R#ok }"

# --- The development channel -------------------------------------------------
# The fake model asks at once: no watch.
rm -f "$WATCH_ID"
FROM="$(wc -l < "$LOG")"
BEFORE="$(api "$BASE/api/questions?status=all&limit=200" | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["questions"]))')"
start_claude ch --dangerously-load-development-channels plugin:clax@clax
CH_NOTE=""
pane_has ch "Channels are not currently available" && CH_NOTE="; Claude Code said: --dangerously-load-development-channels ignored (plugin:clax@clax), Channels are not currently available"
wait_for "ch: the fake model's AskUserQuestion call" has_tool_use "$FROM" || { stop_claude ch; die "ch: the fake model was never asked"; }
OFFERED="$(python3 - "$LOG" "$FROM" <<'EOF'
import json, sys
for line in open(sys.argv[1]).read().splitlines()[int(sys.argv[2]):]:
    r = json.loads(line)
    if r.get("stop_reason") == "tool_use":
        print("yes" if any(t.get("name") == "AskUserQuestion" for t in r["body"].get("tools") or []) else "no"); break
EOF
)"
mirrored() { [ "$(api "$BASE/api/questions?status=all&limit=200" | python3 -c 'import json,sys; print(len(json.load(sys.stdin)["questions"]))')" -gt "$BEFORE" ]; }
# The call ends one of three ways: mirrored into Clax (the hook ran), a tool
# result without it, or the terminal's dialog without it.
settled() { mirrored || has_result "$FROM" || dialog_shown ch; }
wait_for "ch: the AskUserQuestion call to reach the hook, a result or the terminal" settled || true
if mirrored; then REACHED="reached the hook (mirrored into Clax)"
elif has_result "$FROM"; then REACHED="did not reach the hook; tool result: $(result_after "$FROM")"
elif dialog_shown ch; then REACHED="did not reach the hook; the terminal drew the dialog"
else REACHED="did not reach the hook, and neither a tool result nor the dialog came"; fi
stop_claude ch
CHANNEL="channel: AskUserQuestion offered: $OFFERED; the call $REACHED$CH_NOTE"
echo "smoke: $CHANNEL"

echo
for r in "${RESULTS[@]}"; do echo "smoke: $r"; done
echo "smoke: $CHANNEL"
echo "smoke: PASS (panes in $PANES, requests in $LOG)"
