#!/usr/bin/env bash
# Frees what Bazel leaves behind on a machine with many worktrees.
#
#   scripts/bazel-gc.sh              # prune, and say what was freed
#   scripts/bazel-gc.sh --dry-run    # say what would go
#   scripts/bazel-gc.sh --report     # also print the real sizes (after pruning, or with --dry-run)
#   scripts/bazel-gc.sh --adopt DIR  # let BAZEL_DISK_CACHE name another cache
#
# 1. Output bases of worktrees that no longer exist. Bazel keeps one per
#    workspace directory, outside the repository, and `git worktree remove`
#    does not delete it. One is removed only when its workspace directory is
#    gone, was inside this repository's main checkout, and no Bazel server
#    is running in it.
# 2. The shared compile cache (.bazelrc's --disk_cache): entries not used
#    for .bazelrc's --experimental_disk_cache_gc_max_age, then the least
#    recently used until it is under its --experimental_disk_cache_gc_max_size
#    (BAZEL_GC_MAX_AGE_DAYS and BAZEL_GC_MAX_SIZE_GB override; whole numbers
#    above zero). The cache is shared with ../changelist under one bound,
#    which both .bazelrc files state: this warns when changelist's
#    differs. Bazel touches an entry whenever it reads it, and trims by the
#    same rule itself, but only while a server sits idle; this bound does
#    not depend on one.
#
# 3. Private project copies that the web checks' runner left in the
#    temporary directory when it was killed outright (BAZEL_GC_TMP_ROOTS,
#    default $TMPDIR and /tmp).
#
# Safe while builds run: a build that finds an entry gone runs the action,
# and a project copy whose process is alive is never touched.

set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

# _rc <file> <flag>: the value of `build --<flag>=` in a .bazelrc, from a
# line with no config name (a `build:ci` line is CI's alone).
_rc() {
    sed -n "s/^build --$2=\\(.*\\)\$/\\1/p" "$1" 2>/dev/null | tail -1
}
rc_size="$(_rc .bazelrc experimental_disk_cache_gc_max_size)"
rc_age="$(_rc .bazelrc experimental_disk_cache_gc_max_age)"
rc_cache="$(_rc .bazelrc disk_cache)"
max_size_gb="${BAZEL_GC_MAX_SIZE_GB:-${rc_size%G}}"
max_age_days="${BAZEL_GC_MAX_AGE_DAYS:-${rc_age%d}}"
default_cache="${rc_cache/#\~/$HOME}"
default_cache="${default_cache:-$HOME/.cache/bazel-disk}"
disk_cache="${BAZEL_DISK_CACHE:-$default_cache}"
# Bytes in a "GB" of the bound; the test makes it a megabyte.
unit="${BAZEL_GC_UNIT_BYTES:-1073741824}"

# Both bounds are whole numbers above zero. Anything else is refused before
# a file is touched: in shell arithmetic a word that is not a number is 0,
# and a bound of 0 deletes everything.
for bound in "BAZEL_GC_MAX_AGE_DAYS=$max_age_days" "BAZEL_GC_MAX_SIZE_GB=$max_size_gb"; do
    if ! [[ "${bound#*=}" =~ ^[1-9][0-9]*$ ]]; then
        echo "error: ${bound%%=*} must be a whole number above zero (got '${bound#*=}')" >&2
        exit 2
    fi
done

# Files are deleted only from a directory known to be a Bazel disk cache:
# .bazelrc's own, or one this script or changelist's was told to adopt,
# which they mark.
marker=".clax-bazel-cache"
sibling_marker=".changelist-bazel-cache"
if [ "${1:-}" = "--adopt" ]; then
    dir="${2:?usage: $0 --adopt DIR}"
    if [ ! -d "$dir/ac" ] || [ ! -d "$dir/cas" ]; then
        echo "error: $dir has no ac/ and cas/: it is not a Bazel disk cache" >&2
        exit 2
    fi
    : > "$dir/$marker"
    echo "adopted $dir: BAZEL_DISK_CACHE=$dir may now name it"
    exit 0
fi

dry=0
report=0
for arg in "$@"; do
    case "$arg" in
        --dry-run) dry=1 ;;
        --report) report=1 ;;
        *) echo "usage: $0 [--dry-run] [--report] | --adopt DIR" >&2; exit 2 ;;
    esac
done

# The cache is shared with ../changelist (beside the main checkout) under
# one bound: a different one there would trim it to its own.
checkout="$(cd "$(git rev-parse --git-common-dir)/.." && pwd -P)"
sibling="$(dirname "$checkout")/changelist/.bazelrc"
if [ -f "$sibling" ]; then
    their_size="$(_rc "$sibling" experimental_disk_cache_gc_max_size)"
    their_age="$(_rc "$sibling" experimental_disk_cache_gc_max_age)"
    if [ "${their_size%G}" != "${rc_size%G}" ] || [ "${their_age%d}" != "${rc_age%d}" ]; then
        echo "warning: the shared cache has two bounds: .bazelrc states ${rc_size:-none} and ${rc_age:-none}," \
            "$sibling states ${their_size:-none} and ${their_age:-none}; state the same in both" >&2
    fi
fi

if [ -e "$disk_cache" ] && [ "$disk_cache" != "$default_cache" ] &&
    [ ! -f "$disk_cache/$marker" ] && [ ! -f "$disk_cache/$sibling_marker" ]; then
    echo "error: BAZEL_DISK_CACHE=$disk_cache is not the cache .bazelrc names;" \
        "if it is a Bazel disk cache, run: $0 --adopt $disk_cache" >&2
    exit 2
fi

# Bazel's user root: where every workspace's output base lives.
case "$(uname -s)" in
    Darwin) user_root="$HOME/Library/Caches/bazel/_bazel_$(id -un)" ;;
    *) user_root="${XDG_CACHE_HOME:-$HOME/.cache}/bazel/_bazel_$(id -un)" ;;
esac
user_root="${BAZEL_GC_OUTPUT_USER_ROOT:-$user_root}"

# Modification time, size in 512-byte blocks and name: BSD stat, or GNU.
if stat -f '%m' . >/dev/null 2>&1; then
    stat_fields=(-f '%m %b %N')
else
    stat_fields=(-c '%Y %b %n')
fi

_kb() { du -sk "$1" 2>/dev/null | cut -f1; }
_gb() { awk -v kb="$1" -v unit="$unit" 'BEGIN { printf "%.1f GB", kb * 1024 / unit }'; }

freed=0
if [ -d "$user_root" ]; then
    for base in "$user_root"/*/; do
        base="${base%/}"
        [ -f "$base/DO_NOT_BUILD_HERE" ] || continue
        workspace="$(cat "$base/DO_NOT_BUILD_HERE")"
        [ -n "$workspace" ] || continue
        [ ! -e "$workspace" ] || continue          # the worktree is still there
        case "$workspace" in
            "$checkout"/*) ;;
            *) continue ;;                          # not this repository's
        esac
        pid="$(cat "$base/server/server.pid.txt" 2>/dev/null || true)"
        if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
            echo "kept: $workspace is gone but its Bazel server ($pid) is still running"
            continue
        fi
        size="$(_kb "$base")"
        if [ "$dry" = 1 ]; then
            echo "would remove the output base of $workspace ($(_gb "$size"))"
        else
            # Bazel makes its outputs read-only.
            chmod -R u+w "$base" 2>/dev/null || true
            rm -rf "$base"
            echo "removed the output base of $workspace ($(_gb "$size"))"
        fi
        freed=$((freed + size))
    done
fi

# Only the two directories of entries: ac/ (action results) and cas/ (files).
# Bazel's own bookkeeping beside them is left alone.
entries=()
for part in ac cas; do
    if [ -d "$disk_cache/$part" ]; then entries+=("$disk_cache/$part"); fi
done
if [ "${#entries[@]}" -gt 0 ]; then
    before="$(_kb "$disk_cache")"
    if [ "$dry" = 1 ]; then
        old="$(find "${entries[@]}" -type f -mtime "+$max_age_days" 2>/dev/null | wc -l | tr -d ' ')"
        echo "would drop $old cache entries unused for $max_age_days days from $disk_cache ($(_gb "$before") now)"
    else
        find "${entries[@]}" -type f -mtime "+$max_age_days" -delete 2>/dev/null || true
        # Oldest first until under the cap. Sizes are in 512-byte blocks.
        limit=$((max_size_gb * unit / 512))
        find "${entries[@]}" -type f -exec stat "${stat_fields[@]}" {} + 2>/dev/null |
            sort -rn |
            awk -v limit="$limit" '{ used += $2; if (used > limit) { sub(/^[0-9]+ [0-9]+ /, ""); print } }' |
            while IFS= read -r entry; do rm -f "$entry"; done
        after="$(_kb "$disk_cache")"
        echo "cache: $disk_cache is $(_gb "$after") (was $(_gb "$before"); cap $max_size_gb GB, $max_age_days days)"
        freed=$((freed + before - after))
    fi
fi

# 3. Private project copies (bazel/js/npm_script.mjs) whose process was
#    killed before it could remove its own: each is a whole project with
#    its packages. One is removed only if it carries the runner's marker,
#    the marker is over an hour old, and the process it names is gone.
tmp_roots="${BAZEL_GC_TMP_ROOTS:-${TMPDIR:-/tmp}:/tmp}"
IFS=: read -r -a roots <<<"$tmp_roots"
for root in "${roots[@]}"; do
    [ -d "$root" ] || continue
    for copy in "$root"/npm-script-*/; do
        copy="${copy%/}"
        marker="$copy/.npm-script-private"
        [ -f "$marker" ] || continue
        [ -n "$(find "$marker" -mmin +60 2>/dev/null)" ] || continue
        pid="$(head -1 "$marker" 2>/dev/null || true)"
        if [[ "$pid" =~ ^[0-9]+$ ]] && kill -0 "$pid" 2>/dev/null; then continue; fi
        size="$(_kb "$copy")"
        if [ "$dry" = 1 ]; then
            echo "would remove the leftover project copy $copy ($(_gb "$size"))"
        else
            chmod -R u+w "$copy" 2>/dev/null || true
            rm -rf "$copy"
            echo "removed the leftover project copy $copy ($(_gb "$size"))"
        fi
        freed=$((freed + size))
    done
done

if [ "$report" = 1 ]; then
    # Real disk: a clone shares its blocks, so du over-counts output bases.
    echo "report: cache $disk_cache $(_gb "$(_kb "$disk_cache")")"
    if [ -d "$user_root" ]; then
        for base in "$user_root"/*/; do
            [ -f "$base/DO_NOT_BUILD_HERE" ] || continue
            echo "report: output base of $(cat "$base/DO_NOT_BUILD_HERE") $(_gb "$(_kb "$base")") (du; clones count in full)"
        done
        [ -d "$user_root/cache/repos" ] && echo "report: downloads $(_gb "$(_kb "$user_root/cache/repos")")"
    fi
fi

if [ "$dry" = 1 ]; then
    echo "would free about $(_gb "$freed") of output bases"
else
    echo "freed about $(_gb "$freed") as du counts it (a cloned file counts in each place)"
fi
