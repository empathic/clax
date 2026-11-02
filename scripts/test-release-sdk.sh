#!/usr/bin/env bash
# Tests scripts/release-sdk.sh in scratch git repositories, each with a bare
# origin holding main and prod, with fakes of gh, npm and pnpm on PATH and
# an SSH key of the test's own for signing (gpg.format=ssh in each scratch
# repository's own config). No network, no GitHub, no npm; git reads no
# global or system config.
#
# A scratch repository holds a copy of release-sdk.sh, of the helpers it
# sources (scripts/lib/release-common.sh), and a stand-in for
# web/sdk/package.json holding the package's name and version. Its
# history: "SDK 0.1.0", then "SDK 0.2.0", which main and prod are at
# ($PROD0). The fakes log every call to $FAKE_STATE/calls. The fake gh
# plays GitHub from the bare origin: a CI run (201) on any commit prod
# holds, its bazel job's state $FAKE_STATE/201 (default "completed
# success"); a publish-sdk run (401, 402, ...) per dispatch, titled as
# .github/workflows/publish-sdk.yml titles it, whose states `gh run view`
# reports one per call from $FAKE_STATE/<ID> (the last one sticks; none
# means "completed success"); one seen to succeed puts the version its
# commit carries on npm. The fake npm has the package at the versions
# $FAKE_STATE/npm-versions lists (0.1.0 at first; the last is its latest),
# none while $FAKE_STATE/npm-absent exists.
# The cases and their helpers run through $CASES, out of shellcheck's sight.
# shellcheck disable=SC2329
set -uo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"
T="$(cd "$(mktemp -d)" && pwd -P)"
# shellcheck source=scripts/fake-exe.sh
. "$HERE/scripts/fake-exe.sh"
trap 'rm -rf "$T"' EXIT
FAILED=0
pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1"; FAILED=1; }

# git sees only each scratch repository's own configuration.
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
export GH_REPO=empathic/clax-test CLAX_RELEASE_POLL_SECONDS=0 CLAX_RELEASE_MAX_POLLS=3
# release-sdk.sh runs under the bash running this test.
TEST_BASH="$BASH"

ssh-keygen -q -t ed25519 -N '' -C release-test -f "$T/key" || { echo "FAIL: ssh-keygen"; exit 1; }
printf 'release-test@example.com namespaces="git" %s\n' "$(cat "$T/key.pub")" > "$T/allowed_signers"

mkdir -p "$T/bin"
fake_exe "$T/bin/gh" <<'SH'
#!/bin/sh
S="$FAKE_STATE"
echo "gh $*" >> "$S/calls"
origin() { git --git-dir="$FAKE_REMOTE" "$@"; }
flag() {
    want="$1"; shift
    while [ $# -gt 0 ]; do
        if [ "$1" = "$want" ]; then echo "$2"; return 0; fi
        shift
    done
}
# The next state run $1 reports: $FAKE_STATE/$1's first line, kept once it
# is the last.
next_state() {
    state="completed success"
    if [ -s "$S/$1" ]; then
        state="$(sed -n 1p "$S/$1")"
        if [ "$(wc -l < "$S/$1")" -gt 1 ]; then sed -i.bak 1d "$S/$1"; fi
    fi
}
case "$1 $2" in
    "auth status")
        if [ -e "$S/no-auth" ]; then echo "You are not logged into any GitHub hosts." >&2; exit 1; fi ;;
    "api repos/"*)
        # The npm environment: limited to prod unless $FAKE_STATE/no-env
        # (none) or env-open (any branch).
        case "$2" in
            */environments/npm)
                if [ -e "$S/no-env" ]; then echo "gh: Not Found (HTTP 404)" >&2; exit 1; fi
                if [ -e "$S/env-open" ]; then echo none; else echo "false true"; fi ;;
            */environments/npm/deployment-branch-policies) echo "branch:prod" ;;
            *) echo "fake gh: unexpected: $*" >&2; exit 2 ;;
        esac ;;
    "run list")
        case "$(flag --workflow "$@")" in
            ci.yml)
                sha="$(flag --commit "$@")"
                if origin merge-base --is-ancestor "$sha" refs/heads/prod 2>/dev/null; then echo "201 $sha"; fi ;;
            publish-sdk.yml) cat "$S/publish-runs" 2>/dev/null || true ;;
            *) echo "fake gh: unexpected: $*" >&2; exit 2 ;;
        esac ;;
    "run view")
        case "$3 $*" in
            "201 "*" jobs "*) next_state 201; echo "$state" ;;
            "40"?" "*"status,conclusion"*)
                next_state "$3"
                # A publish run that succeeds puts its commit's version on npm.
                if [ "$state" = "completed success" ]; then
                    sha="$(awk -F '\t' -v id="$3" '$1 == id { sub("Publish SDK package ", "", $2); print $2 }' "$S/publish-runs")"
                    origin show "$sha:web/sdk/package.json" \
                        | sed -n 's/^  "version": "\(.*\)",$/\1/p' >> "$S/npm-versions"
                fi
                echo "$state" ;;
            *) echo "fake gh: unexpected: $*" >&2; exit 2 ;;
        esac ;;
    "workflow run")
        case "$3 $*" in
            "publish-sdk.yml "*" --ref prod -f dry_run=false -f sha="*)
                for a in "$@"; do case "$a" in sha=*) sha="${a#sha=}" ;; esac; done
                n=$(( $(cat "$S/publish-runs" 2>/dev/null | wc -l) + 401 ))
                { printf '%s\tPublish SDK package %s\n' "$n" "$sha"; cat "$S/publish-runs" 2>/dev/null; } > "$S/publish-runs.new"
                mv "$S/publish-runs.new" "$S/publish-runs" ;;
            *) echo "fake gh: unexpected: $*" >&2; exit 2 ;;
        esac ;;
    *) echo "fake gh: unexpected: $*" >&2; exit 2 ;;
esac
SH
# npm: `npm whoami` fails while $FAKE_STATE/npm-no-login exists. A publish
# logs the commit it runs in and adds its package.json's version.
fake_exe "$T/bin/npm" <<'SH'
#!/bin/sh
S="$FAKE_STATE"
echo "npm $*" >> "$S/calls"
case "$1" in
    view)
        if [ -e "$S/npm-absent" ]; then
            echo "npm error code E404" >&2
            echo "npm error 404 Not Found - GET https://registry.npmjs.org/$2 - Not found" >&2
            exit 1
        fi
        case "$2" in
            @empathic/clax-sdk@*) grep -x "${2##*@}" "$S/npm-versions" 2>/dev/null || true ;;
            @empathic/clax-sdk) tail -n 1 "$S/npm-versions" ;;
            *) echo "fake npm: unexpected: $*" >&2; exit 2 ;;
        esac ;;
    whoami)
        if [ -e "$S/npm-no-login" ]; then echo "npm error code ENEEDAUTH" >&2; exit 1; fi
        echo owner ;;
    publish)
        echo "npm publish at $(git rev-parse HEAD)" >> "$S/calls"
        sed -n 's/^  "version": "\(.*\)",$/\1/p' package.json >> "$S/npm-versions"
        rm -f "$S/npm-absent" ;;
    *) echo "fake npm: unexpected: $*" >&2; exit 2 ;;
esac
SH
# pnpm: `run build:sdk` writes dist-sdk/package.json at the version
# sdk/package.json gives, as the real build does.
fake_exe "$T/bin/pnpm" <<'SH'
#!/bin/sh
echo "pnpm $* (in $(basename "$(dirname "$PWD")")/$(basename "$PWD"))" >> "$FAKE_STATE/calls"
if [ "$*" = "run build:sdk" ]; then
    mkdir -p dist-sdk
    v="$(sed -n 's/^  "version": "\(.*\)",$/\1/p' sdk/package.json)"
    printf '{\n  "name": "@empathic/clax-sdk",\n  "version": "%s",\n  "type": "module"\n}\n' "$v" > dist-sdk/package.json
fi
SH
FAKE_PATH="$T/bin:$PATH"

# sdk_file VERSION: web/sdk/package.json's stand-in at VERSION.
sdk_file() {
    printf '{\n  "name": "@empathic/clax-sdk",\n  "version": "%s",\n  "private": true\n}\n' "$1" \
        > "$C/repo/web/sdk/package.json"
}
# A fresh case: $C/repo on main, prod and main pushed to $C/origin.git at
# "SDK 0.2.0" ($PROD0); npm has 0.1.0.
new_case() {
    C="$(mktemp -d "$T/case.XXXXXX")"
    mkdir -p "$C/state" "$C/repo/scripts/lib" "$C/repo/web/sdk"
    export FAKE_STATE="$C/state" FAKE_REMOTE="$C/origin.git"
    echo 0.1.0 > "$C/state/npm-versions"
    git init -q --bare -b main "$C/origin.git"
    git init -q -b main "$C/repo"
    local r="$C/repo"
    git -C "$r" config user.name "Release Test"
    git -C "$r" config user.email release-test@example.com
    git -C "$r" config gpg.format ssh
    git -C "$r" config gpg.ssh.program ssh-keygen
    git -C "$r" config user.signingkey "$T/key"
    git -C "$r" config gpg.ssh.allowedSignersFile "$T/allowed_signers"
    cp "$HERE/scripts/release-sdk.sh" "$r/scripts/release-sdk.sh"
    cp "$HERE/scripts/lib/release-common.sh" "$r/scripts/lib/release-common.sh"
    printf 'target/\n' > "$r/.gitignore"
    sdk_file 0.1.0
    git -C "$r" add -A
    git -C "$r" -c commit.gpgsign=false commit -q -m "SDK 0.1.0"
    sdk_file 0.2.0
    git -C "$r" -c commit.gpgsign=false commit -q -am "SDK 0.2.0"
    git -C "$r" branch prod
    git -C "$r" remote add origin "$C/origin.git"
    git -C "$r" push -q origin main prod
    git -C "$r" fetch -q origin
    PROD0="$(git -C "$r" rev-parse prod)"
}

# Runs release-sdk.sh from another directory; its output is in $C/out.
release() {
    (cd "$T" && PATH="${CASE_PATH:-$FAKE_PATH}" "$TEST_BASH" "$C/repo/scripts/release-sdk.sh" "$@") > "$C/out" 2>&1
}
act() { release "${1:-0.2.0}" --i-know-what-i-am-doing; }
# Everything a run could change: the work tree, ignored files included,
# the refs here and on origin, and the versions npm has.
snapshot() {
    git -C "$C/repo" status --porcelain=v1 -uall --ignored
    git -C "$C/repo" for-each-ref --format='%(refname) %(objectname)'
    git -C "$C/origin.git" for-each-ref --format='%(refname) %(objectname)'
    cat "$FAKE_STATE/npm-versions"
}
# The calls that change something.
actions() { grep -E '^(gh workflow run|npm publish|pnpm)' "$FAKE_STATE/calls" 2>/dev/null; }
count() { grep -c -x -- "$1" "$FAKE_STATE/calls"; }
has() { grep -F -- "$1" "$C/out" >/dev/null; }
o() { git --git-dir="$C/origin.git" "$@"; }
dispatch() { echo "gh workflow run publish-sdk.yml --ref prod -f dry_run=false -f sha=$1"; }
# Origin has a signed tag sdk-v0.2.0 "Clax SDK 0.2.0" on commit $1
# (default: $PROD0), npm has 0.2.0, and no worktree is left.
published() {
    local c="${1:-$PROD0}"
    [ "$(o rev-parse -q --verify 'refs/tags/sdk-v0.2.0^{commit}')" = "$c" ] \
        && [ "$(o for-each-ref --format='%(contents:subject)' refs/tags/sdk-v0.2.0)" = "Clax SDK 0.2.0" ] \
        && git -C "$C/repo" verify-tag sdk-v0.2.0 2>/dev/null \
        && grep -x 0.2.0 "$FAKE_STATE/npm-versions" >/dev/null \
        && [ "$(git -C "$C/repo" worktree list | wc -l | tr -d ' ')" = 1 ]
}
# refuse NAME EXPECTED [ARGS...]: release-sdk.sh with ARGS (default: 0.2.0
# and the act flag) exits 1 naming EXPECTED and changes nothing.
refuse() {
    local name="$1" expected="$2" before rc
    shift 2
    [ $# -gt 0 ] || set -- 0.2.0 --i-know-what-i-am-doing
    : > "$FAKE_STATE/calls"
    before="$(snapshot)"
    release "$@"; rc=$?
    if [ "$rc" = 1 ] && has "$expected" && [ -z "$(actions)" ] && [ "$before" = "$(snapshot)" ] \
        && [ ! -e "$C/repo/target" ]; then
        pass "preflight refuses $name and changes nothing"
    else fail "preflight refuses $name (rc=$rc: $(actions); $(cat "$C/out"))"; fi
}

# --- the cases ---------------------------------------------------------------------
# Each case runs in a subshell of its own (case_N below), several at a time,
# and their output is printed in order.
case_1() {
    # A dry run checks, lists the steps and changes nothing.
    new_case
    before="$(snapshot)"
    release 0.2.0; rc=$?
    if [ "$rc" = 0 ] && [ "$before" = "$(snapshot)" ] && [ -z "$(actions)" ] && [ ! -e "$C/repo/target" ] \
        && has "Dry run: nothing is changed" && has "ok: @empathic/clax-sdk 0.2.0 from $PROD0 on origin/prod" \
        && has "2. [todo] Tag the commit: git tag -s sdk-v0.2.0 -m \"Clax SDK 0.2.0\" $PROD0" \
        && has "3. [todo] Push the tag: git push origin refs/tags/sdk-v0.2.0" \
        && has "4. [todo] Publish the SDK package: $(dispatch "$PROD0")" && has "starts at step 2"; then
        pass "a dry run checks, lists the steps and changes nothing"
    else fail "a dry run (rc=$rc: $(cat "$C/out"))"; fi
}
case_2() {
    # A later version publishes through publish-sdk.yml, from prod's tip.
    new_case
    act; rc=$?
    if [ "$rc" = 0 ] && published && [ "$(count "$(dispatch "$PROD0")")" = 1 ] \
        && ! grep '^npm publish' "$FAKE_STATE/calls" >/dev/null && ! grep '^pnpm' "$FAKE_STATE/calls" >/dev/null \
        && has "Publish SDK package: run 401" && has "@empathic/clax-sdk 0.2.0 is on npm, published from $PROD0" \
        && ! has "Trusted Publisher"; then
        pass "a later version is tagged and published through publish-sdk.yml on prod"
    else fail "a publish through the workflow (rc=$rc: $(cat "$C/out"))"; fi
    # Run again, it finds every step done and changes nothing.
    : > "$FAKE_STATE/calls"
    before="$(snapshot)"
    act; rc=$?
    if [ "$rc" = 0 ] && [ -z "$(actions)" ] && [ "$before" = "$(snapshot)" ] \
        && has "step 2/4 Tag the commit: done already" && has "step 4/4 Publish the SDK package: done already" \
        && has "note: @empathic/clax-sdk@0.2.0 is on npm and origin has sdk-v0.2.0: every step is done"; then
        pass "a finished publish run again reports every step done"
    else fail "a finished publish run again (rc=$rc: $(actions); $(cat "$C/out"))"; fi
}
case_3() {
    # The first version, while the package is not on npm: built, tested and
    # published from here, from a worktree of prod's tip.
    new_case
    : > "$FAKE_STATE/npm-absent"
    release 0.2.0; rc=$?
    if [ "$rc" = 0 ] && has "@empathic/clax-sdk is not on npm yet: step 4 publishes its first version from this machine"; then
        pass "the dry run says the first version is published from here"
    else fail "the dry run with the package not on npm (rc=$rc: $(cat "$C/out"))"; fi
    : > "$FAKE_STATE/calls"
    act; rc=$?
    if [ "$rc" = 0 ] && published && [ "$(count "npm publish at $PROD0")" = 1 ] \
        && [ "$(count "npm publish --access public")" = 1 ] \
        && [ "$(count "pnpm install --frozen-lockfile (in publish/web)")" = 1 ] \
        && [ "$(count "pnpm run build:sdk (in publish/web)")" = 1 ] && [ "$(count "pnpm run test:sdk (in publish/web)")" = 1 ] \
        && ! grep '^gh workflow run' "$FAKE_STATE/calls" >/dev/null \
        && has "@empathic/clax-sdk@0.2.0 is on npm. Later versions publish through publish-sdk.yml" \
        && has "Publisher, GitHub Actions: organization empathic, repository clax-test, workflow" \
        && [ ! -e "$C/repo/target/release-sdk/0.2.0/publish" ]; then
        pass "a package not on npm is built, tested and published from here, with the trusted-publisher setup"
    else fail "the first publish (rc=$rc: $(actions); $(cat "$C/out"))"; fi
}
case_4() {
    # A failed publish run stops at step 4, the tag pushed; the next run
    # dispatches anew from the tag's commit, though prod has moved on to
    # another SDK version meanwhile.
    new_case
    echo "completed failure" > "$FAKE_STATE/401"
    act; rc=$?
    if [ "$rc" = 1 ] && has "step 4 (Publish the SDK package) failed" && has "Publish SDK package concluded failure" \
        && [ "$(o rev-parse -q --verify 'refs/tags/sdk-v0.2.0^{commit}')" = "$PROD0" ] \
        && ! grep -x 0.2.0 "$FAKE_STATE/npm-versions" >/dev/null; then
        pass "a failed publish run stops at step 4, with the tag pushed"
    else fail "a failed publish run (rc=$rc: $(cat "$C/out"))"; fi
    sdk_file 0.3.0
    git -C "$C/repo" -c commit.gpgsign=false commit -q -am "SDK 0.3.0"
    git -C "$C/repo" push -q origin HEAD:prod
    : > "$FAKE_STATE/calls"
    act; rc=$?
    if [ "$rc" = 0 ] && published && [ "$(count "$(dispatch "$PROD0")")" = 1 ] \
        && has "step 3/4 Push the tag: done already" && has "Publish SDK package: run 402"; then
        pass "the next run dispatches the publish anew, from the tag's commit"
    else fail "resume after a failed publish run (rc=$rc: $(actions); $(cat "$C/out"))"; fi
}
case_5() {
    # A publish run still going when the wait gives up is waited for again,
    # not dispatched anew.
    new_case
    printf 'in_progress \nin_progress \nin_progress \nin_progress \ncompleted success\n' > "$FAKE_STATE/401"
    act; rc=$?
    if [ "$rc" = 1 ] && has "gave up waiting for run 401 after 3 polls"; then
        pass "a wait gives up on a run that has not finished"
    else fail "a wait on an unfinished run (rc=$rc: $(cat "$C/out"))"; fi
    act; rc=$?
    if [ "$rc" = 0 ] && published && [ "$(grep -c '^gh workflow run' "$FAKE_STATE/calls")" = 1 ] \
        && has "Publish SDK package: run 401"; then
        pass "the next run waits for the run under way"
    else fail "resume while the run is under way (rc=$rc: $(actions); $(cat "$C/out"))"; fi
}
case_6() {
    new_case
    refuse "a version prod does not carry" "origin/prod's tip ($PROD0) carries SDK version 0.2.0, not 0.3.0" \
        0.3.0 --i-know-what-i-am-doing
    refuse "a version prod does not carry, in a dry run too" "carries SDK version 0.2.0, not 0.3.0" 0.3.0
}
case_7() {
    new_case
    echo 0.2.0 >> "$FAKE_STATE/npm-versions"
    refuse "a version npm has, with no tag of it" "npm has @empathic/clax-sdk@0.2.0 already, and origin has no tag sdk-v0.2.0"
    new_case
    echo 0.3.0 >> "$FAKE_STATE/npm-versions"
    refuse "a version older than npm's" "npm has @empathic/clax-sdk at 0.3.0, not older than 0.2.0"
}
case_8() {
    new_case
    : > "$FAKE_STATE/npm-absent"
    : > "$FAKE_STATE/npm-no-login"
    refuse "a first publish with npm signed out" "@empathic/clax-sdk is not on npm yet, so step 4 publishes its first version from here: sign in first"
    new_case
    : > "$FAKE_STATE/npm-absent"
    CASE_PATH="$T/no-pnpm:/usr/bin:/bin"
    mkdir -p "$T/no-pnpm"
    for t in gh npm; do [ -e "$T/no-pnpm/$t" ] || ln -s "$(readlink "$T/bin/$t")" "$T/no-pnpm/$t"; done
    refuse "a first publish without pnpm" "pnpm is not on PATH"
    unset CASE_PATH
}
case_9() {
    new_case
    echo "completed failure" > "$FAKE_STATE/201"
    refuse "a commit CI failed on" "CI's bazel job has not passed on $PROD0 (completed failure)"
    new_case
    : > "$FAKE_STATE/no-auth"
    refuse "gh signed out" "gh is not signed in (gh auth login)"
    refuse "a version that is not X.Y.Z" "0.2 is not a version (X.Y.Z)" 0.2
}
case_10() {
    # The npm environment open to any branch: a dry run warns, a publish
    # refuses.
    new_case
    : > "$FAKE_STATE/env-open"
    release 0.2.0; rc=$?
    if [ "$rc" = 0 ] && has "note: WARN: the environment npm does not limit its deployment branches to prod"; then
        pass "a dry run warns of an npm environment open to any branch"
    else fail "a dry run with an open npm environment (rc=$rc: $(cat "$C/out"))"; fi
    refuse "an npm environment open to any branch" "the environment npm does not limit its deployment branches to prod"
}
case_11() {
    # A tag of the version on a commit prod lacks.
    new_case
    git -C "$C/repo" -c commit.gpgsign=false commit -q --allow-empty -m "Not on prod"
    git -C "$C/repo" tag -s sdk-v0.2.0 -m "Clax SDK 0.2.0"
    refuse "a tag of a commit prod lacks" "the tag sdk-v0.2.0 tags $(git -C "$C/repo" rev-parse HEAD), which is not on origin/prod"
}

JOBS="$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 4)"
[ "$JOBS" -le 8 ] || JOBS=8
CASES="case_1 case_2 case_3 case_4 case_5 case_6 case_7 case_8 case_9 case_10 case_11"
# A case's subshell exits 0 once its last check has run; one that dies on
# the way exits otherwise, and fails the gate whatever it printed before.
batch=""
# wait_batch: waits for each "case:pid" in $batch, keeping its exit status.
wait_batch() {
    local entry
    for entry in $batch; do
        if wait "${entry#*:}"; then echo 0; else echo $?; fi > "$T/status.${entry%%:*}"
    done
    batch=""
}
i=0
for c in $CASES; do
    ("$c"; exit 0) > "$T/result.$c" 2>&1 &
    batch="$batch $c:$!"
    i=$((i + 1))
    if [ $((i % JOBS)) = 0 ]; then wait_batch; fi
done
wait_batch
for c in $CASES; do
    cat "$T/result.$c"
    if grep '^FAIL' "$T/result.$c" >/dev/null; then FAILED=1; fi
    status="$(cat "$T/status.$c" 2>/dev/null || echo missing)"
    if [ "$status" != 0 ]; then fail "$c exited with status $status before its last check"; fi
    if ! grep -E '^(PASS|FAIL)' "$T/result.$c" >/dev/null; then fail "$c reported nothing"; fi
done
exit "$FAILED"
