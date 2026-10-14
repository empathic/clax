#!/usr/bin/env bash
# check-wasm.sh [CARGO ARGS...]: checks the crates a hosted Worker runs
# (clax-core, clax-rt, clax-mcp, clax-server) for wasm32-unknown-unknown,
# and fails on any error or warning.
#
# SQLite for that target (rusqlite's sqlite-wasm-rs) is C compiled with a
# clang that has a wasm32 backend; Apple's clang has none. The LLVM is
# $LLVM_BIN when set, else the first of Homebrew's LLVM (in ~/.local/homebrew,
# /opt/homebrew or /usr/local), the clang on PATH or a Debian/Ubuntu
# /usr/lib/llvm-<N> that can target wasm32.
# Arguments, when given, go to cargo in place of the crate list (for example
# `-p clax-server`).
set -euo pipefail
cd "$(dirname "$0")/.."

can_target_wasm() {
  [ -x "$1/clang" ] && [ -x "$1/llvm-ar" ] &&
    "$1/clang" --print-targets 2>/dev/null | grep '^ *wasm32 ' >/dev/null
}

llvm=""
if [ -n "${LLVM_BIN:-}" ]; then
  can_target_wasm "$LLVM_BIN" || {
    echo "check-wasm: LLVM_BIN=$LLVM_BIN has no clang and llvm-ar that target wasm32" >&2
    exit 1
  }
  llvm="$LLVM_BIN"
else
  candidates=("$HOME/.local/homebrew/opt/llvm/bin" /opt/homebrew/opt/llvm/bin /usr/local/opt/llvm/bin)
  if command -v clang >/dev/null; then
    candidates+=("$(dirname "$(command -v clang)")")
  fi
  # Debian and Ubuntu install each LLVM under /usr/lib/llvm-<N>/bin, with
  # only versioned names (llvm-ar-<N>) on PATH: newest first.
  while IFS= read -r dir; do
    candidates+=("$dir")
  done < <(printf '%s\n' /usr/lib/llvm-*/bin | sort -V -r)
  for dir in "${candidates[@]}"; do
    if can_target_wasm "$dir"; then
      llvm="$dir"
      break
    fi
  done
fi
if [ -z "$llvm" ]; then
  echo "check-wasm: no clang with a wasm32 target found; install LLVM (brew install llvm) or set LLVM_BIN" >&2
  exit 1
fi
if ! rustup target list --installed 2>/dev/null | grep -x wasm32-unknown-unknown >/dev/null; then
  echo "check-wasm: the wasm32-unknown-unknown target is missing: rustup target add wasm32-unknown-unknown" >&2
  exit 1
fi

export CC_wasm32_unknown_unknown="$llvm/clang"
export AR_wasm32_unknown_unknown="$llvm/llvm-ar"
if [ "$#" -eq 0 ]; then
  set -- -p clax-core -p clax-rt -p clax-mcp -p clax-server
fi
# A warning fails the check too, as `-D warnings` does in the native gates;
# cargo reports cached warnings again, so a rerun still sees them.
log="$(mktemp)"
trap 'rm -f "$log"' EXIT
status=0
cargo check --target wasm32-unknown-unknown "$@" 2>&1 | tee "$log" || status=$?
if [ "$status" -ne 0 ]; then
  exit "$status"
fi
if grep -q '^warning' "$log"; then
  echo "check-wasm: warnings for wasm32-unknown-unknown" >&2
  exit 1
fi
