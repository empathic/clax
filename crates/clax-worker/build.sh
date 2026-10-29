#!/usr/bin/env bash
# Builds the Worker wrangler deploys, the one wrangler.jsonc's `main` names:
# target/clax-worker/build/ at the repository root (index.js, index_bg.wasm,
# package.json, worker/shim.mjs). wrangler runs it as the custom build.
#
# With Bazel (scripts/lib/build-system.sh decides, CLAX_BUILD overrides) it
# builds //crates/clax-worker:bundle (bazel/wasm/worker.bzl) and links
# target/clax-worker/build to Bazel's output. Natively it runs worker-build
# with the workspace's `worker-release` profile into that directory;
# worker-build is installed once into the repository's target/tools, and
# SQLite's C sources need a clang with a wasm32 target: Apple clang has
# none, so on macOS CC defaults to Homebrew LLVM, elsewhere to `clang` and
# the newest `llvm-ar` on PATH.
#
# Given CLAX_BUILD_COMMIT (deploy.mjs gives it the checked-out commit), it
# records the build beside it, in target/clax-worker/built-from.json: that
# commit and index_bg.wasm's sha256, which `deploy.mjs --no-build` checks
# (deploy-lib.mjs builtFromRefusal). A build without it leaves no record.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
out="$root/target/clax-worker/build"
cd "$root"
# shellcheck source=scripts/lib/build-system.sh
. scripts/lib/build-system.sh
build_system_choose rust >/dev/null || exit $?
record="$(dirname "$out")/built-from.json"
mkdir -p "$(dirname "$out")"
rm -f "$record"

# The record of a build in $out from $CLAX_BUILD_COMMIT.
record_build() {
    [ -n "${CLAX_BUILD_COMMIT:-}" ] || return 0
    local sum
    if command -v sha256sum >/dev/null; then
        sum="$(sha256sum "$out/index_bg.wasm")"
    else
        sum="$(shasum -a 256 "$out/index_bg.wasm")"
    fi
    printf '{"commit":"%s","wasm_sha256":"%s"}\n' "$CLAX_BUILD_COMMIT" "${sum%% *}" > "$record"
}

if [ "$BUILD_SYSTEM" = bazel ]; then
    "$BAZEL" build --lockfile_mode=error "$@" //crates/clax-worker:bundle
    built="$("$BAZEL" cquery --lockfile_mode=error --output=files "$@" //crates/clax-worker:bundle 2>/dev/null)"
    [ -d "$built" ] || { echo "build.sh: Bazel built no bundle directory ($built)" >&2; exit 1; }
    # A link, not a copy: Bazel's output is the bundle.
    rm -rf "$out"
    ln -s "$root/$built" "$out"
    record_build
    exit 0
fi

tools="$root/target/tools"
if [ ! -x "$tools/bin/worker-build" ]; then
    cargo install worker-build@0.8.7 --locked --root "$tools"
fi
if [ "$(uname)" = Darwin ] || [ -n "${LLVM_PREFIX:-}" ]; then
    llvm="${LLVM_PREFIX:-$(brew --prefix llvm 2>/dev/null || echo /opt/homebrew/opt/llvm)}"
    cc="$llvm/bin/clang" ar="$llvm/bin/llvm-ar"
else
    cc=clang
    ar="$(compgen -c llvm-ar | sort -V | tail -n 1)"
fi
export CC_wasm32_unknown_unknown="${CC_wasm32_unknown_unknown:-$cc}"
export AR_wasm32_unknown_unknown="${AR_wasm32_unknown_unknown:-$ar}"
# A link left by a Bazel build is not worker-build's directory to swap.
[ -L "$out" ] && rm "$out"
cd "$here"
"$tools/bin/worker-build" --profile worker-release --out-dir "$out" "$@"
record_build
