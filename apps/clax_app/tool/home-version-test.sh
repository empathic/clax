#!/usr/bin/env bash
# home-version-test.sh SCRIPT: checks tool/home-version.sh against an app
# directory of its own and a key in a scratch directory: the same sources
# keep their version, changed ones (a change undone included) take a
# higher one, the manifest's version is a floor, `peek` records nothing,
# and a ledger without its key stops.
set -euo pipefail
script="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
tmp="$(mktemp -d "${TEST_TMPDIR:-${TMPDIR:-/tmp}}/home-version.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT
fails=0
ok() { echo "PASS: $1"; }
bad() { echo "FAIL: $1"; fails=1; }
expect() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: got '$2', want '$3'"; fi; }

app="$(cd "$tmp" && pwd)/app"
mkdir -p "$app/home" "$app/assets/kit" "$app/tool"
manifest() {
    printf '{\n  "title": "Home",\n  "version": %s,\n  "release": "1.5"\n}\n' "$1" > "$app/home/manifest.json"
}
manifest 11
echo one > "$app/home/index.html"
echo kit > "$app/assets/kit/kit.js"
echo qual > "$app/home/.qual"
export CLAX_HOME_BUNDLE_KEY="$tmp/keys/key"
ledger="$tmp/keys/releases"
field() { sed -n "s/^$1=//p"; }
version() { bash "$script" "$app" "${1:-record}" | field version; }

out="$(bash "$script" "$app" record)"
expect "no key and no ledger: the development key" "$(field mode <<<"$out")" dev
expect "the development key's seed" "$(field key <<<"$out")" "$app/tool/home-dev.key"
expect "the development version" "$(field version <<<"$out")" 12
[ -e "$ledger" ] && bad "a development build records nothing" || ok "a development build records nothing"

mkdir -p "$tmp/keys"
echo seed > "$CLAX_HOME_BUNDLE_KEY"
expect "peek, with no ledger yet" "$(version peek)" 12
[ -e "$ledger" ] && bad "peek records nothing" || ok "peek records nothing"
out="$(bash "$script" "$app" record)"
expect "a release" "$(field mode <<<"$out")" release
expect "the release key" "$(field key <<<"$out")" "$CLAX_HOME_BUNDLE_KEY"
expect "the first release is above the manifest's version" "$(field version <<<"$out")" 12
expect "the same sources keep it" "$(version)" 12
expect "the ledger has one line" "$(wc -l < "$ledger" | tr -d ' ')" 1
echo qual2 > "$app/home/.qual"
expect "a dot-named file is not a source" "$(version)" 12

echo two > "$app/home/index.html"
expect "peek a change" "$(version peek)" 13
expect "a change takes the next version" "$(version)" 13
echo kit2 > "$app/assets/kit/kit.js"
expect "a kit change takes the next version" "$(version)" 14
echo one > "$app/home/index.html"
echo kit > "$app/assets/kit/kit.js"
expect "a change undone still takes a higher version" "$(version)" 15
expect "and keeps it" "$(version)" 15

manifest 20
expect "a raised manifest version is a floor" "$(version)" 21
expect "the ledger has a line per version" "$(cut -d' ' -f1 "$ledger" | tr '\n' ' ')" "12 13 14 15 21 "

rm "$CLAX_HOME_BUNDLE_KEY"
if err="$(bash "$script" "$app" record 2>&1)"; then
    bad "a ledger without its key stops"
else
    case "$err" in *"release key is not at"*) ok "a ledger without its key stops" ;; *) bad "a ledger without its key stops: $err" ;; esac
fi
exit "$fails"
