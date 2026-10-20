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
# (the last one sticks; none means "completed success"). Run 101 seen to
# succeed publishes the release ($FAKE_STATE/release) unless it exists, with
# a SOURCE_TREE asset ($FAKE_STATE/release-tree) of the tag commit's tree.
# $FAKE_STATE/earlier-run set to "same" or "other" stands for a release an
# earlier build made, of the tag commit's tree or of another tree: its
# SOURCE_TREE says so, or with $FAKE_STATE/no-tree-asset (a release from
# before that asset) the earlier successful run on the tag that uploaded it
# does. `gh api` reads commits' trees from origin. The fake prints what each
# --jq filter would.
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
# release.sh runs under the bash running this test (`/bin/bash
# scripts/test-release-script.sh` tries macOS's bash 3.2).
TEST_BASH="$BASH"
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
        # into MODULE.bazel.lock; the third run changes nothing. With
        # $FAKE_STATE/sync-forever, every run changes MODULE.bazel.lock.
        v="$(sed -n 's/^CLAX_VERSION="\(.*\)"$/\1/p' scripts/ensure-clax.sh)"
        if [ -e "$FAKE_STATE/sync-forever" ]; then
            echo "again" >> MODULE.bazel.lock
        elif [ "$(cat bazel/workspace.bzl)" != "version = $v" ]; then
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
        if [ -e "$S/release" ] && [ -n "$(origin tag -l "$3")" ]; then
            case " $* " in
                *" tagName "*) echo "$3" ;;
                *" createdAt "*) echo "2026-10-20T00:00:10Z" ;;
                *) printf 'clax-%s-aarch64-apple-darwin.tar.gz\ninstall.sh\nSOURCE_TREE\nSHA256SUMS\n' "${3#v}" ;;
            esac
        else
            echo "release not found" >&2; exit 1
        fi ;;
    "release download")
        commit="$(origin rev-parse "$3^{commit}")"
        if [ ! -e "$S/release" ]; then echo "release not found" >&2; exit 1; fi
        if [ -e "$S/no-tree-asset" ]; then echo "no assets match the file pattern" >&2; exit 1; fi
        if [ -s "$S/release-tree" ]; then cat "$S/release-tree"; exit 0; fi
        case "$(cat "$S/earlier-run" 2>/dev/null)" in
            same) origin rev-parse "$commit^{tree}" ;;
            other) origin rev-parse "$commit^^{tree}" ;;
            *) echo "no assets match the file pattern" >&2; exit 1 ;;
        esac ;;
    "api repos/"*)
        # The npm environment: limited to main unless $FAKE_STATE/no-env
        # (none), env-open (any branch) or env-branch (another branch).
        case "$2" in
            */environments/npm)
                if [ -e "$S/no-env" ]; then echo "gh: Not Found (HTTP 404)" >&2; exit 1; fi
                if [ -e "$S/env-open" ]; then echo none; else echo "false true"; fi ;;
            */environments/npm/deployment-branch-policies)
                if [ -e "$S/env-branch" ]; then echo "branch:release"; else echo "branch:main"; fi ;;
            */git/commits/*) origin rev-parse "${2##*/}^{tree}" ;;
            *) echo "fake gh: unexpected: $*" >&2; exit 2 ;;
        esac ;;
    "run list")
        sha="$(flag --commit "$@")"
        case "$(flag --workflow "$@")" in
            release.yml)
                # $FAKE_STATE/release-delay: how many listings miss the run.
                n="$(cat "$S/release-delay" 2>/dev/null || echo 0)"
                if [ "$(flag --status "$@")" = success ]; then
                    # The --jq filter's answer: the commit of the newest
                    # successful run before the release.
                    tag="$(flag --branch "$@")"
                    commit="$(origin rev-parse "$tag^{commit}")"
                    case "$(cat "$S/earlier-run" 2>/dev/null)" in
                        same) tree="$(origin rev-parse "$commit^{tree}")" ;;
                        other) tree="$(origin rev-parse "$commit^^{tree}")" ;;
                        *) tree="" ;;
                    esac
                    if [ -n "$tree" ]; then
                        GIT_AUTHOR_NAME=Earlier GIT_AUTHOR_EMAIL=e@example.com GIT_COMMITTER_NAME=Earlier \
                            GIT_COMMITTER_EMAIL=e@example.com GIT_AUTHOR_DATE=2026-10-01T00:00:00Z \
                            GIT_COMMITTER_DATE=2026-10-01T00:00:00Z origin commit-tree "$tree" -m "Release (earlier)"
                    fi
                elif [ "$n" -gt 0 ]; then
                    echo $((n - 1)) > "$S/release-delay"
                elif [ -n "$(origin tag --points-at "$sha" 'v*')" ]; then
                    echo "101 $sha"
                fi ;;
            ci.yml) if origin merge-base --is-ancestor "$sha" refs/heads/main 2>/dev/null; then echo "201 $sha"; fi ;;
            publish-pi.yml) cat "$S/publish-runs" 2>/dev/null || true ;;
            *) echo "fake gh: unexpected: $*" >&2; exit 2 ;;
        esac ;;
    "run view")
        # ci.yml's run 201 as a whole reads cancelled (its perf-log job), so
        # only its bazel job (--json jobs) says whether CI passed.
        case "$3 $*" in
            "201 "*"status,conclusion"*) echo "completed cancelled"; exit 0 ;;
            "201 "*" jobs "*) ;;
            "201 "*) echo "fake gh: unexpected: $*" >&2; exit 2 ;;
        esac
        # $FAKE_STATE/push-on-<ID>: the first view of run <ID> sees other work
        # land on origin's main.
        if [ -e "$S/push-on-$3" ]; then rm "$S/push-on-$3"; foreign_push; fi
        state="completed success"
        if [ -s "$S/$3" ]; then
            state="$(sed -n 1p "$S/$3")"
            if [ "$(wc -l < "$S/$3")" -gt 1 ]; then sed -i.bak 1d "$S/$3"; fi
        fi
        # A Publish Pi package run that succeeds puts its commit's version on npm.
        case "$3:$state" in
            30?:"completed success")
                sha="$(awk -F '\t' -v id="$3" '$1 == id { sub("Publish Pi package ", "", $2); print $2 }' "$S/publish-runs")"
                origin show "$sha:plugins/pi/package.json" | sed -n 's/^  "version": "\(.*\)",$/\1/p' >> "$S/npm-versions" ;;
        esac
        if [ "$3" = 101 ] && [ "$state" = "completed success" ] && [ ! -e "$S/release" ]; then
            : > "$S/release"
            # The release's tag is the highest version: tags made in the same
            # second tie on their dates, and git breaks the tie by name.
            origin rev-parse "$(origin tag -l 'v*' --sort=-version:refname | sed -n 1p)^{tree}" > "$S/release-tree"
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
# npm: the Pi package exists at 0.5.0 unless $FAKE_STATE/npm-absent does;
# $FAKE_STATE/npm-versions lists the versions it has besides; `npm whoami`
# fails while $FAKE_STATE/npm-no-login exists. A publish logs the commit it
# runs in and adds its package.json's version.
fake_exe "$T/bin/npm" <<'SH'
#!/bin/sh
S="$FAKE_STATE"
echo "npm $*" >> "$S/calls"
absent() {
    echo "npm error code E404" >&2
    echo "npm error 404 Not Found - GET https://registry.npmjs.org/$1 - Not found" >&2
    exit 1
}
case "$1" in
    view)
        [ ! -e "$S/npm-absent" ] || absent "$2"
        case "$2" in
            ?*@*) v="${2##*@}"; if grep -x "$v" "$S/npm-versions" 2>/dev/null; then :; fi ;;
            *) echo 0.5.0 ;;
        esac ;;
    whoami)
        if [ -e "$S/npm-no-login" ]; then echo "npm error code ENEEDAUTH" >&2; exit 1; fi
        echo owner ;;
    publish)
        # $FAKE_STATE/npm-hide: the registry has not listed it yet.
        echo "npm publish at $(git rev-parse HEAD)" >> "$S/calls"
        if [ -e "$S/npm-hide" ]; then
            sed -n 's/^  "version": "\(.*\)",$/\1/p' package.json >> "$S/npm-hidden"
        else
            sed -n 's/^  "version": "\(.*\)",$/\1/p' package.json >> "$S/npm-versions"
            rm -f "$S/npm-absent"
        fi ;;
    *) echo "fake npm: unexpected: $*" >&2; exit 2 ;;
esac
SH
fake_exe "$T/bin/pnpm" <<'SH'
#!/bin/sh
echo "pnpm $* (in $(basename "$(dirname "$PWD")")/$(basename "$PWD"))" >> "$FAKE_STATE/calls"
SH
FAKE_PATH="$T/bin:$PATH"
# origin's pre-receive hook: refuses a push of main while
# $FAKE_STATE/reject-push exists.
fake_exe "$T/pre-receive" <<'SH'
#!/bin/sh
main=0
while read -r old new ref; do
    if [ "$ref" = refs/heads/main ]; then main=1; fi
done
if [ "$main" = 1 ] && [ -e "$FAKE_STATE/reject-push" ]; then
    echo "fake origin: refusing main" >&2
    exit 1
fi
exit 0
SH

# A fresh case: $C/repo on main at 0.5.0, pinned to 0.5.0, tagged v0.4.0
# and v0.5.0, pushed to $C/origin.git.
new_case() {
    C="$(mktemp -d "$T/case.XXXXXX")"
    mkdir -p "$C/state" "$C/repo/scripts" "$C/repo/bazel" "$C/repo/plugins/pi/scripts"
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
    cp "$HERE/scripts/release-exists-check.sh" "$r/scripts/release-exists-check.sh"
    printf 'target/\n' > "$r/.gitignore"
    printf 'CLAX_VERSION="0.5.0"\nPINNED_VERSION="0.5.0"\n' > "$r/scripts/ensure-clax.sh"
    printf 'CLAX_VERSION="0.5.0"\nPINNED_VERSION="0.5.0"\n' > "$r/plugins/pi/scripts/ensure-clax.sh"
    printf '{\n  "name": "@empathic/clax-pi",\n  "version": "0.5.0",\n  "type": "module"\n}\n' > "$r/plugins/pi/package.json"
    echo "version = 0.5.0" > "$r/bazel/workspace.bzl"
    echo "version = 0.5.0" > "$r/MODULE.bazel.lock"
    cat > "$r/scripts/check-version.sh" <<'SH'
#!/usr/bin/env bash
cd "$(dirname "$0")/.." || exit 1
v="$(sed -n 's/^CLAX_VERSION="\(.*\)"$/\1/p' scripts/ensure-clax.sh)"
[ -n "$v" ] || { echo "versions differ" >&2; exit 1; }
echo "$v"
SH
    cat > "$r/scripts/bump-version.sh" <<'SH'
#!/usr/bin/env bash
set -e
cd "$(dirname "$0")/.."
echo "bump $1" >> "$FAKE_STATE/calls"
sed -i.bak "s/^CLAX_VERSION=.*/CLAX_VERSION=\"$1\"/" scripts/ensure-clax.sh plugins/pi/scripts/ensure-clax.sh
sed -i.bak "s/^  \"version\": \".*\",/  \"version\": \"$1\",/" plugins/pi/package.json
rm scripts/ensure-clax.sh.bak plugins/pi/scripts/ensure-clax.sh.bak plugins/pi/package.json.bak
SH
    cat > "$r/scripts/pin-release.sh" <<'SH'
#!/usr/bin/env bash
set -e
cd "$(dirname "$0")/.."
echo "pin $1" >> "$FAKE_STATE/calls"
sed -i.bak "s/^PINNED_VERSION=.*/PINNED_VERSION=\"${1#v}\"/" scripts/ensure-clax.sh plugins/pi/scripts/ensure-clax.sh
rm scripts/ensure-clax.sh.bak plugins/pi/scripts/ensure-clax.sh.bak
SH
    git -C "$r" add -A
    git -C "$r" -c commit.gpgsign=false commit -q -m "Clax 0.4.0"
    git -C "$r" -c tag.gpgsign=false tag -a v0.4.0 -m "Clax 0.4.0"
    git -C "$r" -c commit.gpgsign=false commit -q --allow-empty -m "Release 0.5.0"
    git -C "$r" -c tag.gpgsign=false tag -a v0.5.0 -m "Clax 0.5.0"
    git -C "$r" remote add origin "$C/origin.git"
    ln -s "$(readlink "$T/pre-receive")" "$C/origin.git/hooks/pre-receive"
    git -C "$r" push -q origin main v0.4.0 v0.5.0
    git -C "$r" fetch -q origin
}

# Runs release.sh from another directory; its output is in $C/out.
release() {
    (cd "$T" && PATH="${CASE_PATH:-$FAKE_PATH}" "$TEST_BASH" "$C/repo/scripts/release.sh" "$@") > "$C/out" 2>&1
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
actions() { grep -E '^(bump|pin|just|gh workflow run|npm publish|pnpm)' "$FAKE_STATE/calls" 2>/dev/null; }
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

# Someone else's commit lands on origin's main.
other_work() {
    local o="$C/origin.git" c
    c="$(GIT_AUTHOR_NAME=Other GIT_AUTHOR_EMAIL=other@example.com GIT_COMMITTER_NAME=Other \
        GIT_COMMITTER_EMAIL=other@example.com git --git-dir="$o" commit-tree "$(git --git-dir="$o" rev-parse 'main^{tree}')" \
        -p main -m "Other work")" && git --git-dir="$o" update-ref refs/heads/main "$c"
}
# merged_release: origin's main is a signed merge of other work on top of the
# pin commit, the tag still on the release commit under it, and the Pi
# package dispatched from the merge.
merged_release() {
    local o="$C/origin.git" m
    m="$(git --git-dir="$o" rev-parse main)"
    [ "$(git --git-dir="$o" log -1 --format=%s "$m")" = "Merge origin/main into the Clax 0.6.0 release" ] \
        && [ "$(git --git-dir="$o" log -1 --format=%s "$m^1")" = "Pin the plugins to Clax 0.6.0" ] \
        && [ "$(git --git-dir="$o" log -1 --format=%s "$m^2")" = "Other work" ] \
        && [ "$(git --git-dir="$o" rev-parse 'v0.6.0^{commit}')" = "$(git --git-dir="$o" rev-parse "$m^1^")" ] \
        && git -C "$C/repo" verify-commit "$m" 2>/dev/null && git -C "$C/repo" verify-tag v0.6.0 2>/dev/null \
        && [ "$(count "gh workflow run publish-pi.yml --ref main -f dry_run=false -f sha=$m")" = 1 ] \
        && [ "$(git -C "$C/repo" rev-parse HEAD)" = "$m" ] && [ -z "$(git -C "$C/repo" status --porcelain)" ]
}
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

# --- the cases ---------------------------------------------------------------------
# Each case runs in a subshell of its own (case_N below), several at a time,
# and their output is printed in order.
case_1() {
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
}
case_2() {

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
}
case_3() {

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
}
case_4() {

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
}
case_5() {

    # --- a release of the version from an earlier build ----------------------------------
    # The release exists already, built by an earlier run from a commit with
    # the same tree (its SOURCE_TREE says so), as after re-signing the tagged
    # commit: it is this tag's build, even before the tag's own run is done.
    new_case
    : > "$FAKE_STATE/release"
    echo same > "$FAKE_STATE/earlier-run"
    echo "in_progress " > "$FAKE_STATE/101"
    act; rc=$?
    if [ "$rc" = 0 ] && released && has "Step 8: Wait for the Release workflow: done already"; then
        pass "a release built from the tag's tree by an earlier run, as after re-signing, is accepted"
    else fail "a release built from the tag's tree by an earlier run (rc=$rc: $(cat "$C/out"))"; fi
}
case_5b() {
    # A release from before SOURCE_TREE: the run that uploaded it, the newest
    # successful one before it, names its tree.
    new_case
    : > "$FAKE_STATE/release"
    : > "$FAKE_STATE/no-tree-asset"
    echo same > "$FAKE_STATE/earlier-run"
    echo "in_progress " > "$FAKE_STATE/101"
    act; rc=$?
    if [ "$rc" = 0 ] && released && has "Step 8: Wait for the Release workflow: done already"; then
        pass "a release with no SOURCE_TREE is judged by the run that uploaded it"
    else fail "a release with no SOURCE_TREE judged by its run (rc=$rc: $(cat "$C/out"))"; fi
}
case_5c() {
    # A release from before SOURCE_TREE with no successful run before it.
    new_case
    : > "$FAKE_STATE/release"
    : > "$FAKE_STATE/no-tree-asset"
    echo "completed failure" > "$FAKE_STATE/101"
    act; rc=$?
    if [ "$rc" = 1 ] && has "has no SOURCE_TREE and no successful Release run on v0.6.0 before it" \
        && has "holds a build of an unknown tree" && [ "$(count "pin v0.6.0")" = 0 ]; then
        pass "a release no rule can place is never pinned"
    else fail "a release no rule can place (rc=$rc: $(cat "$C/out"))"; fi
}
case_6() {
    # Made from another tree: the tag's own run fails on it, and it is never pinned.
    new_case
    : > "$FAKE_STATE/release"
    echo other > "$FAKE_STATE/earlier-run"
    echo "completed failure" > "$FAKE_STATE/101"
    act; rc=$?
    if [ "$rc" = 1 ] && has "step 8 (Wait for the Release workflow) failed" && has "gh release delete v0.6.0 --yes" \
        && has "holds a build of $(git --git-dir="$C/origin.git" rev-parse 'v0.6.0^^{tree}'), not of this tag's tree" \
        && [ "$(count "pin v0.6.0")" = 0 ]; then
        pass "a release built from another tree stops before the pin, saying to delete it"
    else fail "a release built from another tree (rc=$rc: $(cat "$C/out"))"; fi
    rm "$FAKE_STATE/release" "$FAKE_STATE/earlier-run"
    echo "completed success" > "$FAKE_STATE/101"
    act; rc=$?
    if [ "$rc" = 0 ] && released && [ -e "$FAKE_STATE/release" ]; then
        pass "once that release is deleted and the run rerun, the next run finishes"
    else fail "resume after deleting a release of another tree (rc=$rc: $(cat "$C/out"))"; fi
}
case_7() {
    # The tag's own run succeeding does not make another tree's release its own.
    new_case
    : > "$FAKE_STATE/release"
    echo other > "$FAKE_STATE/earlier-run"
    act; rc=$?
    if [ "$rc" = 1 ] && has "not of this tag's tree" && [ "$(count "pin v0.6.0")" = 0 ]; then
        pass "a release built from another tree is never pinned, even when the tag's run succeeds"
    else fail "a release built from another tree, the tag's run succeeding (rc=$rc: $(cat "$C/out"))"; fi
}
case_8() {

    # --- resume after main moves past a pushed pin commit ---------------------------------
    new_case
    echo "completed failure" > "$FAKE_STATE/201"
    act; rc=$?
    if [ "$rc" = 1 ] && has "step 13 (Wait for CI on main) failed" && has "CI on main (bazel) concluded failure" \
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
}
case_9() {

    # --- main moving during the release -------------------------------------------------
    new_case
    : > "$FAKE_STATE/push-on-101"
    act; rc=$?
    if [ "$rc" = 0 ] && merged_release && has "origin/main has moved on; merging it"; then
        pass "main moving during the Release wait is merged on top of the pin commit, signed, keeping the tag"
    else fail "main moving during the Release wait (rc=$rc: $(cat "$C/out"))"; fi
}
case_10() {
    new_case
    : > "$FAKE_STATE/reject-push"
    act; rc=$?
    if [ "$rc" = 1 ] && has "step 12 (Push main) failed" && has "the push of main was refused" \
        && [ "$(git --git-dir="$C/origin.git" log -1 --format=%s main)" = "Release 0.5.0" ] \
        && [ "$(git -C "$C/repo" log -1 --format=%s)" = "Pin the plugins to Clax 0.6.0" ]; then
        pass "a refused push of main stops at step 12 with the pin commit made"
    else fail "a refused push of main stops at step 12 (rc=$rc: $(cat "$C/out"))"; fi
    rm "$FAKE_STATE/reject-push"
    other_work
    act; rc=$?
    if [ "$rc" = 0 ] && merged_release && has "Step 11: Commit the pin: done already"; then
        pass "after main moved on, the next run merges it on top of the pin commit and finishes"
    else fail "resume after a refused push with main moved on (rc=$rc: $(cat "$C/out"))"; fi
}
case_11() {

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
}
case_12() {
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
}
case_13() {

    # --- gh failing during a wait ---------------------------------------------------------
    new_case
    echo 2 > "$FAKE_STATE/gh-fail"
    act; rc=$?
    if [ "$rc" = 0 ] && released; then pass "a wait rides out gh failing twice"
    else fail "a wait rides out gh failing twice (rc=$rc: $(cat "$C/out"))"; fi
}
case_14() {
    new_case
    echo 99 > "$FAKE_STATE/gh-fail"
    CLAX_RELEASE_MAX_POLLS=10 act; rc=$?
    if [ "$rc" = 1 ] && has "gh failed 5 times in a row" && has "HTTP 502: Bad Gateway (fake)" \
        && has "step 8 (Wait for the Release workflow) failed"; then
        pass "gh failing on every poll stops the wait, naming gh's error"
    else fail "gh failing on every poll stops the wait (rc=$rc: $(cat "$C/out"))"; fi
}
case_15() {

    # --- the Pi package on npm ---------------------------------------------------------
    # Not on npm yet: the first version is published from here, from the pin
    # commit, and the trusted-publisher setup is printed.
    new_case
    : > "$FAKE_STATE/npm-absent"
    release 0.6.0; rc=$?
    if [ "$rc" = 0 ] && has "@empathic/clax-pi is not on npm yet: step 14 publishes its first version from this machine"; then
        pass "the dry run says the Pi package's first version is published from here"
    else fail "the dry run with the Pi package not on npm (rc=$rc: $(cat "$C/out"))"; fi
    act; rc=$?
    pin="$(git --git-dir="$C/origin.git" rev-parse main)"
    if [ "$rc" = 0 ] && released && [ "$(count "npm publish at $pin")" = 1 ] && [ "$(count "npm publish --access public")" = 1 ] \
        && [ "$(count "pnpm install --frozen-lockfile (in plugins/pi)")" = 1 ] && [ "$(count "pnpm run typecheck (in plugins/pi)")" = 1 ] \
        && [ "$(grep -c '^gh workflow run' "$FAKE_STATE/calls")" = 0 ] \
        && has "GitHub Actions: organization empathic, repository clax-test, workflow" \
        && has "publish-pi.yml, environment npm." \
        && has "switch README.md and plugins/pi/README.md to lead their Pi install with" \
        && [ ! -e "$C/repo/target/release/0.6.0/pi-publish" ] \
        && [ "$(git -C "$C/repo" worktree list | wc -l | tr -d ' ')" = 1 ]; then
        pass "a Pi package not on npm is published from here, from the pin commit, with the trusted-publisher setup"
    else fail "the first Pi publish (rc=$rc: $(actions); $(cat "$C/out"))"; fi
}
case_15b() {
    # The npm environment missing or open to more than main: a dry run warns
    # with the steps to make it, a release refuses before anything.
    new_case
    : > "$FAKE_STATE/no-env"
    release 0.6.0; rc=$?
    if [ "$rc" = 0 ] && has "note: WARN: the repository has no environment npm" \
        && has "Settings, Environments, New environment (or npm), name npm; Deployment branches and tags: Selected branches and tags; Add deployment branch or tag rule: branch main"; then
        pass "a dry run warns that the npm environment is missing, with the steps to make it"
    else fail "a dry run with no npm environment (rc=$rc: $(cat "$C/out"))"; fi
    refuse "a release with no npm environment" "FAIL: the repository has no environment npm"
    rm "$FAKE_STATE/no-env"
    : > "$FAKE_STATE/env-open"
    refuse "an npm environment open to every branch" "the environment npm does not limit its deployment branches to main"
    rm "$FAKE_STATE/env-open"
    : > "$FAKE_STATE/env-branch"
    refuse "an npm environment for another branch" "the environment npm allows branch:release, not main alone"
    rm "$FAKE_STATE/env-branch"
    # The version on npm already: the environment no longer matters.
    : > "$FAKE_STATE/no-env"
    echo 0.6.0 > "$FAKE_STATE/npm-versions"
    release 0.6.0; rc=$?
    if [ "$rc" = 0 ] && ! has "WARN"; then pass "a version npm has already needs no npm environment"
    else fail "a version npm has already, with no npm environment (rc=$rc: $(cat "$C/out"))"; fi
}
case_16() {
    # A publish npm has not listed yet when the wait gives up: the rerun finds
    # step 14 done and still prints the trusted-publisher setup.
    new_case
    : > "$FAKE_STATE/npm-absent"
    : > "$FAKE_STATE/npm-hide"
    act; rc=$?
    if [ "$rc" = 1 ] && has "npm does not list @empathic/clax-pi@0.6.0 after the publish" \
        && [ -e "$C/repo/target/release/0.6.0/pi-first-publish" ]; then
        pass "a first publish npm has not listed yet stops step 14, keeping the publish recorded"
    else fail "a first publish npm has not listed yet (rc=$rc: $(cat "$C/out"))"; fi
    rm "$FAKE_STATE/npm-absent" "$FAKE_STATE/npm-hide"
    mv "$FAKE_STATE/npm-hidden" "$FAKE_STATE/npm-versions"
    act; rc=$?
    if [ "$rc" = 0 ] && has "Step 14: Publish the Pi package: done already" \
        && has "once: set up npm trusted publishing for @empathic/clax-pi" \
        && [ "$(count "npm publish --access public")" = 1 ]; then
        pass "the rerun finds the publish done and prints the trusted-publisher setup"
    else fail "the rerun after a first publish npm listed late (rc=$rc: $(cat "$C/out"))"; fi
}
case_16c() {
    # A merge of origin/main made by hand and committed unsigned: step 12
    # refuses it, saying how to sign it; signed, the next run pushes it.
    new_case
    : > "$FAKE_STATE/reject-push"
    act
    rm "$FAKE_STATE/reject-push"
    other_work
    git -C "$C/repo" fetch -q origin main
    git -C "$C/repo" -c commit.gpgsign=false merge -q --no-ff -m "Merge origin/main into the Clax 0.6.0 release" origin/main
    act; rc=$?
    if [ "$rc" = 1 ] && has "carries no signature; not pushing main." \
        && has "Sign it (git commit --amend -S --no-edit), then run this again." \
        && [ "$(git --git-dir="$C/origin.git" log -1 --format=%s main)" = "Other work" ]; then
        pass "an unsigned hand-made merge is not pushed, and the refusal says how to sign it"
    else fail "an unsigned hand-made merge (rc=$rc: $(cat "$C/out"))"; fi
    git -C "$C/repo" commit -q --amend -S --no-edit
    act; rc=$?
    if [ "$rc" = 0 ] && merged_release; then
        pass "once signed as told, the next run pushes the merge and finishes"
    else fail "the hand-made merge once signed (rc=$rc: $(cat "$C/out"))"; fi
}
case_16b() {
    # On npm: the workflow publishes, never npm here.
    new_case
    act; rc=$?
    if [ "$rc" = 0 ] && released && [ "$(count "npm publish --access public")" = 0 ] \
        && [ "$(grep -c '^gh workflow run publish-pi.yml' "$FAKE_STATE/calls")" = 1 ] && ! has "Trusted Publisher"; then
        pass "a Pi package on npm is published through publish-pi.yml"
    else fail "a Pi package on npm is published through publish-pi.yml (rc=$rc: $(cat "$C/out"))"; fi
}
case_17() {
    # The version on npm already: step 14 is done.
    new_case
    echo 0.6.0 > "$FAKE_STATE/npm-versions"
    act; rc=$?
    if [ "$rc" = 0 ] && has "note: @empathic/clax-pi@0.6.0 is on npm already: step 14 is done" \
        && has "Step 14: Publish the Pi package: done already" && ! grep -E '^(gh workflow run|npm publish)' "$FAKE_STATE/calls" >/dev/null; then
        pass "a version npm has already is not published again"
    else fail "a version npm has already (rc=$rc: $(cat "$C/out"))"; fi
}
case_18() {

    # --- preflight refusals --------------------------------------------------------------
    new_case
    echo "local edit" >> "$C/repo/bazel/workspace.bzl"
    refuse "a changed file" "uncommitted changes: bazel/workspace.bzl"
}
case_19() {
    new_case
    echo scratch > "$C/repo/notes.txt"
    refuse "an untracked file" "untracked files: notes.txt"
}
case_20() {
    new_case
    git -C "$C/repo" -c commit.gpgsign=false commit -q --allow-empty -m "Elsewhere"
    git -C "$C/repo" push -q origin main
    git -C "$C/repo" reset -q --hard HEAD~1
    refuse "a main behind origin/main" "main is 1 commit(s) behind origin/main"
}
case_21() {
    new_case
    git -C "$C/repo" -c commit.gpgsign=false commit -q --allow-empty -m "Unpushed"
    refuse "a commit on main that is not the release's" 'not this release'"'"'s: "Unpushed"'
}
case_22() {
    new_case
    git -C "$C/repo" checkout -q -b side
    refuse "a branch other than main" "the checkout is on side, not main"
}
case_23() {
    new_case
    refuse "the newest tag's version, a release on main with no pin" "v0.5.0's release commit is on origin/main without a \"Pin the plugins to Clax 0.5.0\" commit" 0.5.0 --i-know-what-i-am-doing
    refuse "an older version" "0.4.9 is not newer than the tag v0.5.0" 0.4.9 --i-know-what-i-am-doing
    refuse "a version that is not X.Y.Z" "0.6 is not a release version (X.Y.Z)" 0.6 --i-know-what-i-am-doing
    refuse "a bad version in a dry run too" "0.6 is not a release version (X.Y.Z)" 0.6
    git -C "$C/origin.git" tag v0.7.0 main
    refuse "a version below a tag only origin has" "0.6.0 is not newer than the tag v0.7.0"
}
case_24() {
    new_case
    git -C "$C/repo" commit -q -S --allow-empty -m "Release 0.6.0"
    git -C "$C/repo" -c commit.gpgsign=false revert -q --no-edit HEAD 2>/dev/null \
        || git -C "$C/repo" -c commit.gpgsign=false commit -q --allow-empty -m 'Revert "Release 0.6.0"'
    git -C "$C/repo" push -q origin main
    refuse "an old \"Release 0.6.0\" commit on origin/main with no tag" "origin/main has a \"Release 0.6.0\" commit already"
}
case_25() {
    new_case
    git -C "$C/repo" -c tag.gpgsign=false tag -a v0.6.0 -m "Clax 0.6.0"
    refuse "a tag of the version here on another commit" "the tag v0.6.0 tags $(git -C "$C/repo" rev-parse HEAD), not a \"Release 0.6.0\" commit"
}
case_26() {
    new_case
    git --git-dir="$C/origin.git" tag v0.6.0 main
    refuse "a tag of the version on origin on another commit" "the tag v0.6.0 tags $(git -C "$C/repo" rev-parse HEAD), not a \"Release 0.6.0\" commit"
}
case_27() {
    new_case
    : > "$FAKE_STATE/no-auth"
    refuse "gh signed out" "gh is not signed in (gh auth login)"
}
case_28() {
    new_case
    : > "$FAKE_STATE/npm-absent"
    : > "$FAKE_STATE/npm-no-login"
    refuse "a first Pi publish with npm signed out" "is not on npm yet, so step 14 publishes its first version from here: sign in first (npm login; npm whoami fails)"
}
case_29() {
    new_case
    git -C "$C/repo" config user.signingkey "$T/no-such-key"
    refuse "a signing key that does not sign" "commit signing does not work here"
}
case_30() {
    new_case
    sed -i.bak 's/^CLAX_VERSION="0.5.0"/CLAX_VERSION="0.7.0"/' "$C/repo/scripts/ensure-clax.sh"
    git -C "$C/repo" -c commit.gpgsign=false commit -q -am "Bump to 0.7.0"
    git -C "$C/repo" push -q origin main
    refuse "a checkout at a newer version" "the checkout is at 0.7.0, newer than 0.6.0"
}
case_31() {
    new_case
    sed -i.bak 's/^CLAX_VERSION=/CLAX_VERSION_GONE=/' "$C/repo/scripts/ensure-clax.sh"
    git -C "$C/repo" -c commit.gpgsign=false commit -q -am "Lose a version"
    git -C "$C/repo" push -q origin main
    refuse "versions that disagree" "the checkout's versions disagree (scripts/check-version.sh)"
}
case_32() {
    new_case
    mkdir -p "$T/no-just"
    for tool in gh bazelisk; do [ -e "$T/no-just/$tool" ] || ln -s "$(readlink "$T/bin/$tool")" "$T/no-just/$tool"; done
    CASE_PATH="$T/no-just:/usr/bin:/bin" refuse "a missing just" "just is not on PATH"
}
case_33() {
    new_case
    GH_REPO="" refuse "an origin that is not on GitHub, with no GH_REPO" "is not a GitHub repository; set GH_REPO=owner/name"
}
case_34() {

    # --- the Release run listed late, or never -------------------------------------------
    new_case
    echo 2 > "$FAKE_STATE/release-delay"
    act; rc=$?
    if [ "$rc" = 0 ] && released && [ "$(grep -c '^gh run list --workflow release.yml' "$FAKE_STATE/calls")" -ge 3 ]; then
        pass "the wait polls until the Release run is listed"
    else fail "the wait polls until the Release run is listed (rc=$rc: $(cat "$C/out"))"; fi
}
case_35() {
    new_case
    echo 99 > "$FAKE_STATE/release-delay"
    act; rc=$?
    if [ "$rc" = 1 ] && has "nothing after 3 polls" && has "no Release workflow run on v0.6.0 appeared" \
        && has "step 8 (Wait for the Release workflow) failed"; then
        pass "a Release run that is never listed stops the wait after its polls"
    else fail "a Release run that is never listed (rc=$rc: $(cat "$C/out"))"; fi
}
case_36() {

    # --- bazel-sync that never settles --------------------------------------------------
    new_case
    : > "$FAKE_STATE/sync-forever"
    act; rc=$?
    if [ "$rc" = 1 ] && [ "$(count "just bazel-sync")" = 4 ] && has "just bazel-sync still changed files on its 4th run" \
        && has "step 3 (Sync Bazel with Cargo) failed" && [ "$(count "just ci")" = 0 ]; then
        pass "bazel-sync still changing files after 4 runs stops at step 3"
    else fail "bazel-sync that never settles (rc=$rc: $(cat "$C/out"))"; fi
}
case_37() {

    new_case
    before="$(snapshot)"
    release 0.6.0 --force; rc=$?
    if [ "$rc" = 2 ] && [ "$before" = "$(snapshot)" ]; then pass "an unknown flag is a usage error"; else fail "an unknown flag is a usage error (rc=$rc)"; fi
}

JOBS="$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 4)"
[ "$JOBS" -le 8 ] || JOBS=8
CASES="case_1 case_2 case_3 case_4 case_5 case_5b case_5c case_6 case_7 case_8 case_9 case_10 case_11 case_12 case_13 case_14 case_15 case_15b case_16 case_16b case_16c case_17 case_18 case_19 case_20 case_21 case_22 case_23 case_24 case_25 case_26 case_27 case_28 case_29 case_30 case_31 case_32 case_33 case_34 case_35 case_36 case_37"
# A case's subshell exits 0 once its last check has run; one that dies on
# the way (an unbound variable under set -u, say) exits otherwise, and fails
# the gate whatever it printed before. Each case's status is collected by its
# PID.
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
