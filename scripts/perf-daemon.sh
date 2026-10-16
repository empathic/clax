#!/usr/bin/env bash
# Daemon latency gate: starts a release `clax` on a scratch CLAX_HOME and a
# free port, seeds a large home, and checks that cheap requests stay within
# scripts/perf-daemon-budget.json while heavy ones run alongside (see
# scripts/perf-daemon.py for the loads and how the budgets are judged).
# Prints a table; exits non-zero when a median is over its limit.
#
# Usage: scripts/perf-daemon.sh [--quick]
# --quick runs the short version quality_gates.sh uses: the same budgets,
# fewer and shorter measurements (the budget file's `quick` overrides).
# CLAX_PERF_BIN=<path> uses that binary instead of building the release one.
# The release build embeds web/dist, so build the web UI first
# (`cd web && pnpm run build`); quality_gates.sh does.
set -euo pipefail
cd "$(dirname "$0")/.."

BIN="${CLAX_PERF_BIN:-}"
# Absolute, as Bazel names it relative to the repository (runfiles) root.
case "$BIN" in "" | /*) ;; *) BIN="$PWD/$BIN" ;; esac
if [ -z "$BIN" ]; then
    if [ ! -f web/dist/index.html ] || [ ! -f web/dist/artifact.html ]; then
        echo "perf-daemon: web/dist holds no built web UI; run 'cd web && pnpm run build' first" >&2
        exit 2
    fi
    cargo build -q --release -p clax-cli
    BIN="$PWD/target/release/clax"
fi
exec python3 scripts/perf-daemon.py "$@" "$BIN" scripts/perf-daemon-budget.json
