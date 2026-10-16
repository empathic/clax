#!/bin/sh
# build-home-bundle.sh [--i-know-what-i-am-doing]: packs the home app's
# source (apps/clax_app/home/, with the kit files its manifest names) into
# the bundle the Clax app ships, apps/clax_app/assets/home/home.clax, and
# its release record, apps/clax_app/assets/home/release.json, with
# apps/clax_app/tool/pack_home.dart. Without the flag it is a dry run: it
# says what it would write and writes nothing. `just home-bundle` passes
# the flag.
#
# The bundle is signed by the home app's own Ed25519 key, whose seed is kept
# outside the repository, owner-only, at
# ${XDG_CONFIG_HOME:-~/.config}/clax/home-bundle/key (CLAX_HOME_BUNDLE_KEY
# names another file). The first packing makes it; every later release must
# be signed by the same key, so keep that file backed up: without it,
# packing a new release stops (pack_home.dart says why).
set -eu

act=0
case "${1:-}" in
    --i-know-what-i-am-doing) act=1 ;;
    "") ;;
    *) echo "usage: $0 [--i-know-what-i-am-doing]" >&2; exit 2 ;;
esac

root="$(cd "$(dirname "$0")/.." && pwd)"
app="$root/apps/clax_app"
key="${CLAX_HOME_BUNDLE_KEY:-${XDG_CONFIG_HOME:-$HOME/.config}/clax/home-bundle/key}"

command -v dart > /dev/null || { echo "build-home-bundle.sh: dart (from Flutter) is not on PATH" >&2; exit 1; }

# The seed file and its directory are the owner's only.
umask 077
if [ "$act" -eq 1 ]; then
    mkdir -p "$(dirname "$key")"
    set -- --key "$key" --write
else
    set -- --key "$key"
fi

cd "$app"
dart run tool/pack_home.dart "$@"
if [ "$act" -eq 0 ]; then
    echo "run again with --i-know-what-i-am-doing (or: just home-bundle) to write"
fi
