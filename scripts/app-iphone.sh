#!/usr/bin/env bash
# app-iphone.sh [--testflight] [--i-know-what-i-am-doing]: builds the Clax
# app for an iPhone or iPad from this checkout's committed HEAD, signed with
# the team's files in ~/.config/clax/apple (CLAX_APPLE_DIR names another;
# docs/usage.md, "Signing for a device and TestFlight"), and installs it on
# the plugged-in or paired device, keeping the app's data; then launches it
# and reads the daemon's daemon.json from the app's data container until it
# names the launched PID. With --testflight it builds the App Store-signed
# app instead and uploads it to App Store Connect for TestFlight.
#
# Without --i-know-what-i-am-doing it prints the command of each step and
# changes nothing (it only lists devices). From any directory.
#
# HEAD is built in place when the checkout is HEAD exactly (no changes, no
# untracked files), else from a clean copy of HEAD at target/app-iphone/tree,
# one directory reused for every HEAD so its Bazel output keeps the work of
# the last build. A device build is
#   bazelisk build -c opt --ios_multi_cpus=arm64 --objc_enable_binary_stripping
#     --features=dead_strip //apps/clax_app:ipa_dev
# and the TestFlight build the same of //apps/clax_app:ipa_appstore with
# --define=clax_build_number=<n>, the app's CFBundleVersion.
#
# The build number: `builds` in the signing directory is this machine's
# ledger of them, one line per marketing version (pubspec.yaml's version
# name at HEAD), `<version> <last number>`. A TestFlight run builds with the
# last number plus one (1 for a version with no line) and writes it to the
# ledger before the upload, so a number is never given to two uploads, even
# when an upload fails. Nothing is committed.
#
# The upload is `xcrun altool --upload-package <ipa> --api-key <ID>
# --api-issuer <issuer ID>`, with the key AuthKey_<ID>.p8 found through
# API_PRIVATE_KEYS_DIR (the signing directory). When altool refuses it,
# Transporter (the app) uploads the same .ipa.
#
# Before building it stops on what the run lacks: a signing file (named,
# each one), a ledger line it cannot read, two devices and no
# APP_IPHONE_DEVICE. A device need not be attached until the build is done:
# it then waits for one. While it builds it prints the step it is on, and
# the time every 30 s.
#
# Environment:
#   CLAX_APPLE_DIR          the signing directory (default ~/.config/clax/apple)
#   APP_IPHONE_DEVICE       which device (its name, UDID or identifier), when
#                           more than one is attached
#   APP_IPHONE_WAIT         seconds to wait for a device after the build
#                           (default 900)
#   APP_IPHONE_DAEMON_WAIT  seconds to wait for the daemon after the launch
#                           (default 60)
set -euo pipefail
cd "$(dirname "$0")/.."
root="$PWD"

act="" testflight=""
for arg in "$@"; do
    case "$arg" in
        --i-know-what-i-am-doing) act=1 ;;
        --testflight) testflight=1 ;;
        *) echo "usage: $0 [--testflight] [--i-know-what-i-am-doing]" >&2; exit 2 ;;
    esac
done

bundle_id=dev.empathic.clax
apple="${CLAX_APPLE_DIR:-$HOME/.config/clax/apple}"
ledger="$apple/builds"
docs='docs/usage.md, "Signing for a device and TestFlight"'
device_wait="${APP_IPHONE_WAIT:-900}"
daemon_wait="${APP_IPHONE_DAEMON_WAIT:-60}"

for tool in git xcrun plutil; do
    command -v "$tool" >/dev/null || { echo "app-iphone: $tool is not on PATH (xcrun and plutil come with Xcode)" >&2; exit 1; }
done
if command -v bazelisk >/dev/null; then bazel="$(command -v bazelisk)"
elif command -v bazel >/dev/null; then bazel="$(command -v bazel)"
else echo "app-iphone: Bazel builds the iOS app and bazelisk is not on PATH (brew install bazelisk)" >&2; exit 1; fi

tmpd="$(mktemp -d)"
trap 'rm -rf "$tmpd"' EXIT

sha="$(git rev-parse --short=8 HEAD)"
dirty=""
if [ -n "$(git status --porcelain)" ]; then
    dirty=1
    echo "app-iphone: uncommitted changes are not in this build (HEAD $sha)"
fi
if [ -n "$dirty" ]; then where="target/app-iphone/tree"; else where="."; fi
log="$root/target/app-iphone/$sha.log"
out="target/app-iphone/$sha"

# The signing files this run needs, and those of them missing.
if [ -n "$testflight" ]; then
    use="the TestFlight build"
    needs=(team-id clax-appstore.mobileprovision api-key-id api-issuer-id)
else
    use="a development device build"
    needs=(team-id clax-development.mobileprovision)
fi
missing=()
for f in "${needs[@]}"; do [ -s "$apple/$f" ] || missing+=("$f"); done
key_id="" issuer=""
if [ -n "$testflight" ]; then
    [ -s "$apple/api-key-id" ] && key_id="$(tr -d '[:space:]' < "$apple/api-key-id")"
    [ -s "$apple/api-issuer-id" ] && issuer="$(tr -d '[:space:]' < "$apple/api-issuer-id")"
    if [ -z "$key_id" ]; then missing+=("AuthKey_<KEYID>.p8 (its name comes from api-key-id)")
    elif [ ! -s "$apple/AuthKey_$key_id.p8" ]; then missing+=("AuthKey_$key_id.p8"); fi
fi
missing_report() {
    echo "The Apple signing inputs for $use are not ready, in $apple:"
    [ -d "$apple" ] || echo "  the directory does not exist: create it with mode 0700 (mkdir -m 700 $apple)"
    printf '  %s: missing\n' "${missing[@]}"
    echo "Each file, and how to get it: $docs."
}
if [ "${#missing[@]}" -gt 0 ] && [ -n "$act" ]; then
    { echo "app-iphone: stopped before building."; missing_report; } >&2
    exit 1
fi

# The build number: the ledger's last for HEAD's marketing version, plus one.
version="$(git show HEAD:apps/clax_app/pubspec.yaml | sed -nE 's/^version: *([0-9][0-9.]*)(\+.*)?$/\1/p' | sed -n 1p)"
[ -n "$version" ] || { echo "app-iphone: HEAD's apps/clax_app/pubspec.yaml has no version: <name>+<code> line" >&2; exit 1; }
# last_number: the ledger's number for $version, 0 when it has none; fails
# naming the first line it cannot read.
last_number() {
    [ -f "$ledger" ] || { echo 0; return 0; }
    awk -v v="$version" -v f="$ledger" '
        !/^[0-9][0-9.]* [0-9]+$/ { printf "app-iphone: line %d of %s is not \"<version> <build number>\": %s\n", NR, f, $0 > "/dev/stderr"; bad = 1; exit 1 }
        $1 == v { n = $2 }
        END { if (!bad) print n + 0 }' "$ledger"
}
if [ -n "$testflight" ]; then
    last="$(last_number)" || exit 1
    number=$((last + 1))
fi

# take_number: records $number for $version in the ledger, one writer at a
# time; fails when another run took it (or a higher one) since this one
# read the ledger.
take_number() {
    local lock="$ledger.lock" tries=0 now
    umask 077
    until mkdir "$lock" 2>/dev/null; do
        tries=$((tries + 1))
        if [ "$tries" -ge 300 ]; then
            echo "app-iphone: $lock has been held for 30 s; remove it if no upload is running" >&2
            return 1
        fi
        sleep 0.1
    done
    now="$(last_number)" || { rmdir "$lock"; return 1; }
    if [ "$now" -ge "$number" ]; then
        rmdir "$lock"
        echo "app-iphone: build $number of $version was taken by another run (the ledger is at $now); run again" >&2
        return 1
    fi
    { [ -f "$ledger" ] && awk -v v="$version" '$1 != v' "$ledger"; echo "$version $number"; } > "$ledger.new"
    mv "$ledger.new" "$ledger"
    rmdir "$lock"
}

flags=(-c opt --ios_multi_cpus=arm64 --objc_enable_binary_stripping --features=dead_strip)
if [ -n "$testflight" ]; then
    target=//apps/clax_app:ipa_appstore
    flags+=("--define=clax_build_number=$number")
else
    target=//apps/clax_app:ipa_dev
fi
build=("$bazel" build --lockfile_mode=error --curses=no --color=no --show_progress_rate_limit=1 "${flags[@]}" "$target")

# The paired iOS devices that can be reached, one per line:
# <identifier> TAB <udid> TAB <name>.
devices() {
    local j="$tmpd/devices.json" n i
    xcrun devicectl list devices --quiet --json-output "$j" >/dev/null 2>&1 || return 0
    n="$(plutil -extract result.devices raw -o - "$j" 2>/dev/null || echo 0)"
    field() { plutil -extract "result.devices.$i.$1" raw -o - "$j" 2>/dev/null || true; }
    for ((i = 0; i < n; i++)); do
        [ "$(field hardwareProperties.platform)" = iOS ] || continue
        [ "$(field connectionProperties.pairingState)" = paired ] || continue
        [ "$(field connectionProperties.tunnelState)" = unavailable ] && continue
        printf '%s\t%s\t%s\n' "$(field identifier)" "$(field hardwareProperties.udid)" "$(field deviceProperties.name)"
    done
}
# pick: sets device and device_name; 1 when there is none (or none is
# APP_IPHONE_DEVICE), 2 (after naming them) when there are several.
device="" device_name=""
pick() {
    local list line
    list="$(devices)"
    if [ -n "${APP_IPHONE_DEVICE:-}" ]; then
        line="$(awk -F'\t' -v d="$APP_IPHONE_DEVICE" '$1 == d || $2 == d || $3 == d { print; exit }' <<<"$list")"
    else
        case "$(printf '%s\n' "$list" | grep -c .)" in
            0) line="" ;;
            1) line="$list" ;;
            *) echo "app-iphone: more than one device; set APP_IPHONE_DEVICE to the name or UDID of one of:" >&2
               awk -F'\t' '{ printf "  %s (%s)\n", $3, $2 }' <<<"$list" >&2
               return 2 ;;
        esac
    fi
    [ -n "$line" ] || return 1
    device="$(cut -f1 <<<"$line")"
    device_name="$(cut -f3 <<<"$line")"
}
if [ -z "$testflight" ]; then
    picked=0; pick || picked=$?
    [ "$picked" = 2 ] && exit 1
    if [ "$picked" = 1 ]; then
        echo "app-iphone: no iPhone or iPad ${APP_IPHONE_DEVICE:+named $APP_IPHONE_DEVICE }attached yet; it is needed once the build is done"
    fi
fi

daemon_path="Library/Application Support/clax-home/daemon.json"
upload=(xcrun altool --upload-package "<the .ipa>" --api-key "${key_id:-<api-key-id>}" --api-issuer "${issuer:-<api-issuer-id>}" --show-progress)

if [ -z "$act" ]; then
    if [ "${#missing[@]}" -gt 0 ]; then
        missing_report
        echo "(a real run stops here, before building)"
    fi
    if [ "$where" = . ]; then echo "would build HEAD $sha here (log: target/app-iphone/$sha.log):"
    else echo "would build HEAD $sha in a clean copy at $where (log: target/app-iphone/$sha.log):"; fi
    if [ -n "$testflight" ]; then
        if [ "$last" = 0 ]; then echo "  as $version build $number, the first of $version in $ledger:"
        else echo "  as $version build $number, the next after $last in $ledger:"; fi
        echo "  ${build[*]}"
        echo "would write \"$version $number\" to $ledger, then upload it with the API key in $apple:"
        echo "  API_PRIVATE_KEYS_DIR=$apple ${upload[*]}"
        echo "and print build $number and https://appstoreconnect.apple.com/apps"
    else
        echo "  ${build[*]}"
        echo "would unzip the app into $out and install it on ${device_name:-the device attached by then}, keeping its data:"
        echo "  xcrun devicectl device install app --device ${device:-<its identifier>} $out/Payload/Clax.app"
        echo "would launch it and wait up to $daemon_wait s for daemon.json in its data container to name the launched PID:"
        echo "  xcrun devicectl device process launch --device ${device:-<its identifier>} --terminate-existing $bundle_id"
        echo "  xcrun devicectl device copy from --device ${device:-<its identifier>} --domain-type appDataContainer --domain-identifier $bundle_id --source \"$daemon_path\""
    fi
    echo "run again with --i-know-what-i-am-doing (or: just app-iphone${testflight:+-testflight}) to do it"
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
            printf 'app-iphone: %dm%02ds  %s\n' $((e / 60)) $((e % 60)) "$step"
        elif [ $((e - told)) -ge 30 ]; then
            told=$e
            printf 'app-iphone: %dm%02ds  still %s\n' $((e / 60)) $((e % 60)) "${shown:-starting}"
        fi
    done
    trap - INT TERM
    if ! wait "$build_pid"; then
        tail -20 "$log" >&2
        echo "app-iphone: build failed; the whole log is $log" >&2
        return 1
    fi
    e=$((SECONDS - start))
    printf 'app-iphone: built in %dm%02ds\n' $((e / 60)) $((e % 60))
}

mkdir -p target/app-iphone
if [ "$where" != . ]; then
    echo "app-iphone: copying HEAD $sha to $where"
    rm -rf "$where"
    mkdir -p "$where"
    git archive HEAD | tar -x -C "$where"
fi
if [ -n "$testflight" ]; then
    echo "app-iphone: building HEAD $sha for TestFlight as $version build $number (log: $log)"
else
    echo "app-iphone: building HEAD $sha for a device (log: $log)"
fi
(cd "$where" && CLAX_APPLE_DIR="$apple" "${build[@]}") > "$log" 2>&1 &
watch $! || exit 1
ipa="$(cd "$where" && "$bazel" cquery --lockfile_mode=error "${flags[@]}" --output=files "$target" 2>/dev/null | sed -n 1p)"
ipa="$(cd "$where" && pwd)/$ipa"
[ -f "$ipa" ] || { echo "app-iphone: no .ipa for $target" >&2; exit 1; }

if [ -n "$testflight" ]; then
    take_number || exit 1
    echo "app-iphone: build $number of $version is taken ($ledger); uploading $ipa"
    upload[3]="$ipa"
    if ! API_PRIVATE_KEYS_DIR="$apple" "${upload[@]}"; then
        echo "app-iphone: altool refused the upload; build $number stays taken and is never given again." >&2
        echo "app-iphone: upload $ipa with Transporter (the app) instead, or run again for build $((number + 1))" >&2
        exit 1
    fi
    echo "app-iphone: uploaded $version build $number; it shows in TestFlight once App Store Connect has processed it:"
    echo "  https://appstoreconnect.apple.com/apps"
    exit 0
fi

# Wait for the device, if it was not attached when the build began.
picked=0; pick || picked=$?
[ "$picked" = 2 ] && exit 1
if [ "$picked" = 1 ]; then
    echo "app-iphone: built; connect the iPhone or iPad, unlock it and trust this Mac (waiting up to $((device_wait / 60)) min)"
    start=$SECONDS told=0
    while :; do
        picked=0; pick || picked=$?
        [ "$picked" = 0 ] && break
        [ "$picked" = 2 ] && exit 1
        e=$((SECONDS - start))
        if [ "$e" -ge "$device_wait" ]; then
            echo "app-iphone: no device after $((device_wait / 60)) min; the build is kept, so run again to install it" >&2
            exit 1
        fi
        if [ $((e - told)) -ge 60 ]; then
            told=$e; printf 'app-iphone: still waiting for a device (%dm%02ds)\n' $((e / 60)) $((e % 60))
        fi
        sleep 1
    done
fi

echo "app-iphone: installing $sha on $device_name"
rm -rf "$out"
mkdir -p "$out"
unzip -q "$ipa" -d "$out"
xcrun devicectl device install app --device "$device" "$root/$out/Payload/Clax.app"
# Keep the newest three copies (and their logs).
ls -1t target/app-iphone | { grep -v '\.log$' | grep -vx tree || true; } | tail -n +4 | while read -r old; do
    rm -rf "target/app-iphone/$old" "target/app-iphone/$old.log"
done

xcrun devicectl device process launch --device "$device" --terminate-existing --quiet \
    --json-output "$tmpd/launch.json" "$bundle_id" >/dev/null
pid="$(plutil -extract result.process.processIdentifier raw -o - "$tmpd/launch.json" 2>/dev/null || true)"
case "$pid" in '' | *[!0-9]*) echo "app-iphone: the launch named no PID (see $tmpd/launch.json)" >&2; exit 1 ;; esac
echo "app-iphone: launched Clax (PID $pid); waiting up to $daemon_wait s for its daemon"

# The daemon's daemon.json in the app's data container names the PID.
info="$tmpd/daemon.json"
start=$SECONDS
while :; do
    rm -f "$info"
    xcrun devicectl device copy from --device "$device" --domain-type appDataContainer \
        --domain-identifier "$bundle_id" --source "$daemon_path" --destination "$info" --quiet >/dev/null 2>&1 || true
    if [ "$(sed -nE 's/^ *"pid" *: *([0-9]+).*/\1/p' "$info" 2>/dev/null | sed -n 1p)" = "$pid" ]; then break; fi
    if [ $((SECONDS - start)) -ge "$daemon_wait" ]; then
        echo "app-iphone: the daemon did not come up in $daemon_wait s (no daemon.json naming PID $pid in the app's data container)" >&2
        exit 1
    fi
    sleep 2
done
echo "app-iphone: $sha installed on $device_name; Clax is open and its daemon (PID $pid) is up"
