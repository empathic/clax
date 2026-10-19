#!/usr/bin/env bash
# ios-app-check.sh APP_ZIP HOME_BUNDLE STATIC_LIB...: checks the Bazel-built
# iOS simulator app (APP_ZIP, rules_apple's .ipa of Clax.app) and the
# daemon's static libraries (STATIC_LIB, the device and simulator archives
# of //crates/clax-ios:clax_ios_builds).
#
# The app's Info.plist must carry the bundle ID dev.empathic.clax, the name
# Clax, pubspec.yaml's version ($VERSION_NAME, $VERSION_CODE: this is the iOS
# form of :version_sync), the minimum iOS $IOS_MINIMUM_OS, iPhone and iPad,
# App Transport Security's loopback keys (NSAllowsLocalNetworking, and
# localhost with its subdomains as an exception domain) and no arbitrary
# loads, no full-screen requirement and no background modes. The bundle
# must hold Flutter's two frameworks and the home bundle HOME_BUNDLE (the
# one //apps/clax_app:home_bundle built) in Flutter's assets. The main
# binary must be built for the iOS simulator (platform 7) at minos
# $IOS_MINIMUM_OS and export the daemon's four C functions. Every object of
# each STATIC_LIB that names an iOS must name one no newer than
# $IOS_MINIMUM_OS, so a mismatch fails here rather than warning at link.
set -euo pipefail
zip="$1" home="$2"
shift 2
fails=0
ok() { echo "PASS: $1"; }
bad() { echo "FAIL: $1"; fails=1; }
dir="$(mktemp -d "${TEST_TMPDIR:-${TMPDIR:-/tmp}}/ios-app.XXXXXX")"
trap 'rm -rf "$dir"' EXIT
unzip -q "$zip" -d "$dir"
app="$dir/Payload/Clax.app"
[ -d "$app" ] || { echo "FAIL: no Payload/Clax.app in $zip"; exit 1; }
info="$app/Info.plist"
raw() { plutil -extract "$1" raw -o - "$info" 2>/dev/null || true; }
has() { plutil -extract "$1" raw -o - "$info" > /dev/null 2>&1; }
want() { # want KEY VALUE NAME
    if [ "$(raw "$1")" = "$2" ]; then ok "$3"; else bad "$3: $1 is '$(raw "$1")', not '$2'"; fi
}
want CFBundleIdentifier dev.empathic.clax "bundle ID"
want CFBundleName Clax "bundle name"
want CFBundleDisplayName Clax "display name"
want CFBundleShortVersionString "$VERSION_NAME" "version name"
want CFBundleVersion "$VERSION_CODE" "build number"
want MinimumOSVersion "$IOS_MINIMUM_OS" "minimum iOS"
families="$(plutil -extract UIDeviceFamily json -o - "$info" 2>/dev/null | tr -d ' \n' || true)"
if [ "$families" = "[1,2]" ]; then ok "iPhone and iPad"; else bad "iPhone and iPad: UIDeviceFamily is '$families'"; fi
want NSAppTransportSecurity.NSAllowsLocalNetworking true "loopback over HTTP"
want NSAppTransportSecurity.NSExceptionDomains.localhost.NSIncludesSubdomains true "localhost's subdomains are an exception domain"
want NSAppTransportSecurity.NSExceptionDomains.localhost.NSExceptionAllowsInsecureHTTPLoads true "localhost's subdomains over HTTP"
for k in NSAppTransportSecurity.NSAllowsArbitraryLoads UIRequiresFullScreen UIBackgroundModes; do
    if has "$k"; then bad "no $k"; else ok "no $k"; fi
done
if plutil -convert xml1 -o - "$info" | grep -q '\$('; then
    bad "no Xcode variable left in Info.plist"
else
    ok "no Xcode variable left in Info.plist"
fi
for f in Frameworks/App.framework Frameworks/Flutter.framework; do
    if [ -d "$app/$f" ]; then ok "$f"; else bad "$f"; fi
done
assets="$app/Frameworks/App.framework/flutter_assets/assets/home"
if cmp -s "$assets/home.clax" "$home"; then ok "the home bundle built from the sources"; else bad "the home bundle built from the sources ($assets/home.clax and $home)"; fi
bin="$app/$(raw CFBundleExecutable)"
if [ -f "$bin" ]; then
    ok "executable"
    build="$(otool -l "$bin" | grep -A4 LC_BUILD_VERSION | awk '$1 == "platform" || $1 == "minos" { print $1, $2 }' | sort -u | tr '\n' ' ')"
    if [ "$build" = "minos $IOS_MINIMUM_OS platform 7 " ]; then ok "built for the iOS simulator at $IOS_MINIMUM_OS"; else bad "built for the iOS simulator at $IOS_MINIMUM_OS: $build"; fi
    exported="$(nm -gU "$bin" | awk '{ print $NF }')"
    for s in _clax_start _clax_stop _clax_state _clax_trim_memory; do
        if grep -qx "$s" <<<"$exported"; then ok "the binary has $s"; else bad "the binary has $s"; fi
    done
else
    bad "executable"
fi
# The objects of an archive with an iOS newer than the minimum: one
# "<member> <minos>" line each.
newer() {
    otool -l "$1" | awk -v min="$IOS_MINIMUM_OS" '
        function newer(v, m,   a, b, i, n) {
            split(v, a, "."); split(m, b, ".")
            for (i = 1; i <= 3; i++) {
                if ((a[i] + 0) > (b[i] + 0)) return 1
                if ((a[i] + 0) < (b[i] + 0)) return 0
            }
            return 0
        }
        /^[^ \t].*:$/ { member = $0 }
        $1 == "cmd" { build = ($2 == "LC_BUILD_VERSION") }
        build && $1 == "minos" { if (newer($2, min)) print member, $2; build = 0 }
    '
}
[ "$#" -gt 0 ] || bad "a static library to check"
for lib in "$@"; do
    late="$(newer "$lib")"
    objects="$(otool -l "$lib" | grep -c LC_BUILD_VERSION || true)"
    if [ "$objects" -eq 0 ]; then
        bad "$lib has objects for an iOS"
    elif [ -z "$late" ]; then
        ok "$lib: $objects objects for iOS $IOS_MINIMUM_OS or older"
    else
        bad "$lib has objects for an iOS newer than $IOS_MINIMUM_OS: $(head -3 <<<"$late")"
    fi
done
exit "$fails"
