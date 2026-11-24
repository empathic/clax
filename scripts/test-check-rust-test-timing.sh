#!/usr/bin/env bash
# Checks scripts/check-rust-test-timing.sh on a minimal workspace: a
# #[cfg(test)] mod or fn without the outer warn attribute, a test crate
# root (discovered, or a [[test]] path with autotests off) and clax-testkit's
# root without the inner one each fail, naming the file; with them it
# passes, as do product code, a #[cfg(test)] statement and a module file
# that is no crate root.
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1
d="$(mktemp -d)"
trap 'rm -rf "$d"' EXIT
fails=0
W='#![warn(clippy::disallowed_methods)]'
w='#[warn(clippy::disallowed_methods)]'

_tree() { # a passing workspace
    rm -rf "$d/w"
    mkdir -p "$d/w/crates/a/src" "$d/w/crates/a/tests/common" "$d/w/crates/b/src" "$d/w/crates/b/tests" \
        "$d/w/crates/clax-testkit/src"
    printf '[package]\nname = "a"\n' > "$d/w/crates/a/Cargo.toml"
    printf '[package]\nname = "b"\nautotests = false\n\n[[test]]\nname = "integration"\npath = "tests/integration.rs"\n' \
        > "$d/w/crates/b/Cargo.toml"
    printf '[package]\nname = "clax-testkit"\n' > "$d/w/crates/clax-testkit/Cargo.toml"
    cat > "$d/w/crates/a/src/lib.rs" <<RS
//! A.
pub fn f() -> std::time::Instant {
    #[cfg(test)]
    hook();
    std::time::Instant::now()
}

#[cfg(test)]
$w
fn hook() {}

#[cfg(test)]
$w
mod tests {}
RS
    printf '//! A test.\n%s\n\nmod common;\n' "$W" > "$d/w/crates/a/tests/api.rs"
    printf 'pub fn helper() {}\n' > "$d/w/crates/a/tests/common/mod.rs"
    printf 'pub fn g() {}\n' > "$d/w/crates/b/src/lib.rs"
    printf '%s\n\nmod api_x;\n' "$W" > "$d/w/crates/b/tests/integration.rs"
    printf 'fn t() {}\n' > "$d/w/crates/b/tests/api_x.rs"
    printf '//! Test support.\n%s\n' "$W" > "$d/w/crates/clax-testkit/src/lib.rs"
}

expect() { # expect pass|fail NAME [NEEDLE]
    if scripts/check-rust-test-timing.sh "$d/w" > "$d/out" 2>&1; then got=pass; else got=fail; fi
    if [ "$got" = "$1" ] && { [ -z "${3:-}" ] || grep -F -- "$3" "$d/out" >/dev/null; }; then
        echo "PASS: $2"
    else
        echo "FAIL: $2 (expected $1${3:+ naming $3}, got $got)"; cat "$d/out"; fails=1
    fi
}

_tree; expect pass "a tree whose test code carries the attributes"
_tree; python3 - "$d/w/crates/a/src/lib.rs" <<'PY'
import sys; p = sys.argv[1]; s = open(p).read()
open(p, "w").write(s.replace("#[cfg(test)]\n#[warn(clippy::disallowed_methods)]\nmod tests", "#[cfg(test)]\nmod tests"))
PY
expect fail "a #[cfg(test)] mod without it" "crates/a/src/lib.rs:13: a #[cfg(test)] mod"
_tree; python3 - "$d/w/crates/a/src/lib.rs" <<'PY'
import sys; p = sys.argv[1]; s = open(p).read()
open(p, "w").write(s.replace("#[cfg(test)]\n#[warn(clippy::disallowed_methods)]\nfn hook", "#[cfg(test)]\nfn hook"))
PY
expect fail "a #[cfg(test)] fn without it" "a #[cfg(test)] fn"
_tree; printf 'mod common;\n' > "$d/w/crates/a/tests/api.rs"
expect fail "a discovered test root without it" "crates/a/tests/api.rs:1"
_tree; printf 'mod api_x;\n' > "$d/w/crates/b/tests/integration.rs"
expect fail "a [[test]] path without it" "crates/b/tests/integration.rs:1"
_tree; printf '//! Test support.\n' > "$d/w/crates/clax-testkit/src/lib.rs"
expect fail "clax-testkit's root without it" "crates/clax-testkit/src/lib.rs:1"
if scripts/check-rust-test-timing.sh > "$d/out" 2>&1; then echo "PASS: the repository's test code is in the rule"; else
    echo "FAIL: the repository"; cat "$d/out"; fails=1; fi
exit "$fails"
