#!/usr/bin/env bash
# check-ios-spawns.sh [ROOT]: fails on a process launch (`Command::new`,
# `posix_spawn`, `fork()`, `exec*()`) in the crates the iOS app's core links,
# since iOS lets an app start no other program, unless
# scripts/ios-spawns.allow names the function it is in. An entry is
# `<path> <fn>  # how it is kept off on iOS`, or `skip <path>  # why` for a
# module compiled out on iOS: its `mod` line must sit under a `#[cfg(not(…))]`
# naming `target_os = "ios"`. An entry with no launch left fails too.
# Test code is not scanned: files named `*_tests.rs` or `tests.rs`, and a
# top-level item under `#[cfg(test)]`. ROOT defaults to the repository.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "${1:-$(dirname "$0")/..}" && pwd)"
cd "$root"
# clax-embed (the core linked into the iOS app), and clax-server and what it
# links outside its dev-dependencies.
crates=(clax-core clax-embed clax-embedded clax-http clax-mcp clax-realtime clax-rt clax-sample clax-server clax-store)
files=()
for c in "${crates[@]}"; do
    [ -d "crates/$c/src" ] || continue
    while IFS= read -r -d '' f; do files+=("$f"); done \
        < <(find "crates/$c/src" -name '*.rs' ! -name '*_tests.rs' ! -name 'tests.rs' -print0)
done
python3 "$here/check-ios-spawns.py" scripts/ios-spawns.allow "${files[@]}"
