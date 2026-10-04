#!/usr/bin/env bash
# Try rooms and sample() by hand. Starts a daemon in a fresh scratch CLAX_HOME
# on an ephemeral port (Codex push off), publishes the room and sample demo
# pages (web/e2e/pages/room.html and sample.html) with their capabilities,
# prints both URLs, and waits; Ctrl-C stops the daemon and removes its home.
# sample() uses the stub provider, which answers "echo: <prompt>", unless
# ANTHROPIC_API_KEY is set: then [sample] keeps its defaults and every call
# the page makes spends that key. It never uses ~/.clax or ~/.clax-dev, and it
# never builds the web UI (a build rewrites web/dist, which `just dev` and the
# quality gates serve): when web/dist is older than its sources it stops and
# says so.
#
# Usage: scripts/demo-room-sample.sh
set -euo pipefail
cd "$(dirname "$0")/.."
REPO="$PWD"
TMPROOT="${TMPDIR:-/tmp}"
SCRATCH="$(mktemp -d "${TMPROOT%/}/clax-demo-room-sample.XXXXXX")"
export CLAX_HOME="$SCRATCH/home"
export CLAX_CODEX_BIN=
export CLAX_NO_OPEN=1
BIN="$REPO/target/debug/clax"

die() { echo "demo: $1" >&2; exit 1; }
DAEMON_PID=""
cleanup() {
    if [ -n "$DAEMON_PID" ]; then kill "$DAEMON_PID" 2>/dev/null || true; wait "$DAEMON_PID" 2>/dev/null || true; fi
    rm -rf "$SCRATCH"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

# The daemon serves web/dist; it must be built from the current sources.
# scripts/build-web.sh keeps the times of output it did not change and marks
# the build's time in web/node_modules/.clax-web-built.
for out in web/dist/index.html web/dist/_clax/bridge.js; do
    [ -f "$out" ] || die "$out is missing: run (cd web && npm run build) first"
    built="$out"
    if [ web/node_modules/.clax-web-built -nt "$out" ]; then built=web/node_modules/.clax-web-built; fi
    newer="$(find web/shell web/bridge web/vite.shell.config.ts web/vite.bridge.config.ts web/package.json web/package-lock.json \
        -type f ! -name '*.test.ts' -newer "$built" -print -quit)"
    [ -z "$newer" ] || die "web/dist is older than $newer: run (cd web && npm run build) first"
done

mkdir -p "$CLAX_HOME"
if [ -n "${ANTHROPIC_API_KEY:-}" ]; then
    echo "demo: ANTHROPIC_API_KEY is set: sample() calls from the demo page spend that key"
else
    printf '[sample]\nprovider = "stub"\n' >"$CLAX_HOME/config.toml"
    echo "demo: sample() uses the stub provider (set ANTHROPIC_API_KEY to use a real key)"
fi

echo "demo: building clax"
cargo build -q -p clax-cli
"$BIN" serve --foreground --bind 127.0.0.1 --port 0 >"$SCRATCH/daemon.log" 2>&1 &
DAEMON_PID=$!
for _ in $(seq 1 100); do [ -f "$CLAX_HOME/daemon.json" ] && break; sleep 0.1; done
[ -f "$CLAX_HOME/daemon.json" ] || die "the daemon did not start: $(cat "$SCRATCH/daemon.log")"

read -r PORT TOKEN < <(python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print(d["port"], d["token"])' "$CLAX_HOME/daemon.json")
BASE="http://127.0.0.1:$PORT"
for _ in $(seq 1 50); do curl -sf "$BASE/healthz" >/dev/null && break; sleep 0.1; done

publish_page() { # title, capabilities JSON, page file; prints the artifact's URL
    python3 -c 'import json,sys; print(json.dumps({"title": sys.argv[1], "capabilities": json.loads(sys.argv[2]), "files": {"index.html": {"content": open(sys.argv[3]).read(), "encoding": "utf8"}}}))' "$1" "$2" "$3" \
        | curl -sf -X POST "$BASE/api/artifacts" -H "authorization: Bearer $TOKEN" -H 'content-type: application/json' --data-binary @- \
        | python3 -c 'import json,sys; print(json.load(sys.stdin)["url"])' | sed "s|^/|$BASE/|"
}
ROOM_URL="$(publish_page "Room demo" '{"room": {"topics": {"reaction": "interact"}}}' web/e2e/pages/room.html)" \
    || die "could not publish the room page"
SAMPLE_URL="$(publish_page "Sample demo" '{"sample": {}}' web/e2e/pages/sample.html)" \
    || die "could not publish the sample page"

echo
echo "Room:   $ROOM_URL   (open it in two tabs)"
echo "Sample: $SAMPLE_URL"
echo
echo "Ctrl-C stops the daemon and removes its home."
wait "$DAEMON_PID"
