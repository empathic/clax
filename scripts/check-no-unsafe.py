#!/usr/bin/env python3
"""Prints each use of `unsafe` in SOURCE... and the manifests that relax the
`unsafe_code` lint, and fails if there is any.

    scripts/check-no-unsafe.py SOURCE...

scripts/check-no-unsafe.sh, which lists the sources, describes the
contract. Paths are read relative to the current directory, the root.
"""

import glob
import os
import re
import sys

ONLY = "only the four exported functions may use unsafe"
# The exports of clax-embed: True for those declared `unsafe` (they take pointers).
EMBED = {
    "clax_embed_open": True,
    "clax_embed_call": True,
    "clax_embed_subscribe": False,
    "clax_embed_close": False,
}
EMBED_PARAM = {"u64", "i64", "i32", "usize", "*const u8", "ClaxPost"}
SLICE = re.compile(r"let \w+ = unsafe \{ std::slice::from_raw_parts\(\w+, \w+\) \};$", re.A)
OWN_LINTS = {"crates/clax-embed/Cargo.toml", "crates/clax-worker/Cargo.toml"}
# Whitespace as the regular expressions' \s, which is ASCII only.
WS = " \t\n\r\f\v"

bad = False


def finding(message):
    global bad
    print(message)
    bad = True


def read(path):
    """The lines of PATH, each with its line ending; bytes that are not
    UTF-8 pass through unchanged."""
    with open(path, encoding="utf-8", errors="surrogateescape", newline="") as f:
        return f.readlines()


def manifest(path):
    """A manifest as {"table.key": value}, for the plain TOML Cargo.toml
    uses: one key per line, [table] headers, # comments outside strings.
    None when it cannot be read."""
    try:
        lines = read(path)
    except OSError:
        return None
    keys = {}
    table = ""
    for line in lines:
        line = line.strip(WS)
        if line == "" or line.startswith("#"):
            continue
        if line.startswith("[["):
            table = "[[array]]"
            continue
        m = re.match(r"\[\s*([^\]]+?)\s*\]", line, re.A)
        if m:
            table = re.sub(r"\s*\.\s*", ".", m.group(1), flags=re.A)
            continue
        m = re.match(r'([\w.\-" ]+?)\s*=\s*(.*?)\s*(?:#[^"]*)?$', line, re.A)
        if m:
            key = re.sub(r'[\s"]', "", m.group(1), flags=re.A)
            keys[key if table == "" else f"{table}.{key}"] = m.group(2)
    return keys


def value(keys, key):
    """The value of KEY, also when its parent is an inline table."""
    if keys is None:
        return None
    if key in keys:
        return keys[key]
    m = re.match(r"(.*)\.([^.]+)$", key)
    if not m:
        return None
    parent, leaf = m.groups()
    inline = value(keys, parent)
    if inline is None:
        return None
    m = re.match(r"\{.*\b" + re.escape(leaf) + r"\s*=\s*([^,}]+?)\s*[,}]", inline, re.A)
    return m.group(1) if m else None


def level(keys, key):
    """The level of a lint: "deny" or { level = "deny", ... }."""
    v = value(keys, key)
    if v is None:
        return ""
    m = re.match(r'"(\w+)"$', v, re.A) or re.search(r'\blevel\s*=\s*"(\w+)"', v, re.A)
    return m.group(1) if m else ""


def exports(text, code):
    """The lines granted to the four exports of crates/clax-embed/src/ffi.rs:
    their `#[allow(unsafe_code)]`, their `#[unsafe(no_mangle)]` and the
    slices their bodies read. Each granted signature line is cut, in CODE,
    to what follows its `(`, which is checked as any other line."""
    granted, named = set(), set()
    for i in range(1, len(code) - 1):
        if code[i] != "#[unsafe(no_mangle)]" or code[i - 1] != "#[allow(unsafe_code)]":
            continue
        m = re.match(r'pub (unsafe )?extern "C" fn (\w+)\(', code[i + 1], re.A)
        if not m:
            continue
        marked, name = bool(m.group(1)), m.group(2)
        if name not in EMBED or marked != EMBED[name] or name in named:
            continue
        named.add(name)
        # The signature, from its line to the one closing its parameters.
        end = i + 1
        sig = code[end]
        while ")" not in sig and end < len(code) - 1:
            end += 1
            sig += " " + code[end]
        m = re.search(r"\(([^)]*)\)", sig)
        if not m:
            continue
        params = [p for p in m.group(1).split(",") if re.search(r"\S", p, re.A)]
        types = [re.match(r"\s*\w+\s*:\s*(.+?)\s*$", p, re.A) for p in params]
        if any(t is None or t.group(1) not in EMBED_PARAM for t in types):
            continue
        body = set()
        if EMBED[name]:
            # The body runs to the first line that is a lone `}`.
            close = end
            while close < len(code) - 1 and not re.match(r"\}\s*$", text[close], re.A):
                close += 1
            slices = [
                j
                for j in range(end + 1, close)
                if SLICE.match(code[j]) and code[j - 1] == "" and re.match(r"\s*// SAFETY:", text[j - 1], re.A)
            ]
            if not slices:
                continue
            body.add(slices[0])
        granted.update({i - 1, i} | body)
        # The signature line up to its `(`; what follows is checked.
        code[i + 1] = re.sub(r"^[^(]*\(", "", code[i + 1], count=1)
    return granted


def main(sources):
    workspace = manifest("Cargo.toml")
    if not (workspace and level(workspace, "workspace.lints.rust.unsafe_code") == "forbid"):
        finding("Cargo.toml: the workspace must forbid unsafe_code")

    for path in sorted(glob.glob("crates/*/Cargo.toml")):
        if path in OWN_LINTS:
            continue
        keys = manifest(path) or {}
        takes = (value(keys, "lints.workspace") or "") == "true" and not any(
            k.startswith("lints.") and k != "lints.workspace" for k in keys
        )
        if not takes:
            finding(f"{path}: does not take the workspace's lints")
    if os.path.exists("crates/clax-embed/Cargo.toml"):
        embed = manifest("crates/clax-embed/Cargo.toml")
        if level(embed, "lints.rust.unsafe_code") != "deny":
            finding("crates/clax-embed/Cargo.toml: must deny unsafe_code")
        if level(embed, "lints.rust.unsafe_op_in_unsafe_fn") != "forbid":
            finding("crates/clax-embed/Cargo.toml: must forbid unsafe_op_in_unsafe_fn")

    for path in sources:
        try:
            text = read(path)
        except OSError as e:
            sys.exit(f"{path}: {e.strerror}")
        code = [re.sub(r"//.*", "", t).strip(WS) for t in text]
        embed = path.startswith("crates/clax-embed/")
        granted = exports(text, code) if path == "crates/clax-embed/src/ffi.rs" else set()
        for i, c in enumerate(code):
            if i in granted:
                continue
            n = i + 1
            if re.search(r"\bunsafe\b(?!-)", c, re.A):
                line = text[i][:-1] if text[i].endswith("\n") else text[i]
                finding(f"{path}:{n}: {ONLY}" if embed else f"{path}:{n}: {line}")
            elif embed and re.search(r"\bunsafe_code\b", c, re.A):
                finding(f"{path}:{n}: {ONLY}")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8", errors="surrogateescape")
    sys.exit(main(sys.argv[1:]))
