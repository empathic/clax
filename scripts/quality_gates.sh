#!/usr/bin/env bash
# Runs every check CI runs. Pass --verbose to stream each gate's output, each
# line prefixed with its gate's name.
#
# The web UI is built and its unit tests run first, alone. The other
# independent gates then run concurrently in lanes (web e2e, Rust, web, the
# release build, Pi, scripts), each printing a line per gate as it finishes;
# the lanes that take longest start first. The perf gates then run alone, in
# their quick mode, on the one release binary. At the end a table gives each
# gate's time and the total; times are reported, never judged. `just perf`
# runs the perf gates' full versions.
#
# The script is functions and one call at its end, so bash reads all of it
# before running any of it: editing this file during a run cannot break that
# run.
main() {
set -euo pipefail
cd "$(dirname "$0")/.."
# One run at a time per checkout: parallel runs corrupt each other (an
# `npm ci` replaces node_modules under the other's tests). The lock lives in
# this checkout's git dir, so separate worktrees still run in parallel.
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
GATES_TMP="$(mktemp -d "${TMPDIR:-/tmp}/clax-gates.XXXXXX")"
GATES_TMP="$(cd "$GATES_TMP" && pwd -P)"
LANES=""
START="$(now)"
trap 'finish' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
VERBOSE="${1:-}"
PLAYWRIGHT_INSTALL="npx playwright install chromium"
if [ -n "${CI:-}" ]; then PLAYWRIGHT_INSTALL="npx playwright install --with-deps chromium"; fi
export PLAYWRIGHT_INSTALL
# One debug `clax` (no test features) for every gate that runs the binary:
# the Rust tests that cannot name it (CLAX_TEST_BIN), the plugin wrapper
# test, the comment loop, the Pi tests and the browser tests' daemons. A
# copy, so a later `cargo build` that replaces target/debug/clax cannot pull
# it from under a running gate, kept under target/clax-bin by its content
# (scripts/stable-bin.sh) and named here by a symbolic link. macOS assesses
# a new executable on its first run, which can take minutes on a loaded
# machine: a kept copy pays that once per build, not once per run, and pays
# it when it is made, never inside a gate's time limit (scripts/fake-exe.sh
# does the same for the gates' fakes).
export CLAX_TEST_BIN="$GATES_TMP/debug/clax"
# The one release `clax` the perf gates share, kept the same way.
export CLAX_PERF_BIN="$GATES_TMP/release/clax"

# --- lanes ------------------------------------------------------------------

lane_web() {
    run "web bundle size"       bash -c 'cd web && node scripts/bundle-size.mjs' &&
    run "web lint"              bash -c 'cd web && npm run lint' &&
    run "web typecheck"         bash -c 'cd web && npm run typecheck'
}
lane_pi() {
    run "pi npm ci"             scripts/npm-ci-stamped.sh plugins/pi --silent &&
    await clax-built rust &&
    run "pi extension"          bash -c 'cd plugins/pi && npm run typecheck && npm test -- --reporter=dot'
}
lane_scripts() {
    run "justfile"              scripts/test-justfile.sh &&
    run "release scripts"       scripts/test-release.sh &&
    run "tool hook gate"        scripts/test-tool-hook.sh &&
    run "extension signing"     scripts/test-extension-signing.sh
}
lane_installer() {
    run "release installer"     scripts/test-install.sh
}
lane_dev() {
    run "dev scripts"           scripts/test-dev.sh
}
lane_plugins() {
    run "plugins"               scripts/test-plugins.sh
}
lane_binary() {
    await clax-built rust &&
    run "plugin wrapper"        scripts/test-ensure-clax.sh &&
    run "comment loop"          scripts/smoke-comment-loop.sh
}
# Web e2e starts once cargo has built and checked everything (at once on a
# warm cache), so no compile loads the machine under the browser tests.
lane_e2e() {
    run "playwright browser"    bash -c 'cd web && $PLAYWRIGHT_INSTALL >/dev/null' &&
    await tests-built rust &&
    await linted lint &&
    await release-built release &&
    run "web e2e"               bash -c 'cd web && npm run e2e'
}
# The Rust tests are built before clippy runs, so they start running as soon
# as they can: cargo builds one thing at a time in target/, and clippy's
# checks share no artifacts with the test build. The test build's `clax`
# (the one the Rust tests name) is run once when it is built, as
# CLAX_TEST_BIN is.
lane_rust() {
    run "build clax"            bash -c 'cargo build -q -p clax-cli && mkdir -p "$(dirname "$CLAX_TEST_BIN")" && kept="$(scripts/stable-bin.sh target/debug/clax target/clax-bin/debug)" && ln -sfn "$PWD/$kept" "$CLAX_TEST_BIN"' &&
    mark clax-built &&
    if cargo nextest --version >/dev/null 2>&1; then
        run "build tests"       bash -c 'cargo nextest run --cargo-quiet --workspace --no-run && target/debug/clax --version >/dev/null' &&
        mark tests-built &&
        run "cargo nextest"     cargo nextest run --workspace --no-fail-fast
    else
        run "build tests"       bash -c 'cargo test -q --workspace --no-run && target/debug/clax --version >/dev/null' &&
        mark tests-built &&
        run "cargo test"        bash -c 'echo "WARNING: cargo-nextest is not installed, so the Rust tests ran under cargo test (slower); install it with: cargo install --locked cargo-nextest"; cargo test --workspace'
    fi
}
lane_lint() {
    # No `unsafe` in any Rust source (the `unsafe_code` lint forbids it too);
    # a line counts only where the word appears before any `//` comment.
    run "no unsafe code"        bash -c 'find crates -name "*.rs" -not -path "*/target/*" -print0 | xargs -0 perl -ne '"'"'(my $c = $_) =~ s{//.*}{}; if ($c =~ /\bunsafe\b/) { print "$ARGV:$.: $_"; $bad = 1 } close ARGV if eof; END { exit($bad ? 1 : 0) }'"'"'' &&
    # No hard links in any source or script; a symbolic link does instead.
    run "no hard links"         bash -c "scripts/test-check-no-hard-links.sh >/dev/null && scripts/check-no-hard-links.sh" &&
    # No pipefail pipeline ends in a reader that stops early (grep -q,
    # head): its writer can die of SIGPIPE and fail a pipeline that matched.
    run "pipefail pipes"        bash -c "scripts/test-check-pipefail-pipes.sh >/dev/null && scripts/check-pipefail-pipes.sh" &&
    run "cargo fmt --check"     cargo fmt --all -- --check &&
    await tests-built rust &&
    run "cargo clippy"          cargo clippy --workspace --all-targets -- -D warnings &&
    # The libraries and binaries alone, so without the test-only features
    # that dev-dependencies turn on. Clippy denies rustc's warnings too, and
    # shares the flags and dependency builds of the pass above, which a
    # `RUSTFLAGS=-Dwarnings cargo check` would not.
    run "clippy (no test features)" cargo clippy --workspace -- -D warnings &&
    mark linted
}
lane_release() {
    run "release build"         bash -c 'cargo build -q --release -p clax-cli && mkdir -p "$(dirname "$CLAX_PERF_BIN")" && kept="$(scripts/stable-bin.sh target/release/clax target/clax-bin/release)" && ln -sfn "$PWD/$kept" "$CLAX_PERF_BIN"' &&
    mark release-built
}

# The web UI is built first: a debug build serves web/dist from disk and a
# release build embeds it. The shell's unit tests include timing-sensitive
# ones, so they run next, with nothing else loading the machine.
echo "quality gates: web build and unit tests"
run "web npm ci"            scripts/npm-ci-stamped.sh web --silent || exit 1
run "web build"             scripts/build-web.sh || exit 1
run "web unit"              bash -c 'cd web && npm test -- --reporter=dot' || exit 1

# Web e2e runs beside the other lanes: they wait mostly on processes and
# sockets, not the CPU, while the browser tests need the CPU.
echo "quality gates: concurrent lanes (web e2e, Rust, web, Pi, scripts, release build)"
lane e2e lane_e2e
lane rust lane_rust
lane installer lane_installer
lane binary lane_binary
lane dev lane_dev
lane scripts lane_scripts
lane pi lane_pi
lane web lane_web
lane plugins lane_plugins
lane lint lane_lint
lane release lane_release
FAILED=""
for entry in $LANES; do
    if ! wait "${entry%%:*}"; then FAILED="$FAILED ${entry#*:}"; fi
done
LANES=""
if [ -n "$FAILED" ]; then
    echo
    echo "quality gates: FAIL in lane(s):$FAILED"
    exit 1
fi

# The perf gates measure latency, so they run alone, one at a time; their
# quick modes keep the full runs' budgets and idle-baseline scaling.
echo "quality gates: perf gates, one at a time"
run "daemon latency"        scripts/perf-daemon.sh --quick || exit 1
run "realtime clients"      scripts/perf-clients.sh --quick || exit 1
run "time to usable"        bash -c 'cd web && CLAX_PERF_QUICK=1 npm run perf' || exit 1
echo "all gates passed"
}

# --- helpers (defined before main runs; see the note at the top) -------------

# Seconds since the epoch, to the millisecond.
now() { perl -MTime::HiRes=time -e 'printf "%.3f\n", time'; }

# run NAME CMD...: runs one gate, records its time and prints its verdict.
# Its output goes to a log, printed in full when it fails.
run() {
    local name="$1"; shift
    local log t0 rc
    log="$GATES_TMP/log.$(printf '%s' "$name" | tr -c 'A-Za-z0-9' '_')"
    t0="$(now)"
    if [ "$VERBOSE" = "--verbose" ]; then
        set +e
        "$@" 2>&1 | awk -v p="[$name] " '{ print p $0; fflush() }'
        rc="${PIPESTATUS[0]}"
        set -e
    else
        set +e
        "$@" >"$log" 2>&1
        rc=$?
        set -e
    fi
    local dt
    dt="$(perl -e 'printf "%.1f", $ARGV[1] - $ARGV[0]' "$t0" "$(now)")"
    printf '%s\t%s\t%s\n' "$name" "$dt" "$rc" >> "$GATES_TMP/times"
    if [ "$rc" = 0 ]; then
        # A gate that passed without judging (no budgets on this platform,
        # say) still says so.
        { printf '%-30s ok    %6ss\n' "$name" "$dt"
          [ "$VERBOSE" = "--verbose" ] || grep -E '^WARNING: ' "$log" || true; } | flush
        return 0
    fi
    { printf '%-30s FAIL  %6ss\n' "$name" "$dt"
      [ "$VERBOSE" = "--verbose" ] || sed "s/^/  [$name] /" "$log"; } | flush
    return 1
}

# Writes stdin to stdout in one write where it fits, so concurrent lanes'
# verdicts do not interleave mid-line.
flush() { perl -e 'local $/; my $s = <STDIN>; syswrite STDOUT, $s if defined $s'; }

# lane NAME FUNCTION: runs FUNCTION in the background; NAME.done marks its end.
lane() {
    # shellcheck disable=SC2064 # the lane's name is fixed when it starts
    ( trap "touch '$GATES_TMP/$1.done'" EXIT; "$2" ) &
    LANES="$LANES $!:$1"
}

# mark NAME: tells lanes awaiting NAME to go on.
mark() { touch "$GATES_TMP/$1.ready"; }

# await NAME LANE: waits until NAME is marked; fails when LANE ended without
# marking it (one of its gates failed), or after 30 minutes.
await() {
    local i=0
    while [ ! -e "$GATES_TMP/$1.ready" ]; do
        if [ -e "$GATES_TMP/$2.done" ] && [ ! -e "$GATES_TMP/$1.ready" ]; then
            printf '%-30s skipped: the %s lane failed\n' "(waiting for $1)" "$2" | flush
            return 1
        fi
        i=$((i + 1))
        if [ "$i" -gt 9000 ]; then echo "quality gates: gave up waiting for $1" | flush; return 1; fi
        sleep 0.2
    done
}

# Every descendant of a PID, children before their parent's siblings.
descendants() {
    local kid
    for kid in $(ps -A -o pid= -o ppid= | awk -v p="$1" '$2 == p { print $1 }'); do
        echo "$kid"
        descendants "$kid"
    done
}

# The timing table, the lanes stopped, the scratch directory and the lock removed.
finish() {
    local rc=$? entry pids
    for entry in $LANES; do
        pids="${entry%%:*} $(descendants "${entry%%:*}")"
        kill $pids 2>/dev/null || true
    done
    if [ -s "$GATES_TMP/times" ]; then
        echo
        echo "gate times (reported only; a slow gate never fails the run):"
        awk -F'\t' '{ printf "  %-30s %7ss%s\n", $1, $2, ($3 == 0 ? "" : "  (failed)") }' "$GATES_TMP/times"
        perl -e 'printf "  %-30s %7.1fs\n", "total (wall clock)", $ARGV[1] - $ARGV[0]' "$START" "$(now)"
    fi
    rm -rf "$GATES_TMP" "$LOCK"
    exit "$rc"
}

main "$@"; exit
