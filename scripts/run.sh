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
#   scripts/run.sh perf              the perf benchmarks, on demand (they gate nothing; on a
#                                    Mac also the Clax app's launch, scripts/perf-app-launch.sh)
#   scripts/run.sh worker-test       crates/clax-worker's checks: its format check (Bazel's
#                                    rustfmt, or cargo fmt), then scripts/worker-checks.sh,
#                                    natively either way (its own Cargo workspace)
#   scripts/run.sh build             the clax binary
#   scripts/run.sh web               web/dist and web/dist-extension, built
#   scripts/run.sh clax [args]       build the debug clax and run it (serve, stop, doctor, …)
#   scripts/run.sh bin               build the debug clax and print its path
#   scripts/run.sh install           build the release clax and install it in $CARGO_HOME/bin
#                                    (CLAX_INSTALL_BIN=<binary> installs that one instead: tests)
#   scripts/run.sh rust-project      rust-project.json for rust-analyzer, from Bazel
#   scripts/run.sh app-test          the Clax app's analysis and tests (apps/clax_app)
#   scripts/run.sh app-integration   the Clax app's macOS integration tests (they open its window;
#                                    --ios the simulator's, --android the emulator's)
#                                    (--ios: its simulator ones instead, on "Clax iPhone"; Bazel only)
#   scripts/run.sh app-apk           the Clax app's release APK, and print its path
#   scripts/run.sh app-macos         the Clax app's macOS app, and print its path
#   scripts/run.sh home-bundle       the home app's bundle, as the app builds pack it, and print its path
#   scripts/run.sh which             say which build system would run, and why
#   scripts/run.sh --dry-run <task>  say what would run, and run nothing
#
# Which build system (scripts/lib/build-system.sh): Bazel when Bazelisk is
# on PATH, else Cargo and pnpm. CLAX_BUILD=bazel|native|auto overrides:
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
    sed -n '2,/^#   scripts\/run.sh --dry-run/p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//' >&2
    exit 2
}

# What each task needs when run natively.
case "$task" in
    ci|check|lint|perf|which) needs=(rust web) ;;
    test|build|clax|bin|rust-project) needs=(rust) ;;
    install) needs=(rust web) ;;
    web-test|pi-test|web) needs=(web) ;;
    web-e2e) needs=(rust web) ;;
    worker-test) needs=(rust) ;;
    app-test|app-integration|app-apk|app-macos) needs=(rust web flutter) ;;
    home-bundle) needs=(flutter) ;;
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

# _bazel_bin <label>: builds a binary and prints its absolute path; Bazel's
# messages go to stderr.
_bazel_bin() {
    local out
    _run "${awake[@]+"${awake[@]}"}" "$BAZEL" build --lockfile_mode=error "$1" >&2 || return $?
    if [ "$dry" = 1 ]; then echo "<the path of $1>"; return 0; fi
    out="$("$BAZEL" cquery --lockfile_mode=error --output=files "$1" 2>/dev/null | sed -n 1p)"
    [ -n "$out" ] || { echo "error: no output file for $1" >&2; return 1; }
    echo "$PWD/$out"
}

# _bazel_out <label>: builds a target and prints the absolute path of its
# output (an APK, an app's zip, a binary); Bazel's messages go to stderr.
_bazel_out() {
    local out
    _run "${awake[@]+"${awake[@]}"}" "$BAZEL" build --lockfile_mode=error "$1" >&2 || return $?
    if [ "$dry" = 1 ]; then echo "<the output of $1>"; return 0; fi
    out="$("$BAZEL" cquery --lockfile_mode=error --output=files "$1" 2>/dev/null | sed -n 1p)"
    [ -n "$out" ] || { echo "error: no output file for $1" >&2; return 1; }
    echo "$PWD/$out"
}

# _install <binary>: installs it as $CARGO_HOME/bin/clax by a rename, never a
# copy over the running file (macOS kills a binary overwritten in place).
_install() {
    local dest="${CARGO_HOME:-$HOME/.cargo}/bin"
    if [ "$dry" = 1 ]; then echo "would run: install $1 as $dest/clax"; return 0; fi
    mkdir -p "$dest"
    cp "$1" "$dest/.clax.new.$$" && chmod 755 "$dest/.clax.new.$$" && mv -f "$dest/.clax.new.$$" "$dest/clax" &&
        echo "installed $dest/clax ($("$dest/clax" --version))"
}

# _perf_app_launch: on a Mac, after the other benchmarks, the Clax app's
# cold start to a usable home screen (scripts/perf-app-launch.sh). It
# gates nothing: a launch that cannot be measured is reported and the run
# goes on.
_perf_app_launch() {
    [ "$(uname -s)" = Darwin ] || return 0
    _run env CLAX_PERF_JSON="$PERF_JSON" ./scripts/perf-app-launch.sh --macos --i-know-what-i-am-doing ||
        echo "── the Clax app's launch was not measured (it gates nothing; scripts/perf-app-launch.sh says why) ──"
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
            # Cargo reads a crate's clippy.toml from beside its Cargo.toml;
            # Bazel takes one per command, so each crate that has one is
            # linted alone with it (as scripts/bazel-test.sh does), and the
            # rest with the root one.
            local toml crate own=() rest=(//crates/...)
            for toml in crates/*/clippy.toml; do
                [ -f "$toml" ] || continue
                crate="$(dirname "$toml")"
                own+=("$crate")
                rest+=("-//$crate/...")
            done
            _run "${b[@]}" build --lockfile_mode=error --config=fmt //crates/... &&
                _run "${b[@]}" build --lockfile_mode=error --config=clippy -- "${rest[@]}" || return $?
            for crate in "${own[@]+"${own[@]}"}"; do
                _run "${b[@]}" build --lockfile_mode=error --config=clippy \
                    "--@rules_rust//rust/settings:clippy.toml=//$crate:clippy.toml" "//$crate/..." || return $?
            done
            _run "${b[@]}" test --lockfile_mode=error //web:lint
            ;;
        web-test) _run "${b[@]}" test --lockfile_mode=error //web:lint //web:typecheck //web:unit //web:bundle_size "$@" ;;
        pi-test) _run "${b[@]}" test --lockfile_mode=error //plugins/pi:typecheck //plugins/pi:test "$@" ;;
        web-e2e) _run "${b[@]}" test --lockfile_mode=error //web:e2e_gesture //web:e2e_chrome_overlay //web:e2e_rest "$@" ;;
        worker-test)
            # Only the format check is Bazel's; the rest is Cargo and npm.
            _run "${b[@]}" test --lockfile_mode=error //crates/clax-worker:rustfmt &&
                echo "── the Worker's tests, clippy and wasm build run natively (its own Cargo workspace) ──" &&
                _run "${awake[@]+"${awake[@]}"}" ./scripts/worker-checks.sh
            ;;
        perf)
            _run "${b[@]}" test --lockfile_mode=error --nocache_test_results --test_output=all \
                --test_env=CLAX_PERF_JSON="$PERF_JSON" --sandbox_writable_path="$(dirname "$PERF_JSON")" \
                //scripts:perf_daemon_full //scripts:perf_clients_full //web:perf_usable_full //web:bundle_size "$@"
            local status=$?
            _perf_app_launch
            return "$status"
            ;;
        build) _run "${b[@]}" build --lockfile_mode=error //crates/clax-cli:clax "$@" ;;
        bin) _bazel_bin //crates/clax-cli:clax ;;
        clax)
            local bin
            bin="$(_bazel_bin //crates/clax-cli:clax)" || return $?
            _run "$bin" "$@"
            ;;
        install)
            local bin="${CLAX_INSTALL_BIN:-}"
            [ -n "$bin" ] || bin="$(_bazel_bin //crates/clax-cli:clax_opt)" || return $?
            _install "$bin"
            ;;
        rust-project) _run "${b[@]}" run --lockfile_mode=error @rules_rust//tools/rust_analyzer:gen_rust_project -- //crates/... ;;
        web)
            _run "${b[@]}" build --lockfile_mode=error //web:dist //web:dist_extension "$@" || return $?
            _copy_dist dist && _copy_dist dist-extension
            ;;
        app-test) _run "${b[@]}" test --lockfile_mode=error //apps/clax_app/... "$@" ;;
        app-integration)
            # The macOS tests, or with --ios the simulator ones (ios_*), or
            # with --android the emulator ones (android_*).
            local t targets=() kind=macos
            case "${1:-}" in
                --ios) kind=ios; shift ;;
                --android) kind=android; shift ;;
            esac
            for t in apps/clax_app/integration_test/*_test.dart; do
                t="${t##*/}"
                case "$t" in
                    ios_*) [ "$kind" = ios ] || continue ;;
                    android_*) [ "$kind" = android ] || continue ;;
                    *) [ "$kind" = macos ] || continue ;;
                esac
                targets+=("//apps/clax_app:integration_${t%_test.dart}")
            done
            _run "${b[@]}" test --lockfile_mode=error "${targets[@]}" "$@"
            ;;
        app-apk) _bazel_out //apps/clax_app:apk ;;
        app-macos) _bazel_out //apps/clax_app:macos_app ;;
        home-bundle) _bazel_out //apps/clax_app:home_bundle ;;
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
        worker-test)
            # The workspace's own members only: `--all` would also format the
            # Clax crates it uses by path.
            _run bash -c 'cd crates/clax-worker && cargo fmt -- --check' &&
                _run "${awake[@]+"${awake[@]}"}" ./scripts/worker-checks.sh
            ;;
        perf)
            _run bash -c 'cd web && pnpm install --frozen-lockfile' && _run ./scripts/build-web.sh &&
                _run cargo build -q --release -p clax-cli &&
                _run env CLAX_PERF_JSON="$PERF_JSON" CLAX_PERF_BIN="$PWD/target/release/clax" ./scripts/perf-daemon.sh &&
                _run env CLAX_PERF_JSON="$PERF_JSON" CLAX_PERF_BIN="$PWD/target/release/clax" ./scripts/perf-clients.sh &&
                _run env CLAX_PERF_JSON="$PERF_JSON" bash -c 'cd web && pnpm exec playwright install chromium >/dev/null && pnpm run perf' &&
                _run env CLAX_PERF_JSON="$PERF_JSON" bash -c 'cd web && node scripts/bundle-size.mjs'
            local status=$?
            _perf_app_launch
            return "$status"
            ;;
        build) _run cargo build --workspace "$@" ;;
        bin)
            # The debug build serves web/dist from the checkout.
            if [ ! -f web/dist/index.html ]; then
                _run bash -c 'cd web && pnpm install --frozen-lockfile --silent && pnpm run build' >&2 || return $?
            fi
            _run cargo build -q -p clax-cli --bin clax >&2 || return $?
            echo "$PWD/target/debug/clax"
            ;;
        clax) _run cargo run -q -p clax-cli -- "$@" ;;
        install)
            if [ -n "${CLAX_INSTALL_BIN:-}" ]; then _install "$CLAX_INSTALL_BIN"; return $?; fi
            _run bash -c 'cd web && pnpm install --frozen-lockfile' && _run ./scripts/build-web.sh &&
                _run cargo build -q --release -p clax-cli &&
                _install "$PWD/target/release/clax"
            ;;
        rust-project) echo "Natively rust-analyzer reads Cargo.toml: nothing to generate." ;;
        web) _run bash -c 'cd web && pnpm install --frozen-lockfile' && _run ./scripts/build-web.sh ;;
        app-test)
            _run ./scripts/build-home-bundle.sh --i-know-what-i-am-doing &&
                _run bash -c 'cd apps/clax_app && tool/check.sh'
            ;;
        app-integration)
            case "${1:-}" in
                --ios | --android)
                    echo "app-integration ${1}: these tests run only with Bazel (brew install bazelisk)" >&2
                    return 2
                    ;;
            esac
            # One file per flutter call: a second file in the same call fails to
            # launch. The iOS and Android files are the simulator's and the
            # emulator's, which only Bazel runs.
            _run ./scripts/build-app-clax.sh macos &&
                _run ./scripts/build-home-bundle.sh --i-know-what-i-am-doing &&
                _run ./scripts/patch-pub.sh --i-know-what-i-am-doing &&
                _run env PUB_CACHE="$PWD/apps/clax_app/build-native/pub-cache" bash -c 'cd apps/clax_app && for f in integration_test/*_test.dart; do case "${f##*/}" in ios_* | android_*) continue ;; esac; flutter test "$f" -d macos || exit 1; done'
            ;;
        app-apk)
            # arm64 and x86_64 only (the ABIs the core is built for); disable-abi-filtering keeps
            # Flutter's Gradle plugin from adding armeabi-v7a back to the app's abiFilters.
            _run ./scripts/build-app-clax.sh android &&
                _run ./scripts/build-home-bundle.sh --i-know-what-i-am-doing &&
                _run echo "app-apk: packaging the APK with Gradle" &&
                _run bash -c 'cd apps/clax_app && flutter build apk --release --target-platform android-arm64,android-x64 -P disable-abi-filtering=true' &&
                { [ "$dry" = 1 ] || echo "$PWD/apps/clax_app/build/app/outputs/flutter-apk/app-release.apk"; }
            ;;
        app-macos)
            _run ./scripts/build-app-clax.sh macos &&
                _run ./scripts/build-home-bundle.sh --i-know-what-i-am-doing &&
                _run ./scripts/patch-pub.sh --i-know-what-i-am-doing &&
                _run env PUB_CACHE="$PWD/apps/clax_app/build-native/pub-cache" bash -c 'cd apps/clax_app && flutter build macos --release' &&
                { [ "$dry" = 1 ] || echo "$PWD/apps/clax_app/build/macos/Build/Products/Release/Clax.app"; }
            ;;
        home-bundle)
            _run ./scripts/build-home-bundle.sh --i-know-what-i-am-doing &&
                { [ "$dry" = 1 ] || echo "$PWD/apps/clax_app/assets/home/home.clax"; }
            ;;
    esac
}

# The perf benchmarks write their measures to PERF_JSON; the run's record
# (scripts/perf-log.sh) is printed and appended to target/perf-log.jsonl,
# never pushed.
PERF_JSON=""
if [ "$task" = perf ]; then
    if [ "$dry" = 1 ]; then PERF_JSON="<a temporary directory>/metrics.jsonl"; else PERF_JSON="$(mktemp -d)/metrics.jsonl"; fi
fi
_perf_record() {
    [ "$task" = perf ] && [ "$dry" = 0 ] || return 0
    echo "── perf log record (also appended to target/perf-log.jsonl; scripts/perf-log.sh show --file target/perf-log.jsonl) ──"
    ./scripts/perf-log.sh record --append target/perf-log.jsonl "$PERF_JSON"
    rm -rf "$(dirname "$PERF_JSON")"
}

if [ "$BUILD_SYSTEM" = bazel ]; then
    _bazel "$@"
    status=$?
    _perf_record
    exit "$status"
fi

# say: a notice for the person; on stderr for `bin`, whose stdout is a path.
say() {
    if [ "$task" = bin ]; then echo "$@" >&2; else echo "$@"; fi
}

if [ "$BUILD_SYSTEM_FALLBACK" = 1 ]; then
    say "── $BUILD_SYSTEM_WHY: running \`$task\` with the native tools (Cargo and pnpm) ──"
    say "   With Bazelisk the same command keeps its results in a cache every worktree shares,"
    say "   needs no target/ or node_modules in the worktree, and runs the checks CI runs."
    say "   Install it: $(build_system_install_hint)"
fi
for warning in "${BUILD_SYSTEM_WARNINGS[@]+"${BUILD_SYSTEM_WARNINGS[@]}"}"; do
    say "   note: $warning"
done

_native "$@"
status=$?
_perf_record

if [ "$status" -eq 0 ]; then result="passed"; else result="failed (exit $status)"; fi
case "$task" in build|web|bin|install|rust-project|app-apk|app-macos|home-bundle) result="${result/passed/done}" ;; esac
line="── \`$task\` $result with the native tools, not Bazel. CI gates on Bazel, and the two can differ: this does not say what CI will."
if [ "$BUILD_SYSTEM_FALLBACK" = 1 ]; then
    line="$line Install Bazelisk for the same checks: $(build_system_install_hint)"
fi
say "$line ──"
exit "$status"
