#!/usr/bin/env bash
# The local entry point for checks and builds: runs a task with Bazel when
# it is installed and with the native tools when it is not, and says which.
#
#   scripts/run.sh ci [args]         every check CI runs
#   scripts/run.sh check [args]      auto-format, then ci
#   scripts/run.sh test [args]       the Rust tests
#   scripts/run.sh lint              rustfmt, clippy and the web lint
#   scripts/run.sh web-test          the web UI's lint, type checks, unit tests and bundle gate
#   scripts/run.sh pi-test           the Pi extension's type check and tests
#   scripts/run.sh web-e2e           the browser tests
#   scripts/run.sh perf              the perf gates' full versions
#   scripts/run.sh build             the clax binary
#   scripts/run.sh web               web/dist and web/dist-extension, built
#   scripts/run.sh which             say which build system would run, and why
#   scripts/run.sh --dry-run <task>  say what would run, and run nothing
#
# Which build system (scripts/lib/build-system.sh): the native tools for a
# task whose native build state this worktree already has (target/debug,
# node_modules: faster after an edit), else Bazel when Bazelisk is on PATH,
# else Cargo and pnpm. CLAX_BUILD=bazel|native|auto overrides:
# `bazel` fails rather than fall back, `native` never looks for Bazel. The
# `just` recipes call this; their `-native` twins set CLAX_BUILD=native.
#
# With Bazel a task is what CI runs (scripts/bazel-test.sh and Bazel
# targets). Natively it is what the repository ran before Bazel
# (scripts/quality_gates.sh, cargo nextest, pnpm), unchanged.
#
# When Bazel was wanted and not found, this says so once before the task
# and once after it, with how to install it. After any native run the last
# line says the result came from the native tools: CI gates on Bazel, and
# the two can differ. The exit status is the task's own.
#
# CI does not go through here: its workflow calls scripts/bazel-test.sh
# directly, so a missing Bazel there is a failure and never a fallback.

set -uo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1
# shellcheck source=scripts/lib/build-system.sh
source scripts/lib/build-system.sh

dry=0
if [ "${1:-}" = "--dry-run" ]; then
    dry=1
    shift
fi
task="${1:-}"
[ "$#" -gt 0 ] && shift

usage() {
    sed -n '2,17p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//' >&2
    exit 2
}

# What each task needs when run natively, and the native build state that
# makes it faster natively than under Bazel after an edit (measured in
# docs/bazel.md, "Speed"): with all of it in the worktree, `auto` runs the
# task natively.
BUILD_SYSTEM_WARM=()
case "$task" in
    ci|check) needs=(rust web); BUILD_SYSTEM_WARM=(target/debug web/node_modules plugins/pi/node_modules) ;;
    lint|perf|which) needs=(rust web) ;;
    test) needs=(rust); BUILD_SYSTEM_WARM=(target/debug) ;;
    build) needs=(rust) ;;
    web-test) needs=(web); BUILD_SYSTEM_WARM=(web/node_modules) ;;
    pi-test) needs=(web); BUILD_SYSTEM_WARM=(plugins/pi/node_modules) ;;
    web) needs=(web) ;;
    web-e2e) needs=(rust web); BUILD_SYSTEM_WARM=(target/debug web/node_modules) ;;
    *) usage ;;
esac

if [ "$task" = which ]; then
    why="$(mktemp)"
    if build_system_choose "${needs[@]}" 2>"$why"; then
        rm -f "$why"
        echo "$BUILD_SYSTEM: $BUILD_SYSTEM_WHY"
        exit 0
    fi
    echo "none: $(tr '\n' ' ' < "$why" | sed 's/^error: //; s/  */ /g')"
    rm -f "$why"
    exit 1
fi

build_system_choose "${needs[@]}" || exit 2

# _run <command...>: runs it, or with --dry-run says what it would run.
_run() {
    if [ "$dry" = 1 ]; then
        echo "would run: $*"
        return 0
    fi
    "$@"
}

awake=()
if [ "$dry" = 0 ] && command -v caffeinate >/dev/null 2>&1; then awake=(caffeinate -i); fi

# _copy_dist <dir>: Bazel's build of web/<dir> into the checkout as writable
# regular files (Bazel's outputs are read-only, and a later native build
# must be able to replace them), never links; .gitkeep stays.
_copy_dist() {
    local dir="$1" from
    from="$("$BAZEL" info bazel-bin 2>/dev/null)/web/$dir"
    if [ "$dry" = 1 ]; then
        echo "would run: copy $from to web/$dir"
        return 0
    fi
    [ -d "$from" ] || { echo "error: $from is missing" >&2; return 1; }
    find "web/$dir" -mindepth 1 -maxdepth 1 ! -name .gitkeep -exec rm -rf {} +
    cp -R "$from/." "web/$dir/"
    chmod -R u+w "web/$dir"
}

_bazel() {
    local b
    b=("${awake[@]+"${awake[@]}"}" "$BAZEL")
    case "$task" in
        ci) _run ./scripts/bazel-test.sh --ci "$@" ;;
        check)
            if command -v cargo >/dev/null 2>&1; then
                _run cargo fmt --all || return $?
            else
                echo "── not auto-formatting: cargo is not installed (the format check still runs) ──"
            fi
            _run ./scripts/bazel-test.sh --ci "$@"
            ;;
        test) _run ./scripts/bazel-test.sh --rust "$@" ;;
        lint)
            _run "${b[@]}" build --lockfile_mode=error --config=fmt //crates/... &&
                _run "${b[@]}" build --lockfile_mode=error --config=clippy -- //crates/... -//crates/clax-cli/... &&
                _run "${b[@]}" build --lockfile_mode=error --config=clippy \
                    --@rules_rust//rust/settings:clippy.toml=//crates/clax-cli:clippy.toml //crates/clax-cli/... &&
                _run "${b[@]}" test --lockfile_mode=error //web:lint
            ;;
        web-test) _run "${b[@]}" test --lockfile_mode=error //web:lint //web:typecheck //web:unit //web:bundle_size "$@" ;;
        pi-test) _run "${b[@]}" test --lockfile_mode=error //plugins/pi:typecheck //plugins/pi:test "$@" ;;
        web-e2e) _run "${b[@]}" test --lockfile_mode=error //web:e2e_gesture //web:e2e_chrome_overlay //web:e2e_rest "$@" ;;
        perf) _run "${b[@]}" test --lockfile_mode=error //scripts:perf_daemon_full //scripts:perf_clients_full //web:perf_usable_full "$@" ;;
        build) _run "${b[@]}" build --lockfile_mode=error //crates/clax-cli:clax "$@" ;;
        web)
            _run "${b[@]}" build --lockfile_mode=error //web:dist //web:dist_extension "$@" || return $?
            _copy_dist dist && _copy_dist dist-extension
            ;;
    esac
}

_native() {
    case "$task" in
        ci) _run "${awake[@]+"${awake[@]}"}" ./scripts/quality_gates.sh "$@" ;;
        check) _run "${awake[@]+"${awake[@]}"}" ./scripts/check.sh "$@" ;;
        test) _run "${awake[@]+"${awake[@]}"}" ./scripts/native-test.sh "$@" ;;
        lint)
            _run cargo clippy --workspace --all-targets -- -D warnings &&
                _run cargo clippy --workspace -- -D warnings &&
                _run bash -c 'cd web && pnpm run lint'
            ;;
        web-test) _run bash -c 'cd web && pnpm install --frozen-lockfile && pnpm run lint && pnpm run typecheck && pnpm test' ;;
        pi-test) _run bash -c 'cd plugins/pi && pnpm install --frozen-lockfile && pnpm run typecheck && pnpm test' ;;
        web-e2e)
            _run bash -c 'cd web && pnpm install --frozen-lockfile' && _run ./scripts/build-web.sh &&
                _run bash -c 'cd web && pnpm exec playwright install --with-deps chromium && pnpm run e2e'
            ;;
        perf)
            _run bash -c 'cd web && pnpm install --frozen-lockfile' && _run ./scripts/build-web.sh &&
                _run cargo build -q --release -p clax-cli &&
                _run env CLAX_PERF_BIN="$PWD/target/release/clax" ./scripts/perf-daemon.sh &&
                _run env CLAX_PERF_BIN="$PWD/target/release/clax" ./scripts/perf-clients.sh &&
                _run bash -c 'cd web && pnpm exec playwright install chromium >/dev/null && pnpm run perf'
            ;;
        build) _run cargo build --workspace "$@" ;;
        web) _run bash -c 'cd web && pnpm install --frozen-lockfile' && _run ./scripts/build-web.sh ;;
    esac
}

if [ "$BUILD_SYSTEM" = bazel ]; then
    _bazel "$@"
    exit $?
fi

if [ "$BUILD_SYSTEM_FALLBACK" = 1 ]; then
    echo "── $BUILD_SYSTEM_WHY: running \`$task\` with the native tools (Cargo and pnpm) ──"
    echo "   With Bazelisk the same command keeps its results in a cache every worktree shares,"
    echo "   needs no target/ or node_modules in the worktree, and runs the checks CI runs."
    echo "   Install it: $(build_system_install_hint)"
fi
if [ "$BUILD_SYSTEM_WARM_CHOSEN" = 1 ]; then
    echo "── $BUILD_SYSTEM_WHY: running \`$task\` with the native tools (CLAX_BUILD=bazel runs it under Bazel) ──"
fi
for warning in "${BUILD_SYSTEM_WARNINGS[@]+"${BUILD_SYSTEM_WARNINGS[@]}"}"; do
    echo "   note: $warning"
done

_native "$@"
status=$?

if [ "$status" -eq 0 ]; then result="passed"; else result="failed (exit $status)"; fi
case "$task" in build|web) result="${result/passed/done}" ;; esac
line="── \`$task\` $result with the native tools, not Bazel. CI gates on Bazel, and the two can differ: this does not say what CI will."
if [ "$BUILD_SYSTEM_FALLBACK" = 1 ]; then
    line="$line Install Bazelisk for the same checks: $(build_system_install_hint)"
fi
echo "$line ──"
exit "$status"
