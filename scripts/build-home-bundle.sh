#!/bin/sh
# build-home-bundle.sh [--i-know-what-i-am-doing]: packs the home app's
# source (apps/clax_app/home/, with the kit files its manifest names) into
# the bundle a native build of the Clax app ships,
# apps/clax_app/assets/home/home.clax, and its release record,
# apps/clax_app/assets/home/release.json, with
# apps/clax_app/tool/pack_home.dart run by `dart run`. Both are build
# outputs, ignored by git. Without the flag it is a dry run: it says the
# version it would give and writes nothing. The native builds that need the
# bundle (scripts/run.sh app-apk, app-macos and app-test with
# CLAX_BUILD=native) run it with the flag; under Bazel the bundle is
# //apps/clax_app:home_bundle, which every app build depends on.
#
# The version, and the seed that signs the bundle, are this machine's home
# release (apps/clax_app/tool/home-version.sh): the version is the last one
# while the sources are unchanged and the next one when they changed,
# recorded in the ledger beside the home app's release key,
# ${XDG_CONFIG_HOME:-~/.config}/clax/home-bundle/key (CLAX_HOME_BUNDLE_KEY
# names another file). That key's seed is kept outside the repository,
# owner-only; every release must be signed by it, so keep that directory
# backed up. Without the key (and without a ledger) the bundle is signed by
# the development key, on the development lineage.
set -eu

act=0
case "${1:-}" in
    --i-know-what-i-am-doing) act=1 ;;
    "") ;;
    *) echo "usage: $0 [--i-know-what-i-am-doing]" >&2; exit 2 ;;
esac

app="$(cd "$(dirname "$0")/../apps/clax_app" && pwd)"
command -v dart > /dev/null || { echo "build-home-bundle.sh: dart (from Flutter) is not on PATH" >&2; exit 1; }

if [ "$act" -eq 0 ]; then
    release="$("$app/tool/home-version.sh" "$app" peek)"
else
    release="$("$app/tool/home-version.sh" "$app" record)"
fi
field() { printf '%s\n' "$release" | sed -n "s/^$1=//p"; }
version="$(field version)" key="$(field key)" mode="$(field mode)"
what="v$version, signed by $key"
[ "$mode" = dev ] && what="$what (the development key: no release key on this machine)"

if [ "$act" -eq 0 ]; then
    echo "would pack apps/clax_app/home into apps/clax_app/assets/home/home.clax and release.json: $what"
    echo "run again with --i-know-what-i-am-doing to write"
    exit 0
fi
set -- --app "$app" --key "$key" --version "$version" --out "$app/assets/home"
[ "$mode" = dev ] && set -- "$@" --dev
cd "$app"
dart run tool/pack_home.dart "$@"
