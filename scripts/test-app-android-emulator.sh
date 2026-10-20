#!/usr/bin/env bash
# Tests scripts/app-android-emulator.sh on stub tools: the dry runs of
# --start and --stop 123 print the AVD, the emulator's command line and the
# port it would take, and start, write or stop nothing; a busy port moves
# it to the next even one; --start --i-know-what-i-am-doing writes the
# AVD's config.ini (naming the system image's path and abi.type=arm64-v8a)
# and its .ini into a scratch checkout's target/, links the SDK parts,
# boots the stub emulator, waits for the stub adb's boot_completed and
# prints serial and PID; a second --start prints the same emulator; --stop
# refuses a PID it did not start, and stops its own by `adb emu kill`, the
# PID's exit and the adb server's kill-server; every adb call names the
# emulator's serial and an adb server on the emulator's port + 10000.
#
# The stub emulator is a process that runs until the stub adb's `emu kill`
# ends it; no emulator or adb runs.

set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

tmp="$(cd "$(mktemp -d)" && pwd -P)"
cleanup() {
    if [ -f "$tmp/emulator.pid" ]; then kill "$(cat "$tmp/emulator.pid")" 2>/dev/null || true; fi
    rm -rf "$tmp"
}
trap cleanup EXIT

fails=0
_ok() { echo "ok: $1"; }
_fail() {
    echo "FAIL: $1" >&2
    sed 's/^/    | /' "$tmp/out" >&2
    fails=$((fails + 1))
}

repo="$tmp/repo"
mkdir -p "$repo/scripts" "$tmp/bin" "$tmp/sdk/emulator" "$tmp/sdk/image" "$tmp/sdk/platform-tools"
cp scripts/app-android-emulator.sh "$repo/scripts/"
script="$repo/scripts/app-android-emulator.sh"

# Ports the stub lsof reports busy, one per line.
: > "$tmp/busy"
cat > "$tmp/bin/lsof" <<STUB
#!/usr/bin/env bash
for a in "\$@"; do
    case "\$a" in -iTCP:*) grep -qx "\${a#-iTCP:}" "$tmp/busy" && exit 0 ;; esac
done
exit 1
STUB

# The emulator: records its arguments and environment, then runs until
# killed.
cat > "$tmp/sdk/emulator/emulator" <<STUB
#!/usr/bin/env bash
echo "\$*" > "$tmp/emulator.args"
echo "sdk=\$ANDROID_SDK_ROOT avd=\$ANDROID_AVD_HOME adbport=\$ANDROID_ADB_SERVER_PORT" > "$tmp/emulator.env"
echo \$\$ > "$tmp/emulator.pid"
exec tail -f /dev/null
STUB

# adb: records each call with its server port; the device is up and
# booted; `emu kill` ends the stub emulator.
cat > "$tmp/sdk/platform-tools/adb" <<STUB
#!/usr/bin/env bash
echo "port=\$ANDROID_ADB_SERVER_PORT \$*" >> "$tmp/adb.calls"
case "\$*" in
    *get-state) echo device ;;
    *"shell getprop sys.boot_completed") echo 1 ;;
    *"emu kill") kill "\$(cat "$tmp/emulator.pid")" ;;
    kill-server) ;;
    *) exit 1 ;;
esac
STUB
echo "Pkg.Revision=7" > "$tmp/sdk/image/source.properties"
chmod +x "$tmp/bin/lsof" "$tmp/sdk/emulator/emulator" "$tmp/sdk/platform-tools/adb"

export PATH="$tmp/bin:$PATH"
export CLAX_ANDROID_EMULATOR="$tmp/sdk/emulator/emulator"
export CLAX_ANDROID_SYSTEM_IMAGE="$tmp/sdk/image/source.properties"
export CLAX_ANDROID_ADB="$tmp/sdk/platform-tools/adb"

# --- the dry run of --start
if "$script" --start > "$tmp/out" 2>&1 &&
    grep -q 'would make the AVD "Clax Pixel" at .*/target/android-avd/Clax_Pixel.avd' "$tmp/out" &&
    grep -q "would run: $tmp/sdk/emulator/emulator -avd Clax_Pixel -port 5554 -no-audio -no-boot-anim -no-snapshot -no-metrics -gpu swiftshader_indirect -no-window" "$tmp/out" &&
    grep -q 'would take port 5554 (serial emulator-5554, adb server on 15554)' "$tmp/out" &&
    [ ! -e "$repo/target" ] && [ ! -e "$tmp/emulator.args" ] && [ ! -e "$tmp/adb.calls" ]; then
    _ok "the dry run of --start prints the AVD, the command line and the port, and does nothing"
else
    _fail "the dry run of --start"
fi

# --- a busy port, and a window
echo 5554 > "$tmp/busy"
if "$script" --start --window > "$tmp/out" 2>&1 &&
    grep -q -- '-port 5556 -no-audio -no-boot-anim -no-snapshot -no-metrics -gpu auto$' "$tmp/out" &&
    grep -q 'would take port 5556' "$tmp/out"; then
    _ok "a busy 5554 moves it to 5556; --window drops -no-window"
else
    _fail "a busy port"
fi
: > "$tmp/busy"

# --- the dry run of --stop 123
if "$script" --stop 123 > "$tmp/out" 2>&1 &&
    grep -q 'would run: adb -s emulator-<port> emu kill' "$tmp/out" &&
    grep -q 'would wait at most 60 s for 123 to exit, then kill 123' "$tmp/out" &&
    [ ! -e "$tmp/adb.calls" ]; then
    _ok "the dry run of --stop 123 prints the stop and runs no adb"
else
    _fail "the dry run of --stop 123"
fi

# --- --start, acting
if "$script" --start --i-know-what-i-am-doing > "$tmp/out" 2>&1; then
    pid="$(cat "$tmp/emulator.pid")"
    config="$repo/target/android-avd/Clax_Pixel.avd/config.ini"
    if grep -qx "serial=emulator-5554 pid=$pid" "$tmp/out" &&
        grep -qx 'image.sysdir.1=system-images/android-36/google_apis/arm64-v8a/' "$config" &&
        grep -qx 'abi.type=arm64-v8a' "$config" &&
        grep -qx 'AvdId=Clax_Pixel' "$config" &&
        grep -qx "path=$repo/target/android-avd/Clax_Pixel.avd" "$repo/target/android-avd/Clax_Pixel.ini" &&
        [ "$(readlink "$repo/target/android-sdk/system-images/android-36/google_apis/arm64-v8a")" = "$(cd "$tmp/sdk/image" && pwd -P)" ] &&
        [ "$(readlink "$repo/target/android-sdk/emulator")" = "$(cd "$tmp/sdk/emulator" && pwd -P)" ] &&
        grep -qx -- '-avd Clax_Pixel -port 5554 -no-audio -no-boot-anim -no-snapshot -no-metrics -gpu swiftshader_indirect -no-window' "$tmp/emulator.args" &&
        grep -qx "sdk=$repo/target/android-sdk avd=$repo/target/android-avd adbport=15554" "$tmp/emulator.env" &&
        kill -0 "$pid"; then
        _ok "--start writes the AVD, links the SDK, boots and prints serial=emulator-5554 pid=$pid"
    else
        _fail "--start, acting"
    fi
else
    _fail "--start, acting, exited non-zero"
    pid=""
fi

# --- a second --start
if "$script" --start --i-know-what-i-am-doing > "$tmp/out" 2>&1 &&
    grep -qx "serial=emulator-5554 pid=$pid" "$tmp/out" && grep -q 'already running' "$tmp/out" &&
    [ "$(cat "$tmp/emulator.pid")" = "$pid" ] &&
    [ "$("$script" --serial 2> /dev/null)" = emulator-5554 ]; then
    _ok "a second --start prints the running emulator; --serial names it"
else
    _fail "a second --start"
fi

# --- --stop of a PID it did not start
if ! "$script" --stop 999999 --i-know-what-i-am-doing > "$tmp/out" 2>&1 &&
    grep -q 'is not an emulator this script started' "$tmp/out" && kill -0 "$pid" &&
    ! grep -q 'emu kill' "$tmp/adb.calls"; then
    _ok "--stop refuses a PID it did not start"
else
    _fail "--stop of another PID"
fi

# --- --stop, acting
if "$script" --stop "$pid" --i-know-what-i-am-doing > "$tmp/out" 2>&1 &&
    ! kill -0 "$pid" 2> /dev/null &&
    grep -qx 'port=15554 -s emulator-5554 emu kill' "$tmp/adb.calls" &&
    grep -qx 'port=15554 kill-server' "$tmp/adb.calls" &&
    [ ! -e "$repo/target/android-avd/Clax_Pixel.running" ] &&
    ! "$script" --serial > /dev/null 2>&1; then
    _ok "--stop ends the emulator by emu kill and stops its adb server"
else
    _fail "--stop, acting"
fi

# --- every adb call named the emulator, on its own server
if ! grep -v -e '^port=15554 -s emulator-5554 ' -e '^port=15554 kill-server$' "$tmp/adb.calls" > "$tmp/out"; then
    _ok "every adb call names emulator-5554 on the server at 15554"
else
    _fail "an adb call without the emulator's serial or server"
fi

[ "$fails" -eq 0 ] || { echo "$fails failed" >&2; exit 1; }
echo "all passed"
