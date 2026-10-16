#!/usr/bin/env bash
# The Rust workspace tests with Cargo (the shell tests need `just web` once
# first): cargo nextest when installed, with one prebuilt clax for the tests
# that cannot name it (CLAX_TEST_BIN), kept by content (scripts/stable-bin.sh)
# so macOS assesses it once per build rather than once per run.
set -euo pipefail
cd "$(dirname "$0")/.."
if cargo nextest --version >/dev/null 2>&1; then
    cargo build -q -p clax-cli
    bin="$(scripts/stable-bin.sh target/debug/clax target/clax-bin/debug)"
    CLAX_TEST_BIN="$PWD/$bin" cargo nextest run --workspace "$@"
else
    cargo test --workspace "$@"
fi
