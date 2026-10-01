#!/usr/bin/env bash
# Checks that every place a Clax version is written agrees.
#   check-version.sh           exit 0 when they agree; print each one when not
#   check-version.sh v1.2.3    also require the tag to be v<version>
#   check-version.sh --print   print the version (after checking)
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1
exec python3 - "$@" <<'PY'
import json, re, sys

def first(pattern, text):
    m = re.search(pattern, text, re.M)
    return m.group(1) if m else None

def load(path):
    with open(path) as f:
        return json.load(f)

cargo = open("Cargo.toml").read()
ws = re.search(r'^\[workspace\.package\]\s*$(.*?)(?=^\[|\Z)', cargo, re.M | re.S)
lock = open("Cargo.lock").read()
market = load(".claude-plugin/marketplace.json")
pi_lock = load("plugins/pi/package-lock.json")
versions = {
    "Cargo.toml [workspace.package]": first(r'^version\s*=\s*"([^"]+)"', ws.group(1)) if ws else None,
    "plugins/claude-code/.claude-plugin/plugin.json": load("plugins/claude-code/.claude-plugin/plugin.json").get("version"),
    "plugins/clax/.codex-plugin/plugin.json": load("plugins/clax/.codex-plugin/plugin.json").get("version"),
    ".claude-plugin/marketplace.json version": market.get("version"),
    ".claude-plugin/marketplace.json plugins[clax]": next((p.get("version") for p in market.get("plugins", []) if p.get("name") == "clax"), None),
    "plugins/pi/package.json": load("plugins/pi/package.json").get("version"),
    "plugins/pi/package-lock.json version": pi_lock.get("version"),
    'plugins/pi/package-lock.json packages[""]': pi_lock.get("packages", {}).get("", {}).get("version"),
    "scripts/ensure-clax.sh CLAX_VERSION": first(r'^CLAX_VERSION="([^"]+)"', open("scripts/ensure-clax.sh").read()),
}
for crate in ("clax-core", "clax-server", "clax-cli", "clax-mcp", "clax-hooks"):
    versions[f"Cargo.lock {crate}"] = first(r'^name = "%s"\nversion = "([^"]+)"' % re.escape(crate), lock)

args = sys.argv[1:]
values = set(versions.values())
if None in values or len(values) != 1:
    print("versions differ:", file=sys.stderr)
    for k, v in versions.items():
        print(f"  {k}: {v}", file=sys.stderr)
    sys.exit(1)
version = values.pop()
if not re.fullmatch(r'\d+\.\d+\.\d+(-[0-9A-Za-z.]+)?', version):
    sys.exit(f"{version!r} is not a release version (X.Y.Z or X.Y.Z-pre)")
if args == ["--print"]:
    print(version)
elif len(args) == 1:
    if args[0] != f"v{version}":
        sys.exit(f"the tag {args[0]} does not match the version {version} (expected v{version})")
elif args:
    sys.exit("usage: check-version.sh [vX.Y.Z | --print]")
PY
