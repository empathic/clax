#!/usr/bin/env python3
"""Fails when a workspace crate depends on a workspace crate it may not.

    scripts/check-crate-layers.py [ROOT]
    scripts/check-crate-layers.py --publishable [ROOT]

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

With --publishable it checks instead that the `clax` binary (clax-cli and
every workspace crate it reaches through normal dependencies) can be
published as scripts/crate-package.py assembles it: no `[patch]` section in
the workspace's Cargo.toml or .cargo/config.toml (crates.io ignores
`[patch]`, so a published package would build other code than the one
tested), and no crate under third_party/ (patched copies of third-party
crates, which are not on crates.io: the owner publishes them first, and the
package then depends on them by version).

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
# ClaxBot's run loop and its sandbox contract, which the routes and the
# daemon wire in.
BOT = STORE | SAMPLE | {"clax-bot", "clax-bot-sandbox"}
HTTP = REALTIME | SAMPLE | BOT | {"clax-http"}
MCP = CORE | {"clax-mcp"}
HOOKS = CORE | {"clax-hooks"}
SERVER = HTTP | MCP | {"clax-server", "clax-bot-js"}
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
    "clax-http": REALTIME | SAMPLE | BOT,
    # The native sandbox, optional (`native-sandbox`).
    "clax-server": HTTP | MCP | {"clax-bot-js"},
    "clax-commands": STORE | MCP | HOOKS,
    "clax-cli": SERVER | COMMANDS | {"clax-embedded"},
    # The core inside the Clax app's process, behind four C functions:
    # what `clax serve` runs, without its listener, and no command.
    "clax-embed": SERVER | {"clax-embedded"},
    "clax-testkit": SERVER | {"clax-fake-exe"},
    # The hosted Worker: the server's routes in a Durable Object, its
    # database on the SQLite VFS over the object's storage (clax-do-vfs,
    # which depends on no workspace crate).
    "clax-do-vfs": set(),
    "clax-worker": HTTP | {"clax-server", "clax-git", "clax-content-git", "clax-do-vfs"},
    # ClaxBot's code-mode sandbox: the contract (pure types, a trait and
    # the conformance suite, for both targets) and its native QuickJS
    # implementation over it.
    "clax-bot-sandbox": set(),
    "clax-bot-js": {"clax-bot-sandbox", "clax-rquickjs-core"},
    # ClaxBot's run loop: the script API, prompts, the step state machine
    # and its driver, over the store, the Messages client and the sandbox
    # contract (never a sandbox implementation: the server wires one in).
    "clax-bot": STORE | SAMPLE | {"clax-bot-sandbox"},
    # Patched copies of third-party crates under names of their own
    # (third_party/, docs/bazel.md, "Patched third-party crates").
    "clax-rquickjs-sys": set(),
    "clax-rquickjs-core": {"clax-rquickjs-sys"},
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


def publishable(root, meta):
    """Why the binary cannot be published as crate-package assembles it."""
    bad = []
    for config in ("Cargo.toml", ".cargo/config.toml"):
        path = root / config
        if path.is_file() and any(line.strip().startswith("[patch") for line in path.read_text().splitlines()):
            bad.append(f"{config} has a [patch] section, which crates.io ignores: a published "
                       "package would build other code than the one tested")
    members = set(meta["workspace_members"])
    packages = {p["name"]: p for p in meta["packages"] if p["id"] in members}
    seen, todo = set(), ["clax-cli"]
    while todo:
        name = todo.pop()
        if name in seen or name not in packages:
            continue
        seen.add(name)
        p = packages[name]
        path = Path(p["manifest_path"]).parent.relative_to(root)
        if path.parts and path.parts[0] == "third_party":
            bad.append(f"clax-cli reaches {name} ({path}), a patched copy of a third-party crate "
                       "that is not on crates.io: the owner publishes it first, and the package "
                       "depends on it by version")
        # An optional dependency is left out: the package builds without its
        # feature (crate-package.py drops it), as with clax-server's
        # `native-sandbox` and the patched QuickJS crates under it.
        todo.extend(
            d["name"]
            for d in p["dependencies"]
            if (d["kind"] or "normal") == "normal" and not d["optional"]
        )
    return bad


def main():
    args = sys.argv[1:]
    check_publishable = bool(args) and args[0] == "--publishable"
    if check_publishable:
        args = args[1:]
    root = Path(args[0] if args else Path(__file__).resolve().parent.parent).resolve()
    meta = metadata(root)
    if check_publishable:
        bad = publishable(root, meta)
        for line in bad:
            print(f"error: {line}", file=sys.stderr)
        if bad:
            return 1
        print("crate layers: clax-cli is publishable (no [patch], no third_party/ crate)")
        return 0
    members = set(meta["workspace_members"])
    packages = {p["name"]: p for p in meta["packages"] if p["id"] in members}
    bad = []
    for name, p in sorted(packages.items()):
        path = Path(p["manifest_path"]).parent.relative_to(root)
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
