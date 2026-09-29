#!/usr/bin/env bash
# Runs every check CI runs. Pass --verbose to stream each gate's output.
set -euo pipefail
cd "$(dirname "$0")/.."
VERBOSE="${1:-}"
PLAYWRIGHT_INSTALL="npx playwright install chromium"
if [ -n "${CI:-}" ]; then PLAYWRIGHT_INSTALL="npx playwright install --with-deps chromium"; fi
export PLAYWRIGHT_INSTALL
run() {
    local name="$1"; shift
    printf '%-28s' "$name"
    if [ "$VERBOSE" = "--verbose" ]; then echo; "$@"; else
        if out="$("$@" 2>&1)"; then echo ok; else echo FAIL; echo "$out"; exit 1; fi
    fi
}
run "justfile"              scripts/test-justfile.sh
run "installer"             scripts/test-ensure-artifax.sh
run "plugins"               scripts/test-plugins.sh
run "cargo fmt --check"     cargo fmt --all -- --check
run "cargo clippy"          cargo clippy --workspace --all-targets -- -D warnings
run "cargo check (no test features)" env RUSTFLAGS=-Dwarnings cargo check --workspace
run "cargo test"            cargo test --workspace
run "web lint"              bash -c 'cd web && npm ci --silent && npm run lint'
run "web typecheck + unit"  bash -c 'cd web && npm run typecheck && npm test -- --reporter=dot'
run "web build"             bash -c 'cd web && npm run build'
run "pi extension"          bash -c 'cd plugins/pi && npm ci --silent && npm run typecheck && npm test -- --reporter=dot'
run "web e2e"               bash -c 'cd web && $PLAYWRIGHT_INSTALL >/dev/null && npm run e2e'
echo "all gates passed"
