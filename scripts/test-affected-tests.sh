#!/usr/bin/env bash
# Tests scripts/affected-tests.sh on a scratch git checkout with a stub
# bazelisk: which files it hands `bazel query` and as what (a file, a
# package), when it runs every test instead, that `manual` tests are left
# out, the empty cases, removed files, the base-ref and range modes, that any
# query error stops it, and what it passes to `bazel test`.
#
# The stub records each query's text in $tmp/query, its flags in
# $tmp/query.args and each `bazel test` command line in $tmp/test, prints $tmp/query.out on stdout and
# $tmp/query.err on stderr, and exits QUERY_EXIT for a query and TEST_EXIT
# for a test (default 0).

set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

fails=0
_ok() { echo "ok: $1"; }
_fail() {
    echo "FAIL: $1" >&2
    for f in out err query test; do
        [ -f "$tmp/$f" ] && sed "s/^/    $f | /" "$tmp/$f" >&2
    done
    fails=$((fails + 1))
}

mkdir -p "$tmp/bin" "$tmp/home"
cat > "$tmp/bin/bazelisk" <<'STUB'
#!/usr/bin/env bash
case "$1" in
    query)
        printf '%s\n' "$@" > "$STUB_DIR/query.args"
        for a in "$@"; do
            case "$a" in --query_file=*) cat "${a#--query_file=}" >> "$STUB_DIR/query" ;; esac
        done
        cat "$STUB_DIR/query.out"
        cat "$STUB_DIR/query.err" >&2
        exit "${QUERY_EXIT:-0}"
        ;;
    test)
        printf '%s\n' "$@" > "$STUB_DIR/test"
        exit "${TEST_EXIT:-0}"
        ;;
esac
exit 1
STUB
chmod +x "$tmp/bin/bazelisk"

export STUB_DIR="$tmp" HOME="$tmp/home" TMPDIR="$tmp" LC_ALL=C GIT_CONFIG_NOSYSTEM=1
export PATH="$tmp/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@example.com GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@example.com

# The fixture: a root package, packages a and web, a MODULE.bazel that
# names web's lockfile, an ignored directory and a git-ignored file.
repo="$tmp/repo"
mkdir -p "$repo/scripts/lib" "$repo/a/src" "$repo/a/sub" "$repo/docs" "$repo/web" "$repo/bazel"
cp scripts/affected-tests.sh "$repo/scripts/"
cp scripts/lib/bazel-test-flags.sh "$repo/scripts/lib/"
cat > "$repo/MODULE.bazel" <<'EOF'
npm.npm_translate_lock(
    name = "npm_web",
    pnpm_lock = "//web:pnpm-lock.yaml",
)
EOF
printf 'vendor\n# a comment\n' > "$repo/.bazelignore"
printf 'ignored.txt\n' > "$repo/.gitignore"
for f in BUILD.bazel README.md a/BUILD.bazel a/src/x.rs a/src/y.rs a/sub/z.txt docs/n.md \
    web/BUILD.bazel web/pnpm-lock.yaml bazel/defs.bzl; do
    echo "$f" > "$repo/$f"
done
git -C "$repo" init -q -b main
git -C "$repo" add -A
git -C "$repo" commit -qm fixture

# _run <args...>: the script in the fixture, after the stub's answers were
# set; its stdout in $tmp/out, stderr in $tmp/err, status in $status.
_run() {
    rm -f "$tmp/query" "$tmp/query.args" "$tmp/test"
    status=0
    "$repo/scripts/affected-tests.sh" "$@" > "$tmp/out" 2> "$tmp/err" || status=$?
}

# _answer <stdout> [stderr]: what the stub query prints, as
# --output=label_kind prints it.
_answer() {
    printf '%s' "$1" > "$tmp/query.out"
    printf '%s' "${2:-}" > "$tmp/query.err"
}

# _clean: the fixture back at main's tip, with nothing changed.
_clean() {
    git -C "$repo" checkout -q main
    git -C "$repo" reset -q --hard
    git -C "$repo" clean -qfdx
    unset QUERY_EXIT TEST_EXIT
    _answer ""
}

manual=' except attr(tags, "\bmanual\b", //...)'
# _files_query <regex> [set]: the query for files matched by <regex>, with
# the patterns in <set> (`"//a:*"`) beside them.
_files_query() {
    # shellcheck disable=SC2016 # the query's own variable
    local from='$files'
    [ -z "${2:-}" ] || from+=" + set( $2 )"
    printf '%s' "let files = filter(\"^($1)\$\", kind(\"source file\", deps(//...))) in (tests(rdeps(//..., $from))$manual) + \$files"
}

# --- Nothing changed: no query, no test.
_clean
_run
if [ "$status" = 0 ] && grep -q "nothing changed against HEAD" "$tmp/err" &&
    [ ! -f "$tmp/query" ] && [ ! -f "$tmp/test" ] && [ ! -s "$tmp/out" ]; then
    _ok "a clean tree runs nothing and exits 0"
else
    _fail "a clean tree runs nothing and exits 0"
fi

# --- Files to labels: an edited file, a new one, a changed BUILD file (its
# package), a file no target uses, one under .bazelignore, a git-ignored one.
_clean
echo edit >> "$repo/a/src/x.rs"
echo new > "$repo/a/src/new.rs"
echo edit >> "$repo/a/BUILD.bazel"
echo edit >> "$repo/docs/n.md"
mkdir -p "$repo/vendor"
echo new > "$repo/vendor/v.txt"
echo new > "$repo/ignored.txt"
_answer $'rust_test rule //a:x_test\nsh_test rule //web:unit\nsource file //a:src/new.rs\nsource file //a:src/x.rs\n'
_run --list
want="$(_files_query '\/\/a:src\/new\.rs|\/\/a:src\/x\.rs|\/\/:docs\/n\.md' '"//a:*"')"
if [ "$status" = 0 ] && [ "$(cat "$tmp/query")" = "$want" ]; then
    _ok "edited and new files go to the query as their packages' source files, a BUILD file as its package"
else
    _fail "edited and new files go to the query as their packages' source files, a BUILD file as its package"
fi
if grep -qx -- "--output=label_kind" "$tmp/query.args" && ! grep -qx -- "--keep_going" "$tmp/query.args"; then
    _ok "the query runs without --keep_going"
else
    _fail "the query runs without --keep_going"
fi
if grep -A1 "no Bazel target uses" "$tmp/err" | grep -q "docs/n.md" &&
    grep -A1 "directory Bazel ignores" "$tmp/err" | grep -q "vendor/v.txt" &&
    ! grep -q "ignored.txt" "$tmp/err" "$tmp/query"; then
    _ok "files no target uses and ignored ones are named; git-ignored ones are not seen"
else
    _fail "files no target uses and ignored ones are named; git-ignored ones are not seen"
fi
if [ "$(cat "$tmp/out")" = $'//a:x_test\n//web:unit' ] && [ ! -f "$tmp/test" ]; then
    _ok "--list prints the tests and runs none"
else
    _fail "--list prints the tests and runs none"
fi

# --- Running: Bazel's flags, the arguments passed through, Bazel's status.
export TEST_EXIT=3
_run --test_output=errors -- --test_arg x
if [ "$status" = 3 ] && grep -qx -- "--keep_going" "$tmp/test" &&
    grep -qx -- "--lockfile_mode=error" "$tmp/test" &&
    grep -qx -- "--test_output=errors" "$tmp/test" &&
    grep -qx -- "--test_arg" "$tmp/test" && grep -qx -- "x" "$tmp/test" &&
    [ "$(sed -n '/^--$/,$p' "$tmp/test")" = $'--\n//a:x_test\n//web:unit' ] &&
    [ "$(cat "$tmp/out")" = $'//a:x_test\n//web:unit' ]; then
    _ok "runs the tests with --keep_going and the arguments given, and exits with Bazel's status"
else
    _fail "runs the tests with --keep_going and the arguments given, and exits with Bazel's status"
fi

# --- Files that change the build itself: every test but the manual ones.
for change in MODULE.bazel web/pnpm-lock.yaml bazel/defs.bzl a/defs.bzl .bazelrc Cargo.lock; do
    _clean
    echo edit >> "$repo/$change"
    _answer $'rust_test rule //a:x_test\n'
    _run --list
    if [ "$status" = 0 ] && [ "$(cat "$tmp/query")" = "tests(//...)$manual" ] &&
        grep -q "every test: $change" "$tmp/err"; then
        _ok "$change selects every test and says why"
    else
        _fail "$change selects every test and says why"
    fi
done
_clean
rm "$repo/a/BUILD.bazel"
_run --list
if [ "$(cat "$tmp/query")" = "tests(//...)$manual" ] && grep -q "a/BUILD.bazel, a package's BUILD file, was removed" "$tmp/err"; then
    _ok "a removed BUILD file selects every test"
else
    _fail "a removed BUILD file selects every test"
fi

# --- Manual tests: both selections leave them out (above), and a test
# selected through a file is never one.
# shellcheck disable=SC2016 # the query's own variable
case "$want" in
    *'except attr(tags, "\bmanual\b", //...)) + $files') _ok "the selection through files leaves out manual tests" ;;
    *) _fail "the selection through files leaves out manual tests" ;;
esac

# --- Removed files: their package, once each.
_clean
rm "$repo/a/src/y.rs" "$repo/a/sub/z.txt" "$repo/README.md"
_run --list
if [ "$(cat "$tmp/query")" = "tests(rdeps(//..., set( \"//:*\" \"//a:*\" )))$manual" ]; then
    _ok "a removed file selects its former package"
else
    _fail "a removed file selects its former package"
fi

# --- A file in a subdirectory of its package, its name holding regex
# characters and a space: matched literally, and named unused only when the
# query does not list it.
_clean
echo new > "$repo/a/sub/p+q (1).txt"
_answer $'rust_test rule //a:x_test\nsource file //a:sub/p+q (1).txt\n'
_run --list
if [ "$status" = 0 ] &&
    [ "$(cat "$tmp/query")" = "$(_files_query '\/\/a:sub\/p\+q \(1\)\.txt')" ] &&
    [ "$(cat "$tmp/out")" = "//a:x_test" ] && ! grep -q "no Bazel target uses" "$tmp/err"; then
    _ok "a file's label is matched literally, under its package"
else
    _fail "a file's label is matched literally, under its package"
fi

# --- Only files no target uses: no test runs.
_clean
echo edit >> "$repo/docs/n.md"
_answer ""
_run
if [ "$status" = 0 ] && grep -q "no tests affected" "$tmp/err" &&
    grep -A1 "no Bazel target uses" "$tmp/err" | grep -q "docs/n.md" &&
    [ ! -f "$tmp/test" ] && [ ! -s "$tmp/out" ]; then
    _ok "changes no test reaches run nothing and exit 0"
else
    _fail "changes no test reaches run nothing and exit 0"
fi

# --- Query errors: any error stops the run, one in an external repository
# as much as one in this repository's packages.
_clean
echo edit >> "$repo/a/src/x.rs"
_answer $'rust_test rule //a:x_test\n' "ERROR: /r/a/BUILD.bazel:1:1: no such package '@@ndk//linux-x86_64': BUILD file not found
ERROR: Evaluation of query \"rdeps(...)\" failed: errors were encountered while computing transitive closure
"
export QUERY_EXIT=7
_run --list
if [ "$status" = 2 ] && grep -q "no such package '@@ndk" "$tmp/err" && grep -q "bazel query failed (exit 7)" "$tmp/err" &&
    [ ! -s "$tmp/out" ]; then
    _ok "an external repository's missing package stops the run"
else
    _fail "an external repository's missing package stops the run"
fi
_answer $'rust_test rule //a:x_test\n' $'ERROR: /r/web/BUILD.bazel:3:1: name \'oops\' is not defined\n'
_run
if [ "$status" = 2 ] && grep -q "is not defined" "$tmp/err" && [ ! -f "$tmp/test" ]; then
    _ok "an error in this repository's BUILD files stops the run"
else
    _fail "an error in this repository's BUILD files stops the run"
fi
_answer "" "ERROR: server crashed"
export QUERY_EXIT=37
_run
if [ "$status" = 2 ] && [ ! -f "$tmp/test" ]; then
    _ok "a failed query stops the run"
else
    _fail "a failed query stops the run"
fi

# --- A base ref: the branch's commits since the merge-base count, the
# base's own later commits do not, and uncommitted work still does.
_clean
git -C "$repo" checkout -q -b feature
echo feature >> "$repo/a/src/x.rs"
git -C "$repo" commit -qam "feature edit"
git -C "$repo" checkout -q main
echo main >> "$repo/docs/n.md"
git -C "$repo" commit -qam "main edit"
git -C "$repo" checkout -q feature
_answer $'rust_test rule //a:x_test\n'
_run --list
if [ "$status" = 0 ] && grep -q "nothing changed against HEAD" "$tmp/err"; then
    _ok "without a base, committed work is not a change"
else
    _fail "without a base, committed work is not a change"
fi
echo uncommitted > "$repo/web/new.ts"
_run --list main
if [ "$status" = 0 ] &&
    [ "$(cat "$tmp/query")" = "$(_files_query '\/\/a:src\/x\.rs|\/\/web:new\.ts')" ] &&
    grep -q "against main (merge-base" "$tmp/err"; then
    _ok "a base adds the branch's commits since the merge-base, not the base's"
else
    _fail "a base adds the branch's commits since the merge-base, not the base's"
fi
rm "$repo/web/new.ts"
_run --list HEAD^
if [ "$(cat "$tmp/query")" = "$(_files_query '\/\/a:src\/x\.rs')" ]; then
    _ok "<sha>^ as the base selects one commit's change"
else
    _fail "<sha>^ as the base selects one commit's change"
fi
_run --list no-such-ref
if [ "$status" = 2 ] && grep -q "no merge-base of 'no-such-ref'" "$tmp/err"; then
    _ok "an unknown base is refused"
else
    _fail "an unknown base is refused"
fi
echo uncommitted > "$repo/web/new.ts"
_run --list main..feature
if [ "$status" = 0 ] && [ "$(cat "$tmp/query")" = "$(_files_query '\/\/a:src\/x\.rs')" ]; then
    _ok "a range A..B selects its commits' changes and not the working tree's"
else
    _fail "a range A..B selects its commits' changes and not the working tree's"
fi
_run --list main..no-such-ref
if [ "$status" = 2 ] && grep -q "not a range of two commits" "$tmp/err"; then
    _ok "a range with an unknown end is refused"
else
    _fail "a range with an unknown end is refused"
fi

if [ "$fails" -gt 0 ]; then
    echo "$fails failed" >&2
    exit 1
fi
echo "all passed"
