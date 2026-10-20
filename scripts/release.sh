#!/usr/bin/env bash
# Cuts a Clax release, from the version bump to the Pi package on npm: the
# procedure docs/usage.md ("Releasing") describes, in one command.
#
#   scripts/release.sh X.Y.Z                              a dry run
#   scripts/release.sh X.Y.Z --i-know-what-i-am-doing     the release
#
# The dry run, the default, runs the read-only checks and prints every step
# with whether it is done already; it changes nothing (it fetches origin's
# main, which moves only origin/main). With --i-know-what-i-am-doing it
# acts, as the checkout's git identity, signing with its signing key:
#
#    1  preflight: on main, clean, equal to origin/main after a fetch; gh
#       signed in; X.Y.Z newer than every v* tag; commit signing works; the
#       tools present. A failure here changes nothing.
#    2  scripts/bump-version.sh X.Y.Z
#    3  just bazel-sync, until it changes nothing (at most 4 runs)
#    4  just ci
#    5  a signed commit "Release X.Y.Z" (git commit -S -a)
#    6  a signed tag vX.Y.Z, "Clax X.Y.Z"
#    7  git push origin refs/tags/vX.Y.Z (the tag alone)
#    8  wait for the Release workflow on that tag, then check that the GitHub
#       release lists SHA256SUMS
#    9  scripts/pin-release.sh vX.Y.Z
#   10  just ci
#   11  a signed commit "Pin the plugins to Clax X.Y.Z"
#   12  git push origin main
#   13  wait for CI on the pin commit
#   14  gh workflow run publish-pi.yml (dry_run off), and wait for it
#   15  print what is left by hand (clax.dev, crates.io)
#
# A run that fails stops at that step, says which, and changes nothing after
# it. Run the same command again: each step's state is read from git and
# GitHub (the version files, the two commits by subject, the tag here and on
# origin, the release's assets, the workflow runs), so the run resumes at
# the first step not done. The one thing not on GitHub, the publish-pi run
# this script started, is kept in target/release/X.Y.Z/publish-pi-run. A
# tag of X.Y.Z is resumed only while the release is under way: its commit
# not yet on origin/main, or the pin commit origin/main's tip.
# Uncommitted changes are taken only while a step that makes them is
# pending its commit (2-4 into the release commit, 9-10 into the pin
# commit); any other change, an untracked file, or a commit on main other
# than these two, refuses to run. It never force-pushes, never deletes a
# tag, and checks that every commit and the tag it pushes carry a signature.
# The output of the long steps (just, the waits) is also appended to
# target/release/X.Y.Z.log.
#
# It works from any directory: the checkout is the one holding this script.
#
# Environment:
#   GH_REPO                     the GitHub repository (default: origin's)
#   CLAX_RELEASE_POLL_SECONDS   seconds between polls of a workflow run (20)
#   CLAX_RELEASE_MAX_POLLS      polls before a wait gives up (270, 90 min)
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
POLL="${CLAX_RELEASE_POLL_SECONDS:-20}"
MAX_POLLS="${CLAX_RELEASE_MAX_POLLS:-270}"
ACT_FLAG="--i-know-what-i-am-doing"

usage() {
    echo "usage: scripts/release.sh X.Y.Z [$ACT_FLAG]" >&2
    exit 2
}
V=""
ACT=0
for arg in "$@"; do
    case "$arg" in
        "$ACT_FLAG") ACT=1 ;;
        -h|--help) sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed '$d; s/^# \{0,1\}//'; exit 0 ;;
        -*) usage ;;
        *) [ -z "$V" ] || usage; V="$arg" ;;
    esac
done
[ -n "$V" ] || usage
TAG="v$V"
REL_SUBJECT="Release $V"
PIN_SUBJECT="Pin the plugins to Clax $V"
STATE_DIR="$ROOT/target/release/$V"
LOG="$ROOT/target/release/$V.log"
RESUME="scripts/release.sh $V $ACT_FLAG"
# 1 once the run acts: from then on, say and logged append to LOG.
LOGGING=0

g() { git -C "$ROOT" "$@"; }
say() {
    printf '%s\n' "$*"
    if [ "$LOGGING" = 1 ]; then printf '%s\n' "$*" >> "$LOG"; fi
}
# Runs a command, its output shown and appended to the log.
logged() {
    "$@" 2>&1 | tee -a "$LOG"
}

# --- state, read from git and GitHub -------------------------------------------

semver_ok() { [[ $1 =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]]; }
# ver_cmp A B: prints -1, 0 or 1 for the X.Y.Z cores of A and B.
ver_cmp() {
    local a b i
    IFS=. read -r -a a <<< "${1%%-*}"
    IFS=. read -r -a b <<< "${2%%-*}"
    for i in 0 1 2; do
        if [ "$((10#${a[i]}))" -gt "$((10#${b[i]}))" ]; then echo 1; return; fi
        if [ "$((10#${a[i]}))" -lt "$((10#${b[i]}))" ]; then echo -1; return; fi
    done
    echo 0
}
checkout_version() { bash "$ROOT/scripts/check-version.sh" --print 2>/dev/null; }
pinned_version() { sed -n 's/^PINNED_VERSION="\(.*\)"$/\1/p' "$ROOT/scripts/ensure-clax.sh"; }
# The oldest commit in revision range $2 whose subject is exactly $1.
subject_in() {
    g log --format='%H%x09%s' --fixed-strings --grep="$1" "$2" -- | awk -F '\t' -v s="$1" '$2 == s { h = $1 } END { print h }'
}
local_tag_commit() { g rev-parse -q --verify "refs/tags/$TAG^{commit}" 2>/dev/null || true; }
# The commit origin's tag of this version tags, or nothing; fails when
# origin cannot be read.
remote_tag_commit() {
    local out
    out="$(g ls-remote origin "refs/tags/$TAG" "refs/tags/$TAG^{}")" || return 1
    awk -v t="refs/tags/$TAG" '$2 == t "^{}" { p = $1 } $2 == t { q = $1 } END { print (p != "" ? p : q) }' <<< "$out"
}
is_signed_commit() { g cat-file commit "$1" | grep -E '^gpgsig(-sha256)? ' >/dev/null; }
is_signed_tag() { g cat-file tag "$TAG" | grep -E -- '^-----BEGIN [A-Z ]*SIGNATURE-----$' >/dev/null; }
release_has_sums() {
    gh release view "$TAG" --json assets --jq '.assets[].name' 2>/dev/null | grep -x SHA256SUMS >/dev/null
}
# The newest run of workflow $1 for event $2 on commit $3, or nothing.
find_run() {
    gh run list --workflow "$1" --event "$2" --commit "$3" --limit 20 \
        --json databaseId,headSha --jq '.[] | "\(.databaseId) \(.headSha)"' \
        | awk -v s="$3" '$2 == s { print $1 }' | sed -n 1p
}
run_state() { gh run view "$1" --json status,conclusion --jq '"\(.status) \(.conclusion)"'; }

# Reads the state the steps are judged by; fails, saying why in
# REFRESH_ERR, when origin cannot be read.
refresh() {
    CUR="$(checkout_version || true)"
    PINNED="$(pinned_version)"
    LOCAL_TAG="$(local_tag_commit)"
    REMOTE_TAG="$(remote_tag_commit)" || { REFRESH_ERR="cannot read origin's tags (git ls-remote origin)"; return 1; }
    # The release commit is the one the tag names, once there is a tag;
    # before, only a "Release X.Y.Z" commit this checkout has on top of
    # origin/main, never one from older history (preflight checks both).
    TAG_COMMIT="${LOCAL_TAG:-$REMOTE_TAG}"
    if [ -n "$TAG_COMMIT" ]; then
        REL_COMMIT="$TAG_COMMIT"
    else
        REL_COMMIT="$(subject_in "$REL_SUBJECT" origin/main..HEAD)"
    fi
    PIN_COMMIT=""
    if [ -n "$REL_COMMIT" ]; then
        PIN_COMMIT="$(subject_in "$PIN_SUBJECT" "$REL_COMMIT..HEAD")"
        [ -n "$PIN_COMMIT" ] || PIN_COMMIT="$(subject_in "$PIN_SUBJECT" "$REL_COMMIT..origin/main")"
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
        || problem "neither bazelisk nor bazel is on PATH (just ci and just bazel-sync run Bazel)"
}

resolve_repo() {
    local url
    if [ -n "${GH_REPO:-}" ]; then return 0; fi
    url="$(g remote get-url origin 2>/dev/null || true)"
    case "$url" in
        git@github.com:*) GH_REPO="${url#git@github.com:}" ;;
        https://github.com/*) GH_REPO="${url#https://github.com/}" ;;
        ssh://git@github.com/*) GH_REPO="${url#ssh://git@github.com/}" ;;
        *) problem "origin ($url) is not a GitHub repository; set GH_REPO=owner/name"; return 0 ;;
    esac
    GH_REPO="${GH_REPO%.git}"
    export GH_REPO
}

# Commit signing, as git commit -S would sign: a commit object written to a
# scratch object directory, removed after, so the repository is untouched.
check_signing() {
    local scratch objects sha body ok=0
    scratch="$(mktemp -d)"
    objects="$(g rev-parse --path-format=absolute --git-common-dir)/objects"
    if sha="$(GIT_OBJECT_DIRECTORY="$scratch" GIT_ALTERNATE_OBJECT_DIRECTORIES="$objects" \
            g commit-tree -S -p HEAD -m "release.sh signing check" "HEAD^{tree}" 2>"$scratch.err")" \
        && body="$(GIT_OBJECT_DIRECTORY="$scratch" GIT_ALTERNATE_OBJECT_DIRECTORIES="$objects" g cat-file commit "$sha")" \
        && grep -E '^gpgsig(-sha256)? ' >/dev/null <<< "$body"; then
        ok=1
    fi
    if [ "$ok" = 0 ]; then
        problem "commit signing does not work here (git commit -S): $(tr '\n' ' ' < "$scratch.err")"
    fi
    rm -rf "$scratch" "$scratch.err"
}

check_version() {
    local remote tags t old
    if ! semver_ok "$V"; then
        problem "$V is not a release version (X.Y.Z)"
        return 0
    fi
    if [ -z "$CUR" ]; then
        problem "the checkout's versions disagree (scripts/check-version.sh)"
    elif [ "$(ver_cmp "$CUR" "$V")" = 1 ]; then
        problem "the checkout is at $CUR, newer than $V"
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
    # A tag of this version is this release's only when it tags a
    # "Release X.Y.Z" commit main has, here or on origin, and both tags agree.
    # Without a tag, a "Release X.Y.Z" commit already on origin/main is not
    # this release's to tag.
    if [ -n "$LOCAL_TAG" ] && [ -n "$REMOTE_TAG" ] && [ "$LOCAL_TAG" != "$REMOTE_TAG" ]; then
        problem "the tag $TAG tags $LOCAL_TAG here and $REMOTE_TAG on origin"
    elif [ -n "$TAG_COMMIT" ] && [ "$(g log -1 --format=%s "$TAG_COMMIT" 2>/dev/null)" != "$REL_SUBJECT" ]; then
        problem "the tag $TAG tags $TAG_COMMIT, not a \"$REL_SUBJECT\" commit"
    elif [ -n "$TAG_COMMIT" ] && ! g merge-base --is-ancestor "$TAG_COMMIT" HEAD \
        && ! g merge-base --is-ancestor "$TAG_COMMIT" origin/main; then
        problem "the tag $TAG tags $TAG_COMMIT, which neither this checkout nor origin/main has"
    elif [ -z "$TAG_COMMIT" ] && old="$(subject_in "$REL_SUBJECT" origin/main)" && [ -n "$old" ]; then
        problem "origin/main has a \"$REL_SUBJECT\" commit already ($old) and no tag $TAG names it"
    elif [ -n "$LOCAL_TAG$REMOTE_TAG" ] \
        && g merge-base --is-ancestor "$REL_COMMIT" origin/main \
        && { [ -z "$PIN_COMMIT" ] || [ "$PIN_COMMIT" != "$(g rev-parse origin/main)" ]; }; then
        problem "$V is not newer than the tag $TAG, a release finished or taken over by later commits on main"
    fi
}

check_tree() {
    local branch behind expected i dirty untracked subject
    branch="$(g symbolic-ref --short -q HEAD || true)"
    [ "$branch" = main ] || problem "the checkout is on ${branch:-a detached HEAD}, not main"
    [ "$FETCHED" = 1 ] || return 0
    behind="$(g rev-list --count HEAD..origin/main)"
    [ "$behind" = 0 ] || problem "main is $behind commit(s) behind origin/main (git pull --ff-only)"
    # Ahead only by this release's own commits, in order.
    expected=("$REL_SUBJECT" "$PIN_SUBJECT")
    i=0
    while IFS= read -r subject; do
        [ -n "$subject" ] || continue
        if [ "$i" -ge 2 ] || [ "$subject" != "${expected[i]}" ]; then
            problem "the checkout has a commit origin/main lacks that is not this release's: \"$subject\""
            break
        fi
        i=$((i + 1))
    done < <(g log --reverse --format=%s origin/main..HEAD)
    untracked="$(g ls-files --others --exclude-standard "$ROOT")"
    [ -z "$untracked" ] || problem "untracked files: $(echo "$untracked" | tr '\n' ' ')"
    dirty="$(g diff --name-only HEAD)"
    if [ -n "$dirty" ]; then
        # Changes are this release's while a step that makes them waits for
        # its commit: the bump (2-4), the pin (9-10).
        if { [ "$CUR" = "$V" ] && [ -z "$REL_COMMIT" ]; } \
            || { [ "$PINNED" = "$V" ] && [ -n "$REL_COMMIT" ] && [ -z "$PIN_COMMIT" ]; }; then
            :
        else
            problem "uncommitted changes: $(echo "$dirty" | tr '\n' ' ')"
        fi
    fi
}

preflight() {
    check_tools
    command -v git >/dev/null 2>&1 || return 0
    resolve_repo
    FETCHED=0
    if g fetch --quiet --no-tags origin main; then FETCHED=1; else problem "cannot fetch main from origin"; fi
    refresh || problem "$REFRESH_ERR"
    check_tree
    check_version
    if command -v gh >/dev/null 2>&1 && ! gh auth status >/dev/null 2>&1; then
        problem "gh is not signed in (gh auth login)"
    fi
    check_signing
}

# --- steps 2-15 -------------------------------------------------------------------

TITLES=(
    [2]="Bump the version"
    [3]="Sync Bazel with Cargo"
    [4]="Run the gates"
    [5]="Commit the release"
    [6]="Tag the release"
    [7]="Push the tag"
    [8]="Wait for the Release workflow"
    [9]="Pin the plugins to the release"
    [10]="Run the gates"
    [11]="Commit the pin"
    [12]="Push main"
    [13]="Wait for CI on main"
    [14]="Publish the Pi package"
    [15]="What is left"
)
COMMANDS=(
    [2]="scripts/bump-version.sh $V"
    [3]="just bazel-sync, until it changes nothing (at most 4 runs)"
    [4]="just ci"
    [5]="git commit -S -a -m \"$REL_SUBJECT\""
    [6]="git tag -s $TAG -m \"Clax $V\""
    [7]="git push origin refs/tags/$TAG"
    [8]="gh run list --workflow release.yml (the run on $TAG), wait; gh release view $TAG lists SHA256SUMS"
    [9]="scripts/pin-release.sh $TAG"
    [10]="just ci"
    [11]="git commit -S -a -m \"$PIN_SUBJECT\""
    [12]="git push origin main"
    [13]="gh run list --workflow ci.yml (the push run on the pin commit), wait"
    [14]="gh workflow run publish-pi.yml --ref main -f dry_run=false, wait"
    [15]="print the steps left by hand"
)

publish_run_file="$STATE_DIR/publish-pi-run"

# done_N: whether step N is done, from the state refresh read.
done_2() { [ "$CUR" = "$V" ]; }
done_3() { [ -n "$REL_COMMIT" ]; }
done_4() { [ -n "$REL_COMMIT" ]; }
done_5() { [ -n "$REL_COMMIT" ]; }
done_6() { [ -n "$LOCAL_TAG" ] || [ -n "$REMOTE_TAG" ]; }
done_7() { [ -n "$REMOTE_TAG" ]; }
done_8() { release_has_sums; }
done_9() { [ "$PINNED" = "$V" ]; }
done_10() { [ -n "$PIN_COMMIT" ]; }
done_11() { [ -n "$PIN_COMMIT" ]; }
done_12() { [ -n "$PIN_COMMIT" ] && g merge-base --is-ancestor "$PIN_COMMIT" origin/main; }
done_13() {
    local id
    done_14 && return 0
    id="$(find_run ci.yml push "$PIN_COMMIT")"
    [ -n "$id" ] && [ "$(run_state "$id")" = "completed success" ]
}
done_14() { [ -s "$publish_run_file" ] && [ "$(run_state "$(cat "$publish_run_file")")" = "completed success" ]; }
done_15() { false; }

# A poll's gh call may fail this many times in a row (a network or API
# hiccup) before the wait gives up.
GH_RETRIES=5

# poll_once CMD...: runs CMD, its output in POLL_OUT. A failure counts in
# POLL_FAILS and returns 1; GH_RETRIES failures in a row return 2, after
# saying so (on stderr, as callers capture stdout) with gh's error.
POLL_FAILS=0
poll_once() {
    local err
    err="$(mktemp)"
    if POLL_OUT="$("$@" 2>"$err")"; then
        POLL_FAILS=0
        rm -f "$err"
        return 0
    fi
    POLL_FAILS=$((POLL_FAILS + 1))
    if [ "$POLL_FAILS" -ge "$GH_RETRIES" ]; then
        say "  gh failed $POLL_FAILS times in a row ($*): $(tr '\n' ' ' < "$err")" >&2
        rm -f "$err"
        return 2
    fi
    rm -f "$err"
    return 1
}

# Polls until "$@" prints something, at most MAX_POLLS times; a failed call
# counts as an empty poll. Says why it gave up, on stderr.
await_output() {
    local polls=0 rc
    POLL_FAILS=0
    while :; do
        rc=0
        poll_once "$@" || rc=$?
        [ "$rc" != 2 ] || return 1
        if [ "$rc" = 0 ] && [ -n "$POLL_OUT" ]; then echo "$POLL_OUT"; return 0; fi
        polls=$((polls + 1))
        if [ "$polls" -ge "$MAX_POLLS" ]; then
            if [ "$rc" = 0 ]; then say "  nothing after $polls polls" >&2
            else say "  nothing after $polls polls; the last gh call failed ($*)" >&2; fi
            return 1
        fi
        sleep "$POLL"
    done
}

# Waits for workflow run $1 ($2 names it) to complete; fails unless it
# succeeded.
await_run() {
    local id="$1" what="$2" state last="" polls=0 rc
    say "  $what: run $id (gh run watch $id --repo $GH_REPO)"
    POLL_FAILS=0
    while :; do
        rc=0
        poll_once run_state "$id" || rc=$?
        [ "$rc" != 2 ] || return 1
        state=""
        [ "$rc" != 0 ] || state="$POLL_OUT"
        if [ "$rc" = 0 ] && [ "$state" != "$last" ]; then say "  $what: ${state% }"; last="$state"; fi
        case "$state" in
            "completed success") return 0 ;;
            completed\ *)
                say "  $what concluded ${state#completed }: see gh run view $id --repo $GH_REPO --log-failed;"
                say "  once it is fixed, rerun it (gh run rerun $id --failed --repo $GH_REPO)"
                return 1 ;;
        esac
        polls=$((polls + 1))
        if [ "$polls" -ge "$MAX_POLLS" ]; then
            say "  gave up waiting for run $id after $polls polls; it may still be running"
            return 1
        fi
        sleep "$POLL"
    done
}

worktree_digest() { { g diff HEAD; g status --porcelain=v1 -uall; } | cksum; }

step_2() { bash "$ROOT/scripts/bump-version.sh" "$V"; }
step_3() {
    local before after n
    for n in 1 2 3 4; do
        before="$(worktree_digest)"
        say "  just bazel-sync (run $n)"
        (cd "$ROOT" && logged just bazel-sync)
        after="$(worktree_digest)"
        if [ "$before" = "$after" ]; then return 0; fi
    done
    say "  just bazel-sync still changed files on its 4th run"
    return 1
}
step_4() { (cd "$ROOT" && logged just ci); }
step_5() {
    say "  committing:"
    g status --short | sed 's/^/    /'
    g commit -S -a -q -m "$REL_SUBJECT"
    is_signed_commit HEAD || { say "  the commit carries no signature"; return 1; }
}
step_6() {
    g tag -s "$TAG" -m "Clax $V" "$REL_COMMIT"
    is_signed_tag || { say "  the tag $TAG carries no signature"; return 1; }
}
step_7() {
    is_signed_tag || { say "  the tag $TAG carries no signature; not pushing it"; return 1; }
    is_signed_commit "$REL_COMMIT" || { say "  $REL_COMMIT carries no signature; not pushing it"; return 1; }
    g push origin "refs/tags/$TAG"
}
step_8() {
    local id
    say "  waiting for the Release workflow's run on $TAG (polls every ${POLL}s, at most $MAX_POLLS)"
    id="$(await_output find_run release.yml push "$REL_COMMIT")" \
        || { say "  no Release workflow run on $TAG appeared"; return 1; }
    await_run "$id" "Release $TAG" || return 1
    release_has_sums || { say "  the release $TAG does not list SHA256SUMS (gh release view $TAG)"; return 1; }
}
step_9() { bash "$ROOT/scripts/pin-release.sh" "$TAG"; }
step_10() { step_4; }
step_11() {
    say "  committing:"
    g status --short | sed 's/^/    /'
    g commit -S -a -q -m "$PIN_SUBJECT"
    is_signed_commit HEAD || { say "  the commit carries no signature"; return 1; }
}
step_12() {
    local c
    for c in $(g rev-list origin/main..HEAD); do
        is_signed_commit "$c" || { say "  $c carries no signature; not pushing main"; return 1; }
    done
    g push origin refs/heads/main:refs/heads/main
    g fetch --quiet --no-tags origin main
}
step_13() {
    local id
    say "  waiting for CI on $PIN_COMMIT (polls every ${POLL}s, at most $MAX_POLLS)"
    id="$(await_output find_run ci.yml push "$PIN_COMMIT")" || { say "  no CI run on $PIN_COMMIT appeared"; return 1; }
    await_run "$id" "CI on main"
}
# The IDs of publish-pi.yml's dispatched runs on main, sorted.
publish_runs() {
    gh run list --workflow publish-pi.yml --event workflow_dispatch --branch main --limit 50 \
        --json databaseId --jq '.[].databaseId' | sort
}
# The first of them not in $1, the IDs before the dispatch.
new_publish_run() { publish_runs | comm -13 <(printf '%s\n' "$1") - | sed -n 1p; }
step_14() {
    local id before
    if [ -s "$publish_run_file" ]; then
        id="$(cat "$publish_run_file")"
    else
        before="$(publish_runs)"
        gh workflow run publish-pi.yml --ref main -f dry_run=false
        id="$(await_output new_publish_run "$before")" || { say "  the publish-pi run did not appear"; return 1; }
        mkdir -p "$STATE_DIR"
        echo "$id" > "$publish_run_file"
    fi
    await_run "$id" "Publish Pi package"
}
step_15() {
    local prev t migrations=""
    prev=""
    for t in $(g tag -l 'v*'); do
        [[ $t =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || continue
        [ "$(ver_cmp "${t#v}" "$V")" = -1 ] || continue
        if [ -z "$prev" ] || [ "$(ver_cmp "${t#v}" "${prev#v}")" = 1 ]; then prev="$t"; fi
    done
    say "Left by hand:"
    if [ -n "$prev" ] && g rev-parse -q --verify "refs/tags/$TAG" >/dev/null; then
        if g diff --quiet "$prev" "$TAG" -- crates/clax-store/src/migrations.rs; then
            migrations="no new migrations since $prev"
        else
            migrations="crates/clax-store/src/migrations.rs changed since $prev"
        fi
        say "  - redeploy clax.dev with \`just clax-dev-deploy\` (a dry run; then $ACT_FLAG) if the"
        say "    release has migrations or hosted changes ($migrations)"
    else
        say "  - redeploy clax.dev with \`just clax-dev-deploy\` (a dry run; then $ACT_FLAG) if the"
        say "    release has migrations or hosted changes"
    fi
    say "  - crates.io is not published by this script (docs/usage.md, \"The crates.io package\")"
    say "  - the release: https://github.com/${GH_REPO:-empathic/clax}/releases/tag/$TAG"
}

# --- main -------------------------------------------------------------------------

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
else
    say "  ok: on main and clean against origin/main ($GH_REPO), gh signed in, $V newer than every tag, signing works"
fi

if [ "$ACT" = 0 ]; then
    say ""
    say "The steps (checkout at ${CUR:-?}, plugins pinned to ${PINNED:-?}):"
    next=""
    for n in $(seq 2 15); do
        if [ -z "$next" ] && "done_$n"; then
            say "  $n. [done] ${TITLES[n]}: ${COMMANDS[n]}"
        else
            next="${next:-$n}"
            say "  $n. [todo] ${TITLES[n]}: ${COMMANDS[n]}"
        fi
    done
    say ""
    if [ "${#PROBLEMS[@]}" -gt 0 ]; then
        say "Preflight failed: fix the above first. Nothing was changed."
        exit 1
    fi
    say "A run with $ACT_FLAG starts at step $next. Nothing was changed."
    exit 0
fi

mkdir -p "$ROOT/target/release"
LOGGING=1
say "== $(date -u +%Y-%m-%dT%H:%M:%SZ) release.sh $V in $ROOT; the log: $LOG"
for n in $(seq 2 15); do
    if ! refresh; then
        say "release.sh: $REFRESH_ERR before step $n; run \`$RESUME\` again to resume."
        exit 1
    fi
    if "done_$n"; then
        say "Step $n: ${TITLES[n]}: done already"
        continue
    fi
    say "Step $n: ${TITLES[n]}: ${COMMANDS[n]}"
    set +e
    (set -e; "step_$n")
    rc=$?
    set -e
    if [ "$rc" != 0 ]; then
        say ""
        say "release.sh: step $n (${TITLES[n]}) failed."
        say "Fix the cause, then run \`$RESUME\` again: it resumes at the first step not done."
        say "The log: $LOG"
        exit 1
    fi
done
say ""
say "Clax $V is released."
