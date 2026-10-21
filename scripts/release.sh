#!/usr/bin/env bash
# Cuts a Clax release, from the version bump to the Pi package on npm: the
# procedure docs/usage.md ("Releasing") describes, in one command.
#
#   scripts/release.sh X.Y.Z                              a dry run
#   scripts/release.sh X.Y.Z --i-know-what-i-am-doing     the release
#   ... --verbose                                         streams every step's output
#
# The dry run, the default, runs the read-only checks and prints every step
# with whether it is done already; it changes nothing (it fetches origin's
# main, which moves only origin/main). With --i-know-what-i-am-doing it
# acts, as the checkout's git identity, signing with its signing key:
#
#    1  preflight: on main and clean; after a fetch, ahead of origin/main
#       only by this release's commits (the release commit, the pin commit,
#       merges of origin/main on top), and not behind it, except between the
#       release commit and step 12, which merges origin/main in; gh signed
#       in; X.Y.Z newer than every v* tag; commit signing works; the tools
#       present; npm reachable (and signed in, for a first Pi publish); the
#       npm environment limited to main (a dry run only warns). A failure
#       here changes nothing.
#    2  scripts/bump-version.sh X.Y.Z
#    3  just bazel-sync, until it changes nothing (at most 4 runs)
#    4  just ci
#    5  a signed commit "Release X.Y.Z" (git commit -S -a)
#    6  a signed tag vX.Y.Z, "Clax X.Y.Z"
#    7  git push origin refs/tags/vX.Y.Z (the tag alone)
#    8  wait for the Release workflow on that tag, then check that the GitHub
#       release lists SHA256SUMS and was built from the tag commit's tree
#       (scripts/release-exists-check.sh --tree)
#    9  scripts/pin-release.sh vX.Y.Z
#   10  just ci
#   11  a signed commit "Pin the plugins to Clax X.Y.Z"
#   12  git push origin main; when origin's main has moved on since, first a
#       signed merge of origin/main on top of the pin commit
#   13  wait for CI on the commit step 12 pushed (the pin commit, or the
#       merge on it)
#   14  gh workflow run publish-pi.yml (dry_run off) on that commit, and
#       wait for it. While the Pi package does not exist on npm (trusted
#       publishing needs an existing package), its first version is
#       published from here instead: npm publish, from a temporary worktree
#       of that commit, after the checks publish-pi.yml makes; then the
#       trusted-publisher setup is printed. Done once npm has the version.
#   15  print what is left by hand (the clax.dev deploy, crates.io, and after
#       a first Pi publish its trusted-publisher setup and the README switch)
#
# A run that fails stops at that step, says which, and changes nothing after
# it. Run the same command again: each step's state is read from git and
# GitHub and npm (the version files, the commits by subject, the tag here
# and on origin, the release's assets, the workflow runs, the versions npm
# has), so the run resumes at
# the first step not done. The release is known by its tag and its pin
# commit, so it resumes also after main has moved past the pin commit (a
# failed CI wait or Pi publish), from a checkout pulled up to origin/main.
# Uncommitted changes are taken only while a step that makes them is
# pending its commit (2-4 into the release commit, 9-10 into the pin
# commit); any other change, an untracked file, or a commit on main other
# than the release's own, refuses to run. It never force-pushes, never deletes a
# tag, and checks that every commit and the tag it pushes carry a signature.
#
# Before the first step it lists every step, done or to do, with an
# estimate of the time in all and still to run: the steps' durations in
# earlier runs' logs (target/release/*.log), averaged, or else typical
# ones (just ci 7m30s, the Release workflow 11m30s, CI on main 9m, the Pi
# publish 2m). Each step then prints one line, with the share of the
# estimate done ([42%]; steps found done count in full), and a step that
# runs longer prints a heartbeat about every 30 seconds: the time it has
# taken, the time left, and what is cheap to tell (Bazel's tests done out
# of the total, the workflow run's state and jobs done). Everything a step
# prints goes to target/release/X.Y.Z.log only; a failed step's last 50
# lines are printed with the log's path. With --verbose the steps' output
# streams to the terminal too, as well as to the log, and there is no
# heartbeat. The npm publish of a first Pi version and what is left by hand
# always reach the terminal.
#
# It works from any directory: the checkout is the one holding this script.
#
# Environment:
#   GH_REPO                     the GitHub repository (default: origin's)
#   CLAX_RELEASE_NPM            the npm command (npm); for tests
#   CLAX_RELEASE_PNPM           the pnpm command (pnpm); for tests
#   CLAX_RELEASE_POLL_SECONDS   seconds between polls of a workflow run (20)
#   CLAX_RELEASE_MAX_POLLS      polls before a wait gives up (270, 90 min)
#   CLAX_RELEASE_HEARTBEAT_SECONDS
#                               seconds between heartbeats (30; 0: none)
#   CLAX_RELEASE_NOW_FILE       a file holding the time, in seconds since
#                               the epoch, read in place of the clock; for
#                               tests
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
POLL="${CLAX_RELEASE_POLL_SECONDS:-20}"
MAX_POLLS="${CLAX_RELEASE_MAX_POLLS:-270}"
ACT_FLAG="--i-know-what-i-am-doing"
NPM="${CLAX_RELEASE_NPM:-npm}"
# The GitHub environment publish-pi.yml's job runs in, which npm's trusted
# publisher names and the repository limits to main.
NPM_ENV=npm
PNPM="${CLAX_RELEASE_PNPM:-pnpm}"

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
REL_SUBJECT="Release $V"
PIN_SUBJECT="Pin the plugins to Clax $V"
MERGE_SUBJECT="Merge origin/main into the Clax $V release"
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
# of the durations earlier runs' logs recorded ("== step N ran Ss").
WEIGHTS=([1]=10 [2]=5 [3]=30 [4]=450 [5]=5 [6]=5 [7]=10 [8]=690 [9]=5 [10]=450 [11]=5 [12]=15 [13]=540 [14]=120 [15]=5)
LEARNED_FROM=0
learn_weights() {
    local f n s
    local -a logs=()
    for f in "$ROOT"/target/release/*.log; do [ -f "$f" ] && logs+=("$f"); done
    [ "${#logs[@]}" -gt 0 ] || return 0
    while read -r n s; do
        WEIGHTS[n]="$s"
        LEARNED_FROM="${#logs[@]}"
    done < <(awk '$1 == "==" && $2 == "step" && $4 == "ran" && $3 ~ /^[0-9]+$/ && $5 ~ /^[0-9]+s$/ {
            sum[$3] += $5 + 0; c[$3]++ }
        END { for (n in c) { a = int(sum[n] / c[n] + 0.5); print n, (a < 1 ? 1 : a) } }' "${logs[@]}")
}
TOTAL_W=0
total_weight() {
    local n t=0
    for n in $(seq 1 15); do t=$((t + WEIGHTS[n])); done
    echo "$t"
}
pct() { echo $(($1 * 100 / TOTAL_W)); }
# The weight of the steps done, and the step under way: STEP, its start
# STEP_START, the log's size then STEP_OFFSET.
DONE_W=0
STEP=1
STEP_START=0
STEP_OFFSET=0
# HB_DIR holds what a step says of itself for the heartbeat: detail (one
# line), run (the workflow run it waits for), pause (no heartbeat now).
HB_DIR=""
TICKER=""
detail() { if [ -n "$HB_DIR" ]; then printf '%s\n' "$*" > "$HB_DIR/detail"; fi; }
# What a heartbeat can tell of the step under way.
step_detail() {
    local d="" tests id jobs
    if [ -s "$HB_DIR/detail" ]; then d="$(cat "$HB_DIR/detail")"; fi
    # Bazel's progress line ("[1,234 / 2,345] 40 / 56 tests; ..."), in what
    # this step logged.
    tests="$(tail -c "+$((STEP_OFFSET + 1))" "$LOG" 2>/dev/null | tail -c 200000 | tr '\r' '\n' \
        | grep -oE '[0-9,]+ / [0-9,]+ tests' | tail -n 1 | tr -d ', ' | sed 's/tests$/ tests/')" || tests=""
    if [ -n "$tests" ]; then d="${d:+$d, }$tests"; fi
    if [ -s "$HB_DIR/run" ]; then
        id="$(cat "$HB_DIR/run")"
        jobs="$(gh run view "$id" --json jobs \
            --jq '"\([.jobs[] | select(.status == "completed")] | length)/\(.jobs | length) jobs done"' 2>/dev/null)" || jobs=""
        if [ -n "$jobs" ]; then d="${d:+$d, }$jobs"; fi
    fi
    printf '%s' "$d"
}
# One heartbeat line: "[42%] step 4/15 Run the gates: 40/56 tests, 3m12s
# (about 18m left)". The step counts for no more than its weight.
heartbeat() {
    local e cur d left p
    e=$(($(now) - STEP_START))
    cur="$e"
    [ "$cur" -le "${WEIGHTS[STEP]}" ] || cur="${WEIGHTS[STEP]}"
    left=$((TOTAL_W - DONE_W - cur))
    p="$(pct $((DONE_W + cur)))"
    [ "$p" -le 99 ] || p=99
    d="$(step_detail)"
    printf '[%d%%] step %d/15 %s: %s%s (%s left)\n' "$p" "$STEP" "${TITLES[STEP]}" "${d:+$d, }" "$(dur "$e")" "$(about "$left")"
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
# The state of ci.yml run $1's gate, its `bazel` job, as run_state prints
# it: the run's own conclusion also counts the perf-log job, which waits in
# a queue of its own and is cancelled when pushes pile up.
ci_gate_state() {
    gh run view "$1" --json jobs --jq '.jobs[] | select(.name == "bazel") | "\(.status) \(.conclusion)"' | sed -n 1p
}
release_exists() { gh release view "$TAG" --json tagName >/dev/null 2>&1; }
# The Pi package's npm name, from plugins/pi/package.json.
pi_package() { sed -n 's/^  "name": "\(.*\)",$/\1/p' "$ROOT/plugins/pi/package.json"; }
# Whether the Pi package exists on npm: prints yes or no; fails, saying
# npm's error, when npm cannot tell.
pi_on_npm() {
    local out
    if out="$("$NPM" view "$(pi_package)" version 2>&1)"; then echo yes; return 0; fi
    case "$out" in
        *E404* | *"404 Not Found"*) echo no ;;
        *) echo "$out" | tr '\n' ' ' >&2; return 1 ;;
    esac
}
# Whether npm has the Pi package at version $1 (default: this release's).
pi_has_version() {
    local out
    out="$("$NPM" view "$(pi_package)@${1:-$V}" version 2>/dev/null)" || return 1
    [ "$out" = "${1:-$V}" ]
}
# The tree the release was built from, by the rule the Release workflow
# also uses (scripts/release-exists-check.sh): its SOURCE_TREE asset, or for
# an older release the run that uploaded it. Nothing when there is none.
release_tree() { bash "$ROOT/scripts/release-exists-check.sh" --tree "$TAG"; }
# Whether the release's files are this tag's build: it lists SHA256SUMS and
# was built from the tag commit's tree. Trees, not commits: a tag moved to a
# re-signed copy of its commit names another commit with the same tree,
# and the Release workflow then keeps the release it already made.
release_of_tree() {
    local tree
    release_has_sums || return 1
    tree="$(release_tree)" || return 1
    [ -n "$tree" ] && [ "$tree" = "$(g rev-parse "$REL_COMMIT^{tree}")" ]
}
# Says what to do about a release of this version built from another tree
# than this tag's; $1 is the run on the tag.
stale_release() {
    local tree
    tree="$(release_tree 2>/dev/null)" || tree=""
    say "  the release $TAG holds a build of ${tree:-an unknown tree}, not of this tag's tree"
    say "  ($(g rev-parse "$REL_COMMIT^{tree}")): its files are another build's. Delete it, keeping the tag"
    say "  (gh release delete $TAG --yes --repo $GH_REPO), rerun the run (gh run rerun $1"
    say "  --repo $GH_REPO), then run this again."
}

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
    # The commit step 12 pushes, which CI and the Pi package are run on: the
    # pin commit, or the newest merge of origin/main on top of it.
    PUBLISH_COMMIT="$PIN_COMMIT"
    if [ -n "$PIN_COMMIT" ]; then
        local ref=HEAD merge
        if g merge-base --is-ancestor "$PIN_COMMIT" origin/main; then ref=origin/main; fi
        merge="$(g log --ancestry-path --format='%H%x09%s' "$PIN_COMMIT..$ref" -- \
            | awk -F '\t' -v s="$MERGE_SUBJECT" '$2 == s { print $1 }' | sed -n 1p)"
        PUBLISH_COMMIT="${merge:-$PIN_COMMIT}"
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
    elif [ -n "$TAG_COMMIT" ] && [ -z "$PIN_COMMIT" ] && g merge-base --is-ancestor "$TAG_COMMIT" origin/main; then
        # Step 12 pushes the release commit to main together with its pin.
        problem "$TAG's release commit is on origin/main without a \"$PIN_SUBJECT\" commit after it"
    fi
}

check_tree() {
    local branch behind i dirty untracked subject
    branch="$(g symbolic-ref --short -q HEAD || true)"
    [ "$branch" = main ] || problem "the checkout is on ${branch:-a detached HEAD}, not main"
    [ "$FETCHED" = 1 ] || return 0
    behind="$(g rev-list --count HEAD..origin/main)"
    # Behind origin/main is a release's to merge once its release commit is
    # made and until step 12 pushes it.
    if [ "$behind" != 0 ] && { [ -z "$REL_COMMIT" ] || g merge-base --is-ancestor "$REL_COMMIT" origin/main; }; then
        problem "main is $behind commit(s) behind origin/main (git pull --ff-only)"
    fi
    # Ahead only by this release's own commits, in order: the release
    # commit, the pin commit, and merges of origin/main on top of it.
    i=0
    while IFS= read -r subject; do
        [ -n "$subject" ] || continue
        case "$i:$subject" in
            "0:$REL_SUBJECT") i=1 ;;
            "1:$PIN_SUBJECT" | "2:$MERGE_SUBJECT") i=2 ;;
            *)
                problem "the checkout has a commit origin/main lacks that is not this release's: \"$subject\""
                break ;;
        esac
    done < <(g log --reverse --topo-order --format=%s origin/main..HEAD)
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
    check_npm
    check_npm_environment
    check_signing
}

NOTES=()
# The Pi package on npm. Trusted publishing (publish-pi.yml) needs a package
# that exists, so while it does not, step 14 publishes the first version
# from here, which needs npm signed in and pnpm for the typecheck.
# The npm environment, read-only: it must exist and allow only main, or a
# run dispatched on another ref could get npm's token. Creating it is a
# repository setting the owner makes; this only says how. A dry run warns,
# a release refuses, unless npm has the version already.
check_npm_environment() {
    local err policy branches why=""
    semver_ok "$V" || return 0
    command -v gh >/dev/null 2>&1 && [ -n "${GH_REPO:-}" ] || return 0
    if command -v "$NPM" >/dev/null 2>&1 && pi_has_version; then return 0; fi
    err="$(mktemp)"
    if ! policy="$(gh api "repos/$GH_REPO/environments/$NPM_ENV" \
        --jq '.deployment_branch_policy | if . == null then "none" else "\(.protected_branches) \(.custom_branch_policies)" end' 2>"$err")"; then
        case "$(cat "$err")" in
            *404* | *"Not Found"*) why="the repository has no environment $NPM_ENV" ;;
            *) why="cannot read the environment $NPM_ENV: $(tr '\n' ' ' < "$err")" ;;
        esac
    elif [ "$policy" != "false true" ]; then
        why="the environment $NPM_ENV does not limit its deployment branches to main"
    elif ! branches="$(gh api "repos/$GH_REPO/environments/$NPM_ENV/deployment-branch-policies" \
        --jq '[.branch_policies[] | "\(.type // "branch"):\(.name)"] | join(",")' 2>"$err")"; then
        why="cannot read the environment $NPM_ENV's deployment branches: $(tr '\n' ' ' < "$err")"
    elif [ "$branches" != "branch:main" ]; then
        why="the environment $NPM_ENV allows ${branches:-no branch}, not main alone"
    fi
    rm -f "$err"
    [ -n "$why" ] || return 0
    why="$why. publish-pi.yml runs in it; in GitHub, the repository's Settings, Environments, New environment (or $NPM_ENV), name $NPM_ENV; Deployment branches and tags: Selected branches and tags; Add deployment branch or tag rule: branch main"
    if [ "$ACT" = 1 ]; then problem "$why"; else NOTES+=("WARN: $why"); fi
}

check_npm() {
    local on err
    if ! command -v "$NPM" >/dev/null 2>&1; then problem "npm is not on PATH"; return 0; fi
    semver_ok "$V" || return 0
    err="$(mktemp)"
    if ! on="$(pi_on_npm 2>"$err")"; then
        problem "cannot ask npm about $(pi_package): $(cat "$err")"
        rm -f "$err"
        return 0
    fi
    rm -f "$err"
    if [ "$on" = no ]; then
        if ! "$NPM" whoami >/dev/null 2>&1; then
            problem "$(pi_package) is not on npm yet, so step 14 publishes its first version from here: sign in first (npm login; npm whoami fails)"
        fi
        command -v "$PNPM" >/dev/null 2>&1 || problem "pnpm is not on PATH (the first Pi publish typechecks the package)"
        NOTES+=("$(pi_package) is not on npm yet: step 14 publishes its first version from this machine (npm publish, with npm's own prompts), then prints the trusted-publisher setup")
    elif pi_has_version; then
        NOTES+=("$(pi_package)@$V is on npm already: step 14 is done")
    fi
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
    [12]="git push origin main (after git merge -S origin/main, when origin's main moved on)"
    [13]="gh run list --workflow ci.yml (the push run on the pin commit, or the merge on it), wait for its bazel job"
    [14]="gh workflow run publish-pi.yml --ref main -f dry_run=false -f sha=<that commit>, wait (the first version: npm publish from here)"
    [15]="print the steps left by hand"
)

# done_N: whether step N is done, from the state refresh read.
done_2() { [ "$CUR" = "$V" ]; }
done_3() { [ -n "$REL_COMMIT" ]; }
done_4() { [ -n "$REL_COMMIT" ]; }
done_5() { [ -n "$REL_COMMIT" ]; }
done_6() { [ -n "$LOCAL_TAG" ] || [ -n "$REMOTE_TAG" ]; }
done_7() { [ -n "$REMOTE_TAG" ]; }
# Done when the release is the build of this tag's tree.
done_8() { release_of_tree; }
done_9() { [ "$PINNED" = "$V" ]; }
done_10() { [ -n "$PIN_COMMIT" ]; }
done_11() { [ -n "$PIN_COMMIT" ]; }
done_12() { [ -n "$PUBLISH_COMMIT" ] && g merge-base --is-ancestor "$PUBLISH_COMMIT" origin/main; }
done_13() {
    local id
    done_14 && return 0
    id="$(find_run ci.yml push "$PUBLISH_COMMIT")"
    [ -n "$id" ] && [ "$(ci_gate_state "$id")" = "completed success" ]
}
done_14() { pi_has_version; }
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

# Runs "$@" until it succeeds, at most GH_RETRIES times, POLL seconds apart;
# prints its output, or says why it gave up.
gh_retry() {
    local rc
    POLL_FAILS=0
    while :; do
        rc=0
        poll_once "$@" || rc=$?
        case "$rc" in
            0) printf '%s\n' "$POLL_OUT"; return 0 ;;
            2) return 1 ;;
        esac
        sleep "$POLL"
    done
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
# succeeded. $3 reads the state (default run_state).
await_run() {
    local id="$1" what="$2" read="${3:-run_state}" state last="" polls=0 rc
    say "  $what: run $id (gh run watch $id --repo $GH_REPO)"
    detail "run $id"
    if [ -n "$HB_DIR" ]; then printf '%s\n' "$id" > "$HB_DIR/run"; fi
    POLL_FAILS=0
    while :; do
        rc=0
        poll_once "$read" "$id" || rc=$?
        [ "$rc" != 2 ] || return 1
        state=""
        [ "$rc" != 0 ] || state="$POLL_OUT"
        if [ "$rc" = 0 ] && [ "$state" != "$last" ]; then
            say "  $what: ${state% }"
            detail "run $id ${state% }"
            last="$state"
        fi
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
        detail "run $n"
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
    detail "waiting for the run to start"
    id="$(await_output find_run release.yml push "$REL_COMMIT")" \
        || { say "  no Release workflow run on $TAG appeared"; return 1; }
    if ! await_run "$id" "Release $TAG"; then
        if release_exists && ! release_of_tree; then stale_release "$id"; fi
        return 1
    fi
    release_has_sums || { say "  the release $TAG does not list SHA256SUMS (gh release view $TAG)"; return 1; }
    release_of_tree || { stale_release "$id"; return 1; }
}
step_9() { bash "$ROOT/scripts/pin-release.sh" "$TAG"; }
step_10() { step_4; }
step_11() {
    say "  committing:"
    g status --short | sed 's/^/    /'
    g commit -S -a -q -m "$PIN_SUBJECT"
    is_signed_commit HEAD || { say "  the commit carries no signature"; return 1; }
}
# Pushes main. When origin's main has moved on since the release began, it
# is merged on top of the pin commit first, signed: the tag stays on the
# release commit, which a rebase would rewrite.
step_12() {
    local c
    g fetch --quiet --no-tags origin main
    if ! g merge-base --is-ancestor origin/main HEAD; then
        tell "  origin/main has moved on; merging it: $MERGE_SUBJECT"
        if ! g merge -S --no-ff -q -m "$MERGE_SUBJECT" origin/main; then
            g merge --abort 2>/dev/null || true
            say "  the merge does not apply cleanly. Merge by hand, signed and with this subject:"
            say "    git merge -S origin/main -m \"$MERGE_SUBJECT\""
            say "  resolve, then commit it signed (git commit -S; git does not carry -S past a conflict),"
            say "  then run this again."
            return 1
        fi
    fi
    for c in $(g rev-list origin/main..HEAD); do
        is_signed_commit "$c" && continue
        say "  $c carries no signature; not pushing main."
        if [ "$c" = "$(g rev-parse HEAD)" ]; then
            say "  Sign it (git commit --amend -S --no-edit), then run this again."
        else
            say "  It is not the newest commit; signing it rewrites what follows, so sign it by hand."
        fi
        return 1
    done
    if ! g push origin refs/heads/main:refs/heads/main; then
        say "  the push of main was refused; if origin's main moved on meanwhile, run this again:"
        say "  it merges origin/main on top of the pin commit and pushes again."
        return 1
    fi
    g fetch --quiet --no-tags origin main
}
step_13() {
    local id
    say "  waiting for CI's bazel job on $PUBLISH_COMMIT (polls every ${POLL}s, at most $MAX_POLLS; not perf-log)"
    detail "waiting for the run to start"
    id="$(await_output find_run ci.yml push "$PUBLISH_COMMIT")" || { say "  no CI run on $PUBLISH_COMMIT appeared"; return 1; }
    await_run "$id" "CI on main (bazel)" ci_gate_state
}
# The newest publish-pi.yml run that publishes (not a dry run) from commit
# $1, by the title the workflow gives it, or nothing.
publish_run() {
    gh run list --workflow publish-pi.yml --event workflow_dispatch --limit 50 \
        --json databaseId,displayTitle --jq '.[] | "\(.databaseId)\t\(.displayTitle)"' \
        | awk -F '\t' -v t="Publish Pi package $1" '$2 == t { print $1 }' | sed -n 1p
}
# That run, when it is not run $2 (the newest before a dispatch).
new_publish_run() {
    local id
    id="$(publish_run "$1")" || return 1
    if [ "$id" != "$2" ]; then echo "$id"; fi
}
# Publishes from the commit step 12 pushed, never main's tip: a run already under way
# for it is waited for; none, or a failed one, is dispatched anew.
step_14() {
    local id state on
    on="$(pi_on_npm)" || { say "  cannot ask npm about $(pi_package)"; return 1; }
    if [ "$on" = no ]; then pi_first_publish; return; fi
    id="$(gh_retry publish_run "$PUBLISH_COMMIT")" || return 1
    state=""
    if [ -n "$id" ]; then state="$(gh_retry run_state "$id")" || return 1; fi
    case "$state" in
        "" | completed\ *)
            gh workflow run publish-pi.yml --ref main -f dry_run=false -f "sha=$PUBLISH_COMMIT"
            detail "dispatched; waiting for the run to start"
            id="$(await_output new_publish_run "$PUBLISH_COMMIT" "$id")" \
                || { say "  no Publish Pi package run for $PUBLISH_COMMIT appeared"; return 1; } ;;
    esac
    await_run "$id" "Publish Pi package"
}

# The Pi package's first version, published from here: from a temporary
# worktree of PUBLISH_COMMIT, after the checks publish-pi.yml makes (the
# wrapper pins the package's version, npm lacks it, the typecheck passes);
# npm publish runs on this terminal, so npm's sign-in and one-time-password
# prompts reach the person.
pi_first_publish() {
    local dir="$ROOT/target/release/$V/pi-publish" rc=0
    say "  $(pi_package) is not on npm yet: publishing its first version from here"
    if [ -e "$dir" ]; then g worktree remove --force "$dir" 2>/dev/null || rm -rf "$dir"; fi
    g worktree prune
    mkdir -p "$(dirname "$dir")"
    g worktree add --quiet --detach "$dir" "$PUBLISH_COMMIT"
    set +e
    (set -e; pi_publish_from "$dir/plugins/pi")
    rc=$?
    set -e
    g worktree remove --force "$dir"
    [ "$rc" = 0 ] || return "$rc"
    tell "  $(pi_package)@$V is on npm. Later releases publish through publish-pi.yml, which uses npm"
    tell "  trusted publishing; set it up once, now: on npmjs.com, the package's Settings, Trusted"
    tell "  Publisher, GitHub Actions: organization ${GH_REPO%%/*}, repository ${GH_REPO#*/}, workflow"
    tell "  publish-pi.yml, environment $NPM_ENV."
}
pi_publish_from() {
    local pkg="$1" version pinned
    cd "$pkg"
    version="$(sed -n 's/^  "version": "\(.*\)",$/\1/p' package.json)"
    pinned="$(sed -n 's/^PINNED_VERSION="\(.*\)"$/\1/p' scripts/ensure-clax.sh)"
    [ "$version" = "$V" ] || { say "  plugins/pi/package.json is at $version, not $V"; return 1; }
    [ "$pinned" = "$version" ] || { say "  plugins/pi/scripts/ensure-clax.sh pins $pinned, not $version"; return 1; }
    if pi_has_version "$version"; then say "  $(pi_package)@$version is on npm already"; return 1; fi
    say "  typecheck: $PNPM install --frozen-lockfile && $PNPM run typecheck"
    logged "$PNPM" install --frozen-lockfile
    logged "$PNPM" run typecheck
    # On the terminal, for npm's prompts, with no heartbeat meanwhile.
    tell "  $NPM publish --access public (plugins/pi at $PUBLISH_COMMIT)"
    if [ -n "$HB_DIR" ]; then : > "$HB_DIR/pause"; fi
    "$NPM" publish --access public >&3 2>&4
    if [ -n "$HB_DIR" ]; then rm -f "$HB_DIR/pause"; fi
    # Kept as soon as npm took the package, so step 15 prints the
    # trusted-publisher setup even if the wait below gives up.
    : > "$ROOT/target/release/$V/pi-first-publish"
    POLL_FAILS=0
    await_output pi_npm_version "$version" >/dev/null \
        || { say "  npm does not list $(pi_package)@$version after the publish"; return 1; }
}
# Prints version $1 once npm lists it.
pi_npm_version() { if pi_has_version "$1"; then echo "$1"; fi; }
step_15() {
    local prev t migrations=""
    prev=""
    for t in $(g tag -l 'v*'); do
        [[ $t =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || continue
        [ "$(ver_cmp "${t#v}" "$V")" = -1 ] || continue
        if [ -z "$prev" ] || [ "$(ver_cmp "${t#v}" "${prev#v}")" = 1 ]; then prev="$t"; fi
    done
    tell "Left by hand:"
    if [ -n "$prev" ] && g rev-parse -q --verify "refs/tags/$TAG" >/dev/null; then
        if g diff --quiet "$prev" "$TAG" -- crates/clax-store/src/migrations.rs; then
            migrations="no new migrations since $prev"
        else
            migrations="crates/clax-store/src/migrations.rs changed since $prev"
        fi
        tell "  - redeploy clax.dev with \`just clax-dev-deploy\` (a dry run; then $ACT_FLAG) if the"
        tell "    release has migrations or hosted changes ($migrations)"
    else
        tell "  - redeploy clax.dev with \`just clax-dev-deploy\` (a dry run; then $ACT_FLAG) if the"
        tell "    release has migrations or hosted changes"
    fi
    if [ -e "$ROOT/target/release/$V/pi-first-publish" ]; then
        tell "  - once: set up npm trusted publishing for $(pi_package) (npmjs.com, the package's Settings,"
        tell "    Trusted Publisher, GitHub Actions: organization ${GH_REPO%%/*}, repository ${GH_REPO#*/},"
        tell "    workflow publish-pi.yml, environment $NPM_ENV), so later releases publish through the workflow"
        tell "  - switch README.md and plugins/pi/README.md to lead their Pi install with"
        tell "    \`pi install npm:$(pi_package)\`"
    fi
    tell "  - crates.io is not published by this script (docs/usage.md, \"The crates.io package\")"
    tell "  - the release: https://github.com/${GH_REPO:-empathic/clax}/releases/tag/$TAG"
}

# --- main -------------------------------------------------------------------------

# Lists steps 2-15, done or to do, with the estimate of the time they take;
# NEXT is the first step to do.
overview() {
    local n left=0 from
    say ""
    say "The steps (checkout at ${CUR:-?}, plugins pinned to ${PINNED:-?}):"
    NEXT=""
    for n in $(seq 2 15); do
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
    say "Estimated: $(about "$TOTAL_W") in all, $(about "$left") for the steps to do ($from)."
    say ""
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
else
    say "  ok: on main and clean against origin/main ($GH_REPO), gh signed in, $V newer than every tag, signing works"
fi
for note in ${NOTES[@]+"${NOTES[@]}"}; do say "  note: $note"; done
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
printf '== step 1 ran %ds\n' "$PREFLIGHT_S" >> "$LOG"
DONE_W="${WEIGHTS[1]}"
RUN_START="$(now)"
for n in $(seq 2 15); do
    if ! refresh; then
        say "release.sh: $REFRESH_ERR before step $n; run \`$RESUME\` again to resume."
        exit 1
    fi
    STEP="$n"
    if "done_$n" 2>/dev/null; then
        DONE_W=$((DONE_W + WEIGHTS[n]))
        say "[$(pct "$DONE_W")%] step $n/15 ${TITLES[n]}: done already"
        continue
    fi
    say "[$(pct "$DONE_W")%] step $n/15 ${TITLES[n]}: ${COMMANDS[n]}"
    STEP_START="$(now)"
    STEP_OFFSET="$(wc -c < "$LOG" | tr -d ' ')"
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
    printf '== step %d ran %ds\n' "$n" $(($(now) - STEP_START)) >> "$LOG"
    DONE_W=$((DONE_W + WEIGHTS[n]))
done
say ""
say "[100%] Clax $V is released. This run took $(dur $(($(now) - RUN_START + PREFLIGHT_S)))."
