#!/usr/bin/env bash
# The checks of crates/clax-worker that run natively, with Cargo and npm:
# the Worker is a Cargo workspace of its own (with vfs/, its SQLite VFS) and
# its own Cargo.lock, which Bazel does not resolve, and its wasm build needs
# worker-build and a clang that targets wasm32 (docs/bazel.md, "Run outside
# Bazel"). In order: npm ci, its tests (the VFS's among them), clippy with
# warnings denied, test-deploy-config.mjs (the config deploy.mjs generates,
# without wrangler), then test-wasm.mjs, which builds the wasm Worker and
# checks it under `wrangler dev --local`, test-sandbox.mjs, ClaxBot's
# sandbox conformance suite on that build, and test-bot.mjs, a ClaxBot run
# driven by the instance's alarm against a scripted Messages API, across a
# restart.
#
# Its format check is not here: under Bazel it is //crates/clax-worker:rustfmt,
# natively `cargo fmt -- --check` in crates/clax-worker (scripts/run.sh
# worker-test). Both check the workspace's own sources only, never the Clax
# crates it uses by path, which the root's format check covers.
set -euo pipefail
cd "$(dirname "$0")/.."
./scripts/npm-ci-stamped.sh crates/clax-worker --silent
cd crates/clax-worker
cargo test -q --workspace
cargo clippy -q --workspace --all-targets -- -D warnings
node test-deploy-config.mjs
node test-wasm.mjs
node test-sandbox.mjs --no-build
node test-bot.mjs --no-build
