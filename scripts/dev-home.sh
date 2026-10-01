# Sourced by scripts/dev.sh, scripts/watch.sh and scripts/test-dev.sh.
# shellcheck shell=bash disable=SC2034 # DEV_* are read by the sourcing script

# watch_settings [--shared] [args...]: sets DEV_HOME, DEV_PORT and DEV_ARGS
# (the remaining arguments, for `clax serve`). Without --shared: $CLAX_HOME,
# else ~/.clax-dev, on $CLAX_DEV_PORT, else 7481. With --shared: $CLAX_HOME,
# else ~/.clax (the home agents use), on 7480.
watch_settings() {
    local shared="" a
    DEV_ARGS=()
    for a in "$@"; do
        if [ "$a" = --shared ]; then shared=1; else DEV_ARGS+=("$a"); fi
    done
    if [ -n "$shared" ]; then
        DEV_HOME="${CLAX_HOME:-$HOME/.clax}"
        DEV_PORT=7480
    else
        DEV_HOME="${CLAX_HOME:-$HOME/.clax-dev}"
        DEV_PORT="${CLAX_DEV_PORT:-7481}"
    fi
}

# ensure_dev_home <home> <port>: creates the home (0700) and, when its
# config.toml has no [serve] table, records <port> there, so every daemon
# started for this home listens on it. The agents' home (~/.clax) is left as
# it is: its daemon keeps the default port.
ensure_dev_home() {
    mkdir -p "$1"
    [ "$(cd "$1" && pwd -P)" != "$(cd "$HOME" && pwd -P)/.clax" ] || return 0
    chmod 700 "$1"
    if ! grep -q '^\[serve\]' "$1/config.toml" 2>/dev/null; then
        printf '\n[serve]\nport = %s\n' "$2" >> "$1/config.toml"
    fi
}

# stop_orphan_daemon <home>: stops the home's daemon when the executable it
# recorded no longer exists (an earlier `just dev`, whose temporary directory
# is gone). A daemon whose executable exists, such as `just watch`'s, is left
# alone.
stop_orphan_daemon() {
    local exe pid
    [ -f "$1/daemon.json" ] || return 0
    exe="$(sed -n 's/.*"exe"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$1/daemon.json" | head -1 || true)"
    pid="$(sed -n 's/.*"pid"[[:space:]]*:[[:space:]]*\([0-9][0-9]*\).*/\1/p' "$1/daemon.json" | head -1 || true)"
    if [ -n "$exe" ] && [ ! -e "$exe" ] && [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
        echo "clax dev: stopping the daemon of $1 (pid $pid): its binary $exe is gone"
        kill "$pid"
    fi
}
