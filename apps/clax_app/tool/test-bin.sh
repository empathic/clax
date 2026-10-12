#!/usr/bin/env bash
# Prints the absolute path of a debug clax kept by its content, for CLAX_TEST_BIN.
# Builds clax-cli first; when that build fails (another change is mid-edit),
# prints the newest kept binary instead and warns on stderr. Exits 1 when
# there is none.
set -euo pipefail
cd "$(dirname "$0")/../../.."
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
