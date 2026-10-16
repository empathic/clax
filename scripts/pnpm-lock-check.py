#!/usr/bin/env python3
"""Fails when a package.json and its pnpm-lock.yaml disagree.

    scripts/pnpm-lock-check.py web extension

What `pnpm install --frozen-lockfile` checks before it installs: every
dependency in package.json is in the lockfile with the same specifier, and
the lockfile has none that package.json lacks. Bazel builds from the
lockfile alone, so without this a dependency added to package.json and not
locked would only show up as a failed import.
"""

import json
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
KINDS = ["dependencies", "devDependencies", "optionalDependencies"]


def locked(path):
    """{kind: {name: specifier}} of the lockfile's root importer."""
    # pnpm writes the lockfile as two YAML documents (its own version, then
    # the project), each with an `importers` section; the kinds wanted are
    # in whichever has them.
    importers = re.findall(r"^importers:\n\n  \.:\n(.*?)(?=^\S|^  \S|\Z)", open(path).read(), re.M | re.S)
    if not importers:
        sys.exit("error: %s has no root importer" % path)
    out, kind, name = {}, None, None
    for line in "\n".join(importers).splitlines():
        if re.match(r"^    \S", line):
            kind = line.strip().rstrip(":")
            if kind in KINDS:
                out[kind] = {}
        elif kind in KINDS and re.match(r"^      \S", line):
            name = line.strip().rstrip(":").strip("'\"")
        elif kind in KINDS and name and line.strip().startswith("specifier:"):
            out[kind][name] = line.split("specifier:", 1)[1].strip().strip("'\"")
    return out


def main():
    problems = []
    for project in sys.argv[1:]:
        package = json.load(open(os.path.join(ROOT, project, "package.json")))
        lock = locked(os.path.join(ROOT, project, "pnpm-lock.yaml"))
        for kind in KINDS:
            want, have = package.get(kind, {}), lock.get(kind, {})
            for name in sorted(set(want) | set(have)):
                if name not in have:
                    problems.append("%s: %s is in package.json and not in pnpm-lock.yaml" % (project, name))
                elif name not in want:
                    problems.append("%s: %s is in pnpm-lock.yaml and not in package.json" % (project, name))
                elif want[name] != have[name]:
                    problems.append("%s: %s is %s in package.json and %s in pnpm-lock.yaml" % (
                        project, name, want[name], have[name]))
    for problem in problems:
        print("error: " + problem, file=sys.stderr)
    if problems:
        sys.exit("error: package.json and pnpm-lock.yaml disagree; run `pnpm install` in the project and commit the lockfile")
    print("pnpm lockfiles agree with package.json: " + ", ".join(sys.argv[1:]))


if __name__ == "__main__":
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    main()
