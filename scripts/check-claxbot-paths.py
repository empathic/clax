#!/usr/bin/env python3
"""Judges paths by a ClaxBot profile's writable_paths and protected_paths, with
no daemon: preview.yml's `approve` runs this from the default branch on what a
pull request changes, and refuses a preview if the daemon's own check
(crates/clax-bot-workspace/src/repo.rs) was ever got past.

  check-claxbot-paths.py judge <profile>        paths on stdin, NUL-separated
  check-claxbot-paths.py cases <profile> <file> path-cases.txt's lines
  check-claxbot-paths.py tree  <profile>        every entry names a path here

`judge` prints each refused path on a line, and exits 1 if there is one. `tree`
reads `git ls-files` in the current directory, what is not ignored; an entry
that is no pattern must name something there, unless its line says `# absent:` and why, so the list
cannot rot. It runs outside Bazel with the other scans of tracked files
(scripts/bazel-test.sh).

Python's standard library alone, on any Python 3, since `approve` has nothing
installed and tomllib is 3.11's. The profile's two lists are read by their
lines: a list opens `name = [` and closes `]`, each entry a quoted string on a
line of its own. A list not found, or found empty, is an error.
"""

import re
import subprocess
import sys

ENTRY = re.compile(r'^\s*"([^"]*)",\s*(#\s*absent:.*)?$')


def read_list(text, name):
    """The entries of list `name`, as (entry, may be absent) pairs."""
    lines = iter(text.splitlines())
    for line in lines:
        if line.strip() == name + " = [":
            break
    else:
        sys.exit("no list " + name + " in the profile")
    found = []
    for line in lines:
        if line.strip() == "]":
            if not found:
                sys.exit("the list " + name + " is empty")
            return found
        if not line.strip() or line.strip().startswith("#"):
            continue
        entry = ENTRY.match(line)
        if entry is None:
            sys.exit("unreadable entry in " + name + ": " + line)
        found.append((entry.group(1), entry.group(2) is not None))
    sys.exit("the list " + name + " does not close")


def segment_matches(pattern, text):
    """One path segment of a pattern, each `*` any run of characters."""
    head, star, rest = pattern.partition("*")
    if not star:
        return pattern == text
    if not text.startswith(head):
        return False
    tail = text[len(head):]
    return any(segment_matches(rest, tail[i:]) for i in range(len(tail) + 1))


def pattern_matches(pattern, path):
    """repo.rs's pattern_matches: keep the two in step (path-cases.txt)."""
    if pattern.startswith("**/"):
        return segment_matches(pattern[3:], path.rsplit("/", 1)[-1])
    under = pattern.endswith("/")
    wanted = pattern.rstrip("/").split("/")
    got = path.split("/")
    fits = len(got) >= len(wanted) if under else len(got) == len(wanted)
    return fits and all(segment_matches(w, g) for w, g in zip(wanted, got))


def ascii_lower(text):
    """Rust's to_ascii_lowercase: only A to Z fold."""
    return re.sub("[A-Z]+", lambda m: m.group(0).lower(), text)


def allowed(writable, protected, path):
    lower = ascii_lower(path)
    if any(pattern_matches(ascii_lower(p), lower) for p in protected):
        return False
    return any(pattern_matches(p, path) for p in writable)


def lists(profile):
    with open(profile, encoding="utf-8") as f:
        text = f.read()
    return read_list(text, "writable_paths"), read_list(text, "protected_paths")


def main(argv):
    if len(argv) < 3:
        sys.exit(__doc__)
    command, profile = argv[1], argv[2]
    writable, protected = lists(profile)
    w = [e for e, _ in writable]
    p = [e for e, _ in protected]
    if command == "judge":
        paths = [x for x in sys.stdin.read().split("\0") if x]
        if not paths:
            sys.exit("no paths to judge")
        refused = [x for x in paths if not allowed(w, p, x)]
        for path in refused:
            print(path)
        return 1 if refused else 0
    if command == "cases":
        wrong = 0
        with open(argv[3], encoding="utf-8") as f:
            for line in f:
                line = line.rstrip("\n")
                if not line or line.startswith("#"):
                    continue
                word, path = line.split(" ", 1)
                if word not in ("allow", "refuse"):
                    sys.exit("unreadable case: " + line)
                if allowed(w, p, path) != (word == "allow"):
                    print("wrong: " + line)
                    wrong += 1
        return 1 if wrong else 0
    if command == "tree":
        out = subprocess.run(
            ["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard"],
            check=True,
            stdout=subprocess.PIPE,
        ).stdout
        tracked = out.decode("utf-8", "replace").split("\0")
        gone = 0
        for entry, may_be_absent in writable + protected:
            if "*" in entry or may_be_absent:
                continue
            if not any(pattern_matches(entry, path) for path in tracked):
                print("names nothing in this tree: " + entry)
                gone += 1
        return 1 if gone else 0
    sys.exit(__doc__)


if __name__ == "__main__":
    sys.exit(main(sys.argv))
