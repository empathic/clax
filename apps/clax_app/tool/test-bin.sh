#!/usr/bin/env bash
# Prints the absolute path of a debug clax for CLAX_TEST_BIN.
#
# With Bazel (scripts/run.sh's choice: Bazelisk on PATH, unless
# CLAX_BUILD=native) it is Bazel's debug clax (`scripts/run.sh bin`), from
# the cache every worktree shares. Natively it is Cargo's, kept by its
# content (scripts/stable-bin.sh); when that build fails (another change is
# mid-edit), the newest kept binary instead, with a warning on stderr.
# Exits 1 when there is none.
set -euo pipefail
cd "$(dirname "$0")/../../.."
# shellcheck source=scripts/lib/build-system.sh
source scripts/lib/build-system.sh
build_system_choose rust >/dev/null 2>&1 || true
if [ "${BUILD_SYSTEM:-}" = bazel ]; then
    exec scripts/run.sh bin
fi
if cargo build -q -p clax-cli; then
    kept="$(scripts/stable-bin.sh target/debug/clax target/clax-bin/debug)"
    echo "$PWD/$kept"
    exit 0
fi
# ls fails when nothing is kept; that is the empty case below, not an error.
newest="$(ls -t target/clax-bin/debug/*/clax 2>/dev/null | awk 'NR == 1' || true)"
if [ -z "$newest" ]; then
    echo "test-bin: cargo build failed and no kept clax exists under target/clax-bin/debug" >&2
    exit 1
fi
echo "test-bin: cargo build failed; using the newest kept clax, $newest" >&2
echo "$PWD/$newest"
