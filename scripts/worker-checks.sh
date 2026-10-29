#!/usr/bin/env bash
# The checks of crates/clax-worker that run with npm and wrangler, outside
# the workspace's Rust tests and clippy (docs/bazel.md, "Run outside
# Bazel"). In order: pnpm install, test-deploy-config.mjs (the config deploy.mjs generates,
# without wrangler), then test-wasm.mjs, which builds the wasm Worker and
# checks it under `wrangler dev --local`, test-sandbox.mjs, ClaxBot's
# sandbox conformance suite on that build, test-bot.mjs, a ClaxBot run
# driven by the instance's alarm against a scripted Messages API, across a
# restart, and test-feedback.mjs, clax.dev's own feedback path over HTTP.
#
set -euo pipefail
cd "$(dirname "$0")/.."
./scripts/pnpm-install-stamped.sh crates/clax-worker --silent
cd crates/clax-worker
node test-deploy-config.mjs
node test-wasm.mjs
node test-sandbox.mjs --no-build
node test-bot.mjs --no-build
node test-feedback.mjs --no-build
