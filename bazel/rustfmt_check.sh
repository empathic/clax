#!/usr/bin/env bash
# rustfmt_check.sh RUSTFMT CONFIG EDITION FILE...: fails unless every FILE is
# formatted, with the command rules_rust's rustfmt aspect runs
# (`--config=fmt`): the toolchain's rustfmt, the aspect's configuration,
# each file on its own (skip_children), --check. For Rust sources that no
# Bazel Rust target holds (crates/clax-worker, docs/bazel.md "Targets").
set -euo pipefail
rustfmt="$1" config="$2" edition="$3"
shift 3
exec "$rustfmt" --config-path "$config" --edition "$edition" --config skip_children=true --check "$@"
