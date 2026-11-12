#!/usr/bin/env bash
# Fails, one line per finding, unless every piece of Rust test code under
# ROOT/crates (default: the repository) turns clippy's timing lint back on:
# each `#[cfg(test)]` mod and fn carries `#[warn(clippy::disallowed_methods)]`,
# and each test crate root (tests/, benches/, clax-testkit, clax-fake-exe)
# `#![warn(clippy::disallowed_methods)]`. .bazelrc's build:clippy allows the
# lint on the command line so that product code is never flagged
# (clippy.toml's timing rules); a test scope without the attribute would
# fall out of the rule unnoticed. scripts/check-rust-test-timing.py has
# the details.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
exec python3 "$here/check-rust-test-timing.py" "${1:-$here/..}"
