#!/usr/bin/env python3
"""A home's config.toml `[feedback]` table, for scripts/dev-feedback.sh.

    feedback-config.py queue <config.toml>
        prints the table's `queue`, or nothing when there is no table.
    feedback-config.py write <config.toml> <queue ID> <origin>
        replaces the table (or appends one) naming the queue, the grant file
        feedback-grant.json and the origin, keeping every other line.

A table header is a line that is one outside a multi-line string:
`[name]` or `[[name]]`, with spaces and a trailing comment allowed. The
`[feedback]` table runs from its header to the next header.
"""

import os
import re
import sys

HEADER = re.compile(r"""^\s*(\[\[?)\s*([^\[\]#]*?)\s*(\]\]?)\s*(#.*)?$""")
NAME = re.compile(r"""^[A-Za-z0-9_.\-" ]+$""")
QUEUE = re.compile(r"""^\s*queue\s*=\s*"([^"]*)"\s*(#.*)?$""")


def strip_strings(line):
    """`line` with its one-line strings and its comment taken out."""
    line = re.sub(r'"(?:[^"\\]|\\.)*"', '""', line)
    line = re.sub(r"'[^']*'", "''", line)
    return line.split("#", 1)[0]


def headers(lines):
    """For each line, the table it opens (its name), else None: lines in a
    multi-line string or inside a multi-line array open none."""
    out = []
    inside = None
    depth = 0
    for line in lines:
        if inside:
            out.append(None)
            if line.count(inside) % 2 == 1:
                inside = None
            continue
        m = HEADER.match(line)
        if (
            depth == 0
            and m
            and (m.group(1), m.group(3)) in (("[", "]"), ("[[", "]]"))
            and NAME.match(m.group(2))
        ):
            out.append(m.group(2).replace(" ", ""))
            continue
        out.append(None)
        opened = None
        for quote in ('"""', "'''"):
            if line.count(quote) % 2 == 1:
                opened = quote
                break
        if opened:
            inside = opened
            continue
        bare = strip_strings(line)
        depth = max(0, depth + bare.count("[") - bare.count("]"))
    return out


def feedback_range(lines):
    """The [start, end) of the [feedback] table's lines, or None."""
    names = headers(lines)
    start = None
    for i, name in enumerate(names):
        if name is None:
            continue
        if start is not None:
            return (start, i)
        if name == "feedback":
            start = i
    return (start, len(lines)) if start is not None else None


def queue(lines):
    r = feedback_range(lines)
    if r is None:
        return None
    for line in lines[r[0] + 1 : r[1]]:
        m = QUEUE.match(line)
        if m:
            return m.group(1)
    return None


def write(lines, qid, origin):
    r = feedback_range(lines)
    if r is not None:
        lines = lines[: r[0]] + lines[r[1] :]
    while lines and not lines[-1].strip():
        lines.pop()
    table = [
        "[feedback]",
        f'queue = "{qid}"',
        'service_grant_file = "feedback-grant.json"',
        f'origin = "{origin}"',
    ]
    return (lines + [""] if lines else []) + table


def main(argv):
    if len(argv) >= 2 and argv[0] == "queue":
        path = argv[1]
        lines = open(path).read().splitlines() if os.path.exists(path) else []
        q = queue(lines)
        if q:
            print(q)
        return 0
    if len(argv) == 4 and argv[0] == "write":
        path, qid, origin = argv[1:]
        lines = open(path).read().splitlines() if os.path.exists(path) else []
        text = "\n".join(write(lines, qid, origin)) + "\n"
        tmp = f"{path}.tmp.{os.getpid()}"
        with open(tmp, "w") as f:
            f.write(text)
        if os.path.exists(path):
            os.chmod(tmp, os.stat(path).st_mode & 0o777)
        os.replace(tmp, path)
        return 0
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
