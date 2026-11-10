#!/usr/bin/env bash
# Tests scripts/release.sh in scratch git repositories, each with a bare
# origin holding main and prod, with fakes of gh, just and bazelisk on PATH
# and an SSH key of the test's own for signing (gpg.format=ssh in each
# scratch repository's own config). No network, no GitHub; git reads no
# global or system config.
#
# A scratch repository holds a copy of release.sh, of the helpers it
# sources (scripts/lib/release-common.sh), and stand-ins for the scripts it
# runs (bump-version.sh and check-version.sh write and read CLAX_VERSION in
# scripts/ensure-clax.sh; their own tests are scripts/test-release.sh). The
# fakes log every call to $FAKE_STATE/calls. The fake gh plays GitHub from
# the bare origin: a pull request ($FAKE_STATE/pr, "STATE URL") once one is
# created, which the test's merge_pr merges as GitHub would, with a signed
# merge commit on prod; and, while $FAKE_STATE/deploy-run exists, a Deploy
# prod run (https://github.com/<repo>/actions/runs/401) on origin's prod
# tip, the one `gh run list --workflow deploy-prod.yml` lists. The fake
# prints what each --jq filter would. Origin refuses merge commits on main,
# as GitHub does (GH013).
# A run works on the release branch in a worktree of its own, $WT
# (target/release/worktree-0.6.0 in the scratch repository), and removes it
# once the branch is pushed: the scratch checkout, $C/repo, stays on main,
# untouched, unless the branch is checked out there already.
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
export GH_REPO=empathic/clax-test
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
    *) echo "fake just: unexpected: $*" >&2; exit 2 ;;
esac
SH
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
case "$1 $2" in
    "auth status")
        if [ -e "$S/no-auth" ]; then echo "You are not logged into any GitHub hosts." >&2; exit 1; fi ;;
    "pr list")
        # By --head, "STATE URL"; else the open release pull requests'
        # "BRANCH URL".
        if [ -n "$(flag --head "$@")" ]; then cat "$S/pr" 2>/dev/null || true
        elif read -r state url < "$S/pr" 2>/dev/null && [ "$state" = OPEN ]; then echo "release/0.6.0 $url"; fi ;;
    "pr create")
        # $FAKE_STATE/pr-create-fail: GitHub refuses it.
        if [ -e "$S/pr-create-fail" ]; then echo "GraphQL: Something went wrong (fake)" >&2; exit 1; fi
        url="https://github.com/$GH_REPO/pull/7"
        echo "OPEN $url" > "$S/pr"
        echo "$url" ;;
    "run list")
        case "$(flag --workflow "$@")" in
            deploy-prod.yml)
                # The --jq filter's answer: the run's URL, or "" with none.
                if [ -e "$S/deploy-run" ] && [ "$(flag --commit "$@")" = "$(origin rev-parse refs/heads/prod)" ]; then
                    echo "https://github.com/$GH_REPO/actions/runs/401"
                else
                    echo ""
                fi ;;
            *) echo "fake gh: unexpected: $*" >&2; exit 2 ;;
        esac ;;
    *) echo "fake gh: unexpected: $*" >&2; exit 2 ;;
esac
SH
FAKE_PATH="$T/bin:$PATH"
# origin's pre-receive hook: refuses a merge commit on main, as GitHub's
# rules on main do (GH013), and a push of a release branch while
# $FAKE_STATE/reject-branch exists.
fake_exe "$T/pre-receive" <<'SH'
#!/bin/sh
while read -r old new ref; do
    case "$ref" in
        refs/heads/main)
            range="$old..$new"
            case "$old" in *[!0]*) ;; *) range="$new" ;; esac
            if [ -n "$(git rev-list --merges "$range")" ]; then
                echo "GH013: Repository rule violations: this branch must not contain merge commits (fake)" >&2
                exit 1
            fi ;;
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
    mkdir -p "$C/state" "$C/repo/scripts/lib" "$C/repo/bazel" "$C/repo/plugins/pi/scripts"
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
    cp "$HERE/scripts/lib/release-common.sh" "$r/scripts/lib/release-common.sh"
    printf 'target/\n' > "$r/.gitignore"
    printf 'CLAX_VERSION="0.5.0"\nPINNED_VERSION="0.5.0"\n' > "$r/scripts/ensure-clax.sh"
    printf 'CLAX_VERSION="0.5.0"\nPINNED_VERSION="0.5.0"\n' > "$r/plugins/pi/scripts/ensure-clax.sh"
    printf '{\n  "name": "@empathic/clax-pi",\n  "version": "0.5.0",\n  "type": "module"\n}\n' > "$r/plugins/pi/package.json"
    echo "version = 0.5.0" > "$r/bazel/workspace.bzl"
    echo "version = 0.5.0" > "$r/MODULE.bazel.lock"
    cat > "$r/scripts/check-version.sh" <<'SH'
#!/usr/bin/env bash
cd "$(dirname "$0")/.." || exit 1
if [ "${1:-}" != --print ]; then
    echo "check-version" >> "$FAKE_STATE/calls"
    # $FAKE_STATE/check-advance: seconds each run moves the clock on.
    if [ -s "$FAKE_STATE/check-advance" ]; then
        echo $(( $(cat "$CLAX_RELEASE_NOW_FILE") + $(cat "$FAKE_STATE/check-advance") )) > "$CLAX_RELEASE_NOW_FILE"
    fi
    # $FAKE_STATE/check-heartbeat: the next run waits, at most a minute, for
    # a heartbeat of its step in release.sh's output ($FAKE_OUT): a line
    # with the text the file holds.
    if [ -e "$FAKE_STATE/check-heartbeat" ]; then
        want="$(cat "$FAKE_STATE/check-heartbeat")"
        rm "$FAKE_STATE/check-heartbeat"
        i=0
        while [ "$i" -lt 1200 ] && ! grep -F "$want" "$FAKE_OUT" >/dev/null 2>&1; do
            sleep 0.05
            i=$((i + 1))
        done
    fi
    if [ -e "$FAKE_STATE/check-fail" ]; then echo "fake check-version: plugins/x/VERSION is 0.5.0, not 0.6.0" >&2; exit 1; fi
    echo "fake check-version: the versions agree"
fi
v="$(sed -n 's/^CLAX_VERSION="\(.*\)"$/\1/p' scripts/ensure-clax.sh)"
[ -n "$v" ] || { echo "versions differ" >&2; exit 1; }
bad=0
for f in plugins/*/VERSION; do
    [ -e "$f" ] || continue
    if [ "$(cat "$f")" != "$v" ]; then echo "$f: $(cat "$f"), not $v" >&2; bad=1; fi
done
[ "$bad" = 0 ] || exit 1
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
for f in plugins/*/VERSION; do [ ! -e "$f" ] || echo "$1" > "$f"; done
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
    WT="$r/target/release/worktree-0.6.0"
}

# Runs release.sh from another directory; its output is in $C/out.
release() {
    (cd "$T" && PATH="${CASE_PATH:-$FAKE_PATH}" "$TEST_BASH" "$C/repo/scripts/release.sh" "$@") > "$C/out" 2>&1
}
act() { release 0.6.0 --i-know-what-i-am-doing; }
# Everything a run could change: the work tree, ignored files included, the
# worktrees and the release worktree's files, and the refs here and on
# origin.
snapshot() {
    git -C "$C/repo" status --porcelain=v1 -uall --ignored
    git -C "$C/repo" worktree list --porcelain
    if [ -e "$WT" ]; then git -C "$WT" status --porcelain=v1 -uall --ignored; git -C "$WT" diff; fi
    git -C "$C/repo" for-each-ref --format='%(refname) %(objectname)'
    git -C "$C/origin.git" for-each-ref --format='%(refname) %(objectname)'
    cat "$C/repo/scripts/ensure-clax.sh" "$C/repo/bazel/workspace.bzl" "$C/repo/MODULE.bazel.lock" "$FAKE_STATE/pr" 2>/dev/null
}
# The calls that change something.
actions() { grep -E '^(bump|just|gh pr create|gh pr merge|gh release|gh workflow run)' "$FAKE_STATE/calls" 2>/dev/null; }
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
# The preparation a run leaves on origin: release/0.6.0 a signed "Release
# 0.6.0" commit on a signed merge of main into prod's tip, the pull request
# open, and prod and main as they were.
origin_ok() {
    local rel
    rel="$(o rev-parse -q --verify refs/heads/release/0.6.0)" || return 1
    [ "$(subject "$rel")" = "Release 0.6.0" ] \
        && [ "$(subject "$rel^")" = "Merge origin/main into release/0.6.0" ] \
        && [ "$(o rev-parse "$rel^^1")" = "$PROD0" ] && [ "$(o rev-parse "$rel^^2")" = "$MAIN0" ] \
        && signed "$rel" && signed "$rel^" \
        && [ "$(o show "$rel:scripts/ensure-clax.sh")" = "$(printf 'CLAX_VERSION="0.6.0"\nPINNED_VERSION="0.5.0"')" ] \
        && [ "$(o show "$rel:MODULE.bazel.lock")" = "version = 0.6.0" ] \
        && [ "$(cat "$FAKE_STATE/pr")" = "OPEN https://github.com/empathic/clax-test/pull/7" ] \
        && grep -x -F "gh pr create --base prod --head release/0.6.0 --title Release 0.6.0 --body $PR_BODY_LINE" "$FAKE_STATE/calls" >/dev/null \
        && [ "$(o rev-parse prod)" = "$PROD0" ] && [ "$(o rev-parse main)" = "$MAIN0" ] && [ -z "$(o tag -l v0.6.0)" ]
}
# origin_ok, and here: the checkout still on main at $MAIN0, its status
# what it was ($MAIN_STATUS, clean by default); the release worktree gone,
# from disk and from git's list, with the local release/0.6.0.
prepared_ok() {
    origin_ok \
        && [ "$(git -C "$C/repo" symbolic-ref --short HEAD)" = main ] \
        && [ "$(git -C "$C/repo" rev-parse HEAD)" = "$MAIN0" ] \
        && [ "$(git -C "$C/repo" status --porcelain)" = "${MAIN_STATUS:-}" ] \
        && [ ! -e "$WT" ] && ! git -C "$C/repo" worktree list --porcelain | grep -x -F "worktree $WT" >/dev/null \
        && ! git -C "$C/repo" rev-parse -q --verify refs/heads/release/0.6.0 >/dev/null
}
# The release worktree, made by hand as step 2 makes it.
add_worktree() { git -C "$C/repo" worktree add -q --no-track -b release/0.6.0 "$WT" origin/prod; }

# GitHub merges the pull request: a signed merge commit of origin's
# release/0.6.0 into prod.
merge_pr() {
    local r="$C/repo" tree c
    git -C "$r" fetch -q origin
    tree="$(git -C "$r" merge-tree --write-tree origin/prod origin/release/0.6.0)" || return 1
    c="$(git -C "$r" commit-tree -S -p origin/prod -p origin/release/0.6.0 \
        -m "Merge pull request #7 from empathic/release/0.6.0" "$tree")"
    git -C "$r" push -q origin "$c:refs/heads/prod" || return 1
    echo "MERGED https://github.com/empathic/clax-test/pull/7" > "$FAKE_STATE/pr"
}
# The preparation a run makes (steps 2-9), made here instead, as the
# cases after it need it and a run would only take longer.
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
# released_ok NAME [ARGS...]: once origin/prod is at 0.6.0, release.sh with
# ARGS (default: 0.6.0 and the act flag) says the merge releases it in
# Deploy prod, names the run (or $RUN_LINE), exits 0 and changes nothing.
RUN_LINE="The run: https://github.com/empathic/clax-test/actions/runs/401"
released_ok() {
    local name="$1" before rc tip
    shift
    [ $# -gt 0 ] || set -- 0.6.0 --i-know-what-i-am-doing
    : > "$FAKE_STATE/calls"
    before="$(snapshot)"
    tip="$(o rev-parse prod)"
    release "$@"; rc=$?
    if [ "$rc" = 0 ] && [ -z "$(actions)" ] && [ "$before" = "$(snapshot)" ] \
        && has "origin/prod is at 0.6.0: its merge releases it, in GitHub Actions (Deploy prod), with nothing to" \
        && has "$RUN_LINE" \
        && [ "$(grep -c -x -F -- "gh run list --workflow deploy-prod.yml --commit $tip --limit 1 --json url --jq .[0].url // \"\"" "$FAKE_STATE/calls")" = 1 ] \
        && ! has "FAIL" && ! has "step 2/10" && [ ! -e "$C/repo/target/release/0.6.0.log" ]; then
        pass "$name"
    else fail "$name (rc=$rc: $(actions); $(cat "$C/out"))"; fi
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
        && has "Dry run: nothing is changed" \
        && has "  Here, in a worktree of its own, target/release/worktree-0.6.0; the checkout you run this from is left as it is:" \
        && has "2. [todo] Make the release branch, in a worktree of its own: git worktree add -b release/0.6.0 target/release/worktree-0.6.0 origin/prod" \
        && has "8. [todo] Push the release branch, then remove the worktree: git push origin release/0.6.0; git worktree remove target/release/worktree-0.6.0; git branch -D release/0.6.0" \
        && has "  On GitHub:" \
        && has "9. [todo] Open the pull request into prod: gh pr create --base prod --head release/0.6.0" \
        && has "10. [todo] Merge the pull request (you, on GitHub): once CI and staging pass, merge it (Create a merge commit)" \
        && has "  Then Deploy prod runs on the merge, each job once those it follows are done:" \
        && has "candidate, reel     at once" \
        && ! has "11. " && has "starts at step 2" && has "ok: preparing 0.6.0 from origin/prod (at 0.5.0)" && ! has ", clean,"; then
        pass "a dry run checks, prints the plan and changes nothing"
    else fail "a dry run checks, prints the plan and changes nothing (rc=$rc: $(cat "$C/out"))"; fi
}
case_2() {

    # --- the preparation, in order -----------------------------------------------------
    new_case
    act; rc=$?
    want="$(printf '%s\n' "bump 0.6.0" "just bazel-sync" "just bazel-sync" "just bazel-sync" "check-version" "gh pr create")"
    got="$(grep -E '^(bump|just|check-version|gh pr create|gh run)' "$FAKE_STATE/calls" | sed 's/^gh pr create .*/gh pr create/')"
    if [ "$rc" = 0 ] && [ "$got" = "$want" ] && prepared_ok \
        && has "The release is prepared: the pull request https://github.com/empathic/clax-test/pull/7 into prod, which is staging:" \
        && has "and approve Deploy prod's deploy: it deploys clax.dev from the candidate staging tested," \
        && has "  the pull request: https://github.com/empathic/clax-test/pull/7" \
        && logs "  release/0.6.0 is checked out in $WT" && logs "  removed $WT and the local release/0.6.0" \
        && ! has "step 10/10" && logs "== step 6 of 10 ran 0s" && logs "== step 9 of 10 ran 0s"; then
        pass "the first run makes the release branch from prod in a worktree, merges main, bumps, checks the versions, pushes it, removes the worktree and opens its pull request, the checkout left on main"
    else fail "the first run prepares the release (rc=$rc; calls: $got; $(cat "$C/out"))"; fi
    # Typical durations: 655s in all, 645s for steps 2-10; step N starts at
    # the share of steps 1 to N-1 (10s of 655s for step 2, 625s for 6, 645s
    # for 9).
    if has "  4. [todo] Bump the version: scripts/bump-version.sh 0.6.0" \
        && has "Estimated: about 11m in all, about 11m for the steps to do (typical durations; later runs learn from their logs; the review and Deploy prod not counted)." \
        && has "[1%] step 2/10 Make the release branch, in a worktree of its own: git worktree add -b release/0.6.0 target/release/worktree-0.6.0 origin/prod" \
        && has "[95%] step 6/10 Check the version files: scripts/check-version.sh" && has "[98%] step 9/10 Open the pull request into prod: " \
        && [ "$(grep -c '^\[[0-9]*%\] step [0-9]*/10 ' "$C/out")" = 8 ] \
        && ! has "fake check-version: the versions agree" && ! has "fake bazel-sync" && logs "fake check-version: the versions agree"; then
        pass "a run shows the steps, then one line per step; the steps' output goes to the log alone"
    else fail "a run's terminal output ($(cat "$C/out"))"; fi
    # The pull request's body: the release's checklist.
    if grep -x -F -- "- [ ] CI passes on it." "$FAKE_STATE/calls" >/dev/null \
        && grep -F -- "- [ ] Staging (\`staging.yml\`) deployed this pull request's own instance" "$FAKE_STATE/calls" >/dev/null \
        && grep -F -- "- [ ] Merge with a merge commit (not squash or rebase). Deploy prod then waits for the owner's approval" "$FAKE_STATE/calls" >/dev/null \
        && grep -F -- "publishes v0.6.0, carries the release and the plugins' pin to \`main\` and publishes the Pi package." "$FAKE_STATE/calls" >/dev/null; then
        pass "the pull request's body is the release's checklist: CI, staging, a merge commit, then Deploy prod"
    else fail "the pull request's body ($(sed -n '/^gh pr create/,$p' "$FAKE_STATE/calls"))"; fi

    # Again before the merge, on main, the release branch or prod: the run
    # says where the pull request is, and changes nothing.
    for at in main release/0.6.0 prod; do
        git -C "$C/repo" switch -q "$at"
        : > "$FAKE_STATE/calls"
        before="$(snapshot)"
        act; rc=$?
        if [ "$rc" = 0 ] && [ -z "$(actions)" ] && [ "$before" = "$(snapshot)" ] \
            && has "The release is prepared: the pull request https://github.com/empathic/clax-test/pull/7 into prod, which is staging:" \
            && ! has "FAIL" && ! has "step 2/10"; then
            pass "a run with its pull request open, on $at, names the pull request and changes nothing"
        else fail "a run with its pull request open, on $at (rc=$rc: $(actions); $(cat "$C/out"))"; fi
    done
}
case_2b() {

    # --- once merged: the release is Deploy prod's -------------------------------------
    # Merged, every run, dry or acting, on prod (behind origin's or up to
    # date) or on the release branch, names Deploy prod's run on prod's tip
    # and changes nothing; so does one after Deploy prod tagged it.
    new_case
    prepare_by_hand
    merge_pr
    : > "$FAKE_STATE/deploy-run"
    git -C "$C/repo" switch -q prod
    released_ok "merged, a dry run on a prod behind origin's names the Deploy prod run and changes nothing" 0.6.0
    released_ok "merged, a run on a prod behind origin's names the Deploy prod run and changes nothing"
    git -C "$C/repo" pull -q --ff-only origin prod
    released_ok "merged, a run on prod names the Deploy prod run and changes nothing"
    git -C "$C/repo" switch -q release/0.6.0
    released_ok "merged, a dry run on the release branch names the Deploy prod run and changes nothing" 0.6.0
    released_ok "merged, a run on the release branch names the Deploy prod run and changes nothing"
    o tag v0.6.0 prod
    released_ok "merged and tagged by Deploy prod, a run names its run and changes nothing"
    if [ "$(git -C "$C/repo" tag -l v0.6.0)" = "" ] && [ "$(o rev-parse 'v0.6.0^{commit}')" = "$(o rev-parse prod)" ]; then
        pass "the runs once merged make no tag of their own"
    else fail "the runs once merged make no tag of their own ($(git -C "$C/repo" tag -l))"; fi
}
case_2c() {
    # Merged, with no Deploy prod run listed yet: the run says so, with the
    # command that lists them.
    new_case
    prepare_by_hand
    merge_pr
    RUN_LINE="The run: none on $(o rev-parse prod) yet (gh run list --workflow deploy-prod.yml --repo empathic/clax-test)" \
        released_ok "merged, with no Deploy prod run yet, a run says there is none on prod's tip and changes nothing"
}
case_3() {

    # --- resume after a failing version check ------------------------------------------
    new_case
    : > "$FAKE_STATE/check-fail"
    act; rc=$?
    if [ "$rc" = 1 ] && has "step 6 (Check the version files) failed" && has "scripts/release.sh 0.6.0 --i-know-what-i-am-doing" \
        && has "The last 50 lines of $C/repo/target/release/0.6.0.log:" && has "fake check-version: plugins/x/VERSION is 0.5.0, not 0.6.0" \
        && has "The log: $C/repo/target/release/0.6.0.log" \
        && grep -x 'CLAX_VERSION="0.6.0"' "$WT/scripts/ensure-clax.sh" >/dev/null \
        && grep -x 'CLAX_VERSION="0.5.0"' "$C/repo/scripts/ensure-clax.sh" >/dev/null \
        && [ "$(git -C "$WT" symbolic-ref --short HEAD)" = release/0.6.0 ] \
        && [ "$(git -C "$WT" log -1 --format=%s)" = "Merge origin/main into release/0.6.0" ] \
        && [ "$(git -C "$C/repo" symbolic-ref --short HEAD)" = main ] && [ "$(git -C "$C/repo" rev-parse HEAD)" = "$MAIN0" ] \
        && [ -z "$(git -C "$C/repo" status --porcelain)" ] \
        && [ -z "$(o branch --list release/0.6.0)" ]; then
        pass "a failing version check stops the run before the release commit, naming the step and how to resume, the worktree left and the checkout untouched"
    else fail "a failing version check stops the run (rc=$rc: $(cat "$C/out"))"; fi
    release 0.6.0; rc=$?
    if [ "$rc" = 0 ] && has "  Here, in $WT, where release/0.6.0 is checked out already:" \
        && has "2. [done] Make the release branch, in a worktree of its own" \
        && has "4. [done] Bump the version" && has "5. [todo] Sync Bazel" && has "starts at step 5"; then
        pass "a dry run after it shows the worktree, the bump done and the rest to do"
    else fail "a dry run after a failing version check (rc=$rc: $(cat "$C/out"))"; fi
    rm "$FAKE_STATE/check-fail"
    act; rc=$?
    if [ "$rc" = 0 ] && prepared_ok && [ "$(count "bump 0.6.0")" = 1 ] && has "step 4/10 Bump the version: done already" \
        && has "step 2/10 Make the release branch, in a worktree of its own: done already" \
        && has "step 3/10 Merge main into it: done already" && logs "  removed $WT and the local release/0.6.0"; then
        pass "the next run resumes at the version check in the worktree, prepares the release, bumping once, and removes the worktree"
    else fail "resume after a failing version check (rc=$rc: $(cat "$C/out"))"; fi
}
case_3b() {
    # A refused push of the release branch; with its worktree removed by
    # hand, the next run checks the local branch out in a worktree again and
    # pushes it.
    new_case
    : > "$FAKE_STATE/reject-branch"
    act; rc=$?
    if [ "$rc" = 1 ] && has "step 8 (Push the release branch, then remove the worktree) failed" && [ -z "$(o branch --list release/0.6.0)" ] \
        && [ "$(git -C "$WT" log -1 --format=%s)" = "Release 0.6.0" ] && [ "$(count "gh pr create")" = 0 ] \
        && [ "$(git -C "$C/repo" rev-parse HEAD)" = "$MAIN0" ]; then
        pass "a refused push of the release branch stops at step 8 with the release commit made, in the worktree"
    else fail "a refused push of the release branch (rc=$rc: $(cat "$C/out"))"; fi
    rm "$FAKE_STATE/reject-branch"
    git -C "$C/repo" worktree remove "$WT"
    act; rc=$?
    if [ "$rc" = 0 ] && prepared_ok \
        && has "step 2/10 Make the release branch, in a worktree of its own: git worktree add" \
        && logs "  release/0.6.0 is checked out in $WT" \
        && has "step 7/10 Commit the release: done already" && [ "$(count "check-version")" = 1 ]; then
        pass "the next run, its worktree gone, checks the local release branch out in a worktree again and pushes it"
    else fail "resume at the local release branch with no worktree (rc=$rc: $(cat "$C/out"))"; fi
}
case_3e() {
    # The release branch checked out in the checkout itself: the run works
    # there, and leaves it there after the push.
    new_case
    git -C "$C/repo" switch -q --no-track -c release/0.6.0 origin/prod
    act; rc=$?
    rel="$(o rev-parse -q --verify refs/heads/release/0.6.0)"
    if [ "$rc" = 0 ] && origin_ok && has "step 2/10 Make the release branch, in a worktree of its own: done already" \
        && [ "$(git -C "$C/repo" symbolic-ref --short HEAD)" = release/0.6.0 ] \
        && [ "$(git -C "$C/repo" rev-parse HEAD)" = "$rel" ] && [ -z "$(git -C "$C/repo" status --porcelain)" ] \
        && [ ! -e "$WT" ] && [ "$(git -C "$C/repo" worktree list --porcelain | grep -c '^worktree ')" = 1 ] \
        && ! logs "removed "; then
        pass "a release branch checked out in the checkout already is prepared in place and left there after the push"
    else fail "a release branch checked out in the checkout (rc=$rc: $(cat "$C/out"))"; fi
}
case_3f() {
    # A checkout with changes and untracked files, on main: the run works
    # in its worktree and leaves them as they were.
    new_case
    echo "local edit" >> "$C/repo/bazel/workspace.bzl"
    echo scratch > "$C/repo/notes.txt"
    MAIN_STATUS="$(git -C "$C/repo" status --porcelain)"
    act; rc=$?
    if [ "$rc" = 0 ] && MAIN_STATUS="$MAIN_STATUS" prepared_ok \
        && [ "$(cat "$C/repo/bazel/workspace.bzl")" = "$(printf 'version = 0.5.0\nlocal edit')" ] \
        && [ "$(cat "$C/repo/notes.txt")" = scratch ]; then
        pass "changes and untracked files in the checkout run from do not stop a run, which leaves them as they were"
    else fail "a run from a checkout with changes (rc=$rc: $(git -C "$C/repo" status --porcelain); $(cat "$C/out"))"; fi
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
    if [ "$rc" = 0 ] && prepared_ok && has "step 8/10 Push the release branch, then remove the worktree: done already"; then
        pass "the next run opens the pull request"
    else fail "resume at the pull request (rc=$rc: $(git -C "$C/repo" status --porcelain; git -C "$C/repo" branch; git -C "$C/repo" worktree list); $(cat "$C/out"))"; fi
}
case_3d() {
    # prod and main changed the same lines: the merge takes main's, saying
    # in which file, and the run goes on.
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
    if [ "$rc" = 0 ] && prepared_ok && has "prod and origin/main changed the same lines in: fix.txt; the merge takes origin/main's there" \
        && [ "$(o show release/0.6.0:fix.txt)" = main ]; then
        pass "where prod and main changed the same lines, the merge takes main's and says in which file"
    else fail "a merge of main over the same lines (rc=$rc: $(cat "$C/out"))"; fi
}
case_3d2() {
    # A merge of main git cannot settle by lines (prod changed a file main
    # removed): the run stops saying how to merge by hand; merged so, the
    # next run goes on.
    new_case
    git -C "$C/repo" switch -q prod
    echo "prod/" >> "$C/repo/.gitignore"
    git -C "$C/repo" -c commit.gpgsign=false commit -q -am "Fix on prod"
    git -C "$C/repo" push -q origin prod
    git -C "$C/repo" switch -q main
    git -C "$C/repo" rm -q .gitignore
    git -C "$C/repo" -c commit.gpgsign=false commit -q -m "Remove it on main"
    git -C "$C/repo" push -q origin main
    PROD0="$(o rev-parse prod)" MAIN0="$(o rev-parse main)"
    # main removed .gitignore, so its checkout shows target/, which the run
    # makes: the checkout's commit is checked, not its status.
    act; rc=$?
    if [ "$rc" = 1 ] && has "step 3 (Merge main into it) failed" && has "conflicts in: .gitignore" \
        && has "Merge by hand in $WT, signed and with this subject:" \
        && has "git -C $WT merge -S origin/main -m \"Merge origin/main into release/0.6.0\"" \
        && [ "$(git -C "$WT" rev-parse HEAD)" = "$PROD0" ] && [ -z "$(git -C "$WT" status --porcelain)" ] \
        && [ "$(git -C "$C/repo" rev-parse HEAD)" = "$MAIN0" ] \
        && [ "$(count "bump 0.6.0")" = 0 ]; then
        pass "a merge of main that conflicts stops at step 3, saying how to merge by hand in the worktree"
    else fail "a merge of main that conflicts (rc=$rc: $(cat "$C/out"))"; fi
    git -C "$WT" merge -q -S origin/main -m "Merge origin/main into release/0.6.0" >/dev/null 2>&1
    git -C "$WT" rm -q .gitignore
    git -C "$WT" commit -q -S --no-edit
    act; rc=$?
    if [ "$rc" = 0 ] && has "step 3/10 Merge main into it: done already"; then
        pass "merged by hand as told, the next run goes on"
    else fail "resume after a merge by hand (rc=$rc: $(cat "$C/out"))"; fi
}
case_3d3() {
    # prod holds a release main was never carried (prod is at 0.5.1, main at
    # 0.5.0): lines both changed are not taken from main; the run stops.
    new_case
    git -C "$C/repo" switch -q prod
    sed -i.bak 's/^CLAX_VERSION=.*/CLAX_VERSION="0.5.1"/' "$C/repo/scripts/ensure-clax.sh" "$C/repo/plugins/pi/scripts/ensure-clax.sh"
    rm "$C/repo/scripts/ensure-clax.sh.bak" "$C/repo/plugins/pi/scripts/ensure-clax.sh.bak"
    echo prod > "$C/repo/fix.txt"
    git -C "$C/repo" add -A
    git -C "$C/repo" -c commit.gpgsign=false commit -q -m "Release 0.5.1"
    git -C "$C/repo" push -q origin prod
    git -C "$C/repo" switch -q main
    echo main > "$C/repo/fix.txt"
    git -C "$C/repo" add fix.txt
    git -C "$C/repo" -c commit.gpgsign=false commit -q -m "Fix on main"
    git -C "$C/repo" push -q origin main
    PROD0="$(o rev-parse prod)"
    act; rc=$?
    if [ "$rc" = 1 ] && has "step 3 (Merge main into it) failed" && has "conflicts in: fix.txt" \
        && has "origin/prod is at 0.5.1 and origin/main at 0.5.0: prod holds a release not carried to main" \
        && [ "$(git -C "$WT" rev-parse HEAD)" = "$PROD0" ] && [ -z "$(git -C "$WT" status --porcelain)" ] \
        && [ "$(count "bump 0.6.0")" = 0 ]; then
        pass "while prod holds a release not carried to main, lines both changed stop step 3 for a merge by hand"
    else fail "a merge over the same lines with an uncarried release on prod (rc=$rc: $(cat "$C/out"))"; fi
}
case_18() {

    # --- preflight refusals --------------------------------------------------------------
    # Changes and untracked files count in the release branch's worktree
    # alone (case_3f: the checkout's own do not).
    new_case
    add_worktree
    echo "local edit" >> "$WT/bazel/workspace.bzl"
    refuse "a changed file in the release worktree" "uncommitted changes in $WT: bazel/workspace.bzl"
}
case_18b() {
    # A run whose push was refused leaves the worktree with the release
    # commit; a change made in it after that is refused.
    new_case
    : > "$FAKE_STATE/reject-branch"
    act
    rm "$FAKE_STATE/reject-branch"
    echo "local edit" >> "$WT/main.txt"
    refuse "a change in the worktree a failed run left, after the release commit" "uncommitted changes in $WT: main.txt"
}
case_19() {
    new_case
    add_worktree
    echo scratch > "$WT/notes.txt"
    refuse "an untracked file in the release worktree" "untracked files in $WT: notes.txt"
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
    # Preparing: the pull request reads merged, but origin's prod does not
    # carry the version.
    new_case
    prepare_by_hand
    echo "MERGED https://github.com/empathic/clax-test/pull/7" > "$FAKE_STATE/pr"
    refuse "a merged pull request with prod not at the version" "the pull request https://github.com/empathic/clax-test/pull/7 is merged, but origin/prod is at 0.5.0, not 0.6.0"
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
    refuse "a tag of the version here before prod has it" "the tag v0.6.0 exists, but origin/prod is at 0.5.0, not 0.6.0"
}
case_26() {
    new_case
    o tag v0.6.0 main
    refuse "a tag of the version on origin before prod has it" "the tag v0.6.0 exists, but origin/prod is at 0.5.0, not 0.6.0"
}
case_27() {
    new_case
    : > "$FAKE_STATE/no-auth"
    refuse "gh signed out" "gh is not signed in (gh auth login)"
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
    add_worktree
    sed -i.bak 's/^CLAX_VERSION=/CLAX_VERSION_GONE=/' "$WT/scripts/ensure-clax.sh"
    rm "$WT/scripts/ensure-clax.sh.bak"
    refuse "versions that disagree in the release worktree" "release/0.6.0's versions disagree, in $WT (scripts/check-version.sh)"
}
case_31b() {
    # The checkout's own versions are not read.
    new_case
    sed -i.bak 's/^CLAX_VERSION=/CLAX_VERSION_GONE=/' "$C/repo/scripts/ensure-clax.sh"
    rm "$C/repo/scripts/ensure-clax.sh.bak"
    MAIN_STATUS="$(git -C "$C/repo" status --porcelain)"
    act; rc=$?
    if [ "$rc" = 0 ] && MAIN_STATUS="$MAIN_STATUS" prepared_ok; then
        pass "versions that disagree in the checkout run from do not stop a run"
    else fail "versions that disagree in the checkout run from (rc=$rc: $(cat "$C/out"))"; fi
}
case_31c() {
    # origin's release branch ahead of the local one.
    new_case
    add_worktree
    git -C "$WT" -c commit.gpgsign=false commit -q --allow-empty -m "Elsewhere"
    git -C "$WT" push -q origin release/0.6.0
    git -C "$WT" reset -q --hard origin/prod
    git -C "$C/repo" fetch -q origin
    refuse "an origin release branch ahead of the local one" "origin's release/0.6.0 has commits the local release/0.6.0 lacks (git -C $WT pull --ff-only, or git branch -D release/0.6.0)"
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
case_36() {

    # --- bazel-sync that never settles --------------------------------------------------
    new_case
    : > "$FAKE_STATE/sync-forever"
    act; rc=$?
    if [ "$rc" = 1 ] && [ "$(count "just bazel-sync")" = 4 ] && has "just bazel-sync still changed files on its 4th run" \
        && has "step 5 (Sync Bazel with Cargo) failed" && [ "$(count "check-version")" = 0 ]; then
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
    # earlier log), steps 1-2 are 15s of 655s, steps 1-8 645s.
    new_case
    : > "$FAKE_STATE/pr-create-fail"
    act
    rm "$FAKE_STATE/pr-create-fail" "$C/repo/target/release/0.6.0.log"
    act; rc=$?
    if [ "$rc" = 0 ] && prepared_ok && has "  8. [done] Push the release branch" && has "  9. [todo] Open the pull request into prod" \
        && has "Estimated: about 11m in all, under a minute for the steps to do (typical durations" \
        && has "[2%] step 2/10 Make the release branch, in a worktree of its own: done already" \
        && has "[98%] step 8/10 Push the release branch, then remove the worktree: done already" \
        && has "[98%] step 9/10 Open the pull request into prod: gh pr create" \
        && has "Starting at step 9."; then
        pass "a resumed run counts the steps done in its percentage"
    else fail "a resumed run's percentage (rc=$rc: $(cat "$C/out"))"; fi
}
case_39() {
    # The durations a run records in its log make the next estimate: step 6
    # takes 300s, steps 1-5, 7 and 8 none (counted as 1s); 9, which failed,
    # and 10 keep their typical 5s: 317s in all, 10s to do.
    new_case
    echo 300 > "$FAKE_STATE/check-advance"
    : > "$FAKE_STATE/pr-create-fail"
    act; rc=$?
    if [ "$rc" = 1 ] && logs "== step 6 of 10 ran 300s" && logs "== step 8 of 10 ran 0s" && logs "== step 1 of 10 ran 0s" \
        && ! logs "== step 9 of 10 ran"; then
        pass "a run records each step's duration in its log"
    else fail "a run records each step's duration (rc=$rc: $(cat "$C/out"))"; fi
    rm "$FAKE_STATE/pr-create-fail" "$FAKE_STATE/check-advance"
    release 0.6.0; rc=$?
    if [ "$rc" = 0 ] && has "Estimated: about 5m in all, under a minute for the steps to do (the steps' durations in 1 earlier run log(s)"; then
        pass "the estimate comes from earlier runs' logs"
    else fail "the estimate from earlier runs' logs (rc=$rc: $(cat "$C/out"))"; fi
    # Logs of earlier numberings are not read: "== step N ran Ss", and the
    # 21 steps' "== step N of 21 ran Ss".
    printf '== step 4 ran 9999s\n' > "$C/repo/target/release/0.4.0.log"
    printf '== step 4 of 21 ran 9999s\n' > "$C/repo/target/release/0.5.0.log"
    release 0.6.0; rc=$?
    if [ "$rc" = 0 ] && has "Estimated: about 5m in all"; then
        pass "a log of an earlier step numbering does not count"
    else fail "a log of an earlier step numbering (rc=$rc: $(cat "$C/out"))"; fi
}
case_40() {
    # A heartbeat while the version check runs, its clock moved on by 3m12s,
    # past the step's 5s estimate: the share done (630s of 655s, the step
    # counted for no more than its weight), the time taken, and no time left
    # claimed.
    new_case
    echo 192 > "$FAKE_STATE/check-advance"
    echo "Check the version files: 3m12s" > "$FAKE_STATE/check-heartbeat"
    CLAX_RELEASE_HEARTBEAT_SECONDS=0.05 act; rc=$?
    if [ "$rc" = 0 ] && prepared_ok \
        && has "[96%] step 6/10 Check the version files: 3m12s (past this step's estimate of 5s; how much longer is not known)" \
        && ! grep -F "Check the version files: 3m12s" "$C/out" | grep -F "left" >/dev/null; then
        pass "a heartbeat past its step's estimate tells the share done and the time taken, and claims no time left"
    else fail "a heartbeat past its step's estimate (rc=$rc: $(cat "$C/out"))"; fi
}
case_40b() {
    # A heartbeat while the version check runs, its clock moved on by 3s,
    # within the step's 5s estimate: the share done (628s of 655s) and the
    # time left (27s).
    new_case
    echo 3 > "$FAKE_STATE/check-advance"
    echo "Check the version files: 3s" > "$FAKE_STATE/check-heartbeat"
    CLAX_RELEASE_HEARTBEAT_SECONDS=0.05 act; rc=$?
    if [ "$rc" = 0 ] && prepared_ok \
        && has "[95%] step 6/10 Check the version files: 3s (under a minute left, estimated)" \
        && ! has "past this step's estimate"; then
        pass "a heartbeat within its step's estimate tells the share done, the time taken and the time left"
    else fail "a heartbeat within its step's estimate (rc=$rc: $(cat "$C/out"))"; fi
}
case_41() {
    # --verbose streams the steps' output too, still logging it.
    new_case
    release 0.6.0 --i-know-what-i-am-doing --verbose; rc=$?
    if [ "$rc" = 0 ] && prepared_ok && has "fake check-version: the versions agree" \
        && has "[95%] step 6/10 Check the version files: scripts/check-version.sh" && logs "fake check-version: the versions agree"; then
        pass "--verbose streams the steps' output and logs it"
    else fail "--verbose (rc=$rc: $(cat "$C/out"))"; fi
}

case_42() {
    # --- no version: the current release and the next ones ---------------------------
    new_case
    : > "$FAKE_STATE/calls"
    before="$(snapshot)"
    release; rc=$?
    if [ "$rc" = 0 ] && [ "$before" = "$(snapshot)" ] && [ -z "$(actions)" ] \
        && has "The current release: 0.5.0" && has "origin's newest release tag: v0.5.0" \
        && has "origin/prod: 0.5.0" && has "origin/main: 0.5.0" && ! has "under way" \
        && has "just release 0.5.1    a patch" && has "just release 0.6.0    a minor release" \
        && has "just release 1.0.0    a major release"; then
        pass "with no version, it prints the current release and the next ones"
    else fail "with no version (rc=$rc: $(actions); $(cat "$C/out"))"; fi
    prepare_by_hand
    release; rc=$?
    if [ "$rc" = 0 ] && has "under way: release/0.6.0, https://github.com/empathic/clax-test/pull/7 (just release 0.6.0 says where it is)"; then
        pass "with no version, it names a release pull request open into prod"
    else fail "with no version, an open release pull request (rc=$rc: $(cat "$C/out"))"; fi
    merge_pr
    release; rc=$?
    if [ "$rc" = 0 ] && has "The current release: 0.6.0" && has "origin/prod: 0.6.0, not tagged yet (Deploy prod tags it)" \
        && has "just release 0.6.1    a patch" && ! has "under way"; then
        pass "with no version, prod's merged release counts before its tag"
    else fail "with no version, prod merged before its tag (rc=$rc: $(cat "$C/out"))"; fi
    release --i-know-what-i-am-doing; rc=$?
    if [ "$rc" = 2 ]; then pass "the act flag with no version is a usage error"
    else fail "the act flag with no version (rc=$rc: $(cat "$C/out"))"; fi
}

JOBS="$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 4)"
[ "$JOBS" -le 8 ] || JOBS=8
CASES="case_1 case_2 case_2b case_2c case_3 case_3b case_3c case_3d case_3d2 case_3d3 case_3e case_3f case_18 case_18b case_19 case_21 case_22 case_22b case_22c case_23 case_24 case_25 case_26 case_27 case_29 case_30 case_31 case_31b case_31c case_32 case_33 case_36 case_37 case_38 case_39 case_40 case_40b case_41 case_42"
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
