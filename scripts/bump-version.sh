#!/usr/bin/env bash
# Writes a new Clax version into every place scripts/check-version.sh reads,
# regenerates the skills' tool blocks (which name the plugin version), then
# runs check-version.sh. Usage: bump-version.sh X.Y.Z
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

sub("Cargo.toml", r'(\[workspace\.package\]\s*\nversion\s*=\s*")([^"]+)(")')
for crate in ("clax-core", "clax-server", "clax-cli", "clax-mcp", "clax-hooks"):
    sub("Cargo.lock", r'(^name = "%s"\nversion = ")([^"]+)(")' % re.escape(crate))
for f in ("plugins/claude-code/.claude-plugin/plugin.json", "plugins/clax/.codex-plugin/plugin.json", "plugins/clax-grok/.grok-plugin/plugin.json", "plugins/pi/package.json"):
    sub(f, r'(^  "version": ")([^"]+)(")')
sub(".claude-plugin/marketplace.json", r'("version": ")([^"]+)(")', count=2)
sub("plugins/pi/package-lock.json", r'(^  "version": ")([^"]+)(")')
sub("plugins/pi/package-lock.json", r'("": \{\n      "name": "@empathic/clax-pi",\n      "version": ")([^"]+)(")')
for f in ("scripts/ensure-clax.sh", "plugins/claude-code/scripts/ensure-clax.sh", "plugins/clax/scripts/ensure-clax.sh", "plugins/clax-grok/scripts/ensure-clax.sh"):
    sub(f, r'(^CLAX_VERSION=")([^"]+)(")')
for path, text in pending.items():
    pathlib.Path(path).write_text(text)
PY
python3 scripts/sync-skill-tools.py
scripts/check-version.sh "v$1"
echo "bumped to $1"
