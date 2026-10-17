#!/usr/bin/env bash
# Checks scripts/check-crate-layers.py: the workspace passes; a copy of its
# manifests with an upward edge, a sideways edge in a target table, or a
# crate the layout does not know fails, naming it. Dev-dependencies pass.
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1
d="$(mktemp -d)"
trap 'rm -rf "$d"' EXIT
fails=0
_fail() { echo "FAIL: $1" >&2; fails=$((fails + 1)); }
_copy() {
    rm -rf "$d/w" && mkdir -p "$d/w"
    cp Cargo.toml "$d/w/"
    for m in crates/*/Cargo.toml; do mkdir -p "$d/w/$(dirname "$m")" && cp "$m" "$d/w/$m"; done
}
# _expect pass|fail NAME [TEXT]: runs the check on the copy; TEXT must be in
# what a failure prints.
_expect() {
    local out st
    out="$(python3 scripts/check-crate-layers.py "$d/w" 2>&1)"
    st=$?
    case "$1" in
        pass) [ "$st" = 0 ] || _fail "$2: $out" ;;
        fail) { [ "$st" != 0 ] && grep -qF "$3" <<<"$out"; } || _fail "$2: $out" ;;
    esac
}

_copy
_expect pass "the workspace"

_copy
printf '\n[dependencies]\nclax-store = { path = "../clax-store" }\n' >> "$d/w/crates/clax-core/Cargo.toml"
_expect fail "clax-core on clax-store" "clax-core may not depend on clax-store"

_copy
printf "\n[target.'cfg(unix)'.dependencies]\nclax-store = { path = \"../clax-store\" }\n" >> "$d/w/crates/clax-mcp/Cargo.toml"
_expect fail "clax-mcp on clax-store in a target table" "clax-mcp may not depend on clax-store"

_copy
# clax-hooks' manifest ends in its [dev-dependencies] table.
printf 'clax-testkit = { path = "../clax-testkit" }\n' >> "$d/w/crates/clax-hooks/Cargo.toml"
_expect pass "a dev-dependency on the testkit"

_copy
mkdir -p "$d/w/crates/clax-extra"
printf '[package]\nname = "clax-extra"\n' > "$d/w/crates/clax-extra/Cargo.toml"
sed -i.bak 's#"crates/clax-store",#"crates/clax-store",\n    "crates/clax-extra",#' "$d/w/Cargo.toml"
_expect fail "a crate the layout does not know" "clax-extra has no place in the layout"

if [ "$fails" = 0 ]; then echo "crate layers tests: all passed"; else echo "crate layers tests: $fails failed"; exit 1; fi
