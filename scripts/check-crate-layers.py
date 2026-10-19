#!/usr/bin/env python3
"""Fails when a workspace crate depends on a workspace crate it may not.

    scripts/check-crate-layers.py [ROOT]

The layout (docs/usage.md, "Developing Clax") orders the crates: each may
depend only on the crates above it, never sideways or up. ALLOWED below is
that order, crate by crate. The check
reads the workspace (default: the repository) through `cargo metadata
--no-deps`, so every way a manifest can name a dependency counts: a
`[dependencies]` line, a `[dependencies.<name>]` table, a
`[target.'<cfg>'.dependencies]` table and a renamed dependency (`package =
"…"`) alike. A normal or build dependency on a workspace crate not in the
crate's ALLOWED set fails it, as does a member ALLOWED does not know.
Dev-dependencies are not checked: a crate's tests may reach the test
support and the crates they drive (Bazel's derivation already refuses
features turned on there).

Cargo is `$CARGO` when set (the Bazel test sets it to the toolchain's),
else `cargo` on PATH.
"""
import json
import os
import subprocess
import sys
from pathlib import Path

# The async runtime seam (clax-rt, tokio natively, a Worker's event loop on
# wasm32) sits beside the core vocabulary: neither depends on a workspace
# crate, and every daemon crate may reach both.
CORE = {"clax-core", "clax-rt"}
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
    "clax-rt": set(),
    # Git objects, packs and smart HTTP without I/O, and the artifact
    # content kept in git over them (the hosted Worker's content store).
    "clax-git": set(),
    "clax-content-git": CORE | {"clax-git"},
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


def metadata(root):
    """`cargo metadata --no-deps` of the workspace at `root`, offline."""
    cargo = os.environ.get("CARGO") or "cargo"
    out = subprocess.run(
        [cargo, "metadata", "--format-version", "1", "--no-deps", "--offline",
         "--manifest-path", str(root / "Cargo.toml")],
        capture_output=True, text=True)
    if out.returncode != 0:
        sys.exit("error: cargo metadata failed:\n" + out.stderr)
    return json.loads(out.stdout)


def main():
    root = Path(sys.argv[1] if len(sys.argv) > 1 else Path(__file__).resolve().parent.parent)
    meta = metadata(root.resolve())
    members = set(meta["workspace_members"])
    packages = {p["name"]: p for p in meta["packages"] if p["id"] in members}
    bad = []
    for name, p in sorted(packages.items()):
        path = Path(p["manifest_path"]).parent.relative_to(root.resolve())
        if name not in ALLOWED:
            bad.append(f"{path}: {name} has no place in the layout (scripts/check-crate-layers.py ALLOWED)")
            continue
        for d in p["dependencies"]:
            kind = d["kind"] or "normal"
            if kind == "dev" or d["name"] not in packages:
                continue
            if d["name"] not in ALLOWED[name]:
                how = f" (as {d['rename']})" if d.get("rename") else ""
                where = f" for {d['target']}" if d.get("target") else ""
                bad.append(f"{path}/Cargo.toml: {name} may not depend on {d['name']}{how}"
                           f" ({kind} dependency{where})")
    for line in bad:
        print(f"error: {line}", file=sys.stderr)
    if bad:
        print("The layering rule: a crate depends only on the crates in its ALLOWED "
              "set in scripts/check-crate-layers.py (docs/usage.md, \"Developing Clax\").", file=sys.stderr)
        return 1
    print(f"crate layers: {len(packages)} crates, every dependency in order")
    return 0


if __name__ == "__main__":
    sys.exit(main())
