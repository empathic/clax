#!/usr/bin/env bash
# Tests scripts/bazel-sync.sh --check in a scratch copy of the repository's
# Cargo and Bazel files: current files pass; a new default crate feature, a
# changed workspace lint, a stale MODULE.bazel.lock (Bazel's error, faked), a
# Rust version that differs from rust-toolchain.toml, a workspace crate
# without a BUILD.bazel, and a job limit in .bazelrc each fail, naming the
# cause; --check never writes a file.
set -uo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"
T="$(cd "$(mktemp -d)" && pwd -P)"
trap 'rm -rf "$T"' EXIT
FAILED=0
pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1"; FAILED=1; }
# A copy of every tracked and new (not ignored) file, as in the working tree.
fresh() {
    rm -rf "$T/r"; mkdir -p "$T/r"
    (cd "$HERE" && git ls-files -z --cached --others --exclude-standard | xargs -0 tar -cf - 2>/dev/null) | tar -xf - -C "$T/r"
}
# perl rather than sed -i: one spelling on macOS and Linux.
edit() { perl -0pi -e "$1" "$2"; }
# Bazel's own lockfile check is a fake that passes, unless a case makes it
# fail; every real run (scripts/bazel-test.sh) runs the real one.
. "$HERE/scripts/fake-exe.sh"
mkdir -p "$T/bin"
printf '#!/bin/sh\nexit 0\n' | fake_exe "$T/bin/bazel-ok"
printf '#!/bin/sh\necho "ERROR: MODULE.bazel.lock is not up-to-date" >&2; exit 1\n' | fake_exe "$T/bin/bazel-stale"
check() { (cd "$T/r" && BAZEL_SYNC_BAZEL="${BAZEL:-$T/bin/bazel-ok}" scripts/bazel-sync.sh --check) >"$T/out" 2>&1; }

fresh; if check; then pass "current files pass"; else fail "current files pass: $(cat "$T/out")"; fi

fresh; edit 's/^\[lints\]\n/[features]\ndefault = ["extra"]\nextra = []\n\n[lints]\n/m' "$T/r/crates/clax-hooks/Cargo.toml"
touch "$T/stamp"
if check; then fail "a new default feature fails"; elif grep -q 'just bazel-sync' "$T/out"; then pass "a new default feature fails, naming just bazel-sync"; else fail "message: $(cat "$T/out")"; fi
wrote="$(cd "$T/r" && find . -type f -newer "$T/stamp" ! -path './target/*' | sort)"
[ -z "$wrote" ] && pass "--check writes nothing" || fail "--check wrote: $wrote"

fresh; edit 's/^\[lints\]\n/[features]\nextra = []\n\n[lints]\n/m' "$T/r/crates/clax-hooks/Cargo.toml"
edit 's/^clax-hooks = \{ path = "..\/clax-hooks" \}$/clax-hooks = { path = "..\/clax-hooks", features = ["extra"] }/m' "$T/r/crates/clax-server/Cargo.toml"
if check; then fail "a test feature on a workspace crate fails"; elif grep -q 'clax-testkit' "$T/out"; then pass "a test feature on a workspace crate fails, naming clax-testkit"; else fail "message: $(cat "$T/out")"; fi

fresh; if BAZEL="$T/bin/bazel-stale" check; then fail "a stale lockfile fails"; elif grep -q 'just bazel-sync' "$T/out"; then pass "a stale lockfile fails, naming just bazel-sync"; else fail "message: $(cat "$T/out")"; fi

fresh; edit 's/unsafe_code = "forbid"/unsafe_code = "deny"/' "$T/r/Cargo.toml"
if check; then fail "a changed lint fails"; else pass "a changed lint fails"; fi

fresh; edit 's/channel = "1.94.0"/channel = "1.95.0"/' "$T/r/rust-toolchain.toml"
if check; then fail "a toolchain mismatch fails"; elif grep -q 'rust-toolchain.toml' "$T/out"; then pass "a toolchain mismatch fails"; else fail "message: $(cat "$T/out")"; fi

fresh; rm "$T/r/crates/clax-hooks/BUILD.bazel"
if check; then fail "a crate without BUILD.bazel fails"; elif grep -q 'crates/clax-hooks' "$T/out"; then pass "a crate without BUILD.bazel fails"; else fail "message: $(cat "$T/out")"; fi

fresh; printf 'build --jobs=4\n' >> "$T/r/.bazelrc"
if check; then fail "a job limit fails"; else pass "a job limit in .bazelrc fails"; fi

exit "$FAILED"
