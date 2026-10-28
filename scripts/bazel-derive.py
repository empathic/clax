#!/usr/bin/env python3
"""What the Bazel build derives from the Cargo workspace.

Run by scripts/bazel-sync.sh, which writes or compares the output:

    scripts/bazel-derive.py workspace   -> bazel/workspace.bzl
    scripts/bazel-derive.py release-files -> bazel/release_files.bzl
    scripts/bazel-derive.py browsers    -> bazel/playwright/browsers.bzl
                                        (downloads each archive once, for its
                                        checksum; --check compares versions)
    scripts/bazel-derive.py features    fails unless every third-party crate
                                        Bazel builds has the version and
                                        features Cargo gives it (features())

workspace: each workspace crate's version, targets, default features, path
dependencies and dev-dependencies (refusing one that turns features on),
whether it has a build script, and the rustc flags of its own [lints.rust]
when it has one; and the rustc flags of [workspace.lints.rust].
"""

import json
import os
import re
import subprocess
import sys
import tomllib

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
HOSTS = [
    "aarch64-apple-darwin",
    "aarch64-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-gnu",
]


def read(path):
    return open(os.path.join(ROOT, path)).read()


def fail(message):
    sys.exit("error: scripts/bazel-derive.py: " + message + "; run just bazel-sync and commit the result")


def metadata():
    out = subprocess.run(
        ["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"],
        cwd=ROOT, check=True, capture_output=True, text=True).stdout
    return json.loads(out)


def starlark(value):
    """JSON is Starlark but for its booleans."""
    text = json.dumps(value, indent=4, sort_keys=True)
    return text.replace(": true", ": True").replace(": false", ": False")


def lint_flags(lints=None, table="[workspace.lints.rust]"):
    """rustc flags for a [lints.rust] table: the workspace's by default."""
    if lints is None:
        lints = tomllib.loads(read("Cargo.toml")).get("workspace", {}).get("lints", {}).get("rust", {})
    flags = []
    for name, spec in sorted(lints.items()):
        level = spec if isinstance(spec, str) else spec.get("level")
        extra = {} if isinstance(spec, str) else {k: v for k, v in spec.items() if k not in ("level", "priority")}
        if name == "unexpected_cfgs":
            flags += ["--check-cfg=" + c for c in extra.pop("check-cfg", [])]
        if extra or level not in ("forbid", "deny", "warn", "allow"):
            sys.exit("error: scripts/bazel-derive.py: %s %s = %r: add its mapping to scripts/bazel-derive.py" % (table, name, spec))
        flags.append({"forbid": "-F", "deny": "-D", "warn": "-W", "allow": "-A"}[level] + name.replace("-", "_"))
    return flags


def workspace():
    meta = metadata()
    root = meta["workspace_root"]

    def rel(path):
        return os.path.relpath(path, root)

    packages = {p["name"]: p for p in meta["packages"]}

    def lib_of(p):
        # A cdylib-only library (crates/clax-worker) is a library to Bazel
        # too: its unit tests and its dependents' builds link it as an rlib.
        libs = [t for t in p["targets"] if {"lib", "rlib", "cdylib"} & set(t["kind"])]
        if len(libs) > 1:
            sys.exit("error: scripts/bazel-derive.py: %s has more than one library target" % p["name"])
        return libs[0] if libs else None

    def label(p):
        return "//%s:%s" % (rel(os.path.dirname(p["manifest_path"])), lib_of(p)["name"])

    crates = {}
    for name in sorted(packages):
        p = packages[name]
        directory = rel(os.path.dirname(p["manifest_path"]))
        lib = lib_of(p)

        # The features `default` turns on, without the optional dependencies
        # they pull in (crate_universe supplies every one of those), and the
        # optional workspace crates they turn on (`dep:<name>`), which build
        # as plain path dependencies.
        enabled, todo = set(), ["default"] if "default" in p["features"] else []
        optional_on = set()
        while todo:
            feature = todo.pop()
            if feature in enabled or feature not in p["features"]:
                continue
            enabled.add(feature)
            for f in p["features"][feature]:
                if f.startswith("dep:"):
                    optional_on.add(f[len("dep:"):])
            todo.extend(f for f in p["features"][feature] if ":" not in f and "/" not in f)

        deps = {"deps": set(), "dev_deps": set()}
        for d in p["dependencies"]:
            if not d.get("path"):
                continue
            if d["kind"] == "build":
                sys.exit("error: scripts/bazel-derive.py: %s has a path build-dependency" % name)
            if d["optional"] and d["kind"] != "dev" and d["name"] not in optional_on:
                # Off by default: Bazel builds the default features alone.
                continue
            if not d["uses_default_features"] or d["rename"] or (
                d["optional"] and d["name"] not in optional_on
            ):
                sys.exit("error: scripts/bazel-derive.py: %s -> %s: only a plain path dependency is supported" % (name, d["name"]))
            target = packages[d["name"]]
            if d["kind"] != "dev" and d["features"]:
                sys.exit("error: scripts/bazel-derive.py: %s -> %s: features on a normal path dependency are not supported" % (name, d["name"]))
            if d["kind"] == "dev" and d["features"]:
                # Tests build the libraries the binary links: a feature
                # turned on for tests would compile its crate a second
                # time. Test support lives in clax-testkit instead.
                sys.exit(
                    "error: scripts/bazel-derive.py: %s turns on features %s of %s in a"
                    " dev-dependency; tests link the same libraries as the binary, so put"
                    " test support in crates/clax-testkit (or behind debug_assertions)"
                    % (name, ", ".join(d["features"]), d["name"]))
            if d["name"] == name:
                sys.exit("error: scripts/bazel-derive.py: %s depends on itself" % name)
            deps["dev_deps" if d["kind"] == "dev" else "deps"].add(label(target))
        deps["dev_deps"] -= deps["deps"]

        crates[directory] = {
            "name": name,
            "version": p["version"],
            "edition": p["edition"],
            "lib": lib["name"] if lib else None,
            "lib_root": rel(lib["src_path"])[len(directory) + 1:] if lib else None,
            "bins": {
                t["name"]: rel(t["src_path"])[len(directory) + 1:]
                for t in p["targets"] if "bin" in t["kind"]
            },
            "tests": {
                t["name"]: rel(t["src_path"])[len(directory) + 1:]
                for t in p["targets"] if "test" in t["kind"]
            },
            "default_features": sorted(enabled),
            "features": sorted(p["features"]),
            "deps": sorted(deps["deps"]),
            "dev_deps": sorted(deps["dev_deps"]),
            "has_build_script": any("custom-build" in t["kind"] for t in p["targets"]),
        }

        # A crate with a [lints.rust] of its own rather than the
        # workspace's (scripts/check-no-unsafe.sh says which may).
        with open(p["manifest_path"], "rb") as f:
            lints = tomllib.load(f).get("lints", {})
        if lints.get("workspace") is not True and "rust" in lints:
            crates[directory]["lint_flags"] = lint_flags(lints["rust"], "%s/Cargo.toml [lints.rust]" % directory)

    out = [
        '"""The Cargo workspace, as bazel/cargo.bzl needs it.',
        "",
        "Generated by scripts/bazel-sync.sh from cargo metadata. Do not edit.",
        '"""',
        "",
        "CRATES = " + starlark(crates).replace(": null", ": None"),
        "",
        "# [workspace.lints.rust] in Cargo.toml, as rustc flags.",
        "WORKSPACE_RUSTC_FLAGS = " + starlark(lint_flags()),
    ]
    return "\n".join(out) + "\n"


def label(path):
    """The Bazel label of a repository file: its nearest package's."""
    parts = path.split("/")
    for i in range(len(parts) - 1, -1, -1):
        package = "/".join(parts[:i])
        if os.path.isfile(os.path.join(ROOT, package, "BUILD.bazel")):
            return "//%s:%s" % (package, "/".join(parts[i:]))
    fail("no package holds " + path)


def release_files():
    """The release script test's files (scripts/test-release.files) as labels."""
    paths = [l.strip() for l in read("scripts/test-release.files").splitlines() if l.strip() and not l.startswith("#")]
    out = [
        '"""The files scripts/test-release.sh copies, as labels.',
        "",
        "Generated by scripts/bazel-sync.sh from scripts/test-release.files. Do not edit.",
        '"""',
        "",
        "RELEASE_FILES = " + json.dumps(sorted(label(p) for p in paths), indent=4),
    ]
    return "\n".join(out) + "\n"


# Playwright's names for the hosts Bazel builds on, and Chrome for
# Testing's for the same (playwright-core's registry, cftUrl).
CFT_HOSTS = {
    "mac-arm64": "mac-arm64",
    "mac-x64": "mac-x64",
    "linux-x64": "linux64",
    "linux-arm64": "linux-arm64",
}

# Playwright's ffmpeg archive for each of those hosts (its registry's
# download paths, builds/ffmpeg/<revision>/<archive>.zip).
FFMPEG_HOSTS = {
    "mac-arm64": "ffmpeg-mac-arm64",
    "mac-x64": "ffmpeg-mac",
    "linux-x64": "ffmpeg-linux",
    "linux-arm64": "ffmpeg-linux-arm64",
}


def playwright_version():
    """The playwright-core version web/pnpm-lock.yaml locks."""
    found = set(re.findall(r"^  playwright-core@([0-9][^:]*):$", read("web/pnpm-lock.yaml"), re.M))
    if len(found) != 1:
        fail("web/pnpm-lock.yaml should lock exactly one playwright-core, found %s" % sorted(found))
    return found.pop()


def browsers_json(version):
    """playwright-core's browsers.json at `version`, from its npm tarball."""
    import io
    import tarfile
    import urllib.request
    url = "https://registry.npmjs.org/playwright-core/-/playwright-core-%s.tgz" % version
    data = urllib.request.urlopen(url).read()
    with tarfile.open(fileobj=io.BytesIO(data)) as tar:
        return json.load(tar.extractfile("package/browsers.json"))


def sha256_of(url):
    import hashlib
    import urllib.request
    h = hashlib.sha256()
    with urllib.request.urlopen(url) as r:
        for chunk in iter(lambda: r.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def browsers():
    version = playwright_version()
    found = {b["name"]: b for b in browsers_json(version)["browsers"]}
    out = {}
    for name, prefix, dirname in (
        ("chromium", "chrome", "chromium"),
        ("chromium-headless-shell", "chrome-headless-shell", "chromium_headless_shell"),
    ):
        b = found[name]
        for host, cft in CFT_HOSTS.items():
            url = "https://cdn.playwright.dev/builds/cft/%s/%s/%s-%s.zip" % (b["browserVersion"], cft, prefix, cft)
            out.setdefault(host, []).append({
                "dir": "%s-%s" % (dirname, b["revision"]),
                "sha256": sha256_of(url),
                "url": url,
            })
    # Playwright's own ffmpeg, which it records a page's video with
    # (`recordVideo`: the reel's fast mode, web/reel/camera.ts); its
    # installer fetches it with every browser.
    ffmpeg = found["ffmpeg"]
    for host, archive in FFMPEG_HOSTS.items():
        url = "https://cdn.playwright.dev/builds/ffmpeg/%s/%s.zip" % (ffmpeg["revision"], archive)
        out[host].append({"dir": "ffmpeg-%s" % ffmpeg["revision"], "sha256": sha256_of(url), "url": url})
    return "\n".join([
        '"""The Chromium builds the locked Playwright runs, and its ffmpeg, by host.',
        "",
        "Generated by scripts/bazel-sync.sh from web/pnpm-lock.yaml (playwright-core's",
        "browsers.json): the archives Playwright's installer downloads, with their",
        "checksums. Do not edit.",
        '"""',
        "",
        "PLAYWRIGHT_VERSION = " + json.dumps(version),
        "",
        "BROWSERS = " + json.dumps(out, indent=4, sort_keys=True),
    ]) + "\n"


def bazel_features(hub, triple):
    """{crate-version: features} Bazel builds a repository's crates with."""
    lock = json.loads(read("MODULE.bazel.lock"))
    extension = [v for k, v in lock["moduleExtensions"].items() if k.endswith("extensions.bzl%crate")]
    if len(extension) != 1:
        fail("MODULE.bazel.lock has no crate_universe section")
    out = {}
    for name, spec in extension[0]["general"]["generatedRepoSpecs"].items():
        if not name.startswith(hub + "__"):
            continue
        build = spec["attributes"].get("build_file_content", "")
        rule = re.search(r"\nrust_(?:library|proc_macro)\(.*?\n\)\n", build, re.S)
        if not rule:
            continue
        # crate_features = [common] + select({platform: [more]}), either part optional.
        found = re.search(r"\n    crate_features = (\[.*?\])?(?: \+ )?(?:select\(\{(.*?)\n    \}\))?,\n", rule.group(0), re.S)
        features = set()
        if found and found.group(1):
            features |= set(re.findall(r'"([^"]+)"', found.group(1)))
        if found and found.group(2):
            platform = re.search(r'"@rules_rust//rust/platform:%s": \[(.*?)\]' % re.escape(triple), found.group(2), re.S)
            if platform:
                features |= set(re.findall(r'"([^"]+)",', platform.group(1)))
        out[name[len(hub) + 2:]] = features
    return out


def cargo_features(args):
    """{crate-version: features} of a Cargo build, by `cargo tree`."""
    tree = subprocess.run(
        ["cargo", "tree", "--locked", "--prefix", "none", "-f", "{p}|{f}"] + args,
        cwd=ROOT, check=True, capture_output=True, text=True).stdout
    out = {}
    for line in tree.splitlines():
        if "|" not in line:
            continue
        package, features = line.split("|")
        name, version = package.split()[:2]
        # A workspace member (a Clax crate, or a patched copy of a
        # third-party one under third_party/) is built from its own BUILD
        # file, not from crate_universe's repository.
        if name.startswith("clax-") or "(" + ROOT + "/" in package:
            continue
        # A line cargo has printed before ends " (*)".
        features = features.replace(" (*)", "")
        # Bazel names a version's build metadata with "-" for "+".
        out.setdefault(name + "-" + version.lstrip("v").replace("+", "-"), set()).update(f for f in features.split(",") if f)
    return out


# Where Bazel's repository has more than Cargo gives a crate. crate_universe
# keeps one feature list per crate and platform, so what a build script or a
# proc macro asks of a crate is also what the binary gets of it; Cargo
# builds the crate twice. Each entry is such a case, accepted for every
# version of the crate; the check fails on any other difference and on an
# entry that no longer applies. docs/bazel.md, "Differences from Cargo".
NATIVE_EXTRA = {
    # The Bazel debug build embeds the web UI (spec B11): MODULE.bazel adds
    # debug-embed to rust-embed and its two halves.
    "rust-embed": {"debug-embed"},
    "rust-embed-impl": {"debug-embed"},
    "rust-embed-utils": {"debug-embed"},
}


def features():
    """Fails unless Bazel's third-party crates are Cargo's: on each host, the
    workspace build (`cargo tree --workspace`, with dev-dependencies) but
    for NATIVE_EXTRA."""
    problems = []
    for host in HOSTS:
        bazel = bazel_features("crates", host)
        cargo = cargo_features(["--workspace", "--target", host, "-e", "normal,dev,build"])
        used = set()
        for crate in sorted(cargo):
            name = crate.rsplit("-", 1)[0]
            if crate not in bazel:
                problems.append("crates (%s): Cargo builds %s, Bazel does not" % (host, crate))
                continue
            missing, extra = cargo[crate] - bazel[crate], bazel[crate] - cargo[crate]
            if missing:
                problems.append("crates (%s): %s lacks %s under Bazel" % (host, crate, sorted(missing)))
            if not extra <= NATIVE_EXTRA.get(name, set()):
                problems.append("crates (%s): %s has %s more under Bazel than under Cargo; NATIVE_EXTRA in scripts/bazel-derive.py accepts %s" % (
                    host, crate, sorted(extra), sorted(NATIVE_EXTRA.get(name, set())) or "nothing"))
            if extra:
                used.add(name)
        for name in sorted(set(NATIVE_EXTRA) - used):
            problems.append("crates (%s): NATIVE_EXTRA lists %s, which has nothing extra any more; remove it" % (host, name))
    for problem in problems:
        print("error: " + problem, file=sys.stderr)
    if problems:
        fail("Bazel's dependencies differ from Cargo's")
    return ""


if __name__ == "__main__":
    kinds = {"workspace": workspace, "release-files": release_files, "browsers": browsers, "features": features}
    if sys.argv[1:] == ["playwright-version"]:
        print(playwright_version())
        sys.exit(0)
    if len(sys.argv) != 2 or sys.argv[1] not in kinds:
        sys.exit(__doc__)
    sys.stdout.write(kinds[sys.argv[1]]())
