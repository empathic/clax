#!/usr/bin/env bash
# Realtime load gate: starts a release `clax` on a scratch CLAX_HOME and a
# free port, opens many `/api/stream` clients, writes steadily, and checks
# delivery latency, cheap requests under that load, memory per client, idle
# CPU and a slow client's `resync` against scripts/perf-clients-budget.json
# (see scripts/perf-clients.py for the run and how it is judged).
# Prints a table; exits non-zero when a measure is over its limit.
#
# Usage: scripts/perf-clients.sh [--quick]
# --quick runs the short version quality_gates.sh uses: the same budgets,
# fewer and shorter measurements (the budget file's `quick` overrides).
# CLAX_PERF_BIN=<path> uses that binary instead of building the release one.
# CLAX_PERF_CLIENTS=<n> opens n clients instead of the budget's count.
# The release build embeds web/dist, so build the web UI first
# (`cd web && npm run build`); quality_gates.sh does.
set -euo pipefail
cd "$(dirname "$0")/.."

BIN="${CLAX_PERF_BIN:-}"
if [ -z "$BIN" ]; then
    if [ ! -f web/dist/index.html ] || [ ! -f web/dist/artifact.html ]; then
        echo "perf-clients: web/dist holds no built web UI; run 'cd web && npm run build' first" >&2
        exit 2
    fi
    cargo build -q --release -p clax-cli
    BIN="$PWD/target/release/clax"
fi
exec python3 scripts/perf-clients.py "$@" "$BIN" scripts/perf-clients-budget.json
