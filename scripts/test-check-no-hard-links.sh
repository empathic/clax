#!/usr/bin/env bash
# Checks scripts/check-no-hard-links.sh against code that makes hard links
# and code that only looks like it.
set -euo pipefail
cd "$(dirname "$0")/.."
d="$(mktemp -d)"; trap 'rm -rf "$d"' EXIT
fails=0
expect() { # expect flagged|pass FILE CONTENT
    printf '%s\n' "$3" > "$d/$2"
    if scripts/check-no-hard-links.sh "$d/$2" >/dev/null 2>&1; then got=pass; else got=flagged; fi
    if [ "$got" = "$1" ]; then echo "PASS: $2 $1"; else echo "FAIL: $2 expected $1, got $got"; fails=1; fi
}
expect flagged hard.rs 'std::fs::hard_link(a, b)?;'
expect flagged spawn.rs 'std::process::Command::new("ln").arg(a);'
expect flagged node.ts 'import { linkSync } from "node:fs"; linkSync(a, b);'
expect flagged plain.sh 'ln "$a" "$b"'
expect flagged var.sh '"$FAKE_EXE_LN" "$a" "$b"'
expect flagged cp.sh 'cp -l "$a" "$b"'
expect pass symlink.sh 'ln -s "$a" "$b"'
expect pass variable.rs 'let ln = *map.get(&(a, ln)).unwrap();'
expect pass symlink.ts 'import { symlinkSync, unlinkSync } from "node:fs";'
exit "$fails"
