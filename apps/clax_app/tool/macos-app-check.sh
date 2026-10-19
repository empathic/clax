#!/usr/bin/env bash
# macos-app-check.sh ZIP HOME: checks the Bazel-built macOS app (a zip of
# Clax.app) against what `flutter build macos` makes: its bundle ID, name,
# copyright, version and executable, the app menu's About, Hide and Quit
# items named Clax, Flutter's frameworks, the sandbox entitlements of
# Runner/Release.entitlements, and clax in Contents/Resources signed as a
# sandboxed helper (Runner/ClaxHelper.entitlements), and the home bundle
# HOME (the one //apps/clax_app:home_bundle built) and the third-party
# notices in Flutter's assets.
set -euo pipefail
zip="$1" home="$2"
fails=0
ok() { echo "PASS: $1"; }
bad() { echo "FAIL: $1"; fails=1; }
dir="$(mktemp -d "${TEST_TMPDIR:-${TMPDIR:-/tmp}}/macos-app.XXXXXX")"
trap 'rm -rf "$dir"' EXIT
unzip -q "$zip" -d "$dir"
app="$dir/Clax.app"
[ -d "$app" ] || { echo "FAIL: no Clax.app in $zip"; exit 1; }
# The third-party notices the licences page lists (NOTICES.Z, from
# //apps/clax_app:notices): more than 500 kB, with entries for pub packages,
# the engine's components and the embedded clax's Rust crates.
check_notices() { # check_notices NOTICES.Z
    local text="$1.txt" n
    if ! gunzip -c "$1" > "$text"; then bad "NOTICES.Z is gzip"; return; fi
    n=$(( $(wc -c < "$text") ))
    if [ "$n" -gt 500000 ]; then ok "NOTICES holds the third-party notices ($n bytes)"; else bad "NOTICES holds the third-party notices (only $n bytes)"; fi
    for p in webview_flutter archive flutter skia ring tokio rusqlite; do
        if grep -qxF "$p" "$text"; then ok "NOTICES names $p"; else bad "NOTICES names $p"; fi
    done
}
plist() { /usr/libexec/PlistBuddy -c "Print :$1" "$app/Contents/Info.plist" 2>/dev/null || true; }
if [ "$(plist CFBundleIdentifier)" = dev.empathic.clax ]; then ok "bundle ID"; else bad "bundle ID: $(plist CFBundleIdentifier)"; fi
if [ "$(plist CFBundleName)" = Clax ]; then ok "bundle name"; else bad "bundle name: $(plist CFBundleName)"; fi
copyright="Copyright © 2026 Empathic, Inc."
if [ "$(plist NSHumanReadableCopyright)" = "$copyright" ]; then ok "copyright"; else bad "copyright: $(plist NSHumanReadableCopyright)"; fi
if [ "$(plist CFBundleShortVersionString)" = "$VERSION_NAME" ]; then ok "version name"; else bad "version name: $(plist CFBundleShortVersionString)"; fi
if [ "$(plist CFBundleVersion)" = "$VERSION_CODE" ]; then ok "build number"; else bad "build number: $(plist CFBundleVersion)"; fi
if [ -x "$app/Contents/MacOS/$(plist CFBundleExecutable)" ]; then ok "executable"; else bad "executable"; fi
nib="$app/Contents/Resources/Base.lproj/MainMenu.nib"
[ -d "$nib" ] && nib="$nib/keyedobjects.nib"
for t in "About Clax" "Hide Clax" "Quit Clax" "showAbout:"; do
    if [ -f "$nib" ] && grep -aq "$t" "$nib"; then ok "the menu has $t"; else bad "the menu has $t"; fi
done
if [ -f "$nib" ] && ! grep -aq APP_NAME "$nib"; then ok "the menu names no APP_NAME"; else bad "the menu names no APP_NAME"; fi
for f in Frameworks/FlutterMacOS.framework/FlutterMacOS Frameworks/App.framework/App; do
    if [ -e "$app/Contents/$f" ]; then ok "$f"; else bad "$f"; fi
done
assets="$app/Contents/Frameworks/App.framework/Resources/flutter_assets/assets/home"
if cmp -s "$assets/home.clax" "$home"; then ok "the home bundle built from the sources"; else bad "the home bundle built from the sources ($assets/home.clax and $home)"; fi
if [ -f "$assets/release.json" ]; then ok "the home bundle's release record"; else bad "the home bundle's release record"; fi
cp "$app/Contents/Frameworks/App.framework/Resources/flutter_assets/NOTICES.Z" "$dir/NOTICES.Z"
check_notices "$dir/NOTICES.Z"
ents="$(/usr/bin/codesign -d --entitlements - --xml "$app" 2>/dev/null || true)"
for k in com.apple.security.app-sandbox com.apple.security.network.client com.apple.security.network.server; do
    if grep -q "$k" <<<"$ents"; then ok "app entitlement $k"; else bad "app entitlement $k"; fi
done
helper="$app/Contents/Resources/clax"
if [ -x "$helper" ]; then ok "clax in Resources"; else bad "clax in Resources"; fi
hents="$(/usr/bin/codesign -d --entitlements - --xml "$helper" 2>/dev/null || true)"
for k in com.apple.security.app-sandbox com.apple.security.inherit; do
    if grep -q "$k" <<<"$hents"; then ok "clax entitlement $k"; else bad "clax entitlement $k"; fi
done
if /usr/bin/codesign --verify --deep --strict "$app"; then ok "signature verifies"; else bad "signature verifies"; fi
exit "$fails"
