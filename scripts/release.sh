#!/usr/bin/env bash
# Cuts a Clax release: the pull request from release/X.Y.Z into prod, the
# one local step of a release (docs/usage.md, "Releasing"). The pull
# request is staging; merging it releases it, in GitHub Actions (Deploy
# prod: the deploy, once the owner approves, the GitHub release, the carry
# to main with the plugins' pin, the Pi package).
#
#   scripts/release.sh X.Y.Z                              a dry run
#   scripts/release.sh X.Y.Z --i-know-what-i-am-doing     cuts the pull request
#   ... --verbose                                         streams every step's output
#
# The dry run, the default, runs the read-only checks and prints every step
# with whether it is done already; it changes nothing (it fetches origin's
# main and prod, and the release branch when origin has it, which moves only
# the remote-tracking refs). With --i-know-what-i-am-doing it acts, as the
# checkout's git identity, signing with its signing key. It builds and
# tests nothing: CI does, on the pull request.
#
#    1  preflight: gh signed in; X.Y.Z newer than every v* tag and than
#       origin/prod's version; commit signing works; the tools present; a
#       clean checkout on any branch, or on release/X.Y.Z ahead of
#       origin/prod only by merges of origin/main and the release commit.
#       A failure here changes nothing.
#    2  git switch -c release/X.Y.Z origin/prod (or to that branch, when it
#       exists here or on origin)
#    3  a signed merge of origin/main into it, "Merge origin/main into
#       release/X.Y.Z" (none when it has origin/main already)
#    4  scripts/bump-version.sh X.Y.Z
#    5  just bazel-sync, until it changes nothing (at most 4 runs)
#    6  scripts/check-version.sh: the version files agree
#    7  a signed commit "Release X.Y.Z" (git commit -S -a)
#    8  git push origin release/X.Y.Z
#    9  gh pr create --base prod --head release/X.Y.Z, with the release's
#       checklist; the run stops here
#   10  the owner merges the pull request on GitHub, with a merge commit,
#       once CI and staging pass, and approves Deploy prod's deploy.
#
# A run while the pull request is open prints its URL and changes nothing;
# one once origin's prod is at X.Y.Z prints the Deploy prod run that
# releases it. A run that fails stops at that step, says which, and changes
# nothing after it. Run the same command again: each step's state is read
# from git and GitHub (the version files, the commits by subject, the
# release branch here and on origin, its pull request's state), so the run
# resumes at the first step not done. Uncommitted changes are taken only
# while steps 4-6 are pending the release commit; any other change, an
# untracked file, or a commit other than the release's own, refuses to run.
# It never force-pushes, and checks that every commit it pushes carries a
# signature.
#
# Before the first step it lists every step, done or to do, with an
# estimate of the time in all and still to run: the steps' durations in
# earlier runs' logs (target/release/*.log), averaged, or else typical
# ones; the pull request's review is not counted. Each step then prints one
# line, with the share of the estimate done ([42%]; steps found done count
# in full), and a step that runs longer prints a heartbeat about every 30
# seconds: the time it has taken, the time left, and what is cheap to tell.
# Everything a step prints goes to target/release/X.Y.Z.log only; a failed
# step's last 50 lines are printed with the log's path. With --verbose the
# steps' output streams to the terminal too, as well as to the log, and
# there is no heartbeat. The pull request's URL always reaches the
# terminal.
#
# It works from any directory: the checkout is the one holding this script.
#
# Environment:
#   GH_REPO                     the GitHub repository (default: origin's)
#   CLAX_RELEASE_HEARTBEAT_SECONDS
#                               seconds between heartbeats (30; 0: none)
#   CLAX_RELEASE_NOW_FILE       a file holding the time, in seconds since
#                               the epoch, read in place of the clock; for
#                               tests
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
ACT_FLAG="--i-know-what-i-am-doing"
# The branch releases come from, and the last step.
PROD=prod
LAST=10

usage() {
    echo "usage: scripts/release.sh X.Y.Z [$ACT_FLAG] [--verbose]" >&2
    exit 2
}
V=""
ACT=0
VERBOSE=0
for arg in "$@"; do
    case "$arg" in
        "$ACT_FLAG") ACT=1 ;;
        --verbose) VERBOSE=1 ;;
        -h|--help) sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed '$d; s/^# \{0,1\}//'; exit 0 ;;
        -*) usage ;;
        *) [ -z "$V" ] || usage; V="$arg" ;;
    esac
done
[ -n "$V" ] || usage
TAG="v$V"
RB="release/$V"
REL_SUBJECT="Release $V"
MERGE_SUBJECT="Merge origin/main into $RB"
LOG="$ROOT/target/release/$V.log"
RESUME="scripts/release.sh $V $ACT_FLAG"
# 1 once the run acts: from then on, say and logged append to LOG.
LOGGING=0
# 1 inside a step that is not --verbose: its output goes to LOG already.
IN_LOG=0
HEARTBEAT="${CLAX_RELEASE_HEARTBEAT_SECONDS:-30}"
# The terminal, for what a quiet step shows there (tell, npm publish).
exec 3>&1 4>&2

g() { git -C "$ROOT" "$@"; }
# What this shares with scripts/release-sdk.sh: versions, signing, the npm
# environment, npm, workflow runs and their waits.
# shellcheck source=scripts/lib/release-common.sh
. "$ROOT/scripts/lib/release-common.sh"
say() {
    printf '%s\n' "$*"
    if [ "$LOGGING" = 1 ] && [ "$IN_LOG" = 0 ]; then printf '%s\n' "$*" >> "$LOG"; fi
}
# Says something on the terminal also from inside a quiet step.
tell() {
    if [ "$IN_LOG" = 1 ]; then printf '%s\n' "$*" >&3; fi
    say "$*"
}
# Runs a command, its output appended to the log, and shown with --verbose.
logged() {
    if [ "$IN_LOG" = 1 ]; then "$@" 2>&1; else "$@" 2>&1 | tee -a "$LOG"; fi
}

# --- progress -------------------------------------------------------------------

now() {
    if [ -n "${CLAX_RELEASE_NOW_FILE:-}" ]; then cat "$CLAX_RELEASE_NOW_FILE"; else date +%s; fi
}
# A duration in seconds as 45s, 3m12s or 1h05m.
dur() {
    local s="$1"
    if [ "$s" -ge 3600 ]; then printf '%dh%02dm' $((s / 3600)) $((s % 3600 / 60))
    elif [ "$s" -ge 60 ]; then printf '%dm%02ds' $((s / 60)) $((s % 60))
    else printf '%ds' "$s"; fi
}
# A duration still to come, rounded: "about 18m", or "under a minute".
about() {
    if [ "$1" -ge 60 ]; then printf 'about %dm' $((($1 + 30) / 60)); else printf 'under a minute'; fi
}
# Each step's expected seconds: typical durations, replaced by the average
# of the durations earlier runs' logs recorded ("== step N of 21 ran Ss";
# logs of the earlier numbering, "== step N ran Ss", are not read).
WEIGHTS=([1]=10 [2]=5 [3]=5 [4]=5 [5]=30 [6]=5 [7]=5 [8]=10 [9]=5 [10]=5)
LEARNED_FROM=0
learn_weights() {
    local f n s
    local -a logs=()
    for f in "$ROOT"/target/release/*.log; do [ -f "$f" ] && logs+=("$f"); done
    [ "${#logs[@]}" -gt 0 ] || return 0
    while read -r n s; do
        WEIGHTS[n]="$s"
        LEARNED_FROM="${#logs[@]}"
    done < <(awk -v last="$LAST" '$1 == "==" && $2 == "step" && $3 ~ /^[0-9]+$/ && $4 == "of" && $5 == last \
            && $6 == "ran" && $7 ~ /^[0-9]+s$/ { sum[$3] += $7 + 0; c[$3]++ }
        END { for (n in c) { a = int(sum[n] / c[n] + 0.5); print n, (a < 1 ? 1 : a) } }' "${logs[@]}")
}
TOTAL_W=0
total_weight() {
    local n t=0
    for n in $(seq 1 "$LAST"); do t=$((t + WEIGHTS[n])); done
    echo "$t"
}
pct() { echo $(($1 * 100 / TOTAL_W)); }
# The weight of the steps done, and the step under way: STEP, its start
# STEP_START.
DONE_W=0
STEP=1
STEP_START=0
# HB_DIR holds what a step says of itself for the heartbeat: detail (one
# line), run (the workflow run it waits for), pause (no heartbeat now).
HB_DIR=""
TICKER=""
detail() { if [ -n "$HB_DIR" ]; then printf '%s\n' "$*" > "$HB_DIR/detail"; fi; }
# What a heartbeat can tell of the step under way.
step_detail() {
    local d="" id jobs
    if [ -s "$HB_DIR/detail" ]; then d="$(cat "$HB_DIR/detail")"; fi
    if [ -s "$HB_DIR/run" ]; then
        id="$(cat "$HB_DIR/run")"
        jobs="$(gh run view "$id" --json jobs \
            --jq '"\([.jobs[] | select(.status == "completed")] | length)/\(.jobs | length) jobs done"' 2>/dev/null)" || jobs=""
        if [ -n "$jobs" ]; then d="${d:+$d, }$jobs"; fi
    fi
    printf '%s' "$d"
}
# One heartbeat line: "[40%] step 5/10 Sync Bazel with Cargo: run 2,
# 1m12s (about 1m left)". The step counts for no more than its
# weight.
heartbeat() {
    local e cur d left p
    e=$(($(now) - STEP_START))
    cur="$e"
    [ "$cur" -le "${WEIGHTS[STEP]}" ] || cur="${WEIGHTS[STEP]}"
    left=$((TOTAL_W - DONE_W - cur))
    p="$(pct $((DONE_W + cur)))"
    [ "$p" -le 99 ] || p=99
    d="$(step_detail)"
    printf '[%d%%] step %d/%d %s: %s%s (%s left)\n' "$p" "$STEP" "$LAST" "${TITLES[STEP]}" "${d:+$d, }" "$(dur "$e")" "$(about "$left")"
}
# Prints a heartbeat to the terminal every HEARTBEAT seconds until stopped.
HB_SLEEP=""
ticker() {
    trap - EXIT
    trap 'if [ -n "$HB_SLEEP" ]; then kill "$HB_SLEEP" 2>/dev/null; fi; exit 0' TERM
    while :; do
        sleep "$HEARTBEAT" &
        HB_SLEEP=$!
        wait "$HB_SLEEP" || true
        HB_SLEEP=""
        [ -e "$HB_DIR/pause" ] || heartbeat >&3 2>/dev/null || true
    done
}
start_ticker() {
    rm -f "$HB_DIR/detail" "$HB_DIR/run" "$HB_DIR/pause"
    if [ "$VERBOSE" = 0 ] && [ "$HEARTBEAT" != 0 ]; then
        ticker &
        TICKER=$!
    fi
}
stop_ticker() {
    if [ -n "$TICKER" ]; then
        kill "$TICKER" 2>/dev/null || true
        wait "$TICKER" 2>/dev/null || true
        TICKER=""
    fi
}
cleanup() {
    stop_ticker
    if [ -n "$HB_DIR" ]; then rm -rf "$HB_DIR"; fi
}
trap cleanup EXIT

# --- state, read from git and GitHub -------------------------------------------

checkout_version() { bash "$ROOT/scripts/check-version.sh" --print 2>/dev/null; }
# The Clax version commit $1 carries (its scripts/ensure-clax.sh), or nothing.
version_at() { g show "$1:scripts/ensure-clax.sh" 2>/dev/null | sed -n 's/^CLAX_VERSION="\(.*\)"$/\1/p'; }
# The oldest commit in revision range $2 whose subject is exactly $1.
subject_in() {
    g log --format='%H%x09%s' --fixed-strings --grep="$1" "$2" -- 2>/dev/null | awk -F '\t' -v s="$1" '$2 == s { h = $1 } END { print h }'
}
has_ref() { g rev-parse -q --verify "$1^{commit}" >/dev/null 2>&1; }
is_signed_commit() { g cat-file commit "$1" | grep -E '^gpgsig(-sha256)? ' >/dev/null; }
# The newest pull request from the release branch into prod, as "STATE URL"
# (OPEN, CLOSED or MERGED), or nothing.
pr_state() {
    gh pr list --head "$RB" --base "$PROD" --state all --limit 1 --json state,url --jq '.[] | "\(.state) \(.url)"'
}

CUR="" BRANCH="" PROD_TIP="" PROD_VERSION="" PHASE=1 REMOTE_RB="" RB_REF="" REL_COMMIT="" PR_STATE="" PR_URL=""
# Reads the state the steps are judged by; fails, saying why in
# REFRESH_ERR, when origin or GitHub cannot be read.
refresh() {
    local out
    CUR="$(checkout_version || true)"
    BRANCH="$(g symbolic-ref --short -q HEAD || true)"
    PROD_TIP="$(g rev-parse -q --verify "refs/remotes/origin/$PROD^{commit}" 2>/dev/null || true)"
    PROD_VERSION=""
    if [ -n "$PROD_TIP" ]; then PROD_VERSION="$(version_at "$PROD_TIP")"; fi
    # Merged once origin's prod carries this version (Deploy prod releases
    # it from there); preparing until then.
    PHASE=1
    if [ -n "$PROD_VERSION" ] && [ "$PROD_VERSION" = "$V" ]; then PHASE=2; fi
    out="$(g ls-remote origin "refs/heads/$RB")" \
        || { REFRESH_ERR="cannot read origin (git ls-remote origin)"; return 1; }
    REMOTE_RB="$(awk -v b="refs/heads/$RB" '$2 == b { print $1 }' <<< "$out")"
    # The release branch: here, or else origin's copy.
    RB_REF=""
    if has_ref "refs/heads/$RB"; then RB_REF="refs/heads/$RB"
    elif [ -n "$REMOTE_RB" ] && has_ref "refs/remotes/origin/$RB"; then RB_REF="refs/remotes/origin/$RB"; fi
    # The release commit: the one on the release branch after origin/prod.
    REL_COMMIT=""
    if [ "$PHASE" = 1 ] && [ -n "$RB_REF" ] && [ -n "$PROD_TIP" ]; then
        REL_COMMIT="$(subject_in "$REL_SUBJECT" "$PROD_TIP..$RB_REF")"
    fi
    # The pull request, once the branch is on origin and until prod has it.
    PR_STATE=""
    PR_URL=""
    if [ "$PHASE" = 1 ] && [ -n "$REMOTE_RB" ]; then
        out="$(pr_state)" || { REFRESH_ERR="cannot read the pull requests from $RB (gh pr list)"; return 1; }
        PR_STATE="${out%% *}"
        PR_URL="${out#* }"
    fi
}

# --- step 1: preflight ------------------------------------------------------------

PROBLEMS=()
problem() { PROBLEMS+=("$1"); }

check_tools() {
    local t
    for t in git gh just python3 curl; do
        command -v "$t" >/dev/null 2>&1 || problem "$t is not on PATH"
    done
    command -v bazelisk >/dev/null 2>&1 || command -v bazel >/dev/null 2>&1 \
        || problem "neither bazelisk nor bazel is on PATH (just bazel-sync runs Bazel)"
}

check_version() {
    local remote tags t old
    if ! semver_ok "$V"; then
        problem "$V is not a release version (X.Y.Z)"
        return 0
    fi
    remote="$(g ls-remote --tags --refs origin 'v*')" || { problem "cannot list origin's tags"; return 0; }
    tags="$( { g tag -l 'v*'; awk '{ sub("refs/tags/", "", $2); print $2 }' <<< "$remote"; } | sort -u)"
    for t in $tags; do
        [[ $t =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-.*)?$ ]] || continue
        [ "$t" != "$TAG" ] || continue
        case "$(ver_cmp "${t#v}" "$V")" in
            1) problem "$V is not newer than the tag $t" ;;
            0) [[ $t == *-* ]] || problem "$V is not newer than the tag $t" ;;
        esac
    done
    # Merged, the release is Deploy prod's, which tags it.
    [ -n "$PROD_TIP" ] && [ "$PHASE" = 1 ] || return 0
    if grep -qxF "$TAG" <<< "$tags"; then
        problem "the tag $TAG exists, but origin/$PROD is at ${PROD_VERSION:-an unknown version}, not $V"
    fi
    if [ -n "$PROD_VERSION" ] && [ "$(ver_cmp "$PROD_VERSION" "$V")" = 1 ]; then
        problem "origin/$PROD is at $PROD_VERSION, newer than $V"
    fi
    if old="$(subject_in "$REL_SUBJECT" origin/main)" && [ -n "$old" ]; then
        problem "origin/main has a \"$REL_SUBJECT\" commit already ($old), but origin/$PROD is not at $V"
    fi
    if [ "$BRANCH" = "$RB" ]; then
        if [ -z "$CUR" ]; then problem "the checkout's versions disagree (scripts/check-version.sh)"
        elif [ "$(ver_cmp "$CUR" "$V")" = 1 ]; then problem "$RB is at $CUR, newer than $V"; fi
    fi
}

# Uncommitted changes and untracked files: refused, but for the changes a
# step makes while it waits for its commit.
check_clean() {
    local dirty untracked
    untracked="$(g ls-files --others --exclude-standard "$ROOT")"
    [ -z "$untracked" ] || problem "untracked files: $(echo "$untracked" | tr '\n' ' ')"
    dirty="$(g diff --name-only HEAD)"
    [ -n "$dirty" ] || return 0
    # The bump (4-6) on the release branch.
    if [ "$BRANCH" = "$RB" ] && [ "$CUR" = "$V" ] && [ -z "$REL_COMMIT" ]; then return 0; fi
    problem "uncommitted changes: $(echo "$dirty" | tr '\n' ' ')"
}

# Preparing: any branch; the release branch, where there is one, holds
# origin/prod and adds only merges of origin/main and the release commit.
check_prepare() {
    local i subject
    [ -n "$RB_REF" ] || return 0
    if ! g merge-base --is-ancestor "$PROD_TIP" "$RB_REF"; then
        problem "$RB does not hold origin/$PROD's tip: origin/$PROD has moved on since the branch was cut"
        return 0
    fi
    if [ -n "$REMOTE_RB" ] && { ! has_ref "$REMOTE_RB" || ! g merge-base --is-ancestor "$REMOTE_RB" "$RB_REF"; }; then
        problem "origin's $RB has commits this checkout's $RB lacks (git switch $RB; git pull --ff-only)"
    fi
    i=0
    while IFS= read -r subject; do
        [ -n "$subject" ] || continue
        case "$i:$subject" in
            "0:$MERGE_SUBJECT") ;;
            "0:$REL_SUBJECT") i=1 ;;
            *)
                problem "$RB has a commit that is not this release's: \"$subject\""
                break ;;
        esac
    done < <(g log --reverse --topo-order --format=%s "$PROD_TIP..$RB_REF" --not origin/main)
    case "$PR_STATE" in
        MERGED) problem "the pull request $PR_URL is merged, but origin/$PROD is at ${PROD_VERSION:-an unknown version}, not $V" ;;
    esac
}

preflight() {
    check_tools
    command -v git >/dev/null 2>&1 || return 0
    resolve_repo
    g fetch --quiet --no-tags origin main || problem "cannot fetch main from origin"
    if ! g fetch --quiet --no-tags origin "$PROD"; then
        problem "cannot fetch $PROD from origin: releases come from a $PROD branch (docs/prod.md)"
    fi
    if g ls-remote --exit-code origin "refs/heads/$RB" >/dev/null 2>&1; then
        g fetch --quiet --no-tags origin "+refs/heads/$RB:refs/remotes/origin/$RB" || problem "cannot fetch $RB from origin"
    fi
    refresh || problem "$REFRESH_ERR"
    if [ -n "${PROD_TIP:-}" ] && [ "$PHASE" = 1 ]; then
        check_clean
        check_prepare
    fi
    check_version
    if command -v gh >/dev/null 2>&1 && ! gh auth status >/dev/null 2>&1; then
        problem "gh is not signed in (gh auth login)"
    fi
    # Merged, nothing is committed here.
    [ "$PHASE" = 2 ] || check_signing
}

# --- steps 2-10 -------------------------------------------------------------------

TITLES=(
    [2]="Cut the release branch"
    [3]="Merge main into it"
    [4]="Bump the version"
    [5]="Sync Bazel with Cargo"
    [6]="Check the version files"
    [7]="Commit the release"
    [8]="Push the release branch"
    [9]="Open the pull request into $PROD"
    [10]="Merge the pull request (you, on GitHub)"
)
COMMANDS=(
    [2]="git switch -c $RB origin/$PROD"
    [3]="git merge -S origin/main -m \"$MERGE_SUBJECT\""
    [4]="scripts/bump-version.sh $V"
    [5]="just bazel-sync, until it changes nothing (at most 4 runs)"
    [6]="scripts/check-version.sh"
    [7]="git commit -S -a -m \"$REL_SUBJECT\""
    [8]="git push origin $RB"
    [9]="gh pr create --base $PROD --head $RB"
    [10]="once CI and staging pass, merge it (Create a merge commit) and approve Deploy prod's deploy: the merge releases it"
)

# done_N: whether step N is done, from the state refresh read. Once origin's
# prod is at this version, every step is.
done_2() { [ "$PHASE" = 2 ] || [ "$BRANCH" = "$RB" ]; }
done_3() {
    [ "$PHASE" = 2 ] || [ -n "$REL_COMMIT" ] && return 0
    [ -n "$RB_REF" ] || return 1
    [ -n "$(subject_in "$MERGE_SUBJECT" "$PROD_TIP..$RB_REF")" ] || g merge-base --is-ancestor origin/main "$RB_REF"
}
done_4() { [ "$PHASE" = 2 ] || [ -n "$REL_COMMIT" ] || { [ "$BRANCH" = "$RB" ] && [ "$CUR" = "$V" ]; }; }
done_5() { [ "$PHASE" = 2 ] || [ -n "$REL_COMMIT" ]; }
done_6() { done_5; }
done_7() { done_5; }
done_8() {
    [ "$PHASE" = 2 ] && return 0
    [ -n "$REL_COMMIT" ] && [ -n "$REMOTE_RB" ] && g merge-base --is-ancestor "$REL_COMMIT" "$REMOTE_RB" 2>/dev/null
}
done_9() { [ "$PHASE" = 2 ] || { done_8 && { [ "$PR_STATE" = OPEN ] || [ "$PR_STATE" = MERGED ]; }; }; }
done_10() { [ "$PHASE" = 2 ]; }

worktree_digest() { { g diff HEAD; g status --porcelain=v1 -uall; } | cksum; }

# Refuses to push commit range $1 unless each of its commits is signed.
all_signed() {
    local c
    for c in $(g rev-list "$@"); do
        is_signed_commit "$c" && continue
        say "  $c (\"$(g log -1 --format=%s "$c")\") carries no signature; not pushing it."
        if [ "$c" = "$(g rev-parse HEAD)" ]; then
            say "  Sign it (git commit --amend -S --no-edit), then run this again."
        else
            say "  It is not the newest commit; signing it rewrites what follows, so sign it by hand."
        fi
        return 1
    done
}

# Switches to the release branch: this checkout's, origin's, or a new one
# from origin/prod.
step_2() {
    if has_ref "refs/heads/$RB"; then g switch -q "$RB"
    elif [ -n "$REMOTE_RB" ]; then g switch -q --track -c "$RB" "origin/$RB"
    else g switch -q --no-track -c "$RB" "origin/$PROD"; fi
}
# Merges origin/main in, signed: main carries every earlier release's
# version and pin (the carry, in Deploy prod), so the version files agree.
step_3() {
    local files
    g fetch --quiet --no-tags origin main
    if g merge-base --is-ancestor origin/main HEAD; then say "  $RB has origin/main already"; return 0; fi
    if ! g merge -S --no-ff -q -m "$MERGE_SUBJECT" origin/main; then
        files="$(g diff --name-only --diff-filter=U | tr '\n' ' ' | sed 's/ $//')"
        g merge --abort 2>/dev/null || true
        say "  the merge of origin/main into $RB conflicts in: ${files:-(see the log)}"
        say "  Merge by hand, signed and with this subject:"
        say "    git merge -S origin/main -m \"$MERGE_SUBJECT\""
        say "  resolve, then commit it signed (git commit -S; git does not carry -S past a conflict),"
        say "  then run this again."
        return 1
    fi
    is_signed_commit HEAD || { say "  the merge carries no signature"; return 1; }
}
step_4() { bash "$ROOT/scripts/bump-version.sh" "$V"; }
step_5() {
    local before after n
    for n in 1 2 3 4; do
        before="$(worktree_digest)"
        say "  just bazel-sync (run $n)"
        detail "run $n"
        (cd "$ROOT" && logged just bazel-sync)
        after="$(worktree_digest)"
        if [ "$before" = "$after" ]; then return 0; fi
    done
    say "  just bazel-sync still changed files on its 4th run"
    return 1
}
# The gates run in CI, on the pull request; this checks only that the
# version files agree before they are committed.
step_6() { (cd "$ROOT" && logged bash scripts/check-version.sh); }
step_7() {
    say "  committing:"
    g status --short | sed 's/^/    /'
    g commit -S -a -q -m "$REL_SUBJECT"
    is_signed_commit HEAD || { say "  the commit carries no signature"; return 1; }
}
# Pushes the release branch: the commits of its own (the merges and the
# release commit) are signed; main's it brings along are on origin already.
step_8() {
    g fetch --quiet --no-tags origin main
    all_signed "$PROD_TIP..HEAD" --not origin/main || return 1
    g push origin "refs/heads/$RB:refs/heads/$RB"
}
step_9() {
    local url
    url="$(gh pr create --base "$PROD" --head "$RB" --title "$REL_SUBJECT" --body "$(pr_body)")"
    tell "  the pull request: $url"
}
pr_body() {
    cat <<EOF
Clax $V: \`origin/$PROD\` with \`origin/main\` merged in and the version bumped (\`scripts/release.sh $V\`).

This pull request is staging (docs/prod.md, "Releasing"):

- [ ] CI passes on it.
- [ ] Staging (\`staging.yml\`) deployed this pull request's own instance, which it comments here, and the release candidate's binaries; it was tried there.
- [ ] Merge with a merge commit (not squash or rebase). Deploy prod then waits for the owner's approval, deploys clax.dev from the candidate staging tested, publishes $TAG, carries the release and the plugins' pin to \`main\` and publishes the Pi package.
EOF
}
# --- main -------------------------------------------------------------------------

# Lists steps 2-10, done or to do, with the estimate of the time they take;
# NEXT is the first step to do.
overview() {
    local n left=0 from
    say ""
    say "The steps (checkout on ${BRANCH:-a detached HEAD} at ${CUR:-?}; origin/$PROD at ${PROD_VERSION:-?}):"
    NEXT=""
    for n in $(seq 2 "$LAST"); do
        if [ "$n" = 2 ]; then say "  Prepare, then the pull request into $PROD:"; fi
        if [ -z "$NEXT" ] && "done_$n" 2>/dev/null; then
            say "  $n. [done] ${TITLES[n]}: ${COMMANDS[n]}"
        else
            NEXT="${NEXT:-$n}"
            left=$((left + WEIGHTS[n]))
            say "  $n. [todo] ${TITLES[n]}: ${COMMANDS[n]}"
        fi
    done
    if [ "$LEARNED_FROM" -gt 0 ]; then
        from="the steps' durations in $LEARNED_FROM earlier run log(s) under target/release/"
    else
        from="typical durations; later runs learn from their logs"
    fi
    say "Estimated: $(about "$TOTAL_W") in all, $(about "$left") for the steps to do ($from; the review not counted)."
    say ""
}

# The end of the preparation: the pull request waits for the owner.
prepared() {
    say ""
    say "The release is prepared: the pull request ${PR_URL:-from $RB} into $PROD, which is staging:"
    say "staging.yml deploys its own instance and comments the address there. Once CI and staging"
    say "pass, merge it on GitHub with a merge commit (Create a merge commit, not squash or rebase),"
    say "and approve Deploy prod's deploy: it deploys clax.dev from the candidate staging tested,"
    say "publishes $TAG, carries the release and its pin to main and publishes the Pi package."
}

# Once origin's prod carries this version, the merge has released it, in
# Deploy prod: the run there to watch.
released() {
    local url
    url="$(gh run list --workflow deploy-prod.yml --commit "$PROD_TIP" --limit 1 --json url --jq '.[0].url // ""' 2>/dev/null)" || url=""
    say ""
    say "origin/$PROD is at $V: its merge releases it, in GitHub Actions (Deploy prod), with nothing to"
    say "run here. The run: ${url:-none on $PROD_TIP yet (gh run list --workflow deploy-prod.yml --repo $GH_REPO)}"
}

learn_weights
TOTAL_W="$(total_weight)"
T0="$(now)"
if [ "$ACT" = 0 ]; then
    say "Dry run: nothing is changed. Run with $ACT_FLAG to release."
    say ""
fi
say "Step 1: preflight"
preflight
if [ "${#PROBLEMS[@]}" -gt 0 ]; then
    for p in "${PROBLEMS[@]}"; do say "  FAIL: $p"; done
    if [ "$ACT" = 1 ] || ! semver_ok "$V"; then
        say "release.sh: preflight failed; nothing was changed."
        exit 1
    fi
elif [ "$PHASE" = 1 ]; then
    say "  ok: preparing $V from origin/$PROD (at ${PROD_VERSION:-?}) and origin/main ($GH_REPO), clean, gh signed in, $V newer than every tag, signing works"
fi
# Merged already: the release is Deploy prod's, and a run says where.
if [ "${#PROBLEMS[@]}" = 0 ] && [ "$PHASE" = 2 ]; then
    released
    exit 0
fi
# Its pull request open already: the release waits for the owner, and a
# run says where, changing nothing.
if [ "${#PROBLEMS[@]}" = 0 ] && [ "$PHASE" = 1 ] && [ "$PR_STATE" = OPEN ]; then
    prepared
    exit 0
fi
PREFLIGHT_S=$(($(now) - T0))

if [ "$ACT" = 0 ]; then
    overview
    if [ "${#PROBLEMS[@]}" -gt 0 ]; then
        say "Preflight failed: fix the above first. Nothing was changed."
        exit 1
    fi
    say "A run with $ACT_FLAG starts at step $NEXT. Nothing was changed."
    exit 0
fi

mkdir -p "$ROOT/target/release"
LOGGING=1
HB_DIR="$(mktemp -d)"
say "== $(date -u +%Y-%m-%dT%H:%M:%SZ) release.sh $V in $ROOT; the log: $LOG"
overview
say "Starting at step $NEXT. Each step's output goes to the log$([ "$VERBOSE" = 1 ] && echo ", and here (--verbose)")."
printf '== step 1 of %d ran %ds\n' "$LAST" "$PREFLIGHT_S" >> "$LOG"
DONE_W="${WEIGHTS[1]}"
# The state preflight read holds until a step runs: a step found done
# changes nothing.
STALE=0
for n in $(seq 2 "$LAST"); do
    if [ "$STALE" = 1 ] && ! refresh; then
        say "release.sh: $REFRESH_ERR before step $n; run \`$RESUME\` again to resume."
        exit 1
    fi
    STEP="$n"
    if "done_$n" 2>/dev/null; then
        DONE_W=$((DONE_W + WEIGHTS[n]))
        say "[$(pct "$DONE_W")%] step $n/$LAST ${TITLES[n]}: done already"
        continue
    fi
    # Step 10 is the owner's: the run ends here until the merge.
    if [ "$n" = 10 ]; then
        prepared
        exit 0
    fi
    say "[$(pct "$DONE_W")%] step $n/$LAST ${TITLES[n]}: ${COMMANDS[n]}"
    STEP_START="$(now)"
    STALE=1
    start_ticker
    set +e
    if [ "$VERBOSE" = 1 ]; then
        (set -e; "step_$n")
    else
        (set -e; IN_LOG=1; "step_$n") >> "$LOG" 2>&1
    fi
    rc=$?
    set -e
    stop_ticker
    if [ "$rc" != 0 ]; then
        if [ "$VERBOSE" = 0 ]; then
            printf '\nThe last 50 lines of %s:\n' "$LOG"
            tail -n 50 "$LOG"
        fi
        say ""
        say "release.sh: step $n (${TITLES[n]}) failed."
        say "Fix the cause, then run \`$RESUME\` again: it resumes at the first step not done."
        say "The log: $LOG"
        exit 1
    fi
    printf '== step %d of %d ran %ds\n' "$n" "$LAST" $(($(now) - STEP_START)) >> "$LOG"
    DONE_W=$((DONE_W + WEIGHTS[n]))
done
