#!/usr/bin/env bash
# Checks scripts/lib/checkout-lock.sh: a second run waits for the holder and
# goes on when it exits, a holder killed outright lets the lock go, and runs
# contending for the lock hold it one at a time.
#
# Every step waits on a FIFO for the event it checks, never on a timer.
set -euo pipefail
cd "$(dirname "$0")/.."
d="$(mktemp -d)"
pids=""
trap 'kill $pids 2>/dev/null || true; rm -rf "$d"' EXIT
fails=0
ok() { echo "PASS: $1"; }
bad() { echo "FAIL: $1"; fails=1; }
L="$d/lock"

# hold NAME: starts a shell that takes the lock, then prints "NAME held" on
# the FIFO $d/NAME.out and keeps the lock until a line arrives on $d/NAME.in.
# What checkout_lock says while waiting goes to the FIFO $d/NAME.err. Sets
# HOLDER to the shell's PID.
hold() {
    mkfifo "$d/$1.in" "$d/$1.out" "$d/$1.err"
    bash -c '. scripts/lib/checkout-lock.sh
        checkout_lock "$1" "test holder" 2> "$2.err"
        echo "$3 held" > "$2.out"
        read -r _ < "$2.in"' _ "$L" "$d/$1" "$1" &
    HOLDER=$!
    pids="$pids $!"
}

# recv VAR FIFO: the first line from FIFO (empty at end of input) in VAR.
# send FIFO: writes a line to FIFO. Each opens its FIFO again when a child's
# exit interrupts the open, which bash does not retry itself.
recv() {
    local _line=""
    until { IFS= read -r _line || true; } 2>/dev/null < "$2"; do :; done
    printf -v "$1" '%s' "$_line"
}
send() { until echo 2>/dev/null > "$1"; do :; done; }

# A second run waits, saying so, until the holder exits; then it holds.
hold a
recv got "$d/a.err"
[ -z "$got" ] && ok "an uncontended lock is taken without waiting" || bad "first holder waited: $got"
recv got "$d/a.out"
hold b
recv waited "$d/b.err"
send "$d/a.in"
recv got "$d/b.out"
case "$waited" in
    "waiting for test holder (lock $L)") [ "$got" = "b held" ] && ok "a second run waits for the holder, then holds" || bad "b: $got" ;;
    *) bad "the second run did not wait: '$waited'" ;;
esac
send "$d/b.in"

# A holder killed with SIGKILL runs no trap; the lock still goes with it.
hold k
recv _ "$d/k.err"
recv _ "$d/k.out"
killed=$HOLDER
hold v
recv waited "$d/v.err"
{ kill -9 "$killed"; wait "$killed"; } 2>/dev/null || true
recv got "$d/v.out"
if [ -n "$waited" ] && [ "$got" = "v held" ]; then
    ok "a holder killed outright lets the lock go"
else
    bad "after kill -9: waited='$waited' got='$got'"
fi

# Runs that all wait for one holder take the lock one at a time once it
# exits: each makes $d/inside while it holds the lock, which fails if
# another holder's is still there.
contenders=""
for i in 1 2 3 4 5 6; do
    bash -c '. scripts/lib/checkout-lock.sh
        checkout_lock "$1" "test holder" 2> /dev/null
        if ! mkdir "$2/inside" 2>/dev/null; then echo "$3" >> "$2/overlaps"; fi
        for _ in 1 2 3 4 5 6 7 8 9 10; do ls "$2" > /dev/null; done
        rmdir "$2/inside" 2>/dev/null || true' _ "$L" "$d" "$i" &
    contenders="$contenders $!"
    pids="$pids $!"
done
send "$d/v.in"
rc=0
for p in $contenders; do wait "$p" || rc=$?; done
if [ "$rc" = 0 ] && [ ! -e "$d/overlaps" ]; then
    ok "contending runs hold the lock one at a time"
else
    bad "contending runs: rc=$rc overlaps=$(tr '\n' ' ' < "$d/overlaps" 2>/dev/null)"
fi

[ -e "$L" ] && ok "the lock file stays for the next run" || bad "the lock file was removed"

exit "$fails"
