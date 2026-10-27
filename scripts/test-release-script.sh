#!/usr/bin/env bash
# Tests scripts/release.sh in scratch git repositories, each with a bare
# origin holding main and prod, with fakes of gh, just and bazelisk on PATH
# and an SSH key of the test's own for signing (gpg.format=ssh in each
# scratch repository's own config). No network, no GitHub; git reads no
# global or system config.
#
# A scratch repository holds a copy of release.sh and stand-ins for the
# scripts it runs (bump-version.sh, check-version.sh, pin-release.sh write
# and read CLAX_VERSION and PINNED_VERSION in scripts/ensure-clax.sh; their
# own tests are scripts/test-release.sh). The fakes log every call to
# $FAKE_STATE/calls. The fake gh plays GitHub from the bare origin: a pull
# request ($FAKE_STATE/pr, "STATE URL") once one is created, which the
# test's merge_pr merges as GitHub would, with a signed merge commit on
# prod; a Release run (ID 101) once a tag on the commit is pushed; a CI run
# (201) once prod or main holds the commit; a publish-pi run (301, 302, ...)
# per dispatch, titled as .github/workflows/publish-pi.yml titles it.
# $FAKE_STATE/<ID> lists the states `gh run view <ID>` reports, one per call
# (the last one sticks; none means "completed success"). Run 101 seen to
# succeed publishes the release ($FAKE_STATE/release) unless it exists, with
# a SOURCE_TREE asset ($FAKE_STATE/release-tree) of the tag commit's tree.
# $FAKE_STATE/earlier-run set to "same" or "other" stands for a release an
# earlier build made, of the tag commit's tree or of another tree: its
# SOURCE_TREE says so, or with $FAKE_STATE/no-tree-asset (a release from
# before that asset) the earlier successful run on the tag that uploaded it
# does. `gh api` reads commits' trees from origin. The fake prints what each
# --jq filter would. Origin refuses merge commits on main, as GitHub does
# (GH013).
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
# No heartbeat but in the case that checks it; each case's clock stands
# still at $C/now unless a fake moves it.
export CLAX_RELEASE_HEARTBEAT_SECONDS=0
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
        # $FAKE_STATE/ci-advance: seconds each run moves the clock on.
        if [ -s "$FAKE_STATE/ci-advance" ]; then
            echo $(( $(cat "$CLAX_RELEASE_NOW_FILE") + $(cat "$FAKE_STATE/ci-advance") )) > "$CLAX_RELEASE_NOW_FILE"
        fi
        # $FAKE_STATE/ci-heartbeat: the next run prints Bazel's progress and
        # waits, at most a minute, for a heartbeat telling it in release.sh's
        # output ($FAKE_OUT).
        if [ -e "$FAKE_STATE/ci-heartbeat" ]; then
            rm "$FAKE_STATE/ci-heartbeat"
            echo "[1,000 / 2,000] 40 / 56 tests; Testing //scripts:release_script; 3s darwin-sandbox"
            i=0
            while [ "$i" -lt 1200 ] && ! grep -F "40/56 tests" "$FAKE_OUT" >/dev/null 2>&1; do
                sleep 0.05
                i=$((i + 1))
            done
        fi
        if [ -e "$FAKE_STATE/ci-fail" ]; then echo "fake ci: a gate failed"; exit 1; fi
        echo "fake ci: every gate passed" ;;
    *) echo "fake just: unexpected: $*" >&2; exit 2 ;;
esac
SH
fake_exe "$T/bin/gh" <<'SH'
#!/bin/sh
S="$FAKE_STATE"
echo "gh $*" >> "$S/calls"
# A heartbeat's count of a run's jobs.
case "$*" in *"jobs done"*) echo "1/3 jobs done"; exit 0 ;; esac
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
    "pr list")
        cat "$S/pr" 2>/dev/null || true ;;
    "pr create")
        # $FAKE_STATE/pr-create-fail: GitHub refuses it.
        if [ -e "$S/pr-create-fail" ]; then echo "GraphQL: Something went wrong (fake)" >&2; exit 1; fi
        url="https://github.com/$GH_REPO/pull/7"
        echo "OPEN $url" > "$S/pr"
        echo "$url" ;;
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
        # The npm environment: limited to prod unless $FAKE_STATE/no-env
        # (none), env-open (any branch) or env-branch (another branch).
        case "$2" in
            */environments/npm)
                if [ -e "$S/no-env" ]; then echo "gh: Not Found (HTTP 404)" >&2; exit 1; fi
                if [ -e "$S/env-open" ]; then echo none; else echo "false true"; fi ;;
            */environments/npm/deployment-branch-policies)
                if [ -e "$S/env-branch" ]; then echo "branch:main"; else echo "branch:prod"; fi ;;
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
            ci.yml)
                if origin merge-base --is-ancestor "$sha" refs/heads/prod 2>/dev/null \
                    || origin merge-base --is-ancestor "$sha" refs/heads/main 2>/dev/null; then echo "201 $sha"; fi ;;
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
# origin's pre-receive hook: refuses a merge commit on main, as GitHub's
# rules on main do (GH013), and a push of main, prod or a release branch
# while $FAKE_STATE/reject-push, reject-prod or reject-branch exists.
fake_exe "$T/pre-receive" <<'SH'
#!/bin/sh
while read -r old new ref; do
    case "$ref" in
        refs/heads/main)
            if [ -e "$FAKE_STATE/reject-push" ]; then echo "fake origin: refusing main" >&2; exit 1; fi
            range="$old..$new"
            case "$old" in *[!0]*) ;; *) range="$new" ;; esac
            if [ -n "$(git rev-list --merges "$range")" ]; then
                echo "GH013: Repository rule violations: this branch must not contain merge commits (fake)" >&2
                exit 1
            fi ;;
        refs/heads/prod)
            if [ -e "$FAKE_STATE/reject-prod" ]; then echo "fake origin: refusing prod" >&2; exit 1; fi ;;
        refs/heads/release/*)
            if [ -e "$FAKE_STATE/reject-branch" ]; then echo "fake origin: refusing $ref" >&2; exit 1; fi ;;
    esac
done
exit 0
SH

# A fresh case: $C/repo on main, pushed to $C/origin.git. The history: "Clax
# 0.4.0" (v0.4.0), "Release 0.5.0" (v0.5.0, at 0.5.0 and pinned to it),
# which prod is at ($PROD0), and on main one commit more, "Main work"
# ($MAIN0).
new_case() {
    C="$(mktemp -d "$T/case.XXXXXX")"
    mkdir -p "$C/state" "$C/repo/scripts" "$C/repo/bazel" "$C/repo/plugins/pi/scripts"
    export FAKE_STATE="$C/state" FAKE_REMOTE="$C/origin.git" FAKE_OUT="$C/out" CLAX_RELEASE_NOW_FILE="$C/now"
    echo 1000000 > "$C/now"
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
    git -C "$r" branch prod
    echo "main work" > "$r/main.txt"
    git -C "$r" add main.txt
    git -C "$r" -c commit.gpgsign=false commit -q -m "Main work"
    git -C "$r" remote add origin "$C/origin.git"
    ln -s "$(readlink "$T/pre-receive")" "$C/origin.git/hooks/pre-receive"
    git -C "$r" push -q origin main prod v0.4.0 v0.5.0
    git -C "$r" fetch -q origin
    PROD0="$(git -C "$r" rev-parse prod)"
    MAIN0="$(git -C "$r" rev-parse main)"
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
    cat "$C/repo/scripts/ensure-clax.sh" "$C/repo/bazel/workspace.bzl" "$C/repo/MODULE.bazel.lock" "$FAKE_STATE/pr" 2>/dev/null
}
# The calls that change something.
actions() { grep -E '^(bump|pin|just|gh pr create|gh workflow run|npm publish|pnpm)' "$FAKE_STATE/calls" 2>/dev/null; }
count() { grep -c -x -- "$1" "$FAKE_STATE/calls"; }
has() { grep -F -- "$1" "$C/out" >/dev/null; }
# The release log has it.
logs() { grep -F -- "$1" "$C/repo/target/release/0.6.0.log" >/dev/null 2>&1; }
o() { git --git-dir="$C/origin.git" "$@"; }
subject() { o log -1 --format=%s "$1"; }
signed() { git -C "$C/repo" verify-commit "$1" 2>/dev/null; }

# The first line of the pull request's body, in Markdown.
# shellcheck disable=SC2016
PR_BODY_LINE='Clax 0.6.0: `origin/prod` with `origin/main` merged in and the version bumped (`scripts/release.sh 0.6.0`).'
# The preparation a run leaves: origin's release/0.6.0 a signed "Release
# 0.6.0" commit on a signed merge of main into prod's tip, the checkout on
# it and clean, the pull request open, and prod and main as they were.
prepared_ok() {
    local rel
    rel="$(o rev-parse -q --verify refs/heads/release/0.6.0)" || return 1
    [ "$(subject "$rel")" = "Release 0.6.0" ] \
        && [ "$(subject "$rel^")" = "Merge origin/main into release/0.6.0" ] \
        && [ "$(o rev-parse "$rel^^1")" = "$PROD0" ] && [ "$(o rev-parse "$rel^^2")" = "$MAIN0" ] \
        && signed "$rel" && signed "$rel^" \
        && [ "$(o show "$rel:scripts/ensure-clax.sh")" = "$(printf 'CLAX_VERSION="0.6.0"\nPINNED_VERSION="0.5.0"')" ] \
        && [ "$(o show "$rel:MODULE.bazel.lock")" = "version = 0.6.0" ] \
        && [ "$(git -C "$C/repo" symbolic-ref --short HEAD)" = release/0.6.0 ] \
        && [ "$(git -C "$C/repo" rev-parse HEAD)" = "$rel" ] && [ -z "$(git -C "$C/repo" status --porcelain)" ] \
        && [ "$(cat "$FAKE_STATE/pr")" = "OPEN https://github.com/empathic/clax-test/pull/7" ] \
        && grep -x -F "gh pr create --base prod --head release/0.6.0 --title Release 0.6.0 --body $PR_BODY_LINE" "$FAKE_STATE/calls" >/dev/null \
        && [ "$(o rev-parse prod)" = "$PROD0" ] && [ "$(o rev-parse main)" = "$MAIN0" ] && [ -z "$(o tag -l v0.6.0)" ]
}

# GitHub merges the pull request: a signed merge commit of origin's
# release/0.6.0 into prod (squashed instead with "squash").
merge_pr() {
    local r="$C/repo" tree c
    git -C "$r" fetch -q origin
    tree="$(git -C "$r" merge-tree --write-tree origin/prod origin/release/0.6.0)" || return 1
    if [ "${1:-}" = squash ]; then
        c="$(git -C "$r" commit-tree -S -p origin/prod -m "Release 0.6.0 (#7)" "$tree")"
    else
        c="$(git -C "$r" commit-tree -S -p origin/prod -p origin/release/0.6.0 \
            -m "Merge pull request #7 from empathic/release/0.6.0" "$tree")"
    fi
    git -C "$r" push -q origin "$c:refs/heads/prod" || return 1
    echo "MERGED https://github.com/empathic/clax-test/pull/7" > "$FAKE_STATE/pr"
}
# The checkout on prod at origin/prod's tip.
to_prod() {
    git -C "$C/repo" switch -q prod && git -C "$C/repo" pull -q --ff-only origin prod
}
# The preparation a run makes (steps 2-9), made here instead, as the
# cases of the cut need it and a run would only take longer.
prepare_by_hand() {
    local r="$C/repo"
    git -C "$r" switch -q --no-track -c release/0.6.0 origin/prod \
        && git -C "$r" merge -q -S --no-ff -m "Merge origin/main into release/0.6.0" origin/main \
        && bash "$r/scripts/bump-version.sh" 0.6.0 \
        && echo "version = 0.6.0" > "$r/bazel/workspace.bzl" && echo "version = 0.6.0" > "$r/MODULE.bazel.lock" \
        && git -C "$r" commit -q -S -am "Release 0.6.0" \
        && git -C "$r" push -q origin release/0.6.0 \
        && echo "OPEN https://github.com/empathic/clax-test/pull/7" > "$FAKE_STATE/pr"
}
# A case prepared, its pull request merged, and the checkout on prod: the
# next run cuts. The calls so far are forgotten.
ready_to_cut() {
    prepare_by_hand && merge_pr && to_prod && : > "$FAKE_STATE/calls"
}

# The release a full run leaves: a signed tag v0.6.0 "Clax 0.6.0" on prod's
# merge of the pull request; origin's prod a signed pin commit on it, the
# checkout there and clean; and main, with no merge commit since $MAIN0,
# signed copies of the release commit and the pin commit on top, carrying
# the version and the pin.
released() {
    local r="$C/repo" tagc pin mpin rel
    tagc="$(o rev-parse -q --verify 'refs/tags/v0.6.0^{commit}')" || return 1
    pin="$(o rev-parse refs/heads/prod)"
    mpin="$(o rev-parse refs/heads/main)"
    rel="$(o rev-parse "$tagc^2" 2>/dev/null)" || return 1
    [ "$(subject "$tagc")" = "Merge pull request #7 from empathic/release/0.6.0" ] \
        && [ "$(subject "$rel")" = "Release 0.6.0" ] \
        && [ "$(subject "$pin")" = "Pin the plugins to Clax 0.6.0" ] && [ "$(o rev-parse "$pin^")" = "$tagc" ] \
        && [ "$(o for-each-ref --format='%(contents:subject)' refs/tags/v0.6.0)" = "Clax 0.6.0" ] \
        && git -C "$r" verify-tag v0.6.0 2>/dev/null && signed "$tagc" && signed "$pin" \
        && [ "$(subject "$mpin")" = "Pin the plugins to Clax 0.6.0" ] && [ "$(subject "$mpin^")" = "Release 0.6.0" ] \
        && [ "$mpin" != "$pin" ] && signed "$mpin" && signed "$mpin^" \
        && o merge-base --is-ancestor "$MAIN0" "$mpin" && [ -z "$(o rev-list --merges "$MAIN0..$mpin")" ] \
        && [ "$(o show "$mpin:scripts/ensure-clax.sh")" = "$(printf 'CLAX_VERSION="0.6.0"\nPINNED_VERSION="0.6.0"')" ] \
        && [ "$(o show "$mpin:MODULE.bazel.lock")" = "version = 0.6.0" ] \
        && [ "$(git -C "$r" symbolic-ref --short HEAD)" = prod ] && [ "$(git -C "$r" rev-parse HEAD)" = "$pin" ] \
        && [ -z "$(git -C "$r" status --porcelain)" ] \
        && grep -x 'CLAX_VERSION="0.6.0"' "$r/scripts/ensure-clax.sh" >/dev/null \
        && grep -x 'PINNED_VERSION="0.6.0"' "$r/scripts/ensure-clax.sh" >/dev/null \
        && [ "$(cat "$r/MODULE.bazel.lock")" = "version = 0.6.0" ] \
        && [ "$(git -C "$r" worktree list | wc -l | tr -d ' ')" = 1 ]
}
published_from_pin() {
    [ "$(count "gh workflow run publish-pi.yml --ref prod -f dry_run=false -f sha=$(o rev-parse prod)")" = 1 ]
}

# Someone else's commit lands on origin's main.
other_work() {
    local c
    c="$(GIT_AUTHOR_NAME=Other GIT_AUTHOR_EMAIL=other@example.com GIT_COMMITTER_NAME=Other \
        GIT_COMMITTER_EMAIL=other@example.com o commit-tree "$(o rev-parse 'main^{tree}')" \
        -p main -m "Other work")" && o update-ref refs/heads/main "$c"
}
# refuse NAME EXPECTED [ARGS...]: release.sh with ARGS (default: 0.6.0 and
# the act flag) exits 1 naming EXPECTED and changes nothing.
refuse() {
    local name="$1" expected="$2" before rc
    shift 2
    [ $# -gt 0 ] || set -- 0.6.0 --i-know-what-i-am-doing
    : > "$FAKE_STATE/calls"
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
        && has "Dry run: nothing is changed" && has "2. [todo] Cut the release branch: git switch -c release/0.6.0 origin/prod" \
        && has "10. [todo] Merge the pull request (you, on GitHub)" \
        && has "20. [todo] Publish the Pi package" && has "starts at step 2" && has "ok: preparing 0.6.0 from origin/prod (at 0.5.0)"; then
        pass "a dry run checks, prints the plan and changes nothing"
    else fail "a dry run checks, prints the plan and changes nothing (rc=$rc: $(cat "$C/out"))"; fi
}
case_2() {

    # --- a full release, in order ------------------------------------------------------
    new_case
    act; rc=$?
    want="$(printf '%s\n' "bump 0.6.0" "just bazel-sync" "just bazel-sync" "just bazel-sync" "just ci" "gh pr create")"
    got="$(grep -E '^(bump|pin|just|gh pr create|gh workflow run|gh run view)' "$FAKE_STATE/calls" | sed 's/^gh pr create .*/gh pr create/')"
    if [ "$rc" = 0 ] && [ "$got" = "$want" ] && prepared_ok \
        && has "The release is prepared: the pull request https://github.com/empathic/clax-test/pull/7 into prod." \
        && has "  the pull request: https://github.com/empathic/clax-test/pull/7" \
        && ! has "step 10/21" && logs "== step 6 of 21 ran 0s"; then
        pass "the first run cuts the release branch from prod, merges main, bumps, tests, pushes it and opens its pull request"
    else fail "the first run prepares the release (rc=$rc; calls: $got; $(cat "$C/out"))"; fi
    if has "  4. [todo] Bump the version: scripts/bump-version.sh 0.6.0" \
        && has "Estimated: about 40m in all, about 40m for the steps to do (typical durations" \
        && has "[0%] step 2/21 Cut the release branch: git switch -c release/0.6.0 origin/prod" \
        && has "[2%] step 6/21 Run the gates: just ci" && has "[21%] step 9/21 Open the pull request into prod: " \
        && [ "$(grep -c '^\[[0-9]*%\] step [0-9]*/21 ' "$C/out")" = 8 ] \
        && ! has "fake ci: every gate passed" && ! has "fake bazel-sync" && logs "fake ci: every gate passed"; then
        pass "a run shows the steps, then one line per step; the steps' output goes to the log alone"
    else fail "a run's terminal output ($(cat "$C/out"))"; fi

    # Again before the merge, on the release branch or on prod: refused.
    refuse "a release whose pull request is not merged yet" \
        "the pull request https://github.com/empathic/clax-test/pull/7 into prod is not merged yet: once CI passes, merge it on GitHub (Create a merge commit)"
    git -C "$C/repo" switch -q prod
    refuse "a release whose pull request is not merged yet, on prod" "into prod is not merged yet"

    merge_pr
    git -C "$C/repo" pull -q --ff-only origin prod
    : > "$FAKE_STATE/calls"
    # The estimates below are the typical ones: no earlier run's log.
    rm "$C/repo/target/release/0.6.0.log"
    printf 'queued \nin_progress \ncompleted success\n' > "$FAKE_STATE/101"
    act; rc=$?
    pin="$(o rev-parse prod)"
    want="$(printf '%s\n' "gh run view 101" "pin v0.6.0" "just ci" "gh run view 201" \
        "gh workflow run publish-pi.yml --ref prod -f dry_run=false -f sha=$pin" "gh run view 301")"
    # Each run's polls, as one line.
    got="$(grep -E '^(bump|pin|just|gh pr create|gh workflow run|gh run view)' "$FAKE_STATE/calls" \
        | sed 's/^\(gh run view [0-9]*\) .*/\1/' | awk '!($0 ~ /^gh run view/ && $0 == prev) { print } { prev = $0 }')"
    if [ "$rc" = 0 ] && [ "$got" = "$want" ] && released && has "[100%] Clax 0.6.0 is released." \
        && logs "Release v0.6.0: in_progress" && has "ok: cutting 0.6.0 on prod at origin/prod" \
        && has "check that clax.dev deployed: each push to prod deploys it from GitHub Actions" \
        && has "gh run list --workflow deploy-prod.yml --repo empathic/clax-test" && ! has "clax-dev-deploy"; then
        pass "once merged, a run on prod tags, waits, pins, pushes prod, carries both commits to main and publishes, in order"
    else fail "the cut, in order (rc=$rc; calls: $got; $(cat "$C/out"))"; fi
    # Steps 2-10 (530s of 2390s) count in full.
    if has "[22%] step 10/21 Merge the pull request (you, on GitHub): done already" \
        && has "[22%] step 11/21 Tag the release: " && has "[94%] step 20/21 Publish the Pi package: " \
        && [ "$(grep -c '^\[[0-9]*%\] step [0-9]*/21 ' "$C/out")" = 20 ]; then
        pass "the run on prod shows the preparation done and one line per step"
    else fail "the run on prod's terminal output ($(cat "$C/out"))"; fi

    : > "$FAKE_STATE/calls"
    before="$(snapshot)"
    act; rc=$?
    if [ "$rc" = 0 ] && [ -z "$(actions)" ] && [ "$before" = "$(snapshot)" ] && has "step 20/21 Publish the Pi package: done already"; then
        pass "a run after a finished release does nothing again"
    else fail "a run after a finished release does nothing again (rc=$rc: $(actions); $(cat "$C/out"))"; fi
}
case_3() {

    # --- resume after a failure in just ci ---------------------------------------------
    new_case
    : > "$FAKE_STATE/ci-fail"
    act; rc=$?
    if [ "$rc" = 1 ] && has "step 6 (Run the gates) failed" && has "scripts/release.sh 0.6.0 --i-know-what-i-am-doing" \
        && has "The last 50 lines of $C/repo/target/release/0.6.0.log:" && has "fake ci: a gate failed" \
        && has "The log: $C/repo/target/release/0.6.0.log" \
        && grep -x 'CLAX_VERSION="0.6.0"' "$C/repo/scripts/ensure-clax.sh" >/dev/null \
        && [ "$(git -C "$C/repo" log -1 --format=%s)" = "Merge origin/main into release/0.6.0" ] \
        && [ -z "$(o branch --list release/0.6.0)" ]; then
        pass "a failing just ci stops the run before the release commit, naming the step and how to resume"
    else fail "a failing just ci stops the run (rc=$rc: $(cat "$C/out"))"; fi
    release 0.6.0; rc=$?
    if [ "$rc" = 0 ] && has "4. [done] Bump the version" && has "5. [todo] Sync Bazel" && has "starts at step 5"; then
        pass "a dry run after it shows the bump done and the rest to do"
    else fail "a dry run after a failing just ci (rc=$rc: $(cat "$C/out"))"; fi
    rm "$FAKE_STATE/ci-fail"
    act; rc=$?
    if [ "$rc" = 0 ] && prepared_ok && [ "$(count "bump 0.6.0")" = 1 ] && has "step 4/21 Bump the version: done already" \
        && has "step 3/21 Merge main into it: done already"; then
        pass "the next run resumes at the gates and prepares the release, bumping once"
    else fail "resume after a failing just ci (rc=$rc: $(cat "$C/out"))"; fi
}
case_3b() {
    # A refused push of the release branch; the next run, from main, switches
    # back to the branch and pushes it.
    new_case
    : > "$FAKE_STATE/reject-branch"
    act; rc=$?
    if [ "$rc" = 1 ] && has "step 8 (Push the release branch) failed" && [ -z "$(o branch --list release/0.6.0)" ] \
        && [ "$(git -C "$C/repo" log -1 --format=%s)" = "Release 0.6.0" ] && [ "$(count "gh pr create")" = 0 ]; then
        pass "a refused push of the release branch stops at step 8 with the release commit made"
    else fail "a refused push of the release branch (rc=$rc: $(cat "$C/out"))"; fi
    rm "$FAKE_STATE/reject-branch"
    git -C "$C/repo" switch -q main
    act; rc=$?
    if [ "$rc" = 0 ] && prepared_ok && has "step 2/21 Cut the release branch: git switch" \
        && has "step 7/21 Commit the release: done already" && [ "$(count "just ci")" = 1 ]; then
        pass "the next run, from main, switches to the release branch and pushes it"
    else fail "resume at the release branch from main (rc=$rc: $(cat "$C/out"))"; fi
}
case_3c() {
    # GitHub refusing the pull request: the next run opens it.
    new_case
    : > "$FAKE_STATE/pr-create-fail"
    act; rc=$?
    if [ "$rc" = 1 ] && has "step 9 (Open the pull request into prod) failed" && has "GraphQL: Something went wrong (fake)" \
        && [ -n "$(o branch --list release/0.6.0)" ]; then
        pass "a refused pull request stops at step 9 with the branch pushed"
    else fail "a refused pull request (rc=$rc: $(cat "$C/out"))"; fi
    rm "$FAKE_STATE/pr-create-fail"
    act; rc=$?
    if [ "$rc" = 0 ] && prepared_ok && has "step 8/21 Push the release branch: done already"; then
        pass "the next run opens the pull request"
    else fail "resume at the pull request (rc=$rc: $(cat "$C/out"))"; fi
}
case_3d() {
    # A merge of main that conflicts: the run stops saying how to merge by
    # hand; merged so, the next run goes on.
    new_case
    git -C "$C/repo" switch -q prod
    echo prod > "$C/repo/fix.txt"
    git -C "$C/repo" add fix.txt
    git -C "$C/repo" -c commit.gpgsign=false commit -q -m "Fix on prod"
    git -C "$C/repo" push -q origin prod
    git -C "$C/repo" switch -q main
    echo main > "$C/repo/fix.txt"
    git -C "$C/repo" add fix.txt
    git -C "$C/repo" -c commit.gpgsign=false commit -q -m "Fix on main"
    git -C "$C/repo" push -q origin main
    PROD0="$(o rev-parse prod)" MAIN0="$(o rev-parse main)"
    act; rc=$?
    if [ "$rc" = 1 ] && has "step 3 (Merge main into it) failed" && has "conflicts in: fix.txt" \
        && has 'git merge -S origin/main -m "Merge origin/main into release/0.6.0"' \
        && [ "$(git -C "$C/repo" rev-parse HEAD)" = "$PROD0" ] && [ -z "$(git -C "$C/repo" status --porcelain)" ] \
        && [ "$(count "bump 0.6.0")" = 0 ]; then
        pass "a merge of main that conflicts stops at step 3, saying how to merge by hand"
    else fail "a merge of main that conflicts (rc=$rc: $(cat "$C/out"))"; fi
    git -C "$C/repo" merge -q -S origin/main -m "Merge origin/main into release/0.6.0" >/dev/null 2>&1
    echo main > "$C/repo/fix.txt"
    git -C "$C/repo" add fix.txt
    git -C "$C/repo" commit -q -S --no-edit
    act; rc=$?
    if [ "$rc" = 0 ] && prepared_ok && has "step 3/21 Merge main into it: done already"; then
        pass "merged by hand as told, the next run goes on"
    else fail "resume after a merge by hand (rc=$rc: $(cat "$C/out"))"; fi
}
case_4() {

    # --- resume after a failure while waiting for the Release workflow ----------------
    new_case
    ready_to_cut
    echo "completed failure" > "$FAKE_STATE/101"
    act; rc=$?
    if [ "$rc" = 1 ] && has "step 13 (Wait for the Release workflow) failed" && has "concluded failure" \
        && has "gh run rerun 101 --failed" && [ -n "$(o tag -l v0.6.0)" ] \
        && [ "$(subject prod)" = "Merge pull request #7 from empathic/release/0.6.0" ] && [ "$(o rev-parse main)" = "$MAIN0" ]; then
        pass "a failed Release run stops the run after the tag push, with prod and main not pushed"
    else fail "a failed Release run stops the run (rc=$rc: $(cat "$C/out"))"; fi
    echo "in_progress " > "$FAKE_STATE/101"
    act; rc=$?
    if [ "$rc" = 1 ] && has "step 12/21 Push the tag: done already" && has "gave up waiting for run 101 after 3 polls" \
        && has "step 13 (Wait for the Release workflow) failed"; then
        pass "a wait that runs out of polls stops at step 13"
    else fail "a wait that runs out of polls stops at step 13 (rc=$rc: $(cat "$C/out"))"; fi
    echo "completed success" > "$FAKE_STATE/101"
    act; rc=$?
    if [ "$rc" = 0 ] && released && [ "$(count "just ci")" = 1 ] && has "step 11/21 Tag the release: done already"; then
        pass "the next run resumes at the wait and finishes the release, tagging and testing once"
    else fail "resume after a failed wait (rc=$rc: $(grep '^just' "$FAKE_STATE/calls"); $(cat "$C/out"))"; fi
}
case_5() {

    # --- a release of the version from an earlier build ----------------------------------
    # The release exists already, built by an earlier run from a commit with
    # the same tree (its SOURCE_TREE says so), as after re-signing the tagged
    # commit: it is this tag's build, even before the tag's own run is done.
    new_case
    ready_to_cut
    : > "$FAKE_STATE/release"
    echo same > "$FAKE_STATE/earlier-run"
    echo "in_progress " > "$FAKE_STATE/101"
    act; rc=$?
    if [ "$rc" = 0 ] && released && has "step 13/21 Wait for the Release workflow: done already"; then
        pass "a release built from the tag's tree by an earlier run, as after re-signing, is accepted"
    else fail "a release built from the tag's tree by an earlier run (rc=$rc: $(cat "$C/out"))"; fi
}
case_5b() {
    # A release from before SOURCE_TREE: the run that uploaded it, the newest
    # successful one before it, names its tree.
    new_case
    ready_to_cut
    : > "$FAKE_STATE/release"
    : > "$FAKE_STATE/no-tree-asset"
    echo same > "$FAKE_STATE/earlier-run"
    echo "in_progress " > "$FAKE_STATE/101"
    act; rc=$?
    if [ "$rc" = 0 ] && released && has "step 13/21 Wait for the Release workflow: done already"; then
        pass "a release with no SOURCE_TREE is judged by the run that uploaded it"
    else fail "a release with no SOURCE_TREE judged by its run (rc=$rc: $(cat "$C/out"))"; fi
}
case_5c() {
    # A release from before SOURCE_TREE with no successful run before it.
    new_case
    ready_to_cut
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
    ready_to_cut
    : > "$FAKE_STATE/release"
    echo other > "$FAKE_STATE/earlier-run"
    echo "completed failure" > "$FAKE_STATE/101"
    act; rc=$?
    if [ "$rc" = 1 ] && has "step 13 (Wait for the Release workflow) failed" && has "gh release delete v0.6.0 --yes" \
        && has "holds a build of $(o rev-parse 'v0.6.0^^{tree}'), not of this tag's tree" \
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
    ready_to_cut
    : > "$FAKE_STATE/release"
    echo other > "$FAKE_STATE/earlier-run"
    act; rc=$?
    if [ "$rc" = 1 ] && has "not of this tag's tree" && [ "$(count "pin v0.6.0")" = 0 ]; then
        pass "a release built from another tree is never pinned, even when the tag's run succeeds"
    else fail "a release built from another tree, the tag's run succeeding (rc=$rc: $(cat "$C/out"))"; fi
}
case_8() {

    # --- resume after main moves past the carried pin ---------------------------------
    new_case
    ready_to_cut
    echo "completed failure" > "$FAKE_STATE/201"
    act; rc=$?
    pin="$(o rev-parse prod)"
    if [ "$rc" = 1 ] && has "step 19 (Wait for CI on prod) failed" && has "CI on prod (bazel) concluded failure" \
        && [ "$(subject main)" = "Pin the plugins to Clax 0.6.0" ] \
        && [ "$(count "gh workflow run publish-pi.yml --ref prod -f dry_run=false -f sha=$pin")" = 0 ]; then
        pass "failed CI on the pin commit stops at step 19, with main carried and before the publish"
    else fail "failed CI on the pin commit stops at step 19 (rc=$rc: $(cat "$C/out"))"; fi
    other_work
    echo "completed success" > "$FAKE_STATE/201"
    act; rc=$?
    if [ "$rc" = 0 ] && has "step 18/21 Carry the release to main: done already" && published_from_pin \
        && [ "$(subject main)" = "Other work" ]; then
        pass "once main has moved past the carried pin, the next run resumes at the CI wait and publishes from prod's pin"
    else fail "resume after main moved past the carried pin (rc=$rc: $(cat "$C/out"))"; fi
    : > "$FAKE_STATE/calls"
    act; rc=$?
    if [ "$rc" = 0 ] && [ -z "$(actions)" ] && has "step 20/21 Publish the Pi package: done already"; then
        pass "a finished release with main moved on reports every step done"
    else fail "a finished release with main moved on (rc=$rc: $(cat "$C/out"))"; fi
}
case_9() {

    # --- main moving during the release -------------------------------------------------
    new_case
    ready_to_cut
    : > "$FAKE_STATE/push-on-101"
    act; rc=$?
    if [ "$rc" = 0 ] && released && [ "$(subject main~2)" = "Other work" ] && published_from_pin; then
        pass "main moving during the Release wait gets the release carried onto its new tip, with no merge"
    else fail "main moving during the Release wait (rc=$rc: $(cat "$C/out"))"; fi
}
case_10() {
    new_case
    ready_to_cut
    : > "$FAKE_STATE/reject-push"
    act; rc=$?
    if [ "$rc" = 1 ] && has "step 18 (Carry the release to main) failed" && has "the push of main was refused" \
        && [ "$(o rev-parse main)" = "$MAIN0" ] && [ "$(subject prod)" = "Pin the plugins to Clax 0.6.0" ] \
        && [ "$(git -C "$C/repo" worktree list | wc -l | tr -d ' ')" = 1 ] && [ ! -e "$C/repo/target/release/0.6.0/carry" ]; then
        pass "a refused push of main stops at step 18 with prod pushed and no worktree left"
    else fail "a refused push of main stops at step 18 (rc=$rc: $(cat "$C/out"))"; fi
    rm "$FAKE_STATE/reject-push"
    other_work
    act; rc=$?
    if [ "$rc" = 0 ] && released && [ "$(subject main~2)" = "Other work" ] && has "step 17/21 Push prod: done already"; then
        pass "after main moved on, the next run carries the release onto its new tip and finishes"
    else fail "resume after a refused push of main with main moved on (rc=$rc: $(cat "$C/out"))"; fi
}
case_11() {

    # --- a carry that conflicts ------------------------------------------------------------
    # main changes the line the bump changes: the run stops with prod
    # pushed and main untouched, saying how to carry by hand; carried so,
    # the next run goes on.
    new_case
    ready_to_cut
    local w="$C/other"
    git clone -q "$C/origin.git" "$w"
    sed -i.bak 's/^CLAX_VERSION=.*/CLAX_VERSION="0.5.0-dev"/' "$w/scripts/ensure-clax.sh"
    git -C "$w" -c user.name=Other -c user.email=other@example.com -c commit.gpgsign=false commit -q -am "Mark main as dev"
    git -C "$w" push -q origin main
    main1="$(o rev-parse main)"
    act; rc=$?
    rel="$(o rev-parse 'v0.6.0^{commit}^2')"
    pin="$(o rev-parse prod)"
    if [ "$rc" = 1 ] && has "step 18 (Carry the release to main) failed" && has "conflicts in: scripts/ensure-clax.sh" \
        && has "Nothing was pushed to main" && has "git cherry-pick -S -x $rel $pin" \
        && [ "$(o rev-parse main)" = "$main1" ] && [ "$(subject prod)" = "Pin the plugins to Clax 0.6.0" ] \
        && [ "$(git -C "$C/repo" worktree list | wc -l | tr -d ' ')" = 1 ] && [ -z "$(git -C "$C/repo" status --porcelain)" ] \
        && [ "$(count "gh workflow run publish-pi.yml --ref prod -f dry_run=false -f sha=$pin")" = 0 ]; then
        pass "a carry that conflicts stops at step 18 with prod pushed, main untouched and what to resolve"
    else fail "a carry that conflicts (rc=$rc: $(cat "$C/out"))"; fi
    # By hand, as the run said.
    local d="$C/repo/target/release/0.6.0/carry"
    git -C "$C/repo" fetch -q origin main
    git -C "$C/repo" worktree add -q --detach "$d" origin/main
    git -C "$d" cherry-pick -S -x "$rel" "$pin" >/dev/null 2>&1
    printf 'CLAX_VERSION="0.6.0"\nPINNED_VERSION="0.5.0"\n' > "$d/scripts/ensure-clax.sh"
    git -C "$d" add scripts/ensure-clax.sh
    GIT_EDITOR=true git -C "$d" -c commit.gpgsign=true cherry-pick --continue >/dev/null 2>&1
    git -C "$d" push -q origin HEAD:refs/heads/main
    git -C "$C/repo" worktree remove "$d"
    act; rc=$?
    if [ "$rc" = 0 ] && released && has "step 18/21 Carry the release to main: done already" && published_from_pin; then
        pass "carried by hand as told, the next run goes on to CI and the Pi package"
    else fail "resume after a carry by hand (rc=$rc: $(cat "$C/out"))"; fi
}
case_12() {
    new_case
    ready_to_cut
    echo "completed failure" > "$FAKE_STATE/301"
    act; rc=$?
    if [ "$rc" = 1 ] && has "step 20 (Publish the Pi package) failed" && has "Publish Pi package concluded failure"; then
        pass "a failed Publish Pi package run stops at step 20"
    else fail "a failed Publish Pi package run stops at step 20 (rc=$rc: $(cat "$C/out"))"; fi
    act; rc=$?
    if [ "$rc" = 0 ] && [ "$(grep -c '^gh workflow run' "$FAKE_STATE/calls")" = 2 ] && logs "Publish Pi package: run 302"; then
        pass "the next run dispatches the publish again and waits for the new run"
    else fail "resume after a failed publish (rc=$rc: $(cat "$C/out"))"; fi
}
case_13() {

    # --- gh failing during a wait ---------------------------------------------------------
    new_case
    ready_to_cut
    echo 2 > "$FAKE_STATE/gh-fail"
    act; rc=$?
    if [ "$rc" = 0 ] && released; then pass "a wait rides out gh failing twice"
    else fail "a wait rides out gh failing twice (rc=$rc: $(cat "$C/out"))"; fi
}
case_14() {
    new_case
    ready_to_cut
    echo 99 > "$FAKE_STATE/gh-fail"
    CLAX_RELEASE_MAX_POLLS=10 act; rc=$?
    if [ "$rc" = 1 ] && has "gh failed 5 times in a row" && has "HTTP 502: Bad Gateway (fake)" \
        && has "step 13 (Wait for the Release workflow) failed"; then
        pass "gh failing on every poll stops the wait, naming gh's error"
    else fail "gh failing on every poll stops the wait (rc=$rc: $(cat "$C/out"))"; fi
}
case_15() {

    # --- the Pi package on npm ---------------------------------------------------------
    # Not on npm yet: the first version is published from here, from the pin
    # commit on prod, and the trusted-publisher setup is printed.
    new_case
    : > "$FAKE_STATE/npm-absent"
    release 0.6.0; rc=$?
    if [ "$rc" = 0 ] && has "@empathic/clax-pi is not on npm yet: step 20 publishes its first version from this machine"; then
        pass "the dry run says the Pi package's first version is published from here"
    else fail "the dry run with the Pi package not on npm (rc=$rc: $(cat "$C/out"))"; fi
    ready_to_cut
    act; rc=$?
    pin="$(o rev-parse prod)"
    if [ "$rc" = 0 ] && released && [ "$(count "npm publish at $pin")" = 1 ] && [ "$(count "npm publish --access public")" = 1 ] \
        && [ "$(count "pnpm install --frozen-lockfile (in plugins/pi)")" = 1 ] && [ "$(count "pnpm run typecheck (in plugins/pi)")" = 1 ] \
        && [ "$(grep -c '^gh workflow run' "$FAKE_STATE/calls")" = 0 ] \
        && has "GitHub Actions: organization empathic, repository clax-test, workflow" \
        && has "publish-pi.yml, environment npm." \
        && has "switch README.md and plugins/pi/README.md to lead their Pi install with" \
        && [ ! -e "$C/repo/target/release/0.6.0/pi-publish" ]; then
        pass "a Pi package not on npm is published from here, from the pin commit, with the trusted-publisher setup"
    else fail "the first Pi publish (rc=$rc: $(actions); $(cat "$C/out"))"; fi
}
case_15b() {
    # The npm environment missing or open to more than prod: a dry run warns
    # with the steps to make it, a release refuses before anything.
    new_case
    : > "$FAKE_STATE/no-env"
    release 0.6.0; rc=$?
    if [ "$rc" = 0 ] && has "note: WARN: the repository has no environment npm" \
        && has "Settings, Environments, New environment (or npm), name npm; Deployment branches and tags: Selected branches and tags; Add deployment branch or tag rule: branch prod"; then
        pass "a dry run warns that the npm environment is missing, with the steps to make it"
    else fail "a dry run with no npm environment (rc=$rc: $(cat "$C/out"))"; fi
    refuse "a release with no npm environment" "FAIL: the repository has no environment npm"
    rm "$FAKE_STATE/no-env"
    : > "$FAKE_STATE/env-open"
    refuse "an npm environment open to every branch" "the environment npm does not limit its deployment branches to prod"
    rm "$FAKE_STATE/env-open"
    : > "$FAKE_STATE/env-branch"
    refuse "an npm environment for main, as before releases came from prod" "the environment npm allows branch:main, not prod alone"
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
    # step 20 done and still prints the trusted-publisher setup.
    new_case
    : > "$FAKE_STATE/npm-absent"
    ready_to_cut
    : > "$FAKE_STATE/npm-hide"
    act; rc=$?
    if [ "$rc" = 1 ] && has "npm does not list @empathic/clax-pi@0.6.0 after the publish" \
        && [ -e "$C/repo/target/release/0.6.0/pi-first-publish" ]; then
        pass "a first publish npm has not listed yet stops step 20, keeping the publish recorded"
    else fail "a first publish npm has not listed yet (rc=$rc: $(cat "$C/out"))"; fi
    rm "$FAKE_STATE/npm-absent" "$FAKE_STATE/npm-hide"
    mv "$FAKE_STATE/npm-hidden" "$FAKE_STATE/npm-versions"
    act; rc=$?
    if [ "$rc" = 0 ] && has "step 20/21 Publish the Pi package: done already" \
        && has "once: set up npm trusted publishing for @empathic/clax-pi" \
        && [ "$(count "npm publish --access public")" = 1 ]; then
        pass "the rerun finds the publish done and prints the trusted-publisher setup"
    else fail "the rerun after a first publish npm listed late (rc=$rc: $(cat "$C/out"))"; fi
}
case_16c() {
    # A pin commit amended by hand unsigned: step 17 refuses to push prod,
    # saying how to sign it; signed, the next run pushes it and finishes.
    new_case
    ready_to_cut
    : > "$FAKE_STATE/reject-prod"
    act; rc=$?
    if [ "$rc" = 1 ] && has "step 17 (Push prod) failed" && has "the push of prod was refused" \
        && [ "$(subject prod)" = "Merge pull request #7 from empathic/release/0.6.0" ] && [ "$(o rev-parse main)" = "$MAIN0" ]; then
        pass "a refused push of prod stops at step 17, before the carry"
    else fail "a refused push of prod (rc=$rc: $(cat "$C/out"))"; fi
    rm "$FAKE_STATE/reject-prod"
    git -C "$C/repo" -c commit.gpgsign=false commit -q --amend --no-edit
    act; rc=$?
    if [ "$rc" = 1 ] && has "carries no signature; not pushing it." \
        && has "Sign it (git commit --amend -S --no-edit), then run this again." \
        && [ "$(subject prod)" = "Merge pull request #7 from empathic/release/0.6.0" ]; then
        pass "an unsigned pin commit is not pushed, and the refusal says how to sign it"
    else fail "an unsigned pin commit (rc=$rc: $(cat "$C/out"))"; fi
    git -C "$C/repo" commit -q --amend -S --no-edit
    act; rc=$?
    if [ "$rc" = 0 ] && released && published_from_pin; then
        pass "once signed as told, the next run pushes prod and finishes"
    else fail "the pin commit once signed (rc=$rc: $(cat "$C/out"))"; fi
}
case_16b() {
    # On npm: the workflow publishes, never npm here.
    new_case
    ready_to_cut
    act; rc=$?
    if [ "$rc" = 0 ] && released && [ "$(count "npm publish --access public")" = 0 ] \
        && [ "$(grep -c '^gh workflow run publish-pi.yml --ref prod' "$FAKE_STATE/calls")" = 1 ] && ! has "Trusted Publisher"; then
        pass "a Pi package on npm is published through publish-pi.yml, on prod"
    else fail "a Pi package on npm is published through publish-pi.yml (rc=$rc: $(cat "$C/out"))"; fi
}
case_17() {
    # The version on npm already: step 20 is done.
    new_case
    echo 0.6.0 > "$FAKE_STATE/npm-versions"
    ready_to_cut
    act; rc=$?
    if [ "$rc" = 0 ] && has "note: @empathic/clax-pi@0.6.0 is on npm already: step 20 is done" \
        && has "step 20/21 Publish the Pi package: done already" && ! grep -E '^(gh workflow run|npm publish)' "$FAKE_STATE/calls" >/dev/null; then
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
    # Cutting: the checkout behind origin/prod, or on another branch.
    new_case
    prepare_by_hand
    merge_pr
    git -C "$C/repo" switch -q prod
    refuse "a prod behind origin/prod" "prod is 4 commit(s) behind origin/prod (git pull --ff-only)"
    git -C "$C/repo" switch -q release/0.6.0
    refuse "a cut from the release branch" "the checkout is on release/0.6.0, not prod: origin/prod is at 0.6.0, so the release is cut from prod"
    git -C "$C/repo" switch -q main
    refuse "a cut from main" "the checkout is on main, not prod"
}
case_21() {
    # Preparing: a commit on the release branch that is not the release's.
    new_case
    git -C "$C/repo" switch -q -c release/0.6.0 origin/prod
    git -C "$C/repo" -c commit.gpgsign=false commit -q --allow-empty -m "Unpushed"
    refuse "a commit on the release branch that is not the release's" 'release/0.6.0 has a commit that is not this release'"'"'s: "Unpushed"'
}
case_22() {
    # Preparing: origin/prod moved on since the release branch was cut.
    new_case
    git -C "$C/repo" switch -q -c release/0.6.0 origin/prod
    git -C "$C/repo" switch -q prod
    git -C "$C/repo" -c commit.gpgsign=false commit -q --allow-empty -m "Elsewhere"
    git -C "$C/repo" push -q origin prod
    git -C "$C/repo" switch -q release/0.6.0
    refuse "a release branch without origin/prod's tip" "release/0.6.0 does not hold origin/prod's tip: origin/prod has moved on"
}
case_22b() {
    # Cutting: a pull request squashed, so prod has no release commit to carry.
    new_case
    prepare_by_hand
    merge_pr squash
    to_prod
    refuse "a squashed pull request" "origin/prod has no \"Release 0.6.0\" commit that origin/main lacks: merge the release's pull request with a merge commit"
}
case_22c() {
    # No prod on origin.
    new_case
    o update-ref -d refs/heads/prod
    git -C "$C/repo" update-ref -d refs/remotes/origin/prod
    refuse "an origin with no prod branch" "cannot fetch prod from origin: releases come from a prod branch"
}
case_23() {
    new_case
    refuse "an older version" "0.4.9 is not newer than the tag v0.5.0" 0.4.9 --i-know-what-i-am-doing
    refuse "a version that is not X.Y.Z" "0.6 is not a release version (X.Y.Z)" 0.6 --i-know-what-i-am-doing
    refuse "a bad version in a dry run too" "0.6 is not a release version (X.Y.Z)" 0.6
    refuse "prod's version, released from main" "origin/prod has no \"Release 0.5.0\" commit that origin/main lacks" 0.5.0 --i-know-what-i-am-doing
    o tag v0.7.0 main
    refuse "a version below a tag only origin has" "0.6.0 is not newer than the tag v0.7.0"
}
case_24() {
    new_case
    git -C "$C/repo" commit -q -S --allow-empty -m "Release 0.6.0"
    git -C "$C/repo" -c commit.gpgsign=false commit -q --allow-empty -m 'Revert "Release 0.6.0"'
    git -C "$C/repo" push -q origin main
    refuse "an old \"Release 0.6.0\" commit on origin/main" "origin/main has a \"Release 0.6.0\" commit already"
}
case_25() {
    new_case
    git -C "$C/repo" -c tag.gpgsign=false tag -a v0.6.0 -m "Clax 0.6.0"
    refuse "a tag of the version here before prod has it" "the tag v0.6.0 exists ($(git -C "$C/repo" rev-parse HEAD)), but origin/prod is at 0.5.0, not 0.6.0"
}
case_26() {
    new_case
    o tag v0.6.0 main
    refuse "a tag of the version on origin before prod has it" "the tag v0.6.0 exists ($(git -C "$C/repo" rev-parse HEAD)), but origin/prod is at 0.5.0, not 0.6.0"
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
    refuse "a first Pi publish with npm signed out" "is not on npm yet, so step 20 publishes its first version from here: sign in first (npm login; npm whoami fails)"
}
case_29() {
    new_case
    git -C "$C/repo" config user.signingkey "$T/no-such-key"
    refuse "a signing key that does not sign" "commit signing does not work here"
}
case_30() {
    new_case
    git -C "$C/repo" switch -q prod
    sed -i.bak 's/^CLAX_VERSION="0.5.0"/CLAX_VERSION="0.7.0"/' "$C/repo/scripts/ensure-clax.sh"
    git -C "$C/repo" -c commit.gpgsign=false commit -q -am "Bump to 0.7.0"
    git -C "$C/repo" push -q origin prod
    git -C "$C/repo" switch -q main
    refuse "a prod at a newer version" "origin/prod is at 0.7.0, newer than 0.6.0"
}
case_31() {
    new_case
    git -C "$C/repo" switch -q -c release/0.6.0 origin/prod
    sed -i.bak 's/^CLAX_VERSION=/CLAX_VERSION_GONE=/' "$C/repo/scripts/ensure-clax.sh"
    rm "$C/repo/scripts/ensure-clax.sh.bak"
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
    ready_to_cut
    echo 2 > "$FAKE_STATE/release-delay"
    act; rc=$?
    if [ "$rc" = 0 ] && released && [ "$(grep -c '^gh run list --workflow release.yml' "$FAKE_STATE/calls")" -ge 3 ]; then
        pass "the wait polls until the Release run is listed"
    else fail "the wait polls until the Release run is listed (rc=$rc: $(cat "$C/out"))"; fi
}
case_35() {
    new_case
    ready_to_cut
    echo 99 > "$FAKE_STATE/release-delay"
    act; rc=$?
    if [ "$rc" = 1 ] && has "nothing after 3 polls" && has "no Release workflow run on v0.6.0 appeared" \
        && has "step 13 (Wait for the Release workflow) failed"; then
        pass "a Release run that is never listed stops the wait after its polls"
    else fail "a Release run that is never listed (rc=$rc: $(cat "$C/out"))"; fi
}
case_36() {

    # --- bazel-sync that never settles --------------------------------------------------
    new_case
    : > "$FAKE_STATE/sync-forever"
    act; rc=$?
    if [ "$rc" = 1 ] && [ "$(count "just bazel-sync")" = 4 ] && has "just bazel-sync still changed files on its 4th run" \
        && has "step 5 (Sync Bazel with Cargo) failed" && [ "$(count "just ci")" = 0 ]; then
        pass "bazel-sync still changing files after 4 runs stops at step 5"
    else fail "bazel-sync that never settles (rc=$rc: $(cat "$C/out"))"; fi
}
case_37() {

    new_case
    before="$(snapshot)"
    release 0.6.0 --force; rc=$?
    if [ "$rc" = 2 ] && [ "$before" = "$(snapshot)" ]; then pass "an unknown flag is a usage error"; else fail "an unknown flag is a usage error (rc=$rc)"; fi
}
case_38() {

    # --- progress --------------------------------------------------------------------
    # Steps a run finds done count in full: with typical durations (no
    # earlier log), steps 1-12 are 545s of 2390s.
    new_case
    ready_to_cut
    echo "completed failure" > "$FAKE_STATE/101"
    act
    rm "$C/repo/target/release/0.6.0.log"
    echo "completed success" > "$FAKE_STATE/101"
    act; rc=$?
    if [ "$rc" = 0 ] && released && has "  12. [done] Push the tag" && has "  13. [todo] Wait for the Release workflow" \
        && has "Estimated: about 40m in all, about 31m for the steps to do (typical durations" \
        && has "[0%] step 2/21 Cut the release branch: done already" \
        && has "[22%] step 12/21 Push the tag: done already" \
        && has "[22%] step 13/21 Wait for the Release workflow: gh run list" \
        && has "Starting at step 13."; then
        pass "a resumed run counts the steps done in its percentage"
    else fail "a resumed run's percentage (rc=$rc: $(cat "$C/out"))"; fi
}
case_39() {
    # The durations a run records in its log make the next estimate: step
    # 15 takes 300s, the rest that ran none (counted as 1s); 2-10, found
    # done, keep their typical durations, and so does 19 (CI on the fake is
    # done at once): 1370s.
    new_case
    echo 300 > "$FAKE_STATE/ci-advance"
    ready_to_cut
    act; rc=$?
    if [ "$rc" = 0 ] && logs "== step 15 of 21 ran 300s" && logs "== step 13 of 21 ran 0s" \
        && has "Clax 0.6.0 is released. This run took 5m00s."; then
        pass "a run records each step's duration in its log"
    else fail "a run records each step's duration (rc=$rc: $(cat "$C/out"))"; fi
    release 0.6.0; rc=$?
    if [ "$rc" = 0 ] && has "Estimated: about 23m in all, under a minute for the steps to do (the steps' durations in 1 earlier run log(s)"; then
        pass "the estimate comes from earlier runs' logs"
    else fail "the estimate from earlier runs' logs (rc=$rc: $(cat "$C/out"))"; fi
    # A log of the earlier numbering is not read.
    printf '== step 4 ran 9999s\n' > "$C/repo/target/release/0.5.0.log"
    release 0.6.0; rc=$?
    if [ "$rc" = 0 ] && has "Estimated: about 23m in all"; then
        pass "a log of the earlier step numbering does not count"
    else fail "a log of the earlier step numbering (rc=$rc: $(cat "$C/out"))"; fi
}
case_40() {
    # A heartbeat while just ci runs, its clock moved on by 3m12s: Bazel's
    # tests from the log, the share done ((55 + 192) / 2390) and the time left.
    new_case
    echo 192 > "$FAKE_STATE/ci-advance"
    : > "$FAKE_STATE/ci-heartbeat"
    CLAX_RELEASE_HEARTBEAT_SECONDS=0.05 act; rc=$?
    if [ "$rc" = 0 ] && prepared_ok && has "[10%] step 6/21 Run the gates: 40/56 tests, 3m12s (about 36m left)"; then
        pass "a heartbeat tells the share done, Bazel's tests, the time taken and the time left"
    else fail "a heartbeat (rc=$rc: $(cat "$C/out"))"; fi
}
case_41() {
    # --verbose streams the steps' output too, still logging it.
    new_case
    release 0.6.0 --i-know-what-i-am-doing --verbose; rc=$?
    if [ "$rc" = 0 ] && prepared_ok && has "fake ci: every gate passed" \
        && has "[2%] step 6/21 Run the gates: just ci" && logs "fake ci: every gate passed"; then
        pass "--verbose streams the steps' output and logs it"
    else fail "--verbose (rc=$rc: $(cat "$C/out"))"; fi
    merge_pr && to_prod
    release 0.6.0 --i-know-what-i-am-doing --verbose; rc=$?
    if [ "$rc" = 0 ] && released && has "Release v0.6.0: completed success" && has "fake ci: every gate passed"; then
        pass "--verbose streams the cut's output too"
    else fail "--verbose on the cut (rc=$rc: $(cat "$C/out"))"; fi
}

JOBS="$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 4)"
[ "$JOBS" -le 8 ] || JOBS=8
CASES="case_1 case_2 case_3 case_3b case_3c case_3d case_4 case_5 case_5b case_5c case_6 case_7 case_8 case_9 case_10 case_11 case_12 case_13 case_14 case_15 case_15b case_16 case_16b case_16c case_17 case_18 case_19 case_20 case_21 case_22 case_22b case_22c case_23 case_24 case_25 case_26 case_27 case_28 case_29 case_30 case_31 case_32 case_33 case_34 case_35 case_36 case_37 case_38 case_39 case_40 case_41"
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
