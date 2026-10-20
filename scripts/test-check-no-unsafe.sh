#!/usr/bin/env bash
# Checks scripts/check-no-unsafe.sh on a copy of itself beside a minimal
# workspace (two crates that take the workspace's lints, the separate
# clax-worker workspace, crates/clax-ios with its four exported C
# functions, and crates/clax-embed): the tree passes, from any directory and with or without a
# root given; any other `unsafe` in clax-ios, a fifth export, a pointer
# argument, another `allow(unsafe_code)`, clax-ios or clax-embed not
# denying the lint,
# the workspace not forbidding it, and another crate with lints of its own
# each fail, naming the finding; CSP's `'unsafe-inline'` passes; `unsafe`
# in another crate fails as it always has.
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1
d="$(mktemp -d)"
trap 'rm -rf "$d"' EXIT
fails=0
_fail() { echo "FAIL: $1" >&2; fails=$((fails + 1)); }

_crate() { # _crate NAME: a crate that takes the workspace's lints
    mkdir -p "$d/w/crates/$1/src"
    cat > "$d/w/crates/$1/Cargo.toml" <<EOF
[package]
name = "$1"
version.workspace = true

[lints]
workspace = true

[dependencies]
EOF
    printf '//! %s.\n\npub fn f() -> i32 {\n    1\n}\n' "$1" > "$d/w/crates/$1/src/lib.rs"
}

_copy() {
    rm -rf "$d/w" && mkdir -p "$d/w/scripts"
    cp scripts/check-no-unsafe.sh "$d/w/scripts/"
    cat > "$d/w/Cargo.toml" <<'EOF'
[workspace]
members = ["crates/*"]

[workspace.lints.rust]
unsafe_code = "forbid"
unexpected_cfgs = { level = "warn", check-cfg = ['cfg(clax_embedded)'] }
EOF
    _crate clax-core
    _crate clax-store
    mkdir -p "$d/w/crates/clax-worker/src"
    cat > "$d/w/crates/clax-worker/Cargo.toml" <<'EOF'
[package]
name = "clax-worker"

[workspace]

[lints.rust]
unsafe_code = "deny"
EOF
    : > "$d/w/crates/clax-worker/src/lib.rs"
    mkdir -p "$d/w/crates/clax-ios/src"
    cat > "$d/w/crates/clax-ios/Cargo.toml" <<'EOF'
[package]
name = "clax-ios"
version.workspace = true

# The workspace forbids unsafe_code; this crate denies it.
[lints.rust]
unsafe_code = "deny"
unexpected_cfgs = { level = "warn", check-cfg = ['cfg(clax_embedded)'] }

[dependencies]
EOF
    cat > "$d/w/crates/clax-ios/src/lib.rs" <<'EOF'
//! The daemon behind four C functions; `unsafe` here is a word in a
//! comment, which the check skips.

mod daemon;

pub use daemon::{start_at, state, stop, trim};

/// Starts the daemon.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn clax_start(port: u16) -> i32 {
    start_at(port)
}

/// Asks the daemon to stop.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn clax_stop() { stop() }

/// The daemon's state.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn clax_state() -> i32 { state() }

/// Shrinks the caches.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn clax_trim_memory() { let _ = trim(std::time::Duration::from_secs(2)); }
EOF
    printf 'pub fn start_at(_: u16) -> i32 {\n    0\n}\n' > "$d/w/crates/clax-ios/src/daemon.rs"
    mkdir -p "$d/w/crates/clax-embed/src"
    cat > "$d/w/crates/clax-embed/Cargo.toml" <<'EOF'
[package]
name = "clax-embed"
version.workspace = true

# The workspace forbids unsafe_code; this crate denies it.
[lints.rust]
unsafe_code = "deny"
unexpected_cfgs = { level = "warn", check-cfg = ['cfg(clax_embedded)'] }

[lib]
crate-type = ["rlib", "cdylib", "staticlib"]

[dependencies]
EOF
    printf '//! The core in an app, behind C functions.\n\npub fn f() -> i32 {\n    1\n}\n' > "$d/w/crates/clax-embed/src/lib.rs"
}

# _expect pass|fail NAME [TEXT]: runs the copy of the check, from another
# directory, on its own tree; TEXT must be in what a failure prints.
_expect() {
    local out st
    out="$(cd / && "$d/w/scripts/check-no-unsafe.sh" 2>&1)"
    st=$?
    case "$1" in
        pass) [ "$st" = 0 ] || _fail "$2: $out" ;;
        fail) { [ "$st" != 0 ] && grep -qF -- "$3" <<<"$out"; } || _fail "$2: exit $st: $out" ;;
    esac
}
# _sub FILE FROM TO: replaces the first FROM in FILE with TO (FROM must be there).
_sub() {
    python3 - "$@" <<'EOF'
import sys
f, old, new = sys.argv[1:]
t = open(f).read()
assert old in t, old
open(f, "w").write(t.replace(old, new, 1))
EOF
}
IOS="$d/w/crates/clax-ios/src/lib.rs"
ONLY="only the four exported functions may use unsafe"

_copy
_expect pass "the tree"

_copy
out="$(scripts/check-no-unsafe.sh "$d/w" 2>&1)" || _fail "the tree, as a root argument: $out"

_copy
rm -rf "$d/w/crates/clax-ios"
_expect pass "the tree without clax-ios"

_copy
_sub "$IOS" "    start_at(port)" "    unsafe { start_at(port) }"
_expect fail "an unsafe block in clax-ios" "crates/clax-ios/src/lib.rs:12: $ONLY"

_copy
printf '\n#[allow(unsafe_code)]\n#[unsafe(no_mangle)]\npub extern "C" fn clax_extra() {}\n' >> "$IOS"
_expect fail "a fifth export" "crates/clax-ios/src/lib.rs:31: $ONLY"

_copy
printf '\n#[allow(unsafe_code)]\n#[unsafe(no_mangle)]\npub extern "C" fn clax_stop() {}\n' >> "$IOS"
_expect fail "an export named twice" "crates/clax-ios/src/lib.rs:31: $ONLY"

_copy
_sub "$IOS" "clax_start(port: u16)" "clax_start(p: *const u8)"
_expect fail "a pointer argument" "crates/clax-ios/src/lib.rs:10: $ONLY"

_copy
_sub "$IOS" "clax_start(port: u16)" "clax_start(port: u16, len: usize)"
_expect fail "a second argument that is not u16 or i32" "crates/clax-ios/src/lib.rs:10: $ONLY"

_copy
_sub "$IOS" "#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern \"C\" fn clax_stop()" "#[unsafe(no_mangle)]
pub extern \"C\" fn clax_stop()"
_expect fail "an export without its allow" "crates/clax-ios/src/lib.rs:16: $ONLY"

_copy
_sub "$IOS" "#[unsafe(no_mangle)]
pub extern \"C\" fn clax_state()" "#[unsafe(export_name = \"clax_state\")]
pub extern \"C\" fn clax_state()"
_expect fail "another unsafe attribute" "crates/clax-ios/src/lib.rs:22: $ONLY"

_copy
printf '\n#[allow(unsafe_code)]\nfn helper() {}\n' >> "$IOS"
_expect fail "an allow on a function that is not exported" "crates/clax-ios/src/lib.rs:30: $ONLY"

_copy
printf '#[allow(unsafe_code)]\npub fn other() {}\n' >> "$d/w/crates/clax-ios/src/daemon.rs"
_expect fail "an allow in another file of clax-ios" "crates/clax-ios/src/daemon.rs:4: $ONLY"

_copy
_sub "$IOS" "mod daemon;" "#![allow(unsafe_code)]
mod daemon;"
_expect fail "an inner allow" "crates/clax-ios/src/lib.rs:4: $ONLY"

_copy
_sub "$d/w/crates/clax-ios/Cargo.toml" 'unsafe_code = "deny"' 'unsafe_code = "allow"'
_expect fail "clax-ios allowing unsafe_code" "crates/clax-ios/Cargo.toml: must deny unsafe_code"

_copy
_sub "$d/w/crates/clax-embed/Cargo.toml" 'unsafe_code = "deny"' 'unsafe_code = "allow"'
_expect fail "clax-embed allowing unsafe_code" "crates/clax-embed/Cargo.toml: must deny unsafe_code"

_copy
rm -rf "$d/w/crates/clax-embed"
_expect pass "the tree without clax-embed"

_copy
_sub "$d/w/crates/clax-ios/Cargo.toml" '[lints.rust]
unsafe_code = "deny"' '[lints]
workspace = true'
_expect fail "clax-ios taking the workspace's lints" "crates/clax-ios/Cargo.toml: must deny unsafe_code"

_copy
_sub "$d/w/crates/clax-store/Cargo.toml" '[lints]
workspace = true' '[lints.rust]
unsafe_code = "deny"'
_expect fail "another crate with lints of its own" "crates/clax-store/Cargo.toml: does not take the workspace's lints"

_copy
_sub "$d/w/crates/clax-store/Cargo.toml" '[lints]
workspace = true' ''
_expect fail "another crate with no lints" "crates/clax-store/Cargo.toml: does not take the workspace's lints"

_copy
_sub "$d/w/crates/clax-store/Cargo.toml" '[lints]
workspace = true' '[lints]
workspace = false'
_expect fail "another crate turning the workspace's lints off" "crates/clax-store/Cargo.toml: does not take the workspace's lints"

_copy
_sub "$d/w/crates/clax-store/Cargo.toml" '[lints]
workspace = true' ''
_sub "$d/w/crates/clax-store/Cargo.toml" '[package]' 'lints.workspace = true

[package]'
_expect pass "the workspace's lints as a dotted key"

_copy
_sub "$d/w/Cargo.toml" 'unsafe_code = "forbid"' 'unsafe_code = "deny"'
_expect fail "the workspace no longer forbidding unsafe_code" "Cargo.toml: the workspace must forbid unsafe_code"

_copy
printf 'pub const CSP: &str = "script-src '"'"'unsafe-inline'"'"'";\n' >> "$d/w/crates/clax-core/src/lib.rs"
printf 'pub const CSP: &str = "script-src '"'"'unsafe-inline'"'"'";\n' >> "$IOS"
_expect pass "unsafe-inline in a string"

_copy
printf 'pub fn g() {\n    unsafe { }\n}\n' > "$d/w/crates/clax-core/src/x.rs"
_expect fail "unsafe in another crate" "crates/clax-core/src/x.rs:2:     unsafe { }"

_copy
printf '#[allow(unsafe_code)]\n#[unsafe(no_mangle)]\npub extern "C" fn clax_start(port: u16) -> i32 {\n    0\n}\n' > "$d/w/crates/clax-core/src/x.rs"
_expect fail "an export in another crate" "crates/clax-core/src/x.rs:2: #[unsafe(no_mangle)]"

[ "$fails" = 0 ] || { echo "$fails case(s) failed" >&2; exit 1; }
echo "check-no-unsafe: all cases pass"
