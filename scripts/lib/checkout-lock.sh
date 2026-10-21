# shellcheck shell=bash
# Sourced, not run: one holder at a time for a lock file.
#
#   source scripts/lib/checkout-lock.sh
#   checkout_lock "$(git rev-parse --git-dir)/some.flock" "what holds it"
#
# checkout_lock PATH WHAT: returns once this shell holds the lock at PATH,
# saying once on stderr, while it waits, that it is waiting for WHAT. The
# lock is a flock(2) on PATH held by a perl helper that watches this shell
# ($$) and exits when it does, however it exits (a kill -9 included), so
# the kernel lets the lock go with its holder: no lock outlives its run, and
# taking one is a single kernel call, so two waiters never both take it.
# The helper also gives up when this shell exits while still waiting.
#
# PATH is never removed: a waiter may already have it open, and removing
# it would let the next run lock a new file beside that waiter.
#
# Once the lock is held, the helper's pipe is closed in this shell, so what
# the caller starts afterwards (daemons included) cannot keep it, and the
# helper lets go of this shell's stderr, so a reader of that stream sees it
# end with the shell.
checkout_lock() {
    local line=""
    exec 9< <(exec perl -MFcntl=:flock -e '
        my ($path, $what, $owner) = @ARGV;
        open(my $fh, ">>", $path) or die "cannot open $path: $!\n";
        my $said = 0;
        until (flock($fh, LOCK_EX | LOCK_NB)) {
            print STDERR "waiting for $what (lock $path)\n" unless $said++;
            exit 1 unless kill 0, $owner;
            select(undef, undef, undef, 0.2);
        }
        $| = 1;
        print "locked\n";
        close STDOUT;
        open(STDERR, ">", "/dev/null");
        select(undef, undef, undef, 0.2) while kill 0, $owner;
    ' "$1" "$2" "$$" < /dev/null)
    IFS= read -r line <&9 || true
    exec 9<&-
    [ "$line" = locked ]
}
