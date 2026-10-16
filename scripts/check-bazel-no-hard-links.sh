#!/usr/bin/env bash
# Fails when Bazel's output base or the disk cache holds a regular file with
# more than one link: Clax makes no hard links, and neither may the rules it
# uses (docs/superpowers/specs/2026-10-15-bazel-overlay-design.md §13).
#
# Usage: scripts/check-bazel-no-hard-links.sh OUTPUT_BASE DISK_CACHE
set -euo pipefail
found="$(find "$@" -type f -links +1 2>/dev/null | head -20 || true)"
if [ -n "$found" ]; then
    echo "error: files with more than one link (hard links) under Bazel's directories:" >&2
    printf '  %s\n' "$found" >&2
    exit 1
fi
echo "no hard links in $*"
