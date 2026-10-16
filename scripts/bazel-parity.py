#!/usr/bin/env python3
"""Fails when nextest and Bazel disagree on which Rust tests exist.

    cargo nextest list --workspace --message-format json > nextest.json
    scripts/bazel-parity.py nextest.json BAZEL_TESTLOGS

nextest.json names every test Cargo builds under its binary (`clax-core`
for a library's unit tests, `clax-cli::bin/clax` for a binary's,
`clax-cli::integration` for a [[test]] target). BAZEL_TESTLOGS is
`bazel-testlogs` (or a pass directory of scripts/bazel-test.sh's
BAZEL_TEST_LOGS): each Rust test target's test.log, where libtest printed
`test <name> ... <result>` for every test it ran or ignored, shards
included.
"""

import collections
import json
import os
import re
import sys

TEST = re.compile(r"^test (.+?)(?: - should panic)? \.\.\. (ok|FAILED|ignored)\b")

# Tests that exist under Bazel only, by design.
BAZEL_ONLY = {
    # The debug build that embeds its web UI is Bazel's (clax_embedded).
    ("clax-server", "routes::shell::tests::an_embedded_debug_build_versions_the_embedded_bridge"),
}


def nextest(path):
    """{binary: {test}} from `cargo nextest list --message-format json`."""
    out = collections.defaultdict(set)
    for binary, suite in json.load(open(path))["rust-suites"].items():
        out[binary] |= set(suite["testcases"])
    return out


def binary_of(crate, target):
    """nextest's name for a Bazel test target of crate directory `crate`.

    A split integration test's targets (`<test>__<module>`, bazel/cargo.bzl)
    are modules of nextest's one binary `<test>`."""
    target = target.split("__")[0]
    if target.endswith("_lib_test"):
        return crate
    if target.endswith("_bin_test"):
        return "%s::bin/%s" % (crate, target[: -len("_bin_test")])
    return "%s::%s" % (crate, target)


def bazel(directory):
    """{binary: {test}} from the test logs under `directory`."""
    out = collections.defaultdict(set)
    crates = os.path.join(directory, "crates")
    for parent, _, files in os.walk(crates):
        if "test.log" not in files:
            continue
        parts = os.path.relpath(parent, crates).split(os.sep)
        if len(parts) < 2:
            continue
        crate, target = parts[0], parts[1]
        binary = binary_of(crate, target)
        for line in open(os.path.join(parent, "test.log"), errors="replace"):
            if (m := TEST.match(line)) and (binary, m.group(1)) not in BAZEL_ONLY:
                out[binary].add(m.group(1))
    return out


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    cargo, ours = nextest(sys.argv[1]), bazel(sys.argv[2])
    problems = []
    count = 0
    for binary in sorted(set(cargo) | set(ours)):
        if binary not in ours:
            problems.append("%s: only nextest runs this test binary" % binary)
            continue
        if binary not in cargo:
            problems.append("%s: only Bazel runs this test binary" % binary)
            continue
        for test in sorted(cargo[binary] ^ ours[binary]):
            side = "nextest" if test in cargo[binary] else "Bazel"
            problems.append("%s: %s: only %s runs it" % (binary, test, side))
        count += len(cargo[binary] & ours[binary])
    for problem in problems:
        print("error: " + problem, file=sys.stderr)
    if problems:
        sys.exit("error: nextest and Bazel disagree in %d places" % len(problems))
    if not count:
        sys.exit("error: no tests were compared")
    print("parity: %d tests, all in both" % count)


if __name__ == "__main__":
    main()
