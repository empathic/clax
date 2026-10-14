#!/usr/bin/env bash
# Builds the Worker (build/index.js and its wasm) with worker-build, which
# wrangler runs as the custom build. worker-build is installed once into the
# repository's target/tools. SQLite's C sources need a clang with a wasm32
# target; Apple clang has none, so on macOS CC defaults to Homebrew LLVM,
# elsewhere to `clang` and the newest `llvm-ar` on PATH.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
tools="$here/../../target/tools"
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
cd "$here"
exec "$tools/bin/worker-build" --release "$@"
