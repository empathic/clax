#!/usr/bin/env python3
"""Prints each pipeline of FILE... that runs under pipefail and pipes into a
reader that can stop early, and fails if there is any.

    scripts/check-pipefail-pipes.py [--all] FILE...

With --all every FILE counts as running under pipefail; without, a FILE
that sets it does, and so does one a FILE that sets it sources.
scripts/check-pipefail-pipes.sh, which lists the files, describes the
contract.
"""

import re
import sys

# `. scripts/x.sh`, `source "$HERE/x.sh"`: the name of the script sourced.
SOURCE = re.compile(
    r"(?:^|[\s;&(-])(?:\.|source)\s+[\"']?(?:\$\{?\w+\}?/)?(?:[\w.-]+/)*([\w.-]+\.(?:sh|bash))\b",
    re.A,
)
# A pipe at the end of the line (or before a continuation).
TRAILING_PIPE = re.compile(r"(?<!\|)\|&?\s*\\?\s*$", re.A)
# Each command after a single pipe (not ||, but |&), up to the next one.
PIPED = re.compile(r"(?<!\|)\|(?!\|)&?\s*([^|]*)", re.A)
# A subshell, a group, an assignment or a wrapper that runs the command
# after it.
WRAPPER = re.compile(r"^(?:[({]\s*|\w+=\S*\s+|(?:command|exec|env)\s+|timeout\s+\S+\s+)", re.A)
# grep options, up to a list operator or closing parenthesis.
GREP = re.compile(r"[ef]?grep\b([^;&)]*)", re.A)
GREP_STOPS = re.compile(r"\s-[a-zA-Z]*[qlLm]|\s--(?:quiet|silent|max-count|files-with(?:out)?-match)", re.A)
# The first quoted program of an awk or sed, or a bare sed command such as
# `sed 1q`.
AWK = re.compile(r"awk\b[^'\"]*(?:'([^']*)'|\"([^\"]*)\")", re.A)
SED = re.compile(r"sed\b[^'\"]*(?:'([^']*)'|\"([^\"]*)\")", re.A)
SED_BARE = re.compile(r"sed\s+(?:-n\s+)?(\S+)", re.A)
SED_QUITS = re.compile(r"(?:^|[\s;{}\d/$])[qQ]\d*\s*(?:[;}]|$)", re.A)
MESSAGE = (
    "a reader that stops early (grep -q, head, sed q, awk exit) can fail a pipefail "
    "pipeline by SIGPIPE; read all the input instead (grep ... >/dev/null, sed -n 1p)"
)


def read(path):
    """The lines of PATH, each with its line ending (none when it cannot be
    read); bytes that are not UTF-8 pass through unchanged."""
    try:
        with open(path, encoding="utf-8", errors="surrogateescape", newline="") as f:
            return f.readlines()
    except OSError as e:
        print(f"Can't open {path}: {e.strerror}.", file=sys.stderr)
        return []


def program(m):
    """The quoted program a match of AWK or SED captured, or None."""
    if not m:
        return None
    return m.group(1) if m.group(1) is not None else m.group(2)


def stops_early(c):
    """Whether the piped command C is a reader that can stop early."""
    n = 1
    while n:
        c, n = WRAPPER.subn("", c, count=1)
    g = GREP.match(c)
    awk = program(AWK.match(c))
    sed = program(SED.match(c))
    bare = SED_BARE.match(c)
    if sed is None and bare:
        sed = bare.group(1)
    return bool(
        (g and GREP_STOPS.search(g.group(1)))
        or re.match(r"head\b", c)
        or (awk is not None and re.search(r"\bexit\b", awk))
        or (sed is not None and SED_QUITS.search(sed))
    )


def main(args):
    everything = args[:1] == ["--all"]
    files = args[1:] if everything else args
    texts = [(path, read(path)) for path in files]
    # The names of the scripts that pipefail scripts source.
    sourced = set()
    if not everything:
        for _, lines in texts:
            if any("pipefail" in line for line in lines):
                sourced.update(m.group(1) for m in map(SOURCE.search, lines) if m)

    found = False
    for path, lines in texts:
        base = path.rsplit("/", 1)[-1]
        if not (everything or base in sourced or any("pipefail" in line for line in lines)):
            continue
        for i, line in enumerate(lines):
            if re.match(r"\s*#", line, re.A):
                continue
            # A pipe at the end of the line leads into the next line.
            j = i + 1
            while j < len(lines) and TRAILING_PIPE.search(line):
                line = re.sub(r"\\?\s*$", " ", line, count=1, flags=re.A)
                line += lines[j]
                j += 1
            if any(stops_early(m.group(1)) for m in PIPED.finditer(line)):
                print(f"{path}:{i + 1}: {line}", end="")
                found = True
    if found:
        print(MESSAGE, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8", errors="surrogateescape")
    sys.exit(main(sys.argv[1:]))
