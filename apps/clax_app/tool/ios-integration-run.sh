#!/usr/bin/env bash
# ios-integration-run.sh APP_IPA APP_IOS_SH: runs an iOS integration test app
# (rules_apple's .ipa of Clax.app built by //apps/clax_app:integration_ios_<name>,
# whose entry point runs one integration_test/ios_<name>_test.dart and prints
# CLAX_INTEGRATION_RESULT=<0|1>) on the simulator "Clax iPhone", and exits 0
# when the test passed.
#
# The simulator is found by APP_IOS_SH (scripts/app-ios.sh --udid; `just
# app-ios` creates it) and booted when it is not running, then shut down
# again by its UDID at the end. The app is uninstalled first, so each test
# starts from an empty container, installed, and launched with its console
# on this script's. What the test prints asks for what only the simulator's
# host can do:
#   CLAX_SHOT=<name>        a screenshot, $TEST_UNDECLARED_OUTPUTS_DIR/<name>.png
#   CLAX_BACKGROUND=<s>     Settings in front for <s> seconds (screenshot
#                           background-<k>.png at its end), then Clax again
#   CLAX_OPENURL=<path>     `simctl openurl` of file://<path>
# A request is a whole console line. Once it is done its line is appended to tmp/clax-runner.log in the
# app's data container, which the test waits for. The console is read for
# at most 10 minutes.
set -euo pipefail
ipa="$1" app_ios="$2"
bundle_id=dev.empathic.clax
out="${TEST_UNDECLARED_OUTPUTS_DIR:-${TEST_TMPDIR:-${TMPDIR:-/tmp}}}"
mkdir -p "$out"
dir="$(mktemp -d "${TEST_TMPDIR:-${TMPDIR:-/tmp}}/ios-integration.XXXXXX")"
console="$dir/console.log"

udid="$("$app_ios" --udid)"
booted=""
console_pid=""
cleanup() {
    status=$?
    if [ -n "$console_pid" ]; then kill "$console_pid" 2>/dev/null || true; fi
    xcrun simctl terminate "$udid" "$bundle_id" >/dev/null 2>&1 || true
    if [ -n "$booted" ]; then
        echo "ios-integration: shutting down the simulator $udid"
        xcrun simctl shutdown "$udid" || true
    fi
    rm -rf "$dir"
    exit "$status"
}
trap cleanup EXIT

if ! xcrun simctl list devices | grep "($udid) (Booted)" >/dev/null; then
    echo "ios-integration: booting the simulator $udid"
    booted=1
fi
xcrun simctl bootstatus "$udid" -b >/dev/null

xcrun simctl uninstall "$udid" "$bundle_id" >/dev/null 2>&1 || true
unzip -q "$ipa" -d "$dir/app"
xcrun simctl install "$udid" "$dir/app/Payload/Clax.app"
data="$(xcrun simctl get_app_container "$udid" "$bundle_id" data)"
mkdir -p "$data/tmp"
done_log="$data/tmp/clax-runner.log"
: > "$done_log"

xcrun simctl launch --console-pty --terminate-running-process "$udid" "$bundle_id" > "$console" 2>&1 &
console_pid=$!

backgrounds=0
result=""
start=$SECONDS
read_lines=0
while [ -z "$result" ]; do
    if [ $((SECONDS - start)) -ge 600 ]; then
        echo "ios-integration: no CLAX_INTEGRATION_RESULT within 10 minutes" >&2
        break
    fi
    total="$(wc -l < "$console" | tr -d ' ')"
    if [ "$total" -le "$read_lines" ]; then
        if ! kill -0 "$console_pid" 2>/dev/null; then
            # The app ended: read what is left once more, then stop.
            total="$(wc -l < "$console" | tr -d ' ')"
            [ "$total" -gt "$read_lines" ] || { echo "ios-integration: the app ended without a result" >&2; break; }
        else
            sleep 0.2
            continue
        fi
    fi
    while IFS= read -r line; do
        line="${line%$'\r'}"
        echo "$line"
        case "$line" in
            CLAX_INTEGRATION_RESULT=*)
                result="${line#CLAX_INTEGRATION_RESULT=}"
                result="${result:0:1}"
                ;;
            CLAX_SHOT=*)
                name="${line#CLAX_SHOT=}"
                xcrun simctl io "$udid" screenshot "$out/$name.png" >/dev/null 2>&1
                echo "CLAX_SHOT=$name" >> "$done_log"
                ;;
            CLAX_BACKGROUND=*)
                secs="${line#CLAX_BACKGROUND=}"
                backgrounds=$((backgrounds + 1))
                xcrun simctl launch "$udid" com.apple.Preferences >/dev/null
                sleep "$secs"
                xcrun simctl io "$udid" screenshot "$out/background-$backgrounds.png" >/dev/null 2>&1
                xcrun simctl launch "$udid" "$bundle_id" >/dev/null
                echo "CLAX_BACKGROUND=$secs" >> "$done_log"
                ;;
            CLAX_OPENURL=*)
                path="${line#CLAX_OPENURL=}"
                xcrun simctl openurl "$udid" "file://$path"
                echo "CLAX_OPENURL=$path" >> "$done_log"
                ;;
        esac
    done < <(sed -n "$((read_lines + 1)),${total}p" "$console")
    read_lines="$total"
done

[ "$result" = 0 ]
