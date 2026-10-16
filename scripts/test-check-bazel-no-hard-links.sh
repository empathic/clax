#!/usr/bin/env bash
# Checks scripts/check-bazel-no-hard-links.sh: a directory of plain files
# passes; a file `find` reports with more than one link fails the check,
# named. It makes no hard link: a fake `find` stands in for one that finds
# such a file.
set -euo pipefail
cd "$(dirname "$0")/.."
d="$(cd "$(mktemp -d)" && pwd -P)"
trap 'chmod -R u+w "$d" 2>/dev/null; rm -rf "$d"' EXIT
fails=0
mkdir -p "$d/tree/sub" "$d/bin"
printf 'a\n' > "$d/tree/a"
printf 'b\n' > "$d/tree/sub/b"
if scripts/check-bazel-no-hard-links.sh "$d/tree" >/dev/null 2>&1; then
    echo "PASS: plain files pass"
else
    echo "FAIL: plain files pass"; fails=1
fi
# shellcheck source=scripts/fake-exe.sh
. scripts/fake-exe.sh
printf '#!/bin/sh\nfor a in "$@"; do case "$a" in +1) echo "%s/tree/a" ;; esac; done\n' "$d" | fake_exe "$d/bin/find"
if out="$(PATH="$d/bin:$PATH" scripts/check-bazel-no-hard-links.sh "$d/tree" 2>&1)"; then
    echo "FAIL: a file with two links fails"; fails=1
elif printf '%s\n' "$out" | grep -F "$d/tree/a" >/dev/null; then
    echo "PASS: a file with two links fails, named"
else
    echo "FAIL: the failure names the file ($out)"; fails=1
fi
exit "$fails"
