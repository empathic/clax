#!/usr/bin/env bash
# Checks scripts/check-no-unsafe.sh on a copy of it and of the
# check-no-unsafe.py it runs, beside a minimal workspace (two crates that
# take the workspace's lints, the separate clax-worker workspace, and
# crates/clax-embed): the tree passes, from any
# directory and with or without a root given; clax-embed not denying the
# lint, the workspace not forbidding it, and another crate with lints of
# its own (clax-ios's former exception among them) each fail, naming the
# finding; in crates/clax-embed/src/ffi.rs, a fifth
# export, an unsafe subscribe or close, a safe open or call, a parameter
# of another type, a second from_raw_parts or one without its SAFETY line,
# any other unsafe expression, and unsafe or an allow anywhere else in the
# crate, `unsafe` after the opening `(` of a signature line, and
# clax-embed not forbidding unsafe_op_in_unsafe_fn fail (an unsafe
# operation in an `unsafe fn` with no `unsafe` block carries no keyword the
# check could see: only that lint refuses it); CSP's `'unsafe-inline'`
# passes; `unsafe` in another crate fails as it always has.
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
    cp scripts/check-no-unsafe.sh scripts/check-no-unsafe.py "$d/w/scripts/"
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
    mkdir -p "$d/w/crates/clax-embed/src"
    cat > "$d/w/crates/clax-embed/Cargo.toml" <<'EOF'
[package]
name = "clax-embed"
version.workspace = true

# The workspace forbids unsafe_code; this crate denies it.
[lints.rust]
unsafe_code = "deny"
unsafe_op_in_unsafe_fn = "forbid"
unexpected_cfgs = { level = "warn", check-cfg = ['cfg(clax_embedded)'] }

[lib]
crate-type = ["rlib", "cdylib", "staticlib"]

[dependencies]
EOF
    printf '//! The core in an app, behind C functions.\n\npub fn f() -> i32 {\n    1\n}\n' > "$d/w/crates/clax-embed/src/lib.rs"
    cat > "$d/w/crates/clax-embed/src/ffi.rs" <<'EOF'
//! The four C functions; `unsafe` in a comment is skipped.

use crate::dart::ClaxPost;

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clax_embed_open(
    home: *const u8,
    home_len: usize,
    post: ClaxPost,
    port: i64,
) -> u64 {
    // SAFETY: Dart passes home_len readable bytes.
    let home = unsafe { std::slice::from_raw_parts(home, home_len) };
    crate::ffi_open(home, post, port)
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clax_embed_call(
    core: u64,
    req: *const u8,
    req_len: usize,
    port: i64,
    tag: i64,
) -> i32 {
    // SAFETY: Dart passes req_len readable bytes.
    let req = unsafe { std::slice::from_raw_parts(req, req_len) };
    crate::ffi_call(core, req, port, tag)
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn clax_embed_subscribe(core: u64, port: i64) -> i32 {
    crate::ffi_subscribe(core, port)
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn clax_embed_close(core: u64) -> i32 {
    crate::ffi_close(core)
}
EOF
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
ONLY="only the four exported functions may use unsafe"

_copy
_expect pass "the tree"

_copy
out="$(scripts/check-no-unsafe.sh "$d/w" 2>&1)" || _fail "the tree, as a root argument: $out"

_copy
_sub "$d/w/crates/clax-embed/Cargo.toml" 'unsafe_code = "deny"' 'unsafe_code = "allow"'
_expect fail "clax-embed allowing unsafe_code" "crates/clax-embed/Cargo.toml: must deny unsafe_code"

_copy
rm -rf "$d/w/crates/clax-embed"
_expect pass "the tree without clax-embed"

FFI="$d/w/crates/clax-embed/src/ffi.rs"
EMBED_LIB="$d/w/crates/clax-embed/src/lib.rs"

_copy
_sub "$FFI" "pub extern \"C\" fn clax_embed_subscribe(core: u64, port: i64) -> i32 {
    crate::ffi_subscribe(core, port)
}" "pub extern \"C\" fn clax_embed_subscribe(core: u64, port: i64) -> i32 { crate::ffi_subscribe(core, port) }"
_expect pass "an export on one line"

_copy
_sub "$FFI" "pub extern \"C\" fn clax_embed_close(core: u64) -> i32 {
    crate::ffi_close(core)
}" "pub extern \"C\" fn clax_embed_close(core: u64) -> i32 { unsafe { *(core as *mut u8) = 0 }; crate::ffi_close(core) }"
_expect fail "unsafe after the brace of an export on one line" "crates/clax-embed/src/ffi.rs:40: $ONLY"

_copy
_sub "$FFI" "pub extern \"C\" fn clax_embed_subscribe(core: u64, port: i64) -> i32 {" "pub extern \"C\" fn clax_embed_subscribe(core: u64, port: i64) -> i32 { let _ = unsafe { 0 };"
_expect fail "unsafe after the brace of a signature line" "crates/clax-embed/src/ffi.rs:34: $ONLY"

_copy
_sub "$d/w/crates/clax-embed/Cargo.toml" 'unsafe_op_in_unsafe_fn = "forbid"
' ''
_sub "$FFI" "    crate::ffi_open(home, post, port)" "    let _b = Box::from_raw(home.as_ptr() as *mut [u8; 4]);
    crate::ffi_open(home, post, port)"
_expect fail "an unsafe operation with no unsafe block, the lint not forbidden" "crates/clax-embed/Cargo.toml: must forbid unsafe_op_in_unsafe_fn"

_copy
_sub "$d/w/crates/clax-embed/Cargo.toml" 'unsafe_op_in_unsafe_fn = "forbid"' 'unsafe_op_in_unsafe_fn = "deny"'
_expect fail "clax-embed only denying unsafe_op_in_unsafe_fn" "crates/clax-embed/Cargo.toml: must forbid unsafe_op_in_unsafe_fn"

_copy
printf '\n#[allow(unsafe_code)]\n#[unsafe(no_mangle)]\npub extern "C" fn clax_embed_extra(core: u64) -> i32 {\n    0\n}\n' >> "$FFI"
_expect fail "a fifth export in clax-embed" "crates/clax-embed/src/ffi.rs:44: $ONLY"

_copy
printf '\n#[allow(unsafe_code)]\n#[unsafe(no_mangle)]\npub extern "C" fn clax_embed_close(core: u64) -> i32 {\n    0\n}\n' >> "$FFI"
_expect fail "a clax-embed export named twice" "crates/clax-embed/src/ffi.rs:44: $ONLY"

_copy
_sub "$FFI" 'pub extern "C" fn clax_embed_subscribe(' 'pub unsafe extern "C" fn clax_embed_subscribe('
_expect fail "an unsafe subscribe" "crates/clax-embed/src/ffi.rs:34: $ONLY"

_copy
_sub "$FFI" 'pub unsafe extern "C" fn clax_embed_open(' 'pub extern "C" fn clax_embed_open('
_expect fail "an open that is not an unsafe fn" "crates/clax-embed/src/ffi.rs:14: $ONLY"

_copy
_sub "$FFI" "    req: *const u8," "    req: *mut u8,"
_expect fail "a *mut parameter" "crates/clax-embed/src/ffi.rs:20: $ONLY"

_copy
_sub "$FFI" "clax_embed_subscribe(core: u64, port: i64)" "clax_embed_subscribe(core: u64, port: u16)"
_expect fail "a parameter of another type" "crates/clax-embed/src/ffi.rs:33: $ONLY"

_copy
_sub "$FFI" "    let req = unsafe { std::slice::from_raw_parts(req, req_len) };
" "    let req = unsafe { std::slice::from_raw_parts(req, req_len) };
    // SAFETY: the same bytes.
    let again = unsafe { std::slice::from_raw_parts(req.as_ptr(), req_len) };
"
_expect fail "a second from_raw_parts in call" "crates/clax-embed/src/ffi.rs:30: $ONLY"

_copy
_sub "$FFI" "    // SAFETY: Dart passes req_len readable bytes." "    // Dart passes req_len readable bytes."
_expect fail "a from_raw_parts without its SAFETY line" "crates/clax-embed/src/ffi.rs:28: $ONLY"

_copy
_sub "$FFI" "    let home = unsafe { std::slice::from_raw_parts(home, home_len) };" "    let home = unsafe { Box::from_raw(home as *mut [u8; 4]) };"
_expect fail "Box::from_raw in an export" "crates/clax-embed/src/ffi.rs:14: $ONLY"

_copy
printf 'pub fn g() {\n    unsafe {}\n}\n' >> "$EMBED_LIB"
_expect fail "unsafe in clax-embed's lib.rs" "crates/clax-embed/src/lib.rs:7: $ONLY"

_copy
printf '#[allow(unsafe_code)]\nfn h() {}\n' >> "$EMBED_LIB"
_expect fail "an allow elsewhere in clax-embed" "crates/clax-embed/src/lib.rs:6: $ONLY"

_copy
cp "$FFI" "$d/w/crates/clax-core/src/ffi.rs"
_expect fail "clax-embed's exports in another crate" "crates/clax-core/src/ffi.rs:6: #[unsafe(no_mangle)]"

_copy
mkdir -p "$d/w/crates/clax-ios/src"
printf '[package]\nname = "clax-ios"\nversion.workspace = true\n\n[lints.rust]\nunsafe_code = "deny"\n\n[dependencies]\n' > "$d/w/crates/clax-ios/Cargo.toml"
printf '#[allow(unsafe_code)]\n#[unsafe(no_mangle)]\npub extern "C" fn clax_start(port: u16) -> i32 {\n    0\n}\n' > "$d/w/crates/clax-ios/src/lib.rs"
_expect fail "clax-ios, no longer an exception, with lints of its own" "crates/clax-ios/Cargo.toml: does not take the workspace's lints"
_expect fail "clax-ios, no longer an exception, exporting a C function" "crates/clax-ios/src/lib.rs:2: #[unsafe(no_mangle)]"

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
printf 'pub const CSP: &str = "script-src '"'"'unsafe-inline'"'"'";\n' >> "$EMBED_LIB"
_expect pass "unsafe-inline in a string"

_copy
printf 'pub fn g() {\n    unsafe { }\n}\n' > "$d/w/crates/clax-core/src/x.rs"
_expect fail "unsafe in another crate" "crates/clax-core/src/x.rs:2:     unsafe { }"

_copy
printf '#[allow(unsafe_code)]\n#[unsafe(no_mangle)]\npub extern "C" fn clax_start(port: u16) -> i32 {\n    0\n}\n' > "$d/w/crates/clax-core/src/x.rs"
_expect fail "an export in another crate" "crates/clax-core/src/x.rs:2: #[unsafe(no_mangle)]"

[ "$fails" = 0 ] || { echo "$fails case(s) failed" >&2; exit 1; }
echo "check-no-unsafe: all cases pass"
