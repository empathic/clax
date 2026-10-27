#!/usr/bin/env bash
# apk-check.sh APK AAPT2 APKSIGNER HOME NM: checks the Bazel-built APK against what
# the app's Gradle build makes (apps/clax_app/android/app/build.gradle.kts):
# its package and version, SDK levels, launcher activity, the adaptive
# launcher icon with its monochrome layer, the splash theme, predictive back
# (the application's enableOnBackInvokedCallback), a release build
# (not debuggable, shrunk by R8) whose manifest asks nothing about
# extracting native libraries (the core is a library the app loads, not an
# executable it runs) and sets no network security config (no cleartext
# HTTP: the WebViews load from the app's scheme), the engine, the app's AOT code and the embedded core
# (libclax_embed.so, exporting the four clax_embed_ functions, read with NM,
# the NDK's llvm-nm) for both CPUs and no clax executable (libclax.so), each
# native library's size printed, every native library in the APK (the
# engine's and plugins' as well as ours) with each PT_LOAD segment aligned
# to at least 16 KB, read with the NDK's llvm-readelf beside NM (a phone
# with 16 KB pages loads no library aligned less, and Google Play takes no
# such app for SDK 35 and later), the home bundle HOME (the
# one //apps/clax_app:home_bundle built), the third-party notices, and a
# debug-key signature, MainActivity's configChanges keeping the
# activity through a resize (a Chromebook's window, a foldable unfolding),
# backup rules (android:dataExtractionRules, android:fullBackupContent)
# that leave out the core's run files and flutter_secure_storage's
# preferences, and Get in Clax's intent filters (clax: links and verified
# app links to clax.dev) with Flutter's deep linking off.
#
# apk-check.sh --manifest DUMP checks only the configChanges, in DUMP, an
# `aapt2 dump xmltree --file AndroidManifest.xml` output.
set -euo pipefail
fails=0
ok() { echo "PASS: $1"; }
bad() { echo "FAIL: $1"; fails=1; }
# check_config_changes MANIFEST: MainActivity's android:configChanges (aapt2
# prints the flags' value) includes screenSize, smallestScreenSize,
# screenLayout, orientation and density (AndroidManifest.xml), so a
# resize never restarts the activity.
check_config_changes() {
    local value missing="" flag bit
    value="$(awk '/E: activity/ { c = "" } /android:configChanges/ { c = $0 } /android:name.*"dev.empathic.clax_app.MainActivity"/ { n = 1 } n && c != "" { sub(/.*=/, "", c); print c; exit } /E: activity/ { n = 0 }' <<<"$1")"
    if [ -z "$value" ]; then bad "MainActivity's configChanges keep it through a resize (none found)"; return; fi
    for flag in screenSize:0x400 smallestScreenSize:0x800 screenLayout:0x100 orientation:0x80 density:0x1000; do
        bit="${flag#*:}"
        if (( (value & bit) == 0 )); then missing="$missing ${flag%%:*}"; fi
    done
    if [ -z "$missing" ]; then
        ok "MainActivity's configChanges keep it through a resize ($value)"
    else
        bad "MainActivity's configChanges keep it through a resize ($value lacks$missing)"
    fi
}
if [ "${1:-}" = "--manifest" ]; then
    check_config_changes "$(cat "$2")"
    exit "$fails"
fi
apk="$1" aapt2="$2" apksigner="$3" home="$4" nm="$5"
check() { # check DESCRIPTION OUTPUT LINE: OUTPUT has LINE as a whole line
    if grep -qxF -- "$3" <<<"$2"; then ok "$1"; else bad "$1 (no line: $3)"; fi
}
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
badging="$("$aapt2" dump badging "$apk")"
check "package and version" "$badging" "package: name='dev.empathic.clax' versionCode='$VERSION_CODE' versionName='$VERSION_NAME' platformBuildVersionName='16' platformBuildVersionCode='36' compileSdkVersion='36' compileSdkVersionCodename='16'"
check "minimum SDK" "$badging" "minSdkVersion:'29'"
check "target SDK" "$badging" "targetSdkVersion:'36'"
check "launcher activity, in the app's namespace" "$badging" "launchable-activity: name='dev.empathic.clax_app.MainActivity'  label='' icon=''"
check "the adaptive icon, as the app's icon" "$badging" "application: label='Clax' icon='res/mipmap-anydpi-v26/ic_launcher.xml'"
for icon in ic_launcher ic_launcher_round; do
    layers="$("$aapt2" dump xmltree --file "res/mipmap-anydpi-v26/$icon.xml" "$apk" | sed -n 's/^ *E: \([a-z-]*\).*/\1/p' | tr '\n' ' ')"
    if [ "$layers" = "adaptive-icon background foreground monochrome " ]; then
        ok "$icon is adaptive with a monochrome layer"
    else
        bad "$icon is adaptive with a monochrome layer (elements: $layers)"
    fi
done
# The splash screen (res/values/styles.xml): MainActivity opens on
# LaunchTheme, Theme.Clax.Starting's alias, the mark on the brand's `bg`.
resources="$("$aapt2" dump resources "$apk")"
# The manifest is read whole first: awk leaving early would break aapt2's
# pipe, and pipefail would end the script.
manifest="$("$aapt2" dump xmltree --file AndroidManifest.xml "$apk")"
activity_theme="$(awk '/E: activity/ { t = "" } /android:theme/ { t = $0 } /android:name.*"dev.empathic.clax_app.MainActivity"/ { sub(/.*=@/, "", t); print t; exit }' <<<"$manifest")"
launch_id="$(sed -n 's/^ *resource \(0x[0-9a-f]*\) style\/LaunchTheme$/\1/p' <<<"$resources")"
if [ -n "$launch_id" ] && [ "$activity_theme" = "$launch_id" ]; then ok "MainActivity opens on LaunchTheme"; else bad "MainActivity opens on LaunchTheme (theme $activity_theme, LaunchTheme ${launch_id:-missing})"; fi
if sed -n '/ style\/LaunchTheme$/{n;p;}' <<<"$resources" | grep 'parent=style/Theme.Clax.Starting ' >/dev/null; then
    ok "LaunchTheme is the splash theme"
else
    bad "LaunchTheme is the splash theme (its parent is not Theme.Clax.Starting)"
fi
splash="$(sed -n '/ style\/Theme.Clax.Starting$/,/ resource /p' <<<"$resources")"
for item in "postSplashScreenTheme(.*)=@style/Theme.Clax" "windowSplashScreenAnimatedIcon(.*)=@drawable/splash_mark" "windowSplashScreenBackground(.*)=@color/clax_bg"; do
    if grep -q -- "$item\$" <<<"$splash"; then ok "Theme.Clax.Starting: ${item%%(*}"; else bad "Theme.Clax.Starting: ${item%%(*} (no $item)"; fi
done
check "the network permission (the Claude API)" "$badging" "uses-permission: name='android.permission.INTERNET'"
check "both native ABIs" "$badging" "native-code: 'arm64-v8a' 'x86_64'"
if grep -q '^application-debuggable' <<<"$badging"; then bad "a release build is not debuggable"; else ok "a release build is not debuggable"; fi
if grep -q 'extractNativeLibs' <<<"$manifest"; then bad "the manifest leaves extractNativeLibs to the platform (it sets it)"; else ok "the manifest leaves extractNativeLibs to the platform"; fi
if grep -q 'networkSecurityConfig' <<<"$manifest"; then bad "the manifest sets no network security config (it sets one)"; else ok "the manifest sets no network security config"; fi
check_config_changes "$manifest"
# Backup and device transfer leave out the core's run files and
# flutter_secure_storage's preferences (res/xml/data_extraction_rules.xml
# for Android 12 and later, res/xml/backup_rules.xml before): the
# application names each, and each excludes them.
for rule in dataExtractionRules:data_extraction_rules fullBackupContent:backup_rules; do
    attr="${rule%%:*}" name="${rule#*:}"
    id="$(sed -n "s/.*android:$attr([^)]*)=@\(0x[0-9a-f]*\).*/\1/p" <<<"$manifest" | sed -n 1p)"
    if [ -z "$id" ] || ! grep -qE "^ *resource $id xml/$name\$" <<<"$resources"; then
        bad "the application's android:$attr is @xml/$name (it is ${id:-unset})"
        continue
    fi
    path="$(sed -n "/^ *resource $id xml\/$name\$/,/ resource /p" <<<"$resources" | sed -n 's/.*(file) \([^ ]*\).*/\1/p' | sed -n 1p)"
    rules="$("$aapt2" dump xmltree --file "$path" "$apk" 2>&1 || true)"
    missing=""
    for p in clax-home/core.lock clax-home/embed.json clax-home/logs clax-home/net-cache FlutterSecureStorage.xml FlutterSecureKeyStorage.xml; do
        grep -qF "\"$p\"" <<<"$rules" || missing="$missing $p"
    done
    if [ -z "$missing" ]; then
        ok "the application's android:$attr is @xml/$name, which excludes the run files and the secure storage's preferences"
    else
        bad "@xml/$name ($path) excludes the run files and the secure storage's preferences (missing:$missing)"
    fi
done
# Predictive back: Android 13+ shows the back-to-home and in-app back
# animations only for an application that opts in.
if grep -q 'enableOnBackInvokedCallback.*=true' <<<"$manifest"; then ok "predictive back is on"; else bad "predictive back is on (no android:enableOnBackInvokedCallback=\"true\")"; fi
# Get in Clax: BundleReceiverActivity takes clax: links and verified app
# links to https://clax.dev/a/<id>/get, and MainActivity pushes no route
# Flutter would take from an intent's data. $receiver is
# BundleReceiverActivity's element and its children, up to the next
# component.
receiver="$(awk '/E: (activity|receiver|service|provider)( |$)/ { if (keep) printf "%s", buf; buf = ""; keep = 0 } { buf = buf $0 "\n" } /"dev.empathic.clax_app.BundleReceiverActivity"/ { keep = 1 } END { if (keep) printf "%s", buf }' <<<"$manifest")"
for t in 'scheme="clax"' 'scheme="https"' 'host="clax.dev"' 'pathPattern="/a/............/get"' 'autoVerify=true'; do
    # aapt2 prints each attribute with its resource ID: android:host(0x…)=…
    if grep -qF "android:${t%%=*}(" <<<"$(grep -F "=${t#*=}" <<<"$receiver")"; then ok "BundleReceiverActivity's filters have $t"; else bad "BundleReceiverActivity's filters have $t"; fi
done
if awk '/flutter_deeplinking_enabled/ { f = 1; next } f && /android:value/ { print; exit }' <<<"$manifest" | grep '=false' >/dev/null; then ok "Flutter's deep linking is off"; else bad "Flutter's deep linking is off"; fi
files="$(unzip -l "$apk" | awk '{print $4}')"
for abi in arm64-v8a x86_64; do
    for lib in libapp.so libflutter.so libclax_embed.so; do
        check "lib/$abi/$lib" "$files" "lib/$abi/$lib"
    done
    if grep -qxF "lib/$abi/libclax.so" <<<"$files"; then bad "no clax executable in lib/$abi (libclax.so is there)"; else ok "no clax executable in lib/$abi"; fi
done
# Each native library's size, as the APK holds it (uncompressed).
unzip -lv "$apk" 'lib/*' | awk '$NF ~ /^lib\// { printf "SIZE: %s %d bytes (%d stored)\n", $NF, $1, $3 }'
# The core exports exactly the four C functions (include/clax_embed.h).
want_syms="clax_embed_call clax_embed_close clax_embed_open clax_embed_subscribe"
libs="$(mktemp -d "${TEST_TMPDIR:-${TMPDIR:-/tmp}}/apk-libs.XXXXXX")"
for abi in arm64-v8a x86_64; do
    so="$libs/$abi-libclax_embed.so"
    if unzip -p "$apk" "lib/$abi/libclax_embed.so" > "$so" 2>/dev/null && [ -s "$so" ]; then
        got="$("$nm" -D --defined-only "$so" | awk '$2 == "T" && $3 ~ /^clax_embed_/ { print $3 }' | sort | paste -sd ' ' -)"
        if [ "$got" = "$want_syms" ]; then ok "lib/$abi/libclax_embed.so exports the four clax_embed_ functions"; else bad "lib/$abi/libclax_embed.so exports the four clax_embed_ functions (it exports: $got)"; fi
    else
        bad "lib/$abi/libclax_embed.so exports the four clax_embed_ functions (no library)"
    fi
done
# Every native library's PT_LOAD segments are aligned to 16 KB or more.
readelf="$(dirname "$nm")/llvm-readelf"
for path in $(grep -E '^lib/[^/]+/[^/]+\.so$' <<<"$files"); do
    so="$libs/align.so"
    if ! unzip -p "$apk" "$path" > "$so" 2>/dev/null; then bad "$path: aligned to 16 KB (unreadable)"; continue; fi
    headers="$("$readelf" -lW "$so")"
    aligns="$(awk '$1 == "LOAD" { print $NF }' <<<"$headers" | paste -sd ' ' -)"
    low=""
    for a in $aligns; do if (( a < 0x4000 )); then low="$low $a"; fi; done
    if [ -n "$aligns" ] && [ -z "$low" ]; then
        ok "$path: every PT_LOAD aligned to at least 16 KB ($aligns)"
    else
        bad "$path: every PT_LOAD aligned to at least 16 KB (PT_LOAD alignments: ${aligns:-none})"
    fi
done
rm -rf "$libs"
check "Flutter's assets" "$files" "assets/flutter_assets/assets/home/home.clax"
check "the home bundle's release record" "$files" "assets/flutter_assets/assets/home/release.json"
if unzip -p "$apk" assets/flutter_assets/assets/home/home.clax | cmp -s - "$home"; then
    ok "the home bundle built from the sources"
else
    bad "the home bundle built from the sources (assets/home/home.clax differs from $home)"
fi
# Shrunk by R8, as Gradle's release build is: one dex of about 1.2 MB.
dexes="$(grep -c '^classes[0-9]*\.dex$' <<<"$files" || true)"
dex_bytes="$(unzip -l "$apk" classes.dex | awk '$4 == "classes.dex" { print $1 }')"
if [ "$dexes" = 1 ] && [ "${dex_bytes:-0}" -gt 0 ] && [ "$dex_bytes" -lt 2000000 ]; then
    ok "one dex, shrunk ($dex_bytes bytes)"
else
    bad "one dex under 2 MB (found $dexes, classes.dex $dex_bytes bytes)"
fi
notices="$(mktemp -d "${TEST_TMPDIR:-${TMPDIR:-/tmp}}/notices.XXXXXX")"
trap 'rm -rf "$notices"' EXIT
unzip -p "$apk" assets/flutter_assets/NOTICES.Z > "$notices/NOTICES.Z"
check_notices "$notices/NOTICES.Z"
# A debug key's DN is CN=Android Debug, O=Android, C=US, printed in either
# order: an Android Studio keystore's as C=US first, rules_android's (the
# key on a machine without one, bazel/android/debug_keystore.bzl) as
# CN=Android Debug first.
certs="$("$apksigner" verify --print-certs "$apk")"
dn="$(sed -n 's/^Signer #1 certificate DN: //p' <<<"$certs" | tr ',' '\n' | sed 's/^ *//' | sort | paste -sd, -)"
if [ "$dn" = "C=US,CN=Android Debug,O=Android" ]; then ok "signed with a debug key"; else bad "signed with a debug key: $certs"; fi
exit "$fails"
