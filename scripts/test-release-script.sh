#!/usr/bin/env bash
# Tests scripts/release.sh in scratch git repositories, each with a bare
# origin, with fakes of gh, just and bazelisk on PATH and an SSH key of the
# test's own for signing (gpg.format=ssh in each scratch repository's own
# config). No network, no GitHub; git reads no global or system config.
#
# A scratch repository holds a copy of release.sh and stand-ins for the
# scripts it runs (bump-version.sh, check-version.sh, pin-release.sh write
# and read CLAX_VERSION and PINNED_VERSION in scripts/ensure-clax.sh; their
# own tests are scripts/test-release.sh). The fakes log every call to
# $FAKE_STATE/calls. The fake gh plays GitHub from the bare origin: a
# Release run (ID 101) exists once a tag on the commit is pushed, a CI run
# (201) once main holds the commit, a publish-pi run (301, 302, ...) per
# dispatch, titled as .github/workflows/publish-pi.yml titles it.
# $FAKE_STATE/<ID> lists the states `gh run view <ID>` reports, one per call
# (the last one sticks; none means "completed success"); the release lists
# SHA256SUMS once run 101 was seen to succeed, published at the time in
# $FAKE_STATE/release-created (run 101 started at 2026-10-20T00:00:05Z).
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
unset CLAX_BUILD

ssh-keygen -q -t ed25519 -N '' -C release-test -f "$T/key" || { echo "FAIL: ssh-keygen"; exit 1; }
printf 'release-test@example.com namespaces="git" %s\n' "$(cat "$T/key.pub")" > "$T/allowed_signers"

mkdir -p "$T/bin"
fake_exe "$T/bin/bazelisk" <<'SH'
#!/bin/sh
echo "bazelisk $*" >> "$FAKE_STATE/calls"
SH
fake_exe "$T/bin/just" <<'SH'
#!/bin/sh
echo "just $*" >> "$FAKE_STATE/calls"
case "$1" in
    bazel-sync)
        # One change per run: the version into bazel/workspace.bzl, then that
        # into MODULE.bazel.lock; the third run changes nothing.
        v="$(sed -n 's/^CLAX_VERSION="\(.*\)"$/\1/p' scripts/ensure-clax.sh)"
        if [ "$(cat bazel/workspace.bzl)" != "version = $v" ]; then
            echo "version = $v" > bazel/workspace.bzl
        elif ! cmp -s bazel/workspace.bzl MODULE.bazel.lock; then
            cp bazel/workspace.bzl MODULE.bazel.lock
        fi
        echo "fake bazel-sync" ;;
    ci)
        if [ -e "$FAKE_STATE/ci-fail" ]; then echo "fake ci: a gate failed"; exit 1; fi
        echo "fake ci: every gate passed" ;;
    *) echo "fake just: unexpected: $*" >&2; exit 2 ;;
esac
SH
fake_exe "$T/bin/gh" <<'SH'
#!/bin/sh
S="$FAKE_STATE"
echo "gh $*" >> "$S/calls"
origin() { git --git-dir="$FAKE_REMOTE" "$@"; }
# Someone else's commit lands on origin's main.
foreign_push() {
    t="$(origin rev-parse 'main^{tree}')"
    c="$(GIT_AUTHOR_NAME=Other GIT_AUTHOR_EMAIL=other@example.com GIT_COMMITTER_NAME=Other \
        GIT_COMMITTER_EMAIL=other@example.com origin commit-tree "$t" -p main -m "Other work")"
    origin update-ref refs/heads/main "$c"
}
flag() {
    want="$1"; shift
    while [ $# -gt 0 ]; do
        if [ "$1" = "$want" ]; then echo "$2"; return 0; fi
        shift
    done
}
# $FAKE_STATE/gh-fail: how many of the next run list and run view calls fail.
case "$1 $2" in
    "run list"|"run view")
        n="$(cat "$S/gh-fail" 2>/dev/null || echo 0)"
        if [ "$n" -gt 0 ]; then
            echo $((n - 1)) > "$S/gh-fail"
            echo "HTTP 502: Bad Gateway (fake)" >&2
            exit 1
        fi ;;
esac
case "$1 $2" in
    "auth status")
        if [ -e "$S/no-auth" ]; then echo "You are not logged into any GitHub hosts." >&2; exit 1; fi ;;
    "release view")
        if [ -e "$S/release-created" ] && [ -n "$(origin tag -l "$3")" ]; then
            case " $* " in
                *" createdAt "*) cat "$S/release-created" ;;
                *) printf 'clax-%s-aarch64-apple-darwin.tar.gz\ninstall.sh\nSHA256SUMS\n' "${3#v}" ;;
            esac
        else
            echo "release not found" >&2; exit 1
        fi ;;
    "run list")
        sha="$(flag --commit "$@")"
        case "$(flag --workflow "$@")" in
            release.yml) [ -z "$(origin tag --points-at "$sha" 'v*')" ] || echo "101 $sha" ;;
            ci.yml) if origin merge-base --is-ancestor "$sha" refs/heads/main 2>/dev/null; then echo "201 $sha"; fi ;;
            publish-pi.yml) cat "$S/publish-runs" 2>/dev/null || true ;;
            *) echo "fake gh: unexpected: $*" >&2; exit 2 ;;
        esac ;;
    "run view")
        # $FAKE_STATE/push-on-<ID>: the first view of run <ID> sees other work
        # land on origin's main.
        if [ -e "$S/push-on-$3" ]; then rm "$S/push-on-$3"; foreign_push; fi
        case " $* " in *" createdAt "*) echo "2026-10-20T00:00:05Z"; exit 0 ;; esac
        state="completed success"
        if [ -s "$S/$3" ]; then
            state="$(sed -n 1p "$S/$3")"
            if [ "$(wc -l < "$S/$3")" -gt 1 ]; then sed -i.bak 1d "$S/$3"; fi
        fi
        if [ "$3" = 101 ] && [ "$state" = "completed success" ] && [ ! -e "$S/release-created" ]; then
            echo "2026-10-20T00:00:10Z" > "$S/release-created"
        fi
        echo "$state" ;;
    "workflow run")
        case "$3 $*" in
            "publish-pi.yml "*" -f dry_run=false -f sha="*)
                for a in "$@"; do case "$a" in sha=*) sha="${a#sha=}" ;; esac; done
                n=$(( $(cat "$S/publish-runs" 2>/dev/null | wc -l) + 301 ))
                { printf '%s\tPublish Pi package %s\n' "$n" "$sha"; cat "$S/publish-runs" 2>/dev/null; } > "$S/publish-runs.new"
                mv "$S/publish-runs.new" "$S/publish-runs" ;;
            *) echo "fake gh: unexpected: $*" >&2; exit 2 ;;
        esac ;;
    *) echo "fake gh: unexpected: $*" >&2; exit 2 ;;
esac
SH
FAKE_PATH="$T/bin:$PATH"

# A fresh case: $C/repo on main at 0.5.0, pinned to 0.5.0, tagged v0.4.0
# and v0.5.0, pushed to $C/origin.git.
N=0
new_case() {
    N=$((N + 1))
    C="$T/case$N"
    mkdir -p "$C/state" "$C/repo/scripts" "$C/repo/bazel"
    export FAKE_STATE="$C/state" FAKE_REMOTE="$C/origin.git"
    git init -q --bare -b main "$C/origin.git"
    git init -q -b main "$C/repo"
    local r="$C/repo"
    git -C "$r" config user.name "Release Test"
    git -C "$r" config user.email release-test@example.com
    git -C "$r" config gpg.format ssh
    git -C "$r" config gpg.ssh.program ssh-keygen
    git -C "$r" config user.signingkey "$T/key"
    git -C "$r" config gpg.ssh.allowedSignersFile "$T/allowed_signers"
    cp "$HERE/scripts/release.sh" "$r/scripts/release.sh"
    printf 'target/\n' > "$r/.gitignore"
    printf 'CLAX_VERSION="0.5.0"\nPINNED_VERSION="0.5.0"\n' > "$r/scripts/ensure-clax.sh"
    echo "version = 0.5.0" > "$r/bazel/workspace.bzl"
    echo "version = 0.5.0" > "$r/MODULE.bazel.lock"
    cat > "$r/scripts/check-version.sh" <<'SH'
#!/usr/bin/env bash
cd "$(dirname "$0")/.." || exit 1
sed -n 's/^CLAX_VERSION="\(.*\)"$/\1/p' scripts/ensure-clax.sh
SH
    cat > "$r/scripts/bump-version.sh" <<'SH'
#!/usr/bin/env bash
set -e
cd "$(dirname "$0")/.."
echo "bump $1" >> "$FAKE_STATE/calls"
sed -i.bak "s/^CLAX_VERSION=.*/CLAX_VERSION=\"$1\"/" scripts/ensure-clax.sh
rm scripts/ensure-clax.sh.bak
SH
    cat > "$r/scripts/pin-release.sh" <<'SH'
#!/usr/bin/env bash
set -e
cd "$(dirname "$0")/.."
echo "pin $1" >> "$FAKE_STATE/calls"
sed -i.bak "s/^PINNED_VERSION=.*/PINNED_VERSION=\"${1#v}\"/" scripts/ensure-clax.sh
rm scripts/ensure-clax.sh.bak
SH
    git -C "$r" add -A
    git -C "$r" -c commit.gpgsign=false commit -q -m "Clax 0.4.0"
    git -C "$r" -c tag.gpgsign=false tag -a v0.4.0 -m "Clax 0.4.0"
    git -C "$r" -c commit.gpgsign=false commit -q --allow-empty -m "Release 0.5.0"
    git -C "$r" -c tag.gpgsign=false tag -a v0.5.0 -m "Clax 0.5.0"
    git -C "$r" remote add origin "$C/origin.git"
    git -C "$r" push -q origin main v0.4.0 v0.5.0
    git -C "$r" fetch -q origin
}

# Runs release.sh from another directory; its output is in $C/out.
release() {
    (cd "$T" && PATH="$FAKE_PATH" bash "$C/repo/scripts/release.sh" "$@") > "$C/out" 2>&1
}
act() { release 0.6.0 --i-know-what-i-am-doing; }
# Everything a run could change: the work tree, ignored files included, and
# the refs here and on origin.
snapshot() {
    git -C "$C/repo" status --porcelain=v1 -uall --ignored
    git -C "$C/repo" for-each-ref --format='%(refname) %(objectname)'
    git -C "$C/origin.git" for-each-ref --format='%(refname) %(objectname)'
    cat "$C/repo/scripts/ensure-clax.sh" "$C/repo/bazel/workspace.bzl" "$C/repo/MODULE.bazel.lock"
}
# The calls that change something.
actions() { grep -E '^(bump|pin|just|gh workflow run)' "$FAKE_STATE/calls" 2>/dev/null; }
count() { grep -c -x -- "$1" "$FAKE_STATE/calls"; }
has() { grep -F -- "$1" "$C/out" >/dev/null; }

# The release a full run leaves: a signed tag v0.6.0 "Clax 0.6.0" on a signed
# "Release 0.6.0" commit, and origin's main a signed pin commit on it.
released() {
    local o="$C/origin.git" r="$C/repo" rel pin
    rel="$(git --git-dir="$o" rev-parse -q --verify 'refs/tags/v0.6.0^{commit}')" || return 1
    pin="$(git --git-dir="$o" rev-parse refs/heads/main)"
    [ "$(git --git-dir="$o" log -1 --format=%s "$rel")" = "Release 0.6.0" ] \
        && [ "$(git --git-dir="$o" log -1 --format=%s "$pin")" = "Pin the plugins to Clax 0.6.0" ] \
        && [ "$(git --git-dir="$o" rev-parse "$pin^")" = "$rel" ] \
        && [ "$(git --git-dir="$o" for-each-ref --format='%(contents:subject)' refs/tags/v0.6.0)" = "Clax 0.6.0" ] \
        && git -C "$r" verify-tag v0.6.0 2>/dev/null \
        && git -C "$r" verify-commit "$rel" 2>/dev/null \
        && git -C "$r" verify-commit "$pin" 2>/dev/null \
        && [ "$(git -C "$r" rev-parse HEAD)" = "$pin" ] \
        && [ -z "$(git -C "$r" status --porcelain)" ] \
        && grep -x 'CLAX_VERSION="0.6.0"' "$r/scripts/ensure-clax.sh" >/dev/null \
        && grep -x 'PINNED_VERSION="0.6.0"' "$r/scripts/ensure-clax.sh" >/dev/null \
        && [ "$(cat "$r/MODULE.bazel.lock")" = "version = 0.6.0" ]
}

# --- a dry run changes nothing ---------------------------------------------------
new_case
before="$(snapshot)"
release 0.6.0; rc=$?
after="$(snapshot)"
if [ "$rc" = 0 ] && [ "$before" = "$after" ] && [ -z "$(actions)" ] && [ ! -e "$C/repo/target" ] \
    && has "Dry run: nothing is changed" && has "2. [todo] Bump the version: scripts/bump-version.sh 0.6.0" \
    && has "14. [todo] Publish the Pi package" && has "starts at step 2"; then
    pass "a dry run checks, prints the plan and changes nothing"
else fail "a dry run checks, prints the plan and changes nothing (rc=$rc: $(cat "$C/out"))"; fi

# --- a full run, in order ------------------------------------------------------
new_case
printf 'queued \nin_progress \ncompleted success\n' > "$FAKE_STATE/101"
act; rc=$?
pin="$(git --git-dir="$C/origin.git" rev-parse main)"
want="$(printf '%s\n' "bump 0.6.0" "just bazel-sync" "just bazel-sync" "just bazel-sync" "just ci" \
    "gh run view 101" "pin v0.6.0" "just ci" "gh run view 201" \
    "gh workflow run publish-pi.yml --ref main -f dry_run=false -f sha=$pin" "gh run view 301")"
# Each run's polls, as one line.
got="$(grep -E '^(bump|pin|just|gh workflow run|gh run view)' "$FAKE_STATE/calls" \
    | sed 's/^\(gh run view [0-9]*\) .*/\1/' | awk '!($0 ~ /^gh run view/ && $0 == prev) { print } { prev = $0 }')"
if [ "$rc" = 0 ] && [ "$got" = "$want" ] && released && has "Clax 0.6.0 is released." \
    && has "Release v0.6.0: in_progress" \
    && [ -s "$C/repo/target/release/0.6.0.log" ] && has "redeploy clax.dev with \`just clax-dev-deploy\`"; then
    pass "a full run bumps, syncs until settled, tests, signs, tags, pushes, waits, pins and publishes, in order"
else fail "a full run, in order (rc=$rc; calls: $got; $(cat "$C/out"))"; fi

: > "$FAKE_STATE/calls"
before="$(snapshot)"
act; rc=$?
if [ "$rc" = 0 ] && [ -z "$(actions)" ] && [ "$before" = "$(snapshot)" ] && has "Step 14: Publish the Pi package: done already"; then
    pass "a run after a finished release does nothing again"
else fail "a run after a finished release does nothing again (rc=$rc: $(actions); $(cat "$C/out"))"; fi

# --- resume after a failure in just ci ---------------------------------------------
new_case
: > "$FAKE_STATE/ci-fail"
act; rc=$?
if [ "$rc" = 1 ] && has "step 4 (Run the gates) failed" && has "scripts/release.sh 0.6.0 --i-know-what-i-am-doing" \
    && grep -x 'CLAX_VERSION="0.6.0"' "$C/repo/scripts/ensure-clax.sh" >/dev/null \
    && [ -z "$(git -C "$C/repo" tag -l v0.6.0)" ] && [ "$(git -C "$C/repo" log -1 --format=%s)" = "Release 0.5.0" ]; then
    pass "a failing just ci stops the run before the release commit, naming the step and how to resume"
else fail "a failing just ci stops the run (rc=$rc: $(cat "$C/out"))"; fi
release 0.6.0; rc=$?
if [ "$rc" = 0 ] && has "2. [done] Bump the version" && has "3. [todo] Sync Bazel" && has "starts at step 3"; then
    pass "a dry run after it shows the bump done and the rest to do"
else fail "a dry run after a failing just ci (rc=$rc: $(cat "$C/out"))"; fi
rm "$FAKE_STATE/ci-fail"
act; rc=$?
if [ "$rc" = 0 ] && released && [ "$(count "bump 0.6.0")" = 1 ] && has "Step 2: Bump the version: done already"; then
    pass "the next run resumes at the gates and finishes the release, bumping once"
else fail "resume after a failing just ci (rc=$rc: $(cat "$C/out"))"; fi

# --- resume after a failure while waiting for the Release workflow ----------------
new_case
echo "completed failure" > "$FAKE_STATE/101"
act; rc=$?
remote_main="$(git --git-dir="$C/origin.git" log -1 --format=%s main)"
if [ "$rc" = 1 ] && has "step 8 (Wait for the Release workflow) failed" && has "concluded failure" \
    && has "gh run rerun 101 --failed" && [ -n "$(git --git-dir="$C/origin.git" tag -l v0.6.0)" ] \
    && [ "$remote_main" = "Release 0.5.0" ]; then
    pass "a failed Release run stops the run after the tag push, with main not pushed"
else fail "a failed Release run stops the run (rc=$rc: $(cat "$C/out"))"; fi
echo "in_progress " > "$FAKE_STATE/101"
act; rc=$?
if [ "$rc" = 1 ] && has "Step 7: Push the tag: done already" && has "gave up waiting for run 101 after 3 polls" \
    && has "step 8 (Wait for the Release workflow) failed"; then
    pass "a wait that runs out of polls stops at step 8"
else fail "a wait that runs out of polls stops at step 8 (rc=$rc: $(cat "$C/out"))"; fi
echo "completed success" > "$FAKE_STATE/101"
act; rc=$?
if [ "$rc" = 0 ] && released && [ "$(count "just ci")" = 2 ] && [ "$(count "bump 0.6.0")" = 1 ] \
    && has "Step 6: Tag the release: done already"; then
    pass "the next run resumes at the wait and finishes the release, tagging and testing once"
else fail "resume after a failed wait (rc=$rc: $(grep '^just' "$FAKE_STATE/calls"); $(cat "$C/out"))"; fi

# --- a release of the version from another build --------------------------------------
new_case
echo "2026-10-01T00:00:00Z" > "$FAKE_STATE/release-created"
echo "completed failure" > "$FAKE_STATE/101"
act; rc=$?
if [ "$rc" = 1 ] && has "step 8 (Wait for the Release workflow) failed" && has "gh release delete v0.6.0 --yes" \
    && has "its files are another build's" && [ "$(count "pin v0.6.0")" = 0 ]; then
    pass "a Release run failing on a release of another build stops before the pin, saying to delete it"
else fail "a Release run failing on a release of another build (rc=$rc: $(cat "$C/out"))"; fi
rm "$FAKE_STATE/release-created"
echo "completed success" > "$FAKE_STATE/101"
act; rc=$?
if [ "$rc" = 0 ] && released && [ "$(cat "$FAKE_STATE/release-created")" = "2026-10-20T00:00:10Z" ]; then
    pass "once that release is deleted and the run rerun, the next run finishes"
else fail "resume after deleting a release of another build (rc=$rc: $(cat "$C/out"))"; fi
new_case
echo "2026-10-01T00:00:00Z" > "$FAKE_STATE/release-created"
act; rc=$?
if [ "$rc" = 1 ] && has "published 2026-10-01T00:00:00Z, before the Release run 101" && [ "$(count "pin v0.6.0")" = 0 ]; then
    pass "a release published before this tag's run is never pinned, even when the run succeeds"
else fail "a release published before this tag's run (rc=$rc: $(cat "$C/out"))"; fi

# --- resume after main moves past a pushed pin commit ---------------------------------
# Someone else's commit lands on origin's main.
other_work() {
    local o="$C/origin.git" c
    c="$(GIT_AUTHOR_NAME=Other GIT_AUTHOR_EMAIL=other@example.com GIT_COMMITTER_NAME=Other \
        GIT_COMMITTER_EMAIL=other@example.com git --git-dir="$o" commit-tree "$(git --git-dir="$o" rev-parse 'main^{tree}')" \
        -p main -m "Other work")" && git --git-dir="$o" update-ref refs/heads/main "$c"
}
new_case
echo "completed failure" > "$FAKE_STATE/201"
act; rc=$?
if [ "$rc" = 1 ] && has "step 13 (Wait for CI on main) failed" && has "CI on main concluded failure" \
    && [ "$(count "gh workflow run publish-pi.yml --ref main -f dry_run=false -f sha=$(git --git-dir="$C/origin.git" rev-parse main)")" = 0 ]; then
    pass "failed CI on the pin commit stops at step 13, before the publish"
else fail "failed CI on the pin commit stops at step 13 (rc=$rc: $(cat "$C/out"))"; fi
pin="$(git --git-dir="$C/origin.git" rev-parse main)"
other_work
git -C "$C/repo" pull -q --ff-only origin main
echo "completed success" > "$FAKE_STATE/201"
act; rc=$?
if [ "$rc" = 0 ] && has "Step 12: Push main: done already" \
    && [ "$(count "gh workflow run publish-pi.yml --ref main -f dry_run=false -f sha=$pin")" = 1 ]; then
    pass "once main has moved past the pin commit, the next run resumes at the CI wait and publishes from the pin"
else fail "resume after main moved past the pin commit (rc=$rc: $(cat "$C/out"))"; fi
: > "$FAKE_STATE/calls"
act; rc=$?
if [ "$rc" = 0 ] && [ -z "$(actions)" ] && has "Step 14: Publish the Pi package: done already"; then
    pass "a finished release with main moved on reports every step done"
else fail "a finished release with main moved on (rc=$rc: $(cat "$C/out"))"; fi

# --- the Pi package comes from the pin commit -------------------------------------------
new_case
: > "$FAKE_STATE/push-on-201"
act; rc=$?
pin="$(git --git-dir="$C/origin.git" rev-parse main^)"
if [ "$rc" = 0 ] && [ "$(git --git-dir="$C/origin.git" log -1 --format=%s main)" = "Other work" ] \
    && [ "$(git --git-dir="$C/origin.git" log -1 --format=%s "$pin")" = "Pin the plugins to Clax 0.6.0" ] \
    && [ "$(count "gh workflow run publish-pi.yml --ref main -f dry_run=false -f sha=$pin")" = 1 ]; then
    pass "the Pi package is published from the pin commit after main moves on"
else fail "the Pi package is published from the pin commit (rc=$rc: $(grep 'workflow run' "$FAKE_STATE/calls"); $(cat "$C/out"))"; fi
new_case
echo "completed failure" > "$FAKE_STATE/301"
act; rc=$?
if [ "$rc" = 1 ] && has "step 14 (Publish the Pi package) failed" && has "Publish Pi package concluded failure"; then
    pass "a failed Publish Pi package run stops at step 14"
else fail "a failed Publish Pi package run stops at step 14 (rc=$rc: $(cat "$C/out"))"; fi
act; rc=$?
if [ "$rc" = 0 ] && [ "$(grep -c '^gh workflow run' "$FAKE_STATE/calls")" = 2 ] && has "Publish Pi package: run 302"; then
    pass "the next run dispatches the publish again and waits for the new run"
else fail "resume after a failed publish (rc=$rc: $(cat "$C/out"))"; fi

# --- gh failing during a wait ---------------------------------------------------------
new_case
echo 2 > "$FAKE_STATE/gh-fail"
act; rc=$?
if [ "$rc" = 0 ] && released; then pass "a wait rides out gh failing twice"
else fail "a wait rides out gh failing twice (rc=$rc: $(cat "$C/out"))"; fi
new_case
echo 99 > "$FAKE_STATE/gh-fail"
CLAX_RELEASE_MAX_POLLS=10 act; rc=$?
if [ "$rc" = 1 ] && has "gh failed 5 times in a row" && has "HTTP 502: Bad Gateway (fake)" \
    && has "step 8 (Wait for the Release workflow) failed"; then
    pass "gh failing on every poll stops the wait, naming gh's error"
else fail "gh failing on every poll stops the wait (rc=$rc: $(cat "$C/out"))"; fi

# --- preflight refusals --------------------------------------------------------------
# refuse NAME EXPECTED [ARGS...]: release.sh with ARGS (default: 0.6.0 and
# the act flag) exits 1 naming EXPECTED and changes nothing.
refuse() {
    local name="$1" expected="$2" before rc
    shift 2
    [ $# -gt 0 ] || set -- 0.6.0 --i-know-what-i-am-doing
    before="$(snapshot)"
    release "$@"; rc=$?
    if [ "$rc" = 1 ] && has "$expected" && [ -z "$(actions)" ] && [ "$before" = "$(snapshot)" ]; then
        pass "preflight refuses $name and changes nothing"
    else fail "preflight refuses $name (rc=$rc: $(actions); $(cat "$C/out"))"; fi
}
new_case
echo "local edit" >> "$C/repo/bazel/workspace.bzl"
refuse "a changed file" "uncommitted changes: bazel/workspace.bzl"
new_case
echo scratch > "$C/repo/notes.txt"
refuse "an untracked file" "untracked files: notes.txt"
new_case
git -C "$C/repo" -c commit.gpgsign=false commit -q --allow-empty -m "Elsewhere"
git -C "$C/repo" push -q origin main
git -C "$C/repo" reset -q --hard HEAD~1
refuse "a main behind origin/main" "main is 1 commit(s) behind origin/main"
new_case
git -C "$C/repo" -c commit.gpgsign=false commit -q --allow-empty -m "Unpushed"
refuse "a commit on main that is not the release's" 'not this release'"'"'s: "Unpushed"'
new_case
git -C "$C/repo" checkout -q -b side
refuse "a branch other than main" "the checkout is on side, not main"
new_case
refuse "the newest tag's version, a release on main with no pin" "v0.5.0's release commit is on origin/main without a \"Pin the plugins to Clax 0.5.0\" commit" 0.5.0 --i-know-what-i-am-doing
refuse "an older version" "0.4.9 is not newer than the tag v0.5.0" 0.4.9 --i-know-what-i-am-doing
refuse "a version that is not X.Y.Z" "0.6 is not a release version (X.Y.Z)" 0.6 --i-know-what-i-am-doing
refuse "a bad version in a dry run too" "0.6 is not a release version (X.Y.Z)" 0.6
git -C "$C/origin.git" tag v0.7.0 main
refuse "a version below a tag only origin has" "0.6.0 is not newer than the tag v0.7.0"
new_case
git -C "$C/repo" commit -q -S --allow-empty -m "Release 0.6.0"
git -C "$C/repo" -c commit.gpgsign=false revert -q --no-edit HEAD 2>/dev/null \
    || git -C "$C/repo" -c commit.gpgsign=false commit -q --allow-empty -m 'Revert "Release 0.6.0"'
git -C "$C/repo" push -q origin main
refuse "an old \"Release 0.6.0\" commit on origin/main with no tag" "origin/main has a \"Release 0.6.0\" commit already"
new_case
git -C "$C/repo" -c tag.gpgsign=false tag -a v0.6.0 -m "Clax 0.6.0"
refuse "a tag of the version here on another commit" "the tag v0.6.0 tags $(git -C "$C/repo" rev-parse HEAD), not a \"Release 0.6.0\" commit"
new_case
git --git-dir="$C/origin.git" tag v0.6.0 main
refuse "a tag of the version on origin on another commit" "the tag v0.6.0 tags $(git -C "$C/repo" rev-parse HEAD), not a \"Release 0.6.0\" commit"
new_case
: > "$FAKE_STATE/no-auth"
refuse "gh signed out" "gh is not signed in (gh auth login)"
new_case
git -C "$C/repo" config user.signingkey "$T/no-such-key"
refuse "a signing key that does not sign" "commit signing does not work here"
new_case
before="$(snapshot)"
release 0.6.0 --force; rc=$?
if [ "$rc" = 2 ] && [ "$before" = "$(snapshot)" ]; then pass "an unknown flag is a usage error"; else fail "an unknown flag is a usage error (rc=$rc)"; fi

exit "$FAILED"
