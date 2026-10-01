#!/usr/bin/env bash
# Tests check-version.sh, bump-version.sh and package-release.sh in a scratch
# copy of the files they touch. No network.
set -uo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
FAILED=0
pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1"; FAILED=1; }

for f in Cargo.toml Cargo.lock .claude-plugin/marketplace.json plugins/claude-code/.claude-plugin/plugin.json \
    plugins/clax/.codex-plugin/plugin.json plugins/pi/package.json plugins/pi/package-lock.json \
    scripts/ensure-clax.sh plugins/claude-code/scripts/ensure-clax.sh plugins/clax/scripts/ensure-clax.sh \
    scripts/check-version.sh scripts/bump-version.sh scripts/package-release.sh \
    scripts/sync-skill-tools.py plugins/pi/test/fixtures/contract.json docs/contract.md README.md \
    plugins/claude-code/skills/clax/SKILL.md plugins/clax/skills/clax/SKILL.md plugins/pi/skills/clax/SKILL.md \
    plugins/claude-code/README.md plugins/clax/README.md plugins/pi/README.md; do
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
    && cmp -s "$T/scripts/ensure-clax.sh" "$T/plugins/claude-code/scripts/ensure-clax.sh" \
    && grep -q '^This is Clax plugin 9.8.7\.' "$T/plugins/pi/skills/clax/SKILL.md" \
    && python3 "$T/scripts/sync-skill-tools.py" --check >/dev/null; then
    pass "bump-version writes every version, the launcher copies and skill blocks included"
else fail "bump-version writes every version"; fi

sed -i.bak 's/"version": "9.8.7"/"version": "9.8.6"/' "$T/plugins/clax/.codex-plugin/plugin.json"
out="$(cd "$T" && scripts/check-version.sh 2>&1)"; rc=$?
if [ "$rc" = 1 ] && echo "$out" | grep -q "plugins/clax/.codex-plugin/plugin.json: 9.8.6"; then pass "a stray version is named"
else fail "a stray version is named ($out)"; fi

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

[ "$FAILED" = 0 ] && echo "release script tests passed" || echo "release script tests FAILED"
exit "$FAILED"
