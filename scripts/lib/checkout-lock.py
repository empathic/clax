#!/usr/bin/env python3
"""The holder behind checkout_lock (scripts/lib/checkout-lock.sh).

    checkout-lock.py PATH WHAT OWNER

Takes a flock(2) on PATH, saying once on stderr, while it waits, that it is
waiting for WHAT, and gives up (exit 1) when the process OWNER exits first.
Once it holds the lock it prints "locked" on stdout, closes stdout, lets go
of stderr, and keeps the lock until OWNER exits, however OWNER exits; the
kernel then lets the lock go with this process.
"""

import fcntl
import os
import signal
import sys
import time


def alive(pid):
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def main():
    path, what, owner = sys.argv[1], sys.argv[2], int(sys.argv[3])
    # SIGINT keeps the disposition it came with, as for any other program.
    if signal.getsignal(signal.SIGINT) is signal.default_int_handler:
        signal.signal(signal.SIGINT, signal.SIG_DFL)
    try:
        fh = open(path, "a")
    except OSError as e:
        sys.exit("cannot open %s: %s" % (path, e.strerror))
    said = False
    while True:
        try:
            fcntl.flock(fh, fcntl.LOCK_EX | fcntl.LOCK_NB)
            break
        except BlockingIOError:
            pass
        if not said:
            sys.stderr.write("waiting for %s (lock %s)\n" % (what, path))
            sys.stderr.flush()
            said = True
        if not alive(owner):
            sys.exit(1)
        time.sleep(0.2)
    sys.stdout.write("locked\n")
    sys.stdout.close()
    os.close(1)
    devnull = os.open(os.devnull, os.O_WRONLY)
    os.dup2(devnull, 2)
    os.close(devnull)
    while alive(owner):
        time.sleep(0.2)


if __name__ == "__main__":
    main()
