#!/usr/bin/env bash
# Runs the daemon and the web bundlers with auto-reload. Extra args go to `artifax serve`.
set -euo pipefail
cd "$(dirname "$0")/.."

PORT=7480
ARGS="$*"

if ! cargo watch --version >/dev/null 2>&1; then
    echo "cargo-watch is required: cargo install cargo-watch" >&2
    exit 1
fi
if curl -fsS "http://localhost:$PORT/healthz" >/dev/null 2>&1; then
    echo "a daemon is already listening on $PORT; run \`just stop\` first" >&2
    exit 1
fi
if [ ! -d web/node_modules ]; then
    (cd web && npm ci)
fi

cleanup() {
    trap - EXIT INT TERM HUP
    local pid
    for pid in $(jobs -p); do
        pkill -P "$pid" 2>/dev/null || true
        kill "$pid" 2>/dev/null || true
    done
    if [ -x target/debug/artifax ]; then
        target/debug/artifax stop >/dev/null 2>&1 || true
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
            echo "Artifax dev: http://localhost:$PORT (backend restarts on Rust changes; reload the browser for frontend changes)"
            exit 0
        fi
        sleep 1
    done
) &

# ARGS is deliberately unquoted so it splits into separate serve flags.
# Run in the background and wait so signals interrupt the wait and fire the trap.
cargo watch -q -w crates -w Cargo.toml -w Cargo.lock \
    -x "run -q -p artifax-cli -- serve --foreground --port $PORT $ARGS" &
watch_pid=$!
while :; do
    wait "$watch_pid" && break
    [ $? -gt 128 ] || break
done
