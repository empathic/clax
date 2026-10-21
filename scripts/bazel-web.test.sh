#!/usr/bin/env bash
# Tests that a web test's result under Bazel depends on the files in the
# checkout and nothing else: not on a file that used to be there, nor on a
# package that used to be in the lockfile.
#
#   scripts/bazel-web.test.sh
#
# It adds a module and a test of it under web/shell/src, then takes them
# away again, running `bazelisk test //web:unit` at each step:
#   1. module and test present            the test runs and passes
#   2. module deleted, test still there   the target fails
#   3. test deleted too                   the target passes; the test is gone
#   4. pako dropped from package.json and pnpm-lock.yaml
#                                         //web:bundle_size, which imports it, fails
# Bazel keeps copies of sources and packages in its output directory and
# does not remove one when its source goes; a tool run there would pass
# steps 2 and 4 on files that no longer exist. bazel/js/npm_script.mjs runs
# each script in a copy of exactly its declared files instead, and these
# steps are what prove it.
#
# None of this touches the checkout. The script copies the tree (what git
# tracks and what it would track) to target/bazel-web-test.<pid>/, which git
# and Bazel both ignore, and edits and builds the copy; the copy and its
# Bazel output base are removed however the script ends. An interrupted
# `just ci` therefore cannot leave package.json or the lockfile changed. It
# takes a few minutes: a Bazel server of its own and four runs of the unit
# tests.

set -uo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

bazel="$(command -v bazelisk || command -v bazel || true)"
if [ -z "$bazel" ]; then
    echo "error: bazelisk not found; install it (brew install bazelisk)" >&2
    exit 1
fi

# Copies a run of this script left when it was killed outright: one whose
# process (the number in its name) is gone. scripts/bazel-gc.sh then
# removes its output base, since its workspace no longer exists.
for old in target/bazel-web-test.*/; do
    [ -d "$old" ] || continue
    pid="${old%/}"; pid="${pid##*.}"
    if ! kill -0 "$pid" 2>/dev/null; then
        chmod -R u+w "$old" 2>/dev/null || true
        rm -rf "$old"
    fi
done

scratch="$PWD/target/bazel-web-test.$$"
_cleanup() {
    if [ -d "$scratch" ]; then
        (cd "$scratch" && "$bazel" clean --expunge >/dev/null 2>&1 && "$bazel" shutdown >/dev/null 2>&1) || true
        chmod -R u+w "$scratch" 2>/dev/null || true
        rm -rf "$scratch"
    fi
}
trap _cleanup EXIT
trap 'exit 130' INT TERM HUP

mkdir -p "$scratch"
# What git tracks and what it would track, as the files are now.
git ls-files -z --cached --others --exclude-standard | python3 -c '
import os, shutil, sys
dest = sys.argv[1]
for path in sys.stdin.buffer.read().split(b"\0"):
    path = path.decode()
    if not path or not os.path.lexists(path) or os.path.isdir(path):
        continue
    target = os.path.join(dest, path)
    os.makedirs(os.path.dirname(target), exist_ok=True)
    if os.path.islink(path):
        os.symlink(os.readlink(path), target)
    else:
        shutil.copy2(path, target)
' "$scratch"
cd "$scratch" || exit 1

module="web/shell/src/zzBazelProbe.ts"
test="web/shell/src/zzBazelProbe.test.ts"
log="$scratch/local-test.log"

fails=0
_ok() { echo "ok: $1"; }
# _fail: counts a failed step and shows why from the step's $log. The copy,
# its Bazel output and the log are removed when the script exits, so what is
# not printed here is lost.
_fail() {
    echo "FAIL: $1" >&2
    fails=$((fails + 1))
    [ -f "$log" ] || return 0
    local failed
    failed="$(grep -E '^[[:space:]]*(×|FAIL[[:space:]])' "$log" | sort -u | sed -n 1,40p)"
    if [ -n "$failed" ]; then
        echo "  failing tests and files:" >&2
        sed 's/^/    /' <<<"$failed" >&2
    fi
    echo "  the last 60 lines of the step's log:" >&2
    tail -n 60 "$log" | sed 's/^/    /' >&2
}

# _ran_probe: whether the step's log shows the probe's test file run.
_ran_probe() { grep -q 'zzBazelProbe\.test' "$log"; }

# _unit: runs the unit tests; the status is Bazel's, the log is in $log.
_unit() {
    "$bazel" test --lockfile_mode=error --test_output=all --test_arg=--reporter=verbose //web:unit >"$log" 2>&1
}

cat > "$module" <<'TS'
export const zzBazelProbe = 41 + 1;
TS
cat > "$test" <<'TS'
import { expect, it } from 'vitest';
import { zzBazelProbe } from './zzBazelProbe';

it('zz bazel probe reads its module', () => {
  expect(zzBazelProbe).toBe(42);
});
TS

if _unit; then unit=0; else unit=1; fi
if ! _ran_probe; then
    _fail "a new test and its module were not run"
elif [ "$unit" -ne 0 ]; then
    _fail "the unit tests failed with a new test and its module present"
else
    _ok "a new test and its module are run"
fi

rm -f "$module"
if _unit; then
    _fail "the unit tests pass though a module one of them imports is deleted"
elif grep -Eq 'FAIL[[:space:]]+shell/src/zzBazelProbe\.test\.ts' "$log"; then
    _ok "deleting an imported module fails the target"
else
    _fail "the target failed, but not in the test whose module is deleted"
fi

# With the test gone the tree would be the one Bazel has already tested, and
# it would answer from its cache without running anything. Another file
# changes too, so the tests really run.
rm -f "$test"
echo "export const zzBazelProbeOther = $$;" > web/shell/src/zzBazelProbeOther.ts
if _unit; then unit=0; else unit=1; fi
if _ran_probe; then
    _fail "a deleted test still runs"
elif [ "$unit" -ne 0 ]; then
    _fail "the unit tests failed with the probe's test deleted, for a reason other than the probe"
elif ! grep -q 'Test Files' "$log"; then
    _fail "the unit tests passed but report no test files: they did not run"
else
    _ok "a deleted test no longer runs"
fi

# `pako` deflates for the bundle gate (web/scripts/bundle-size.mjs); without
# it in the lockfile and package.json nothing may still find it.
python3 - <<'PY'
import json, re
package = json.load(open("web/package.json"))
for kind in ("dependencies", "devDependencies"):
    package.get(kind, {}).pop("pako", None)
json.dump(package, open("web/package.json", "w"), indent=2)
lock = open("web/pnpm-lock.yaml").read()
# The importer's entry, and the package's own under `packages` and `snapshots`.
lock, a = re.subn(r"\n      pako:\n        specifier: [^\n]*\n        version: [^\n]*", "", lock)
lock, b = re.subn(r"\n  pako@[^\n]*\n(?:    [^\n]*\n)*", "\n", lock)
assert a == 1 and b >= 2, (a, b)
open("web/pnpm-lock.yaml", "w").write(lock)
PY
if "$bazel" test --lockfile_mode=error --test_output=all //web:bundle_size >"$log" 2>&1; then
    _fail "the bundle gate passes though a package it imports is gone from the lockfile"
elif grep -q "pako" "$log"; then
    _ok "a package removed from the lockfile is no longer found"
else
    _fail "the target failed without the package, but not for want of it"
fi

if [ "$fails" -gt 0 ]; then
    echo "bazel-web tests: $fails failed" >&2
    exit 1
fi
echo "bazel-web tests: all passed"
