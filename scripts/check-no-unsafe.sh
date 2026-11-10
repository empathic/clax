#!/usr/bin/env bash
# Fails, one line per finding, unless no Rust source under ROOT/crates
# (default: the repository) uses `unsafe` but the exceptions AGENTS.md
# names, and nothing else relaxes the `unsafe_code` lint:
#
#  - the workspace's Cargo.toml forbids `unsafe_code`;
#  - every crate's Cargo.toml takes the workspace's lints, but
#    crates/clax-embed's and the Worker's SQLite VFS's (below);
#  - crates/clax-embed/Cargo.toml, where there is one, denies
#    `unsafe_code` and forbids `unsafe_op_in_unsafe_fn`, so an unsafe
#    operation in the body of its `unsafe fn` exports needs an `unsafe`
#    block, which this check sees;
#  - in crates/clax-embed/src/ffi.rs the only `unsafe` is, for each of
#    clax_embed_open, clax_embed_call, clax_embed_subscribe and
#    clax_embed_close, once: a line `#[unsafe(no_mangle)]` directly after
#    `#[allow(unsafe_code)]` and directly before `pub unsafe extern "C" fn
#    <name>(` (open and call, which take pointers) or `pub extern "C" fn
#    <name>(` (subscribe and close), every parameter (on that line or the
#    lines up to its `)`) a `u64`, `i64`, `i32`, `usize`, `*const u8` or
#    `ClaxPost`, the grant on that signature line ending at its `(` (the
#    rest of the line is checked as any other); and, in open's and call's
#    bodies, exactly one line
#    `let <a> = unsafe { std::slice::from_raw_parts(<p>, <n>) };` directly
#    after a `// SAFETY:` comment line; `unsafe_code` appears nowhere else in
#    that crate;
#  - in the Worker's SQLite VFS, crates/clax-worker/vfs, whose Cargo.toml
#    denies `unsafe_code` and forbids `unsafe_op_in_unsafe_fn`, the only
#    `unsafe` in src/lib.rs is,
#    once, the line `let registered = unsafe {
#    rsqlite_vfs::register_vfs::<IoMethods, Vfs>(VFS_NAME, (), false) };`
#    directly after `#[allow(unsafe_code)]`, directly after a comment whose
#    first line opens with `// SAFETY:`; no other line there allows the
#    lint;
#  - no other source uses `unsafe`.
#
# A source line counts only where the word appears before any `//` comment,
# and not as the start of a hyphenated word such as CSP's `'unsafe-inline'`.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
cd "${1:-$(dirname "$0")/..}"

sources=()
while IFS= read -r -d '' f; do sources+=("$f"); done \
    < <(find crates -name "*.rs" -not -path "*/target/*" -print0 | sort -z)
python3 "$here/check-no-unsafe.py" ${sources[@]+"${sources[@]}"}
