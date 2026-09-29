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
if [ ! -d web/node_modules ]; then
    (cd web && npm ci)
fi

# Run in our own process group so the trap can stop every child.
if [ -z "${ARTIFAX_DEV_GROUP:-}" ] && command -v perl >/dev/null 2>&1; then
    export ARTIFAX_DEV_GROUP=1
    exec perl -e 'use POSIX; setpgrp(0, 0); exec @ARGV or die $!' "$0" "$@"
fi

cleanup() {
    trap - EXIT INT TERM
    if [ -x target/debug/artifax ]; then
        target/debug/artifax stop >/dev/null 2>&1 || true
    fi
    kill -- -$$ 2>/dev/null || true
}
trap cleanup EXIT
trap 'exit 130' INT TERM

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
cargo watch -q -c -w crates -x "run -q -p artifax-cli -- serve --foreground --port $PORT $ARGS"
