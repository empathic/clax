#!/usr/bin/env bash
# Runs the daemon and the web bundlers with auto-reload. Extra args go to `clax serve`.
set -euo pipefail
cd "$(dirname "$0")/.."

# By default the dev daemon runs apart from the installed one that agents'
# plugins use: its own home (~/.clax-dev) on port 7481, so rebuilding never
# takes the agents' Clax down and the two never fight over a port.
# `just dev --shared` serves the real home on 7480 instead (stop the installed
# daemon first). A CLAX_HOME you set yourself always wins.
SHARED=0
ARGS=()
for a in "$@"; do
    if [ "$a" = "--shared" ]; then SHARED=1; else ARGS+=("$a"); fi
done
if [ "$SHARED" = 1 ]; then
    PORT=7480
    export CLAX_HOME="${CLAX_HOME:-$HOME/.clax}"
else
    PORT="${CLAX_DEV_PORT:-7481}"
    export CLAX_HOME="${CLAX_HOME:-$HOME/.clax-dev}"
fi
ARGS="${ARGS[*]:-}"
echo "Clax dev: serving CLAX_HOME=$CLAX_HOME on port $PORT"

if ! cargo watch --version >/dev/null 2>&1; then
    echo "cargo-watch is required: cargo install cargo-watch" >&2
    exit 1
fi
command -v curl >/dev/null || { echo "curl is required" >&2; exit 1; }
if curl -fsS "http://localhost:$PORT/healthz" >/dev/null 2>&1; then
    echo "a daemon is already listening on $PORT; stop it first (CLAX_HOME=$CLAX_HOME clax stop)" >&2
    exit 1
fi
if [ ! -d web/node_modules ]; then
    (cd web && npm ci)
fi

cleanup() {
    trap - EXIT
    trap '' INT TERM HUP
    local pid
    for pid in $(jobs -p); do
        pkill -P "$pid" 2>/dev/null || true
        kill "$pid" 2>/dev/null || true
    done
    if [ -x target/debug/clax ]; then
        target/debug/clax stop >/dev/null 2>&1 || true
    fi
}
trap cleanup EXIT
trap 'exit 130' INT TERM HUP

for cfg in bridge shell; do
    (cd web && npx vite build -c "vite.$cfg.config.ts" --watch 2>&1 | sed -u "s/^/[web:$cfg] /") &
done

(
    for _ in $(seq 1 600); do
        if curl -fsS "http://localhost:$PORT/healthz" >/dev/null 2>&1; then
            echo "Clax dev: http://localhost:$PORT (backend restarts on Rust changes; reload the browser for frontend changes)"
            exit 0
        fi
        sleep 1
    done
) &

# ARGS is deliberately unquoted so it splits into separate serve flags.
# Run in the background and wait so signals interrupt the wait and fire the trap.
cargo watch -q -w crates -w Cargo.toml -w Cargo.lock \
    -x "run -q -p clax-cli -- serve --foreground --port $PORT $ARGS" &
watch_pid=$!
while :; do
    rc=0
    wait "$watch_pid" || rc=$?
    [ "$rc" -gt 128 ] || break
done
exit "$rc"
