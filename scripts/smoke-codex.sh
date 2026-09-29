#!/usr/bin/env bash
# Manual end-to-end check of the Codex path. Not a quality gate: it runs a real
# `codex exec` session, which calls a model and needs credentials.
#
# Installs this repository's Codex marketplace and the artifax plugin into a
# scratch CODEX_HOME, has Codex publish a page through the plugin's MCP server,
# then verifies the page and the registered session. The plugin's installer
# script resolves the working tree's target/debug/artifax (ARTIFAX_BIN), and the
# shim starts the daemon itself in a scratch ARTIFAX_HOME; this script starts
# nothing. The person's own ~/.codex is never read or written, except that
# ~/.codex/auth.json is copied into the scratch home for the run (Codex needs
# credentials) and that copy is deleted on exit.
#
# Modes:
#   (default)  features.hooks is unset, so only the MCP server runs; the
#              session is registered by the shim through the parent-PID path.
#   --hooks    also sets features.hooks = true and passes
#              --dangerously-bypass-hook-trust (the scratch home has no
#              persisted hook trust), then asserts the session carries Codex's
#              session ID from the SessionStart hook.
#
# Usage: scripts/smoke-codex.sh [--hooks] [scratch-dir]
set -euo pipefail
cd "$(dirname "$0")/.."
REPO="$PWD"
HOOKS=""
if [ "${1:-}" = --hooks ]; then HOOKS=1; shift; fi
SCRATCH="${1:-${TMPDIR:-/tmp}/artifax-smoke-codex}"
SCRATCH="$(mkdir -p "$SCRATCH" && cd "$SCRATCH" && pwd -P)"
export CODEX_HOME="$SCRATCH/codex-home"
export ARTIFAX_HOME="$SCRATCH/artifax-home"
export ARTIFAX_BIN="$REPO/target/debug/artifax"
export ARTIFAX_NO_OPEN=1
CWD="$SCRATCH/cwd"
REAL_CODEX_HOME="$(cd "$HOME/.codex" 2>/dev/null && pwd -P || true)"

die() { echo "smoke: FAIL: $1" >&2; exit 1; }
cleanup() {
    "$ARTIFAX_BIN" stop >/dev/null 2>&1 || true
    if [ -f "$CODEX_HOME/auth.json" ]; then
        rm -f "$CODEX_HOME/auth.json"
        echo "smoke: removed the copied auth.json from $CODEX_HOME"
    fi
}
trap cleanup EXIT

[ -n "$REAL_CODEX_HOME" ] && [ "$CODEX_HOME" = "$REAL_CODEX_HOME" ] \
    && die "the scratch CODEX_HOME resolves to ~/.codex"
[ -f "$HOME/.codex/auth.json" ] || die "no ~/.codex/auth.json to copy; log in to Codex first"

rm -rf "$CODEX_HOME" "$ARTIFAX_HOME" "$CWD"
mkdir -p "$CODEX_HOME" "$ARTIFAX_HOME" "$CWD"

echo "smoke: building artifax"
cargo build -q -p artifax-cli

echo "smoke: preparing CODEX_HOME=$CODEX_HOME"
{
    # The model is the only setting taken from the person's config.
    grep -E '^model[[:space:]]*=' "$HOME/.codex/config.toml" 2>/dev/null | head -1 || true
    if [ -n "$HOOKS" ]; then printf '\n[features]\nhooks = true\n'; fi
} > "$CODEX_HOME/config.toml"
cp "$HOME/.codex/auth.json" "$CODEX_HOME/auth.json"
chmod 600 "$CODEX_HOME/auth.json"
echo "smoke: copied ~/.codex/auth.json into the scratch home (deleted on exit)"

codex plugin marketplace add "$REPO" >/dev/null
codex plugin add artifax@artifax >/dev/null
codex mcp list | awk 'NR > 1 { print $1 }' | grep -qx artifax || die "codex mcp list does not show artifax"
echo "smoke: plugin installed; codex mcp list shows artifax"
# `codex exec` runs with approval policy "never", which refuses MCP tool calls
# that would prompt, so the scratch home pre-approves the plugin's tools.
printf '\n[plugins."artifax@artifax".mcp_servers.artifax]\ndefault_tools_approval_mode = "approve"\n' >> "$CODEX_HOME/config.toml"

PROMPT="Publish a one-line HTML page titled Smoke via the artifax publish tool and reply with only its URL"
EXTRA=()
if [ -n "$HOOKS" ]; then EXTRA+=(--dangerously-bypass-hook-trust); fi
echo "smoke: running codex exec${HOOKS:+ (hooks enabled)}"
OUT="$(cd "$CWD" && codex exec --skip-git-repo-check -s read-only ${EXTRA[@]+"${EXTRA[@]}"} -o "$SCRATCH/last-message.txt" "$PROMPT" </dev/null 2>"$SCRATCH/codex-stderr.log")" \
    || die "codex exec exited non-zero; see $SCRATCH/codex-stderr.log"
OUT="$(cat "$SCRATCH/last-message.txt" 2>/dev/null || printf '%s' "$OUT")"
echo "smoke: codex replied: $OUT"

URL="$(printf '%s' "$OUT" | grep -Eo 'https?://[^ )>"`]*/a/[a-z0-9]+' | head -1 || true)"
[ -n "$URL" ] || die "no /a/<id> URL in codex's reply"

BASE="$("$ARTIFAX_BIN" status --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["url"].rstrip("/"))')" \
    || die "no daemon running in $ARTIFAX_HOME (the shim should have started one)"
echo "smoke: daemon at $BASE (started by the shim)"

ID="${URL##*/a/}"
# /a/<id> is the gallery shell; the stored page is served at /c/<id>/v/<n>/.
CODE="$(curl -s -o /dev/null -w '%{http_code}' "$BASE/a/$ID")"
[ "$CODE" = 200 ] || die "GET $BASE/a/$ID returned $CODE"
CODE="$(curl -s -o "$SCRATCH/page.html" -w '%{http_code}' "$BASE/c/$ID/v/1/")"
[ "$CODE" = 200 ] || die "GET $BASE/c/$ID/v/1/ returned $CODE"
grep -q Smoke "$SCRATCH/page.html" || die "page $BASE/c/$ID/v/1/ does not contain Smoke"
echo "smoke: $BASE/a/$ID is 200 and its page contains Smoke"

curl -s "$BASE/api/sessions" > "$SCRATCH/sessions.json"
python3 - "$SCRATCH/sessions.json" "$HOOKS" "$CWD" <<'PY' || die "session check failed (see above)"
import json, os, sys
sessions = [s for s in json.load(open(sys.argv[1]))["sessions"] if s.get("harness") == "codex"]
hooks = bool(sys.argv[2])
cwd = os.path.realpath(sys.argv[3])
if not sessions:
    print("smoke: no codex session in /api/sessions"); sys.exit(1)
for s in sessions:
    print(f"smoke: codex session {s['id']}: harness_session_id={s.get('harness_session_id')!r} "
          f"cwd={s.get('cwd')!r} ended_at={s.get('ended_at')!r}")
if not all(s.get("cwd") and os.path.realpath(s["cwd"]) == cwd for s in sessions):
    print(f"smoke: a codex session's cwd is not {cwd}, where codex exec ran"); sys.exit(1)
print(f"smoke: the codex session's cwd is {cwd}, where codex exec ran")
with_id = any(s.get("harness_session_id") for s in sessions)
if hooks and not with_id:
    print("smoke: hooks were enabled but no codex session carries a harness_session_id"); sys.exit(1)
print("smoke: " + ("the SessionStart hook ran (harness_session_id is set)" if with_id
      else "no harness_session_id: hooks did not run (features.hooks is unset)"))
PY

echo "smoke: OK"
