#!/usr/bin/env bash
# patch-pub.sh [--i-know-what-i-am-doing]: gives the Clax app's native
# Flutter build (the -native recipes) the pub packages Bazel's build
# patches: it fetches the app's packages, at apps/clax_app/pubspec.lock's
# versions, into a pub cache of the checkout's own,
# apps/clax_app/build-native/pub-cache (git-ignored; never ~/.pub-cache),
# and applies each patch under bazel/flutter/plugin_overlays/<package>/<major>/
# to that package's copy there, once. A native build then runs with
# PUB_CACHE set to that directory (scripts/run.sh does). pubspec.lock is
# not changed: the packages stay the hosted ones it names.
#
# Without the flag it prints what it would do and changes nothing.
set -euo pipefail
cd "$(dirname "$0")/.."
root="$PWD"

act=""
case "${1:-}" in
    --i-know-what-i-am-doing) act=1 ;;
    "") ;;
    *) echo "usage: $0 [--i-know-what-i-am-doing]" >&2; exit 2 ;;
esac

cache="$root/apps/clax_app/build-native/pub-cache"
lock="$root/apps/clax_app/pubspec.lock"

# The version pubspec.lock gives package $1.
locked_version() {
    awk -v pkg="  $1:" '
        $0 == pkg { found = 1; next }
        found && /^  [^ ]/ { exit }
        found && $1 == "version:" { gsub(/"/, "", $2); print $2; exit }
    ' "$lock"
}

if [ -z "$act" ]; then
    echo "patch-pub: would run, in apps/clax_app: PUB_CACHE=$cache flutter pub get --enforce-lockfile"
else
    command -v flutter > /dev/null || { echo "patch-pub: flutter is not on PATH" >&2; exit 1; }
    (cd apps/clax_app && PUB_CACHE="$cache" flutter pub get --enforce-lockfile > /dev/null)
fi

for patch in bazel/flutter/plugin_overlays/*/*/*.patch; do
    [ -e "$patch" ] || continue
    rel="${patch#bazel/flutter/plugin_overlays/}"
    pkg="${rel%%/*}"
    major="${rel#*/}"
    major="${major%%/*}"
    version="$(locked_version "$pkg")"
    [ -n "$version" ] || { echo "patch-pub: $pkg is not in pubspec.lock" >&2; exit 1; }
    [ "${version%%.*}" = "$major" ] ||
        { echo "patch-pub: $patch is for $pkg $major.x, the lock has $version" >&2; exit 1; }
    dir="$cache/hosted/pub.dev/$pkg-$version"
    if [ -z "$act" ]; then
        echo "patch-pub: would apply $patch to $dir"
        continue
    fi
    [ -d "$dir" ] || { echo "patch-pub: $dir is missing after pub get" >&2; exit 1; }
    if patch --dry-run --reverse --force --silent -p1 -d "$dir" < "$patch" > /dev/null 2>&1; then
        echo "patch-pub: $pkg $version already patched ($patch)"
    else
        chmod -R u+w "$dir"
        patch --forward --silent -p1 -d "$dir" < "$patch"
        echo "patch-pub: patched $pkg $version ($patch)"
    fi
done
