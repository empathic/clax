#!/usr/bin/env bash
# Checks scripts/stable-bin.sh: concurrent runs and prunes, a hash directory
# left empty or made a file, pruning that spares recent copies, and a source
# that is not a usable binary.
set -euo pipefail
cd "$(dirname "$0")/.."
d="$(mktemp -d)"
trap 'chmod -R u+w "$d" 2>/dev/null; rm -rf "$d"' EXIT
fails=0
ok() { echo "PASS: $1"; }
bad() { echo "FAIL: $1"; fails=1; }

# One source for every case, so its copies hash alike; each case has a store
# of its own.
printf '#!/bin/sh\necho "clax 0.0.0"\n' > "$d/clax"
chmod +x "$d/clax"
sum="$(shasum -a 256 "$d/clax")"
sum="${sum%% *}"

# Two runs at once on one store: both print the same executable copy, and
# no temporary directory is left, inside the kept one or beside it.
s="$d/race"
mkdir -p "$s"
for i in 1 2 3; do mkdir -p "$s/old$i" && touch -t 202001010000 "$s/old$i"; done
scripts/stable-bin.sh "$d/clax" "$s" > "$d/a" 2>&1 & a=$!
scripts/stable-bin.sh "$d/clax" "$s" > "$d/b" 2>&1 & b=$!
ra=0 rb=0
wait "$a" || ra=$?
wait "$b" || rb=$?
if [ "$ra" = 0 ] && [ "$rb" = 0 ] && [ "$(cat "$d/a")" = "$s/$sum/clax" ] && [ "$(cat "$d/b")" = "$s/$sum/clax" ] \
    && [ -x "$s/$sum/clax" ] && [ "$(ls -A "$s/$sum")" = clax ] && [ -z "$(find "$s" -maxdepth 1 -name '.new.*')" ]; then
    ok "concurrent runs and prunes keep one copy and both succeed"
else
    bad "concurrent runs: rc=$ra/$rb a=$(cat "$d/a") b=$(cat "$d/b") store=$(ls -A "$s" "$s/$sum" 2>&1 | tr '\n' ' ')"
fi

# The prune removes old copies beyond the three newest, never a recent one.
s="$d/prune"
mkdir -p "$s/recent1" "$s/recent2" "$s/recent3" "$s/recent4" "$s/stale1" "$s/stale2"
touch -t 202001010000 "$s/stale1" "$s/stale2"
out="$(scripts/stable-bin.sh "$d/clax" "$s")"
if [ "$out" = "$s/$sum/clax" ] && [ ! -e "$s/stale1" ] && [ ! -e "$s/stale2" ] \
    && [ -d "$s/recent1" ] && [ -d "$s/recent2" ] && [ -d "$s/recent3" ] && [ -d "$s/recent4" ]; then
    ok "the prune takes old copies and spares recent ones"
else
    bad "prune: out=$out store=$(ls -A "$s" | tr '\n' ' ')"
fi

# A hash directory left empty is filled.
s="$d/empty"
mkdir -p "$s/$sum"
out="$(scripts/stable-bin.sh "$d/clax" "$s")"
if [ "$out" = "$s/$sum/clax" ] && [ -x "$out" ]; then ok "an empty hash directory is filled"
else bad "empty hash directory: out=$out"; fi

# A file where the hash directory belongs is replaced.
s="$d/file"
mkdir -p "$s"
: > "$s/$sum"
out="$(scripts/stable-bin.sh "$d/clax" "$s")"
if [ "$out" = "$s/$sum/clax" ] && [ -x "$out" ]; then ok "a file at the hash directory's path is replaced"
else bad "file at the hash: out=$out"; fi

# A source that does not run fails, printing no path.
printf '#!/bin/sh\nexit 3\n' > "$d/broken"
chmod +x "$d/broken"
rc=0
out="$(scripts/stable-bin.sh "$d/broken" "$d/broken-store" 2>/dev/null)" || rc=$?
if [ "$rc" != 0 ] && [ -z "$out" ]; then ok "an unusable source fails without a path"
else bad "unusable source: rc=$rc out=$out"; fi

exit "$fails"
