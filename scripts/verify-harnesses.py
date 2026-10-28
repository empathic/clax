#!/usr/bin/env python3
"""The parts of scripts/verify-harnesses.sh that read JSON or watch a process.

    verify-harnesses.py artifact-exe < cargo-messages
    verify-harnesses.py claude-names SETTINGS KNOWN_MARKETPLACES INSTALLED_PLUGINS
    verify-harnesses.py pi-packages SETTINGS
    verify-harnesses.py watchdog SECONDS COMMAND [ARG...]

artifact-exe prints the executable of each `clax` binary in cargo's JSON
messages (one per line, lines that are not JSON skipped). claude-names
prints the keys of Claude Code's registries, sorted per map: every key of
known_marketplaces.json, of installed_plugins.json's "plugins", and of
settings.json's "enabledPlugins" and "extraKnownMarketplaces"; a file that
is missing is skipped, and one whose content is not a JSON object prints
"?unparseable <file>". pi-packages prints the source of each entry of Pi's
"packages" list (an entry is a source string or an object with "source");
a missing file prints nothing and an unparseable one "?unparseable <file>".

watchdog runs COMMAND in a process group of its own and exits with its
status (128 + the signal when a signal ended it). After SECONDS it stops
the group (TERM, then KILL two seconds on) and exits 124. INT, TERM and HUP
stop the group the same way, then end the watchdog by the same signal.
When COMMAND exits, whatever is left in its group is killed.

Runs on Python 3.9.
"""

import json
import os
import signal
import sys
import time


def out(line):
    sys.stdout.buffer.write(line.encode("utf-8", "surrogateescape") + b"\n")


def text(v):
    """A JSON scalar as text: null empty, booleans 1 and 0."""
    if v is None:
        return ""
    if v is True:
        return "1"
    if v is False:
        return "0"
    if isinstance(v, str):
        return v
    if isinstance(v, (int, float)):
        return repr(v)
    return json.dumps(v)


def load(path):
    """The JSON object in `path`: None when it cannot be read, False when it
    is not a JSON object."""
    try:
        with open(path, "rb") as f:
            data = f.read()
    except OSError:
        return None
    try:
        j = json.loads(data.decode("utf-8"))
    except ValueError:
        return False
    return j if isinstance(j, dict) else False


def artifact_exe():
    for line in sys.stdin.buffer:
        try:
            m = json.loads(line.decode("utf-8"))
        except ValueError:
            continue
        if not isinstance(m, dict) or m.get("reason") != "compiler-artifact":
            continue
        t = m.get("target")
        t = t if isinstance(t, dict) else {}
        kinds = t.get("kind")
        if t.get("name") != "clax" or not isinstance(kinds, list) or "bin" not in kinds:
            continue
        if m.get("executable") is not None:
            out(text(m["executable"]))


def claude_names(paths):
    for p in paths:
        j = load(p)
        if j is None:
            continue
        if j is False:
            out("?unparseable " + p)
            continue
        if p.endswith("known_marketplaces.json"):
            maps = [j]
        elif p.endswith("installed_plugins.json"):
            maps = [j.get("plugins")]
        else:
            maps = [j.get("enabledPlugins"), j.get("extraKnownMarketplaces")]
        for m in maps:
            if isinstance(m, dict):
                for k in sorted(m):
                    out(k)


def pi_packages(path):
    j = load(path)
    if j is None:
        return
    if j is False:
        out("?unparseable " + path)
        return
    packages = j.get("packages")
    for p in packages if isinstance(packages, list) else []:
        out(text(p.get("source")) if isinstance(p, dict) else text(p))


def watchdog(seconds, argv):
    pid = 0

    def kill_group(sig):
        try:
            os.killpg(pid, sig)
        except OSError:
            pass

    def stop():
        if not pid:
            return
        kill_group(signal.SIGTERM)
        for _ in range(20):
            try:
                if os.waitpid(pid, os.WNOHANG)[0] != 0:
                    break
            except ChildProcessError:
                break
            time.sleep(0.1)
        kill_group(signal.SIGKILL)
        try:
            os.waitpid(pid, 0)
        except ChildProcessError:
            pass

    def on_signal(sig, _frame):
        stop()
        signal.signal(sig, signal.SIG_DFL)
        os.kill(os.getpid(), sig)
        os._exit(1)

    def on_alarm(_sig, _frame):
        stop()
        os._exit(124)

    for s in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
        signal.signal(s, on_signal)
    signal.signal(signal.SIGALRM, on_alarm)
    pid = os.fork()
    if pid == 0:
        os.setpgid(0, 0)
        # Python ignores these itself; the command gets the defaults.
        signal.signal(signal.SIGPIPE, signal.SIG_DFL)
        signal.signal(signal.SIGXFSZ, signal.SIG_DFL)
        try:
            os.execvp(argv[0], argv)
        except OSError as e:
            sys.stderr.write("exec %s: %s\n" % (argv[0], e.strerror))
            sys.stderr.flush()
            os._exit(e.errno or 255)
    signal.alarm(seconds)
    _, st = os.waitpid(pid, 0)
    signal.alarm(0)
    kill_group(signal.SIGKILL)
    sys.exit(128 + os.WTERMSIG(st) if os.WIFSIGNALED(st) else os.WEXITSTATUS(st))


def main(argv):
    cmd = argv[1] if len(argv) > 1 else ""
    if cmd == "artifact-exe" and len(argv) == 2:
        artifact_exe()
    elif cmd == "claude-names":
        claude_names(argv[2:])
    elif cmd == "pi-packages" and len(argv) == 3:
        pi_packages(argv[2])
    elif cmd == "watchdog" and len(argv) > 3 and argv[2].isdigit():
        watchdog(int(argv[2]), argv[3:])
    else:
        sys.exit(__doc__.split("\n\n")[1])


if __name__ == "__main__":
    main(sys.argv)
