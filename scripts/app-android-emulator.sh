#!/usr/bin/env bash
# app-android-emulator.sh [--start | --stop PID | --serial] [--window]
#                         [--i-know-what-i-am-doing]
#
# The Android emulator the Clax app's Android integration tests run on
# (//apps/clax_app:integration_android_<name>, bazel/flutter/android.bzl;
# docs/bazel.md, "The Clax app"). Without --i-know-what-i-am-doing it
# prints what it would do (the AVD, the emulator's command line and the
# port it would take) and starts and stops nothing.
#
#   --start   makes the AVD "Clax Pixel" under target/android-avd/ once
#             (its config.ini and .ini written here, no avdmanager), boots
#             it headless on the first free even port from 5554 (--window:
#             in a window, for a person to look at), waits for adb and
#             sys.boot_completed=1 (at most 300 s), and prints
#             `serial=emulator-<port> pid=<pid>`. An emulator this script
#             started that still runs is printed again, not started twice.
#             If it does not boot, or the script exits any other way
#             before printing (an interrupt, a signal), it stops that
#             emulator and its adb server before it exits.
#   --stop PID  stops the emulator this script started as PID: `adb emu
#             kill`, a wait of at most 60 s for PID to exit, then `kill PID`;
#             and the adb server it started for it. It refuses a PID it did
#             not start.
#   --serial  prints the serial of the emulator this script started, if it
#             runs.
#
# An emulator is this script's only while target/android-avd's .running
# file names its serial, its PID, the PID's start time and its command, and
# the PID still runs with that start time and command: a PID the system
# has since given to another process is never reported or stopped.
#
# It never touches a device it did not start: every adb call names the
# emulator's serial, and goes to an adb server of its own (port 10000 above
# the emulator's), never the one on 5037 a plugged-in phone is known to.
# Its state stays in this checkout: target/android-avd (the AVD and the
# running emulator's serial and PID), target/android-home (the emulator's
# and adb's home), target/android-sdk (links to the pinned SDK parts).
#
# The SDK parts are Bazel's, pinned by checksum in MODULE.bazel: the
# emulator (@android_emulator), the system image android-36 google_apis
# arm64-v8a (@android_system_image) and adb (@androidsdk's platform tools).
# A test passes them from its runfiles instead, in CLAX_ANDROID_EMULATOR
# (the emulator executable), CLAX_ANDROID_SYSTEM_IMAGE (a file at the top
# of the system image) and CLAX_ANDROID_ADB (adb).
set -euo pipefail

root="$(cd "$(dirname "$(readlink -f "$0" 2>/dev/null || echo "$0")")/.." && pwd)"

act=""
cmd=""
stop_pid=""
window=""
while [ $# -gt 0 ]; do
    case "$1" in
        --start | --serial) cmd="${1#--}" ;;
        --stop)
            cmd=stop
            stop_pid="${2:-}"
            case "$stop_pid" in '' | *[!0-9]*) echo "usage: $0 --stop PID" >&2; exit 2 ;; esac
            shift
            ;;
        --window) window=1 ;;
        --i-know-what-i-am-doing) act=1 ;;
        *)
            echo "usage: $0 [--start | --stop PID | --serial] [--window] [--i-know-what-i-am-doing]" >&2
            exit 2
            ;;
    esac
    shift
done
[ -n "$cmd" ] || { echo "usage: $0 [--start | --stop PID | --serial] [--window] [--i-know-what-i-am-doing]" >&2; exit 2; }

avd_name="Clax_Pixel"
avd_home="$root/target/android-avd"
user_home="$root/target/android-home"
sdk="$root/target/android-sdk"
running="$avd_home/$avd_name.running"
image_dir="system-images/android-36/google_apis/arm64-v8a"

say() { echo "app-android-emulator: $*"; }

# The SDK parts: the test's, or Bazel's.
sdk_parts() {
    if [ -n "${CLAX_ANDROID_EMULATOR:-}" ]; then
        emulator_bin="$CLAX_ANDROID_EMULATOR"
        image_file="$CLAX_ANDROID_SYSTEM_IMAGE"
        adb="$CLAX_ANDROID_ADB"
        return
    fi
    local bazel
    bazel="$(command -v bazelisk || command -v bazel || true)"
    [ -n "$bazel" ] || { echo "app-android-emulator: needs bazelisk (brew install bazelisk) for the pinned emulator" >&2; exit 1; }
    local targets=(@android_emulator//:emulator @android_system_image//:source.properties @androidsdk//:platform-tools/adb)
    if [ -z "$act" ]; then
        emulator_bin="<$("$bazel" info output_base 2>/dev/null || echo output_base)/external/…/emulator>"
        image_file="<@android_system_image>/source.properties"
        adb="<@androidsdk>/platform-tools/adb"
        return
    fi
    (cd "$root" && "$bazel" build --lockfile_mode=error "${targets[@]}" >/dev/null 2>&1) ||
        { echo "app-android-emulator: could not fetch the emulator (bazelisk build ${targets[*]})" >&2; exit 1; }
    local base
    base="$(cd "$root" && "$bazel" info --lockfile_mode=error output_base 2>/dev/null)" ||
        { echo "app-android-emulator: bazelisk info output_base failed" >&2; exit 1; }
    # A file of an external repository: external/<repository>/… under the
    # output base.
    file_of() {
        local f
        f="$(cd "$root" && "$bazel" cquery --lockfile_mode=error --output=files "$1" 2>/dev/null | sed -n 1p)"
        [ -n "$f" ] || { echo "app-android-emulator: bazelisk cquery --output=files $1 named no file" >&2; exit 1; }
        echo "$base/$f"
    }
    emulator_bin="$(file_of "${targets[0]}")"
    image_file="$(file_of "${targets[1]}")"
    adb="$(file_of "${targets[2]}")"
}

# The directory a file names, with links followed.
real_dir() { (cd "$(dirname "$(readlink -f "$1" 2>/dev/null || echo "$1")")" && pwd -P); }

# Whether something listens on TCP port $1.
listening() { lsof -nP -iTCP:"$1" -sTCP:LISTEN >/dev/null 2>&1; }

# The first even port from 5554 with it and the next free, and the adb
# server's port for it free too.
free_port() {
    local p=5554
    while [ "$p" -le 5682 ]; do
        if ! listening "$p" && ! listening "$((p + 1))" && ! listening "$((p + 10000))"; then
            echo "$p"
            return
        fi
        p=$((p + 2))
    done
    echo "app-android-emulator: no free emulator port from 5554 to 5682" >&2
    exit 1
}

# The AVD's two files, written each time from what this script pins.
write_avd() {
    mkdir -p "$avd_home/$avd_name.avd"
    cat > "$avd_home/$avd_name.ini" <<EOF
avd.ini.encoding=UTF-8
path=$avd_home/$avd_name.avd
path.rel=avd/$avd_name.avd
target=android-36
EOF
    cat > "$avd_home/$avd_name.avd/config.ini" <<EOF
AvdId=$avd_name
avd.ini.displayname=Clax Pixel
avd.ini.encoding=UTF-8
abi.type=arm64-v8a
hw.cpu.arch=arm64
hw.cpu.ncore=4
hw.ramSize=3072
vm.heapSize=512
disk.dataPartition.size=6G
hw.lcd.width=1080
hw.lcd.height=2400
hw.lcd.density=420
hw.keyboard=yes
hw.gpu.enabled=yes
hw.gpu.mode=${gpu}
image.sysdir.1=$image_dir/
tag.id=google_apis
tag.display=Google APIs
target=android-36
PlayStore.enabled=false
fastboot.forceColdBoot=yes
EOF
}

# target/android-sdk: the emulator, adb's platform tools and the system
# image, linked where the emulator looks for them.
link_sdk() {
    mkdir -p "$sdk/system-images/android-36/google_apis"
    ln -sfn "$(real_dir "$emulator_bin")" "$sdk/emulator"
    ln -sfn "$(real_dir "$adb")" "$sdk/platform-tools"
    ln -sfn "$(real_dir "$image_file")" "$sdk/$image_dir"
    mkdir -p "$sdk/platforms"
}

# PID $1's start time and command, as ps gives them; empty when it does not
# run.
started_of() { ps -o lstart= -p "$1" 2>/dev/null | sed 's/^ *//; s/ *$//' || true; }
command_of() { ps -o comm= -p "$1" 2>/dev/null | sed 's/^ *//; s/ *$//' || true; }

# Field $1 of the .running file.
running_field() { sed -n "s/^$1=//p" "$running" | head -1; }

# Writes the .running file for serial $1 and PID $2: its start time, and
# its command unless $3 is "booting" (the emulator may exec another program
# while it boots, so the command is taken once it has booted).
record_running() {
    local cmdline="-"
    [ "${3:-}" = booting ] || cmdline="$(command_of "$2")"
    printf 'serial=%s\npid=%s\nstarted=%s\ncommand=%s\n' "$1" "$2" "$(started_of "$2")" "$cmdline" > "$running.tmp"
    mv "$running.tmp" "$running"
}

# Whether PID $1 is the process the .running file names: the same PID,
# running, with the recorded start time, and the recorded command (when one
# is recorded).
is_ours() {
    [ -f "$running" ] || return 1
    local pid started cmdline now
    pid="$(running_field pid)"
    started="$(running_field started)"
    cmdline="$(running_field command)"
    [ -n "$pid" ] && [ "$pid" = "$1" ] && [ -n "$started" ] || return 1
    now="$(started_of "$pid")"
    [ -n "$now" ] && [ "$now" = "$started" ] || return 1
    [ "$cmdline" = - ] || [ "$(command_of "$pid")" = "$cmdline" ]
}

# The running emulator this script started: "serial pid", or nothing.
running_one() {
    [ -f "$running" ] || return 0
    local serial pid
    serial="$(running_field serial)"
    pid="$(running_field pid)"
    if [ -n "$pid" ] && is_ours "$pid"; then echo "$serial $pid"; fi
}

adb_env() {
    local port="$1"
    export ANDROID_ADB_SERVER_PORT=$((port + 10000))
    export ANDROID_USER_HOME="$user_home" ANDROID_EMULATOR_HOME="$user_home"
    export ANDROID_AVD_HOME="$avd_home" ANDROID_SDK_ROOT="$sdk" ANDROID_HOME="$sdk"
}

# Stops the emulator serial $1, PID $2, which is_ours has checked: `adb emu
# kill`, at most 60 s for the PID to exit, then `kill`; then its adb
# server, and the .running file.
stop_emulator() {
    local serial="$1" pid="$2" deadline
    "$adb" -s "$serial" emu kill > /dev/null 2>&1 || true
    deadline=$((SECONDS + 60))
    while is_ours "$pid" && [ "$SECONDS" -lt "$deadline" ]; do sleep 1; done
    if is_ours "$pid"; then
        say "$pid did not exit after emu kill; killing it"
        kill "$pid" 2>/dev/null || true
    fi
    "$adb" kill-server > /dev/null 2>&1 || true
    rm -f "$running"
}

case "$cmd" in
    serial)
        r="$(running_one)"
        [ -n "$r" ] || { echo "app-android-emulator: no emulator of this script runs" >&2; exit 1; }
        echo "${r%% *}"
        ;;
    start)
        if [ -n "$window" ]; then gpu=auto; else gpu=swiftshader_indirect; fi
        r="$(running_one)"
        if [ -n "$r" ]; then
            say "already running"
            echo "serial=${r%% *} pid=${r##* }"
            exit 0
        fi
        sdk_parts
        port="$(free_port)"
        args=(-avd "$avd_name" -port "$port" -no-audio -no-boot-anim -no-snapshot -no-metrics -gpu "$gpu")
        [ -n "$window" ] || args+=(-no-window)
        if [ -z "$act" ]; then
            say "would make the AVD \"Clax Pixel\" at $avd_home/$avd_name.avd (android-36, google_apis, arm64-v8a, image $image_dir)"
            say "would run: $emulator_bin ${args[*]}"
            say "would take port $port (serial emulator-$port, adb server on $((port + 10000)))"
            say "dry run: pass --i-know-what-i-am-doing to act"
            exit 0
        fi
        write_avd
        link_sdk
        mkdir -p "$user_home"
        adb_env "$port"
        serial="emulator-$port"
        log="$avd_home/emulator-$port.log"
        say "booting $serial (log: $log)"
        "$sdk/emulator/emulator" "${args[@]}" > "$log" 2>&1 < /dev/null &
        pid=$!
        record_running "$serial" "$pid" booting
        # Any exit before the emulator is handed over (a failed boot, an
        # interrupt, a signal) stops it and its adb server.
        booted=""
        unbooted() {
            [ -z "$booted" ] || return 0
            if is_ours "$pid"; then
                echo "app-android-emulator: stopping $serial (pid $pid), which did not boot" >&2
                stop_emulator "$serial" "$pid"
            else
                "$adb" kill-server > /dev/null 2>&1 || true
                rm -f "$running"
            fi
        }
        trap unbooted EXIT
        trap 'exit 130' INT
        trap 'exit 143' TERM
        trap 'exit 129' HUP
        # Until the device answers and Android has booted, polled every 2 s
        # for at most 300 s; the emulator's own exit ends the wait.
        deadline=$((SECONDS + 300))
        while [ "$SECONDS" -lt "$deadline" ]; do
            if ! is_ours "$pid"; then
                echo "app-android-emulator: the emulator exited while booting (log: $log)" >&2
                tail -20 "$log" >&2 || true
                exit 1
            fi
            if [ "$("$adb" -s "$serial" get-state 2>/dev/null || true)" = device ] &&
                [ "$("$adb" -s "$serial" shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" = 1 ]; then
                booted=1
                break
            fi
            sleep 2
        done
        if [ -z "$booted" ]; then
            echo "app-android-emulator: $serial did not boot within 300 s (log: $log)" >&2
            exit 1
        fi
        record_running "$serial" "$pid"
        trap - EXIT INT TERM HUP
        echo "serial=$serial pid=$pid"
        ;;
    stop)
        r="$(running_one)"
        if [ -z "$act" ]; then
            if [ -n "$r" ] && [ "${r##* }" = "$stop_pid" ]; then serial="${r%% *}"; else serial="emulator-<port>"; fi
            say "would stop $stop_pid only if this script started it and it runs (now: ${r:-none})"
            say "would run: adb -s $serial emu kill (on the adb server of that emulator's port + 10000)"
            say "would wait at most 60 s for $stop_pid to exit, then kill $stop_pid"
            say "dry run: pass --i-know-what-i-am-doing to act"
            exit 0
        fi
        if [ -z "$r" ] || [ "${r##* }" != "$stop_pid" ]; then
            echo "app-android-emulator: $stop_pid is not an emulator this script started and runs; nothing stopped" >&2
            exit 1
        fi
        serial="${r%% *}"
        port="${serial#emulator-}"
        sdk_parts
        adb_env "$port"
        stop_emulator "$serial" "$stop_pid"
        say "stopped $serial"
        ;;
esac
