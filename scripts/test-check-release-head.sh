#!/usr/bin/env bash
# Tests scripts/check-release-head.sh: a pull request into prod from
# release/X.Y.Z of this repository passes; one from another branch, a
# release branch whose name is not a version, or a fork fails, naming why.
set -uo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"
SCRIPT="$HERE/scripts/check-release-head.sh"
FAILED=0
pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1"; FAILED=1; }

# `expect <case> <want exit> <stderr pattern> <head ref> <head repository>`
expect() {
    local out rc
    out="$("$SCRIPT" "$4" "$5" empathic/clax 2>&1)"; rc=$?
    if [ "$rc" = "$2" ] && grep -qE -- "$3" <<< "$out"; then pass "$1"
    else fail "$1 (rc=$rc: $out)"; fi
}

expect "release/0.7.0 of this repository passes" 0 '^$' release/0.7.0 empathic/clax
expect "release/1.10.2 of this repository passes" 0 '^$' release/1.10.2 empathic/clax
expect "main is refused" 1 'main is not a release branch \(release/X\.Y\.Z\)' main empathic/clax
expect "a release branch whose name is not a version is refused" 1 'release/next is not a release branch \(release/X\.Y\.Z\)' release/next empathic/clax
expect "a prerelease name is refused" 1 'release/0\.7\.0-rc1 is not a release branch' release/0.7.0-rc1 empathic/clax
expect "a release branch of a fork is refused" 1 'comes from someone/clax, not empathic/clax' release/0.7.0 someone/clax
if "$SCRIPT" release/0.7.0 >/dev/null 2>&1; then fail "too few arguments is a usage error"
else [ "$?" = 2 ] && pass "too few arguments is a usage error" || fail "too few arguments exits 2"; fi

exit "$FAILED"
