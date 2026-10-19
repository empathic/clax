#!/usr/bin/env bash
# Tests scripts/app-iphone.sh on a PATH of stub tools, run from a directory
# outside the checkout:
#   - the dry run (the default) prints the device build, install and launch
#     commands in order, and with --testflight the build with the next build
#     number and the upload; it runs none of them and writes no ledger;
#   - the real device run builds, then installs on the device, launches the
#     app and reads daemon.json until it names the launched PID; a device
#     need not be there until the build is done, and two need
#     APP_IPHONE_DEVICE;
#   - the real TestFlight run writes the next build number (`1.0.0 4` gives
#     `1.0.0 5`) before it calls altool, keeps the other versions' lines, and
#     a failing upload leaves the number spent, so the next run takes 6;
#   - without the signing directory a real run stops before building,
#     naming each missing file;
#   - a malformed ledger stops it before building.
#
# The stub xcrun (devicectl, altool), bazelisk and git record their calls in
# $tmp/calls. No build runs, no device is touched and nothing is uploaded.

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
for tool in bash env sh sed awk grep head tail tr cat rm mv cp mktemp dirname basename mkdir rmdir \
    sleep sort ls unzip seq chmod tar plutil wc cut; do
    ln -s "$(command -v "$tool")" "$tmp/sys/$tool"
done

# A scratch checkout holding the script, and the two .ipa files the stub
# build names.
repo="$tmp/repo"
mkdir -p "$repo/scripts" "$repo/bazel-out/ios/bin/apps/clax_app" "$tmp/ipa/Payload/Clax.app"
cp scripts/app-iphone.sh "$repo/scripts/"
echo app > "$tmp/ipa/Payload/Clax.app/Info.plist"
for kind in dev appstore; do
    (cd "$tmp/ipa" && /usr/bin/zip -qr "$repo/bazel-out/ios/bin/apps/clax_app/ipa_$kind.ipa" Payload)
done

apple="$tmp/apple"

mkdir -p "$tmp/bin"
cat > "$tmp/bin/xcrun" <<STUB
#!$tmp/sys/bash
echo "xcrun \$*" >> "$tmp/calls"
# The value after --json-output or --destination.
json="" dest=""
prev=""
for a in "\$@"; do
    case "\$prev" in --json-output) json="\$a" ;; --destination) dest="\$a" ;; esac
    prev="\$a"
done
case "\$1 \$2 \${3:-}" in
    "devicectl list devices")
        # Each listing takes the next file of \$tmp/devices.d, the last one for good.
        next="\$(ls "$tmp/devices.d" | sort -n | sed -n 1p)"
        cat "$tmp/devices.d/\$next" > "\$json"
        [ "\$(ls "$tmp/devices.d" | wc -l)" -gt 1 ] && rm "$tmp/devices.d/\$next"
        ;;
    "devicectl device install") ;;
    "devicectl device process")
        echo '{"result":{"process":{"processIdentifier":4242}}}' > "\$json" ;;
    "devicectl device copy")
        [ -f "$tmp/daemon.json" ] || exit 1
        cat "$tmp/daemon.json" > "\$dest" ;;
    altool*)
        echo "API_PRIVATE_KEYS_DIR=\${API_PRIVATE_KEYS_DIR:-}" >> "$tmp/calls"
        cat "$apple/builds" > "$tmp/ledger-at-upload"
        exit "\$(cat "$tmp/altool-status")" ;;
esac
exit 0
STUB
cat > "$tmp/bin/bazelisk" <<STUB
#!$tmp/sys/bash
echo "bazelisk \$*" >> "$tmp/calls"
case "\$1" in
    cquery)
        for a in "\$@"; do
            case "\$a" in //apps/clax_app:ipa_*) echo "bazel-out/ios/bin/apps/clax_app/\${a#//apps/clax_app:}.ipa" ;; esac
        done ;;
    build) echo "[1 / 2] Signing Clax"; echo "INFO: Build completed successfully" ;;
esac
exit 0
STUB
cat > "$tmp/bin/git" <<STUB
#!$tmp/sys/bash
case "\$1" in
    rev-parse) echo 0123abcd ;;
    status) cat "$tmp/git-status" ;;
    show) echo "name: clax_app"; echo "version: 1.0.0+1" ;;
esac
STUB
chmod +x "$tmp/bin/"*

# _device <identifier> <udid> <name> [pairing] [tunnel]: one device's JSON.
_device() {
    printf '{"identifier":"%s","hardwareProperties":{"platform":"iOS","udid":"%s"},"deviceProperties":{"name":"%s"},"connectionProperties":{"pairingState":"%s","tunnelState":"%s"}}' \
        "$1" "$2" "$3" "${4:-paired}" "${5:-connected}"
}
# _listing <device JSON>...: one devicectl listing.
_listing() {
    local IFS=,
    printf '{"info":{"outcome":"success"},"result":{"devices":[%s]}}\n' "$*"
}
phone="$(_device AB-1 00008140-0001 "Test iPhone")"
pad="$(_device AB-2 00008140-0002 "Second iPad")"
away="$(_device AB-3 00008140-0003 "Away iPhone" paired unavailable)"

# _apple: a complete signing directory.
_apple() {
    rm -rf "$apple"
    mkdir -m 700 "$apple"
    echo ABCDE12345 > "$apple/team-id"
    echo profile > "$apple/clax-development.mobileprovision"
    echo profile > "$apple/clax-appstore.mobileprovision"
    echo KEY1234567 > "$apple/api-key-id"
    echo 57246542-96fe-1a63-e053-0824d011072a > "$apple/api-issuer-id"
    echo key > "$apple/AuthKey_KEY1234567.p8"
    chmod 600 "$apple"/*
}

# _case <listing>... -- <args...>: runs app-iphone.sh from outside the
# checkout, the stub devicectl giving each listing in turn (the last for
# good); its output in $tmp/out, its status in $status.
_case() {
    rm -rf "$tmp/devices.d"
    mkdir -p "$tmp/devices.d"
    local i=0
    while [ "$1" != -- ]; do
        i=$((i + 1))
        printf '%s\n' "$1" > "$tmp/devices.d/$i"
        shift
    done
    shift
    : > "$tmp/calls"
    rm -f "$tmp/ledger-at-upload"
    [ -f "$tmp/git-status" ] || : > "$tmp/git-status"
    [ -f "$tmp/altool-status" ] || echo 0 > "$tmp/altool-status"
    rm -rf "$repo/target"
    status=0
    (cd "$tmp" && env -i HOME="$tmp/home" PATH="$tmp/bin:$tmp/sys" CLAX_APPLE_DIR="$apple" \
        APP_IPHONE_DEVICE="${APP_IPHONE_DEVICE:-}" APP_IPHONE_DAEMON_WAIT="${APP_IPHONE_DAEMON_WAIT:-60}" \
        "$tmp/sys/bash" "$repo/scripts/app-iphone.sh" "$@") > "$tmp/out" 2>&1 || status=$?
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
_acted() { grep -E '^(bazelisk build|xcrun devicectl device (install|process|copy)|xcrun altool)' "$tmp/calls"; }

dev_flags="-c opt --ios_multi_cpus=arm64 --objc_enable_binary_stripping --features=dead_strip"

# 1. The dry run, one device attached: build, install, launch, in order.
_apple
_case "$(_listing "$phone")" --
if [ "$status" = 0 ] && _in_order "$tmp/out" \
    "bazelisk build" "$dev_flags //apps/clax_app:ipa_dev" \
    "xcrun devicectl device install app --device AB-1" "target/app-iphone/0123abcd/Payload/Clax.app" \
    "xcrun devicectl device process launch --device AB-1" "dev.empathic.clax" \
    "daemon.json" \
    "--i-know-what-i-am-doing" &&
    grep -qF "Test iPhone" "$tmp/out" && ! _acted >/dev/null && [ ! -e "$apple/builds" ]; then
    _ok "the dry run prints the build, install and launch, in order, and runs none"
else
    _fail "the dry run"
fi

# 2. The TestFlight dry run takes no number: it says which it would take.
_apple
printf '1.0.0 4\n' > "$apple/builds"
_case "$(_listing)" -- --testflight
if [ "$status" = 0 ] && _in_order "$tmp/out" \
    "5" \
    "bazelisk build" "--define=clax_build_number=5" "//apps/clax_app:ipa_appstore" \
    "xcrun altool --upload-package" "--api-key KEY1234567 --api-issuer 57246542-96fe-1a63-e053-0824d011072a" &&
    ! _acted >/dev/null && [ "$(cat "$apple/builds")" = "1.0.0 4" ]; then
    _ok "the TestFlight dry run names build 5 and the upload, and writes nothing"
else
    _fail "the TestFlight dry run"
fi

# 3. The real TestFlight run: 5 is in the ledger before altool runs, and
# the other versions' lines stay.
_apple
printf '0.9.0 12\n1.0.0 4\n' > "$apple/builds"
_case "$(_listing)" -- --testflight --i-know-what-i-am-doing
if [ "$status" = 0 ] && _in_order "$tmp/calls" \
    "bazelisk build" "--define=clax_build_number=5" "//apps/clax_app:ipa_appstore" \
    "xcrun altool --upload-package $repo/bazel-out/ios/bin/apps/clax_app/ipa_appstore.ipa --api-key KEY1234567 --api-issuer 57246542-96fe-1a63-e053-0824d011072a" \
    "API_PRIVATE_KEYS_DIR=$apple" &&
    grep -qx "1.0.0 5" "$tmp/ledger-at-upload" &&
    [ "$(cat "$apple/builds")" = "$(printf '0.9.0 12\n1.0.0 5')" ] &&
    grep -qF "build 5" "$tmp/out" && grep -qF "https://appstoreconnect.apple.com/apps" "$tmp/out" &&
    ! grep -qF "devicectl device" "$tmp/calls"; then
    _ok "the TestFlight run writes 1.0.0 5 before the upload and keeps 0.9.0 12"
else
    _fail "the TestFlight run (status $status)"
fi

# 4. A failing upload leaves its number spent: the next run takes 6.
_apple
printf '1.0.0 4\n' > "$apple/builds"
echo 1 > "$tmp/altool-status"
_case "$(_listing)" -- --testflight --i-know-what-i-am-doing
if [ "$status" = 1 ] && [ "$(cat "$apple/builds")" = "1.0.0 5" ] && grep -qF "Transporter" "$tmp/out"; then
    _ok "a refused upload leaves 1.0.0 5 and names Transporter"
else
    _fail "a refused upload (status $status)"
fi
echo 0 > "$tmp/altool-status"
_case "$(_listing)" -- --testflight --i-know-what-i-am-doing
if [ "$status" = 0 ] && grep -qF -- "--define=clax_build_number=6" "$tmp/calls" && [ "$(cat "$apple/builds")" = "1.0.0 6" ]; then
    _ok "the run after a refused upload takes 6, never 5 again"
else
    _fail "the run after a refused upload (status $status)"
fi

# 5. A marketing version with no line yet starts at 1, below the others' lines.
_apple
printf '0.9.0 12\n' > "$apple/builds"
_case "$(_listing)" -- --testflight --i-know-what-i-am-doing
if [ "$status" = 0 ] && grep -qF -- "--define=clax_build_number=1" "$tmp/calls" &&
    [ "$(cat "$apple/builds")" = "$(printf '0.9.0 12\n1.0.0 1')" ]; then
    _ok "a new marketing version takes build 1"
else
    _fail "a new marketing version (status $status)"
fi

# 6. A ledger with a line it cannot read stops the run before the build.
_apple
printf '1.0.0 five\n' > "$apple/builds"
_case "$(_listing)" -- --testflight --i-know-what-i-am-doing
if [ "$status" = 1 ] && grep -qF "$apple/builds" "$tmp/out" && ! _acted >/dev/null &&
    [ "$(cat "$apple/builds")" = "1.0.0 five" ]; then
    _ok "a malformed ledger stops the run before building"
else
    _fail "a malformed ledger (status $status)"
fi

# 7. Without the signing directory a real run stops before the build,
# naming each missing file and the section that says how to get it.
rm -rf "$apple"
_case "$(_listing "$phone")" -- --i-know-what-i-am-doing
if [ "$status" = 1 ] && grep -qF "team-id" "$tmp/out" && grep -qF "clax-development.mobileprovision" "$tmp/out" &&
    ! grep -qF "clax-appstore.mobileprovision" "$tmp/out" &&
    grep -qF "Signing for a device and TestFlight" "$tmp/out" && ! _acted >/dev/null; then
    _ok "a device run without the signing directory stops naming its missing files"
else
    _fail "a device run without the signing directory (status $status)"
fi
_case "$(_listing)" -- --testflight --i-know-what-i-am-doing
if [ "$status" = 1 ] && grep -qF "team-id" "$tmp/out" && grep -qF "clax-appstore.mobileprovision" "$tmp/out" &&
    grep -qF "api-key-id" "$tmp/out" && grep -qF "api-issuer-id" "$tmp/out" &&
    ! _acted >/dev/null && [ ! -e "$apple" ]; then
    _ok "a TestFlight run without the signing directory stops naming its missing files"
else
    _fail "a TestFlight run without the signing directory (status $status)"
fi
_case "$(_listing "$phone")" --
if [ "$status" = 0 ] && grep -qF "clax-development.mobileprovision" "$tmp/out" &&
    grep -qF "bazelisk build" "$tmp/out" && ! _acted >/dev/null; then
    _ok "the dry run without the signing directory names what is missing and still prints the steps"
else
    _fail "the dry run without the signing directory (status $status)"
fi

# 8. The real device run: build, install, launch, and the daemon of the
# launched PID; it writes nothing to the signing directory.
_apple
cat > "$tmp/daemon.json" <<'JSON'
{
  "port": 7555,
  "pid": 4242,
  "token": "t",
  "bind": "127.0.0.1"
}
JSON
_case "$(_listing "$phone" "$away")" -- --i-know-what-i-am-doing
if [ "$status" = 0 ] && _in_order "$tmp/calls" \
    "bazelisk build" "$dev_flags //apps/clax_app:ipa_dev" \
    "xcrun devicectl device install app --device AB-1 $repo/target/app-iphone/0123abcd/Payload/Clax.app" \
    "xcrun devicectl device process launch --device AB-1" \
    "xcrun devicectl device copy from --device AB-1" &&
    grep -qF "Library/Application Support/clax-home/daemon.json" "$tmp/calls" &&
    grep -qF "PID 4242" "$tmp/out" && grep -qF "Test iPhone" "$tmp/out" &&
    [ "$(ls -A "$apple" | wc -l | tr -d ' ')" = 6 ] && [ ! -e "$apple/builds" ]; then
    _ok "the device run builds, installs, launches and sees the launched PID's daemon"
else
    _fail "the device run (status $status)"
fi

# 9. No device until the build is done: it builds first, then waits for one.
_case "$(_listing)" "$(_listing)" "$(_listing "$phone")" -- --i-know-what-i-am-doing
if [ "$status" = 0 ] && _in_order "$tmp/calls" "bazelisk build" "xcrun devicectl list devices" \
    "xcrun devicectl device install app --device AB-1" &&
    grep -qF "needed once the build is done" "$tmp/out"; then
    _ok "with no device yet it builds first and then waits for one"
else
    _fail "waiting for a device (status $status)"
fi

# 10. Two devices: it stops before building, naming both; APP_IPHONE_DEVICE
# picks one by name.
_case "$(_listing "$phone" "$pad")" -- --i-know-what-i-am-doing
if [ "$status" = 1 ] && grep -qF "Test iPhone" "$tmp/out" && grep -qF "Second iPad" "$tmp/out" &&
    grep -qF "APP_IPHONE_DEVICE" "$tmp/out" && ! _acted >/dev/null; then
    _ok "two devices stop the run before building, naming both"
else
    _fail "two devices (status $status)"
fi
APP_IPHONE_DEVICE="Second iPad" _case "$(_listing "$phone" "$pad")" -- --i-know-what-i-am-doing
if [ "$status" = 0 ] && grep -qF "xcrun devicectl device install app --device AB-2" "$tmp/calls" &&
    ! grep -qF -- "--device AB-1" "$tmp/calls"; then
    _ok "APP_IPHONE_DEVICE picks the device"
else
    _fail "APP_IPHONE_DEVICE (status $status)"
fi

# 11. A daemon.json from an earlier launch (another PID) is not this launch's.
sed -i.bak 's/"pid": 4242/"pid": 17/' "$tmp/daemon.json"
APP_IPHONE_DAEMON_WAIT=0 _case "$(_listing "$phone")" -- --i-know-what-i-am-doing
if [ "$status" = 1 ] && grep -qF "did not come up" "$tmp/out"; then
    _ok "a daemon.json naming another PID is not this launch's daemon"
else
    _fail "a stale daemon.json (status $status)"
fi

# 12. Uncommitted changes: HEAD is built in a clean copy.
echo " M apps/clax_app/lib/main.dart" > "$tmp/git-status"
_case "$(_listing "$phone")" --
if [ "$status" = 0 ] && grep -qF "target/app-iphone/tree" "$tmp/out" && grep -qF "uncommitted" "$tmp/out"; then
    _ok "with uncommitted changes the dry run builds HEAD in target/app-iphone/tree"
else
    _fail "the dry run with uncommitted changes"
fi
: > "$tmp/git-status"

# 13. An unknown argument is a usage error.
_case "$(_listing)" -- --upload
if [ "$status" = 2 ] && grep -qF "usage:" "$tmp/out"; then
    _ok "an unknown argument is a usage error"
else
    _fail "an unknown argument (status $status)"
fi

if [ "$fails" -gt 0 ]; then
    echo "$fails case(s) failed" >&2
    exit 1
fi
echo "all cases passed"
