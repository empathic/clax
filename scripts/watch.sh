#!/usr/bin/env bash
# `just watch`: runs the daemon and the web bundlers with auto-reload. By
# default it serves its own home ($CLAX_HOME, else ~/.clax-dev) on port
# $CLAX_DEV_PORT, else 7481, so rebuilding never takes the agents' daemon
# down; `--shared` serves the agents' home (~/.clax) on 7480 (stop the
# installed daemon first). Other arguments go to `clax serve`. Outside
# --shared, a daemon an earlier `just dev` left in the dev home, whose
# temporary binary is gone, is stopped first; ~/.clax and port 7480 are never
# touched that way.
set -euo pipefail
cd "$(dirname "$0")/.."
. scripts/dev-home.sh
watch_settings "$@"
export CLAX_HOME="$DEV_HOME"
PORT="$DEV_PORT"
ARGS="${DEV_ARGS[*]-}"
if [ "$PORT" = 7480 ]; then
    echo "Clax watch: --shared: serving CLAX_HOME=$CLAX_HOME on port $PORT, the home your agents use. While a Rust change rebuilds, an agent may start its own daemon here."
else
    ensure_dev_home "$CLAX_HOME" "$PORT"
    stop_orphan_daemon "$CLAX_HOME"
    echo "Clax watch: serving CLAX_HOME=$CLAX_HOME on port $PORT (agents keep their own daemon; \`just watch --shared\` serves theirs)"
fi

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
            echo "Clax watch: http://localhost:$PORT (backend restarts on Rust changes; reload the browser for frontend changes)"
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
