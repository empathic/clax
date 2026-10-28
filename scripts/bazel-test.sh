#!/usr/bin/env bash
# The repository's checks under Bazel: what scripts/quality_gates.sh checks
# with Cargo and pnpm, without a `target/` or a `node_modules` directory.
#
#   scripts/bazel-test.sh              # the Rust tests (--rust)
#   scripts/bazel-test.sh --rust       # the Rust tests
#   scripts/bazel-test.sh --web        # the web UI's and Pi's checks, browser tests included
#   scripts/bazel-test.sh --ci         # every check, in this order:
#       1. scripts/bazel-sync.sh --check (a stale derived file stops the run)
#       2. package.json and pnpm-lock.yaml agree
#       3. the quick checks: the Rust format check, clippy with -D warnings
#          (each crate with its own clippy.toml too), the web lint, the
#          script gates that need no `clax`, the justfile, hard-link and
#          pipefail checks; a failure here stops the run
#       4. one `bazel test`: the Rust tests, the web and Pi checks, every
#          script gate (performance is measured on demand, `just perf`, and
#          gates nothing)
#       5. the other checks that run outside Bazel (docs/bazel.md, "Run
#          outside Bazel")
#   scripts/bazel-test.sh [...] ARGS   # ARGS go to every `bazel test`
#
# A failing phase does not stop the later ones, but for the first and the
# quick checks. Nothing here writes MODULE.bazel.lock or
# bazel/workspace.bzl: when a Cargo.toml or Cargo.lock changed, the run
# stops and names `just bazel-sync`.
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

# Bazel as scripts/run.sh picks it: Bazelisk, or a bazel at .bazelversion.
# shellcheck source=scripts/lib/build-system.sh
. scripts/lib/build-system.sh
if ! _bazel_usable; then
    echo "error: $_bazel_why; install Bazelisk ($(build_system_install_hint))," \
        "or run the native checks: just ci-native" >&2
    exit 1
fi
bazel="$BAZEL"

./scripts/bazel-sync.sh --check || exit 1

# shellcheck source=scripts/lib/bazel-test-flags.sh
source scripts/lib/bazel-test-flags.sh
flags=("${bazel_flags[@]}")
test_flags=("${bazel_test_flags[@]}")

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
    # The query names each target, so a target only for another platform
    # (an iOS one, such as //crates/clax-embed's) is skipped as a pattern would
    # skip it, not refused.
    # shellcheck disable=SC2086  # one label per word
    "${awake[@]+"${awake[@]}"}" "$bazel" test --keep_going --skip_incompatible_explicit_targets "${flags[@]}" "${test_flags[@]}" "$@" $targets 2>&1 | tee "$output"
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

# _key <pathspec...>: a digest of the named files as they are in the working
# tree (tracked and new, not ignored; a deleted one counts by its name), and
# of the v* tags.
_key() {
    {
        git tag -l 'v*'
        git ls-files -co --exclude-standard -z -- "$@" | python3 -c '
import hashlib, os, sys
for name in sorted(set(sys.stdin.buffer.read().split(b"\0")) - {b""}):
    if os.path.isfile(name):
        with open(name, "rb") as f:
            print(hashlib.sha256(f.read()).hexdigest(), name.decode(errors="replace"))
    else:
        print("gone", name.decode(errors="replace"))
'
    } | shasum -a 256 | cut -d' ' -f1
}

# _run_keyed <name> <pathspec...> -- <command...>: a check outside Bazel
# that reads only the files its pathspecs name (and the v* tags), skipped
# when it passed before on the same files, in any worktree, as Bazel caches
# a passed test. A pass is recorded as an empty file named by its key under
# CLAX_CHECKS_CACHE (default ~/.cache/clax-checks), which CI does not keep,
# so CI runs every one.
checks_cache="${CLAX_CHECKS_CACHE:-$HOME/.cache/clax-checks}"
_run_keyed() {
    local name="$1" specs=() key stamp
    shift
    while [ "$1" != "--" ]; do specs+=("$1"); shift; done
    shift
    key="$(_key "${specs[@]}")"
    stamp="$checks_cache/$(printf '%s' "$name" | tr -c 'A-Za-z0-9\n' '-')/$key"
    if [ -f "$stamp" ]; then
        echo "── $name: passed on these files before ($stamp) ──"
        return
    fi
    local before="${#failed[@]}"
    _run "$name" "$@"
    if [ "${#failed[@]}" = "$before" ]; then
        mkdir -p "$(dirname "$stamp")" && : > "$stamp"
    fi
}

if [ "$ci" = 1 ]; then
    # What `pnpm install --frozen-lockfile` refuses: Bazel builds from the
    # lockfile alone, so a dependency missing from it must fail here.
    _run "package.json and pnpm-lock.yaml agree" ./scripts/pnpm-lock-check.py web plugins/pi crates/clax-worker
    # The quick checks first, so a broken format, lint or script fails in
    # minutes rather than after every test: the format check, clippy (each
    # crate with its own clippy.toml too), the web lint, the script gates
    # that need no `clax`, and the outside checks that take seconds. A
    # failure among them stops the run before the long pass.
    _build "rust format" --keep_going --config=fmt --output_groups=rustfmt_checks //crates/...
    _build "rust clippy" --keep_going --config=clippy --output_groups=clippy_checks //crates/...
    # Cargo reads a crate's clippy.toml from beside its Cargo.toml. Bazel
    # takes one for the whole command, so each crate that has one is also
    # linted on its own, with it.
    for toml in crates/*/clippy.toml; do
        [ -f "$toml" ] || continue
        crate="$(dirname "$toml")"
        _build "rust clippy ($crate)" --keep_going --config=clippy --output_groups=clippy_checks \
            "--@rules_rust//rust/settings:clippy.toml=//$crate:clippy.toml" "//$crate/..."
    done
    # Not the hosted Worker's builds (//bazel/wasm/...): a wasm32 build of
    # every crate it runs is no quick check; the pass after this one runs it.
    quick='//scripts/... + //bazel/... - //bazel/wasm/...'
    _pass "quick checks" \
        "//web:lint + tests($quick) except rdeps($quick, //crates/clax-cli:clax) except attr(tags, \"\\b(manual|exclusive)\\b\", //...)" "$@"
    _run "justfile" ./scripts/test-justfile.sh
    _run "no hard links (tracked files)" ./scripts/check-no-hard-links.sh
    _run "pipefail pipes (tracked files)" ./scripts/check-pipefail-pipes.sh
    if [ "${#failed[@]}" -gt 0 ]; then
        printf 'error: failed under Bazel: %s\n' "${failed[@]}" >&2
        echo "error: the quick checks failed; the tests did not run" >&2
        exit 1
    fi
    # Everything else in one invocation, so builds and tests share the
    # machine instead of queueing: every test (Rust, web, Pi, script
    # gates). The format check and clippy are asked for again, and come
    # from the quick checks' results. The perf benchmarks are `manual`: on
    # demand only (`just perf`). //web:playwright is no test, but CI runs
    # it before this script (`install-deps`), so it is built here too.
    _pass "every check" \
        '//crates/... + //web:playwright + tests(//...) except attr(tags, "\bmanual\b", //...)' \
        --config=fmt --config=clippy "$@"
    # Outside Bazel (docs/bazel.md, "Run outside Bazel"): each reads git
    # (tags, tracked files), runs just or cargo, writes into the checkout,
    # or runs Bazel itself.
    # Each outside check that takes seconds is keyed on the files it reads
    # (_run_keyed): a whole-tree key where they are many, as for the plugin
    # checks, which read the plugins, skills, docs, versions and tags.
    _run_keyed "plugins" . -- ./scripts/test-plugins.sh
    _run_keyed "dev scripts" . -- ./scripts/test-dev.sh
    _run "extension signing" ./scripts/test-extension-signing.sh
    # These two test Bazel's setup, not the code: they rerun when a Cargo,
    # Bazel or package manifest, or Bazel's own files, change.
    bazel_setup=(.bazelrc .bazelversion MODULE.bazel MODULE.bazel.lock rust-toolchain.toml
        '*Cargo.toml' Cargo.lock '*BUILD.bazel' bazel/ 'scripts/bazel-*' scripts/test-release.files
        '*package.json' '*pnpm-lock.yaml' '*pnpm-workspace.yaml')
    _run_keyed "bazel sync" "${bazel_setup[@]}" -- ./scripts/bazel-sync.test.sh
    # The web test copies the tree and runs Bazel on it, minutes: it reruns
    # only when what it tests changes, rules_js's setup for web/.
    _run_keyed "bazel web" .bazelrc .bazelversion MODULE.bazel MODULE.bazel.lock bazel/js/ \
        web/BUILD.bazel web/package.json web/pnpm-lock.yaml web/pnpm-workspace.yaml \
        scripts/bazel-web.test.sh -- ./scripts/bazel-web.test.sh
else
    if [ "$rust" = 1 ]; then
        _pass "rust tests" 'tests(//crates/...)' "$@"
    fi
    if [ "$web" = 1 ]; then
        _run "package.json and pnpm-lock.yaml agree" ./scripts/pnpm-lock-check.py web plugins/pi
        _pass "web and pi checks" \
            'tests(//web/... + //plugins/pi/... + //bazel/js/...) except attr(tags, "\b(manual|exclusive)\b", //...)' "$@"
    fi
fi

if [ "${#failed[@]}" -gt 0 ]; then
    printf 'error: failed under Bazel: %s\n' "${failed[@]}" >&2
    exit 1
fi
echo "── bazel: all checks passed ──"
