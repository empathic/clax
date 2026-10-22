#!/usr/bin/env bash
# Tests check-build-version.sh on archives of objects compiled here for
# iOS and its simulator at chosen minimums.
set -euo pipefail

check="$1"
work="${TEST_TMPDIR:-$(mktemp -d)}"
failures=0

object() { # <name> <SDK> <clang target>
    printf 'int %s(void) { return 1; }\n' "$1" >"$work/$1.c"
    xcrun --sdk "$2" clang -target "$3" -c "$work/$1.c" -o "$work/$1.o"
}

archive() { # <archive> <object names…>
    local out="$work/$1.a"
    shift
    local objects=()
    for o in "$@"; do objects+=("$work/$o.o"); done
    rm -f "$out"
    xcrun ar rcs "$out" "${objects[@]}"
    echo "$out"
}

expect() { # <pass|fail> <description> <archive> <platform> <minos>
    local want="$1" what="$2"
    shift 2
    if "$check" "$@" "$work/stamp" >"$work/out" 2>&1; then got=pass; else got=fail; fi
    if [[ "$got" != "$want" ]]; then
        echo "FAIL: $what: expected $want, got $got" >&2
        cat "$work/out" >&2
        failures=$((failures + 1))
    fi
}

object sim17 iphonesimulator arm64-apple-ios17.0-simulator
object sim16 iphonesimulator arm64-apple-ios16.0-simulator
object sim18 iphonesimulator arm64-apple-ios18.0-simulator
object dev17 iphoneos arm64-apple-ios17.0
object mac macosx arm64-apple-macos14.0

expect pass "simulator objects at the minimum" "$(archive a sim17)" 7 17.0
expect pass "a simulator object older than the minimum" "$(archive b sim17 sim16)" 7 17.0
expect fail "a simulator object newer than the minimum" "$(archive c sim17 sim18)" 7 17.0
expect fail "a device object in a simulator archive" "$(archive d sim17 dev17)" 7 17.0
expect pass "device objects at the minimum" "$(archive e dev17)" 2 17.0
expect fail "a simulator object in a device archive" "$(archive f dev17 sim17)" 2 17.0
expect fail "a macOS object" "$(archive g sim17 mac)" 7 17.0
expect fail "17.0 is newer than 9.0, compared by component" "$(archive h sim17)" 7 9.0

if ((failures)); then
    exit 1
fi
echo "PASS: check-build-version.sh"
