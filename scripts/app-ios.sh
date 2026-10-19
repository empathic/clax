#!/usr/bin/env bash
# app-ios.sh [--i-know-what-i-am-doing | --dry-run | --udid]: builds the
# Clax app for the iOS simulator from this checkout (with Bazel, the only
# iOS build: //apps/clax_app:ios_app, -c dbg, --ios_multi_cpus=sim_arm64)
# and runs it on the simulator "Clax iPhone", keeping the app's data, then
# waits for its in-process daemon to answer.
#
# Without an argument, or with --dry-run, it prints the command of each
# step and runs none (it only lists runtimes, device types and devices).
# --udid prints the chosen simulator's UDID and does nothing else; it fails
# when the simulator does not exist yet. --i-know-what-i-am-doing (`just
# app-ios`) acts, printing the step it is on:
#   1. stops unless `xcrun simctl list runtimes` names an iOS runtime;
#   2. builds the app (log: target/app-ios/<sha>.log), uncommitted changes
#      included;
#   3. finds the simulator, or creates it from the newest iOS runtime and
#      the iPhone 17 device type (APP_IOS_DEVICE=ipad: "Clax iPad", the
#      newest iPad Pro 13-inch), boots it and shows it in Simulator;
#   4. unzips the app into target/app-ios/<sha>/ and installs it;
#   5. launches it and waits, every 0.5 s for at most APP_IOS_WAIT seconds
#      (60 by default), for daemon.json in the app's data container naming
#      the launched PID, then for /healthz on its port answering with it.
# It names, boots and installs on no simulator but the chosen one, and
# leaves it running.
#
# Environment:
#   APP_IOS_DEVICE  iphone (default) or ipad
#   APP_IOS_WAIT    seconds to wait for the daemon after the launch (default 60)
set -euo pipefail
cd "$(dirname "$0")/.."
root="$PWD"

act=""
udid_only=""
case "${1:-}" in
    --i-know-what-i-am-doing) act=1 ;;
    --udid) udid_only=1 ;;
    --dry-run | "") ;;
    *) echo "usage: $0 [--i-know-what-i-am-doing | --dry-run | --udid]" >&2; exit 2 ;;
esac

bundle_id=dev.empathic.clax
case "${APP_IOS_DEVICE:-iphone}" in
    iphone) device="Clax iPhone" ;;
    ipad) device="Clax iPad" ;;
    *) echo "app-ios: APP_IOS_DEVICE is iphone or ipad, not ${APP_IOS_DEVICE}" >&2; exit 2 ;;
esac
wait_s="${APP_IOS_WAIT:-60}"

command -v xcrun >/dev/null || { echo "app-ios: xcrun is not on PATH (install Xcode)" >&2; exit 1; }

# 1. An iOS runtime: its identifier, the newest installed.
runtime="$(xcrun simctl list runtimes |
    sed -nE 's/^iOS ([0-9.]+) .* - (com\.apple\.CoreSimulator\.SimRuntime\.iOS-[0-9-]+)$/\1 \2/p' |
    sort -t. -k1,1n -k2,2n -k3,3n | tail -n 1 | sed 's/^[^ ]* //')"
if [ -z "$runtime" ]; then
    echo "app-ios: No iOS simulator runtime: run \`xcodebuild -downloadPlatform iOS\` (about 8.5 GB)" >&2
    exit 1
fi

# The chosen simulator's UDID and state, when it exists (the first of that name).
found="$(xcrun simctl list devices |
    sed -nE "s/^ +$device \(([0-9A-F-]{36})\) \(([A-Za-z ]+)\).*/\1 \2/p" | sed -n 1p)"
udid="${found%% *}"
state="${found#* }"

if [ -n "$udid_only" ]; then
    [ -n "$udid" ] || { echo "app-ios: no $device simulator yet; run: just app-ios" >&2; exit 1; }
    echo "$udid"
    exit 0
fi

if [ -z "$udid" ]; then
    types="$(xcrun simctl list devicetypes | sed -nE 's/.*\((com\.apple\.CoreSimulator\.SimDeviceType\.[^)]+)\)$/\1/p')"
    if [ "$device" = "Clax iPhone" ]; then
        devtype="$(grep -x 'com\.apple\.CoreSimulator\.SimDeviceType\.iPhone-17' <<<"$types" || true)"
    else
        devtype="$(grep -E '^com\.apple\.CoreSimulator\.SimDeviceType\.iPad-Pro-13-inch-M[0-9]+-[0-9]+GB$' <<<"$types" |
            sort -V | tail -n 1 || true)"
    fi
    [ -n "$devtype" ] || { echo "app-ios: no device type for $device (xcrun simctl list devicetypes)" >&2; exit 1; }
fi

if command -v bazelisk >/dev/null; then bazel="$(command -v bazelisk)"
elif command -v bazel >/dev/null; then bazel="$(command -v bazel)"
else echo "app-ios: Bazel builds the iOS app and bazelisk is not on PATH (brew install bazelisk)" >&2; exit 1; fi

sha="$(git rev-parse --short=8 HEAD 2>/dev/null || echo worktree)"
dirty=""
[ -n "$(git status --porcelain 2>/dev/null)" ] && dirty=1
out="target/app-ios/$sha"
log="$root/target/app-ios/$sha.log"
app="$out/Payload/Clax.app"
build=("$bazel" build --lockfile_mode=error --curses=no --color=no --show_progress_rate_limit=1
    -c dbg --ios_multi_cpus=sim_arm64 //apps/clax_app:ios_app)

if [ -z "$act" ]; then
    echo "would build ${dirty:+the working tree at }HEAD $sha${dirty:+ with its uncommitted changes} (log: target/app-ios/$sha.log):"
    echo "  ${build[*]}"
    if [ -z "$udid" ]; then
        echo "would create the simulator $device and boot it:"
        echo "  xcrun simctl create \"$device\" $devtype $runtime"
        echo "  xcrun simctl bootstatus <its UDID> -b"
    else
        echo "would boot $device ($state) and show it in Simulator:"
        echo "  xcrun simctl bootstatus $udid -b"
    fi
    echo "would unzip the app into $out and install it, keeping its data:"
    echo "  xcrun simctl install ${udid:-<its UDID>} $app"
    echo "would launch it and wait up to $wait_s s for its daemon (daemon.json in its data container, then /healthz):"
    echo "  xcrun simctl launch --terminate-running-process ${udid:-<its UDID>} $bundle_id"
    echo "run again with --i-know-what-i-am-doing (or: just app-ios) to do it"
    exit 0
fi

# watch PID: prints Bazel's progress line when it changes (at most every
# 5 s) and otherwise every 30 s, until PID exits; returns its status.
watch() {
    local build_pid="$1" start=$SECONDS shown="" told=0 e step
    trap 'kill "$build_pid" 2>/dev/null' INT TERM
    while kill -0 "$build_pid" 2>/dev/null; do
        sleep 1
        e=$((SECONDS - start))
        step="$(grep -E '^\[[0-9,]+ / [0-9,]+\] ' "$log" 2>/dev/null | tail -n 1 | sed -E 's/;.*//' || true)"
        if [ -n "$step" ] && [ "$step" != "$shown" ] && [ $((e - told)) -ge 5 ]; then
            shown="$step"; told=$e
            printf 'app-ios: %dm%02ds  %s\n' $((e / 60)) $((e % 60)) "$step"
        elif [ $((e - told)) -ge 30 ]; then
            told=$e
            printf 'app-ios: %dm%02ds  still %s\n' $((e / 60)) $((e % 60)) "${shown:-starting}"
        fi
    done
    trap - INT TERM
    if ! wait "$build_pid"; then
        tail -20 "$log" >&2
        echo "app-ios: build failed; the whole log is $log" >&2
        return 1
    fi
    e=$((SECONDS - start))
    printf 'app-ios: built in %dm%02ds\n' $((e / 60)) $((e % 60))
}

mkdir -p target/app-ios
[ -n "$dirty" ] && echo "app-ios: building the working tree, uncommitted changes included (HEAD $sha)"
echo "app-ios: building the simulator app with Bazel (log: $log)"
"${build[@]}" > "$log" 2>&1 &
watch $! || exit 1
ipa="$("$bazel" cquery --lockfile_mode=error -c dbg --ios_multi_cpus=sim_arm64 --output=files //apps/clax_app:ios_app 2>/dev/null | sed -n 1p)"
[ -n "$ipa" ] && [ -f "$root/$ipa" ] || { echo "app-ios: no .ipa for //apps/clax_app:ios_app" >&2; exit 1; }

if [ -z "$udid" ]; then
    echo "app-ios: creating the simulator $device ($devtype, $runtime)"
    udid="$(xcrun simctl create "$device" "$devtype" "$runtime")"
    state=Shutdown
fi
if [ "$state" != Booted ]; then echo "app-ios: booting $device ($udid)"; fi
xcrun simctl bootstatus "$udid" -b >/dev/null
open -a Simulator --args -CurrentDeviceUDID "$udid" || true

echo "app-ios: installing the app on $device"
rm -rf "$out"
mkdir -p "$out"
unzip -q "$root/$ipa" -d "$out"
xcrun simctl install "$udid" "$app"
# Keep the newest three copies (and their logs).
ls -1t target/app-ios | { grep -v '\.log$' || true; } | tail -n +4 | while read -r old; do
    rm -rf "target/app-ios/$old" "target/app-ios/$old.log"
done

launched="$(xcrun simctl launch --terminate-running-process "$udid" "$bundle_id")"
pid="${launched##*: }"
case "$pid" in '' | *[!0-9]*) echo "app-ios: the launch named no PID: $launched" >&2; exit 1 ;; esac
echo "app-ios: launched Clax (PID $pid); waiting up to $wait_s s for its daemon"

# The daemon's daemon.json under the app's data container names the PID
# and the port; /healthz on that port answers with the same PID.
data="$(xcrun simctl get_app_container "$udid" "$bundle_id" data)"
info="$data/Library/Application Support/clax-home/daemon.json"
field() { sed -nE "s/^ *\"$1\" *: *([0-9]+).*/\1/p" "$info" 2>/dev/null | sed -n 1p; }
start=$SECONDS port=""
while :; do
    if [ "$(field pid)" = "$pid" ]; then
        port="$(field port)"
        health="$(curl -fsS --max-time 2 "http://127.0.0.1:$port/healthz" 2>/dev/null || true)"
        if grep -qE "\"pid\" *: *$pid([^0-9]|$)" <<<"$health"; then break; fi
    fi
    if [ $((SECONDS - start)) -ge "$wait_s" ]; then
        echo "app-ios: the daemon did not come up in $wait_s s (no daemon.json naming PID $pid with a /healthz answering; see $data/Library/Caches/clax/daemon.log)" >&2
        exit 1
    fi
    sleep 0.5
done
echo "app-ios: the daemon answers on port $port (PID $pid)"
echo "app-ios: Clax is up on $device"
