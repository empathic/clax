#!/usr/bin/env bash
# apk-check.sh APK AAPT2 APKSIGNER HOME: checks the Bazel-built APK against what
# the app's Gradle build makes (apps/clax_app/android/app/build.gradle.kts):
# its package and version, SDK levels, launcher activity, the adaptive
# launcher icon with its monochrome layer, the splash theme, predictive back
# (the application's enableOnBackInvokedCallback), a release build
# (not debuggable, shrunk by R8) that extracts its native libraries, the engine, the app's
# AOT code and the embedded clax for both CPUs, the home bundle HOME (the
# one //apps/clax_app:home_bundle built), the third-party notices, and a
# debug-key signature, and MainActivity's configChanges keeping the
# activity through a resize (a Chromebook's window, a foldable unfolding).
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
apk="$1" aapt2="$2" apksigner="$3" home="$4"
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
check "the embedded daemon's network permission" "$badging" "uses-permission: name='android.permission.INTERNET'"
check "both native ABIs" "$badging" "native-code: 'arm64-v8a' 'x86_64'"
if grep -q '^application-debuggable' <<<"$badging"; then bad "a release build is not debuggable"; else ok "a release build is not debuggable"; fi
if grep -q 'extractNativeLibs.*=true' <<<"$manifest"; then ok "native libraries are extracted"; else bad "native libraries are extracted"; fi
check_config_changes "$manifest"
# Predictive back: Android 13+ shows the back-to-home and in-app back
# animations only for an application that opts in.
if grep -q 'enableOnBackInvokedCallback.*=true' <<<"$manifest"; then ok "predictive back is on"; else bad "predictive back is on (no android:enableOnBackInvokedCallback=\"true\")"; fi
files="$(unzip -l "$apk" | awk '{print $4}')"
for abi in arm64-v8a x86_64; do
    for lib in libapp.so libflutter.so libclax.so; do
        check "lib/$abi/$lib" "$files" "lib/$abi/$lib"
    done
done
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
certs="$("$apksigner" verify --print-certs "$apk")"
if grep -q 'certificate DN: C=US, O=Android, CN=Android Debug' <<<"$certs"; then ok "signed with a debug key"; else bad "signed with a debug key: $certs"; fi
exit "$fails"
