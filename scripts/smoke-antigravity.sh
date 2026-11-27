#!/usr/bin/env bash
# Manual end-to-end check of the Antigravity path. Not a quality gate: it
# runs a real `agy -p`, which calls a model on the signed-in Google account
# (~/.gemini). The owner runs it.
#
# Clax runs in a scratch root (mktemp -d, or the directory given): a
# CLAX_HOME on a free port whose daemon this script starts and stops by its
# PID, and a fresh git workspace for agy to run in. CLAX_BIN is a kept copy
# of this working tree's debug build. ~/.clax and the daemon on 7480 are
# never touched.
#
# Antigravity itself is the person's own: `clax init --agent antigravity`
# installs the plugin into ~/.gemini/config/plugins/ with `agy plugin
# install`, so the script refuses to start when a plugin named
# clax-antigravity or clax is already installed, and on every exit removes
# what it installed: both plugins, and the paths under ~/.gemini that did
# not exist before the run. It passes no --yes to init, and its agy runs
# carry the permission rule for that run only (--settings), so
# ~/.gemini/antigravity-cli/settings.json is never written; its SHA-256 is
# checked before and after.
#
# Checks (PASS/FAIL each):
#   1  clax init --agent antigravity --json registers the plugin, and the
#      permission offer adds nothing (no terminal, no --yes)
#   2  agy plugin list --json lists clax-antigravity; clax doctor --agent
#      antigravity has plugin, skill and antigravity ok
#   3  one headless run publishes a page through the Clax tools; the page is
#      in the daemon; an antigravity session was registered with the run's
#      conversation ID; the run exited before --print-timeout; hooks.log has
#      agent=antigravity event=stop
#   4  a comment sent to that session while its wait_for_feedback is open
#      is delivered, and the agent replies in the thread
#   5  clax uninit removes the plugin; agy no longer lists it; settings.json
#      is unchanged
# Then it prints the interactive steps it cannot run.
#
# Usage:
#   scripts/smoke-antigravity.sh [scratch-dir]
#   scripts/smoke-antigravity.sh --keep [scratch-dir]   checks 1-4, then leaves
#       the plugin installed and the daemon running for the interactive steps
#       (it cleans up as usual if a step fails)
#   scripts/smoke-antigravity.sh --cleanup <scratch-dir>   what --keep left:
#       check 5 and the cleanup
set -euo pipefail
cd "$(dirname "$0")/.."
REPO="$PWD"
# shellcheck source=scripts/lib/wait.sh
. scripts/lib/wait.sh

MODE=run
case "${1:-}" in
    --keep) MODE=keep; shift ;;
    --cleanup) MODE=cleanup; shift ;;
esac

die() { echo "smoke: FAIL: $1" >&2; exit 1; }
command -v agy >/dev/null || die "agy is not on PATH"
command -v python3 >/dev/null || die "python3 is needed"

# Nothing of the harness this script may be started from reaches agy, its
# MCP server or its hooks.
while read -r v; do unset "$v"; done < <(compgen -e | grep -E '^(CLAUDE|CLAX_|ANTIGRAVITY_|GROK_|CODEX_|PI_CODING|PLUGIN_)' || true)

GEMINI="$HOME/.gemini"
SETTINGS_FILE="$GEMINI/antigravity-cli/settings.json"
CONFIG_DIR="$GEMINI/config"
PLUGINS_DIR="$CONFIG_DIR/plugins"
MANIFEST="$CONFIG_DIR/import_manifest.json"
MCP_DIR="$GEMINI/antigravity-cli/mcp"
DATA_DIR="$GEMINI/antigravity-cli/plugin_data"
MODEL=gemini-3.8-flash-low

if [ "$MODE" = cleanup ]; then
    [ -n "${1:-}" ] || die "--cleanup needs the scratch directory --keep printed"
    SCRATCH="$(cd "$1" && pwd -P)"
    [ -f "$SCRATCH/state.env" ] || die "no state.env in $SCRATCH"
    # shellcheck disable=SC1091
    . "$SCRATCH/state.env"
else
    SCRATCH="${1:-$(mktemp -d "${TMPDIR:-/tmp}/clax-smoke-antigravity.XXXXXX")}"
    SCRATCH="$(mkdir -p "$SCRATCH" && cd "$SCRATCH" && pwd -P)"
fi
CLAX_HOME="$SCRATCH/clax-home"
WS="$SCRATCH/ws"
export CLAX_HOME CLAX_NO_OPEN=1
# clax init registers the Chrome extension's native host; keep it in scratch.
export CLAX_NATIVE_HOST_DIRS="chrome=$SCRATCH/browsers/chrome"
unset XDG_CONFIG_HOME

sha() { if [ -f "$1" ]; then shasum -a 256 "$1" | awk '{ print $1 }'; else echo absent; fi; }
exists() { if [ -e "$1" ]; then echo 1; else echo 0; fi; }
# The installed plugins' names, one per line; fails when agy cannot list
# them or prints anything but a JSON list of objects with a name.
list_plugins() {
    local out
    out="$(agy plugin list --json 2>/dev/null)" || return 1
    printf '%s' "$out" | python3 -c '
import json, sys
try:
    plugins = json.load(sys.stdin)
    names = [p["name"] for p in (plugins or [])]
    assert isinstance(plugins, (list, type(None))) and all(isinstance(n, str) for n in names)
except Exception:
    sys.exit(1)
for n in names:
    print(n)'
}
plugin_names() { list_plugins || echo "smoke: agy plugin list --json failed or did not parse" >&2; }
# shellcheck disable=SC2329 # run by wait_for
clax_serve() { # clax_serve PID: whether PID is alive and runs clax serve
    case "$(ps -o command= -p "$1" 2>/dev/null)" in *clax*serve*) return 0 ;; *) return 1 ;; esac
}
# shellcheck disable=SC2329 # run by wait_for
not_serving() { ! clax_serve "$1"; }

declare -a RESULTS=()
record() { RESULTS+=("$1|$2|$3"); echo "smoke: $2 $1: $3"; }

# Stops the headless agy (AGY_PID) while it runs.
stop_agy() {
    if [ -n "${AGY_PID:-}" ] && ! gone "$AGY_PID"; then
        kill "$AGY_PID" 2>/dev/null || true
        if ! WAIT_FOR_POLLS=200 wait_for "the headless agy to exit" gone "$AGY_PID"; then
            kill -KILL "$AGY_PID" 2>/dev/null || true
        fi
        echo "smoke: stopped the headless agy (pid $AGY_PID)"
    fi
    AGY_PID=""
}

# Removes what this run added to ~/.gemini and stops the daemon it started.
# Each path goes only when it did not exist before the run, and a directory
# only when it is empty, so nothing the person put there is removed.
# shellcheck disable=SC2329 # run by the EXIT trap, or by --cleanup
cleanup() {
    # The headless agy first: its shim would start a daemon again, and it
    # must not outlive the plugin it runs from.
    stop_agy
    local names
    names="$(plugin_names)"
    if printf '%s\n' "$names" | grep -x clax-antigravity >/dev/null; then
        agy plugin uninstall clax-antigravity >/dev/null 2>&1 || echo "smoke: could not uninstall clax-antigravity" >&2
    fi
    # The interactive steps install the Claude Code copy as `clax`; the
    # script refused to start while a plugin of that name was installed.
    if printf '%s\n' "$names" | grep -x clax >/dev/null; then
        agy plugin uninstall clax >/dev/null 2>&1 || echo "smoke: could not uninstall clax" >&2
    fi
    local d
    for d in "$MCP_DIR/clax-antigravity_clax:${PRE_MCP_OURS:-1}" "$MCP_DIR/clax_clax:${PRE_MCP_COPY:-1}" \
        "$DATA_DIR/clax-antigravity:${PRE_DATA_OURS:-1}" "$DATA_DIR/clax:${PRE_DATA_COPY:-1}"; do
        if [ "${d##*:}" = 0 ]; then rm -rf "${d%:*}"; fi
    done
    if [ "${PRE_MANIFEST:-1}" = 0 ]; then rm -f "$MANIFEST"; fi
    for d in "$PLUGINS_DIR:${PRE_PLUGINS:-1}" "$MCP_DIR:${PRE_MCP:-1}" "$DATA_DIR:${PRE_DATA:-1}"; do
        if [ "${d##*:}" = 0 ] && [ -d "${d%:*}" ]; then
            rmdir "${d%:*}" 2>/dev/null || echo "smoke: left ${d%:*}, which is not empty" >&2
        fi
    done
    if [ -n "${DAEMON_PID:-}" ] && kill -0 "$DAEMON_PID" 2>/dev/null; then
        kill "$DAEMON_PID" 2>/dev/null || true
        if WAIT_FOR_POLLS=200 wait_for "the daemon to exit" gone "$DAEMON_PID"; then
            echo "smoke: stopped the scratch daemon (pid $DAEMON_PID)"
        else
            echo "smoke: the daemon (pid $DAEMON_PID) has not exited; kill $DAEMON_PID" >&2
        fi
    fi
    # A daemon some other client started in the scratch home.
    local pid
    pid="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["pid"])' "$CLAX_HOME/daemon.json" 2>/dev/null || true)"
    if [ -n "$pid" ] && [ "$pid" != "${DAEMON_PID:-}" ] && clax_serve "$pid"; then
        kill "$pid" 2>/dev/null || true
        if WAIT_FOR_POLLS=200 wait_for "the daemon $pid to exit" not_serving "$pid"; then
            echo "smoke: stopped the scratch home's daemon (pid $pid)"
        else
            echo "smoke: the scratch home's daemon (pid $pid) has not exited; kill $pid" >&2
        fi
    fi
}

base() { echo "http://127.0.0.1:$PORT"; }
token() { python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["token"])' "$CLAX_HOME/daemon.json"; }
api() { # api METHOD PATH [CURL ARGS...]: with the scratch home's bearer token
    local method="$1" path="$2"
    shift 2
    curl -s -X "$method" -H "Authorization: Bearer $(token)" "$@" "$(base)$path"
}

# 5: uninstall, and what is left of the run in ~/.gemini.
check_uninstall() {
    "$CLAX_BIN" uninit --agent antigravity --json </dev/null >"$SCRATCH/uninit.json" 2>"$SCRATCH/uninit.err" || true
    local status listed after
    status="$(python3 -c '
import json, sys
try:
    print(next(a["status"] for a in json.load(open(sys.argv[1]))["agents"] if a["agent"] == "antigravity"))
except Exception:
    print("none")' "$SCRATCH/uninit.json")"
    listed="$(plugin_names | grep -cx clax-antigravity || true)"
    after="$(sha "$SETTINGS_FILE")"
    if [ "$status" = removed ] && [ "$listed" = 0 ] && [ "$after" = "$SETTINGS_SHA" ]; then
        record 5 PASS "uninit: $status; agy lists no clax-antigravity; settings.json unchanged ($after)"
    else
        record 5 FAIL "uninit: $status (see $SCRATCH/uninit.json); clax-antigravity listed: $listed; settings.json $SETTINGS_SHA before, $after after"
    fi
}

summary() {
    echo
    echo "smoke: results"
    local failed=0 r c res detail
    for r in "${RESULTS[@]}"; do
        IFS='|' read -r c res detail <<<"$r"
        printf '  %-2s %-4s %s\n' "$c" "$res" "$detail"
        [ "$res" = PASS ] || failed=1
    done
    return "$failed"
}

# A signal ends the script through its EXIT trap.
trap 'exit 130' INT
trap 'exit 143' TERM

if [ "$MODE" = cleanup ]; then
    trap cleanup EXIT
    check_uninstall
    cleanup
    trap - EXIT
    after="$(sha "$SETTINGS_FILE")"
    [ "$after" = "$SETTINGS_SHA" ] || record 5 FAIL "settings.json changed: $SETTINGS_SHA before, $after after"
    summary
    exit $?
fi

# Refuse before touching anything: the cleanup removes plugins by these
# names, so it must know that none is the person's.
NAMES="$(list_plugins)" || die "agy plugin list --json failed or did not print a JSON list, so the script cannot tell whether clax-antigravity or clax is yours"
for n in clax-antigravity clax; do
    if printf '%s\n' "$NAMES" | grep -x "$n" >/dev/null; then
        die "a plugin named $n is already installed in agy; this script would remove it on exit (agy plugin uninstall $n first)"
    fi
    [ ! -e "$PLUGINS_DIR/$n" ] || die "$PLUGINS_DIR/$n exists; this script would replace and remove it (agy plugin uninstall $n, or remove the directory, first)"
done

SETTINGS_SHA="$(sha "$SETTINGS_FILE")"
PRE_PLUGINS="$(exists "$PLUGINS_DIR")"
PRE_MANIFEST="$(exists "$MANIFEST")"
PRE_MCP="$(exists "$MCP_DIR")"
PRE_MCP_OURS="$(exists "$MCP_DIR/clax-antigravity_clax")"
PRE_MCP_COPY="$(exists "$MCP_DIR/clax_clax")"
PRE_DATA="$(exists "$DATA_DIR")"
PRE_DATA_OURS="$(exists "$DATA_DIR/clax-antigravity")"
PRE_DATA_COPY="$(exists "$DATA_DIR/clax")"
DAEMON_PID="" AGY_PID=""
trap cleanup EXIT

echo "smoke: building clax"
BUILT="$(scripts/run.sh bin)"
# A copy kept by content: macOS assesses each new executable once.
CLAX_BIN="$(scripts/stable-bin.sh "$BUILT" "$REPO/target/clax-bin/smoke")"
export CLAX_BIN

rm -rf "$CLAX_HOME" "$WS"
mkdir -p "$CLAX_HOME" "$WS"
git -C "$WS" init -q
PORT=""
for _ in 1 2 3 4 5; do
    PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')"
    case "$PORT" in 7480 | 7481 | 7490) PORT="" ;; *) break ;; esac
done
[ -n "$PORT" ] || die "no free port other than 7480, 7481 and 7490"
printf '[serve]\nport = %s\n' "$PORT" > "$CLAX_HOME/config.toml"

"$CLAX_BIN" serve --foreground --port "$PORT" >"$SCRATCH/daemon.log" 2>&1 &
DAEMON_PID=$!
# shellcheck disable=SC2329 # run by wait_for
daemon_up() {
    gone "$DAEMON_PID" && die "the daemon exited; see $SCRATCH/daemon.log"
    [ -f "$CLAX_HOME/daemon.json" ] && curl -sf "$(base)/healthz" >/dev/null 2>&1
}
wait_for "the daemon on $(base)" daemon_up || die "the daemon did not answer; see $SCRATCH/daemon.log"
echo "smoke: scratch root $SCRATCH, Clax $("$CLAX_BIN" --version) on port $PORT (pid $DAEMON_PID), agy $(agy --version)"

# The permission rule and workspace trust for this script's agy runs only.
python3 - "$SCRATCH/agy-settings.json" "$WS" <<'PY'
import json, sys
json.dump({"trustedWorkspaces": [sys.argv[2]],
           "permissions": {"allow": ["mcp(clax-antigravity_clax/*)"]}},
          open(sys.argv[1], "w"))
PY

cat > "$SCRATCH/state.env" <<EOF
CLAX_BIN='$CLAX_BIN'
PORT='$PORT'
DAEMON_PID='$DAEMON_PID'
SETTINGS_SHA='$SETTINGS_SHA'
PRE_PLUGINS='$PRE_PLUGINS'
PRE_MANIFEST='$PRE_MANIFEST'
PRE_MCP='$PRE_MCP'
PRE_MCP_OURS='$PRE_MCP_OURS'
PRE_MCP_COPY='$PRE_MCP_COPY'
PRE_DATA='$PRE_DATA'
PRE_DATA_OURS='$PRE_DATA_OURS'
PRE_DATA_COPY='$PRE_DATA_COPY'
EOF
# An interactive agy with this run's environment and settings.
cat > "$SCRATCH/agy.sh" <<EOF
#!/usr/bin/env bash
# agy in the smoke's workspace, on its scratch Clax home; arguments pass on.
while read -r v; do unset "\$v"; done < <(compgen -e | grep -E '^(CLAUDE|CLAX_|ANTIGRAVITY_|GROK_|CODEX_|PI_CODING|PLUGIN_)' || true)
export CLAX_HOME='$CLAX_HOME' CLAX_BIN='$CLAX_BIN' CLAX_NO_OPEN=1
cd '$WS' && exec agy --model $MODEL --settings '$SCRATCH/agy-settings.json' "\$@"
EOF
chmod +x "$SCRATCH/agy.sh"

# 1: install.
"$CLAX_BIN" init --agent antigravity --json </dev/null >"$SCRATCH/init.json" 2>"$SCRATCH/init.err" || true
read -r INIT_STATUS PERM_STATUS < <(python3 -c '
import json, sys
try:
    a = next(a for a in json.load(open(sys.argv[1]))["agents"] if a["agent"] == "antigravity")
    print(a.get("status", "none"), (a.get("permissions") or {}).get("status", "none"))
except Exception:
    print("none none")' "$SCRATCH/init.json")
if [ "$INIT_STATUS" = registered ] && [ "$PERM_STATUS" = not_added ] && [ "$(sha "$SETTINGS_FILE")" = "$SETTINGS_SHA" ]; then
    record 1 PASS "init: registered; permissions: not_added; settings.json unchanged"
else
    record 1 FAIL "init: $INIT_STATUS; permissions: $PERM_STATUS; see $SCRATCH/init.json and init.err"
fi

# 2: agy lists the plugin; doctor's checks.
LISTED="$(plugin_names | grep -cx clax-antigravity || true)"
"$CLAX_BIN" doctor --agent antigravity --json </dev/null >"$SCRATCH/doctor.json" 2>/dev/null || true
DOCTOR="$(python3 -c '
import json, sys
try:
    checks = {c["name"]: c for c in json.load(open(sys.argv[1]))["checks"]}
except Exception:
    checks = {}
print(" ".join(n + "=" + str((checks.get(n) or {}).get("ok")) for n in ("plugin", "skill", "antigravity")))' "$SCRATCH/doctor.json")"
if [ "$LISTED" = 1 ] && [ "$DOCTOR" = "plugin=True skill=True antigravity=True" ]; then
    record 2 PASS "agy lists clax-antigravity; doctor: $DOCTOR"
else
    record 2 FAIL "clax-antigravity listed: $LISTED; doctor: $DOCTOR (see $SCRATCH/doctor.json)"
fi

# 3 and 4: one headless run. It publishes and opens a bounded wait; the
# script sends a comment to its session once the wait is open (or, should
# it never read open, once the page exists), and the agent answers it.
PROMPT="Publish a one-line HTML page titled Smoke with the Clax publish tool. Then call wait_for_feedback with timeout_s 120. When it brings a comment, reply to that comment in its thread with the single word received, then end your turn."
PRINT_TIMEOUT=300
(cd "$WS" && exec agy -p "$PROMPT" --output-format json --model "$MODEL" \
    --settings "$SCRATCH/agy-settings.json" --print-timeout "${PRINT_TIMEOUT}s" \
    </dev/null >"$SCRATCH/run1.json" 2>"$SCRATCH/run1.err") &
AGY_PID=$!

AID="" SID="" TID="" PUSH="never read open" POLLS_SINCE_PAGE=0
COMMENT="Smoke comment on $(date -u +%FT%TZ): please confirm you saw this."
# One poll of the headless run: finds its page and session, and sends the
# comment once the session's wait reads open, or after 1200 polls with the
# page if it never does. True once the run has exited.
# shellcheck disable=SC2329 # run by wait_for
headless_poll() {
    gone "$AGY_PID" && return 0
    if [ -z "$AID" ]; then
        AID="$(api GET /api/artifacts | python3 -c '
import json, sys
a = json.load(sys.stdin).get("artifacts") or []
print(a[0]["id"] if a else "")' 2>/dev/null || true)"
    fi
    if [ -n "$AID" ] && [ -z "$SID" ]; then
        SID="$(api GET "/api/sessions?live=true" | python3 -c '
import json, sys
s = [s for s in json.load(sys.stdin)["sessions"] if s.get("harness") == "antigravity"]
print(s[0]["id"] if s else "")' 2>/dev/null || true)"
    fi
    if [ -n "$SID" ] && [ -z "$TID" ]; then
        POLLS_SINCE_PAGE=$((POLLS_SINCE_PAGE + 1))
        local open
        open="$(api GET "/api/sessions/$SID" | python3 -c '
import json, sys
p = json.load(sys.stdin).get("push") or {}
print(json.dumps(p) if p.get("available") else "")' 2>/dev/null || true)"
        if [ -n "$open" ] || [ "$POLLS_SINCE_PAGE" -ge 1200 ]; then
            if [ -n "$open" ]; then PUSH="$open"; fi
            TID="$(api POST "/api/artifacts/$AID/threads" \
                -F 'anchor={"kind":"element","selector":"body","quote":""}' \
                --form-string "body=$COMMENT" -F version=1 \
                | python3 -c 'import json,sys; t=json.load(sys.stdin); print(t.get("thread", t)["id"])')"
            api POST "/api/artifacts/$AID/threads/$TID/send" >/dev/null
            echo "smoke: sent thread $TID on $AID to session $SID (push when sent: $PUSH)"
        fi
    fi
    return 1
}
# The hang guard: --print-timeout ends the run first.
RC=0
if ! WAIT_FOR_POLLS=$((PRINT_TIMEOUT * 40)) wait_for "the headless run" headless_poll; then
    RC=hung
    stop_agy
else
    wait "$AGY_PID" || RC=$?
    # Reaped: the PID may name another process from now on.
    AGY_PID=""
fi

CONV="$(python3 -c '
import json, sys
def find(v):
    if isinstance(v, dict):
        for k in ("conversation_id", "conversationId"):
            if isinstance(v.get(k), str):
                return v[k]
        for x in v.values():
            r = find(x)
            if r: return r
    if isinstance(v, list):
        for x in v:
            r = find(x)
            if r: return r
    return ""
try:
    print(find(json.load(open(sys.argv[1]))))
except Exception:
    print("")' "$SCRATCH/run1.json")"
api GET /api/sessions >"$SCRATCH/sessions.json"
ROWS="$(python3 -c '
import json, sys
s = [s for s in json.load(open(sys.argv[1]))["sessions"] if s.get("harness") == "antigravity"]
print(" ".join(x["id"] + ":" + str(x.get("harness_session_id")) for x in s))' "$SCRATCH/sessions.json")"
PAGE=000
# Version 1 of the page, as stored.
[ -n "$AID" ] && PAGE="$(api GET "/api/artifacts/$AID/versions/1/files/index.html" \
    -o "$SCRATCH/page.html" -w '%{http_code}')"
[ "$PAGE" = 200 ] && ! grep -i smoke "$SCRATCH/page.html" >/dev/null && PAGE="200 without Smoke"
STOP="$(grep -c ' hook agent=antigravity event=stop ' "$CLAX_HOME/logs/hooks.log" 2>/dev/null || true)"
if [ "$RC" = 0 ] && [ "$PAGE" = 200 ] && [ -n "$CONV" ] && [[ " $ROWS " == *":$CONV "* ]] && [ "${STOP:-0}" -gt 0 ]; then
    record 3 PASS "exit 0; page $AID is 200; session ${ROWS} has the run's conversation ID; $STOP stop line(s) in hooks.log"
else
    record 3 FAIL "exit $RC (3: --print-timeout passed); page $AID: $PAGE; conversation_id: ${CONV:-none}; antigravity sessions: [${ROWS}]; stop lines: ${STOP:-0}; see $SCRATCH/run1.json, run1.err"
fi

if [ -n "$TID" ]; then
    read -r STATE AGENT_REPLIES < <(api GET "/api/artifacts/$AID/threads/$TID" | python3 -c '
import json, sys
t = json.load(sys.stdin)["thread"]
f = t.get("feedback_state") or {}
print(f.get("state"), sum(1 for c in t.get("comments", []) if c.get("author_kind") == "agent"))')
    case "$STATE" in
        delivered | acknowledged)
            if [ "$AGENT_REPLIES" -gt 0 ]; then
                record 4 PASS "thread $TID is $STATE and has $AGENT_REPLIES agent reply(ies); push when sent: $PUSH"
            else
                record 4 FAIL "thread $TID is $STATE, but the agent did not reply in it"
            fi
            ;;
        *) record 4 FAIL "thread $TID is ${STATE:-unknown}; push when sent: $PUSH" ;;
    esac
else
    record 4 FAIL "no comment was sent: page ${AID:-none}, session ${SID:-none}"
fi

if [ "$MODE" = keep ]; then
    if summary; then
        trap - EXIT
        cat <<TXT

smoke: --keep: the plugin is installed and the daemon is running (pid $DAEMON_PID, $(base)).
smoke: until --cleanup, any agy session started outside agy.sh loads this unmerged plugin against your own Clax; exit every agy.sh session before --cleanup.
smoke: interactive agy: $SCRATCH/agy.sh   when done: scripts/smoke-antigravity.sh --cleanup $SCRATCH
TXT
        exit 0
    fi
    exit 1
fi

check_uninstall
cleanup
trap - EXIT
AFTER="$(sha "$SETTINGS_FILE")"
[ "$AFTER" = "$SETTINGS_SHA" ] || record 5 FAIL "settings.json changed during cleanup: $SETTINGS_SHA before, $AFTER after"

cat <<TXT

smoke: interactive steps (this script cannot run them). Run
\`scripts/smoke-antigravity.sh --keep\`, then $SCRATCH/agy.sh in a terminal
(or the agy.sh of the scratch directory --keep prints), with the daemon's
web UI at the URL it prints:
  1  First turn: ask it to publish a one-line page with Clax. Its first
     prompt can call the tool (mcp_clax-antigravity_clax_publish).
  2  Wake: ask it to watch the page and wait for comments as the skill
     says, and let it end its turn. Comment on the page in the browser and
     Send to agent: a turn starts with no typing, the agent replies in the
     thread and opens a wait again (push.available is true again).
  3  Leave the wait open for more than ten minutes, then send another
     comment: the conversation wakes with it.
  4  /new with a wait open, prompt the new conversation, then send a
     comment to the first one's page: it is handled in the background.
  5  /exit with a wait open: the sessions end, and run/tool-hook/antigravity-<ID>
     and run/wake-nudge/antigravity-<ID>.json are gone. Resume with
     agy.sh --conversation <ID> and ask for its Clax pages: the first Clax
     call registers a session with the same harness session ID.
  6  The imported Claude Code copy, whose environment the design's section 12
     lists as unobserved: agy plugin install $REPO/plugins/claude-code,
     start a session, call one Clax tool, and check that
     $CLAX_HOME/logs/hooks.log has standdown lines with host=antigravity for
     both mode=mcp and mode=hook, and that one Clax session registered. Then
     agy plugin uninstall clax.
  Finish with scripts/smoke-antigravity.sh --cleanup <scratch-dir>.
TXT

FAILED=0
summary || FAILED=1
echo
echo "smoke: for docs/contract.md: Antigravity's agy $(agy --version) run live on $(date +%Y-%m-%d) (scripts/smoke-antigravity.sh)"
exit "$FAILED"
