# The hosted Worker under Bazel

`crates/clax-worker` (the Cloudflare Worker) and `crates/clax-worker/vfs`
(its SQLite VFS) are members of the root Cargo workspace. Natively they build
for their tests and clippy, like every crate; the Worker ships only for
`wasm32-unknown-unknown`. This directory holds what Bazel needs for that
target.

## Two crate universes

`@crates` resolves the whole workspace, as `cargo test --workspace` does,
for the native and mobile platforms. It cannot also serve wasm32:
crate_universe keeps one feature list per crate and platform for the whole
workspace, so wasm32 would get the daemon's features (tokio's `full`,
reqwest's `blocking`), and a proc macro, built once per host, would get the
features the Worker's graph asks of it in native builds too (rmcp-macros'
`local`, which leaves `Send` off the MCP tools' futures and breaks
`clax-mcp` natively).

`@crates_wasm` resolves what `cargo build -p clax-worker --target
wasm32-unknown-unknown` resolves, and nothing else. Its specs,
`crates.MODULE.bazel`, are one `crate.spec` per third-party crate that build
reaches, at its `Cargo.lock` version with the features Cargo gives it; its
lockfile, `Cargo.lock` here, is the root one pruned to them.
`scripts/bazel-sync.sh` derives both (`scripts/bazel-derive.py
wasm-specs`), and its `--check` fails when a crate's features there differ
from Cargo's (`WASM_EXTRA`), when the pruned lockfile holds a version the
root one does not, or when the wasm-bindgen CLI is not the version of the
crate.

No workspace crate is a spec. crate_universe builds a path crate's
repository over its source directory and writes its generated
`BUILD.bazel` there, over the crate's own.

For each workspace crate the Worker builds, `bazel/workspace.bzl` lists its
wasm32 dependencies (`wasm_path_deps`, `wasm_deps`, `wasm_proc_macro_deps`:
those whose `cfg()` holds on wasm32 and that its default features turn on),
and `cargo_package()` declares `<lib>_wasm` (and, for the Worker's cdylib,
`<lib>_wasm_shared`) from the same sources: a second target rather than a
`select` on `<lib>`'s dependencies, since `all_crate_deps()` is itself a
`select` and selects do not nest.

## C for wasm32

ring and sqlite-wasm-rs compile C. cc-rs reads `CC_wasm32_unknown_unknown`
and `AR_wasm32_unknown_unknown`; the annotations in `MODULE.bazel` set them
to the Android NDK's clang and llvm-ar (`c_tools.bzl`), which have a
WebAssembly backend. Bazel already fetches that NDK by checksum for the app;
its prebuilt LLVM is for macOS and x86-64 Linux.

## The bundle

`//crates/clax-worker:bundle` (`worker.bzl`) builds `clax_worker_wasm_shared`
for wasm32 with the root `Cargo.toml`'s `worker-release` profile (opt-level
"z", fat LTO, one codegen unit), then runs `worker_bundle.mjs`: worker-build
0.8.7's steps after rustc (wasm-bindgen, wasm-opt, the producers section,
the shim, esbuild) with the tools `tools.MODULE.bazel` pins.
`crates/clax-worker/build.sh` builds it and links
`target/clax-worker/build`, which `wrangler.jsonc`'s `main` names, to
Bazel's output; without Bazel it runs worker-build into that directory.

The web UI the Worker embeds is `//web:dist` built for the host
(`host_files.bzl`): it builds with Node, which has no wasm32 toolchain.

`//bazel/wasm:check` builds every `<lib>_wasm` with warnings denied:
`scripts/check-wasm.sh` under Bazel.
