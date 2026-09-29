#!/usr/bin/env bash
# Manual end-to-end check of the Claude Code path. Not a quality gate: it runs a
# real `claude -p` session, which calls a model and needs credentials.
#
# Builds artifax, starts a daemon in a scratch ARTIFAX_HOME, has Claude publish a
# page through the MCP shim, then verifies the page and the registered session.
#
# Modes:
#   --plugin-dir  (default when `claude` supports the flag) loads plugins/claude-code
#                 as a plugin, so the installer script, MCP shim, and both hooks run.
#                 Also asserts the session carries a harness session ID and is
#                 ended (SessionEnd) after the run.
#   --mcp-config  points --mcp-config at target/debug/artifax mcp --agent claude.
#
# Usage: scripts/smoke-claude.sh [--plugin-dir|--mcp-config] [scratch-dir]
set -euo pipefail
cd "$(dirname "$0")/.."
REPO="$PWD"
MODE=""
case "${1:-}" in
    --plugin-dir|--mcp-config) MODE="$1"; shift ;;
esac
if [ -z "$MODE" ]; then
    if claude --help 2>&1 | grep -q -- '--plugin-dir'; then MODE=--plugin-dir; else MODE=--mcp-config; fi
fi
SCRATCH="${1:-${TMPDIR:-/tmp}/artifax-smoke-claude}"
SCRATCH="$(mkdir -p "$SCRATCH" && cd "$SCRATCH" && pwd)"
HOME_DIR="$SCRATCH/home"
CWD="$SCRATCH/cwd"
BIN="$REPO/target/debug/artifax"
export ARTIFAX_HOME="$HOME_DIR"
export ARTIFAX_NO_OPEN=1

cleanup() { "$BIN" stop >/dev/null 2>&1 || true; }
trap cleanup EXIT
die() { echo "smoke: FAIL: $1" >&2; exit 1; }

rm -rf "$HOME_DIR" "$CWD"
mkdir -p "$HOME_DIR" "$CWD"

echo "smoke: building artifax"
cargo build -q -p artifax-cli

echo "smoke: starting daemon in $HOME_DIR"
"$BIN" serve --port 0 >/dev/null
BASE="$("$BIN" status --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["url"].rstrip("/"))')"
echo "smoke: daemon at $BASE"

PROMPT="Publish a one-line HTML page titled Smoke via the artifax publish tool, then call the artifax status tool. Reply with only the artifact URL."
TOOLS="mcp__artifax__publish mcp__artifax__status"
# A plugin's MCP server is named plugin_<plugin>_<server>.
PLUGIN_TOOLS="mcp__plugin_artifax_artifax__publish mcp__plugin_artifax_artifax__status"
echo "smoke: running claude ($MODE)"
if [ "$MODE" = --plugin-dir ]; then
    export ARTIFAX_BIN="$BIN"
    OUT="$(cd "$CWD" && claude -p "$PROMPT" --max-turns 4 --plugin-dir "$REPO/plugins/claude-code" --allowedTools $PLUGIN_TOOLS </dev/null)" \
        || die "claude exited non-zero; output: $OUT"
else
    CONFIG="$SCRATCH/mcp.json"
    python3 - "$CONFIG" "$BIN" "$HOME_DIR" <<'PY'
import json, sys
config, binary, home = sys.argv[1:4]
json.dump({"mcpServers": {"artifax": {
    "command": binary,
    "args": ["mcp", "--agent", "claude"],
    "env": {"ARTIFAX_HOME": home, "ARTIFAX_NO_OPEN": "1"},
}}}, open(config, "w"), indent=2)
PY
    OUT="$(cd "$CWD" && claude -p "$PROMPT" --max-turns 4 --mcp-config "$CONFIG" --strict-mcp-config --allowedTools $TOOLS </dev/null)" \
        || die "claude exited non-zero; output: $OUT"
fi
echo "smoke: claude replied: $OUT"

URL="$(printf '%s' "$OUT" | grep -Eo 'https?://[^ )>"]*/a/[a-z0-9]+' | head -1 || true)"
[ -n "$URL" ] || die "no /a/<id> URL in claude's reply"

ID="${URL##*/a/}"
# /a/<id> is the gallery shell; the stored page is served at /c/<id>/v/<n>/.
CODE="$(curl -s -o /dev/null -w '%{http_code}' "$BASE/a/$ID")"
[ "$CODE" = 200 ] || die "GET $BASE/a/$ID returned $CODE"
CODE="$(curl -s -o "$SCRATCH/page.html" -w '%{http_code}' "$BASE/c/$ID/v/1/")"
[ "$CODE" = 200 ] || die "GET $BASE/c/$ID/v/1/ returned $CODE"
grep -q Smoke "$SCRATCH/page.html" || die "page $BASE/c/$ID/v/1/ does not contain Smoke"
echo "smoke: $BASE/a/$ID is 200 and its page contains Smoke"

sessions_check() { # $1: python predicate over `sessions`
    curl -s "$BASE/api/sessions" | python3 -c "
import json, sys
sessions = [s for s in json.load(sys.stdin)['sessions'] if s.get('harness') == 'claude']
sys.exit(0 if ($1) else 1)"
}
sessions_check "sessions" || die "no claude session in /api/sessions"
echo "smoke: claude session registered"
if [ "$MODE" = --plugin-dir ]; then
    sessions_check "any(s.get('harness_session_id') for s in sessions)" \
        || die "no claude session carries a harness_session_id"
    echo "smoke: session carries a harness session ID"
    ended=""
    for _ in $(seq 1 20); do
        if sessions_check "any(s.get('ended_at') for s in sessions)"; then ended=1; break; fi
        sleep 0.5
    done
    [ -n "$ended" ] || die "claude session has no ended_at after the run (SessionEnd)"
    echo "smoke: session ended (SessionEnd hook ran)"
fi

echo "smoke: OK"
