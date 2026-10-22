#!/usr/bin/env bash
# The signing inputs' repository (bazel/apple/signing.bzl) against a fake
# directory: a complete one gives entitlements naming the team, a missing
# profile is named, and a directory others can read is refused with its
# mode. Then this machine's own directory, when it exists, must be
# complete and owner-only.
#
# It runs Bazel on a scratch workspace that holds only signing.bzl, with
# an output base of its own, so it never waits on the server that runs it.
#
#   bazel/apple/signing-test.sh [signing.bzl] [.bazelversion]
#
# Under `bazelisk test //bazel/apple:signing_test` the arguments come from
# the BUILD file. Bazel is $BAZEL, else bazelisk or bazel on PATH or in
# the usual install directories.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
bzl=${1:-$here/signing.bzl}
version_file=${2:-$here/../../.bazelversion}

bazel=${BAZEL:-}
if [ -z "$bazel" ]; then
    for c in bazelisk bazel; do
        bazel=$(command -v "$c" || true)
        [ -n "$bazel" ] && break
    done
fi
if [ -z "$bazel" ]; then
    for d in "${HOME:-/nonexistent}/.local/homebrew/bin" /opt/homebrew/bin /usr/local/bin; do
        for c in bazelisk bazel; do
            if [ -x "$d/$c" ]; then bazel=$d/$c; break 2; fi
        done
    done
fi
if [ -z "$bazel" ]; then
    echo "signing-test: no bazelisk or bazel found; set BAZEL" >&2
    exit 1
fi

tmp=${TMPDIR:-/tmp}
scratch=$(mktemp -d "${tmp%/}/clax-signing.XXXXXX")
ws=$scratch/ws
out=$scratch/out
apple=$scratch/apple
cleanup() {
    "$bazel" --output_base="$out" shutdown >/dev/null 2>&1 || true
    chmod -R u+w "$scratch" 2>/dev/null || true
    rm -rf "$scratch"
}
trap cleanup EXIT

mkdir -p "$ws/bazel/apple"
cp "$bzl" "$ws/bazel/apple/signing.bzl"
cp "$version_file" "$ws/.bazelversion"
: >"$ws/bazel/apple/BUILD.bazel"
cat >"$ws/MODULE.bazel" <<'EOF'
module(name = "signing_test")

signing = use_extension("//bazel/apple:signing.bzl", "signing")
use_repo(signing, "clax_apple_signing")
EOF

# A complete directory: owner-only, as the rule requires.
mkdir -m 700 "$apple"
printf 'ABCDE12345\n' >"$apple/team-id"
printf 'a development profile\n' >"$apple/clax-development.mobileprovision"
printf 'an App Store profile\n' >"$apple/clax-appstore.mobileprovision"
printf -- '-----BEGIN PRIVATE KEY-----\n' >"$apple/AuthKey_KEY1234567.p8"
printf 'KEY1234567\n' >"$apple/api-key-id"
printf '69a6de7f-0000-47e3-e053-5b8c7c11a4d1\n' >"$apple/api-issuer-id"
chmod 600 "$apple"/*

log=$scratch/log
build() {
    (cd "$ws" && CLAX_APPLE_DIR="$apple" "$bazel" --ignore_all_rc_files \
        --output_base="$out" --max_idle_secs=60 \
        build --noshow_progress "$@") >"$log" 2>&1
}
fails=0
check() {
    if ! "$@"; then
        echo "FAIL: $*" >&2
        sed 's/^/    /' "$log" >&2
        fails=$((fails + 1))
    fi
}
names() { grep -qF -- "$1" "$log"; }
refetch() {
    (cd "$ws" && "$bazel" --ignore_all_rc_files --output_base="$out" \
        fetch --force --repo=@clax_apple_signing) >"$log" 2>&1
}

# 1. Complete: the development entitlements name the team and the app.
check build @clax_apple_signing//:entitlements_development \
    @clax_apple_signing//:entitlements_appstore @clax_apple_signing//:development \
    @clax_apple_signing//:appstore @clax_apple_signing//:api_key @clax_apple_signing//:team_id
dev=$out/external/+signing+clax_apple_signing/entitlements_development.plist
store=$out/external/+signing+clax_apple_signing/entitlements_appstore.plist
check grep -q '<string>ABCDE12345\.\$(CFBundleIdentifier)</string>' "$dev"
check grep -q '<string>ABCDE12345</string>' "$dev"
check grep -q '<key>get-task-allow</key>' "$dev"
check grep -q '<string>ABCDE12345\.\$(CFBundleIdentifier)</string>' "$store"
if grep -q 'get-task-allow' "$store"; then
    echo "FAIL: the App Store entitlements carry get-task-allow" >&2
    fails=$((fails + 1))
fi

# 2. The development profile missing: a development build names it, and
# the guide's section; the App Store profile is still there.
rm "$apple/clax-development.mobileprovision"
if build @clax_apple_signing//:entitlements_development; then
    echo "FAIL: built the development entitlements without the development profile" >&2
    fails=$((fails + 1))
fi
check names "clax-development.mobileprovision"
check names "$apple"
check names 'docs/usage.md, "Signing for a device and TestFlight"'
check build @clax_apple_signing//:appstore

# 3. Nothing at all: every file a TestFlight build needs is named.
mv "$apple" "$apple.away"
if build @clax_apple_signing//:appstore; then
    echo "FAIL: built the App Store profile without the directory" >&2
    fails=$((fails + 1))
fi
for f in team-id clax-appstore.mobileprovision 'AuthKey_<KEYID>.p8' api-key-id api-issuer-id; do
    check names "$f"
done
mv "$apple.away" "$apple"

# 4. The profile back: it builds. Then the directory readable by others:
# refused, naming its mode; and owner-only again: it builds. Bazel does not
# see a directory's own mode change, so each is followed by the fetch the
# refusal names (a server start does the same).
printf 'a development profile\n' >"$apple/clax-development.mobileprovision"
chmod 600 "$apple/clax-development.mobileprovision"
check build @clax_apple_signing//:entitlements_development
chmod 755 "$apple"
refetch
if build @clax_apple_signing//:entitlements_development; then
    echo "FAIL: built with the signing directory at mode 0755" >&2
    fails=$((fails + 1))
fi
check names "$apple is mode 0755"
check names "bazelisk fetch --force --repo=@clax_apple_signing"
chmod 700 "$apple"
refetch
check build @clax_apple_signing//:entitlements_development

# 5. A key readable by the group: refused, naming the file.
chmod 640 "$apple/AuthKey_KEY1234567.p8"
if build @clax_apple_signing//:api_key; then
    echo "FAIL: built with the API key at mode 0640" >&2
    fails=$((fails + 1))
fi
check names "$apple/AuthKey_KEY1234567.p8 is mode 0640"
chmod 600 "$apple/AuthKey_KEY1234567.p8"

# 6. Fixed again: it builds.
check build @clax_apple_signing//:entitlements_development @clax_apple_signing//:api_key

# 7. This machine's own directory, once it exists: every input is in place
# and none is refused.
real=${HOME:-/nonexistent}/.config/clax/apple
if [ -d "$real" ]; then
    if ! (cd "$ws" && env -u CLAX_APPLE_DIR "$bazel" --ignore_all_rc_files \
        --output_base="$out" --max_idle_secs=60 build --noshow_progress \
        @clax_apple_signing//...) >"$log" 2>&1; then
        echo "FAIL: the signing inputs in $real" >&2
        sed 's/^/    /' "$log" >&2
        fails=$((fails + 1))
    fi
else
    echo "signing-test: $real does not exist; its inputs were not checked"
fi

if [ "$fails" -ne 0 ]; then
    echo "signing-test: $fails check(s) failed" >&2
    exit 1
fi
echo "signing-test: ok"
