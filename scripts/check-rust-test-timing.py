#!/usr/bin/env python3
"""Prints each piece of Rust test code that does not turn the timing
rules' lint back on, and fails if there is any.

    scripts/check-rust-test-timing.py ROOT

Clippy's timing rules (clippy.toml's disallowed-methods) are for test
code only: the command line allows `clippy::disallowed_methods` (.bazelrc's
build:clippy), so product code is never flagged, and test code warns
again by its own attribute, which `-D warnings` makes an error. A scope that does not carry it would leave its tests out of
the rule unnoticed, so each must:

- an item under `#[cfg(test)]` that is a `mod` or a `fn`: the attribute
  `#[warn(clippy::disallowed_methods)]` among its outer attributes;
- a test crate's root (Cargo's `[[test]]` and `[[bench]]` paths, or with
  automatic discovery every `tests/*.rs` and `benches/*.rs`), and the
  roots of clax-testkit and clax-fake-exe (`src/lib.rs`, `src/main.rs`,
  `src/bin/*.rs`): the inner attribute `#![warn(clippy::disallowed_methods)]`.

The crates are found as ROOT/crates/**/Cargo.toml (a nested crate, such
as the Worker's VFS, counts as its own).
"""

import glob
import os
import re
import sys

OUTER = "#[warn(clippy::disallowed_methods)]"
INNER = "#![warn(clippy::disallowed_methods)]"
TEST_SUPPORT = ("clax-testkit", "clax-fake-exe")
CFG_TEST = re.compile(r"^\s*#\[cfg\(test\)\]\s*$")
ITEM = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:(?:async|const|unsafe)\s+)*(mod|fn)\b")


def lines(path):
    with open(path, encoding="utf-8") as f:
        return f.read().split("\n")


def attribute_block(ls, i):
    """The lines of the attribute and doc-comment block around line I, and
    the index of the line that follows it (the item's)."""
    start = i
    while start > 0 and ls[start - 1].strip().startswith(("#[", "///")):
        start -= 1
    end = i + 1
    depth = 0
    while end < len(ls):
        s = ls[end].strip()
        if depth == 0 and not s.startswith(("#[", "///")):
            break
        depth += s.count("[") - s.count("]")
        end += 1
    return ls[start:end], end


def cfg_test_items(path):
    """(line number, kind) of each `#[cfg(test)]` mod or fn in PATH
    without the outer attribute."""
    ls = lines(path)
    found = []
    for i, l in enumerate(ls):
        if not CFG_TEST.match(l):
            continue
        block, item = attribute_block(ls, i)
        m = ITEM.match(ls[item]) if item < len(ls) else None
        if not m:
            continue
        if not any(b.strip() == OUTER for b in block):
            found.append((item + 1, m.group(1)))
    return found


def crate_roots(crate):
    """The test crate roots of the crate at CRATE (a directory), and its
    own roots if it is test support."""
    manifest = open(os.path.join(crate, "Cargo.toml"), encoding="utf-8").read()
    roots = []
    for kind, auto, folder in (("test", "autotests", "tests"), ("bench", "autobenches", "benches")):
        paths = re.findall(r"\[\[" + kind + r"\]\][^\[]*?\bpath\s*=\s*\"([^\"]+)\"", manifest, re.S)
        roots += [os.path.join(crate, p) for p in paths]
        if not re.search(r"^\s*" + auto + r"\s*=\s*false", manifest, re.M):
            roots += sorted(glob.glob(os.path.join(crate, folder, "*.rs")))
    if os.path.basename(crate) in TEST_SUPPORT:
        for p in ["src/lib.rs", "src/main.rs"] + sorted(
            os.path.relpath(b, crate) for b in glob.glob(os.path.join(crate, "src/bin/*.rs"))
        ):
            if os.path.exists(os.path.join(crate, p)):
                roots.append(os.path.join(crate, p))
    return sorted(set(roots))


def main(root):
    os.chdir(root)
    found = []
    for manifest in sorted(glob.glob("crates/**/Cargo.toml", recursive=True)):
        if "/target/" in manifest:
            continue
        for r in crate_roots(os.path.dirname(manifest)):
            if not any(l.strip() == INNER for l in lines(r)):
                found.append(f"{r}:1: a test crate root without {INNER}")
    for path in sorted(glob.glob("crates/**/*.rs", recursive=True)):
        if "/target/" in path:
            continue
        for n, kind in cfg_test_items(path):
            found.append(f"{path}:{n}: a #[cfg(test)] {kind} without {OUTER}")
    for f in found:
        print(f)
    if found:
        print(
            f"{len(found)} piece(s) of test code outside clippy's timing rules "
            "(scripts/check-rust-test-timing.py says what each needs)",
            file=sys.stderr,
        )
    return 1 if found else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1] if len(sys.argv) > 1 else "."))
