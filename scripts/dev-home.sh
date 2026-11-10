# Sourced by scripts/dev.sh, scripts/watch.sh, the justfile's install and
# uninstall recipes, and scripts/test-dev.sh.
# shellcheck shell=bash disable=SC2034 # DEV_* are read by the sourcing script

# The ports a linked worktree's dev home takes, the first free one when the
# home is made; the main checkout's is 7481. Tests name others.
DEV_WORKTREE_PORTS="${DEV_WORKTREE_PORTS:-$(seq 7482 7499)}"

# home_port <home>: prints the port <home>'s config.toml records in its
# [serve] table, or nothing.
home_port() {
    [ -f "$1/config.toml" ] || return 0
    sed -n '/^\[serve\]/,/^\[/s/^port[[:space:]]*=[[:space:]]*\([0-9][0-9]*\).*/\1/p' "$1/config.toml" | sed -n 1p
}

# free_dev_port: prints the first of DEV_WORKTREE_PORTS that no dev home
# (~/.clax-dev*) records and nothing on 127.0.0.1 listens on, or nothing.
free_dev_port() {
    local p h taken
    for p in $DEV_WORKTREE_PORTS; do
        taken=""
        for h in "$HOME"/.clax-dev*; do
            [ -d "$h" ] && [ "$(home_port "$h")" = "$p" ] && taken=1
        done
        [ -z "$taken" ] || continue
        (exec 3<>"/dev/tcp/127.0.0.1/$p") 2>/dev/null && continue
        echo "$p"
        return 0
    done
}

# dev_defaults [<checkout>]: sets DEV_DEFAULT_HOME and DEV_DEFAULT_PORT for
# the checkout ($CLAX_DEV_CHECKOUT, which tests set, else the working
# directory, by default). The repository's main
# checkout: ~/.clax-dev on 7481. A linked git worktree: a home of its own,
# ~/.clax-dev-<its directory's name>, so one branch's migrations never
# upgrade the database another branch's build opens, on the port that home
# records, else the first free one (free_dev_port; empty when there is none).
dev_defaults() {
    local root="${1:-${CLAX_DEV_CHECKOUT:-.}}" git_dir common
    git_dir="$(git -C "$root" rev-parse --absolute-git-dir 2>/dev/null || true)"
    common="$(git -C "$root" rev-parse --path-format=absolute --git-common-dir 2>/dev/null || true)"
    if [ -z "$git_dir" ] || [ "$git_dir" = "$common" ]; then
        DEV_DEFAULT_HOME="$HOME/.clax-dev"
        DEV_DEFAULT_PORT=7481
        return 0
    fi
    DEV_DEFAULT_HOME="$HOME/.clax-dev-$(basename "$(git -C "$root" rev-parse --show-toplevel)")"
    DEV_DEFAULT_PORT="$(home_port "$DEV_DEFAULT_HOME")"
    [ -n "$DEV_DEFAULT_PORT" ] || DEV_DEFAULT_PORT="$(free_dev_port)"
}

# watch_settings [--shared] [args...]: sets DEV_HOME, DEV_PORT and DEV_ARGS
# (the remaining arguments, for `clax serve`). Without --shared: $CLAX_HOME
# on $CLAX_DEV_PORT, else the port it records, else 7481; without
# $CLAX_HOME, this checkout's dev home and port (dev_defaults), with
# $CLAX_DEV_PORT still winning. With --shared: $CLAX_HOME, else ~/.clax (the
# home agents use), on 7480. Fails, saying why, when a new worktree home
# finds no free port.
watch_settings() {
    local shared="" a
    DEV_ARGS=()
    for a in "$@"; do
        if [ "$a" = --shared ]; then shared=1; else DEV_ARGS+=("$a"); fi
    done
    if [ -n "$shared" ]; then
        DEV_HOME="${CLAX_HOME:-$HOME/.clax}"
        DEV_PORT=7480
    elif [ -n "${CLAX_HOME:-}" ]; then
        DEV_HOME="$CLAX_HOME"
        DEV_PORT="${CLAX_DEV_PORT:-$(home_port "$CLAX_HOME")}"
        DEV_PORT="${DEV_PORT:-7481}"
    else
        dev_defaults
        DEV_HOME="$DEV_DEFAULT_HOME"
        DEV_PORT="${CLAX_DEV_PORT:-$DEV_DEFAULT_PORT}"
        if [ -z "$DEV_PORT" ]; then
            # shellcheck disable=SC2086 # the list splits into its ports
            set -- $DEV_WORKTREE_PORTS
            echo "clax dev: no free port in $1-${!#} for this worktree's dev home $DEV_HOME: other dev homes (~/.clax-dev*) record them all. Remove the homes of worktrees you no longer use, or name a port: CLAX_DEV_PORT=<port> just ..." >&2
            return 1
        fi
    fi
}

# log_size <home>: prints the size in bytes of <home>'s daemon log (0 when
# there is none), to pass to explain_home_failure.
log_size() {
    if [ -f "$1/logs/daemon.log" ]; then wc -c < "$1/logs/daemon.log" | tr -d ' '; else echo 0; fi
}

# explain_home_failure <home> <from>: run when a daemon could not start on
# <home>. When what its log gained past byte <from> (log_size before the
# attempt) says the database could not be upgraded, says why that happens
# and what to do, and returns 0; otherwise returns 1.
explain_home_failure() {
    local line
    line="$(tail -c "+$(($2 + 1))" "$1/logs/daemon.log" 2>/dev/null | grep "could not upgrade the database" | tail -n 1 | cut -c1-300 || true)"
    [ -n "$line" ] || return 1
    cat >&2 <<EOF

clax dev: this build cannot open the dev home $1:
  $line

Its database was upgraded by a build whose migrations differ from this
checkout's: another branch's build sharing this home, or an earlier build of
this branch whose unmerged migrations have changed since. The database is as
it was. Either:
  - start over: move the home aside (mv "$1" "$1.old") and run this again;
    nothing is deleted, and moving it back restores it;
  - or keep it and use another home: CLAX_HOME=<a new directory> just ...
EOF
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
