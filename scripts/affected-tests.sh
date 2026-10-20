#!/usr/bin/env bash
# Runs the Bazel tests downstream of what changed: the build graph, not a
# person or an agent, chooses them.
#
#   scripts/affected-tests.sh [--list] [BASE | A..B] [BAZEL ARGS...] [-- BAZEL ARGS...]
#
#   (no BASE)  the working tree against HEAD: tracked edits, staged or not,
#              and new files git does not ignore
#   BASE       the working tree against BASE's merge-base with HEAD, so a
#              branch's commits count too (`main`, `origin/flutter-app`,
#              `HEAD^` for the last commit and the working tree)
#   A..B       the commits in A..B alone, without the working tree
#              (`<sha>^..<sha>` for one commit), on today's build graph
#   --list     print the tests and run none
#
# Arguments that start with `-` go to `bazel test` (`--test_output=all`);
# after `--`, every argument does. The tests are printed on stdout, one per
# line, before they run; what the script decided about the changed files
# goes to stderr. The exit status is Bazel's, or 0 when no test is
# affected.
#
# How files become tests:
#   - A file Bazel's build reads (MODULE.bazel, MODULE.bazel.lock, .bazelrc,
#     .bazelversion, .bazelignore, any *.bzl, anything under bazel/,
#     Cargo.lock, and every file MODULE.bazel names by label, such as the
#     lockfiles its extensions read) changes the build itself: every test
#     runs, as `just check` runs them.
#   - A changed BUILD file selects its whole package; a removed one, every
#     test.
#   - A removed file selects the whole package it was in.
#   - Any other file is the source file it is in its package (by the
#     nearest BUILD file), and a file no target in //... uses selects
#     nothing (the script says which).
#   - The tests are tests(rdeps(//..., those targets)), less the ones tagged
#     `manual`: the selection scripts/bazel-test.sh --ci makes of //...
#
# The query runs without --keep_going: any error it reports stops the run.
#
# It runs `bazel query` once and `bazel test` once, with the flags
# scripts/bazel-test.sh gives its tests, so their results are cached for
# each other.

set -uo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

_say() { echo "affected-tests: $*" >&2; }
# _files <what> <path...>: a line saying what the paths are, then up to ten
# of them.
_files() {
    local what="$1" n=0
    shift
    _say "$# $what:"
    for f in "$@"; do
        n=$((n + 1))
        if [ "$n" -gt 10 ]; then
            echo "    … and $(($# - 10)) more" >&2
            break
        fi
        echo "    $f" >&2
    done
}
_die() {
    echo "error: $*" >&2
    exit 2
}

list=0
base=""
bazel_args=()
while [ $# -gt 0 ]; do
    case "$1" in
        --list) list=1 ;;
        -h | --help)
            sed -n '2,/^$/s/^# \{0,1\}//p' "${BASH_SOURCE[0]}"
            exit 0
            ;;
        --)
            shift
            bazel_args+=("$@")
            break
            ;;
        -*) bazel_args+=("$1") ;;
        *)
            [ -z "$base" ] || _die "one base at most: '$base', then '$1'"
            base="$1"
            ;;
    esac
    shift
done

bazel="$(command -v bazelisk || command -v bazel || true)"
[ -n "$bazel" ] || _die "bazelisk not found; install it (brew install bazelisk)"

range=""
case "$base" in
    *..*)
        range="$base"
        against="$base"
        for end in "${base%%..*}" "${base#*..}"; do
            git rev-parse -q --verify "$end^{commit}" > /dev/null ||
                _die "not a range of two commits: '$base'"
        done
        ;;
    ?*)
        from="$(git merge-base "$base" HEAD)" || _die "no merge-base of '$base' and HEAD"
        against="$base (merge-base ${from:0:12})"
        ;;
    *)
        from=HEAD
        against=HEAD
        ;;
esac

changed=()
while IFS= read -r -d '' f; do
    changed+=("$f")
done < <(
    if [ -n "$range" ]; then
        # The commits in A..B, as `git log A..B` lists them: B against its
        # merge-base with A.
        git diff --name-only --no-renames -z "${range%%..*}...${range#*..}" --
    else
        git diff --name-only --no-renames -z "$from" --
        git ls-files --others --exclude-standard -z
    fi | LC_ALL=C sort -zu
)
if [ "${#changed[@]}" = 0 ]; then
    _say "nothing changed against $against; no tests to run"
    exit 0
fi
_say "${#changed[@]} files changed against $against"

# What the build itself reads: every repository file MODULE.bazel names by
# label ("//pkg:file").
module_files=()
if [ -f MODULE.bazel ]; then
    while IFS= read -r label; do
        label="${label#//}"
        case "$label" in
            :*) module_files+=("${label#:}") ;;
            *:*) module_files+=("${label%%:*}/${label#*:}") ;;
        esac
    done < <(grep -oE '"//[^"@]*:[^"]+"' MODULE.bazel | tr -d '"')
fi

# The directories Bazel ignores (.bazelignore): no target can use a file
# under them.
ignored=()
if [ -f .bazelignore ]; then
    while IFS= read -r line; do
        line="${line%%#*}"
        line="${line%/}"
        line="$(printf '%s' "$line" | tr -d '[:space:]')"
        [ -n "$line" ] && ignored+=("$line")
    done < .bazelignore
fi

# _package <path>: the package a path is in, as in a label (`a/b`, or ``
# for the root), by the nearest BUILD file above it in the working tree.
_package() {
    local dir
    dir="$(dirname "$1")"
    while [ "$dir" != "." ]; do
        if [ -f "$dir/BUILD.bazel" ] || [ -f "$dir/BUILD" ]; then
            echo "$dir"
            return
        fi
        dir="$(dirname "$dir")"
    done
    echo ""
}

whole=""     # why every test runs, when one does
files=()     # source files, each its own label
packages=()  # //pkg:* for a changed BUILD file or a removed file
unbuilt=()   # files under a directory Bazel ignores
for f in "${changed[@]}"; do
    reason=""
    case "$f" in
        MODULE.bazel | MODULE.bazel.lock | .bazelrc | .bazelversion | .bazelignore | Cargo.lock | \
            *.bzl | bazel/*)
            reason="$f changes the build itself"
            ;;
    esac
    if [ -z "$reason" ]; then
        for m in "${module_files[@]+"${module_files[@]}"}"; do
            if [ "$f" = "$m" ]; then
                reason="$f is read by MODULE.bazel"
                break
            fi
        done
    fi
    if [ -n "$reason" ]; then
        [ -n "$whole" ] || whole="$reason"
        continue
    fi
    skip=0
    for i in "${ignored[@]+"${ignored[@]}"}"; do
        case "$f" in "$i" | "$i"/*) skip=1 ;; esac
    done
    if [ "$skip" = 1 ]; then
        unbuilt+=("$f")
        continue
    fi
    name="$(basename "$f")"
    if [ "$name" = BUILD.bazel ] || [ "$name" = BUILD ]; then
        if [ -e "$f" ]; then
            dir="$(dirname "$f")"
            [ "$dir" = . ] && dir=""
            packages+=("//$dir:*")
        else
            [ -n "$whole" ] || whole="$f, a package's BUILD file, was removed"
        fi
    elif [ -e "$f" ] || [ -L "$f" ]; then
        files+=("$f")
    else
        packages+=("//$(_package "$f"):*")
    fi
done

if [ "${#unbuilt[@]}" -gt 0 ]; then
    _files "files under a directory Bazel ignores select no test" "${unbuilt[@]}"
fi

manual='attr(tags, "\bmanual\b", //...)'
labels=() # files[i] as a label: //pkg:path/in/pkg
if [ -n "$whole" ]; then
    _say "every test: $whole"
    query="tests(//...) except $manual"
elif [ "${#files[@]}" = 0 ] && [ "${#packages[@]}" = 0 ]; then
    _say "no tests affected"
    exit 0
else
    # Each file as the label of its package's source file, matched by a
    # regular expression against the source files //... uses: a file no
    # target uses matches nothing, where naming it would be an error to
    # `query`. Regex characters are escaped; a double quote, which a query
    # string cannot hold, matches any character.
    alts=""
    for f in "${files[@]+"${files[@]}"}"; do
        p="$(_package "$f")"
        if [ -n "$p" ]; then label="//$p:${f#"$p"/}"; else label="//:$f"; fi
        labels+=("$label")
        re="$(printf '%s' "$label" | sed -e 's/[][\.^$*+?(){}|/]/\\&/g' -e 's/"/./g')"
        alts+="${alts:+|}$re"
    done
    pkgs=""
    while IFS= read -r p; do
        [ -n "$p" ] && pkgs+=" \"$p\""
    done < <(printf '%s\n' "${packages[@]+"${packages[@]}"}" | LC_ALL=C sort -u)
    if [ -n "$alts" ]; then
        # shellcheck disable=SC2016 # the query's own variable
        from='$files'
        [ -z "$pkgs" ] || from+=" + set($pkgs )"
        # The tests, then the files some target uses, which the output
        # tells apart by kind.
        query="let files = filter(\"^($alts)\$\", kind(\"source file\", deps(//...))) in"
        query+=" (tests(rdeps(//..., $from)) except $manual) + \$files"
    else
        query="tests(rdeps(//..., set($pkgs ))) except $manual"
    fi
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
printf '%s\n' "$query" > "$tmp/query"

"$bazel" query --lockfile_mode=error --output=label_kind \
    --query_file="$tmp/query" > "$tmp/targets" 2> "$tmp/err"
status=$?
if [ "$status" != 0 ]; then
    cat "$tmp/err" >&2
    _die "bazel query failed (exit $status)"
fi

targets=()
used=$'\n' # the files' labels some target uses, one per line
while IFS= read -r line; do
    case "$line" in
        "") ;;
        "source file "*) used+="${line#source file }"$'\n' ;;
        *) targets+=("${line##* }") ;;
    esac
done < "$tmp/targets"
unused=()
for i in ${labels[@]+"${!labels[@]}"}; do
    case "$used" in *$'\n'"${labels[$i]}"$'\n'*) ;; *) unused+=("${files[$i]}") ;; esac
done
if [ "${#unused[@]}" -gt 0 ]; then
    _files "files no Bazel target uses select no test" "${unused[@]}"
fi

if [ "${#targets[@]}" = 0 ]; then
    _say "no tests affected"
    exit 0
fi
_say "${#targets[@]} tests:"
printf '%s\n' "${targets[@]}"
[ "$list" = 0 ] || exit 0

# shellcheck source=scripts/lib/bazel-test-flags.sh
source scripts/lib/bazel-test-flags.sh
# A machine that sleeps in the middle of a run fails tests that were in
# flight (scripts/bazel-test.sh).
awake=()
if command -v caffeinate >/dev/null 2>&1; then awake=(caffeinate -i); fi
# Each target is named, so one only for another platform (clax-ios's) is
# skipped as a pattern would skip it, not refused.
"${awake[@]+"${awake[@]}"}" "$bazel" test --keep_going --skip_incompatible_explicit_targets \
    "${bazel_flags[@]}" "${bazel_test_flags[@]}" "${bazel_args[@]+"${bazel_args[@]}"}" -- "${targets[@]}"
