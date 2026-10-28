#!/usr/bin/env bash
# Writes a new Clax version into every place scripts/check-version.sh reads
# (the Kubernetes overlays' image tag included), regenerates the skills'
# tool blocks (which name the plugin version), then runs check-version.sh.
# Usage: bump-version.sh X.Y.Z
#
# Cargo.lock names the workspace crates' versions. It is rewritten here,
# offline, for every member whose manifest takes `version.workspace`, so a
# `--locked` build passes without a `cargo update`.
set -euo pipefail
cd "$(dirname "$0")/.."
[ $# = 1 ] || { echo "usage: bump-version.sh X.Y.Z" >&2; exit 2; }
python3 - "$1" <<'PY'
import re, sys, pathlib
new = sys.argv[1]
if not re.fullmatch(r'\d+\.\d+\.\d+(-[0-9A-Za-z.]+)?', new):
    sys.exit(f"{new!r} is not a release version (X.Y.Z or X.Y.Z-pre)")

# Every substitution is made in memory first; files are written only once all
# of them have matched, so a pattern that fails to match changes nothing.
pending = {}

def sub(path, pattern, count=1):
    text = pending[path] if path in pending else pathlib.Path(path).read_text()
    out, n = re.subn(pattern, lambda m: m.group(1) + new + m.group(3), text, count=count, flags=re.M)
    if n != count:
        sys.exit(f"{path}: expected {count} version(s) matching {pattern!r}, found {n}; no file was changed")
    pending[path] = out

def workspace_versioned():
    """The workspace members whose manifest takes the workspace version."""
    cargo = pathlib.Path("Cargo.toml").read_text()
    block = re.search(r'^members\s*=\s*\[(.*?)\]', cargo, re.M | re.S)
    if not block:
        sys.exit("Cargo.toml: no [workspace] members; no file was changed")
    names = []
    for member in re.findall(r'"([^"]+)"', block.group(1)):
        manifest = pathlib.Path(member, "Cargo.toml").read_text()
        if re.search(r'^version(\.workspace\s*=\s*true|\s*=\s*\{\s*workspace\s*=\s*true\s*\})', manifest, re.M):
            names.append(re.search(r'^name\s*=\s*"([^"]+)"', manifest, re.M).group(1))
    return names

sub("Cargo.toml", r'(\[workspace\.package\]\s*\nversion\s*=\s*")([^"]+)(")')
crates = workspace_versioned()
for crate in crates:
    sub("Cargo.lock", r'(^name = "%s"\nversion = ")([^"]+)(")' % re.escape(crate))
for f in ("plugins/claude-code/.claude-plugin/plugin.json", "plugins/clax/.codex-plugin/plugin.json", "plugins/clax-grok/.grok-plugin/plugin.json", "plugins/pi/package.json"):
    sub(f, r'(^  "version": ")([^"]+)(")')
sub(".claude-plugin/marketplace.json", r'("version": ")([^"]+)(")', count=2)
for f in ("scripts/ensure-clax.sh", "plugins/claude-code/scripts/ensure-clax.sh", "plugins/clax/scripts/ensure-clax.sh", "plugins/clax-grok/scripts/ensure-clax.sh", "plugins/pi/scripts/ensure-clax.sh"):
    sub(f, r'(^CLAX_VERSION=")([^"]+)(")')
# The clax-fleet image the Kubernetes overlays run, which the release tag's
# fleet-image workflow publishes, and the docs' examples of it.
for f in ("deploy/k8s/sqlite/kustomization.yaml", "deploy/k8s/postgres/kustomization.yaml"):
    sub(f, r'(^    newTag: ")([^"]+)(")')
sub("docs/kubernetes.md", r'(/deploy/k8s/postgres\?ref=v)([0-9][0-9A-Za-z.-]*)()')
sub("docs/kubernetes.md", r'(ghcr\.io/empathic/clax-fleet:)([0-9][0-9A-Za-z.-]*)()')
for path, text in pending.items():
    pathlib.Path(path).write_text(text)
PY
python3 scripts/sync-skill-tools.py
scripts/check-version.sh "v$1"
echo "bumped to $1"
