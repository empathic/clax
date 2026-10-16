#!/usr/bin/env bash
# Tests scripts/bazel-gc.sh on throwaway directories: what it refuses, what
# it deletes and what it must leave, and the bound it reads from .bazelrc
# beside changelist's. It runs a copy of the script in a scratch checkout
# (a git repository with its own .bazelrc) beside a scratch ../changelist.
# No Bazel runs, and nothing is found by pgrep.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp="$(cd "$(mktemp -d)" && pwd -P)"
trap 'chmod -R u+w "$tmp" 2>/dev/null; rm -rf "$tmp"' EXIT

# _bazelrc <file> <size> <age>: the three cache lines.
_bazelrc() {
    printf 'build --disk_cache=~/.cache/bazel-disk\nbuild --experimental_disk_cache_gc_max_size=%s\nbuild --experimental_disk_cache_gc_max_age=%s\nbuild:ci --experimental_disk_cache_gc_max_size=6G\n' "$2" "$3" > "$1"
}
mkdir -p "$tmp/w/clax/scripts" "$tmp/w/changelist"
cp "$here/scripts/bazel-gc.sh" "$tmp/w/clax/scripts/"
_bazelrc "$tmp/w/clax/.bazelrc" 80G 7d
_bazelrc "$tmp/w/changelist/.bazelrc" 80G 7d
cd "$tmp/w/clax"
git init -q

fails=0
_ok() { echo "ok: $1"; }
_fail() { echo "FAIL: $1" >&2; fails=$((fails + 1)); }

# A cache with four 1 MB entries, the first two a month old.
_cache() {
    rm -rf "$1"
    mkdir -p "$1/ac/00" "$1/cas/00" "$1/gc"
    : > "$1/gc/lock"
    for n in 1 2 3 4; do
        dd if=/dev/zero of="$1/cas/00/entry$n" bs=1024 count=1024 2>/dev/null
    done
    touch -t 202001010000 "$1/cas/00/entry1" "$1/cas/00/entry2"
    : > "$1/ac/00/result"
}
_entries() { find "$1/ac" "$1/cas" -type f | wc -l | tr -d ' '; }

# An empty user root and no temporary directories, so no real output base
# or project copy is ever looked at.
export BAZEL_GC_TMP_ROOTS="$tmp/none"
export BAZEL_GC_OUTPUT_USER_ROOT="$tmp/root"
mkdir -p "$BAZEL_GC_OUTPUT_USER_ROOT"

# 1. Bounds that are not whole numbers above zero are refused, and nothing
#    is deleted. `abc` is the dangerous one: shell arithmetic reads it as 0.
cache="$tmp/cache"
_cache "$cache"
./scripts/bazel-gc.sh --adopt "$cache" >/dev/null
for bad in abc 0 -1 20GB 1.5 ""; do
    for var in BAZEL_GC_MAX_SIZE_GB BAZEL_GC_MAX_AGE_DAYS; do
        if env "$var=$bad" BAZEL_DISK_CACHE="$cache" ./scripts/bazel-gc.sh >/dev/null 2>&1; then
            # An empty value means "unset": the default applies.
            [ -z "$bad" ] || _fail "$var='$bad' was accepted"
        fi
    done
done
_cache "$cache"; : > "$cache/.clax-bazel-cache"
BAZEL_GC_MAX_SIZE_GB=abc BAZEL_DISK_CACHE="$cache" ./scripts/bazel-gc.sh >/dev/null 2>&1 || true
[ "$(_entries "$cache")" = 5 ] && _ok "a bound that is not a number deletes nothing" ||
    _fail "a bound that is not a number deleted entries"

# 2. A directory that is not a known cache is refused untouched.
other="$tmp/other"
_cache "$other"
if BAZEL_DISK_CACHE="$other" ./scripts/bazel-gc.sh >/dev/null 2>&1; then
    _fail "an unmarked directory was accepted as the cache"
elif [ "$(_entries "$other")" = 5 ]; then
    _ok "an unmarked directory is refused and left alone"
else
    _fail "an unmarked directory was refused but entries are gone"
fi
if ./scripts/bazel-gc.sh --adopt "$tmp" >/dev/null 2>&1; then
    _fail "--adopt accepted a directory with no ac/ and cas/"
else
    _ok "--adopt refuses a directory that is not a disk cache"
fi

# 3. On an adopted cache: old entries go, fresh ones and Bazel's own files
#    stay.
./scripts/bazel-gc.sh --adopt "$other" >/dev/null
BAZEL_DISK_CACHE="$other" ./scripts/bazel-gc.sh >/dev/null
if [ ! -e "$other/cas/00/entry1" ] && [ ! -e "$other/cas/00/entry2" ] &&
    [ -e "$other/cas/00/entry3" ] && [ -e "$other/cas/00/entry4" ] &&
    [ -e "$other/ac/00/result" ] && [ -e "$other/gc/lock" ]; then
    _ok "entries past the age bound go; the rest and gc/lock stay"
else
    _fail "the age bound deleted the wrong files"
fi

# 4. --dry-run deletes nothing.
_cache "$other"; : > "$other/.changelist-bazel-cache"
BAZEL_DISK_CACHE="$other" ./scripts/bazel-gc.sh --dry-run >/dev/null
[ "$(_entries "$other")" = 5 ] && _ok "--dry-run deletes nothing" || _fail "--dry-run deleted entries"

# 5. Output bases: one whose workspace exists is kept; one whose workspace
#    is gone is removed only when that workspace lay inside this checkout.
checkout="$(cd "$(git rev-parse --git-common-dir)/.." && pwd -P)"
for name in live gone foreign running; do mkdir -p "$BAZEL_GC_OUTPUT_USER_ROOT/$name/server"; done
echo "$checkout/.claude/worktrees/no-such-worktree-r$$" > "$BAZEL_GC_OUTPUT_USER_ROOT/running/DO_NOT_BUILD_HERE"
echo "$$" > "$BAZEL_GC_OUTPUT_USER_ROOT/running/server/server.pid.txt"
sh -c 'exit 0' & gone_pid=$!
wait "$gone_pid"
echo "$gone_pid" > "$BAZEL_GC_OUTPUT_USER_ROOT/gone/server/server.pid.txt"
pwd -P > "$BAZEL_GC_OUTPUT_USER_ROOT/live/DO_NOT_BUILD_HERE"
echo "$checkout/.claude/worktrees/no-such-worktree-$$" > "$BAZEL_GC_OUTPUT_USER_ROOT/gone/DO_NOT_BUILD_HERE"
echo "$tmp/elsewhere" > "$BAZEL_GC_OUTPUT_USER_ROOT/foreign/DO_NOT_BUILD_HERE"
BAZEL_DISK_CACHE="$other" ./scripts/bazel-gc.sh >/dev/null
if [ -d "$BAZEL_GC_OUTPUT_USER_ROOT/live" ] && [ ! -e "$BAZEL_GC_OUTPUT_USER_ROOT/gone" ] &&
    [ -d "$BAZEL_GC_OUTPUT_USER_ROOT/foreign" ] && [ -d "$BAZEL_GC_OUTPUT_USER_ROOT/running" ]; then
    _ok "only the output base of a deleted worktree of this checkout whose server is dead is removed"
else
    _fail "the wrong output bases were removed"
fi

# 6. Leftover project copies: only one with the runner's marker, old, and
#    whose process is gone, is removed.
copies="$tmp/copies"
mkdir -p "$copies"
_copy() { # <name> <pid, or - for no marker> <old|new>
    mkdir -p "$copies/npm-script-$1/web"
    : > "$copies/npm-script-$1/web/file"
    if [ "$2" != - ]; then
        echo "$2" > "$copies/npm-script-$1/.npm-script-private"
        if [ "$3" = old ]; then touch -t 202001010000 "$copies/npm-script-$1/.npm-script-private"; fi
    fi
}
# A PID that is not running: one just reaped.
sh -c 'exit 0' & dead=$!
wait "$dead"
_copy dead "$dead" old
_copy alive "$$" old
_copy fresh "$dead" new
_copy unmarked - old
BAZEL_GC_TMP_ROOTS="$copies" BAZEL_DISK_CACHE="$other" ./scripts/bazel-gc.sh >/dev/null
if [ ! -e "$copies/npm-script-dead" ] && [ -d "$copies/npm-script-alive" ] &&
    [ -d "$copies/npm-script-fresh" ] && [ -d "$copies/npm-script-unmarked" ]; then
    _ok "only an old, marked project copy whose process is gone is removed"
else
    _fail "the wrong leftover project copies were removed"
fi

# 7. The bound comes from .bazelrc: 3 "GB" (megabytes here) over five
#    fresh 1 MB entries leaves at most three; changelist's other bound is
#    warned about, naming both, and the same bound is not.
_bazelrc .bazelrc 3G 7d
_cache "$other"; : > "$other/.clax-bazel-cache"
touch "$other/cas/00/entry1" "$other/cas/00/entry2"
dd if=/dev/zero of="$other/cas/00/entry5" bs=1024 count=1024 2>/dev/null
BAZEL_GC_UNIT_BYTES=1048576 BAZEL_DISK_CACHE="$other" ./scripts/bazel-gc.sh >/dev/null 2>&1
n="$(find "$other/cas" -type f | wc -l | tr -d ' ')"
[ "$n" -le 3 ] && [ "$n" -ge 1 ] && _ok "the size bound .bazelrc states holds ($n entries left)" ||
    _fail "the size bound .bazelrc states: $n entries left"
_bazelrc .bazelrc 80G 7d
_bazelrc ../changelist/.bazelrc 20G 7d
out="$(BAZEL_DISK_CACHE="$other" ./scripts/bazel-gc.sh --dry-run 2>&1)"
if echo "$out" | grep "warning: the shared cache has two bounds" >/dev/null && echo "$out" | grep 80G >/dev/null &&
    echo "$out" | grep 20G >/dev/null && echo "$out" | grep changelist/.bazelrc >/dev/null; then
    _ok "another bound in changelist's .bazelrc is warned about, naming both"
else
    _fail "no warning about changelist's bound: $out"
fi
_bazelrc ../changelist/.bazelrc 80G 7d
out="$(BAZEL_DISK_CACHE="$other" ./scripts/bazel-gc.sh --dry-run 2>&1)"
echo "$out" | grep "two bounds" >/dev/null && _fail "a warning with the same bound" || _ok "the same bound in both, no warning"

# 8. --report prints the sizes.
out="$(BAZEL_DISK_CACHE="$other" ./scripts/bazel-gc.sh --dry-run --report 2>&1)"
echo "$out" | grep "^report: cache $other" >/dev/null && _ok "--report prints the cache's size" || _fail "--report: $out"

if [ "$fails" -gt 0 ]; then
    echo "bazel-gc tests: $fails failed" >&2
    exit 1
fi
echo "bazel-gc tests: all passed"
