#!/usr/bin/env bash
# app-macos-dev.sh [--i-know-what-i-am-doing] [flutter run args…]: runs the
# Clax app on this Mac with Flutter's hot reload (r) and hot restart (R).
# It builds the embedded clax natively (scripts/build-app-clax.sh macos,
# a release build that embeds the web UI, into
# apps/clax_app/build-native/macos/clax), packs the home app's
# bundle (scripts/build-home-bundle.sh), then runs `flutter run -d macos`
# in apps/clax_app, passing on any further arguments. Without the flag it
# prints the steps and changes nothing.
#
# A debug build runs without the App Sandbox, so its data is apart from the
# release app's container; set CLAX_APP_DIR for a scratch home. Swift and
# menu changes need a new run; a change to apps/clax_app/home needs this
# script again (it repacks the bundle), then R.
set -euo pipefail
cd "$(dirname "$0")/.."

act=""
if [ "${1:-}" = --i-know-what-i-am-doing ]; then
    act=1
    shift
fi

steps=(
    "./scripts/build-app-clax.sh macos"
    "./scripts/build-home-bundle.sh --i-know-what-i-am-doing"
    "(cd apps/clax_app && flutter run -d macos $*)"
)

if [ -z "$act" ]; then
    echo "app-macos-dev: would run, in $(pwd):"
    printf '  %s\n' "${steps[@]}"
    echo "run again with --i-know-what-i-am-doing (or: just app-macos-dev) to do it"
    exit 0
fi

command -v flutter >/dev/null || { echo "app-macos-dev: flutter is not on PATH" >&2; exit 1; }
echo "app-macos-dev: building clax for macOS"
./scripts/build-app-clax.sh macos
echo "app-macos-dev: packing the home app's bundle"
./scripts/build-home-bundle.sh --i-know-what-i-am-doing
echo "app-macos-dev: starting flutter run (r reloads, R restarts, q quits)"
cd apps/clax_app
exec flutter run -d macos "$@"
