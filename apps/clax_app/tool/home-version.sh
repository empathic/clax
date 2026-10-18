#!/usr/bin/env bash
# home-version.sh APP peek|record: the version the home app's bundle, packed
# from APP/home and APP/assets/kit, takes on this machine, and the seed that
# signs it. The build runs it (bazel/flutter/home_release.bzl with `record`,
# scripts/build-home-bundle.sh); nothing else needs to.
#
# Prints three lines:
#   version=<n>      the bundle's manifest version
#   key=<seed file>  the seed to sign with
#   mode=release|dev
#
# The key is the home app's release key, at
# ${CLAX_HOME_BUNDLE_KEY:-${XDG_CONFIG_HOME:-~/.config}/clax/home-bundle/key}.
# Beside it, `releases` is the ledger of the versions this machine gave: one
# line per version, `<version> <digest>`, the digest being SHA-256 over every
# file under home/ and assets/kit/ (dot-named paths aside, as the packer
# skips them). The version is the last line's when the sources still have
# its digest; else one above the greater of that line's version and
# home/manifest.json's `version` (the last version released before the
# ledger), and `record` appends it. So a build whose sources changed since
# the last one carries a higher version than every earlier build on this
# machine, and a rebuild of the same sources carries the same version.
# `peek` says what `record` would give and writes nothing.
#
# With no release key and no ledger (a machine that never released, such
# as CI) the bundle is signed by the development key, tool/home-dev.key,
# whose seed is public, on the development lineage (home/manifest.json's
# `dev_lineage`), at the manifest's version plus one: such a bundle is a
# different app to an install that has the released one. With a ledger
# and no key, it stops: that machine's releases would continue under
# another key, a foreign app to every install.
set -euo pipefail

app="${1:?usage: home-version.sh APP peek|record}"
mode="${2:?usage: home-version.sh APP peek|record}"
case "$mode" in peek|record) ;; *) echo "usage: home-version.sh APP peek|record" >&2; exit 2 ;; esac
app="$(cd "$app" && pwd)"
key="${CLAX_HOME_BUNDLE_KEY:-${XDG_CONFIG_HOME:-$HOME/.config}/clax/home-bundle/key}"
ledger="$(dirname "$key")/releases"

# The manifest's `version`: one `"version": <n>` member (home/manifest.json
# keeps one member per line).
floor="$(sed -n 's/^ *"version": *\([0-9][0-9]*\),\{0,1\} *$/\1/p' "$app/home/manifest.json")"
case "$floor" in
    ''|*[!0-9]*) echo "home-version.sh: home/manifest.json has no \"version\": <n> line" >&2; exit 1 ;;
esac

if [ ! -f "$key" ]; then
    if [ -f "$ledger" ]; then
        echo "home-version.sh: this machine released the home app (its ledger is $ledger), and the release key is not at $key; restore it, or every later release is a foreign app to every install" >&2
        exit 1
    fi
    echo "version=$((floor + 1))"
    echo "key=$app/tool/home-dev.key"
    echo "mode=dev"
    exit 0
fi

if command -v shasum > /dev/null; then sha() { shasum -a 256; }
elif command -v sha256sum > /dev/null; then sha() { sha256sum; }
else echo "home-version.sh: no shasum or sha256sum" >&2; exit 1
fi
digest="$(
    cd "$app"
    find home assets/kit -type f ! -path '*/.*' | LC_ALL=C sort | while IFS= read -r f; do
        printf '%s %s\n' "$(sha < "$f" | cut -d' ' -f1)" "$f"
    done | sha | cut -d' ' -f1
)"

# version_for: the version for $digest from the ledger's last line.
version_for() {
    local last n d
    last="$( [ -f "$ledger" ] && tail -n 1 "$ledger" || true)"
    n="${last%% *}" d="${last#* }"
    case "$n" in ''|*[!0-9]*) n=0 d="" ;; esac
    if [ "$d" = "$digest" ] && [ "$n" -gt "$floor" ]; then echo "$n"
    elif [ "$n" -gt "$floor" ]; then echo $((n + 1))
    else echo $((floor + 1))
    fi
}

if [ "$mode" = peek ]; then
    version="$(version_for)"
else
    # One writer at a time: builds in two checkouts may record at once.
    lock="$ledger.lock"
    mkdir -p "$(dirname "$ledger")"
    tries=0
    until mkdir "$lock" 2> /dev/null; do
        tries=$((tries + 1))
        if [ "$tries" -ge 300 ]; then
            echo "home-version.sh: $lock has been held for 30 s; remove it if no build is running" >&2
            exit 1
        fi
        sleep 0.1
    done
    trap 'rmdir "$lock"' EXIT
    version="$(version_for)"
    last="$( [ -f "$ledger" ] && tail -n 1 "$ledger" || true)"
    if [ "$last" != "$version $digest" ]; then
        printf '%s %s\n' "$version" "$digest" >> "$ledger"
    fi
fi
echo "version=$version"
echo "key=$key"
echo "mode=release"
