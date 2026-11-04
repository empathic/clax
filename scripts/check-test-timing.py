#!/usr/bin/env python3
"""Prints each sleep in the test scripts FILE... that stands in for a
condition, each clock reading a comparison judges, and each stale
exemption.

    scripts/check-test-timing.py FILE...
    scripts/check-test-timing.py --sh-tests BUILD

The first form exits 1 when it printed anything and CLAX_TIMING is
`deny` (DEFAULT when unset), 0 otherwise (report mode). The second
prints the scripts the `sh_test` targets of the BUILD file BUILD run
(each one's `srcs`), one per line. scripts/check-test-timing.sh, which
lists the files, describes the contract.
"""

import os
import re
import sys

# A `sleep` where a command starts (at the line's start, or after a list
# operator, a pipe, a subshell or group, or a keyword that runs a command)
# with a delay: a number, a variable, or `infinity`.
SHELL_SLEEP = re.compile(
    r"(?:^|[;&|({]|\b(?:then|do|else|time|!)\s)\s*(?:command\s+)?sleep\s+"
    r"(?:[0-9.]+[smhd]?|\"?\$\{?\w+\}?\"?|infinity)(?=[\s;&|)]|$)",
    re.A,
)
# Python's (in a fake server, or a heredoc a test script runs).
PYTHON_SLEEP = re.compile(r"\b(?:time|asyncio)\.sleep\s*\(", re.A)
# A reading of the wall or monotonic clock: Python's, JavaScript's (in a
# `node -e` a script runs) and the shell's.
CLOCK = re.compile(
    r"\btime\.(?:time|monotonic|perf_counter)(?:_ns)?\s*\("
    r"|\bDate\.now\s*\(|\bperformance\.now\s*\("
    r"|\bdate\s+\+%s"
    r"|(?<!\w)\$?\{?(?:SECONDS|EPOCHSECONDS|EPOCHREALTIME)\b",
    re.A,
)
# An assignment in Python or JavaScript (`x = …`, `x["k"] = …`, `x += …`,
# `const x = …`), and the shell's (`x=…`, `local x=… y=…`).
CODE_ASSIGN = re.compile(
    r"^\s*(?:(?:const|let|var)\s+)?([A-Za-z_]\w*)(?:\[[^\]]*\])*\s*[-+*/]?=(?!=)(.*)$", re.A
)
SHELL_ASSIGN = re.compile(r"(?:^|(?<=[\s;(]))([A-Za-z_]\w*)=(?!=)", re.A)
# A comparison: a shell test operator, or `<`, `<=`, `>`, `>=` once
# redirections (to a quoted, variable or absolute path, a descriptor),
# heredocs, process substitutions and arrows are taken out.
SHELL_COMPARE = re.compile(r"\s-(?:lt|le|gt|ge)\s", re.A)
NOT_COMPARE = re.compile(r"\d?>&\d?|&>|\d>|>>|<<-?|<<<|[<>]\(|->|=>|[<>]\s*[\"$/]", re.A)
COMPARE = re.compile(r"(?<![<>=!])(?:<=?|>=?)(?![<>=])", re.A)
# Report mode until the scripts are clean: "deny" here makes a finding
# fail every run, as CLAX_TIMING=deny (--config=timing) does now.
DEFAULT = "report"
# The exemption, at the end of the line or alone on the line before it.
MARK = re.compile(r"#\s*timing:\s*(.*)$", re.A)
HINT = {
    "sleep": "wait on the condition (wait_for in scripts/lib/wait.sh), or model a "
    "hang with a process that never ends by itself (exec sleep, sleep ... &, "
    "read < fifo), killed by its PID",
    "time.sleep": "wait on the condition with a hang guard (scripts/lib/wait.sh's "
    "wait_for around the check), or block on what the test controls",
    "clock": "a clock reading judged: assert the outcome, not how long it took "
    "(print a measurement rather than judge it), and bound a wait by a count of "
    "polls (wait_for), not by the clock",
}


def read(path):
    """The lines of PATH (none when it cannot be read)."""
    try:
        with open(path, encoding="utf-8", errors="surrogateescape") as f:
            return f.read().splitlines()
    except OSError as e:
        print(f"Can't open {path}: {e.strerror}.", file=sys.stderr)
        return []


def code(line):
    """LINE without a trailing shell or Python comment: from a `#` at the
    start or after a space, outside quotes."""
    quote = None
    for i, c in enumerate(line):
        if quote:
            if c == quote:
                quote = None
        elif c in "'\"":
            quote = c
        elif c == "#" and (i == 0 or line[i - 1].isspace()):
            return line[:i]
    return line


def sleeps(c):
    """The sleep rule code C breaks, or None. A `sleep` that is `exec`'d or
    put in the background (`sleep N &`, not `&&`) is a process standing in
    for one that never ends, not a wait."""
    if PYTHON_SLEEP.search(c):
        return "time.sleep"
    for m in SHELL_SLEEP.finditer(c):
        before = c[: m.start()]
        after = c[m.end():]
        if re.search(r"\bexec\b", before):
            continue
        if re.match(r"\s*\)?\s*&(?!&)", after):
            continue
        return "sleep"
    return None


def reads_clock(c, timed):
    """Whether code C reads the clock, or one of the names TIMED (bare, or
    as $name) that hold a reading."""
    if CLOCK.search(c):
        return True
    return any(re.search(rf"(?<![\w.]){re.escape(n)}\b", c) for n in timed)


def assigned(c):
    """The names code C assigns, each with the text of its value."""
    m = CODE_ASSIGN.match(c)
    if m and not re.match(r"\s*(?:if|elif|while|for|assert|return|local|export)\b", c):
        return [(m.group(1), m.group(2))]
    found = list(SHELL_ASSIGN.finditer(c))
    return [
        (m.group(1), c[m.end(): found[i + 1].start() if i + 1 < len(found) else len(c)])
        for i, m in enumerate(found)
    ]


def compares(c):
    """Whether code C holds a comparison."""
    return bool(SHELL_COMPARE.search(c) or COMPARE.search(NOT_COMPARE.sub(" ", c)))


def rules(lines):
    """The rule each of LINES breaks, or None, in order. A name assigned a
    clock reading, or a value computed from one, is followed to the
    comparisons that read it, until it is assigned something else: a
    measurement printed is no finding, a budget or a deadline judged is."""
    timed = set()
    out = []
    for line in lines:
        c = code(line)
        rule = sleeps(c)
        for name, value in assigned(c):
            if reads_clock(value, timed):
                timed.add(name)
            else:
                timed.discard(name)
        if not rule and compares(c) and reads_clock(c, timed):
            rule = "clock"
        out.append(rule)
    return out


def check(path):
    """The findings of PATH, as printed lines."""
    lines = read(path)
    broken = rules(lines)
    found = []
    for n, line in enumerate(lines, 1):
        rule = broken[n - 1]
        alone = code(line).strip() == ""
        mark = MARK.search(line)
        if mark and alone:
            # An exemption on its own line is the next line's.
            if not (n < len(lines) and broken[n]):
                found.append(f"{path}:{n}: stale exemption: nothing to exempt on the next line")
            continue
        if not mark and n > 1 and code(lines[n - 2]).strip() == "":
            mark = MARK.search(lines[n - 2])
        if rule and mark and mark.group(1).strip():
            continue
        if rule and mark:
            found.append(f"{path}:{n}: {rule}: an exemption needs its reason (# timing: <why>)")
        elif rule:
            found.append(f"{path}:{n}: {rule}: {HINT[rule]}")
        elif mark and not alone and MARK.search(line):
            found.append(f"{path}:{n}: stale exemption: nothing to exempt on this line")
    return found


def sh_tests(build):
    """The `srcs` of each `sh_test(...)` call in the BUILD file BUILD: the
    scripts its tests run."""
    text = "\n".join(read(build))
    srcs = []
    for call in re.finditer(r"^sh_test\((.*?)^\)", text, re.M | re.S):
        m = re.search(r"^\s*srcs\s*=\s*\[([^\]]*)\]", call.group(1), re.M)
        if m:
            srcs += re.findall(r'"([^"]+)"', m.group(1))
    return srcs


def main(args):
    if args[:1] == ["--sh-tests"]:
        if len(args) != 2:
            print("usage: check-test-timing.py --sh-tests BUILD", file=sys.stderr)
            return 2
        for src in sh_tests(args[1]):
            print(src)
        return 0
    found = [f for path in args for f in check(path)]
    for f in found:
        print(f)
    deny = os.environ.get("CLAX_TIMING", DEFAULT) == "deny"
    if found:
        mode = "failing (CLAX_TIMING=deny)" if deny else "report mode: CLAX_TIMING=deny makes them fail"
        print(f"{len(found)} timing finding(s) in the test scripts ({mode})", file=sys.stderr)
    return 1 if found and deny else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
