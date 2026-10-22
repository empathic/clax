#!/usr/bin/env bash
# Tests scripts/app-ios.sh on a PATH of stub tools: the dry run (the
# default) prints the build, create, boot, install and launch commands in
# order and runs none of them; a missing iOS runtime stops it with the
# download line; --udid prints the device's UDID and nothing else; the real
# run (--i-know-what-i-am-doing) builds, creates the simulator only when it
# is missing, installs, launches and waits for a core.json naming the
# launched PID ready, and fails when none names it or it names it failed.
#
# The stub xcrun, bazelisk, git, curl and open record their calls in
# $tmp/calls. No build runs and no simulator is touched.

set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

tmp="$(mktemp -d)"
trap 'chmod -R u+w "$tmp" 2>/dev/null; rm -rf "$tmp"' EXIT

fails=0
_ok() { echo "ok: $1"; }
_fail() {
    echo "FAIL: $1" >&2
    sed 's/^/    | /' "$tmp/out" >&2
    fails=$((fails + 1))
}

mkdir -p "$tmp/sys"
for tool in bash env sh sed awk grep head tail tr cat rm mktemp dirname mkdir sleep sort ls unzip seq; do
    ln -s "$(command -v "$tool")" "$tmp/sys/$tool"
done

# A scratch checkout holding the script, and the app's .ipa the stub build
# names.
repo="$tmp/repo"
mkdir -p "$repo/scripts" "$repo/bazel-out/sim/bin/apps/clax_app" "$tmp/ipa/Payload/Clax.app"
cp scripts/app-ios.sh "$repo/scripts/"
echo app > "$tmp/ipa/Payload/Clax.app/Info.plist"
(cd "$tmp/ipa" && /usr/bin/zip -qr "$repo/bazel-out/sim/bin/apps/clax_app/ios_app.ipa" Payload)

container="$tmp/container"
UDID=11111111-2222-3333-4444-555555555555

mkdir -p "$tmp/bin"
cat > "$tmp/bin/xcrun" <<STUB
#!$tmp/sys/bash
echo "xcrun \$*" >> "$tmp/calls"
[ "\$1" = simctl ] || exit 1
shift
case "\$1 \${2:-}" in
    "list runtimes") cat "$tmp/runtimes" ;;
    "list devicetypes")
        echo "== Device Types =="
        echo "iPhone 16 (com.apple.CoreSimulator.SimDeviceType.iPhone-16)"
        echo "iPhone 17 Pro (com.apple.CoreSimulator.SimDeviceType.iPhone-17-Pro)"
        echo "iPhone 17 (com.apple.CoreSimulator.SimDeviceType.iPhone-17)"
        echo "iPad Pro 13-inch (M4) (com.apple.CoreSimulator.SimDeviceType.iPad-Pro-13-inch-M4-8GB)"
        echo "iPad Pro 13-inch (M5) (com.apple.CoreSimulator.SimDeviceType.iPad-Pro-13-inch-M5-12GB)"
        ;;
    "list devices") echo "== Devices =="; echo "-- iOS 26.5 --"; cat "$tmp/devices" ;;
    create*) echo "    \$2 ($UDID) (Shutdown)" >> "$tmp/devices"; echo "$UDID" ;;
    launch*) echo "dev.empathic.clax: 4242" ;;
    get_app_container*) echo "$container" ;;
esac
exit 0
STUB
cat > "$tmp/bin/bazelisk" <<STUB
#!$tmp/sys/bash
echo "bazelisk \$*" >> "$tmp/calls"
case "\$1" in
    cquery) echo "bazel-out/sim/bin/apps/clax_app/ios_app.ipa" ;;
    build) echo "[1 / 2] Linking Clax"; echo "INFO: Build completed successfully" ;;
esac
exit 0
STUB
cat > "$tmp/bin/git" <<STUB
#!$tmp/sys/bash
case "\$1" in
    rev-parse) echo 0123abcd ;;
    status) ;;
esac
STUB
cat > "$tmp/bin/curl" <<STUB
#!$tmp/sys/bash
echo "curl \$*" >> "$tmp/calls"
echo '{"version":"0.1.0","pid":4242,"started_at":"2026-10-19T00:00:00Z"}'
STUB
cat > "$tmp/bin/open" <<STUB
#!$tmp/sys/bash
echo "open \$*" >> "$tmp/calls"
STUB
chmod +x "$tmp/bin/"*

runtime_line='iOS 26.5 (26.5 - 23F77) - com.apple.CoreSimulator.SimRuntime.iOS-26-5'

# _case <runtimes> <devices> <args...>: runs app-ios.sh with those stubs'
# answers, its output in $tmp/out, its status in $status.
_case() {
    printf '== Runtimes ==\n%s\n' "$1" > "$tmp/runtimes"
    printf '%s' "$2" > "$tmp/devices"
    shift 2
    : > "$tmp/calls"
    rm -rf "$repo/target"
    status=0
    env -i HOME="$tmp" PATH="$tmp/bin:$tmp/sys" APP_IOS_DEVICE="${APP_IOS_DEVICE:-}" APP_IOS_WAIT="${APP_IOS_WAIT:-60}" \
        "$tmp/sys/bash" "$repo/scripts/app-ios.sh" "$@" > "$tmp/out" 2>&1 || status=$?
}

# _in_order <file> <pattern>...: each fixed string appears on the line of
# the last or after it.
_in_order() {
    local file="$1" at=0 n p
    shift
    for p in "$@"; do
        n="$(grep -nF -- "$p" "$file" | awk -F: -v at="$at" '$1 >= at { print $1; exit }')"
        [ -n "$n" ] || { echo "missing after line $at: $p" >> "$tmp/out"; return 1; }
        at="$n"
    done
}

# The acting commands, none of which a dry run may run.
_acted() { grep -E '^(bazelisk build|xcrun simctl (create|boot|bootstatus|install|launch))|^open ' "$tmp/calls"; }

# 1. The dry run, with no "Clax iPhone" yet.
_case "$runtime_line" ""
if [ "$status" = 0 ] && _in_order "$tmp/out" \
    "bazelisk build" "-c dbg --ios_multi_cpus=sim_arm64 //apps/clax_app:ios_app" \
    'xcrun simctl create "Clax iPhone" com.apple.CoreSimulator.SimDeviceType.iPhone-17 com.apple.CoreSimulator.SimRuntime.iOS-26-5' \
    "xcrun simctl bootstatus" \
    "xcrun simctl install" "target/app-ios/0123abcd/Payload/Clax.app" \
    "xcrun simctl launch" "dev.empathic.clax" \
    "--i-know-what-i-am-doing" && ! _acted >/dev/null; then
    _ok "the dry run prints build, create, boot, install and launch, in order, and runs none"
else
    _fail "the dry run"
fi

# 2. The dry run with "Clax iPhone" there: no create; its UDID is used.
_case "$runtime_line" "    Clax iPhone (AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE) (Shutdown)
    Other (FFFFFFFF-BBBB-CCCC-DDDD-EEEEEEEEEEEE) (Booted)
"
if [ "$status" = 0 ] && ! grep -qF "simctl create" "$tmp/out" &&
    _in_order "$tmp/out" "xcrun simctl bootstatus AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE -b" \
        "xcrun simctl install AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE" &&
    ! grep -qF FFFFFFFF "$tmp/out" && ! _acted >/dev/null; then
    _ok "an existing Clax iPhone is booted, not created, and no other simulator is named"
else
    _fail "the dry run with Clax iPhone there"
fi

# 3. --dry-run says the same as no argument.
_case "$runtime_line" "" --dry-run
if [ "$status" = 0 ] && grep -qF "simctl launch" "$tmp/out" && ! _acted >/dev/null; then
    _ok "--dry-run is the dry run"
else
    _fail "--dry-run"
fi

# 4. No iOS runtime: exit 1 with the download line, nothing built.
_case "watchOS 26.0 (26.0 - 23R1) - com.apple.CoreSimulator.SimRuntime.watchOS-26-0" ""
if [ "$status" = 1 ] && grep -qF 'No iOS simulator runtime: run `xcodebuild -downloadPlatform iOS` (about 8.5 GB)' "$tmp/out" &&
    ! grep -q '^bazelisk' "$tmp/calls"; then
    _ok "no iOS runtime stops with the download line"
else
    _fail "no iOS runtime (status $status)"
fi

# 5. --udid prints the chosen device's UDID only; with APP_IOS_DEVICE=ipad, the iPad's.
_case "$runtime_line" "    Clax iPhone (AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE) (Shutdown)
    Clax iPad (99999999-BBBB-CCCC-DDDD-EEEEEEEEEEEE) (Shutdown)
" --udid
if [ "$status" = 0 ] && [ "$(cat "$tmp/out")" = AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE ] && ! _acted >/dev/null; then
    _ok "--udid prints Clax iPhone's UDID"
else
    _fail "--udid"
fi
APP_IOS_DEVICE=ipad _case "$runtime_line" "    Clax iPhone (AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE) (Shutdown)
    Clax iPad (99999999-BBBB-CCCC-DDDD-EEEEEEEEEEEE) (Shutdown)
" --udid
if [ "$status" = 0 ] && [ "$(cat "$tmp/out")" = 99999999-BBBB-CCCC-DDDD-EEEEEEEEEEEE ]; then
    _ok "APP_IOS_DEVICE=ipad --udid prints Clax iPad's UDID"
else
    _fail "APP_IOS_DEVICE=ipad --udid"
fi
_case "$runtime_line" "" --udid
if [ "$status" = 1 ] && grep -qF "no Clax iPhone" "$tmp/out" && ! _acted >/dev/null; then
    _ok "--udid with no Clax iPhone fails and creates nothing"
else
    _fail "--udid with no device (status $status)"
fi

# 6. The iPad dry run creates "Clax iPad" from the newest iPad Pro 13-inch.
APP_IOS_DEVICE=ipad _case "$runtime_line" ""
if [ "$status" = 0 ] && grep -qF 'xcrun simctl create "Clax iPad" com.apple.CoreSimulator.SimDeviceType.iPad-Pro-13-inch-M5-12GB com.apple.CoreSimulator.SimRuntime.iOS-26-5' "$tmp/out"; then
    _ok "the iPad dry run creates Clax iPad from the newest iPad Pro 13-inch"
else
    _fail "the iPad dry run"
fi

# 7. The real run: builds, creates, boots, installs, launches, and finds
# the core of the launched PID open.
mkdir -p "$container/Library/Application Support"
printf '{"pid":4242,"state":"ready"}' > "$container/Library/Application Support/core.json"
_case "$runtime_line" "" --i-know-what-i-am-doing
if [ "$status" = 0 ] && _in_order "$tmp/calls" \
    "bazelisk build" "xcrun simctl create Clax iPhone com.apple.CoreSimulator.SimDeviceType.iPhone-17" \
    "xcrun simctl bootstatus $UDID -b" "xcrun simctl install $UDID" \
    "xcrun simctl launch --terminate-running-process $UDID dev.empathic.clax" &&
    grep -qF "Clax is up on Clax iPhone" "$tmp/out" && grep -qF "the core is open (PID 4242)" "$tmp/out" &&
    [ -f "$repo/target/app-ios/0123abcd/Payload/Clax.app/Info.plist" ]; then
    _ok "the real run builds, creates, boots, installs, launches and sees the core open"
else
    _fail "the real run (status $status)"
fi

# 8. A core.json from an earlier launch (another PID) is not taken for this one.
printf '{"pid":17,"state":"ready"}' > "$container/Library/Application Support/core.json"
APP_IOS_WAIT=0 _case "$runtime_line" "    Clax iPhone ($UDID) (Booted)
" --i-know-what-i-am-doing
if [ "$status" = 1 ] && grep -qF "core did not open in 0 s" "$tmp/out" &&
    ! grep -qF "simctl create" "$tmp/calls"; then
    _ok "a core.json naming another PID is not this launch's core"
else
    _fail "a stale core.json (status $status)"
fi

# 9. A core that failed to open ends the wait at once, saying why.
printf '{"pid":4242,"state":"failed","code":"migration_failed"}' > "$container/Library/Application Support/core.json"
APP_IOS_WAIT=20 _case "$runtime_line" "    Clax iPhone ($UDID) (Booted)
" --i-know-what-i-am-doing
if [ "$status" = 1 ] && grep -qF "the core did not open: " "$tmp/out" && grep -qF "migration_failed" "$tmp/out"; then
    _ok "a core.json naming this launch failed ends the wait"
else
    _fail "a failed core.json (status $status)"
fi

if [ "$fails" -gt 0 ]; then
    echo "$fails case(s) failed" >&2
    exit 1
fi
echo "all cases passed"
