#!/usr/bin/env bash
# stable-bin.sh SRC STORE: prints the path of a copy of the clax binary SRC
# kept in STORE under a hash of its content, kept (and run once, with
# --version) only when no copy of that content is there yet.
#
# A copy, so that a later `cargo build` replacing SRC cannot change the
# binary under a run that uses it. Kept by content because macOS assesses
# every new executable file on its first run (syspolicyd), which can take
# minutes on a loaded machine: a fresh copy per run would pay that each run,
# where a kept one pays it once per build. The first run happens here,
# before the copy takes its final name, so no caller ever makes it.
#
# The hash is of the copy, not of SRC, so a build replacing SRC meanwhile
# cannot file one build's bytes under another's hash. Pruning is
# best-effort: a kept copy goes only once it is outside the three used last
# and untouched for PRUNE_MINUTES, so a copy a run may still be using is
# never removed, and a pruner racing another never fails the run.
set -euo pipefail
PRUNE_MINUTES=240
src="$1"
store="$2"
name="$(basename "$src")"
mkdir -p "$store"
tmp="$(mktemp -d "$store/.new.XXXXXX")"
trap 'chmod -R u+w "$tmp" 2>/dev/null; rm -rf "$tmp"' EXIT
cp "$src" "$tmp/$name"
sum="$(shasum -a 256 "$tmp/$name")"
sum="${sum%% *}"
dir="$store/$sum"
if [ ! -x "$dir/$name" ]; then
    "$tmp/$name" --version >/dev/null
    chmod a-w "$tmp/$name"
    # A file where the directory belongs (never made here) is replaced.
    if [ -e "$dir" ] && [ ! -d "$dir" ]; then rm -f "$dir"; fi
    mkdir -p "$dir"
    # Renaming the file is atomic; a run of the copy it replaces, kept by
    # another run meanwhile with the same bytes, keeps its own file.
    mv -f "$tmp/$name" "$dir/$name"
fi
touch -c "$dir"
n=0
while IFS= read -r old; do
    old="${old%/}"
    n=$((n + 1))
    if [ "$n" -le 3 ] || [ "$old" = "$dir" ]; then continue; fi
    if [ -n "$(find "$old" -maxdepth 0 -mmin "+$PRUNE_MINUTES" 2>/dev/null)" ]; then
        { chmod -R u+w "$old" && rm -rf "$old"; } 2>/dev/null || true
    fi
done < <(ls -1dt "$store"/*/ 2>/dev/null || true)
# Temporary directories a killed run left behind.
find "$store" -mindepth 1 -maxdepth 1 -type d -name '.new.*' -mmin "+$PRUNE_MINUTES" \
    -exec sh -c 'chmod -R u+w "$1" && rm -rf "$1"' _ {} \; 2>/dev/null || true
if [ ! -x "$dir/$name" ]; then
    echo "stable-bin: $dir/$name is missing" >&2
    exit 1
fi
printf '%s\n' "$dir/$name"
