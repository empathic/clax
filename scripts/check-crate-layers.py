#!/usr/bin/env python3
"""Fails when a workspace crate depends on a workspace crate it may not.

    scripts/check-crate-layers.py [ROOT]

The layout (docs/superpowers/specs/2026-10-16-workspace-layout-design.md
§2.1) orders the crates: each may depend only on the crates above it, never
sideways or up. ALLOWED below is that picture, crate by crate. The check
reads the workspace's members from ROOT/Cargo.toml (default: the
repository) and each member's [dependencies] and [build-dependencies]; a
workspace crate named there and not in the crate's ALLOWED set fails it, as
does a member ALLOWED does not know. Dev-dependencies are not checked: a
crate's tests may reach the test support and the crates they drive (Bazel's
derivation already refuses features turned on there).
"""
import re
import sys
from pathlib import Path

CORE = {"clax-core"}
STORE = CORE | {"clax-store"}
REALTIME = STORE | {"clax-realtime"}
SAMPLE = CORE | {"clax-sample"}
HTTP = REALTIME | SAMPLE | {"clax-http"}
MCP = CORE | {"clax-mcp"}
HOOKS = CORE | {"clax-hooks"}
SERVER = HTTP | MCP | {"clax-server"}
COMMANDS = STORE | MCP | HOOKS | {"clax-commands"}

# What each crate may depend on (outside its tests).
ALLOWED = {
    "clax-core": set(),
    "clax-fake-exe": set(),
    "clax-embedded": set(),
    "clax-store": CORE,
    "clax-sample": CORE,
    "clax-mcp": CORE,
    "clax-hooks": CORE,
    "clax-realtime": STORE,
    "clax-http": REALTIME | SAMPLE,
    "clax-server": HTTP | MCP,
    "clax-commands": STORE | MCP | HOOKS,
    "clax-cli": SERVER | COMMANDS | {"clax-embedded"},
    "clax-testkit": SERVER | {"clax-fake-exe"},
}


def tables(text):
    """The keys of each table of a Cargo.toml: {table: [key]}, a
    `[target.'<cfg>'.dependencies]` table counted as `dependencies`. Enough
    TOML for the workspace's manifests (the tests' Python has no tomllib):
    `[table]` headers and `key = ...` or `key.workspace = ...` lines."""
    out, table = {}, None
    for line in text.splitlines():
        line = line.split("#", 1)[0].strip()
        m = re.fullmatch(r"\[([^\[\]]+)\]", line)
        if m:
            table = m.group(1)
            for kind in ("dependencies", "build-dependencies", "dev-dependencies"):
                if table.startswith("target.") and table.endswith("." + kind):
                    table = kind
            out.setdefault(table, [])
            continue
        m = re.match(r"([A-Za-z0-9_-]+)(\.[A-Za-z0-9_-]+)?\s*=", line)
        if m and table is not None:
            out[table].append(m.group(1))
    return out


def members_of(text):
    m = re.search(r"^members\s*=\s*\[(.*?)\]", text, re.S | re.M)
    return re.findall(r'"([^"]+)"', m.group(1)) if m else []


def package_name(text):
    m = re.search(r'^\[package\][^\[]*?^name\s*=\s*"([^"]+)"', text, re.S | re.M)
    return m.group(1)


def main():
    root = Path(sys.argv[1] if len(sys.argv) > 1 else Path(__file__).resolve().parent.parent)
    manifests = {}
    for m in members_of((root / "Cargo.toml").read_text()):
        text = (root / m / "Cargo.toml").read_text()
        manifests[package_name(text)] = (m, tables(text))
    bad = []
    for name, (path, doc) in sorted(manifests.items()):
        if name not in ALLOWED:
            bad.append(f"{path}: {name} has no place in the layout (scripts/check-crate-layers.py ALLOWED)")
            continue
        for table in ("dependencies", "build-dependencies"):
            for dep in doc.get(table, []):
                if dep in manifests and dep not in ALLOWED[name]:
                    bad.append(f"{path}/Cargo.toml [{table}]: {name} may not depend on {dep}")
    for line in bad:
        print(f"error: {line}", file=sys.stderr)
    if bad:
        print("The layering rule: a crate depends only on crates above it in "
              "docs/superpowers/specs/2026-10-16-workspace-layout-design.md §2.1.", file=sys.stderr)
        return 1
    print(f"crate layers: {len(manifests)} crates, every dependency in order")
    return 0


if __name__ == "__main__":
    sys.exit(main())
