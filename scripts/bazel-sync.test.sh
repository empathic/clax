#!/usr/bin/env bash
# Tests scripts/bazel-sync.sh --check in a scratch copy of the repository's
# Cargo and Bazel files: current files pass; a new default crate feature, a
# changed workspace lint, a stale MODULE.bazel.lock (Bazel's error, faked), a
# Rust version that differs from rust-toolchain.toml, a workspace crate
# without a BUILD.bazel, and a job limit in .bazelrc each fail, naming the
# cause; --check never writes a file. A sync whose resolution rewrites
# Cargo.lock (crate_universe, faked) resolves again, so one run is enough.
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
# edit PATTERN REPLACEMENT FILE: replaces the first match of the Python
# regular expression PATTERN (^ and $ match at each line) in FILE, in place;
# one spelling on macOS and Linux, unlike sed -i.
edit() {
    python3 -c 'import re, sys
pattern, repl, path = sys.argv[1:]
with open(path, newline="") as f: text = f.read()
with open(path, "w", newline="") as f: f.write(re.sub(pattern, repl, text, count=1, flags=re.M))' "$@"
}
# Bazel's own lockfile check is a fake that passes, unless a case makes it
# fail; every real run (scripts/bazel-test.sh) runs the real one.
. "$HERE/scripts/fake-exe.sh"
mkdir -p "$T/bin"
printf '#!/bin/sh\nexit 0\n' | fake_exe "$T/bin/bazel-ok"
printf '#!/bin/sh\necho "ERROR: MODULE.bazel.lock is not up-to-date" >&2; exit 1\n' | fake_exe "$T/bin/bazel-stale"
check() { (cd "$T/r" && BAZEL_SYNC_BAZEL="${BAZEL:-$T/bin/bazel-ok}" scripts/bazel-sync.sh --check) >"$T/out" 2>&1; }

fresh; if check; then pass "current files pass"; else fail "current files pass: $(cat "$T/out")"; fi

fresh; edit '^\[lints\]\n' '[features]\ndefault = ["extra"]\nextra = []\n\n[lints]\n' "$T/r/crates/clax-hooks/Cargo.toml"
touch "$T/stamp"
if check; then fail "a new default feature fails"; elif grep -q 'just bazel-sync' "$T/out"; then pass "a new default feature fails, naming just bazel-sync"; else fail "message: $(cat "$T/out")"; fi
wrote="$(cd "$T/r" && find . -type f -newer "$T/stamp" ! -path './target/*' | sort)"
[ -z "$wrote" ] && pass "--check writes nothing" || fail "--check wrote: $wrote"

fresh; edit '^\[lints\]\n' '[features]\nextra = []\n\n[lints]\n' "$T/r/crates/clax-hooks/Cargo.toml"
edit '^clax-hooks = \{ path = "\.\./clax-hooks" \}$' 'clax-hooks = { path = "../clax-hooks", features = ["extra"] }' "$T/r/crates/clax-server/Cargo.toml"
if check; then fail "a test feature on a workspace crate fails"; elif grep -q 'clax-testkit' "$T/out"; then pass "a test feature on a workspace crate fails, naming clax-testkit"; else fail "message: $(cat "$T/out")"; fi

fresh; if BAZEL="$T/bin/bazel-stale" check; then fail "a stale lockfile fails"; elif grep -q 'just bazel-sync' "$T/out"; then pass "a stale lockfile fails, naming just bazel-sync"; else fail "message: $(cat "$T/out")"; fi

fresh; edit 'unsafe_code = "forbid"' 'unsafe_code = "deny"' "$T/r/Cargo.toml"
if check; then fail "a changed lint fails"; else pass "a changed lint fails"; fi

fresh; edit 'channel = "1\.94\.0"' 'channel = "1.95.0"' "$T/r/rust-toolchain.toml"
if check; then fail "a toolchain mismatch fails"; elif grep -q 'rust-toolchain.toml' "$T/out"; then pass "a toolchain mismatch fails"; else fail "message: $(cat "$T/out")"; fi

fresh; rm "$T/r/crates/clax-hooks/BUILD.bazel"
if check; then fail "a crate without BUILD.bazel fails"; elif grep -q 'crates/clax-hooks' "$T/out"; then pass "a crate without BUILD.bazel fails"; else fail "message: $(cat "$T/out")"; fi

fresh; printf 'build --jobs=4\n' >> "$T/r/.bazelrc"
if check; then fail "a job limit fails"; else pass "a job limit in .bazelrc fails"; fi

# A Cargo.lock that names a workspace crate at another version than its
# manifest: the fake rewrites it on resolving, as crate_universe does, and
# logs each resolution.
V="$("$HERE/scripts/check-version.sh" --print)"
fake_exe "$T/bin/bazel-repin" <<SH
#!/bin/sh
echo resolve >> "\$BAZEL_SYNC_TEST_LOG"
python3 - <<'PY'
with open("Cargo.lock", newline="") as f: text = f.read()
old = 'name = "clax-fake-exe"\nversion = "0.0.1"'
with open("Cargo.lock", "w", newline="") as f: f.write(text.replace(old, 'name = "clax-fake-exe"\nversion = "$V"', 1))
PY
SH
fresh; edit '(name = "clax-fake-exe"\nversion = ")[^"]+"' '\g<1>0.0.1"' "$T/r/Cargo.lock"
: > "$T/resolves"
if (cd "$T/r" && BAZEL_SYNC_TEST_LOG="$T/resolves" BAZEL_SYNC_BAZEL="$T/bin/bazel-repin" scripts/bazel-sync.sh) >"$T/out" 2>&1 \
    && [ "$(wc -l < "$T/resolves" | tr -d ' ')" = 2 ] && grep -q "resolving again" "$T/out"; then
    pass "a sync whose resolution rewrites Cargo.lock resolves again"
else fail "a sync whose resolution rewrites Cargo.lock resolves again ($(wc -l < "$T/resolves") resolutions: $(cat "$T/out"))"; fi

# With no BAZEL_SYNC_BAZEL, Bazel is picked as scripts/run.sh picks it: a
# plain bazel at another version than .bazelversion, and no Bazelisk, is
# refused, naming .bazelversion, and never run, before anything is derived.
# PATH holds that bazel and links to the system tools the choice needs.
mkdir -p "$T/wrong" "$T/sys"
printf '#!/bin/sh\nif [ "$1" = --version ]; then echo "bazel 0.0.1"; exit 0; fi\necho "$*" >> "%s"\nexit 0\n' "$T/ran" \
    | fake_exe "$T/wrong/bazel"
for tool in bash env sed awk tr dirname uname; do
    ln -s "$(command -v "$tool")" "$T/sys/$tool"
done
fresh
if (cd "$T/r" && env -u BAZEL_SYNC_BAZEL PATH="$T/wrong:$T/sys" scripts/bazel-sync.sh --check) >"$T/out" 2>&1; then
    fail "a bazel at another version is refused: it passed"
elif grep -q '.bazelversion' "$T/out" && [ ! -e "$T/ran" ]; then
    pass "a bazel at another version than .bazelversion is refused, unrun"
else fail "a bazel at another version is refused (ran: $(cat "$T/ran" 2>/dev/null)): $(cat "$T/out")"; fi

exit "$FAILED"
