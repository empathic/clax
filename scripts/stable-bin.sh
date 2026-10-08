#!/usr/bin/env bash
# stable-bin.sh SRC STORE: prints the path of a copy of the clax binary SRC
# kept in STORE under a hash of its content, made (and run once, with
# --version) only when no copy of that content is there yet.
#
# A copy, so that a later `cargo build` replacing SRC cannot change the
# binary under a run that uses it. Kept by content because macOS assesses
# every new executable file on its first run (syspolicyd), which can take
# minutes on a loaded machine: a fresh copy per run would pay that each run,
# where a kept one pays it once per build. The first run happens here, before
# the copy takes its final name, so no caller ever makes it. STORE keeps the
# three copies used last.
set -euo pipefail
src="$1"
store="$2"
sum="$(shasum -a 256 "$src")"
sum="${sum%% *}"
name="$(basename "$src")"
dir="$store/$sum"
if [ ! -x "$dir/$name" ]; then
    mkdir -p "$store"
    tmp="$(mktemp -d "$store/.new.XXXXXX")"
    trap 'rm -rf "$tmp"' EXIT
    cp "$src" "$tmp/$name"
    "$tmp/$name" --version >/dev/null
    chmod a-w "$tmp/$name"
    # Another run may have kept the same content meanwhile; either copy will do.
    [ -e "$dir" ] || mv "$tmp" "$dir"
fi
touch "$dir"
# The copies used last are the newest directories; older ones go.
ls -1dt "$store"/*/ | tail -n +4 | while IFS= read -r old; do
    chmod -R u+w "$old" && rm -rf "$old"
done
printf '%s\n' "$dir/$name"
