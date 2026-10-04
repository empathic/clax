#!/usr/bin/env bash
# Runs `npm ci` in the given directory unless its node_modules was installed
# from the same package-lock.json by the same Node and npm. The stamp,
# node_modules/.clax-ci-stamp, holds a hash of all three and is written only
# after `npm ci` succeeds, so an interrupted install runs again next time.
#
# Usage: scripts/npm-ci-stamped.sh <dir> [npm ci args...]
# CLAX_NPM_CI=always ignores the stamp.
set -euo pipefail
dir="$1"; shift
cd "$dir"
stamp="node_modules/.clax-ci-stamp"
want="$( { cat package-lock.json; node --version; npm --version; } | shasum -a 256 | cut -d' ' -f1)"
if [ "${CLAX_NPM_CI:-}" != "always" ] && [ -f "$stamp" ] && [ "$(cat "$stamp")" = "$want" ]; then
    echo "npm ci: $dir/node_modules matches package-lock.json; skipped"
    exit 0
fi
npm ci "$@"
printf '%s\n' "$want" > "$stamp"
