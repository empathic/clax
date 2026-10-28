# shellcheck shell=bash
# Sourced, not run: one holder at a time for a lock file.
#
#   source scripts/lib/checkout-lock.sh
#   checkout_lock "$(git rev-parse --git-dir)/some.flock" "what holds it"
#
# checkout_lock PATH WHAT: returns once this shell holds the lock at PATH,
# saying once on stderr, while it waits, that it is waiting for WHAT. The
# lock is a flock(2) on PATH held by a helper (checkout-lock.py, beside this
# file) that watches this shell ($$) and exits when it does, however it
# exits (a kill -9 included), so the kernel lets the lock go with its
# holder: no lock outlives its run, and taking one is a single kernel call,
# so two waiters never both take it.
# The helper also gives up when this shell exits while still waiting.
#
# PATH is never removed: a waiter may already have it open, and removing
# it would let the next run lock a new file beside that waiter.
#
# Once the lock is held, the helper's pipe is closed in this shell, so what
# the caller starts afterwards (daemons included) cannot keep it, and the
# helper lets go of this shell's stderr, so a reader of that stream sees it
# end with the shell.

# The helper's directory, fixed when this file is sourced, so a caller that
# changes directory afterwards still finds it.
_checkout_lock_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
checkout_lock() {
    local line=""
    exec 9< <(exec python3 "$_checkout_lock_dir/checkout-lock.py" "$1" "$2" "$$" < /dev/null)
    IFS= read -r line <&9 || true
    exec 9<&-
    [ "$line" = locked ]
}
