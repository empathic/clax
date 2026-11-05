#!/usr/bin/env bash
# Checks that this checkout raises the Clax version over a base ref: CI's
# `version` job runs it on a pull request into prod, with the base
# `origin/prod`.
#   check-version-bump.sh <base ref>
# Exits 0 when this checkout's version (scripts/check-version.sh --print,
# which also checks that every version file agrees) is X.Y.Z and greater
# than the version at <base ref> (its Cargo.toml's [workspace.package]
# version); otherwise 1, with a one-line reason on stderr. Reads only git
# and the working tree; fetching the base ref is the caller's.
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1
no() { echo "check-version-bump: $*" >&2; exit 1; }
[ $# = 1 ] || { echo "usage: check-version-bump.sh <base ref>" >&2; exit 2; }
base="$1"

git rev-parse --verify --quiet "$base^{commit}" >/dev/null \
    || no "the base ref $base is not in this repository (fetch it first)"
head="$(scripts/check-version.sh --print 2>/dev/null)" \
    || no "this checkout's version files disagree or name no release version (scripts/check-version.sh lists them)"
cargo="$(git show "$base:Cargo.toml" 2>/dev/null)" || no "$base has no Cargo.toml"

exec python3 - "$base" "$head" "$cargo" <<'PY'
import re, sys

base_ref, head, cargo = sys.argv[1:]

def no(reason):
    print(f"check-version-bump: {reason}", file=sys.stderr)
    sys.exit(1)

# Semver's X.Y.Z: no leading zeros, no pre-release or build suffix.
SEMVER = r'(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)'

def parse(version):
    m = re.fullmatch(SEMVER, version)
    return tuple(int(g) for g in m.groups()) if m else None

ws = re.search(r'^\[workspace\.package\]\s*$(.*?)(?=^\[|\Z)', cargo, re.M | re.S)
m = re.search(r'^version\s*=\s*"([^"]+)"', ws.group(1), re.M) if ws else None
if not m:
    no(f"{base_ref}'s Cargo.toml names no [workspace.package] version")
base = m.group(1)

if parse(head) is None:
    no(f"this checkout's version {head} is not X.Y.Z")
if parse(base) is None:
    no(f"{base_ref}'s version {base} is not X.Y.Z")
if parse(head) <= parse(base):
    no(f"this checkout's version {head} is not greater than {base_ref}'s {base}: run scripts/bump-version.sh")
print(f"version {head} raises {base_ref}'s {base}")
PY
