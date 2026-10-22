#!/usr/bin/env bash
# apk-check-test.sh APK_CHECK: checks apk-check.sh's reading of MainActivity's
# configChanges from `aapt2 dump xmltree` (--manifest mode): the release
# manifest's value passes, and one without screenSize fails, naming it.
set -euo pipefail
check="$1"
dir="$(mktemp -d "${TEST_TMPDIR:-${TMPDIR:-/tmp}}/apk-check.XXXXXX")"
trap 'rm -rf "$dir"' EXIT
fails=0

# An `aapt2 dump xmltree --file AndroidManifest.xml` of the APK, cut to the
# activity, with MainActivity's configChanges as the flags value $1.
dump() {
    cat <<EOF
N: android=http://schemas.android.com/apk/res/android (line=2)
  E: manifest (line=2)
      E: application (line=73)
          E: activity (line=80)
            A: http://schemas.android.com/apk/res/android:name(0x01010003)="io.flutter.plugins.other.Activity" (Raw: "io.flutter.plugins.other.Activity")
            A: http://schemas.android.com/apk/res/android:configChanges(0x0101001f)=0x00000000
          E: activity (line=86)
            A: http://schemas.android.com/apk/res/android:theme(0x01010000)=@0x7f0c0007
            A: http://schemas.android.com/apk/res/android:name(0x01010003)="dev.empathic.clax_app.MainActivity" (Raw: "dev.empathic.clax_app.MainActivity")
            A: http://schemas.android.com/apk/res/android:exported(0x01010010)=true
            A: http://schemas.android.com/apk/res/android:configChanges(0x0101001f)=$1
            A: http://schemas.android.com/apk/res/android:windowSoftInputMode(0x0101022b)=0x00000010
EOF
}

# The release APK's value: orientation, keyboardHidden, keyboard,
# screenSize, smallestScreenSize, locale, layoutDirection, fontScale,
# screenLayout, density and uiMode.
dump 0x40003fb4 > "$dir/kept.txt"
# The same less screenSize (0x400).
dump 0x40003bb4 > "$dir/no-screen-size.txt"

if out="$(bash "$check" --manifest "$dir/kept.txt")"; then
    echo "PASS: the release manifest's configChanges pass"
else
    echo "FAIL: the release manifest's configChanges pass: $out"; fails=1
fi
if out="$(bash "$check" --manifest "$dir/no-screen-size.txt")"; then
    echo "FAIL: a manifest without screenSize fails"; fails=1
elif grep -q 'screenSize' <<<"$out"; then
    echo "PASS: a manifest without screenSize fails, naming it"
else
    echo "FAIL: a manifest without screenSize fails, naming it: $out"; fails=1
fi
exit "$fails"
