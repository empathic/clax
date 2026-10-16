#!/usr/bin/env bash
# Checks that scripts/fake-exe.sh keeps its store in CLAX_FAKE_EXE_DIR when
# set, and in $TMPDIR/clax-fake-exe when not.
set -euo pipefail
cd "$(dirname "$0")/.."
d="$(cd "$(mktemp -d)" && pwd -P)"
trap 'chmod -R u+w "$d" 2>/dev/null; rm -rf "$d"' EXIT
(
    export CLAX_FAKE_EXE_DIR="$d/store"
    # shellcheck source=scripts/fake-exe.sh
    . scripts/fake-exe.sh
    mkdir -p "$d/bin"
    printf '#!/bin/sh\necho hi\n' | fake_exe "$d/bin/hi"
    [ "$("$d/bin/hi")" = hi ]
    target="$(readlink "$d/bin/hi")"
    case "$target" in "$d/store/"*) ;; *) echo "FAIL: linked to $target, not under $d/store" >&2; exit 1 ;; esac
)
(
    export TMPDIR="$d/tmp"; mkdir -p "$TMPDIR"; unset CLAX_FAKE_EXE_DIR
    # shellcheck source=scripts/fake-exe.sh
    . scripts/fake-exe.sh
    mkdir -p "$d/bin2"
    printf '#!/bin/sh\necho hi\n' | fake_exe "$d/bin2/hi"
    case "$(readlink "$d/bin2/hi")" in "$d/tmp/clax-fake-exe/"*) ;; *) echo "FAIL: default store" >&2; exit 1 ;; esac
)
echo "PASS: fake-exe.sh store"
