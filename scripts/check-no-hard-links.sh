#!/usr/bin/env bash
# Fails when a tracked source file or script makes a hard link: a call of
# `hard_link` (Rust), `link`/`linkSync` (Node), `os.link` (Python) or
# `linkat`, an `ln` (or a `$..._LN` variable naming it) without -s, or a
# `cp -l`, or a Bazel copy rule's `hardlink = "on"` or `"auto"` (in .bzl,
# BUILD.bazel, MODULE.bazel and .bazelrc files). Clax makes no hard links,
# in its code, its tests or its scripts; a symbolic link does instead. With file arguments, checks those files
# (for trying the check out); without, every tracked file of the kinds
# below.
set -euo pipefail
cd "$(dirname "$0")/.."

if [ $# -gt 0 ]; then
    files=("$@")
else
    files=()
    while IFS= read -r -d '' f; do
        case "$f" in scripts/check-no-hard-links.sh|scripts/check-no-hard-links.py|scripts/test-check-no-hard-links.sh) ;; *) files+=("$f") ;; esac
    done < <(git ls-files -z -- '*.rs' '*.ts' '*.tsx' '*.js' '*.mjs' '*.cjs' '*.svelte' \
        '*.py' '*.sh' '*.bash' 'justfile' '**/justfile' 'Makefile' '**/Makefile' \
        '*.bzl' 'BUILD.bazel' '**/BUILD.bazel' 'MODULE.bazel' '.bazelrc' '*.bazelrc')
fi

python3 scripts/check-no-hard-links.py "${files[@]}"
