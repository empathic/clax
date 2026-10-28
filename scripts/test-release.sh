#!/usr/bin/env bash
# Tests check-version.sh, bump-version.sh, package-release.sh and
# pin-release.sh in a scratch copy of the files they touch, and
# release-exists-check.sh against a fake gh. No network: pin-release.sh
# fetches from scripts/fake-release-server.py on 127.0.0.1.
set -uo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"
T="$(cd "$(mktemp -d)" && pwd -P)"
# shellcheck source=scripts/fake-exe.sh
. "$HERE/scripts/fake-exe.sh"
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

FILES="$(grep -v '^#' "$HERE/scripts/test-release.files")"
for f in $FILES; do
    mkdir -p "$T/$(dirname "$f")"
    cp "$HERE/$f" "$T/$f"
done
V="$("$HERE/scripts/check-version.sh" --print)"

if (cd "$T" && scripts/check-version.sh); then pass "the versions agree"; else fail "the versions agree"; fi
if (cd "$T" && scripts/check-version.sh "v$V"); then pass "the matching tag is accepted"; else fail "the matching tag is accepted"; fi
out="$(cd "$T" && scripts/check-version.sh v9.9.9 2>&1)"; rc=$?
if [ "$rc" = 1 ] && echo "$out" | grep "does not match the version $V" >/dev/null; then pass "another tag is refused"; else fail "another tag is refused ($out)"; fi

if (cd "$T" && scripts/bump-version.sh 9.8.7 >/dev/null) && [ "$(cd "$T" && scripts/check-version.sh --print)" = 9.8.7 ] \
    && grep -q '^CLAX_VERSION="9.8.7"$' "$T/plugins/clax/scripts/ensure-clax.sh" \
    && cmp -s "$T/scripts/ensure-clax.sh" "$T/plugins/clax-grok/scripts/ensure-clax.sh" \
    && grep -q '^This is Clax plugin 9.8.7\.' "$T/plugins/clax-grok/skills/clax/SKILL.md" \
    && cmp -s "$T/scripts/ensure-clax.sh" "$T/plugins/claude-code/scripts/ensure-clax.sh" \
    && cmp -s "$T/scripts/ensure-clax.sh" "$T/plugins/pi/scripts/ensure-clax.sh" \
    && grep -q '^This is Clax plugin 9.8.7\.' "$T/plugins/pi/skills/clax/SKILL.md" \
    && python3 "$T/scripts/sync-skill-tools.py" --check >/dev/null \
    && grep -A1 '^name = "clax-fake-exe"$' "$T/Cargo.lock" | grep '^version = "9.8.7"$' >/dev/null \
    && grep -q '^    newTag: "9.8.7"$' "$T/deploy/k8s/sqlite/kustomization.yaml" \
    && grep -q '^    newTag: "9.8.7"$' "$T/deploy/k8s/postgres/kustomization.yaml" \
    && grep -q 'deploy/k8s/postgres?ref=v9.8.7$' "$T/docs/kubernetes.md" \
    && grep -q 'ghcr.io/empathic/clax-fleet:9.8.7 ' "$T/docs/kubernetes.md"; then
    pass "bump-version writes every version, the launcher copies, skill blocks and fleet image tags included"
else fail "bump-version writes every version"; fi
# The Worker's two crates keep their own version (0.0.0) in Cargo.lock.
if grep -A1 -E '^name = "(clax-worker|clax-do-vfs)"$' "$T/Cargo.lock" | grep '^version = ' | grep -v '"0.0.0"' | grep . >/dev/null; then
    fail "bump-version leaves the Worker's crates at 0.0.0"
else pass "bump-version leaves the Worker's crates at 0.0.0"; fi

sed -i.bak 's/"version": "9.8.7"/"version": "9.8.6"/' "$T/plugins/clax/.codex-plugin/plugin.json"
out="$(cd "$T" && scripts/check-version.sh 2>&1)"; rc=$?
if [ "$rc" = 1 ] && echo "$out" | grep "plugins/clax/.codex-plugin/plugin.json: 9.8.6" >/dev/null; then pass "a stray version is named"
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
    if [ "$rc" = 1 ] && echo "$out" | grep "$f CLAX_VERSION: 9.9.9" >/dev/null; then pass "a stray launcher copy is named ($f)"
    else fail "a stray launcher copy is named ($f: $out)"; fi
done
for f in plugins/claude-code/skills/clax/SKILL.md plugins/clax/skills/clax/SKILL.md plugins/pi/skills/clax/SKILL.md plugins/clax-grok/skills/clax/SKILL.md; do
    fresh "$T/stray"
    sed -i.bak "s/^This is Clax plugin [^ ]*\. /This is Clax plugin 9.9.9. /" "$T/stray/$f"
    out="$(cd "$T/stray" && scripts/check-version.sh 2>&1)"; rc=$?
    if [ "$rc" = 1 ] && echo "$out" | grep "$f tool block: 9.9.9" >/dev/null; then pass "a stray skill block is named ($f)"
    else fail "a stray skill block is named ($f: $out)"; fi
done

for f in deploy/k8s/sqlite/kustomization.yaml deploy/k8s/postgres/kustomization.yaml; do
    fresh "$T/stray"
    sed -i.bak 's/^    newTag: ".*"$/    newTag: "9.9.9"/' "$T/stray/$f"
    out="$(cd "$T/stray" && scripts/check-version.sh 2>&1)"; rc=$?
    if [ "$rc" = 1 ] && echo "$out" | grep "$f newTag: 9.9.9" >/dev/null; then pass "a stray fleet image tag is named ($f)"
    else fail "a stray fleet image tag is named ($f: $out)"; fi
done

# A pattern that fails to match late in the list leaves every file as it was.
fresh "$T/half"
sed -i.bak 's/^CLAX_VERSION=/CLAX_VERSION_GONE=/' "$T/half/plugins/clax/scripts/ensure-clax.sh"
rm "$T/half/plugins/clax/scripts/ensure-clax.sh.bak"
before="$(cd "$T/half" && find . -type f | sort | xargs shasum)"
out="$(cd "$T/half" && scripts/bump-version.sh 9.8.7 2>&1)"; rc=$?
after="$(cd "$T/half" && find . -type f | sort | xargs shasum)"
if [ "$rc" != 0 ] && [ "$before" = "$after" ] && echo "$out" | grep "no file was changed" >/dev/null; then
    pass "a bump that cannot match every version changes no file"
else fail "a bump that cannot match every version changes no file (rc=$rc: $out)"; fi

if out="$(cd "$T" && scripts/bump-version.sh not-a-version 2>&1)"; then fail "a bad version is refused"
elif echo "$out" | grep "is not a release version" >/dev/null; then pass "a bad version is refused"
else fail "a bad version is refused ($out)"; fi

mkdir -p "$T/bin"
printf '#!/bin/sh\necho "clax 1.2.3"\n' | fake_exe "$T/bin/clax"
"$T/scripts/package-release.sh" archive 1.2.3 x86_64-unknown-linux-musl "$T/bin/clax" "$T/dist" >/dev/null
list="$(tar -tzf "$T/dist/clax-1.2.3-x86_64-unknown-linux-musl.tar.gz" | sed 's#/$##' | sort)"
if [ "$list" = "$(printf 'clax-1.2.3-x86_64-unknown-linux-musl\nclax-1.2.3-x86_64-unknown-linux-musl/clax')" ]; then
    pass "an archive holds exactly clax-<version>-<target>/clax"
else fail "an archive holds exactly clax-<version>-<target>/clax ($list)"; fi
if out="$("$T/scripts/package-release.sh" archive 1.2.4 x86_64-unknown-linux-musl "$T/bin/clax" "$T/dist" 2>&1)"; then
    fail "a binary of another version is refused"
elif echo "$out" | grep "not 'clax 1.2.4'" >/dev/null; then pass "a binary of another version is refused"
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
printf '#!/bin/sh\necho "clax %s"\n' "$V" | fake_exe "$T/bin/clax-v"
for t in aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-musl aarch64-unknown-linux-musl; do
    "$T/scripts/package-release.sh" archive "$V" "$t" "$T/bin/clax-v" "$REL/good/v$V" > /dev/null
done
"$T/scripts/package-release.sh" sums "$REL/good/v$V" > /dev/null
# Only the file names change: a pattern on the whole line could match inside a
# checksum and write dots into it.
grep -v aarch64-unknown-linux-musl "$REL/good/v$V/SHA256SUMS" | sed "s/  clax-${V//./\\.}-/  clax-0.0.1-/" > "$REL/good/v0.0.1/SHA256SUMS"
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
if [ "$rc" = 1 ] && echo "$out" | grep "does not list clax-0.0.1-aarch64-unknown-linux-musl.tar.gz" >/dev/null && [ "$before" = "$(sums_of "$T/pin")" ]; then
    pass "a SHA256SUMS missing a target changes no file"
else fail "a SHA256SUMS missing a target changes no file (rc=$rc: $out)"; fi
out="$(cd "$T/pin" && CLAX_RELEASE_BASE_URL="$BASE/none" scripts/pin-release.sh "v$V" 2>&1)"; rc=$?
if [ "$rc" = 1 ] && echo "$out" | grep "answered HTTP 404" >/dev/null && [ "$before" = "$(sums_of "$T/pin")" ]; then
    pass "an unpublished release changes no file"
else fail "an unpublished release changes no file (rc=$rc: $out)"; fi
out="$(cd "$T/pin" && CLAX_RELEASE_BASE_URL="$BASE/ok" scripts/pin-release.sh v99.0.0 2>&1)"; rc=$?
if [ "$rc" = 1 ] && echo "$out" | grep "newer than this checkout" >/dev/null && [ "$before" = "$(sums_of "$T/pin")" ]; then
    pass "a release newer than the checkout is refused"
else fail "a release newer than the checkout is refused (rc=$rc: $out)"; fi
out="$(cd "$T/pin" && scripts/pin-release.sh 0.3.0 2>&1)"; rc=$?
if [ "$rc" = 2 ] && echo "$out" | grep "not a release tag" >/dev/null; then pass "pin-release wants a vX.Y.Z tag"
else fail "pin-release wants a vX.Y.Z tag (rc=$rc: $out)"; fi

# The default download base names GH_REPO's releases; a fake curl that
# fails keeps the request off the network.
mkdir -p "$T/nocurl"
printf '#!/bin/sh\nexit 7\n' | fake_exe "$T/nocurl/curl"
out="$(cd "$T/pin" && GH_REPO=someone/fork PATH="$T/nocurl:$PATH" scripts/pin-release.sh "v$V" 2>&1)"; rc=$?
if [ "$rc" = 1 ] && echo "$out" | grep -F "https://github.com/someone/fork/releases/download/v$V/SHA256SUMS" >/dev/null; then
    pass "pin-release downloads from GH_REPO's releases"
else fail "pin-release downloads from GH_REPO's releases (rc=$rc: $out)"; fi

# --- release-exists-check.sh ------------------------------------------------------
# A fake gh: the release exists when $FAKE_GH/release does (published
# 2026-10-20T00:00:10Z); its SOURCE_TREE asset is $FAKE_GH/source-tree when
# that exists; `gh run list` prints $FAKE_GH/built (what its --jq makes of
# the newest successful run before the release: that run's commit); `gh api
# .../commits/<sha>` prints $FAKE_GH/tree-<sha>; and $FAKE_GH/view-error
# makes `gh release view` fail otherwise than "not found".
mkdir -p "$T/ghbin"
fake_exe "$T/ghbin/gh" <<'SH'
#!/bin/sh
d="$FAKE_GH"
case "$1 $2" in
    "release view")
        if [ -e "$d/view-error" ]; then echo "HTTP 502: Bad Gateway" >&2; exit 1; fi
        if [ -e "$d/release" ]; then echo "2026-10-20T00:00:10Z"; else echo "release not found" >&2; exit 1; fi ;;
    "release download")
        if [ -e "$d/source-tree" ]; then cat "$d/source-tree"; else echo "no assets match the file pattern" >&2; exit 1; fi ;;
    "run list") cat "$d/built" 2>/dev/null || true ;;
    api*) cat "$d/tree-${2##*/}" ;;
    *) echo "fake gh: unexpected: $*" >&2; exit 2 ;;
esac
SH
check() { # case-dir [args]: runs the check (default: on v1.2.3 at commit "new")
    local d="$1"
    shift
    [ $# -gt 0 ] || set -- v1.2.3 new
    (FAKE_GH="$d" GH_REPO=empathic/clax PATH="$T/ghbin:$PATH" "$HERE/scripts/release-exists-check.sh" "$@") 2>"$d/err"
}
G="$T/gh-none"; mkdir -p "$G"
out="$(check "$G")"; rc=$?
if [ "$rc" = 0 ] && [ "$out" = "skip=false" ]; then pass "release-exists-check: no release, so build and publish"
else fail "release-exists-check: no release (rc=$rc: $out $(cat "$G/err"))"; fi
out="$(check "$G" --tree v1.2.3)"; rc=$?
if [ "$rc" = 0 ] && [ -z "$out" ]; then pass "release-exists-check --tree: no release, no tree"
else fail "release-exists-check --tree: no release (rc=$rc: $out $(cat "$G/err"))"; fi
G="$T/gh-same"; mkdir -p "$G"; : > "$G/release"; echo t1 > "$G/source-tree"; echo t1 > "$G/tree-new"
out="$(check "$G")"; rc=$?
if [ "$rc" = 0 ] && [ "$out" = "skip=true" ] && grep "built from tree t1, which new shares" "$G/err" >/dev/null \
    && grep "nothing is rebuilt or uploaded" "$G/err" >/dev/null; then
    pass "release-exists-check: a release whose SOURCE_TREE is the commit's tree is kept, saying so"
else fail "release-exists-check: same tree (rc=$rc: $out $(cat "$G/err"))"; fi
out="$(check "$G" --tree v1.2.3)"; rc=$?
if [ "$rc" = 0 ] && [ "$out" = t1 ]; then pass "release-exists-check --tree: the release's SOURCE_TREE"
else fail "release-exists-check --tree: SOURCE_TREE (rc=$rc: $out $(cat "$G/err"))"; fi
# SOURCE_TREE wins over the runs: a run on another tree before the release
# does not decide.
echo old > "$G/built"; echo t9 > "$G/tree-old"
out="$(check "$G")"; rc=$?
if [ "$rc" = 0 ] && [ "$out" = "skip=true" ]; then pass "release-exists-check: SOURCE_TREE decides, not the runs"
else fail "release-exists-check: SOURCE_TREE decides (rc=$rc: $out $(cat "$G/err"))"; fi
G="$T/gh-other"; mkdir -p "$G"; : > "$G/release"; echo t1 > "$G/source-tree"; echo t2 > "$G/tree-new"
out="$(check "$G")"; rc=$?
if [ "$rc" = 1 ] && [ -z "$out" ] && grep "another tree; delete the release" "$G/err" >/dev/null; then
    pass "release-exists-check: a release built from another tree fails the run"
else fail "release-exists-check: another tree (rc=$rc: $out $(cat "$G/err"))"; fi
# A release from before SOURCE_TREE: the run that uploaded it decides.
G="$T/gh-old-same"; mkdir -p "$G"; : > "$G/release"; echo old > "$G/built"; echo t1 > "$G/tree-old"; echo t1 > "$G/tree-new"
out="$(check "$G")"; rc=$?
if [ "$rc" = 0 ] && [ "$out" = "skip=true" ]; then
    pass "release-exists-check: a release with no SOURCE_TREE is judged by the run that uploaded it"
else fail "release-exists-check: no SOURCE_TREE, same tree (rc=$rc: $out $(cat "$G/err"))"; fi
G="$T/gh-old-other"; mkdir -p "$G"; : > "$G/release"; echo old > "$G/built"; echo t1 > "$G/tree-old"; echo t2 > "$G/tree-new"
out="$(check "$G")"; rc=$?
if [ "$rc" = 1 ] && grep "another tree" "$G/err" >/dev/null; then
    pass "release-exists-check: a release with no SOURCE_TREE from another tree's run fails the run"
else fail "release-exists-check: no SOURCE_TREE, another tree (rc=$rc: $out $(cat "$G/err"))"; fi
G="$T/gh-norun"; mkdir -p "$G"; : > "$G/release"
out="$(check "$G")"; rc=$?
if [ "$rc" = 1 ] && grep "has no SOURCE_TREE and no successful Release run on v1.2.3 before it" "$G/err" >/dev/null; then
    pass "release-exists-check: a release neither rule places fails the run"
else fail "release-exists-check: no rule places it (rc=$rc: $out $(cat "$G/err"))"; fi
G="$T/gh-error"; mkdir -p "$G"; : > "$G/view-error"
out="$(check "$G")"; rc=$?
if [ "$rc" = 1 ] && grep "cannot tell whether the release v1.2.3 exists: HTTP 502" "$G/err" >/dev/null; then
    pass "release-exists-check: gh failing fails the run"
else fail "release-exists-check: gh failing (rc=$rc: $out $(cat "$G/err"))"; fi

[ "$FAILED" = 0 ] && echo "release script tests passed" || echo "release script tests FAILED"
exit "$FAILED"
