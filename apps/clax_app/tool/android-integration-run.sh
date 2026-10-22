#!/usr/bin/env bash
# android-integration-run.sh APK EMULATOR_SCRIPT EMULATOR SYSTEM_IMAGE ADB:
# runs an Android integration test app (a debug APK built by
# //apps/clax_app:integration_android_<name>, whose entry point runs one
# integration_test/android_<name>_test.dart and logs its result) on the
# emulator "Clax Pixel", and exits 0 only when the test logs
# CLAX_INTEGRATION_RESULT=0.
#
# EMULATOR_SCRIPT is scripts/app-android-emulator.sh; EMULATOR, SYSTEM_IMAGE
# and ADB are the pinned emulator, a file of the system image and adb, from
# the test's runfiles. It starts the emulator by the script, uninstalls the
# app and installs the APK, launches it, reads the log (tag flutter) for at
# most 10 minutes for the result, copies a screenshot for each
# CLAX_SHOT=<name> line to $TEST_UNDECLARED_OUTPUTS_DIR/<name>.png, and
# stops the emulator by its PID when it exits. Every adb call names the
# emulator's serial, on the emulator's own adb server.
set -euo pipefail

[ $# -eq 5 ] || { echo "usage: $0 APK EMULATOR_SCRIPT EMULATOR SYSTEM_IMAGE ADB" >&2; exit 2; }
abs() { echo "$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"; }
apk="$(abs "$1")"
emulator_script="$(abs "$2")"
export CLAX_ANDROID_EMULATOR="$(abs "$3")"
export CLAX_ANDROID_SYSTEM_IMAGE="$(abs "$4")"
export CLAX_ANDROID_ADB="$(abs "$5")"
adb="$CLAX_ANDROID_ADB"
pkg=dev.empathic.clax
out="${TEST_UNDECLARED_OUTPUTS_DIR:-$(mktemp -d)}"
mkdir -p "$out"

started="$("$emulator_script" --start --i-know-what-i-am-doing | tail -1)"
case "$started" in
    serial=emulator-*" pid="*) ;;
    *) echo "android-integration-run: the emulator did not start: $started" >&2; exit 1 ;;
esac
serial="${started#serial=}"
serial="${serial%% *}"
pid="${started##*pid=}"
export ANDROID_ADB_SERVER_PORT=$((${serial#emulator-} + 10000))

logcat_pid=""
stop() {
    [ -z "$logcat_pid" ] || kill "$logcat_pid" 2>/dev/null || true
    "$emulator_script" --stop "$pid" --i-know-what-i-am-doing >&2 || true
}
trap stop EXIT

echo "android-integration-run: installing on $serial"
"$adb" -s "$serial" uninstall "$pkg" > /dev/null 2>&1 || true
"$adb" -s "$serial" install -r -t "$apk"
"$adb" -s "$serial" logcat -c
log="$out/logcat.txt"
"$adb" -s "$serial" logcat -v raw -s flutter:V > "$log" 2>&1 &
logcat_pid=$!
"$adb" -s "$serial" shell am start -W -n "$pkg/dev.empathic.clax_app.MainActivity"

# The result, polled every second for at most 10 minutes; a screenshot for
# each CLAX_SHOT line as it appears.
deadline=$((SECONDS + 600))
shots=0
result=""
while [ "$SECONDS" -lt "$deadline" ]; do
    while IFS= read -r name; do
        "$adb" -s "$serial" exec-out screencap -p > "$out/$name.png"
        echo "android-integration-run: screenshot $name.png"
        shots=$((shots + 1))
    done < <(grep -o 'CLAX_SHOT=[A-Za-z0-9_.-]*' "$log" | sed 's/^CLAX_SHOT=//' | tail -n +$((shots + 1)))
    result="$(grep -o 'CLAX_INTEGRATION_RESULT=[01]' "$log" | sed -n '1s/.*=//p' || true)"
    [ -z "$result" ] || break
    if ! "$adb" -s "$serial" shell pidof "$pkg" > /dev/null 2>&1 && [ "$SECONDS" -gt 30 ] &&
        ! grep -q 'CLAX_INTEGRATION_RESULT' "$log"; then
        # The app is gone without a result: its crash is in the log.
        sleep 1
        result="$(grep -o 'CLAX_INTEGRATION_RESULT=[01]' "$log" | sed -n '1s/.*=//p' || true)"
        [ -n "$result" ] || { echo "android-integration-run: the app exited without a result" >&2; break; }
    fi
    sleep 1
done
cat "$log"
if [ -z "$result" ]; then
    "$adb" -s "$serial" logcat -d -t 200 > "$out/logcat-all.txt" 2>&1 || true
    echo "android-integration-run: no CLAX_INTEGRATION_RESULT (the last of the log: $out/logcat-all.txt)" >&2
    exit 1
fi
[ "$result" = 0 ]
