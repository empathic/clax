#!/usr/bin/env bash
# apk-check.sh APK AAPT2 APKSIGNER: checks the Bazel-built APK against what
# the app's Gradle build makes (apps/clax_app/android/app/build.gradle.kts):
# its package and version, SDK levels, launcher activity, a release build
# (not debuggable) that extracts its native libraries, the engine, the app's
# AOT code and the embedded clax for both CPUs, and a debug-key signature.
set -euo pipefail
apk="$1" aapt2="$2" apksigner="$3"
fails=0
ok() { echo "PASS: $1"; }
bad() { echo "FAIL: $1"; fails=1; }
check() { # check DESCRIPTION OUTPUT LINE: OUTPUT has LINE as a whole line
    if grep -qxF -- "$3" <<<"$2"; then ok "$1"; else bad "$1 (no line: $3)"; fi
}
badging="$("$aapt2" dump badging "$apk")"
check "package and version" "$badging" "package: name='dev.empathic.clax' versionCode='$VERSION_CODE' versionName='$VERSION_NAME' platformBuildVersionName='16' platformBuildVersionCode='36' compileSdkVersion='36' compileSdkVersionCodename='16'"
check "minimum SDK" "$badging" "minSdkVersion:'29'"
check "target SDK" "$badging" "targetSdkVersion:'36'"
check "launcher activity, in the app's namespace" "$badging" "launchable-activity: name='dev.empathic.clax_app.MainActivity'  label='' icon=''"
check "the embedded daemon's network permission" "$badging" "uses-permission: name='android.permission.INTERNET'"
check "both native ABIs" "$badging" "native-code: 'arm64-v8a' 'x86_64'"
if grep -q '^application-debuggable' <<<"$badging"; then bad "a release build is not debuggable"; else ok "a release build is not debuggable"; fi
manifest="$("$aapt2" dump xmltree --file AndroidManifest.xml "$apk")"
if grep -q 'extractNativeLibs.*=true' <<<"$manifest"; then ok "native libraries are extracted"; else bad "native libraries are extracted"; fi
files="$(unzip -l "$apk" | awk '{print $4}')"
for abi in arm64-v8a x86_64; do
    for lib in libapp.so libflutter.so libclax.so; do
        check "lib/$abi/$lib" "$files" "lib/$abi/$lib"
    done
done
check "Flutter's assets" "$files" "assets/flutter_assets/assets/home/home.clax"
certs="$("$apksigner" verify --print-certs "$apk")"
if grep -q 'certificate DN: C=US, O=Android, CN=Android Debug' <<<"$certs"; then ok "signed with a debug key"; else bad "signed with a debug key: $certs"; fi
exit "$fails"
