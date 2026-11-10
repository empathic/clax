#!/usr/bin/env python3
"""Fails on a process launch in FILE... that ALLOW does not name.

    scripts/check-ios-spawns.py ALLOW FILE...

scripts/check-ios-spawns.sh, which lists the files, describes the contract.
Paths are read relative to the current directory, which is the root.
"""

import os
import re
import sys

# Whitespace as the regular expressions' \s, which is ASCII only.
WS = " \t\n\r\f\v"
FN = re.compile(
    r'^\s*(?:pub(?:\([^)]*\))?\s+)?(?:const\s+)?(?:async\s+)?(?:unsafe\s+)?(?:extern\s+"C"\s+)?fn\s+(\w+)',
    re.A,
)
LAUNCH = re.compile(
    r"\bCommand::new\b|\bposix_spawnp?\b|\bfork\s*\(\s*\)|\bexec(?:l|lp|le|v|vp|ve|vpe)\s*\(",
    re.A,
)


def lines(path):
    """The lines of PATH, each with its line ending; bytes that are not
    UTF-8 pass through unchanged."""
    try:
        with open(path, encoding="utf-8", errors="surrogateescape", newline="") as f:
            return f.readlines()
    except OSError as e:
        sys.exit(f"{path}: cannot read: {e.strerror}")


def main(allow, files):
    allowed, skip, used, bad = {}, {}, set(), []
    for n, line in enumerate(lines(allow), 1):
        line = re.sub(r"#.*", "", line).strip(WS)
        if not line:
            continue
        m = re.match(r"skip\s+(\S+)$", line)
        if m:
            skip[m.group(1)] = n
            continue
        m = re.match(r"(\S+)\s+(\w+)$", line, re.A)
        if m:
            allowed[f"{m.group(1)} {m.group(2)}"] = n
        else:
            bad.append(f"{allow}:{n}: neither `<path> <fn>` nor `skip <path>`")

    for f in sorted(files):
        if f in skip:
            continue
        fn, test, pending = "(top level)", False, False
        for n, line in enumerate(lines(f), 1):
            # A top-level item under #[cfg(test)] runs to its closing `}`
            # at the start of a line, or is one line ending in `;`.
            if test:
                if line.startswith("}"):
                    test = False
                continue
            if line.startswith("#[cfg(test)]"):
                pending = True
                continue
            if pending:
                if line.startswith("#["):
                    continue
                pending = False
                test = not re.search(r";\s*$", line)
                continue
            c = re.sub(r"//.*", "", line)
            m = FN.search(c)
            if m:
                fn = m.group(1)
            if not LAUNCH.search(c):
                continue
            if f"{f} {fn}" in allowed:
                used.add(f"{f} {fn}")
                continue
            bad.append(
                f"{f}:{n}: a process launch in `{fn}`, which iOS refuses "
                f"(gate it off on iOS and list it in {allow}): {line.strip(WS)}"
            )

    for k in sorted(allowed):
        if k in used:
            continue
        f, fn = k.split(" ")
        bad.append(f"{allow}:{allowed[k]}: no process launch left in `{fn}` of {f}")

    for f in sorted(skip):
        if not os.path.isfile(f):
            bad.append(f"{allow}:{skip[f]}: no file {f}")
            continue
        directory = re.sub(r"/[^/]+$", "", f)
        stem = re.sub(r"^.*/|\.rs$", "", f)
        mod = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+" + re.escape(stem) + r"\s*;")
        # The `#[cfg(...)]` line directly above the first `mod <stem>;` of
        # the module's parent.
        gated = None
        for p in (f"{directory}/{name}" for name in ("lib.rs", "main.rs", "mod.rs")):
            try:
                with open(p, encoding="utf-8", errors="surrogateescape", newline="") as ph:
                    prev = ""
                    for line in ph:
                        if mod.search(line):
                            gated = bool(re.search(r'^\s*#\[cfg\(not\(.*target_os\s*=\s*"ios"', prev))
                            break
                        prev = line
            except OSError:
                continue
            if gated is not None:
                break
        if not gated:
            bad.append(
                f"{f}: skipped, but its `mod {stem};` is not under a "
                f'`#[cfg(not(…))]` naming target_os = "ios"'
            )

    for b in bad:
        print(b)
    return 1 if bad else 0


if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8", errors="surrogateescape")
    sys.exit(main(sys.argv[1], sys.argv[2:]))
