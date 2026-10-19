#!/usr/bin/env bash
# Usage: check-build-version.sh <archive.a> <platform> <minos> <stamp>
#
# Fails unless every Mach-O object in <archive.a> carries exactly one
# LC_BUILD_VERSION (or LC_VERSION_MIN_IPHONEOS) whose platform is <platform> (2 iOS, 7 the iOS
# simulator; <mach-o/loader.h>) and whose minos is no newer than <minos>,
# the app's minimum: `ld` only warns on a newer one. Members that are not
# objects are skipped. On success it writes each object's member name,
# count, platform and minos to <stamp>.
set -euo pipefail

archive="$1"
platform="$2"
minos="$3"
stamp="$4"

listing="$(mktemp)"
trap 'rm -f "$listing"' EXIT
xcrun otool -l "$archive" >"$listing"

# One line per object: "<member> <count> <platforms> <minoses>". Rust's
# prebuilt compiler-builtins objects carry Rust's own minimum for the
# triple (14.0 for the simulator, LC_VERSION_MIN_IPHONEOS 10.0 for a
# device in Rust 1.94); everything Bazel compiles carries --ios_minimum_os.
report="$(awk '
    function flush() {
        if (member != "") print member, count, (plats == "" ? "-" : plats), (mins == "" ? "-" : mins)
        member = ""; count = 0; plats = ""; mins = ""
    }
    /^Archive : / { next }
    /\): is not an object file$/ { flush(); next }
    /^[^ ].*\(.*\):$/ {
        flush()
        member = $0
        sub(/^.*\(/, "", member); sub(/\):$/, "", member)
        next
    }
    # LC_VERSION_MIN_IPHONEOS, the older form, names iOS (2) and its
    # minimum as "version".
    $1 == "cmd" {
        inbuild = ($2 == "LC_BUILD_VERSION")
        inmin = ($2 == "LC_VERSION_MIN_IPHONEOS")
        if (inbuild || inmin) count++
        if (inmin) plats = plats (plats == "" ? "" : ",") "2"
        next
    }
    inbuild && $1 == "platform" { plats = plats (plats == "" ? "" : ",") $2; next }
    inbuild && $1 == "minos" { mins = mins (mins == "" ? "" : ",") $2; next }
    inmin && $1 == "version" { mins = mins (mins == "" ? "" : ",") $2; next }
    END { flush() }
' "$listing")"

if [[ -z "$report" ]]; then
    echo "error: $archive has no Mach-O objects" >&2
    exit 1
fi

# Dotted versions compared a component at a time: 9.0 is older than 17.0.
bad="$(awk -v p="$platform" -v m="$minos" '
    function newer(a, b,    x, y, n, i) {
        n = split(a, x, "."); split(b, y, ".")
        if (length(y) > n) n = length(y)
        for (i = 1; i <= n; i++) {
            if ((x[i] + 0) > (y[i] + 0)) return 1
            if ((x[i] + 0) < (y[i] + 0)) return 0
        }
        return 0
    }
    $2 != 1 || $3 != p || $4 == "-" || newer($4, m)
' <<<"$report")"
if [[ -n "$bad" ]]; then
    echo "error: $archive has objects not built for platform $platform at minos $minos (--ios_minimum_os) or older:" >&2
    echo "member, LC_BUILD_VERSION count, platform, minos:" >&2
    echo "$bad" >&2
    exit 1
fi

printf '%s\n' "$report" >"$stamp"
