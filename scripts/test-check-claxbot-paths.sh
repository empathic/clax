#!/usr/bin/env bash
# Checks scripts/check-claxbot-paths.py: deploy/claxbot-workspace/clax.toml comes
# to what path-cases.txt says, the answers the daemon's matcher gives on the same
# file; `judge` names what it refuses and fails closed on nothing to read; `tree`
# fails on an entry the tree no longer holds and passes one marked `# absent:`.
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1
d="$(mktemp -d)"
trap 'rm -rf "$d"' EXIT
check="$PWD/scripts/check-claxbot-paths.py"
profile="$PWD/deploy/claxbot-workspace/clax.toml"
fails=0
_fail() { echo "FAIL: $1" >&2; fails=$((fails + 1)); }

out="$(python3 "$check" cases "$profile" deploy/claxbot-workspace/path-cases.txt 2>&1)" \
    || _fail "the profile and path-cases.txt disagree: $out"

# judge
out="$(printf 'docs/feedback.md\0crates/clax-core/src/lib.rs\0' | python3 "$check" judge "$profile" 2>&1)" \
    || _fail "writable paths were refused: $out"
out="$(printf 'docs/feedback.md\0Cargo.lock\0web/package.json\0' | python3 "$check" judge "$profile" 2>&1)"
st=$?
[ "$st" = 1 ] || _fail "a lockfile and a manifest passed (exit $st)"
[ "$out" = "$(printf 'Cargo.lock\nweb/package.json')" ] || _fail "judge named: $out"
printf '' | python3 "$check" judge "$profile" >/dev/null 2>&1 && _fail "no paths passed"

# A profile the check cannot read refuses everything.
printf 'protected_paths = [\n  "a",\n]\n' >"$d/half.toml"
printf 'x\0' | python3 "$check" judge "$d/half.toml" >/dev/null 2>&1 && _fail "a profile with no writable_paths passed"
printf 'writable_paths = [\n  "a/",\n  bad,\n]\nprotected_paths = [\n  "b",\n]\n' >"$d/bad.toml"
printf 'a/x\0' | python3 "$check" judge "$d/bad.toml" >/dev/null 2>&1 && _fail "an unreadable entry passed"

# tree
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
git init --quiet "$d/t"
mkdir -p "$d/t/docs/sub"
: >"$d/t/docs/sub/a.md"
: >"$d/t/Cargo.lock"
git -C "$d/t" add -A
cat >"$d/t/p.toml" <<'EOF'
writable_paths = [
  "docs/",
  "crates/*/src/",
]
protected_paths = [
  "Cargo.lock",
  "**/build.rs",
  "user.bazelrc",  # absent: git-ignored, and read if it is made
]
EOF
out="$(cd "$d/t" && python3 "$check" tree p.toml 2>&1)" || _fail "a tree holding every entry failed: $out"
sed -i.bak 's/"Cargo.lock",/"Gone.lock",/' "$d/t/p.toml"
out="$(cd "$d/t" && python3 "$check" tree p.toml 2>&1)" && _fail "an entry gone from the tree passed"
grep -qF 'Gone.lock' <<<"$out" || _fail "tree named: $out"

[ "$fails" = 0 ] && echo "check-claxbot-paths: ok"
exit "$((fails > 0))"
