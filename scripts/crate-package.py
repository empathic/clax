#!/usr/bin/env python3
"""Assembles the `clax` package for crates.io in target/crate-package/clax.

    scripts/crate-package.py

crates.io takes one package per crate, holding only files under that
package, so the workspace's `clax` binary cannot be published as it stands:
clax-cli depends on eleven workspace crates by path, and clax-embedded
embeds the web UI, the extension and the plugins from outside its own
directory. This script writes a single package instead, which publishes
nothing internal under a name of its own:

- clax-cli's `src/` is the package's binary (`clax`), and every workspace
  crate it depends on, directly or not, is copied beside it under
  `crates/<name>/src/` and declared a module of the binary
  (`crate::clax_core`, ...). In those crates' sources `crate::` becomes
  `crate::<module>::`, and in every bundled source `clax_<name>::` becomes
  `crate::clax_<name>::`.
- The repository layout is kept, so the sources' relative `include_str!`
  and `include_bytes!` paths resolve: every file they name that exists is
  copied to the same path. The embedded directories and files named in
  crates/clax-embedded/build.rs are copied too, and `web/dist` and
  `web/dist-extension` (build them first with `just web`).
- The dependencies are the union of the bundled crates' normal
  dependencies, as `cargo metadata` resolves them. Where two crates require
  different versions of one dependency, the later crate's is renamed
  `<name>_<version>` (`package = "<name>"`) and its sources' `<name>::`
  paths follow. The workspace's
  Cargo.lock is copied so `cargo install --locked` builds what CI tests.
- scripts/crate-package/build.rs replaces the bundled crates' build
  scripts; the commit the package is assembled from goes in BUILD_COMMIT.
- An optional dependency is left out, with the features that turn it on:
  the package builds without them (clax-server's `native-sandbox`, whose
  patched QuickJS crates are not on crates.io, so the published `clax`
  answers ClaxBot's scripts `unavailable`). In the bundled sources a
  `feature = "<name>"` of a feature left out reads as `any()`, never true.

The package is "GPL-2.0-only AND AGPL-3.0-only": its Rust code is GPL-2.0
(the dual-licensed shared crates under that license) and the web UI it
embeds is AGPL-3.0 (LICENSE). It never uploads anything; `just
crate-package` runs this script, then `cargo package` to check that the
package builds. Run from any directory.
"""
import json
import re
import shutil
import subprocess
import sys
import tomllib
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
OUT = REPO / "target" / "crate-package" / "clax"
BIN_CRATE = "clax-cli"

PACKAGE = {
    "name": "clax",
    "description": "A home for the HTML pages your coding agents publish: "
    "every version kept, comments sent back to the agent",
    "homepage": "https://github.com/empathic/clax",
    "readme": "README.md",
    "keywords": ["agents", "artifacts", "mcp", "claude", "codex"],
    "categories": ["command-line-utilities", "development-tools", "web-programming::http-server"],
    "license": "GPL-2.0-only AND AGPL-3.0-only",
}

# What never travels in the package.
SKIP_NAMES = {".qual", "BUILD.bazel", ".DS_Store", "node_modules"}


def die(msg):
    print(f"crate-package: {msg}", file=sys.stderr)
    sys.exit(1)


def metadata():
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps", "--offline"],
        cwd=REPO, check=True, capture_output=True, text=True,
    ).stdout
    return json.loads(out)


def bundled_crates(packages):
    """The workspace crates the binary reaches through normal dependencies,
    the binary's own first."""
    by_name = {p["name"]: p for p in packages}
    order, todo = [], [BIN_CRATE]
    while todo:
        name = todo.pop(0)
        if name in order:
            continue
        order.append(name)
        for d in by_name[name]["dependencies"]:
            if d["kind"] is None and d["name"] in by_name and not d["optional"]:
                todo.append(d["name"])
    return [by_name[n] for n in order]


def dropped_features(crates):
    """{crate: [feature]}: the features of the bundled crates that turn on
    an optional dependency, which the package leaves out, with every
    feature that turns one of them on."""
    out = {}
    for c in crates:
        optional = {d["name"] for d in c["dependencies"] if d["optional"]}
        features = c.get("features", {})
        dropped = {f for f, on in features.items()
                   if any(x.split(":", 1)[-1].split("/")[0] in optional for x in on
                          if x.startswith("dep:") or "/" in x)}
        changed = True
        while changed:
            changed = False
            for f, on in features.items():
                if f not in dropped and any(x in dropped for x in on):
                    dropped.add(f)
                    changed = True
        if dropped:
            out[c["name"]] = sorted(dropped)
    return out


def merged_dependencies(crates, workspace):
    """({target or None: {key: spec}} over the bundled crates' normal
    dependencies outside the workspace, {crate: {name: renamed key}})."""
    tables, renames = {}, {}
    required = {}
    for c in crates:
        for d in c["dependencies"]:
            if d["kind"] is not None or d["name"] in workspace:
                continue
            if d.get("source") != "registry+https://github.com/rust-lang/crates.io-index":
                die(f"{c['name']} depends on {d['name']} from {d.get('source')}, not crates.io")
            if d["optional"]:
                # Left out with its feature (see the module documentation).
                continue
            key = d["rename"] or d["name"]
            first = required.setdefault(key, d["req"])
            if first != d["req"]:
                new = key + "_" + re.sub(r"\W+", "_", d["req"]).strip("_")
                renames.setdefault(c["name"], {})[key] = new
                d = dict(d, rename=new)
                key = new
            table = tables.setdefault(d["target"], {})
            spec = table.get(key)
            if spec is None:
                table[key] = {
                    "package": d["name"] if d["rename"] else None,
                    "version": d["req"],
                    "features": set(d["features"]),
                    "default-features": d["uses_default_features"],
                    "from": c["name"],
                }
            else:
                if spec["version"] != d["req"]:
                    die(f"{key} is required as {spec['version']} by {spec['from']} "
                        f"and as {d['req']} by {c['name']}")
                spec["features"] |= set(d["features"])
                spec["default-features"] |= d["uses_default_features"]
    return tables, renames


def toml_str(s):
    return json.dumps(s)


def dependency_line(key, spec):
    parts = [f"version = {toml_str(spec['version'])}"]
    if spec["package"]:
        parts.append(f"package = {toml_str(spec['package'])}")
    if not spec["default-features"]:
        parts.append("default-features = false")
    if spec["features"]:
        parts.append("features = [" + ", ".join(toml_str(f) for f in sorted(spec["features"])) + "]")
    return f"{key} = {{ {', '.join(parts)} }}"


def manifest(version, edition, rust_version, repository, tables, lints, files):
    lines = ["[package]", f"name = {toml_str(PACKAGE['name'])}",
             f"version = {toml_str(version)}", f"edition = {toml_str(edition)}",
             f"rust-version = {toml_str(rust_version)}",
             f"license = {toml_str(PACKAGE['license'])}",
             f"description = {toml_str(PACKAGE['description'])}",
             f"repository = {toml_str(repository)}",
             f"homepage = {toml_str(PACKAGE['homepage'])}",
             f"readme = {toml_str(PACKAGE['readme'])}",
             "keywords = [" + ", ".join(toml_str(k) for k in PACKAGE["keywords"]) + "]",
             "categories = [" + ", ".join(toml_str(k) for k in PACKAGE["categories"]) + "]",
             'build = "build.rs"',
             "# Every file the script wrote, by name: Cargo leaves out dotfiles",
             "# (the plugins' manifests among them) unless they are named.",
             "include = [" + ", ".join(toml_str("/" + f) for f in files) + "]",
             "autobins = false", "autoexamples = false", "autotests = false", "autobenches = false",
             "",
             "# Its own workspace: the package is assembled inside the Clax repository.",
             "[workspace]", "",
             "[[bin]]", 'name = "clax"', f'path = "crates/{BIN_CRATE}/src/main.rs"', ""]
    for target in sorted(tables, key=lambda t: (t is not None, t or "")):
        lines.append("[dependencies]" if target is None else f"[target.'{target}'.dependencies]")
        for key in sorted(tables[target]):
            lines.append(dependency_line(key, tables[target][key]))
        lines.append("")
    lines.append(lints)
    return "\n".join(lines)


def workspace_lints():
    """The `[workspace.lints.*]` tables of the root Cargo.toml, as the
    package's `[lints.*]`."""
    text = (REPO / "Cargo.toml").read_text()
    out, keep = [], False
    for line in text.splitlines():
        if line.startswith("["):
            keep = line.startswith("[workspace.lints.")
            if keep:
                line = line.replace("[workspace.lints.", "[lints.", 1)
        if keep:
            out.append(line)
    return "\n".join(out).strip() + "\n"


def copy_tree(src, dst):
    for path in sorted(src.rglob("*")):
        rel = path.relative_to(src)
        if any(part in SKIP_NAMES for part in rel.parts) or path.is_dir():
            continue
        target = dst / rel
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(path, target)


def copy_file(rel):
    src = REPO / rel
    if not src.is_file():
        die(f"{rel} is missing")
    (OUT / rel).parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(src, OUT / rel)


def rewrite_sources(crates, renames, dropped=None):
    modules = [c["name"].replace("-", "_") for c in crates if c["name"] != BIN_CRATE]
    external = re.compile(r"(?<![\w:$])(" + "|".join(modules) + r")::")
    for c in crates:
        module = c["name"].replace("-", "_")
        for path in sorted((OUT / "crates" / c["name"] / "src").rglob("*.rs")):
            text = path.read_text()
            if c["name"] != BIN_CRATE:
                text = re.sub(r"(?<![\w$])crate::", f"crate::{module}::", text)
            text = external.sub(r"crate::\1::", text)
            for old, new in renames.get(c["name"], {}).items():
                text = re.sub(r"(?<![\w:$])" + re.escape(old) + "::", new + "::", text)
            for f in (dropped or {}).get(c["name"], []):
                text = re.sub(r'feature\s*=\s*"' + re.escape(f) + '"', "any()", text)
            path.write_text(text)
    main = OUT / "crates" / BIN_CRATE / "src" / "main.rs"
    decls = ["", "// The workspace crates the binary bundles (scripts/crate-package.py)."]
    for c in crates:
        if c["name"] == BIN_CRATE:
            continue
        decls.append("#[allow(dead_code, unused_imports)]")
        decls.append(f'#[path = "../../{c["name"]}/src/lib.rs"]')
        decls.append(f"mod {c['name'].replace('-', '_')};")
    main.write_text(main.read_text() + "\n".join(decls) + "\n")


def included_files(crates):
    """Repository files the bundled sources name with include_str! or
    include_bytes!, outside every bundled src/ tree (copied and rewritten
    already), that exist. A missing one
    is only included by tests, which the package does not build."""
    pattern = re.compile(r'include_(?:str|bytes)!\(\s*"([^"]+)"\s*\)')
    srcs = [Path(c["manifest_path"]).parent / "src" for c in crates]
    found = set()
    for src in srcs:
        for path in src.rglob("*.rs"):
            for rel in pattern.findall(path.read_text()):
                target = (path.parent / rel).resolve()
                if target.is_file() and not any(target.is_relative_to(s) for s in srcs):
                    found.add(target.relative_to(REPO))
    return sorted(found)


def embedded_paths():
    """The DIRS and FILES that crates/clax-embedded/build.rs names."""
    text = (REPO / "crates/clax-embedded/build.rs").read_text()
    lists = {}
    for name in ("DIRS", "FILES"):
        m = re.search(name + r": &\[&str\] = &\[(.*?)\];", text, re.S)
        if not m:
            die(f"crates/clax-embedded/build.rs no longer has a {name} list")
        lists[name] = re.findall(r'"([^"]+)"', m.group(1))
    return lists["DIRS"], lists["FILES"]


def main():
    meta = metadata()
    packages = meta["packages"]
    workspace = {p["name"] for p in packages}
    crates = bundled_crates(packages)
    bin_pkg = crates[0]

    # crates.io ignores [patch], and third_party/ crates are not on it: a
    # package that reached either would not build what was tested.
    checked = subprocess.run([sys.executable, str(REPO / "scripts/check-crate-layers.py"), "--publishable", str(REPO)])
    if checked.returncode != 0:
        die("clax-cli cannot be published as it stands (above); see scripts/check-crate-layers.py --publishable")

    for d in ("web/dist", "web/dist-extension"):
        if not (REPO / d).is_dir() or not any((REPO / d).iterdir()):
            die(f"{d} is empty: build the web UI and the extension first (`just web`)")
    if not (REPO / "web/dist/index.html").is_file():
        die("web/dist has no index.html: build the web UI first (`just web`)")

    if OUT.exists():
        shutil.rmtree(OUT)
    OUT.mkdir(parents=True)

    for c in crates:
        copy_tree(Path(c["manifest_path"]).parent / "src", OUT / "crates" / c["name"] / "src")
    tables, renames = merged_dependencies(crates, workspace)
    dropped = dropped_features(crates)
    rewrite_sources(crates, renames, dropped)
    repository = tomllib.loads((REPO / "Cargo.toml").read_text())["workspace"]["package"]["repository"]

    dirs, files = embedded_paths()
    for d in dirs:
        if (REPO / d).is_dir():
            copy_tree(REPO / d, OUT / d)
    for f in files + [str(p) for p in included_files(crates)]:
        copy_file(f)
    copy_tree(REPO / "web/dist", OUT / "web/dist")
    key = REPO / "web/extension/key/key.pub.b64"
    if key.is_file():
        copy_file("web/extension/key/key.pub.b64")
    for f in ("README.md", "LICENSE", "NOTICE", "Cargo.lock"):
        copy_file(f)
    copy_tree(REPO / "LICENSES", OUT / "LICENSES")
    shutil.copyfile(REPO / "scripts/crate-package/build.rs", OUT / "build.rs")

    commit = subprocess.run(["git", "rev-parse", "HEAD"], cwd=REPO, check=True,
                            capture_output=True, text=True).stdout.strip()
    (OUT / "BUILD_COMMIT").write_text(commit + "\n")

    toolchain = (REPO / "rust-toolchain.toml").read_text()
    m = re.search(r'channel = "(\d+\.\d+)', toolchain)
    if not m:
        die("rust-toolchain.toml names no numbered channel")
    files = sorted(str(f.relative_to(OUT)) for f in OUT.rglob("*") if f.is_file())
    (OUT / "Cargo.toml").write_text(manifest(
        bin_pkg["version"], bin_pkg["edition"], m.group(1), repository,
        tables, workspace_lints(), files + ["Cargo.toml"]))

    names = ", ".join(c["name"] for c in crates[1:])
    for name, features in sorted(dropped.items()):
        print(f"crate-package: {name} without {', '.join(features)}")
    print(f"crate-package: wrote {OUT.relative_to(REPO)} (clax {bin_pkg['version']}, "
          f"commit {commit[:12]}), bundling {names}")


if __name__ == "__main__":
    main()
