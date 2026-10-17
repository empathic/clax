#!/usr/bin/env bash
# version-sync.sh NAME CODE: fails unless apps/clax_app/pubspec.yaml's
# version is NAME+CODE, the version the Bazel-built APK carries
# (apps/clax_app/BUILD.bazel VERSION_NAME and VERSION_CODE).
set -euo pipefail
if [ -n "${TEST_SRCDIR:-}" ]; then
    pubspec="$TEST_SRCDIR/$TEST_WORKSPACE/apps/clax_app/pubspec.yaml"
else
    pubspec="$(dirname "$0")/../pubspec.yaml"
fi
want="version: $1+$2"
got="$(grep -E '^version:' "$pubspec")"
if [ "$got" != "$want" ]; then
    echo "pubspec.yaml has '$got'; apps/clax_app/BUILD.bazel builds the APK as '$want'. Change VERSION_NAME and VERSION_CODE there with it." >&2
    exit 1
fi
