#!/usr/bin/env bash
# Runs `pnpm install --frozen-lockfile` in the given directory unless its
# node_modules was installed from the same pnpm-lock.yaml by the same Node
# and pnpm. The stamp, node_modules/.clax-install-stamp, holds a hash of all
# three and is written only after the install succeeds, so an interrupted
# install runs again next time.
#
# Usage: scripts/pnpm-install-stamped.sh <dir> [pnpm install args...]
# CLAX_PNPM_INSTALL=always ignores the stamp.
set -euo pipefail
dir="$1"; shift
cd "$dir"
stamp="node_modules/.clax-install-stamp"
want="$( { cat pnpm-lock.yaml; node --version; pnpm --version; } | shasum -a 256 | cut -d' ' -f1)"
if [ "${CLAX_PNPM_INSTALL:-}" != "always" ] && [ -f "$stamp" ] && [ "$(cat "$stamp")" = "$want" ]; then
    echo "pnpm install: $dir/node_modules matches pnpm-lock.yaml; skipped"
    exit 0
fi
pnpm install --frozen-lockfile "$@"
printf '%s\n' "$want" > "$stamp"
