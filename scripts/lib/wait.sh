# shellcheck shell=bash
# The test scripts' wait for a condition. Source it: `. scripts/lib/wait.sh`.

# wait_for <what> <command> [<argument>...]: runs the command until it
# succeeds, every 0.05 s, and returns 0 then. A hang guard ends a wait for a
# condition that never comes: after WAIT_FOR_POLLS tries (default 2400, about
# 120 s on an idle machine, longer on a loaded one) it prints "timed out
# waiting for <what>" on standard error and returns 1. The guard only ends a
# test that is stuck; it never decides one that passes.
wait_for() {
    local what=$1 tries=0
    shift
    until "$@"; do
        tries=$((tries + 1))
        if [ "$tries" -ge "${WAIT_FOR_POLLS:-2400}" ]; then
            echo "timed out waiting for $what (after $tries tries of: $*)" >&2
            return 1
        fi
        sleep 0.05
    done
}

# gone <pid>: whether no process has that PID (a child of this shell counts as
# gone once the shell has reaped it).
gone() {
    ! kill -0 "$1" 2>/dev/null
}
