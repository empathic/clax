#!/usr/bin/env bash
# Tests check-version.sh, bump-version.sh, package-release.sh and
# pin-release.sh in a scratch copy of the files they touch. No network:
# pin-release.sh fetches from scripts/fake-release-server.py on 127.0.0.1.
set -uo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"
T="$(cd "$(mktemp -d)" && pwd -P)"
SERVER_PID=""
# shellcheck disable=SC2329 # run by the EXIT trap
cleanup() {
    if [ -n "$SERVER_PID" ]; then kill "$SERVER_PID" 2>/dev/null; wait "$SERVER_PID" 2>/dev/null; fi
    rm -rf "$T"
    return 0
}
trap cleanup EXIT
FAILED=0
pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1"; FAILED=1; }

FILES="Cargo.toml Cargo.lock .claude-plugin/marketplace.json plugins/claude-code/.claude-plugin/plugin.json \
    plugins/clax/.codex-plugin/plugin.json plugins/pi/package.json plugins/pi/package-lock.json \
    scripts/ensure-clax.sh plugins/claude-code/scripts/ensure-clax.sh plugins/clax/scripts/ensure-clax.sh \
    scripts/check-version.sh scripts/bump-version.sh scripts/package-release.sh \
    scripts/sync-skill-tools.py plugins/pi/test/fixtures/contract.json docs/contract.md README.md \
    plugins/claude-code/skills/clax/SKILL.md plugins/clax/skills/clax/SKILL.md plugins/pi/skills/clax/SKILL.md \
    plugins/claude-code/README.md plugins/clax/README.md plugins/pi/README.md \
    plugins/clax-grok/.grok-plugin/plugin.json plugins/clax-grok/scripts/ensure-clax.sh \
    plugins/clax-grok/skills/clax/SKILL.md plugins/clax-grok/README.md \
    plugins/pi/scripts/ensure-clax.sh scripts/pin-release.sh"
for f in $FILES; do
    mkdir -p "$T/$(dirname "$f")"
    cp "$HERE/$f" "$T/$f"
done
V="$("$HERE/scripts/check-version.sh" --print)"

if (cd "$T" && scripts/check-version.sh); then pass "the versions agree"; else fail "the versions agree"; fi
if (cd "$T" && scripts/check-version.sh "v$V"); then pass "the matching tag is accepted"; else fail "the matching tag is accepted"; fi
out="$(cd "$T" && scripts/check-version.sh v9.9.9 2>&1)"; rc=$?
if [ "$rc" = 1 ] && echo "$out" | grep -q "does not match the version $V"; then pass "another tag is refused"; else fail "another tag is refused ($out)"; fi

if (cd "$T" && scripts/bump-version.sh 9.8.7 >/dev/null) && [ "$(cd "$T" && scripts/check-version.sh --print)" = 9.8.7 ] \
    && grep -q '^CLAX_VERSION="9.8.7"$' "$T/plugins/clax/scripts/ensure-clax.sh" \
    && cmp -s "$T/scripts/ensure-clax.sh" "$T/plugins/clax-grok/scripts/ensure-clax.sh" \
    && grep -q '^This is Clax plugin 9.8.7\.' "$T/plugins/clax-grok/skills/clax/SKILL.md" \
    && cmp -s "$T/scripts/ensure-clax.sh" "$T/plugins/claude-code/scripts/ensure-clax.sh" \
    && cmp -s "$T/scripts/ensure-clax.sh" "$T/plugins/pi/scripts/ensure-clax.sh" \
    && grep -q '^This is Clax plugin 9.8.7\.' "$T/plugins/pi/skills/clax/SKILL.md" \
    && python3 "$T/scripts/sync-skill-tools.py" --check >/dev/null; then
    pass "bump-version writes every version, the launcher copies and skill blocks included"
else fail "bump-version writes every version"; fi

sed -i.bak 's/"version": "9.8.7"/"version": "9.8.6"/' "$T/plugins/clax/.codex-plugin/plugin.json"
out="$(cd "$T" && scripts/check-version.sh 2>&1)"; rc=$?
if [ "$rc" = 1 ] && echo "$out" | grep -q "plugins/clax/.codex-plugin/plugin.json: 9.8.6"; then pass "a stray version is named"
else fail "a stray version is named ($out)"; fi

# A fresh copy of the version files, for the cases below that need one.
fresh() {
    rm -rf "$1"; mkdir -p "$1"
    (cd "$HERE" && for f in $FILES; do mkdir -p "$1/$(dirname "$f")"; cp "$f" "$1/$f"; done)
}
for f in plugins/claude-code/scripts/ensure-clax.sh plugins/clax/scripts/ensure-clax.sh plugins/clax-grok/scripts/ensure-clax.sh plugins/pi/scripts/ensure-clax.sh; do
    fresh "$T/stray"
    sed -i.bak "s/^CLAX_VERSION=\".*\"$/CLAX_VERSION=\"9.9.9\"/" "$T/stray/$f"
    out="$(cd "$T/stray" && scripts/check-version.sh 2>&1)"; rc=$?
    if [ "$rc" = 1 ] && echo "$out" | grep -q "$f CLAX_VERSION: 9.9.9"; then pass "a stray launcher copy is named ($f)"
    else fail "a stray launcher copy is named ($f: $out)"; fi
done
for f in plugins/claude-code/skills/clax/SKILL.md plugins/clax/skills/clax/SKILL.md plugins/pi/skills/clax/SKILL.md plugins/clax-grok/skills/clax/SKILL.md; do
    fresh "$T/stray"
    sed -i.bak "s/^This is Clax plugin [^ ]*\. /This is Clax plugin 9.9.9. /" "$T/stray/$f"
    out="$(cd "$T/stray" && scripts/check-version.sh 2>&1)"; rc=$?
    if [ "$rc" = 1 ] && echo "$out" | grep -q "$f tool block: 9.9.9"; then pass "a stray skill block is named ($f)"
    else fail "a stray skill block is named ($f: $out)"; fi
done

# A pattern that fails to match late in the list leaves every file as it was.
fresh "$T/half"
sed -i.bak 's/^CLAX_VERSION=/CLAX_VERSION_GONE=/' "$T/half/plugins/clax/scripts/ensure-clax.sh"
rm "$T/half/plugins/clax/scripts/ensure-clax.sh.bak"
before="$(cd "$T/half" && find . -type f | sort | xargs shasum)"
out="$(cd "$T/half" && scripts/bump-version.sh 9.8.7 2>&1)"; rc=$?
after="$(cd "$T/half" && find . -type f | sort | xargs shasum)"
if [ "$rc" != 0 ] && [ "$before" = "$after" ] && echo "$out" | grep -q "no file was changed"; then
    pass "a bump that cannot match every version changes no file"
else fail "a bump that cannot match every version changes no file (rc=$rc: $out)"; fi

if out="$(cd "$T" && scripts/bump-version.sh not-a-version 2>&1)"; then fail "a bad version is refused"
elif echo "$out" | grep -q "is not a release version"; then pass "a bad version is refused"
else fail "a bad version is refused ($out)"; fi

mkdir -p "$T/bin"
printf '#!/bin/sh\necho "clax 1.2.3"\n' > "$T/bin/clax"
chmod +x "$T/bin/clax"
"$T/scripts/package-release.sh" archive 1.2.3 x86_64-unknown-linux-musl "$T/bin/clax" "$T/dist" >/dev/null
list="$(tar -tzf "$T/dist/clax-1.2.3-x86_64-unknown-linux-musl.tar.gz" | sed 's#/$##' | sort)"
if [ "$list" = "$(printf 'clax-1.2.3-x86_64-unknown-linux-musl\nclax-1.2.3-x86_64-unknown-linux-musl/clax')" ]; then
    pass "an archive holds exactly clax-<version>-<target>/clax"
else fail "an archive holds exactly clax-<version>-<target>/clax ($list)"; fi
if out="$("$T/scripts/package-release.sh" archive 1.2.4 x86_64-unknown-linux-musl "$T/bin/clax" "$T/dist" 2>&1)"; then
    fail "a binary of another version is refused"
elif echo "$out" | grep -q "not 'clax 1.2.4'"; then pass "a binary of another version is refused"
else fail "a binary of another version is refused ($out)"; fi

echo "#!/bin/sh" > "$T/dist/install.sh"
"$T/scripts/package-release.sh" sums "$T/dist" >/dev/null
if (cd "$T/dist" && { sha256sum -c SHA256SUMS 2>/dev/null || shasum -a 256 -c SHA256SUMS; } >/dev/null) \
    && [ "$(wc -l < "$T/dist/SHA256SUMS" | tr -d ' ')" = 2 ] && ! grep -q SHA256SUMS "$T/dist/SHA256SUMS"; then
    pass "SHA256SUMS covers every other file and verifies"
else fail "SHA256SUMS covers every other file and verifies"; fi

# --- pin-release.sh -------------------------------------------------------------
# A release v$V of every target (and a v0.0.1 whose SHA256SUMS lacks one),
# served at /ok; /none answers 404.
PY="$(python3 -c 'import sys; print(sys.executable)')"
REL="$T/rel"
mkdir -p "$REL/good/v$V" "$REL/good/v0.0.1"
printf '#!/bin/sh\necho "clax %s"\n' "$V" > "$T/bin/clax-v"
chmod +x "$T/bin/clax-v"
for t in aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-musl aarch64-unknown-linux-musl; do
    "$T/scripts/package-release.sh" archive "$V" "$t" "$T/bin/clax-v" "$REL/good/v$V" > /dev/null
done
"$T/scripts/package-release.sh" sums "$REL/good/v$V" > /dev/null
grep -v aarch64-unknown-linux-musl "$REL/good/v$V/SHA256SUMS" | sed "s/$V/0.0.1/g" > "$REL/good/v0.0.1/SHA256SUMS"
: > "$T/requests.log"
"$PY" "$HERE/scripts/fake-release-server.py" "$REL" "$T/requests.log" "$T/port" &
SERVER_PID=$!
i=0
while [ ! -s "$T/port" ] && [ "$i" -lt 100 ]; do sleep 0.05; i=$((i + 1)); done
BASE="http://127.0.0.1:$(cat "$T/port")"
WRAPPERS="scripts/ensure-clax.sh plugins/claude-code/scripts/ensure-clax.sh plugins/clax/scripts/ensure-clax.sh plugins/clax-grok/scripts/ensure-clax.sh plugins/pi/scripts/ensure-clax.sh"
sums_of() { (cd "$1" && for w in $WRAPPERS; do shasum "$w"; done); }

fresh "$T/pin"
out="$(cd "$T/pin" && CLAX_RELEASE_BASE_URL="$BASE/ok" scripts/pin-release.sh "v$V" 2>&1)"; rc=$?
want_mac="$(awk -v f="clax-$V-aarch64-apple-darwin.tar.gz" '$2 == f { print $1 }' "$REL/good/v$V/SHA256SUMS")"
want_musl="$(awk -v f="clax-$V-aarch64-unknown-linux-musl.tar.gz" '$2 == f { print $1 }' "$REL/good/v$V/SHA256SUMS")"
ok=1
for w in $WRAPPERS; do
    grep -q "^PINNED_VERSION=\"$V\"$" "$T/pin/$w" && grep -q "^SHA256_AARCH64_APPLE_DARWIN=\"$want_mac\"$" "$T/pin/$w" \
        && grep -q "^SHA256_AARCH64_UNKNOWN_LINUX_MUSL=\"$want_musl\"$" "$T/pin/$w" && ! grep -q '^SHA256_[A-Z0-9_]*=""$' "$T/pin/$w" \
        && cmp -s "$T/pin/scripts/ensure-clax.sh" "$T/pin/$w" && [ -x "$T/pin/$w" ] || ok=0
done
if [ "$rc" = 0 ] && [ "$ok" = 1 ] && grep -q "^/ok/v$V/SHA256SUMS$" "$T/requests.log" && (cd "$T/pin" && scripts/check-version.sh); then
    pass "pin-release writes the version and the four checksums into every wrapper copy, the Pi package's included"
else fail "pin-release writes the pin into every wrapper copy (rc=$rc: $out)"; fi

fresh "$T/pin"
before="$(sums_of "$T/pin")"
out="$(cd "$T/pin" && CLAX_RELEASE_BASE_URL="$BASE/ok" scripts/pin-release.sh v0.0.1 2>&1)"; rc=$?
if [ "$rc" = 1 ] && echo "$out" | grep -q "does not list clax-0.0.1-aarch64-unknown-linux-musl.tar.gz" && [ "$before" = "$(sums_of "$T/pin")" ]; then
    pass "a SHA256SUMS missing a target changes no file"
else fail "a SHA256SUMS missing a target changes no file (rc=$rc: $out)"; fi
out="$(cd "$T/pin" && CLAX_RELEASE_BASE_URL="$BASE/none" scripts/pin-release.sh "v$V" 2>&1)"; rc=$?
if [ "$rc" = 1 ] && echo "$out" | grep -q "answered HTTP 404" && [ "$before" = "$(sums_of "$T/pin")" ]; then
    pass "an unpublished release changes no file"
else fail "an unpublished release changes no file (rc=$rc: $out)"; fi
out="$(cd "$T/pin" && CLAX_RELEASE_BASE_URL="$BASE/ok" scripts/pin-release.sh v99.0.0 2>&1)"; rc=$?
if [ "$rc" = 1 ] && echo "$out" | grep -q "newer than this checkout" && [ "$before" = "$(sums_of "$T/pin")" ]; then
    pass "a release newer than the checkout is refused"
else fail "a release newer than the checkout is refused (rc=$rc: $out)"; fi
out="$(cd "$T/pin" && scripts/pin-release.sh 0.3.0 2>&1)"; rc=$?
if [ "$rc" = 2 ] && echo "$out" | grep -q "not a release tag"; then pass "pin-release wants a vX.Y.Z tag"
else fail "pin-release wants a vX.Y.Z tag (rc=$rc: $out)"; fi

[ "$FAILED" = 0 ] && echo "release script tests passed" || echo "release script tests FAILED"
exit "$FAILED"
