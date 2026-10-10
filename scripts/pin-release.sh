#!/usr/bin/env bash
# Pins the plugins to a published Clax release: fetches the release's
# SHA256SUMS and writes PINNED_VERSION and the four per-target SHA256 values
# into scripts/ensure-clax.sh and every plugin's copy of it (the Claude Code,
# Codex, Grok Build and Pi plugins), so that the plugins download and run that
# release when neither CLAX_BIN nor the `bin` setting names a binary.
#
# Usage: pin-release.sh vX.Y.Z
#
# It refuses a release newer than this checkout's version (the plugins'
# skills describe this checkout's clax), and a SHA256SUMS that does not list
# all four archives, clax-X.Y.Z-<target>.tar.gz. Files are written only once
# every value has been found. Version fields are not changed: the plugins'
# version is the checkout's (scripts/bump-version.sh), and a harness updates
# an installed plugin only when that version changes, so land the pin in the
# same push as the version it ships with (see docs/usage.md, "Releasing").
#
# Environment:
#   CLAX_RELEASE_BASE_URL   release download base; SHA256SUMS comes from
#                           <base>/vX.Y.Z/SHA256SUMS (default: the GitHub
#                           releases of empathic/clax); for tests
set -euo pipefail
cd "$(dirname "$0")/.."
[ $# = 1 ] || { echo "usage: pin-release.sh vX.Y.Z" >&2; exit 2; }
tag="$1"
[[ $tag =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "pin-release: '$tag' is not a release tag (vX.Y.Z)" >&2; exit 2; }
version="${tag#v}"
checkout="$(scripts/check-version.sh --print)"
newest="$(printf '%s\n%s\n' "$version" "${checkout%%-*}" | sort -t. -k1,1n -k2,2n -k3,3n | tail -1)"
if [ "$version" != "${checkout%%-*}" ] && [ "$newest" = "$version" ]; then
    echo "pin-release: $tag is newer than this checkout ($checkout); pin it from a checkout at $version or later" >&2
    exit 1
fi
base="${CLAX_RELEASE_BASE_URL:-https://github.com/empathic/clax/releases/download}"
url="$base/$tag/SHA256SUMS"
sums="$(mktemp)"
trap 'rm -f "$sums"' EXIT
code="$(curl -sSL --connect-timeout 10 --max-time 60 -o "$sums" -w '%{http_code}' "$url" 2>/dev/null)" \
    || { echo "pin-release: cannot download $url" >&2; exit 1; }
[ "$code" = 200 ] || { echo "pin-release: $url answered HTTP $code (is the release published?)" >&2; exit 1; }
python3 - "$version" "$sums" <<'PY'
import pathlib, re, sys
version, sums = sys.argv[1:3]

listed = {}
for line in pathlib.Path(sums).read_text().splitlines():
    parts = line.split()
    if len(parts) == 2 and re.fullmatch(r'[0-9a-f]{64}', parts[0]):
        listed[parts[1].lstrip("*")] = parts[0]
targets = {
    "AARCH64_APPLE_DARWIN": "aarch64-apple-darwin",
    "X86_64_APPLE_DARWIN": "x86_64-apple-darwin",
    "X86_64_UNKNOWN_LINUX_MUSL": "x86_64-unknown-linux-musl",
    "AARCH64_UNKNOWN_LINUX_MUSL": "aarch64-unknown-linux-musl",
}
values = {"PINNED_VERSION": version}
for var, target in targets.items():
    name = f"clax-{version}-{target}.tar.gz"
    if name not in listed:
        sys.exit(f"pin-release: SHA256SUMS of v{version} does not list {name}; nothing was changed")
    values[f"SHA256_{var}"] = listed[name]

files = ["scripts/ensure-clax.sh"] + [f"plugins/{p}/scripts/ensure-clax.sh" for p in ("claude-code", "clax", "clax-grok", "pi")]
pending = {}
for f in files:
    text = pathlib.Path(f).read_text()
    for var, value in values.items():
        text, n = re.subn(rf'^{var}="[^"]*"$', f'{var}="{value}"', text, count=1, flags=re.M)
        if n != 1:
            sys.exit(f"pin-release: {f} has no {var}=\"...\" line; nothing was changed")
    pending[f] = text
for f, text in pending.items():
    pathlib.Path(f).write_text(text)
for var, value in values.items():
    print(f"{var}={value}")
PY
echo "pinned clax $version in scripts/ensure-clax.sh and the plugins' copies"
