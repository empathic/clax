#!/usr/bin/env bash
# Manual end-to-end check of the Grok Build path. Not a quality gate: it runs
# real `grok -p` sessions, which call a model and need XAI_API_KEY (or Grok's
# own login, copied into the scratch GROK_HOME for the run and deleted on
# exit). The owner runs it; agents never do.
#
# Everything runs in a scratch root: HOME, GROK_HOME, CLAX_HOME and a free
# port, so ~/.grok, ~/.claude, ~/.clax and ports 7480/7481 are never touched.
# CLAX_BIN is this working tree's target/debug/clax.
#
# It checks, and prints a PASS/FAIL line for each (open-questions.md Q4):
#   a  clax init --agent grok installs clax-grok (`grok plugin install <dir> --trust`)
#      and uninit removes it; `grok plugin list --json` names its source
#   c  a session registers as harness grok with GROK_SESSION_ID; the hooks run
#   d  a sent comment blocks Stop once at the end of a turn (stopHookActive on the next)
#   e  with the Claude Code copy also enabled (scratch ~/.claude/plugins), only
#      clax_grok acts: one live session, standdown lines for the Claude copy
#   m  `clax feedback follow` under the monitor tool prints one line per comment
#      (interactive: the script prints the steps for an idle TUI and waits)
#   v  `grok --version`, recorded for docs/contract.md
# Usage: scripts/smoke-grok.sh [scratch-dir]
set -euo pipefail
cd "$(dirname "$0")/.."
REPO="$PWD"
REAL_HOME="$HOME"
SCRATCH="${1:-${TMPDIR:-/tmp}/clax-smoke-grok}"
SCRATCH="$(mkdir -p "$SCRATCH" && cd "$SCRATCH" && pwd -P)"
REAL_HOME_P="$(cd "$REAL_HOME" && pwd -P)"

die() { echo "smoke: FAIL: $1" >&2; exit 1; }
[ "$SCRATCH" = "$REAL_HOME_P" ] && die "the scratch root is your home"
case "$REAL_HOME_P/" in "$SCRATCH"/*) die "the scratch root contains your home" ;; esac
command -v grok >/dev/null || die "grok is not on PATH"
command -v python3 >/dev/null || die "python3 is needed"

export HOME="$SCRATCH/home"
export GROK_HOME="$SCRATCH/grok-home"
export CLAX_HOME="$SCRATCH/clax-home"
export CLAX_BIN="$REPO/target/debug/clax"
export CLAX_NO_OPEN=1
# clax init registers the Chrome extension's native host; keep it in scratch.
export CLAX_NATIVE_HOST_DIRS="chrome=$SCRATCH/browsers/chrome"
unset XDG_CONFIG_HOME
unset GROK_SESSION_ID GROK_HOOK_EVENT GROK_PLUGIN_ROOT CLAUDE_PID CLAUDE_CODE_SESSION_ID \
    CLAUDE_PLUGIN_ROOT CLAUDE_PROJECT_DIR CLAUDE_CONFIG_DIR CLAX_SESSION_ID
CWD="$SCRATCH/cwd"
COPIED_AUTH=""

# shellcheck disable=SC2329 # run by the EXIT trap
cleanup() {
    "$CLAX_BIN" stop >/dev/null 2>&1 || true
    if [ -n "$COPIED_AUTH" ] && [ -f "$GROK_HOME/auth.json" ]; then
        rm -f "$GROK_HOME/auth.json"
        echo "smoke: removed the copied auth.json from $GROK_HOME"
    fi
}
trap cleanup EXIT

rm -rf "$HOME" "$GROK_HOME" "$CLAX_HOME" "$CWD"
mkdir -p "$HOME" "$GROK_HOME" "$CLAX_HOME" "$CWD"

PORT=""
for _ in 1 2 3 4 5; do
    PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')"
    case "$PORT" in 7480 | 7481 | 7490) PORT="" ;; *) break ;; esac
done
[ -n "$PORT" ] || die "no free port other than 7480, 7481 and 7490"
printf '[serve]\nport = %s\n' "$PORT" > "$CLAX_HOME/config.toml"

if [ -z "${XAI_API_KEY:-}" ]; then
    [ -f "$REAL_HOME/.grok/auth.json" ] || die "set XAI_API_KEY, or log in to Grok first (no ~/.grok/auth.json)"
    cp "$REAL_HOME/.grok/auth.json" "$GROK_HOME/auth.json"
    chmod 600 "$GROK_HOME/auth.json"
    COPIED_AUTH=1
    echo "smoke: copied ~/.grok/auth.json into the scratch GROK_HOME (deleted on exit)"
fi

echo "smoke: building clax"
cargo build -q -p clax-cli --bin clax
echo "smoke: scratch root $SCRATCH, Clax on port $PORT"

declare -a RESULTS=()
record() { RESULTS+=("$1|$2|$3"); echo "smoke: $2 $1: $3"; }

base() { echo "http://127.0.0.1:$PORT"; }
token() { python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["token"])' "$CLAX_HOME/daemon.json"; }
api() { # api METHOD PATH [JSON]
    local args=(-s -X "$1" -H "Authorization: Bearer $(token)")
    if [ -n "${3:-}" ]; then args+=(-H 'content-type: application/json' --data "$3"); fi
    curl "${args[@]}" "$(base)$2"
}
grok_p() { # grok_p LOG ARGS...: a headless turn in the scratch project
    local log="$1"; shift
    (cd "$CWD" && grok "$@" --always-approve </dev/null >"$log" 2>&1)
}
grok_sessions() { # the live grok rows, one "<id> <harness_session_id>" per line
    api GET "/api/sessions?live=true" | python3 -c '
import json, sys
for s in json.load(sys.stdin)["sessions"]:
    if s.get("harness") == "grok":
        print(s["id"], s.get("harness_session_id") or "")'
}
claude_rows() {
    api GET "/api/sessions?live=true" | python3 -c '
import json, sys
print(sum(1 for s in json.load(sys.stdin)["sessions"] if s.get("harness") == "claude"))'
}
newest_artifact() {
    api GET /api/artifacts | python3 -c '
import json, sys
a = json.load(sys.stdin)["artifacts"]
print(a[0]["id"] if a else "")'
}
send_comment() { # send_comment AID TEXT: a browser thread, then Send to agent; prints its ID
    local tid
    tid="$(curl -s -X POST "$(base)/api/artifacts/$1/threads" \
        -F 'anchor={"kind":"element","selector":"body","quote":""}' \
        -F "body=$2" -F version=1 \
        | python3 -c 'import json,sys; t=json.load(sys.stdin); print(t.get("thread", t)["id"])')"
    curl -s -X POST "$(base)/api/artifacts/$1/threads/$tid/send" >/dev/null
    echo "$tid"
}
feedback_state() { # feedback_state AID TID: "<state> <tier>"
    curl -s "$(base)/api/artifacts/$1/threads/$2" | python3 -c '
import json, sys
f = json.load(sys.stdin)["thread"]["feedback_state"]
print(f.get("state"), f.get("tier"))'
}

# a: install.
echo "smoke: a: clax init --agent grok"
INIT="$("$CLAX_BIN" init --agent grok --json 2>"$SCRATCH/init.err" || true)"
MARKET="$CLAX_HOME/marketplace"
GROK_STATUS="$(printf '%s' "$INIT" | python3 -c '
import json, sys
try:
    print(next(a["status"] for a in json.load(sys.stdin)["agents"] if a["agent"] == "grok"))
except Exception:
    print("none")')"
LIST="$( (cd "$HOME" && grok plugin list --json) 2>/dev/null || true)"
A_INSTALL=FAIL
if [ "$GROK_STATUS" = registered ] && printf '%s' "$LIST" | grep -q "$MARKET/plugins/clax-grok"; then
    A_INSTALL=PASS
fi
echo "smoke: a: init said $GROK_STATUS; grok plugin list --json: $LIST"

# c, d: a session registers, the hooks run, a sent comment blocks Stop once.
echo "smoke: c: one headless turn"
grok_p "$SCRATCH/turn1.log" -p "Publish a one-line page titled Smoke with clax, then stop." || true
SESSIONS="$(grok_sessions 2>/dev/null || true)"
HSID="$(printf '%s\n' "$SESSIONS" | awk 'NF == 2 { print $2; exit }')"
if [ -n "$HSID" ] && grep -q ' hook agent=grok event=session-start ' "$CLAX_HOME/logs/hooks.log" 2>/dev/null; then
    record c PASS "grok session $HSID; hooks.log has agent=grok session-start"
else
    record c FAIL "grok rows: [${SESSIONS//$'\n'/; }]; see $SCRATCH/turn1.log and $CLAX_HOME/logs/hooks.log (if it fails, change the variable names in host.rs and the wrapper)"
fi

AID="$(newest_artifact 2>/dev/null || true)"
if [ -n "$HSID" ] && [ -n "$AID" ]; then
    COMMENT="Smoke comment $(date +%s): make the heading bigger."
    TID="$(send_comment "$AID" "$COMMENT")"
    echo "smoke: d: sent thread $TID on $AID; resuming $HSID"
    grok_p "$SCRATCH/turn2.log" -p --resume "$HSID" "Say done." || true
    STATE="$(feedback_state "$AID" "$TID")"
    if grep -rqF "$COMMENT" "$GROK_HOME/sessions/" 2>/dev/null && [ "${STATE#* }" = stop_hook ]; then
        record d PASS "the comment reached the turn; feedback is $STATE"
    else
        record d FAIL "feedback is $STATE; transcript has the comment: $(grep -rqF "$COMMENT" "$GROK_HOME/sessions/" 2>/dev/null && echo yes || echo no) (if it fails, change the Stop filter)"
    fi
else
    record d FAIL "no grok session or no artifact from c"
fi

# e: the Claude Code copy enabled as well.
echo "smoke: e: enabling the Claude Code copy from a scratch ~/.claude/plugins"
mkdir -p "$HOME/.claude/plugins"
cat > "$HOME/.claude/plugins/installed_plugins.json" <<JSON
{"version": 2, "plugins": {"clax@clax": [{"scope": "user", "installPath": "$MARKET/plugins/claude-code", "version": "local"}]}}
JSON
(cd "$HOME" && grok plugin enable clax) >"$SCRATCH/enable.log" 2>&1 || true
BEFORE="$(grok_sessions 2>/dev/null | awk '{ print $1 }' | sort)"
grok_p "$SCRATCH/turn3.log" -p "List the names of every tool you have whose name starts with clax, one per line, then publish a one-line page titled Smoke2 with clax." || true
AFTER="$(grok_sessions 2>/dev/null | awk '{ print $1 }' | sort)"
NEW="$(comm -13 <(printf '%s\n' "$BEFORE") <(printf '%s\n' "$AFTER") | grep -c . || true)"
CLAUDE="$(claude_rows 2>/dev/null || echo '?')"
if [ "$NEW" = 1 ] && [ "$CLAUDE" = 0 ] && grep -q ' standdown ' "$CLAX_HOME/logs/hooks.log" 2>/dev/null; then
    record e PASS "one new grok row, no claude row, standdown lines in hooks.log"
else
    record e FAIL "new grok rows: $NEW, claude rows: $CLAUDE; see $SCRATCH/turn3.log and $SCRATCH/enable.log (if both servers do not load, rename the server)"
fi
echo "smoke: e: the model listed these clax tools (both clax__status and clax_grok__publish expected):"
grep -Eo 'clax[a-z_]*__[a-z_]+' "$SCRATCH/turn3.log" | sort -u | sed 's/^/smoke:   /' || echo "smoke:   (none)"

# m: the monitor wakes an idle TUI (interactive).
cat <<TXT

smoke: m: in another terminal, run with this environment:

  export HOME='$HOME' GROK_HOME='$GROK_HOME' CLAX_HOME='$CLAX_HOME' CLAX_BIN='$CLAX_BIN' CLAX_NO_OPEN=1
  cd '$CWD' && grok

Ask it to publish a one-line page titled Monitor with clax and to follow the
skill's "Live feedback in Grok" step (it starts \`clax feedback follow\` with
the monitor tool). Leave the session idle, then press Enter here.
TXT
read -r _
MAID="$(newest_artifact 2>/dev/null || true)"
if [ -n "$MAID" ]; then
    MTID="$(send_comment "$MAID" "Monitor smoke: say hello in the page.")"
    echo "smoke: m: sent thread $MTID on $MAID"
    printf 'smoke: m: did the idle TUI wake with "[clax] New comment on ..." and read the thread? [y/n] '
    read -r yn
    case "$yn" in
        y | Y) record m PASS "the idle TUI woke on the monitor's line" ;;
        *) record m FAIL "the TUI did not wake (if lines are lost while busy, have the skill call comments_read after each wake)" ;;
    esac
else
    record m FAIL "no artifact to comment on"
fi

# a: uninstall.
"$CLAX_BIN" stop >/dev/null 2>&1 || true
"$CLAX_BIN" uninit --agent grok --json >"$SCRATCH/uninit.json" 2>&1 || true
LIST2="$( (cd "$HOME" && grok plugin list --json) 2>/dev/null || true)"
if [ "$A_INSTALL" = PASS ] && ! printf '%s' "$LIST2" | grep -q clax-grok; then
    record a PASS "init installed clax-grok from $MARKET and uninit removed it"
else
    record a FAIL "init: $GROK_STATUS (install $A_INSTALL); after uninit grok lists: $LIST2 (adjust grok_additions, grok_removals or grok_uses)"
fi

# v: the version.
VERSION="$(grok --version 2>/dev/null | head -1 || true)"
if [ -n "$VERSION" ]; then record v PASS "$VERSION"; else record v FAIL "grok --version printed nothing"; fi

echo
echo "smoke: results"
printf '  %-2s %-4s %s\n' check res detail
FAILED=0
for r in "${RESULTS[@]}"; do
    IFS='|' read -r c res detail <<<"$r"
    printf '  %-2s %-4s %s\n' "$c" "$res" "$detail"
    [ "$res" = PASS ] || FAILED=1
done
echo
echo "smoke: for docs/contract.md: Grok Build ${VERSION:-<version>} run live on $(date +%Y-%m-%d) (scripts/smoke-grok.sh)"
exit "$FAILED"
