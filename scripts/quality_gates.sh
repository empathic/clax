#!/usr/bin/env bash
# Runs every check CI runs. Pass --verbose to stream each gate's output.
# The whole script is one function, so bash reads all of it before running
# any of it: editing this file during a run cannot break that run.
main() {
set -euo pipefail
cd "$(dirname "$0")/.."
# One run at a time per checkout: parallel runs corrupt each other (each
# `npm ci` replaces web/node_modules under the other's tests). The lock lives
# in this checkout's git dir, so separate worktrees still run in parallel.
LOCK="$(git rev-parse --git-dir)/quality-gates.lock"
while ! mkdir "$LOCK" 2>/dev/null; do
    holder="$(cat "$LOCK/pid" 2>/dev/null || true)"
    if [ -n "$holder" ] && ! kill -0 "$holder" 2>/dev/null; then
        rm -rf "$LOCK"; continue
    fi
    echo "quality gates: waiting for the run holding $LOCK (pid ${holder:-unknown})" >&2
    sleep 10
done
echo $$ > "$LOCK/pid"
trap 'rm -rf "$LOCK"' EXIT
VERBOSE="${1:-}"
PLAYWRIGHT_INSTALL="npx playwright install chromium"
if [ -n "${CI:-}" ]; then PLAYWRIGHT_INSTALL="npx playwright install --with-deps chromium"; fi
export PLAYWRIGHT_INSTALL
run() {
    local name="$1"; shift
    printf '%-28s' "$name"
    if [ "$VERBOSE" = "--verbose" ]; then echo; "$@"; else
        if out="$("$@" 2>&1)"; then
            echo ok
            # A gate that passed without judging (no budgets on this
            # platform, say) still says so.
            grep -E '^WARNING: ' <<<"$out" || true
        else echo FAIL; echo "$out"; exit 1; fi
    fi
}
run "justfile"              scripts/test-justfile.sh
run "plugin wrapper"        scripts/test-ensure-clax.sh
run "release scripts"       scripts/test-release.sh
run "release installer"     scripts/test-install.sh
run "tool hook gate"          scripts/test-tool-hook.sh
run "dev scripts"           scripts/test-dev.sh
run "plugins"               scripts/test-plugins.sh
# The web UI is built before the cargo gates: a debug build serves web/dist
# from disk and a release build embeds it, so `cargo test` and the comment
# loop need it, and a fresh checkout holds only web/dist/.gitkeep.
run "web lint"              bash -c 'cd web && npm ci --silent && npm run lint'
run "web typecheck + unit"  bash -c 'cd web && npm run typecheck && npm test -- --reporter=dot'
run "web build"             bash -c 'cd web && npm run build'
run "web bundle size"       bash -c 'cd web && node scripts/bundle-size.mjs'
run "cargo fmt --check"     cargo fmt --all -- --check
run "cargo clippy"          cargo clippy --workspace --all-targets -- -D warnings
run "cargo check (no test features)" env RUSTFLAGS=-Dwarnings cargo check --workspace
run "cargo test"            cargo test --workspace
run "daemon latency"        scripts/perf-daemon.sh
run "comment loop"          scripts/smoke-comment-loop.sh
run "pi extension"          bash -c 'cd plugins/pi && npm ci --silent && npm run typecheck && npm test -- --reporter=dot'
run "web e2e"               bash -c 'cd web && $PLAYWRIGHT_INSTALL >/dev/null && npm run e2e'
run "time to usable"        bash -c 'cd web && npm run perf'
echo "all gates passed"
}
main "$@"; exit
