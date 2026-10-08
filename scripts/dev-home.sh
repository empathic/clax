# Sourced by scripts/dev.sh, scripts/watch.sh, the justfile's install and
# uninstall recipes, and scripts/test-dev.sh.
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
    ! is_agents_home "$1" || return 0
    chmod 700 "$1"
    if ! grep -q '^\[serve\]' "$1/config.toml" 2>/dev/null; then
        printf '\n[serve]\nport = %s\n' "$2" >> "$1/config.toml"
    fi
}

# is_agents_home <home>: true when <home> is ~/.clax, the home agents use.
is_agents_home() {
    [ -d "$1" ] && [ "$(cd "$1" && pwd -P)" = "$(cd "$HOME" && pwd -P)/.clax" ]
}

# daemon_record <home>: sets DAEMON_EXE and DAEMON_PID from <home>/daemon.json,
# each empty when the file or the field is missing.
daemon_record() {
    DAEMON_EXE="" DAEMON_PID=""
    [ -f "$1/daemon.json" ] || return 0
    DAEMON_EXE="$(sed -n 's/.*"exe"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$1/daemon.json" | sed -n 1p || true)"
    DAEMON_PID="$(sed -n 's/.*"pid"[[:space:]]*:[[:space:]]*\([0-9][0-9]*\).*/\1/p' "$1/daemon.json" | sed -n 1p || true)"
}

# wait_gone <pid>: waits up to 5 s for <pid> to exit; false if it has not.
wait_gone() {
    local _
    for _ in $(seq 1 50); do
        kill -0 "$1" 2>/dev/null || return 0
        sleep 0.1
    done
    ! kill -0 "$1" 2>/dev/null
}

# stop_orphan_daemon <home>: stops the home's daemon when the executable it
# recorded no longer exists (an earlier `just dev`, whose temporary directory
# is gone), and waits for it to exit, so its port is free. A daemon whose
# executable exists, such as `just watch`'s, is left alone, and so is
# anything in ~/.clax, the agents' home.
stop_orphan_daemon() {
    ! is_agents_home "$1" || return 0
    daemon_record "$1"
    # The PID must still be that daemon: a reused PID belongs to some other
    # program, whose command line will not name the recorded binary.
    if [ -n "$DAEMON_EXE" ] && [ ! -e "$DAEMON_EXE" ] && [ -n "$DAEMON_PID" ] && kill -0 "$DAEMON_PID" 2>/dev/null \
        && ps -o command= -p "$DAEMON_PID" 2>/dev/null | grep -F -- "$DAEMON_EXE" >/dev/null; then
        echo "clax: stopping the daemon of $1 (pid $DAEMON_PID): its binary $DAEMON_EXE is gone (an earlier \`just dev\`)"
        kill "$DAEMON_PID" 2>/dev/null || true
        wait_gone "$DAEMON_PID" || echo "clax: pid $DAEMON_PID has not exited yet" >&2
    fi
    return 0
}

# stop_installed_daemon <home> <clax>: run by `just install` after it has
# replaced <clax>, and by `just uninstall` before it removes it. When <home>'s
# daemon runs from <clax> (the canonical path daemon.json records), it is
# stopped with `<clax> stop`, so the next agent call starts the build now at
# <clax>. Clients keep a daemon of their own version, so without this a
# same-version reinstall would leave the old build running. A daemon of any
# other executable, such as `just watch --shared`'s, is left running and
# named.
stop_installed_daemon() {
    local want
    daemon_record "$1"
    if [ -z "$DAEMON_PID" ] || ! kill -0 "$DAEMON_PID" 2>/dev/null; then
        return 0
    fi
    want="$(realpath "$2" 2>/dev/null || printf '%s\n' "$2")"
    if [ -n "$DAEMON_EXE" ] && [ "$DAEMON_EXE" = "$want" ] && [ -x "$2" ]; then
        echo "clax: stopping the daemon of $1 (pid $DAEMON_PID), which runs $2; the next agent call starts it again from the new build"
        CLAX_HOME="$1" "$2" stop || echo "warning: could not stop it; run: CLAX_HOME=$1 $2 stop" >&2
    else
        echo "clax: the daemon of $1 (pid $DAEMON_PID) runs ${DAEMON_EXE:-an unrecorded executable}, not $2; left running"
    fi
    return 0
}
