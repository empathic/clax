#!/usr/bin/env bash
# Packs the release build of the Clax Chrome extension for the Chrome Web
# Store (spec 2026-10-05 L15, §6.7): builds it (scripts/build-web.sh), then
# zips web/dist-extension into dist/clax-extension-<version>.zip and, with
# --crx, packs a signed dist/clax-extension-<version>.crx with Chromium's
# --pack-extension-key.
#
# The private key comes from the owner's 1Password (CLAX_EXTENSION_KEY_REF,
# its full `op://` reference; with the 1Password app's CLI integration, the
# app asks the owner to approve the read) into an owner-only temporary file
# that is removed on exit, interrupt or termination. It is never written
# into the repository and never printed. The build's manifest `key` must be
# the private key's public half (run scripts/extension-pubkey.sh, commit its
# output and rebuild first), so the listed ID is the one Clax derives.
#
# The zip's manifest.json is the build's without `key`: the Web Store
# refuses an upload whose manifest has one ("key field is not allowed in
# manifest"; chromium-extensions group, 2021-09-18, x_NBS6_-NKs;
# testomatio/browser-extension#391). The .crx keeps it.
#
# --first-upload adds the private key at the zip's root as key.pem, which
# the Web Store reads only on a listing's first upload to fix its ID to the
# key's (Chrome's packaging doc, "Upload a previously packaged extension");
# it re-signs every later release itself. That zip holds the private key, so
# it is written to a new owner-only directory outside the repository, to
# delete once uploaded; until the zip is complete, an exit, interrupt or
# termination removes that directory too.
#
# Signing is local and owner-run: the script refuses to run when CI is set.
# CLAX_CHROMIUM names the Chromium binary for --crx; otherwise Google Chrome
# or Chromium is looked for in /Applications and on PATH.
#
# Usage: CLAX_EXTENSION_KEY_REF=op://… scripts/pack-extension.sh [--first-upload] [--crx]
# Exits 2 on a usage error, when CI is set or CLAX_EXTENSION_KEY_REF is
# unset; 1 on any other failure.
set -euo pipefail

usage() { echo "usage: scripts/pack-extension.sh [--first-upload] [--crx]" >&2; exit 2; }
FIRST_UPLOAD=0
CRX=0
for arg in "$@"; do
    case "$arg" in
        --first-upload) FIRST_UPLOAD=1 ;;
        --crx) CRX=1 ;;
        *) usage ;;
    esac
done
if [ -n "${CI:-}" ]; then
    echo "pack-extension.sh: CI is set; signing is local and owner-run" >&2
    exit 2
fi
if [ -z "${CLAX_EXTENSION_KEY_REF:-}" ]; then
    echo "pack-extension.sh: set CLAX_EXTENSION_KEY_REF to the op:// reference of the extension's private key" >&2
    exit 2
fi
for tool in op openssl zip node; do
    command -v "$tool" > /dev/null || { echo "pack-extension.sh: $tool is not on PATH" >&2; exit 1; }
done

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BUILD="$ROOT/web/dist-extension"
OUT="$ROOT/dist"

CHROMIUM=""
if [ "$CRX" = 1 ]; then
    if [ -n "${CLAX_CHROMIUM:-}" ]; then
        CHROMIUM="$CLAX_CHROMIUM"
    else
        for c in "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
            "/Applications/Chromium.app/Contents/MacOS/Chromium" \
            google-chrome google-chrome-stable chromium chromium-browser; do
            if p="$(command -v "$c" 2> /dev/null)"; then CHROMIUM="$p"; break; fi
        done
    fi
    [ -n "$CHROMIUM" ] && [ -x "$CHROMIUM" ] || {
        echo "pack-extension.sh: --crx needs Chromium; set CLAX_CHROMIUM to its binary" >&2
        exit 1
    }
fi

"$ROOT/scripts/build-web.sh"

version="$(sed -n 's/^ *"version": *"\([^"]*\)",\{0,1\}$/\1/p' "$BUILD/manifest.json" | sed -n 1p)"
[ -n "$version" ] || { echo "pack-extension.sh: no version in $BUILD/manifest.json" >&2; exit 1; }
manifest_key="$(sed -n 's/^ *"key": *"\([^"]*\)",\{0,1\}$/\1/p' "$BUILD/manifest.json" | sed -n 1p)"

umask 077
SECRET="$(mktemp -d "${TMPDIR:-/tmp}/clax-extension-key.XXXXXX")"
key="$SECRET/key.pem"
upload=""
written=0
cleanup() {
    rm -rf "$SECRET"
    if [ -n "$upload" ] && [ "$written" = 0 ]; then rm -rf "$upload"; fi
}
trap cleanup EXIT
trap 'cleanup; exit 130' INT
trap 'cleanup; exit 143' TERM

# PKCS#8, the form Chromium reads with --pack-extension-key and writes as
# key.pem itself. openssl's stderr is dropped (a parse error); op's is the
# owner's to read.
if ! op read "$CLAX_EXTENSION_KEY_REF" | openssl pkcs8 -topk8 -nocrypt > "$key" 2> /dev/null \
    || [ ! -s "$key" ]; then
    echo "pack-extension.sh: no RSA private key came from 1Password at \$CLAX_EXTENSION_KEY_REF" >&2
    exit 1
fi
pub="$(openssl rsa -in "$key" -pubout -outform DER 2> /dev/null | openssl base64 -A)"
if [ "$manifest_key" != "$pub" ]; then
    echo "pack-extension.sh: the build's manifest key is not the 1Password key's public half;" >&2
    echo "run scripts/extension-pubkey.sh, commit web/extension/key/key.pub.b64 and pack again" >&2
    exit 1
fi

name="clax-extension-$version"
# The store's copy of the build: the manifest without `key`.
STORE="$SECRET/store"
cp -R "$BUILD" "$STORE"
rm -f "$STORE/.gitkeep"
node -e 'const fs = require("fs"); const f = process.argv[1];
const m = JSON.parse(fs.readFileSync(f, "utf8")); delete m.key;
fs.writeFileSync(f, JSON.stringify(m, null, 2) + "\n");' "$STORE/manifest.json"
if [ "$FIRST_UPLOAD" = 1 ]; then
    upload="$(mktemp -d "${TMPDIR:-/tmp}/clax-extension-upload.XXXXXX")"
    zipfile="$upload/$name.zip"
else
    mkdir -p "$OUT"
    zipfile="$OUT/$name.zip"
fi
rm -f "$zipfile.tmp"
(cd "$STORE" && zip -qrX "$zipfile.tmp" .)
if [ "$FIRST_UPLOAD" = 1 ]; then
    zip -qjX "$zipfile.tmp" "$key"
fi
mv -f "$zipfile.tmp" "$zipfile"
written=1
echo "Wrote $zipfile"
if [ "$FIRST_UPLOAD" = 1 ]; then
    echo "It holds the private key as key.pem: upload it as the listing's first version,"
    echo "then delete it: rm -r '$upload'"
fi

if [ "$CRX" = 1 ]; then
    # Chromium writes <dir>.crx beside the directory it packs, so it packs a
    # copy in the temporary directory, with a profile of its own.
    cp -R "$BUILD" "$SECRET/$name"
    rm -f "$SECRET/$name/.gitkeep"
    "$CHROMIUM" --no-first-run --no-default-browser-check --use-mock-keychain --password-store=basic \
        --user-data-dir="$SECRET/profile" --no-message-box \
        --pack-extension="$SECRET/$name" --pack-extension-key="$key" > /dev/null 2>&1 || true
    [ -s "$SECRET/$name.crx" ] || { echo "pack-extension.sh: $CHROMIUM wrote no .crx" >&2; exit 1; }
    mkdir -p "$OUT"
    mv -f "$SECRET/$name.crx" "$OUT/$name.crx"
    echo "Wrote $OUT/$name.crx"
fi
