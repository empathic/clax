#!/usr/bin/env bash
# Manual end-to-end check of the Claude Code path. Not a quality gate: it runs a
# real `claude -p` session, which calls a model and needs credentials.
#
# Builds artifax, starts a daemon in a scratch ARTIFAX_HOME, has Claude publish a
# page through the MCP shim, then verifies the page and the registered session.
#
# Usage: scripts/smoke-claude.sh [scratch-dir]
set -euo pipefail
cd "$(dirname "$0")/.."
REPO="$PWD"
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

echo "smoke: running claude"
OUT="$(cd "$CWD" && claude -p "Publish a one-line HTML page titled Smoke via the artifax publish tool, then call the artifax status tool. Reply with only the artifact URL." --max-turns 4 --mcp-config "$CONFIG" --strict-mcp-config --allowedTools mcp__artifax__publish mcp__artifax__status </dev/null)" \
    || die "claude exited non-zero; output: $OUT"
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

SESSIONS="$(curl -s "$BASE/api/sessions?live=true")"
printf '%s' "$SESSIONS" | python3 -c '
import json, sys
sessions = json.load(sys.stdin)["sessions"]
sys.exit(0 if any(s.get("harness") == "claude" for s in sessions) else 1)
' || die "no live claude session in /api/sessions?live=true: $SESSIONS"
echo "smoke: live claude session registered"

echo "smoke: OK"
