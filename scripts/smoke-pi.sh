#!/usr/bin/env bash
# Manual end-to-end check of the Pi extension. Not a quality gate.
#
# Runs `pi -p "hello"` with only this repository's extension loaded
# (plugins/pi), a scratch PI_CODING_AGENT_DIR, and a scratch ARTIFAX_HOME. The
# extension's session_start handler starts the daemon (through ARTIFAX_BIN,
# the working tree's target/debug/artifax) and registers the Pi session, before
# Pi calls a model. Without a provider API key the model call fails; the
# script reports that and still checks the registration. The person's own
# ~/.pi is never read or written: the script fails if anything under it
# changed during the run.
#
# Pi is $PI when set, else plugins/pi/node_modules/.bin/pi (after `npm ci`
# there), else `npx --yes @mariozechner/pi-coding-agent@0.73.1`.
#
# The daemon listens on the CLI's default port, which must be free.
#
# Usage: scripts/smoke-pi.sh [scratch-dir]
set -euo pipefail
cd "$(dirname "$0")/.."
REPO="$PWD"
SCRATCH="${1:-${TMPDIR:-/tmp}/artifax-smoke-pi}"
SCRATCH="$(mkdir -p "$SCRATCH" && cd "$SCRATCH" && pwd -P)"
export PI_CODING_AGENT_DIR="$SCRATCH/pi-home"
export ARTIFAX_HOME="$SCRATCH/artifax-home"
export ARTIFAX_BIN="$REPO/target/debug/artifax"
export ARTIFAX_NO_OPEN=1
export PI_OFFLINE=1
CWD="$SCRATCH/cwd"
MARKER="$SCRATCH/started"

die() { echo "smoke: FAIL: $1" >&2; exit 1; }
cleanup() { "$ARTIFAX_BIN" stop >/dev/null 2>&1 || true; }
trap cleanup EXIT

REAL_PI_HOME="$(cd "$HOME/.pi/agent" 2>/dev/null && pwd -P || true)"
[ -n "$REAL_PI_HOME" ] && [ "$PI_CODING_AGENT_DIR" = "$REAL_PI_HOME" ] \
    && die "the scratch PI_CODING_AGENT_DIR resolves to ~/.pi/agent"

if [ -n "${PI:-}" ]; then PI_CMD=("$PI")
elif [ -x "$REPO/plugins/pi/node_modules/.bin/pi" ]; then PI_CMD=("$REPO/plugins/pi/node_modules/.bin/pi")
else PI_CMD=(npx --yes @mariozechner/pi-coding-agent@0.73.1)
fi

rm -rf "$PI_CODING_AGENT_DIR" "$ARTIFAX_HOME" "$CWD"
mkdir -p "$PI_CODING_AGENT_DIR" "$ARTIFAX_HOME" "$CWD"
touch "$MARKER"

echo "smoke: building artifax"
cargo build -q -p artifax-cli

echo "smoke: running ${PI_CMD[*]} -p hello (PI_CODING_AGENT_DIR=$PI_CODING_AGENT_DIR, ARTIFAX_HOME=$ARTIFAX_HOME)"
set +e
(cd "$CWD" && "${PI_CMD[@]}" --no-session --no-extensions --no-skills -nc -e "$REPO/plugins/pi" -p "hello") \
    >"$SCRATCH/pi.out" 2>"$SCRATCH/pi.err" </dev/null
PI_STATUS=$?
set -e
echo "smoke: pi exited with status $PI_STATUS"
echo "--- pi stdout"; cat "$SCRATCH/pi.out"
echo "--- pi stderr"; cat "$SCRATCH/pi.err"
echo "---"

if [ -d "$HOME/.pi" ] && [ -n "$(find "$HOME/.pi" -newer "$MARKER" -print -quit)" ]; then
    die "something under ~/.pi changed during the run"
fi

INFO="$ARTIFAX_HOME/daemon.json"
[ -f "$INFO" ] || die "the extension started no daemon ($INFO is missing); see $ARTIFAX_HOME/logs/daemon.log"
PORT="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["port"])' "$INFO")"
SESSIONS="$(curl -sf "http://127.0.0.1:$PORT/api/sessions")" || die "GET /api/sessions failed"
echo "smoke: GET /api/sessions -> $SESSIONS"
python3 - "$SESSIONS" "$CWD" <<'PY' || die "no pi session was registered"
import json, sys
sessions = [s for s in json.loads(sys.argv[1])["sessions"] if s["harness"] == "pi"]
if len(sessions) != 1:
    sys.exit(f"expected one pi session, found {len(sessions)}")
s = sessions[0]
assert s["cwd"] == sys.argv[2], f"cwd {s['cwd']!r} != {sys.argv[2]!r}"
assert s["harness_session_id"], "no harness_session_id"
print(f"smoke: pi session {s['id']} registered (harness_session_id {s['harness_session_id']}, cwd {s['cwd']})")
print("smoke: session " + ("ended at " + s["ended_at"] if s["ended_at"] else "still open (session_shutdown did not end it)"))
PY

if [ "$PI_STATUS" -eq 0 ]; then
    echo "smoke: OK: pi ran to completion and the extension registered its session"
elif grep -qiE "api key|no model|credential|auth" "$SCRATCH/pi.err" "$SCRATCH/pi.out"; then
    echo "smoke: OK (partial): pi failed at the model call for lack of a provider key; the extension registered its session before that"
else
    die "pi exited with status $PI_STATUS for a reason other than a missing provider key; see the output above"
fi
