#!/usr/bin/env python3
"""Prints each line of FILE... that makes a hard link, and fails if any does.

    scripts/check-no-hard-links.py FILE...

scripts/check-no-hard-links.sh, which lists the files, describes the
contract.
"""

import re
import sys

Q = "[\"']"
CODE = [
    # Node fs.link / fs.linkSync / promises.link, Python os.link, nix or
    # libc link and linkat, and link imported from Node fs or Python os;
    # "symlinkSync" and "unlinkSync" have no word boundary before "link".
    r"\b(?:fs|promises|os|unistd|libc)\s*(?:\.|::)\s*link(?:at)?\s*\(",
    r"\blinkSync\b|\blinkat\s*\(",
    r"\blink\b.*\bfrom\s+" + Q + r"(?:node:)?fs(?:/promises)?" + Q,
    r"\bfrom\s+os\s+import\b.*\blink\b",
    # cp -l / cp --link.
    r"(?:^|[\s;&|(`\"'])cp\s+(?:-[a-zA-Z]*l[a-zA-Z]*|--link)\b",
    # Code that runs ln as a program.
    r'Command::new\(\s*"ln"\s*\)',
    r"\b(?:spawn|spawnSync|execFile|execFileSync|exec|execSync)\s*\(\s*[\"'`]ln\b",
    r"\bsubprocess\.\w+\(\s*\[\s*" + Q + "ln" + Q,
]
CODE = [re.compile(p, re.A) for p in CODE]
# Rust std::fs::hard_link, libc/nix linkat, a hard-link crate, and the
# hardlink attribute of a Bazel copy rule set to anything but "off".
HARD_LINK = re.compile(r"\bhard_?link\b", re.A | re.I)
OFF = re.compile(r'\bhardlink\s*=\s*"off"', re.A)
# In shell (scripts, justfiles, Makefiles): ln, or a variable naming it,
# with its options. Elsewhere a bare "ln" is an identifier, such as a Rust
# variable.
SHELL = re.compile(r"\.(?:sh|bash)$|(?:^|/)(?:justfile|Makefile)$")
LN = re.compile(r"(?:^|[\s;&|(`\"'])(?:ln|\$\{?\w*_LN\}?)\"?((?:\s+-[-\w]+)*)(?=\s|$)", re.A)
SYMBOLIC = re.compile(r"\s-[a-zA-Z]*s|--symbolic", re.A)


def bad(line, shell):
    if HARD_LINK.search(line) and not OFF.search(line):
        return True
    if any(p.search(line) for p in CODE):
        return True
    # An ln whose options do not include -s.
    return shell and any(not SYMBOLIC.search(m.group(1)) for m in LN.finditer(line))


def main(files):
    found = False
    for path in files:
        try:
            f = open(path, encoding="utf-8", errors="surrogateescape", newline="")
        except OSError as e:
            print(f"Can't open {path}: {e.strerror}.", file=sys.stderr)
            continue
        shell = bool(SHELL.search(path))
        with f:
            for n, line in enumerate(f, 1):
                if bad(line, shell):
                    print(f"{path}:{n}: {line}", end="")
                    found = True
    if found:
        print("hard links are not allowed in Clax; use a symbolic link", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8", errors="surrogateescape")
    sys.exit(main(sys.argv[1:]))
