#!/usr/bin/env bash
# Formats the Rust code, then runs every quality gate.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo fmt --all
exec scripts/quality_gates.sh "$@"
