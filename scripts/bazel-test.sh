#!/usr/bin/env bash
# The repository's checks under Bazel: what scripts/quality_gates.sh checks
# with Cargo and pnpm, without a `target/` or a `node_modules` directory.
#
#   scripts/bazel-test.sh              # the Rust tests (--rust)
#   scripts/bazel-test.sh --rust       # the Rust tests
#   scripts/bazel-test.sh --web        # the web UI's and Pi's checks, browser tests included
#   scripts/bazel-test.sh --ci         # every check, in this order:
#       1. scripts/bazel-sync.sh --check (a stale derived file stops the run)
#       2. the Rust format check and clippy with -D warnings
#       3. the Rust tests
#       4. the web and Pi checks
#       5. the script gates
#       6. the checks that run outside Bazel (docs/bazel.md, "Run outside Bazel")
#       7. the perf gates, one at a time
#       8. no hard link in Bazel's output base or the disk cache
#   scripts/bazel-test.sh [...] ARGS   # ARGS go to every `bazel test`
#
# A failing phase does not stop the later ones, but for the first. Nothing
# here writes MODULE.bazel.lock or bazel/workspace.bzl: when a Cargo.toml or
# Cargo.lock changed, the run stops and names `just bazel-sync`.
#
# BAZEL_TEST_LOGS=<dir> keeps the Rust tests' logs under <dir>/<pass>, for
# scripts/bazel-parity.py.

set -uo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

ci=0
web=0
rust=0
while :; do
    case "${1:-}" in
        --ci) ci=1; shift ;;
        --web) web=1; shift ;;
        --rust) rust=1; shift ;;
        *) break ;;
    esac
done
if [ "$ci" = 1 ]; then rust=1; web=1; fi
if [ "$web" = 0 ] && [ "$ci" = 0 ]; then rust=1; fi

bazel="$(command -v bazelisk || command -v bazel || true)"
if [ -z "$bazel" ]; then
    echo "error: bazelisk not found; install it (brew install bazelisk)," \
        "or run the native checks: just ci-native" >&2
    exit 1
fi

./scripts/bazel-sync.sh --check || exit 1

flags=(--lockfile_mode=error)

# Fake executables keep one store across tests and runs (CLAX_FAKE_EXE_DIR,
# crates/clax-fake-exe), outside each test's temporary directory, so each
# text is assessed by macOS once and not once per test.
fakes="${TMPDIR:-/tmp}"
fakes="${fakes%/}/clax-fake-exe"
mkdir -p "$fakes"
test_flags=(--test_env=CLAX_FAKE_EXE_DIR="$fakes" --sandbox_writable_path="$fakes")

# A machine that sleeps in the middle of a run fails tests that were in
# flight: their timeouts see the gap. On macOS the builds and tests run under
# caffeinate, which holds off idle sleep while the command runs.
awake=()
if command -v caffeinate >/dev/null 2>&1; then
    awake=(caffeinate -i)
fi

failed=()
output="$(mktemp)"
trap 'rm -f "$output"' EXIT

# _summary <name> <seconds>: Bazel's own account of a phase for the CI job
# summary.
_summary() {
    [ -n "${GITHUB_STEP_SUMMARY:-}" ] || return 0
    {
        echo "**bazel: $1** took $2s"
        echo '```'
        grep -E '^(INFO: [0-9]+ process|Executed [0-9]+ out of)' "$output" || true
        echo '```'
    } >> "$GITHUB_STEP_SUMMARY"
}

# _build <name> <bazel build arguments...>
_build() {
    local name="$1" start=$SECONDS
    shift
    echo "── bazel: $name ──"
    "${awake[@]+"${awake[@]}"}" "$bazel" build "${flags[@]}" "$@" 2>&1 | tee "$output"
    [ "${PIPESTATUS[0]}" -eq 0 ] || failed+=("$name")
    echo "── bazel: $name took $((SECONDS - start))s ──"
    _summary "$name" $((SECONDS - start))
}

# _pass <name> <query> [bazel test arguments...]
_pass() {
    local name="$1" query="$2" start=$SECONDS targets testlogs label status
    shift 2
    echo "── bazel: $name ──"
    if ! targets="$("$bazel" query --lockfile_mode=error "$query" 2>"$output")"; then
        cat "$output" >&2
        echo "error: could not list the test targets for '$name'" >&2
        failed+=("$name")
        return
    fi
    if [ -z "$targets" ]; then
        echo "error: no test targets for '$name'" >&2
        failed+=("$name")
        return
    fi
    # shellcheck disable=SC2086  # one label per word
    "${awake[@]+"${awake[@]}"}" "$bazel" test --keep_going "${flags[@]}" "${test_flags[@]}" "$@" $targets 2>&1 | tee "$output"
    status="${PIPESTATUS[0]}"
    [ "$status" -eq 0 ] || failed+=("$name")
    echo "── bazel: $name took $((SECONDS - start))s ──"
    _summary "$name" $((SECONDS - start))

    # Only a pass whose tests all ran (0: passed, 3: some failed) has a
    # current log for every target.
    if [ -n "${BAZEL_TEST_LOGS:-}" ]; then
        rm -rf "${BAZEL_TEST_LOGS:?}/$name"
        if [ "$status" -ne 0 ] && [ "$status" -ne 3 ]; then
            echo "warning: '$name' did not build; its test logs are not kept" >&2
            return
        fi
        testlogs="$("$bazel" info bazel-testlogs 2>/dev/null)"
        for label in $targets; do
            label="${label#//}"
            mkdir -p "$BAZEL_TEST_LOGS/$name/${label/://}"
            cp -R "$testlogs/${label/://}/." "$BAZEL_TEST_LOGS/$name/${label/://}/" 2>/dev/null ||
                echo "warning: no test log for //$label" >&2
        done
    fi
}

# _run <name> <command...>: a check that runs outside Bazel.
_run() {
    local name="$1" start=$SECONDS
    shift
    echo "── $name ──"
    "${awake[@]+"${awake[@]}"}" "$@" || failed+=("$name")
    echo "── $name took $((SECONDS - start))s ──"
}

if [ "$ci" = 1 ]; then
    _build "rust fmt check" --config=fmt //crates/...
    # Cargo reads a crate's clippy.toml from beside its Cargo.toml. Bazel
    # takes one for the whole command, so each crate that has one is linted
    # on its own.
    plain=(//crates/...)
    for toml in crates/*/clippy.toml; do
        [ -f "$toml" ] || continue
        crate="$(dirname "$toml")"
        plain+=("-//$crate/...")
        _build "rust clippy ($crate)" --config=clippy \
            "--@rules_rust//rust/settings:clippy.toml=//$crate:clippy.toml" "//$crate/..."
    done
    _build "rust clippy" --config=clippy -- "${plain[@]}"
fi

if [ "$rust" = 1 ]; then
    _pass "rust tests" 'tests(//crates/...)' "$@"
fi

if [ "$web" = 1 ]; then
    # What `pnpm install --frozen-lockfile` refuses: Bazel builds from the
    # lockfile alone, so a dependency missing from it must fail here.
    _run "package.json and pnpm-lock.yaml agree" ./scripts/pnpm-lock-check.py web plugins/pi
    _pass "web and pi checks" \
        'tests(//web/... + //plugins/pi/... + //bazel/js/...) except attr(tags, "\b(manual|exclusive)\b", //...)' "$@"
fi

if [ "$ci" = 1 ]; then
    _pass "script gates" 'tests(//scripts/...) except attr(tags, "\b(manual|exclusive)\b", //scripts/...)' "$@"
    # Outside Bazel: each reads git tags, runs just, or runs Bazel itself.
    _run "plugins" ./scripts/test-plugins.sh
    _run "justfile" ./scripts/test-justfile.sh
    _run "bazel sync" ./scripts/bazel-sync.test.sh
    _run "bazel web" ./scripts/bazel-web.test.sh
    _pass "perf gates" 'attr(tags, "\bexclusive\b", tests(//...)) except attr(tags, "\bmanual\b", //...)' "$@"
    _run "no hard links in Bazel's directories" \
        ./scripts/check-bazel-no-hard-links.sh "$("$bazel" info output_base 2>/dev/null)" "$HOME/.cache/bazel-disk"
fi

if [ "${#failed[@]}" -gt 0 ]; then
    printf 'error: failed under Bazel: %s\n' "${failed[@]}" >&2
    exit 1
fi
echo "── bazel: all checks passed ──"
