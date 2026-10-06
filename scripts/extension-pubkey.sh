#!/usr/bin/env bash
# Writes the Clax Chrome extension's public key to
# web/extension/key/key.pub.b64 (one line of base64 SubjectPublicKeyInfo
# DER), derived from the private key in the owner's 1Password, and prints
# the extension ID it gives (spec 2026-10-05 L15, §6.7).
#
# CLAX_EXTENSION_KEY_REF is the private key's full `op://` reference. The key
# passes only through a pipe from `op read` to `openssl`; nothing else reads
# it and nothing writes it to disk. 1Password asks the owner to approve the
# read. Signing is local and owner-run: the script refuses to run when CI is
# set.
#
# Usage: CLAX_EXTENSION_KEY_REF=op://… scripts/extension-pubkey.sh
# Exits 2 when CI is set or CLAX_EXTENSION_KEY_REF is unset, 1 on any other
# failure.
set -euo pipefail

if [ -n "${CI:-}" ]; then
    echo "extension-pubkey.sh: CI is set; signing is local and owner-run" >&2
    exit 2
fi
if [ -z "${CLAX_EXTENSION_KEY_REF:-}" ]; then
    echo "extension-pubkey.sh: set CLAX_EXTENSION_KEY_REF to the op:// reference of the extension's private key" >&2
    exit 2
fi
for tool in op openssl; do
    command -v "$tool" > /dev/null || { echo "extension-pubkey.sh: $tool is not on PATH" >&2; exit 1; }
done

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/web/extension/key/key.pub.b64"

# openssl's stderr is dropped ("writing RSA key", or a parse error); op's is
# the owner's to read.
if ! pub="$(op read "$CLAX_EXTENSION_KEY_REF" | openssl rsa -pubout -outform DER 2> /dev/null | openssl base64 -A)" \
    || [ -z "$pub" ]; then
    echo "extension-pubkey.sh: no RSA private key came from 1Password at \$CLAX_EXTENSION_KEY_REF" >&2
    exit 1
fi

# Chromium's ID for a manifest key, as clax_core::extension::extension_id_from_key
# derives it: the first 128 bits of the DER's SHA-256, each nibble written as
# a letter a–p.
id="$(printf '%s' "$pub" | openssl base64 -d -A | openssl dgst -sha256 -r | cut -c1-32 | tr 0-9a-f a-p)"

before=""
[ -f "$OUT" ] && before="$(tr -d '\n' < "$OUT")"
printf '%s\n' "$pub" > "$OUT.tmp"
mv -f "$OUT.tmp" "$OUT"

echo "Wrote web/extension/key/key.pub.b64"
echo "Extension ID: $id"
if [ "$before" = "$pub" ]; then
    echo "The public key is unchanged."
else
    echo "Commit web/extension/key/key.pub.b64, then run \`clax init\`: the extension's ID"
    echo "changes once, so each person loads ~/.clax/extension unpacked again."
fi
