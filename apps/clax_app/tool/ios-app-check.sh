#!/usr/bin/env bash
# ios-app-check.sh APP_ZIP HOME_BUNDLE STATIC_LIB...: checks the Bazel-built
# iOS simulator app (APP_ZIP, rules_apple's .ipa of Clax.app) and the
# core's static libraries (STATIC_LIB, the device and simulator archives
# of //crates/clax-embed:clax_embed_ios_builds).
#
# The app's Info.plist must carry the bundle ID dev.empathic.clax, the name
# Clax, pubspec.yaml's version ($VERSION_NAME, $VERSION_CODE: this is the iOS
# form of :version_sync), the minimum iOS $IOS_MINIMUM_OS, iPhone and iPad,
# no App Transport Security exceptions (the WebViews load from the app's
# scheme, not over HTTP), no full-screen requirement and no background
# modes, the .clax
# document type (dev.empathic.clax.bundle, exported, copied in), the clax:
# URL scheme with Flutter's deep linking off, and the
# words of each permission prompt the plugins can raise (camera, photos,
# microphone, speech recognition, location and precise location). The
# bundle must hold Flutter's two frameworks, objective_c's (path_provider's
# FFI), the privacy manifest of each plugin that ships one, the app's own
# privacy manifest (no tracking, the person's requests and app code as
# other user content, and a reason for exactly the required-reason API
# categories the main binary imports), and the home
# bundle HOME_BUNDLE (the one //apps/clax_app:home_bundle built) in
# Flutter's assets, and the App Intents metadata that lists "Comment in
# Clax" (its title and the intent's type) for Shortcuts, Siri and the Action
# Button. The main binary must be built for the iOS simulator
# (platform 7) at minos $IOS_MINIMUM_OS, export the core's four C
# functions (clax_embed_*) and none of the old daemon's (clax_start,
# clax_stop, clax_state, clax_trim_memory), register every plugin with a native iOS half, and hold the
# background channel's name and its tasks' names, and the bundles and
# intents channels'. Every object of each STATIC_LIB that names an iOS must name one
# no newer than $IOS_MINIMUM_OS, so a mismatch fails here rather than
# warning at link.
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
for k in NSAppTransportSecurity UIRequiresFullScreen UIBackgroundModes; do
    if has "$k"; then bad "no $k"; else ok "no $k"; fi
done
if plutil -convert xml1 -o - "$info" | grep '\$(' >/dev/null; then
    bad "no Xcode variable left in Info.plist"
else
    ok "no Xcode variable left in Info.plist"
fi
# .clax bundles: the exported type, Clax as its owner, and a copy handed
# over rather than the file opened in place.
want CFBundleDocumentTypes.0.LSItemContentTypes.0 dev.empathic.clax.bundle "Clax opens dev.empathic.clax.bundle"
want CFBundleDocumentTypes.0.LSHandlerRank Owner "Clax owns .clax files"
want UTExportedTypeDeclarations.0.UTTypeIdentifier dev.empathic.clax.bundle "dev.empathic.clax.bundle is exported"
conforms="$(plutil -extract UTExportedTypeDeclarations.0.UTTypeConformsTo json -o - "$info" 2>/dev/null | tr -d ' \n' || true)"
if [ "$conforms" = '["public.data"]' ]; then ok "dev.empathic.clax.bundle is data, not a zip Files would expand"; else bad "dev.empathic.clax.bundle is data, not a zip Files would expand: UTTypeConformsTo is '$conforms'"; fi
tags="$(plutil -extract UTExportedTypeDeclarations.0.UTTypeTagSpecification json -o - "$info" 2>/dev/null | tr -d ' \n' || true)"
for t in '"public.filename-extension":["clax"]' '"public.mime-type":["application\/vnd.empathic.clax+zip"]'; do
    if grep -qF "$t" <<<"$tags"; then ok "dev.empathic.clax.bundle's tag $t"; else bad "dev.empathic.clax.bundle's tag $t: UTTypeTagSpecification is '$tags'"; fi
done
want LSSupportsOpeningDocumentsInPlace false "a .clax is copied in, not opened in place"
# Get in Clax: clax: links open in Clax, and reach Dart on the bundles
# channel rather than as a route Flutter pushes.
want CFBundleURLTypes.0.CFBundleURLSchemes.0 clax "Clax opens clax: links"
want FlutterDeepLinkingEnabled false "links are not pushed as Flutter routes"
# The permission prompts: iOS shows each string under its question, and
# ends the app on a request whose key is missing.
want NSCameraUsageDescription "An app you made in Clax can take a photo when you let it." "the camera prompt's words"
want NSPhotoLibraryUsageDescription "An app you made in Clax can use a photo you pick." "the photos prompt's words"
want NSMicrophoneUsageDescription "Clax listens while you speak a request or a comment." "the microphone prompt's words"
want NSSpeechRecognitionUsageDescription "Clax turns what you say into text for your request or comment." "the speech recognition prompt's words"
want NSLocationWhenInUseUsageDescription "An app you made in Clax can use your location when you let it." "the location prompt's words"
want NSLocationTemporaryUsageDescriptionDictionary.precise "This app asked for your precise location." "the precise location prompt's words"
for f in Frameworks/App.framework Frameworks/Flutter.framework Frameworks/objective_c.framework; do
    if [ -d "$app/$f" ]; then ok "$f"; else bad "$f"; fi
done
# Each plugin's privacy manifest, at the bundle's root under a name that
# keeps its package (rules_flutter's flutter_ios_privacy_manifests).
for p in file_picker flutter_local_notifications flutter_secure_storage flutter_timezone \
    geolocator image_picker package_info_plus share_plus webview_flutter; do
    m="$(find "$app" -maxdepth 1 -name "*_pub__${p}_*PrivacyInfo.xcprivacy" | sed -n 1p)"
    if [ -n "$m" ] && plutil -lint -s "$m"; then ok "$p's privacy manifest"; else bad "$p's privacy manifest"; fi
done
assets="$app/Frameworks/App.framework/flutter_assets/assets/home"
if cmp -s "$assets/home.clax" "$home"; then ok "the home bundle built from the sources"; else bad "the home bundle built from the sources ($assets/home.clax and $home)"; fi
# Metadata.appintents (rules_apple's app_intents, from ios/Intents): the
# system reads the app's intents from it without running the app.
actions="$app/Metadata.appintents/extract.actionsdata"
if [ -f "$actions" ]; then
    ok "Metadata.appintents"
    for t in CommentInClax "Comment in Clax"; do
        if grep -qF "\"$t\"" "$actions"; then ok "Metadata.appintents names \"$t\""; else bad "Metadata.appintents names \"$t\""; fi
    done
else
    bad "Metadata.appintents"
fi
bin="$app/$(raw CFBundleExecutable)"
if [ -f "$bin" ]; then
    ok "executable"
    build="$(otool -l "$bin" | grep -A4 LC_BUILD_VERSION | awk '$1 == "platform" || $1 == "minos" { print $1, $2 }' | sort -u | tr '\n' ' ')"
    if [ "$build" = "minos $IOS_MINIMUM_OS platform 7 " ]; then ok "built for the iOS simulator at $IOS_MINIMUM_OS"; else bad "built for the iOS simulator at $IOS_MINIMUM_OS: $build"; fi
    exported="$(nm -gU "$bin" | awk '{ print $NF }')"
    for s in _clax_embed_open _clax_embed_call _clax_embed_subscribe _clax_embed_close; do
        if grep -qx "$s" <<<"$exported"; then ok "the binary has $s"; else bad "the binary has $s"; fi
    done
    for s in _clax_start _clax_stop _clax_state _clax_trim_memory; do
        if grep -qx "$s" <<<"$exported"; then bad "the binary has no $s (the old daemon's)"; else ok "the binary has no $s"; fi
    done
    texts="$(strings -a "$bin")"
    # The registrant rules_flutter generates: one registrar per plugin with
    # a native iOS half (path_provider's is FFI, through objective_c).
    for p in FilePickerPlugin FlutterLocalNotificationsPlugin FlutterSecureStorageDarwinPlugin \
        FlutterTimezonePlugin GeolocatorPlugin FPPPackageInfoPlusPlugin FLTImagePickerPlugin \
        FPPSharePlusPlugin SpeechToTextPlugin WebViewFlutterPlugin; do
        if grep -qxF "$p" <<<"$texts"; then ok "the plugin $p is registered"; else bad "the plugin $p is registered"; fi
    done
    # ios/Runner/Background.swift: its channel and its two background tasks.
    # ios/Runner/BundleIntake.swift: the bundles channel.
    # ios/Runner/IntentsChannel.swift: the intents channel.
    for t in dev.empathic.clax/background "Clax trim" "Clax run" dev.empathic.clax/bundles \
        dev.empathic.clax/intents; do
        if grep -qxF "$t" <<<"$texts"; then ok "the binary holds \"$t\""; else bad "the binary holds \"$t\""; fi
    done
else
    bad "executable"
fi
# The app's own privacy manifest (ios/Runner/PrivacyInfo.xcprivacy): it
# tracks no one, declares what goes to Anthropic, and gives a reason for
# exactly the required-reason API categories the main binary links (the
# plugins and Flutter.framework declare their own).
privacy="$app/PrivacyInfo.xcprivacy"
if [ -f "$privacy" ] && plutil -lint -s "$privacy"; then
    ok "the app's privacy manifest"
    praw() { plutil -extract "$1" raw -o - "$privacy" 2>/dev/null || true; }
    pwant() { # pwant KEY VALUE NAME
        if [ "$(praw "$1")" = "$2" ]; then ok "$3"; else bad "$3: $1 is '$(praw "$1")', not '$2'"; fi
    }
    pwant NSPrivacyTracking false "the app tracks no one"
    pwant NSPrivacyTrackingDomains 0 "no tracking domains"
    # One collected type: the person's requests and app code, which go to
    # Anthropic under the person's own key.
    pwant NSPrivacyCollectedDataTypes 1 "one collected data type"
    pwant NSPrivacyCollectedDataTypes.0.NSPrivacyCollectedDataType NSPrivacyCollectedDataTypeOtherUserContent "requests and app code are other user content"
    pwant NSPrivacyCollectedDataTypes.0.NSPrivacyCollectedDataTypeLinked false "other user content is not linked to the person"
    pwant NSPrivacyCollectedDataTypes.0.NSPrivacyCollectedDataTypeTracking false "other user content is not used to track"
    pwant NSPrivacyCollectedDataTypes.0.NSPrivacyCollectedDataTypePurposes 1 "other user content has one purpose"
    pwant NSPrivacyCollectedDataTypes.0.NSPrivacyCollectedDataTypePurposes.0 NSPrivacyCollectedDataTypePurposeAppFunctionality "other user content is for the app's function"
    if [ -f "$bin" ]; then
        # Each required-reason symbol the binary imports, by category and the
        # one reason Clax gives for it: file timestamps inside the container
        # (SQLite, the store, package_info_plus's install date), elapsed time,
        # free space before a write (SQLite), the app's own defaults.
        linked="$(nm -u "$bin" | awk '
            $1 ~ /^_(stat|fstat|fstatat|lstat|getattrlist|getattrlistbulk|fgetattrlist|getattrlistat|NSFileCreationDate|NSFileModificationDate|NSURLContentModificationDateKey|NSURLCreationDateKey)$/ { print "FileTimestamp C617.1" }
            $1 ~ /^_(mach_absolute_time)$/ { print "SystemBootTime 35F9.1" }
            $1 ~ /^_(statfs|fstatfs|statvfs|fstatvfs|NSFileSystemFreeSize|NSFileSystemSize|NSURLVolumeAvailableCapacityKey|NSURLVolumeAvailableCapacityForImportantUsageKey|NSURLVolumeAvailableCapacityForOpportunisticUsageKey|NSURLVolumeTotalCapacityKey)$/ { print "DiskSpace E174.1" }
            $1 == "_OBJC_CLASS_$_NSUserDefaults" { print "UserDefaults CA92.1" }
        ' | sort -u | paste -sd ' ' -)"
        declared="$(
            i=0
            while t="$(plutil -extract "NSPrivacyAccessedAPITypes.$i.NSPrivacyAccessedAPIType" raw -o - "$privacy" 2>/dev/null)"; do
                j=0
                while r="$(plutil -extract "NSPrivacyAccessedAPITypes.$i.NSPrivacyAccessedAPITypeReasons.$j" raw -o - "$privacy" 2>/dev/null)"; do
                    echo "${t#NSPrivacyAccessedAPICategory} $r"
                    j=$((j + 1))
                done
                i=$((i + 1))
            done | sort -u | paste -sd ' ' -
        )"
        if [ -n "$linked" ] && [ "$declared" = "$linked" ]; then
            ok "the privacy manifest declares the required-reason APIs the binary links: $linked"
        else
            bad "the privacy manifest declares the required-reason APIs the binary links: it declares '$declared', the binary links '$linked'"
        fi
    fi
else
    bad "the app's privacy manifest"
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
