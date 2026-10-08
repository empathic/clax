#!/usr/bin/env bash
# Hermetic tests for extension-pubkey.sh and pack-extension.sh (spec
# 2026-10-05 §6.7). Copies of both scripts run in a scratch repository whose
# scripts/build-web.sh is a fake that writes a release build, with a fake
# `op` first on PATH: it serves a throwaway RSA key generated here, only to
# `op read "$CLAX_EXTENSION_KEY_REF"`, and records every call. No 1Password,
# no real repository files. The scripts' TMPDIR is a scratch directory, which
# the leak checks scan with the scratch repository.
set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$(mktemp -d)" && pwd -P)"
BGPID=""
# shellcheck disable=SC2329 # run by the EXIT trap
cleanup() {
    if [ -n "$BGPID" ]; then kill -KILL -- "-$BGPID" 2> /dev/null; wait "$BGPID" 2> /dev/null; fi
    rm -rf "$ROOT"
    return 0
}
trap cleanup EXIT
# shellcheck source=scripts/fake-exe.sh
. "$HERE/fake-exe.sh"
FAILED=0
pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1"; FAILED=1; }

# The 1Password stand-in: a key the scripts may read only through the fake.
VAULT="$ROOT/vault"
mkdir -p "$VAULT"
openssl genrsa 2048 2> /dev/null > "$VAULT/key.pem"
REF="op://Test Vault/Clax extension key/private key"
# A line of the key's body: any file or output holding it holds the key.
KEYLINE="$(sed -n 2p "$VAULT/key.pem")"
# The key in PKCS#8, as the scripts write it for Chromium.
PKCS8_KEYLINE="$(openssl pkcs8 -topk8 -nocrypt < "$VAULT/key.pem" | sed -n 2p)"
PUB="$(openssl rsa -in "$VAULT/key.pem" -pubout -outform DER 2> /dev/null | base64 | tr -d '\n')"

BIN="$ROOT/bin"
fake_exe "$BIN/op" <<'FAKE'
#!/bin/sh
printf '%s\n' "$*" >> "$FAKE_OP_LOG"
if [ "$#" = 2 ] && [ "$1" = read ] && [ "$2" = "$FAKE_OP_REF" ]; then
    exec cat "$FAKE_OP_KEY"
fi
echo "fake op: unexpected call: $*" >&2
exit 1
FAKE
# A zip that does its work with the real zip, then, on a call whose
# arguments hold $FAKE_ZIP_HANG_ON (any call when it is empty), tells the
# test it started and waits to be interrupted.
REAL_ZIP="$(command -v zip)"
fake_exe "$ROOT/hang/zip" <<'FAKE'
#!/bin/sh
"$FAKE_REAL_ZIP" "$@" || exit
case "$*" in *"$FAKE_ZIP_HANG_ON"*) ;; *) exit 0 ;; esac
echo started > "$FAKE_ZIP_FIFO"
exec sleep 600
FAKE

# A scratch repository: copies of the two scripts and a fake build-web.sh that
# writes a minimal release build whose manifest carries the committed public
# key, as web/scripts/extension-manifest.mjs does.
REPO="$ROOT/repo"
mkdir -p "$REPO/scripts" "$REPO/web/extension/key"
cp "$HERE/extension-pubkey.sh" "$HERE/pack-extension.sh" "$REPO/scripts/"
cat > "$REPO/scripts/build-web.sh" <<'BUILD'
#!/bin/sh
set -e
web="$(cd "$(dirname "$0")/../web" && pwd)"
echo built >> "$web/../build.log"
rm -rf "$web/dist-extension"
mkdir -p "$web/dist-extension"
touch "$web/dist-extension/.gitkeep"
key=""
[ -f "$web/extension/key/key.pub.b64" ] && key="$(tr -d '\n' < "$web/extension/key/key.pub.b64")"
{
    printf '{\n  "manifest_version": 3,\n  "name": "Clax",\n  "version": "1.2.3",\n'
    [ -n "$key" ] && printf '  "key": "%s",\n' "$key"
    printf '  "background": {"service_worker": "sw.js"}\n}\n'
} > "$web/dist-extension/manifest.json"
echo '// worker' > "$web/dist-extension/sw.js"
BUILD
chmod +x "$REPO/scripts/build-web.sh"
SCRIPT_TMP="$ROOT/tmp"
mkdir -p "$SCRIPT_TMP"
OUT="$ROOT/out"
mkdir -p "$OUT"
LOG="$ROOT/op.log"

# run NAME SCRIPT ARGS...: runs a scratch copy with the fake op, the scratch
# TMPDIR and CI unset; stdout and stderr go to $OUT/NAME.{out,err}, the exit
# status to $STATUS.
run() {
    local name="$1" script="$2"
    shift 2
    env -u CI PATH="$BIN:$PATH" TMPDIR="$SCRIPT_TMP" CLAX_EXTENSION_KEY_REF="$REF" \
        FAKE_OP_LOG="$LOG" FAKE_OP_REF="$REF" FAKE_OP_KEY="$VAULT/key.pem" \
        "$REPO/scripts/$script" "$@" > "$OUT/$name.out" 2> "$OUT/$name.err"
    STATUS=$?
}
# mode_of FILE: its permission bits in octal. GNU stat first: BSD stat has no
# -c and fails before printing, while GNU's -f is --file-system and would
# print a block before failing on %Lp.
mode_of() { stat -c %a "$1" 2> /dev/null || stat -f %Lp "$1"; }
op_calls() { if [ -f "$LOG" ]; then wc -l < "$LOG" | tr -d ' '; else echo 0; fi; }
# no_key_left CASE: no file under the scratch repository or the scripts'
# TMPDIR holds the private key, and no script output does.
no_key_left() {
    local hits
    hits="$(grep -rlF -e "$KEYLINE" -e "$PKCS8_KEYLINE" "$REPO" "$SCRIPT_TMP" "$OUT" 2> /dev/null)"
    if [ -z "$hits" ] && [ -z "$(find "$REPO" "$SCRIPT_TMP" -name '*.pem' 2> /dev/null)" ]; then
        pass "$1: no copy of the private key is left, and none was printed"
    else
        fail "$1: the private key is left in: $hits $(find "$REPO" "$SCRIPT_TMP" -name '*.pem')"
    fi
}

# Chromium's ID for a manifest key, computed apart from the script, by the
# rule that crates/clax-core/src/extension.rs pins (the key "AAAA" has the ID
# hajoiamiieihkcebbobooenpljpcckig).
id_of() {
    python3 -c 'import base64, hashlib, sys
d = hashlib.sha256(base64.b64decode(sys.argv[1])).hexdigest()[:32]
print("".join(chr(ord("a") + int(c, 16)) for c in d))' "$1"
}
if [ "$(id_of AAAA)" = hajoiamiieihkcebbobooenpljpcckig ]; then
    pass "the test's ID rule gives clax-core's pinned ID"
else
    fail "the test's ID rule gives $(id_of AAAA) for AAAA"
fi

# --- refusals -----------------------------------------------------------------

for script in extension-pubkey.sh pack-extension.sh; do
    rm -f "$LOG"
    env -u CI -u CLAX_EXTENSION_KEY_REF PATH="$BIN:$PATH" TMPDIR="$SCRIPT_TMP" \
        FAKE_OP_LOG="$LOG" FAKE_OP_REF="$REF" FAKE_OP_KEY="$VAULT/key.pem" \
        "$REPO/scripts/$script" > "$OUT/unset.out" 2> "$OUT/unset.err"
    status=$?
    if [ "$status" = 2 ] && [ "$(op_calls)" = 0 ] && grep -q CLAX_EXTENSION_KEY_REF "$OUT/unset.err"; then
        pass "$script: without CLAX_EXTENSION_KEY_REF it exits 2, names it and calls no op"
    else
        fail "$script: without CLAX_EXTENSION_KEY_REF: exit $status, $(op_calls) op calls, stderr: $(cat "$OUT/unset.err")"
    fi
    rm -f "$LOG"
    env CI=true PATH="$BIN:$PATH" TMPDIR="$SCRIPT_TMP" CLAX_EXTENSION_KEY_REF="$REF" \
        FAKE_OP_LOG="$LOG" FAKE_OP_REF="$REF" FAKE_OP_KEY="$VAULT/key.pem" \
        "$REPO/scripts/$script" > "$OUT/ci.out" 2> "$OUT/ci.err"
    status=$?
    if [ "$status" = 2 ] && [ "$(op_calls)" = 0 ] && grep -q "owner-run" "$OUT/ci.err"; then
        pass "$script: with CI set it refuses (exit 2) and calls no op"
    else
        fail "$script: with CI set: exit $status, $(op_calls) op calls, stderr: $(cat "$OUT/ci.err")"
    fi
done
if [ ! -e "$REPO/build.log" ]; then
    pass "pack-extension.sh builds nothing when it refuses"
else
    fail "pack-extension.sh built before refusing"
fi

# --- extension-pubkey.sh ------------------------------------------------------

rm -f "$LOG"
run pubkey extension-pubkey.sh
written="$(cat "$REPO/web/extension/key/key.pub.b64" 2> /dev/null)"
lines="$(wc -l < "$REPO/web/extension/key/key.pub.b64" 2> /dev/null | tr -d ' ')"
if [ "$STATUS" = 0 ] && [ "$written" = "$PUB" ] && [ "$lines" = 1 ]; then
    pass "extension-pubkey.sh writes the key's public half as one line of base64 DER"
else
    fail "extension-pubkey.sh: exit $STATUS, wrote '$written' ($lines lines), stderr: $(cat "$OUT/pubkey.err")"
fi
want_id="$(id_of "$PUB")"
if grep -qx "Extension ID: $want_id" "$OUT/pubkey.out"; then
    pass "extension-pubkey.sh prints the key's extension ID ($want_id)"
else
    fail "extension-pubkey.sh printed: $(cat "$OUT/pubkey.out"), want ID $want_id"
fi
if grep -q "Commit web/extension/key/key.pub.b64" "$OUT/pubkey.out" && grep -q "just install" "$OUT/pubkey.out" \
    && grep -q "clax init" "$OUT/pubkey.out"; then
    pass "extension-pubkey.sh reminds the owner to commit the key, reinstall clax and run clax init"
else
    fail "extension-pubkey.sh gave no reminder: $(cat "$OUT/pubkey.out")"
fi
if [ "$(cat "$LOG")" = "read $REF" ]; then
    pass "extension-pubkey.sh calls op once, as op read \"\$CLAX_EXTENSION_KEY_REF\""
else
    fail "extension-pubkey.sh called op as: $(cat "$LOG")"
fi
no_key_left "extension-pubkey.sh"

# --- pack-extension.sh --------------------------------------------------------

rm -f "$LOG" "$REPO/build.log"
run pack pack-extension.sh
ZIP="$REPO/dist/clax-extension-1.2.3.zip"
if [ "$STATUS" = 0 ] && [ -f "$ZIP" ] && [ -f "$REPO/build.log" ]; then
    pass "pack-extension.sh builds and writes dist/clax-extension-<version>.zip"
else
    fail "pack-extension.sh: exit $STATUS, stderr: $(cat "$OUT/pack.err")"
fi
entries="$(unzip -Z1 "$ZIP" 2> /dev/null | sort | tr '\n' ' ')"
if [ "$entries" = "manifest.json sw.js " ]; then
    pass "the default zip holds the build alone, without key.pem"
else
    fail "the default zip holds: $entries"
fi
if [ "$(cat "$LOG")" = "read $REF" ]; then
    pass "pack-extension.sh calls op once, as op read \"\$CLAX_EXTENSION_KEY_REF\""
else
    fail "pack-extension.sh called op as: $(cat "$LOG")"
fi
# store_manifest ZIP: prints the zipped manifest's version and whether it
# has `key`, or nothing when it is not JSON.
store_manifest() {
    unzip -p "$1" manifest.json 2> /dev/null | node -e 'let s = "";
process.stdin.on("data", (d) => (s += d)).on("end", () => {
  try { const m = JSON.parse(s); console.log(m.version + " " + ("key" in m)); } catch {}
});'
}
if [ "$(store_manifest "$ZIP")" = "1.2.3 false" ] \
    && grep -q '"key"' "$REPO/web/dist-extension/manifest.json"; then
    pass "the zip's manifest.json is the build's without key, which the Web Store refuses"
else
    fail "the zip's manifest is '$(store_manifest "$ZIP")' (want '1.2.3 false'), build's: $(cat "$REPO/web/dist-extension/manifest.json")"
fi
no_key_left "pack-extension.sh"

rm -f "$LOG"
run first pack-extension.sh --first-upload
upload="$(sed -n 's/^Wrote \(.*\.zip\)$/\1/p' "$OUT/first.out")"
entries="$(unzip -Z1 "$upload" 2> /dev/null | sort | tr '\n' ' ')"
case "$upload" in
    "$SCRIPT_TMP"/clax-extension-upload.*/clax-extension-1.2.3.zip) outside=1 ;;
    *) outside=0 ;;
esac
if [ "$STATUS" = 0 ] && [ "$entries" = "key.pem manifest.json sw.js " ] && [ "$outside" = 1 ]; then
    pass "--first-upload writes a zip with key.pem at its root, outside the repository"
else
    fail "--first-upload: exit $STATUS, wrote '$upload' holding: $entries; stderr: $(cat "$OUT/first.err")"
fi
if [ "$(store_manifest "$upload")" = "1.2.3 false" ]; then
    pass "the --first-upload zip's manifest.json has no key"
else
    fail "the --first-upload zip's manifest is '$(store_manifest "$upload")'"
fi
if [ "$(unzip -p "$upload" key.pem 2> /dev/null | openssl rsa -pubout -outform DER 2> /dev/null | base64 | tr -d '\n')" = "$PUB" ] \
    && [ "$(mode_of "${upload%/*}")" = 700 ] && [ "$(mode_of "$upload")" = 600 ]; then
    pass "its key.pem is the 1Password key, and only the owner can read the zip"
else
    fail "the --first-upload zip's key.pem or modes are wrong"
fi
rm -rf "${upload%/*}"
no_key_left "pack-extension.sh --first-upload"

# A build whose manifest key is another key's is refused.
openssl genrsa 2048 2> /dev/null | openssl rsa -pubout -outform DER 2> /dev/null | base64 | tr -d '\n' \
    > "$REPO/web/extension/key/key.pub.b64"
run mismatch pack-extension.sh
if [ "$STATUS" = 1 ] && grep -q "extension-pubkey.sh" "$OUT/mismatch.err"; then
    pass "pack-extension.sh refuses a build whose manifest key is not the 1Password key's"
else
    fail "a mismatched manifest key: exit $STATUS, stderr: $(cat "$OUT/mismatch.err")"
fi
printf '%s\n' "$PUB" > "$REPO/web/extension/key/key.pub.b64"
no_key_left "a refused pack"

# interrupt NAME HANG_ON ARGS...: runs pack-extension.sh with the hanging zip
# in the background, waits until the zip call holding HANG_ON has done its
# work, counts in $HELD the key copies it holds (key files, and zips under
# construction holding key.pem), sends SIGINT and sets $STATUS and $STARTED.
# pack-extension.sh starts with SIGINT at its default disposition: run as a
# background job without job control (a quality-gates lane), this test
# inherits SIGINT ignored, which bash can neither trap nor reset, and the
# interrupt would never arrive.
FIFO="$ROOT/zip.fifo"
mkfifo "$FIFO"
interrupt() {
    local name="$1" hang_on="$2"
    shift 2
    set -m
    # shellcheck disable=SC2016 # perl's own variables
    env -u CI PATH="$ROOT/hang:$BIN:$PATH" TMPDIR="$SCRIPT_TMP" CLAX_EXTENSION_KEY_REF="$REF" \
        FAKE_OP_LOG="$LOG" FAKE_OP_REF="$REF" FAKE_OP_KEY="$VAULT/key.pem" FAKE_ZIP_FIFO="$FIFO" \
        FAKE_REAL_ZIP="$REAL_ZIP" FAKE_ZIP_HANG_ON="$hang_on" \
        perl -e '$SIG{INT} = "DEFAULT"; exec @ARGV or die "exec: $!\n"' \
        "$REPO/scripts/pack-extension.sh" "$@" > "$OUT/$name.out" 2> "$OUT/$name.err" &
    BGPID=$!
    set +m
    STARTED=""
    read -r -t 60 STARTED < "$FIFO"
    HELD="$(find "$SCRIPT_TMP" -name key.pem | wc -l | tr -d ' ')"
    local z
    for z in "$SCRIPT_TMP"/clax-extension-upload.*/*.zip.tmp; do
        if [ -f "$z" ] && unzip -Z1 "$z" 2> /dev/null | grep -x key.pem >/dev/null; then HELD=$((HELD + 1)); fi
    done
    kill -INT -- "-$BGPID"
    wait "$BGPID"
    STATUS=$?
    BGPID=""
}

# Interrupted while zipping: the key's temporary file goes too.
interrupt int ""
if [ "$STARTED" = started ] && [ "$HELD" = 1 ] && [ "$STATUS" = 130 ]; then
    pass "SIGINT mid-run stops pack-extension.sh (exit 130) while it holds the key"
else
    fail "SIGINT mid-run: started '$STARTED', $HELD key copies held, exit $STATUS"
fi
no_key_left "pack-extension.sh after SIGINT"

# Interrupted with key.pem already in the unfinished --first-upload zip: the
# upload directory goes with it (the zip is compressed, so the key-line scan
# alone would miss it).
interrupt int-first key.pem --first-upload
left="$(find "$SCRIPT_TMP" -mindepth 1 2> /dev/null)"
if [ "$STARTED" = started ] && [ "$HELD" = 2 ] && [ "$STATUS" = 130 ] && [ -z "$left" ]; then
    pass "SIGINT during --first-upload leaves no zip with key.pem and no upload directory"
else
    fail "SIGINT during --first-upload: started '$STARTED', $HELD key copies held, exit $STATUS, left: $left"
fi
no_key_left "pack-extension.sh --first-upload after SIGINT"

# --crx with a real Chromium, when there is one: CLAX_CHROMIUM, else
# Playwright's, else an installed Chrome or Chromium.
CHROMIUM="${CLAX_CHROMIUM:-}"
if [ -z "$CHROMIUM" ]; then
    for c in "$HOME"/Library/Caches/ms-playwright/chromium-*/chrome-mac*/"Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing" \
        "$HOME"/.cache/ms-playwright/chromium-*/chrome-linux*/chrome \
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
        "/Applications/Chromium.app/Contents/MacOS/Chromium"; do
        if [ -x "$c" ]; then CHROMIUM="$c"; break; fi
    done
fi
if [ -z "${CLAX_CHROMIUM:-}" ] && [ -n "${CI:-}" ]; then
    echo "SKIP: --crx: CI may have no display for Chromium (set CLAX_CHROMIUM to run it)"
elif [ -z "$CHROMIUM" ]; then
    echo "SKIP: --crx: no Chromium found (set CLAX_CHROMIUM to run it)"
else
    rm -f "$LOG"
    CLAX_CHROMIUM="$CHROMIUM" run crx pack-extension.sh --crx
    CRXF="$REPO/dist/clax-extension-1.2.3.crx"
    if [ "$STATUS" = 0 ] && [ "$(head -c 4 "$CRXF" 2> /dev/null)" = Cr24 ] && [ -f "$ZIP" ]; then
        pass "--crx packs dist/clax-extension-<version>.crx with Chromium beside the zip"
    else
        fail "--crx: exit $STATUS, stderr: $(cat "$OUT/crx.err")"
    fi
    if python3 -c 'import base64, sys
sys.exit(base64.b64decode(sys.argv[1]) not in open(sys.argv[2], "rb").read())' "$PUB" "$CRXF" 2> /dev/null; then
        pass "the .crx is signed with the 1Password key (it carries its public half)"
    else
        fail "the .crx does not carry the 1Password key's public half"
    fi
    no_key_left "pack-extension.sh --crx"
fi

exit "$FAILED"
