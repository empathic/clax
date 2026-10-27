#!/usr/bin/env bash
# Tests scripts/check-version-bump.sh in a scratch git repository holding a
# copy of the version files (scripts/test-release.files), whose commits
# stand for prod's tip: an equal, a lower and a higher version, versions
# compared as numbers, versions that are not X.Y.Z, version files that
# disagree, and a base ref that does not exist. No network; git reads no
# global or system config.
set -uo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"
T="$(cd "$(mktemp -d)" && pwd -P)"
trap 'rm -rf "$T"' EXIT
FAILED=0
pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1"; FAILED=1; }
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1

FILES="$(grep -v '^#' "$HERE/scripts/test-release.files") scripts/check-version-bump.sh"
for f in $FILES; do
    mkdir -p "$T/$(dirname "$f")"
    cp "$HERE/$f" "$T/$f"
done
g() { git -C "$T" -c user.name=test -c user.email=test@example.com -c commit.gpgsign=false "$@"; }
if ! { g init -q -b main && g add -A && g commit -qm base; }; then echo "FAIL: scratch repository"; exit 1; fi
g branch prod

bump() { (cd "$T" && scripts/bump-version.sh "$1" >/dev/null) || fail "bump-version.sh $1"; }
# `expect <case> <want exit> <stderr pattern> [base ref]`
expect() {
    local out rc
    out="$(cd "$T" && scripts/check-version-bump.sh "${4:-prod}" 2>&1)"; rc=$?
    if [ "$rc" = "$2" ] && echo "$out" | grep -E -- "$3" >/dev/null; then pass "$1"
    else fail "$1 (exit $rc: $out)"; fi
}

V="$(cd "$T" && scripts/check-version.sh --print)"
major="${V%%.*}"
expect "an equal version is refused" 1 "version $V is not greater than prod's $V"
bump "$((major + 1)).0.0"
expect "a higher version passes" 0 "version $((major + 1)).0.0 raises prod's $V"
expect "a base ref that does not exist is refused" 1 "the base ref origin/nope is not in this repository" origin/nope
bump 0.0.1
g commit -qam higher && g branch -f prod
bump 0.0.0
expect "a lower version is refused" 1 "version 0.0.0 is not greater than prod's 0.0.1"

bump 0.9.0
g commit -qam 0.9.0 && g branch -f prod
bump 0.10.0
expect "versions compare as numbers (0.10.0 over 0.9.0)" 0 "0.10.0 raises prod's 0.9.0"
bump 0.10.0-rc.1
expect "a pre-release version is refused" 1 "version 0.10.0-rc.1 is not X.Y.Z"
bump 0.010.0
expect "a version with a leading zero is refused" 1 "version 0.010.0 is not X.Y.Z"

bump 1.0.0
sed -i.bak 's/"version": "1.0.0"/"version": "1.0.1"/' "$T/plugins/clax/.codex-plugin/plugin.json"
expect "version files that disagree are refused" 1 "version files disagree"
cp "$T/plugins/clax/.codex-plugin/plugin.json.bak" "$T/plugins/clax/.codex-plugin/plugin.json"
expect "the same files agreeing again pass" 0 "1.0.0 raises prod's 0.9.0"

out="$(cd "$T" && scripts/check-version-bump.sh 2>&1)"; rc=$?
if [ "$rc" = 2 ] && echo "$out" | grep "usage:" >/dev/null; then pass "no base ref is a usage error"; else fail "no base ref is a usage error ($rc: $out)"; fi

exit "$FAILED"
