#!/usr/bin/env bash
# Keeps the Bazel build in step with Cargo, which stays the source of truth.
#
#   scripts/bazel-sync.sh           regenerate what Bazel derives from Cargo
#   scripts/bazel-sync.sh --check   fail if any of it is out of date; writes nothing
#
# Derived:
#   bazel/workspace.bzl   each workspace crate's version, targets, default
#                         features, path dependencies and build script,
#                         and the workspace lints as rustc flags
#                         (scripts/bazel-derive.py workspace); a
#                         dev-dependency that turns features on fails it
#   bazel/wasm/crates.MODULE.bazel
#                         the crate specs @crates_wasm, the hosted Worker's
#                         wasm32 crates, resolves (scripts/bazel-derive.py
#                         wasm-specs)
#   bazel/release_files.bzl
#                         the release script test's files as labels
#                         (scripts/test-release.files)
#   bazel/playwright/browsers.bzl
#                         the Chromium builds the locked Playwright runs
#   bazel/wasm/Cargo.lock Cargo.lock, pruned to what @crates_wasm resolves:
#                         seeded from Cargo.lock, so it takes its versions
#   MODULE.bazel.lock     the pinned third-party crates (Cargo.lock) and
#                         packages (each pnpm-lock.yaml), resolved again
#                         when resolving rewrote either Cargo.lock
# Checked: the Rust version in MODULE.bazel against rust-toolchain.toml, a
# BUILD.bazel in every workspace crate, no job or core limit in .bazelrc,
# and each third-party crate's features against Cargo's.
#
# Sources and third-party dependencies need no step: BUILD files glob the
# former and take the latter from Cargo.toml through crate_universe.
#
# BAZEL_SYNC_BAZEL names the bazel command (by default Bazelisk, or a bazel
# at the version .bazelversion names, as scripts/run.sh picks); the test
# (scripts/bazel-sync.test.sh) sets it to a fake.

set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

mode="write"
case "${1:-}" in
    "") ;;
    --check) mode=check ;;
    *) echo "usage: $0 [--check]" >&2; exit 2 ;;
esac

# Bazel as scripts/run.sh picks it: Bazelisk, or a bazel at .bazelversion.
bazel="${BAZEL_SYNC_BAZEL:-}"
if [ -z "$bazel" ]; then
    # shellcheck source=scripts/lib/build-system.sh
    . scripts/lib/build-system.sh
    if ! _bazel_usable; then
        echo "error: $_bazel_why; install Bazelisk: $(build_system_install_hint)" >&2
        exit 1
    fi
    bazel="$BAZEL"
fi

stale=0
_stale() {
    echo "error: $1 is out of date; run just bazel-sync and commit the result" >&2
    stale=1
}

# What reads the workspace's resolve (the wasm32 dependencies, the
# specs of @crates_wasm): from a Cargo.lock that matches the manifests, so
# a sync writes them once resolving has rewritten it (below).
_derive_from_cargo() {
    local generated
    generated="$(./scripts/bazel-derive.py workspace)" || exit 1
    if [ "$mode" = write ]; then
        printf '%s\n' "$generated" > bazel/workspace.bzl
    elif ! printf '%s\n' "$generated" | diff -q - bazel/workspace.bzl >/dev/null 2>&1; then
        _stale bazel/workspace.bzl
    fi
    generated="$(./scripts/bazel-derive.py wasm-specs)" || exit 1
    if [ "$mode" = write ]; then
        printf '%s\n' "$generated" > bazel/wasm/crates.MODULE.bazel
    elif ! printf '%s\n' "$generated" | diff -q - bazel/wasm/crates.MODULE.bazel >/dev/null 2>&1; then
        _stale bazel/wasm/crates.MODULE.bazel
    fi
}
[ "$mode" = check ] && _derive_from_cargo

generated="$(./scripts/bazel-derive.py release-files)"
if [ "$mode" = write ]; then
    printf '%s\n' "$generated" > bazel/release_files.bzl
elif ! printf '%s\n' "$generated" | diff -q - bazel/release_files.bzl >/dev/null 2>&1; then
    _stale bazel/release_files.bzl
fi

# The pinned Chromium follows the locked Playwright. Deriving it downloads
# each archive once for its checksum, so it is rewritten only when the
# version moved; --check compares the versions.
locked="$(./scripts/bazel-derive.py playwright-version)"
pinned="$(sed -n 's/^PLAYWRIGHT_VERSION = "\(.*\)"$/\1/p' bazel/playwright/browsers.bzl 2>/dev/null || true)"
if [ "$locked" != "$pinned" ]; then
    if [ "$mode" = write ]; then
        ./scripts/bazel-derive.py browsers > bazel/playwright/browsers.bzl.new &&
            mv bazel/playwright/browsers.bzl.new bazel/playwright/browsers.bzl
    else
        _stale "bazel/playwright/browsers.bzl (Playwright $pinned, web/pnpm-lock.yaml locks $locked)"
    fi
fi

# Every workspace crate has a BUILD.bazel (bazel/cargo.bzl's cargo_package).
for dir in $(python3 -c 'import tomllib; print("\n".join(tomllib.load(open("Cargo.toml","rb"))["workspace"]["members"]))'); do
    if [ ! -f "$dir/BUILD.bazel" ]; then
        echo "error: $dir has no BUILD.bazel; give it one that calls cargo_package() (docs/bazel.md, \"Targets\")" >&2
        stale=1
    fi
done

# No limit on jobs or cores is committed: a build takes the whole machine,
# here and in CI. A limit for one machine belongs in user.bazelrc.
if grep -Eq '^(build|test|common)(:[a-z]+)? .*--(jobs|local_resources|local_cpu_resources)[= ]' .bazelrc; then
    echo "error: .bazelrc limits jobs or cores; put a machine's own limit in user.bazelrc instead" >&2
    stale=1
fi

# One Rust version for both build systems.
cargo_rust="$(sed -n 's/^channel = "\(.*\)"/\1/p' rust-toolchain.toml)"
if ! grep -q "versions = \[\"$cargo_rust\"\]" MODULE.bazel; then
    echo "error: MODULE.bazel does not build with Rust $cargo_rust (rust-toolchain.toml);" \
        "set rust.toolchain's versions, rustfmt_version and sha256s to it" >&2
    stale=1
fi

# The lockfile records the hashes of Cargo.toml, Cargo.lock and each
# pnpm-lock.yaml, so resolving the modules is enough to notice a change.
log="$(mktemp)"
trap 'rm -f "$log"' EXIT
# Resolving the modules: MODULE.bazel.lock, and each Cargo.lock as
# crate_universe rewrites it.
_resolve() {
    # @crates_wasm resolves from Cargo.lock's versions: a copy of it, which
    # crate_universe prunes to what the Worker's specs reach.
    cp Cargo.lock bazel/wasm/Cargo.lock
    # crate_universe rewrites a Cargo.lock that its manifests no longer
    # match (a workspace crate's version left behind by a bump, the pruned
    # copy) after MODULE.bazel.lock has recorded the old file's hash;
    # resolving again records the new one. Both must hold still within
    # three rounds.
    for round in 1 2 3; do
        before="$(cat Cargo.lock bazel/wasm/Cargo.lock | cksum)"
        if ! "$bazel" mod deps --lockfile_mode=update >/dev/null 2>"$log"; then
            cat "$log" >&2
            echo "error: bazel could not resolve the modules; MODULE.bazel.lock was not rewritten" >&2
            exit 1
        fi
        [ "$(cat Cargo.lock bazel/wasm/Cargo.lock | cksum)" = "$before" ] && break
        if [ "$round" = 3 ]; then
            echo "error: a Cargo.lock changed on every resolution; MODULE.bazel.lock may not match it" >&2
            exit 1
        fi
        echo "bazel: resolving the modules rewrote a Cargo.lock; resolving again" >&2
    done
}
if [ "$mode" = write ]; then
    _resolve
    # The specs @crates_wasm resolves from follow the resolved Cargo.lock;
    # when they change, it resolves once more with them.
    specs="$(cksum < bazel/wasm/crates.MODULE.bazel)"
    _derive_from_cargo
    if [ "$(cksum < bazel/wasm/crates.MODULE.bazel)" != "$specs" ]; then
        _resolve
    fi
elif ! "$bazel" mod deps --lockfile_mode=error >/dev/null 2>"$log"; then
    # Bazel names the lockfile when that is what is wrong; anything else
    # (a syntax error, no network) is its own failure and is shown as such.
    if grep -q -i 'MODULE.bazel.lock\|lockfile' "$log"; then
        _stale MODULE.bazel.lock
    else
        cat "$log" >&2
        echo "error: bazel could not resolve the modules" >&2
        exit 1
    fi
fi

# Bazel's third-party crates must be Cargo's: the same versions and
# features, but for a short accepted list (scripts/bazel-derive.py).
if [ "$stale" = 0 ] && ! ./scripts/bazel-derive.py features; then
    stale=1
fi

if [ "$stale" = 1 ]; then exit 1; fi
if [ "$mode" = check ]; then
    echo "bazel: derived files are up to date"
else
    echo "bazel: wrote bazel/workspace.bzl, bazel/release_files.bzl, bazel/wasm/crates.MODULE.bazel, bazel/wasm/Cargo.lock and MODULE.bazel.lock"
fi
