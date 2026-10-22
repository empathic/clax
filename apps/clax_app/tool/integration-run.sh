#!/usr/bin/env bash
# integration-run.sh ZIP [ARG...]: runs an integration test app (a zip of
# Clax.app built by //apps/clax_app:integration_<name>, whose entry
# point runs one integration_test/<name>_test.dart and exits with its
# result) with ARGs as its launch arguments, and exits as it does. The app
# opens a window: run it in a logged-in session, as `flutter test
# integration_test/... -d macos` does.
set -euo pipefail
dir="$(mktemp -d "${TEST_TMPDIR:-${TMPDIR:-/tmp}}/integration.XXXXXX")"
trap 'rm -rf "$dir"' EXIT
unzip -q "$1" -d "$dir"
exec "$dir/Clax.app/Contents/MacOS/Clax" "${@:2}"
