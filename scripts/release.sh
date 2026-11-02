#!/usr/bin/env bash
# Cuts a Clax release from the prod branch, from the release branch to the
# Pi package on npm: the procedure docs/usage.md ("Releasing") describes, in
# one command run twice, before and after the release's pull request into
# prod is merged.
#
#   scripts/release.sh X.Y.Z                              a dry run
#   scripts/release.sh X.Y.Z --i-know-what-i-am-doing     the release
#   ... --verbose                                         streams every step's output
#
# The dry run, the default, runs the read-only checks and prints every step
# with whether it is done already; it changes nothing (it fetches origin's
# main and prod, and the release branch when origin has it, which moves only
# the remote-tracking refs). With --i-know-what-i-am-doing it acts, as the
# checkout's git identity, signing with its signing key.
#
# Which phase a run is in follows from origin's prod: until prod's version
# is X.Y.Z, the release is prepared (steps 2-9) and its pull request waits
# for the owner (step 10); once it is, the release is cut (steps 11-21).
#
#    1  preflight. Always: gh signed in; X.Y.Z newer than every v* tag;
#       commit signing works; the tools present; npm reachable (and signed
#       in, for a first Pi publish); the npm environment limited to prod (a
#       dry run only warns). Preparing: origin/prod older than X.Y.Z; a clean
#       checkout on any branch, or on release/X.Y.Z ahead of origin/prod only
#       by merges of origin/main and the release commit; the release's pull
#       request not open (an open one refuses: merge it first). Cutting: on
#       prod, not behind origin/prod and ahead of it only by the pin commit;
#       origin/prod holds a "Release X.Y.Z" commit main lacks (the pull
#       request merged with a merge commit). A failure here changes nothing.
#    2  git switch -c release/X.Y.Z origin/prod (or to that branch, when it
#       exists here or on origin)
#    3  a signed merge of origin/main into it, "Merge origin/main into
#       release/X.Y.Z" (none when it has origin/main already)
#    4  scripts/bump-version.sh X.Y.Z
#    5  just bazel-sync, until it changes nothing (at most 4 runs)
#    6  just ci
#    7  a signed commit "Release X.Y.Z" (git commit -S -a)
#    8  git push origin release/X.Y.Z
#    9  gh pr create --base prod --head release/X.Y.Z; the run stops here,
#       saying to merge it
#   10  the owner merges the pull request on GitHub, with a merge commit;
#       the push to prod deploys clax.dev from GitHub Actions. Done once
#       origin's prod is at X.Y.Z; then the same command, run again from a
#       prod checkout at origin/prod's tip, goes on.
#   11  a signed tag vX.Y.Z, "Clax X.Y.Z", on origin/prod's tip
#   12  git push origin refs/tags/vX.Y.Z (the tag alone)
#   13  wait for the Release workflow on that tag, then check that the GitHub
#       release lists SHA256SUMS and was built from the tag commit's tree
#       (scripts/release-exists-check.sh --tree)
#   14  scripts/pin-release.sh vX.Y.Z
#   15  just ci
#   16  a signed commit "Pin the plugins to Clax X.Y.Z"
#   17  git push origin prod (the owner bypasses prod's pull request rule);
#       the push deploys clax.dev again, with no server change
#   18  carry the release to main: the release commit and the pin commit
#       cherry-picked (-S -x) onto origin/main's tip as two signed commits,
#       from a temporary worktree, and pushed to main, which takes no merge
#       commit. A conflict, or versions that disagree after the cherry-picks
#       (scripts/check-version.sh: main gained a version file since step 3),
#       stops the run, saying what to resolve.
#   19  wait for CI on the pin commit on prod
#   20  gh workflow run publish-pi.yml --ref prod on the pin commit, and
#       wait for it. While the Pi package does not exist on npm (trusted
#       publishing needs an existing package), its first version is
#       published from here instead: npm publish, from a temporary worktree
#       of the pin commit, after the checks publish-pi.yml makes; then the
#       trusted-publisher setup is printed. Done once npm has the version.
#   21  print what is left by hand (the deploy's runs to check, crates.io,
#       and after a first Pi publish its trusted-publisher setup and the
#       README switch)
#
# A run that fails stops at that step, says which, and changes nothing after
# it. Run the same command again: each step's state is read from git and
# GitHub and npm (the version files, the commits by subject, the release
# branch here and on origin, its pull request's state, the tag here and on
# origin, the release's assets, the workflow runs, the versions npm has),
# so the run resumes at the first step not done. Uncommitted changes are
# taken only while a step that makes them is pending its commit (4-6 into
# the release commit, 14-15 into the pin commit); any other change, an
# untracked file, or a commit other than the release's own, refuses to run.
# It never force-pushes, never deletes a tag, and checks that every commit
# and the tag it pushes carry a signature.
#
# Before the first step it lists every step, done or to do, with an
# estimate of the time in all and still to run: the steps' durations in
# earlier runs' logs (target/release/*.log), averaged, or else typical
# ones (just ci 7m30s, the Release workflow 11m30s, CI on prod 9m, the Pi
# publish 2m); the pull request's review is not counted. Each step then
# prints one line, with the share of the estimate done ([42%]; steps found
# done count in full), and a step that runs longer prints a heartbeat about
# every 30 seconds: the time it has taken, the time left, and what is cheap
# to tell (Bazel's tests done out of the total, the workflow run's state
# and jobs done). Everything a step prints goes to target/release/X.Y.Z.log
# only; a failed step's last 50 lines are printed with the log's path. With
# --verbose the steps' output streams to the terminal too, as well as to
# the log, and there is no heartbeat. The pull request's URL, the npm
# publish of a first Pi version and what is left by hand always reach the
# terminal.
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
# publisher names and the repository limits to prod.
NPM_ENV=npm
PNPM="${CLAX_RELEASE_PNPM:-pnpm}"
# The branch releases come from, and the last step.
PROD=prod
LAST=21

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
PIN_SUBJECT="Pin the plugins to Clax $V"
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
WEIGHTS=([1]=10 [2]=5 [3]=5 [4]=5 [5]=30 [6]=450 [7]=5 [8]=10 [9]=5 [10]=5 [11]=5 [12]=10
    [13]=690 [14]=5 [15]=450 [16]=5 [17]=10 [18]=20 [19]=540 [20]=120 [21]=5)
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
# One heartbeat line: "[42%] step 6/21 Run the gates: 40/56 tests, 3m12s
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
# The Clax version commit $1 carries (its scripts/ensure-clax.sh), or nothing.
version_at() { g show "$1:scripts/ensure-clax.sh" 2>/dev/null | sed -n 's/^CLAX_VERSION="\(.*\)"$/\1/p'; }
# The oldest commit in revision range $2 whose subject is exactly $1.
subject_in() {
    g log --format='%H%x09%s' --fixed-strings --grep="$1" "$2" -- 2>/dev/null | awk -F '\t' -v s="$1" '$2 == s { h = $1 } END { print h }'
}
has_ref() { g rev-parse -q --verify "$1^{commit}" >/dev/null 2>&1; }
local_tag_commit() { g rev-parse -q --verify "refs/tags/$TAG^{commit}" 2>/dev/null || true; }
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
# The newest pull request from the release branch into prod, as "STATE URL"
# (OPEN, CLOSED or MERGED), or nothing.
pr_state() {
    gh pr list --head "$RB" --base "$PROD" --state all --limit 1 --json state,url --jq '.[] | "\(.state) \(.url)"'
}
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
    [ -n "$tree" ] && [ "$tree" = "$(g rev-parse "$TAG_COMMIT^{tree}")" ]
}
# Says what to do about a release of this version built from another tree
# than this tag's; $1 is the run on the tag.
stale_release() {
    local tree
    tree="$(release_tree 2>/dev/null)" || tree=""
    say "  the release $TAG holds a build of ${tree:-an unknown tree}, not of this tag's tree"
    say "  ($(g rev-parse "$TAG_COMMIT^{tree}")): its files are another build's. Delete it, keeping the tag"
    say "  (gh release delete $TAG --yes --repo $GH_REPO), rerun the run (gh run rerun $1"
    say "  --repo $GH_REPO), then run this again."
}

CUR="" PINNED="" BRANCH="" PROD_TIP="" PROD_VERSION="" PHASE=1 LOCAL_TAG="" REMOTE_TAG="" REMOTE_RB=""
TAG_COMMIT="" RB_REF="" REL_COMMIT="" PIN_COMMIT="" PR_STATE="" PR_URL=""
# Reads the state the steps are judged by; fails, saying why in
# REFRESH_ERR, when origin or GitHub cannot be read.
refresh() {
    local out
    CUR="$(checkout_version || true)"
    PINNED="$(pinned_version)"
    BRANCH="$(g symbolic-ref --short -q HEAD || true)"
    PROD_TIP="$(g rev-parse -q --verify "refs/remotes/origin/$PROD^{commit}" 2>/dev/null || true)"
    PROD_VERSION=""
    if [ -n "$PROD_TIP" ]; then PROD_VERSION="$(version_at "$PROD_TIP")"; fi
    # Cutting once origin's prod carries this version; preparing until then.
    PHASE=1
    if [ -n "$PROD_VERSION" ] && [ "$PROD_VERSION" = "$V" ]; then PHASE=2; fi
    LOCAL_TAG="$(local_tag_commit)"
    out="$(g ls-remote origin "refs/tags/$TAG" "refs/tags/$TAG^{}" "refs/heads/$RB")" \
        || { REFRESH_ERR="cannot read origin (git ls-remote origin)"; return 1; }
    REMOTE_TAG="$(awk -v t="refs/tags/$TAG" '$2 == t "^{}" { p = $1 } $2 == t { q = $1 } END { print (p != "" ? p : q) }' <<< "$out")"
    REMOTE_RB="$(awk -v b="refs/heads/$RB" '$2 == b { print $1 }' <<< "$out")"
    TAG_COMMIT="${LOCAL_TAG:-$REMOTE_TAG}"
    # The release branch: here, or else origin's copy.
    RB_REF=""
    if has_ref "refs/heads/$RB"; then RB_REF="refs/heads/$RB"
    elif [ -n "$REMOTE_RB" ] && has_ref "refs/remotes/origin/$RB"; then RB_REF="refs/remotes/origin/$RB"; fi
    # The release commit: once cut, the "Release X.Y.Z" commit the tag (or
    # origin's prod) has that main lacks; before, the one on the release
    # branch after origin/prod.
    REL_COMMIT=""
    PIN_COMMIT=""
    if [ "$PHASE" = 2 ]; then
        REL_COMMIT="$(subject_in "$REL_SUBJECT" "origin/main..${TAG_COMMIT:-$PROD_TIP}")"
        if [ -n "$TAG_COMMIT" ]; then
            PIN_COMMIT="$(subject_in "$PIN_SUBJECT" "$TAG_COMMIT..HEAD")"
            [ -n "$PIN_COMMIT" ] || PIN_COMMIT="$(subject_in "$PIN_SUBJECT" "$TAG_COMMIT..$PROD_TIP")"
        fi
    elif [ -n "$RB_REF" ] && [ -n "$PROD_TIP" ]; then
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
    [ -n "$PROD_TIP" ] || return 0
    # A tag of this version is this release's only once origin's prod
    # carries the version: it tags prod's tip, and both tags agree.
    if [ -n "$LOCAL_TAG" ] && [ -n "$REMOTE_TAG" ] && [ "$LOCAL_TAG" != "$REMOTE_TAG" ]; then
        problem "the tag $TAG tags $LOCAL_TAG here and $REMOTE_TAG on origin"
    elif [ -n "$TAG_COMMIT" ] && [ "$PHASE" = 1 ]; then
        problem "the tag $TAG exists ($TAG_COMMIT), but origin/$PROD is at ${PROD_VERSION:-an unknown version}, not $V"
    elif [ -n "$TAG_COMMIT" ] && [ "$(version_at "$TAG_COMMIT")" != "$V" ]; then
        problem "the tag $TAG tags $TAG_COMMIT, whose version is $(version_at "$TAG_COMMIT"), not $V"
    elif [ -n "$TAG_COMMIT" ] && ! g merge-base --is-ancestor "$TAG_COMMIT" "$PROD_TIP" \
        && ! g merge-base --is-ancestor "$TAG_COMMIT" HEAD; then
        problem "the tag $TAG tags $TAG_COMMIT, which neither this checkout nor origin/$PROD has"
    fi
    if [ "$PHASE" = 1 ]; then
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
    else
        [ -n "$CUR" ] || problem "the checkout's versions disagree (scripts/check-version.sh)"
        if [ -z "$REL_COMMIT" ]; then
            problem "origin/$PROD has no \"$REL_SUBJECT\" commit that origin/main lacks: merge the release's pull request with a merge commit (a squash leaves none), so that step 18 can carry that commit to main"
        fi
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
    # The bump (4-6) on the release branch; the pin (14-15) on prod.
    if [ "$PHASE" = 1 ] && [ "$BRANCH" = "$RB" ] && [ "$CUR" = "$V" ] && [ -z "$REL_COMMIT" ]; then return 0; fi
    if [ "$PHASE" = 2 ] && [ "$BRANCH" = "$PROD" ] && [ "$PINNED" = "$V" ] && [ -n "$TAG_COMMIT" ] && [ -z "$PIN_COMMIT" ]; then return 0; fi
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
        OPEN) problem "the pull request $PR_URL into $PROD is not merged yet: once CI passes, merge it on GitHub (Create a merge commit), then run this again on a $PROD checkout at origin/$PROD's tip (git switch $PROD; git pull --ff-only)" ;;
        MERGED) problem "the pull request $PR_URL is merged, but origin/$PROD is at ${PROD_VERSION:-an unknown version}, not $V" ;;
    esac
}

# Cutting: on prod, at origin/prod's tip, or ahead of it by the pin commit.
check_cut() {
    local behind subjects
    if [ "$BRANCH" != "$PROD" ]; then
        problem "the checkout is on ${BRANCH:-a detached HEAD}, not $PROD: origin/$PROD is at $V, so the release is cut from $PROD (git switch $PROD; git pull --ff-only)"
        return 0
    fi
    behind="$(g rev-list --count "HEAD..$PROD_TIP")"
    [ "$behind" = 0 ] || problem "$PROD is $behind commit(s) behind origin/$PROD (git pull --ff-only)"
    subjects="$(g log --format=%s "$PROD_TIP..HEAD")"
    if [ -n "$subjects" ] && { [ "$subjects" != "$PIN_SUBJECT" ] || [ -z "$TAG_COMMIT" ]; }; then
        problem "the checkout has a commit origin/$PROD lacks that is not this release's: \"$(sed -n 1p <<< "$subjects")\""
    fi
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
    if [ -n "${PROD_TIP:-}" ]; then
        check_clean
        if [ "$PHASE" = 1 ]; then check_prepare; else check_cut; fi
    fi
    check_version
    if command -v gh >/dev/null 2>&1 && ! gh auth status >/dev/null 2>&1; then
        problem "gh is not signed in (gh auth login)"
    fi
    check_npm
    check_npm_environment
    check_signing
}

NOTES=()
# The npm environment, read-only: it must exist and allow only prod, or a
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
        why="the environment $NPM_ENV does not limit its deployment branches to $PROD"
    elif ! branches="$(gh api "repos/$GH_REPO/environments/$NPM_ENV/deployment-branch-policies" \
        --jq '[.branch_policies[] | "\(.type // "branch"):\(.name)"] | join(",")' 2>"$err")"; then
        why="cannot read the environment $NPM_ENV's deployment branches: $(tr '\n' ' ' < "$err")"
    elif [ "$branches" != "branch:$PROD" ]; then
        why="the environment $NPM_ENV allows ${branches:-no branch}, not $PROD alone"
    fi
    rm -f "$err"
    [ -n "$why" ] || return 0
    why="$why. publish-pi.yml runs in it; in GitHub, the repository's Settings, Environments, New environment (or $NPM_ENV), name $NPM_ENV; Deployment branches and tags: Selected branches and tags; Add deployment branch or tag rule: branch $PROD"
    if [ "$ACT" = 1 ]; then problem "$why"; else NOTES+=("WARN: $why"); fi
}

# The Pi package on npm. Trusted publishing (publish-pi.yml) needs a package
# that exists, so while it does not, step 20 publishes the first version
# from here, which needs npm signed in and pnpm for the typecheck.
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
            problem "$(pi_package) is not on npm yet, so step 20 publishes its first version from here: sign in first (npm login; npm whoami fails)"
        fi
        command -v "$PNPM" >/dev/null 2>&1 || problem "pnpm is not on PATH (the first Pi publish typechecks the package)"
        NOTES+=("$(pi_package) is not on npm yet: step 20 publishes its first version from this machine (npm publish, with npm's own prompts), then prints the trusted-publisher setup")
    elif pi_has_version; then
        NOTES+=("$(pi_package)@$V is on npm already: step 20 is done")
    fi
}

# --- steps 2-21 -------------------------------------------------------------------

TITLES=(
    [2]="Cut the release branch"
    [3]="Merge main into it"
    [4]="Bump the version"
    [5]="Sync Bazel with Cargo"
    [6]="Run the gates"
    [7]="Commit the release"
    [8]="Push the release branch"
    [9]="Open the pull request into $PROD"
    [10]="Merge the pull request (you, on GitHub)"
    [11]="Tag the release"
    [12]="Push the tag"
    [13]="Wait for the Release workflow"
    [14]="Pin the plugins to the release"
    [15]="Run the gates"
    [16]="Commit the pin"
    [17]="Push $PROD"
    [18]="Carry the release to main"
    [19]="Wait for CI on $PROD"
    [20]="Publish the Pi package"
    [21]="What is left"
)
COMMANDS=(
    [2]="git switch -c $RB origin/$PROD"
    [3]="git merge -S origin/main -m \"$MERGE_SUBJECT\""
    [4]="scripts/bump-version.sh $V"
    [5]="just bazel-sync, until it changes nothing (at most 4 runs)"
    [6]="just ci"
    [7]="git commit -S -a -m \"$REL_SUBJECT\""
    [8]="git push origin $RB"
    [9]="gh pr create --base $PROD --head $RB"
    [10]="once CI passes, merge it (Create a merge commit), which deploys clax.dev; then run this again on $PROD"
    [11]="git tag -s $TAG -m \"Clax $V\" (origin/$PROD's tip)"
    [12]="git push origin refs/tags/$TAG"
    [13]="gh run list --workflow release.yml (the run on $TAG), wait; gh release view $TAG lists SHA256SUMS"
    [14]="scripts/pin-release.sh $TAG"
    [15]="just ci"
    [16]="git commit -S -a -m \"$PIN_SUBJECT\""
    [17]="git push origin $PROD"
    [18]="git cherry-pick -S -x <the release commit> <the pin commit> onto origin/main; git push origin main"
    [19]="gh run list --workflow ci.yml (the push run on the pin commit), wait for its bazel job"
    [20]="gh workflow run publish-pi.yml --ref $PROD -f dry_run=false -f sha=<the pin commit>, wait (the first version: npm publish from here)"
    [21]="print the steps left by hand"
)

# done_N: whether step N is done, from the state refresh read. Once origin's
# prod is at this version, the whole preparation is.
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
done_11() { [ -n "$LOCAL_TAG" ] || [ -n "$REMOTE_TAG" ]; }
done_12() { [ -n "$REMOTE_TAG" ]; }
# Done when the release is the build of this tag's tree.
done_13() { release_of_tree; }
done_14() { [ "$PINNED" = "$V" ]; }
done_15() { [ -n "$PIN_COMMIT" ]; }
done_16() { [ -n "$PIN_COMMIT" ]; }
done_17() { [ -n "$PIN_COMMIT" ] && g merge-base --is-ancestor "$PIN_COMMIT" "$PROD_TIP"; }
# Done when origin's main has both commits, by subject: the cherry-picks
# are copies, or a carry made by hand.
done_18() { [ -n "$(subject_in "$REL_SUBJECT" origin/main)" ] && [ -n "$(subject_in "$PIN_SUBJECT" origin/main)" ]; }
done_19() {
    local id
    done_20 && return 0
    id="$(find_run ci.yml push "$PIN_COMMIT")"
    [ -n "$id" ] && [ "$(ci_gate_state "$id")" = "completed success" ]
}
done_20() { pi_has_version; }
done_21() { false; }

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
# version and pin (step 18), so the version files agree.
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
step_6() { (cd "$ROOT" && logged just ci); }
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

Merge with a merge commit (not squash or rebase) once CI passes: the push to \`$PROD\` deploys clax.dev. Then, on a \`$PROD\` checkout at \`origin/$PROD\`'s tip, \`just release $V $ACT_FLAG\` tags the release, pins the plugins to it, carries both commits to \`main\` and publishes the Pi package (docs/usage.md, "Releasing").
EOF
}
step_11() {
    g tag -s "$TAG" -m "Clax $V" "$PROD_TIP"
    is_signed_tag || { say "  the tag $TAG carries no signature"; return 1; }
}
step_12() {
    is_signed_tag || { say "  the tag $TAG carries no signature; not pushing it"; return 1; }
    is_signed_commit "$TAG_COMMIT" || { say "  $TAG_COMMIT carries no signature; not pushing a tag of it"; return 1; }
    g push origin "refs/tags/$TAG"
}
step_13() {
    local id
    say "  waiting for the Release workflow's run on $TAG (polls every ${POLL}s, at most $MAX_POLLS)"
    detail "waiting for the run to start"
    id="$(await_output find_run release.yml push "$TAG_COMMIT")" \
        || { say "  no Release workflow run on $TAG appeared"; return 1; }
    if ! await_run "$id" "Release $TAG"; then
        if release_exists && ! release_of_tree; then stale_release "$id"; fi
        return 1
    fi
    release_has_sums || { say "  the release $TAG does not list SHA256SUMS (gh release view $TAG)"; return 1; }
    release_of_tree || { stale_release "$id"; return 1; }
}
step_14() { bash "$ROOT/scripts/pin-release.sh" "$TAG"; }
step_15() { step_6; }
step_16() {
    say "  committing:"
    g status --short | sed 's/^/    /'
    g commit -S -a -q -m "$PIN_SUBJECT"
    is_signed_commit HEAD || { say "  the commit carries no signature"; return 1; }
}
# Pushes the pin commit to prod, straight (the owner bypasses the pull
# request rule), never forced.
step_17() {
    all_signed "$PROD_TIP..HEAD" || return 1
    if ! g push origin "refs/heads/$PROD:refs/heads/$PROD"; then
        say "  the push of $PROD was refused. If origin's $PROD moved on meanwhile, this release's"
        say "  tag is no longer its tip: see what landed (git log HEAD..origin/$PROD) before going on."
        return 1
    fi
    g fetch --quiet --no-tags origin "$PROD"
}
# Carries the release to main: the release commit and the pin commit, those
# main lacks, cherry-picked onto origin/main's tip in a temporary worktree,
# signed, and pushed. Main takes no merge commit, and gets the new version
# and its pin in one push.
step_18() {
    local dir="$ROOT/target/release/$V/carry" base head c files
    local -a picks=()
    g fetch --quiet --no-tags origin main
    [ -n "$(subject_in "$REL_SUBJECT" origin/main)" ] || picks+=("$REL_COMMIT")
    [ -n "$(subject_in "$PIN_SUBJECT" origin/main)" ] || picks+=("$PIN_COMMIT")
    [ "${#picks[@]}" -gt 0 ] || return 0
    base="$(g rev-parse origin/main)"
    if [ -e "$dir" ]; then g worktree remove --force "$dir" 2>/dev/null || rm -rf "$dir"; fi
    g worktree prune
    mkdir -p "$(dirname "$dir")"
    g worktree add --quiet --detach "$dir" "$base"
    for c in "${picks[@]}"; do
        say "  git cherry-pick -S -x $c (\"$(g log -1 --format=%s "$c")\")"
        if ! git -C "$dir" cherry-pick -S -x "$c"; then
            files="$(git -C "$dir" diff --name-only --diff-filter=U | tr '\n' ' ' | sed 's/ $//')"
            git -C "$dir" cherry-pick --abort 2>/dev/null || true
            g worktree remove --force "$dir"
            say "  cherry-picking $c (\"$(g log -1 --format=%s "$c")\") onto origin/main ($base)"
            say "  conflicts in: ${files:-(see the log)}. Nothing was pushed to main. Carry it by hand,"
            say "  keeping each commit's subject, each commit signed:"
            say "    git worktree add --detach target/release/$V/carry origin/main"
            say "    cd target/release/$V/carry && git cherry-pick -S -x ${picks[*]}"
            say "  resolve each conflict, git add it and git cherry-pick --continue; check that each"
            say "  commit is signed (git log --show-signature origin/main..HEAD; else git commit --amend -S"
            say "  --no-edit), then git push origin HEAD:refs/heads/main, remove the worktree"
            say "  (git worktree remove target/release/$V/carry) and run this again."
            return 1
        fi
    done
    # The bump was made against the release branch's merge of main: a version
    # file main gained since then still names the old version.
    if ! files="$(bash "$dir/scripts/check-version.sh" 2>&1)"; then
        g worktree remove --force "$dir"
        say "  after the cherry-picks onto origin/main ($base), main's versions disagree"
        say "  (scripts/check-version.sh), as main has gained version files since $RB merged it:"
        printf '%s\n' "$files" | sed 's/^/    /'
        say "  Nothing was pushed to main. Carry it by hand, keeping each commit's subject, each signed:"
        say "    git worktree add --detach target/release/$V/carry origin/main"
        say "    cd target/release/$V/carry && git cherry-pick -S -x ${picks[*]}"
        say "    scripts/bump-version.sh $V && just bazel-sync && scripts/check-version.sh"
        say "    git commit -S -a --amend --no-edit"
        say "  then git push origin HEAD:refs/heads/main, remove the worktree"
        say "  (git worktree remove target/release/$V/carry) and run this again."
        return 1
    fi
    head="$(git -C "$dir" rev-parse HEAD)"
    g worktree remove --force "$dir"
    all_signed "$base..$head" || return 1
    if ! g push origin "$head:refs/heads/main"; then
        say "  the push of main was refused; if origin's main moved on meanwhile, run this again:"
        say "  it cherry-picks onto origin/main's new tip."
        return 1
    fi
    g fetch --quiet --no-tags origin main
}
step_19() {
    local id
    say "  waiting for CI's bazel job on $PIN_COMMIT (polls every ${POLL}s, at most $MAX_POLLS; not perf-log)"
    detail "waiting for the run to start"
    id="$(await_output find_run ci.yml push "$PIN_COMMIT")" || { say "  no CI run on $PIN_COMMIT appeared"; return 1; }
    await_run "$id" "CI on $PROD (bazel)" ci_gate_state
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
# Publishes from the pin commit: a run already under way for it is waited
# for; none, or a failed one, is dispatched anew.
step_20() {
    local id state on
    on="$(pi_on_npm)" || { say "  cannot ask npm about $(pi_package)"; return 1; }
    if [ "$on" = no ]; then pi_first_publish; return; fi
    id="$(gh_retry publish_run "$PIN_COMMIT")" || return 1
    state=""
    if [ -n "$id" ]; then state="$(gh_retry run_state "$id")" || return 1; fi
    case "$state" in
        "" | completed\ *)
            gh workflow run publish-pi.yml --ref "$PROD" -f dry_run=false -f "sha=$PIN_COMMIT"
            detail "dispatched; waiting for the run to start"
            id="$(await_output new_publish_run "$PIN_COMMIT" "$id")" \
                || { say "  no Publish Pi package run for $PIN_COMMIT appeared"; return 1; } ;;
    esac
    await_run "$id" "Publish Pi package"
}

# The Pi package's first version, published from here: from a temporary
# worktree of PIN_COMMIT, after the checks publish-pi.yml makes (the
# wrapper pins the package's version, npm lacks it, the typecheck passes);
# npm publish runs on this terminal, so npm's sign-in and one-time-password
# prompts reach the person.
pi_first_publish() {
    local dir="$ROOT/target/release/$V/pi-publish" rc=0
    say "  $(pi_package) is not on npm yet: publishing its first version from here"
    if [ -e "$dir" ]; then g worktree remove --force "$dir" 2>/dev/null || rm -rf "$dir"; fi
    g worktree prune
    mkdir -p "$(dirname "$dir")"
    g worktree add --quiet --detach "$dir" "$PIN_COMMIT"
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
    tell "  $NPM publish --access public (plugins/pi at $PIN_COMMIT)"
    if [ -n "$HB_DIR" ]; then : > "$HB_DIR/pause"; fi
    "$NPM" publish --access public >&3 2>&4
    if [ -n "$HB_DIR" ]; then rm -f "$HB_DIR/pause"; fi
    # Kept as soon as npm took the package, so step 21 prints the
    # trusted-publisher setup even if the wait below gives up.
    : > "$ROOT/target/release/$V/pi-first-publish"
    POLL_FAILS=0
    await_output pi_npm_version "$version" >/dev/null \
        || { say "  npm does not list $(pi_package)@$version after the publish"; return 1; }
}
# Prints version $1 once npm lists it.
pi_npm_version() { if pi_has_version "$1"; then echo "$1"; fi; }
step_21() {
    tell "Left by hand:"
    tell "  - check that clax.dev deployed: each push to $PROD deploys it from GitHub Actions (the"
    tell "    merge, then the pin; docs/prod.md): gh run list --workflow deploy-prod.yml --repo ${GH_REPO:-empathic/clax}"
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

# Lists steps 2-21, done or to do, with the estimate of the time they take;
# NEXT is the first step to do.
overview() {
    local n left=0 from
    say ""
    say "The steps (checkout on ${BRANCH:-a detached HEAD} at ${CUR:-?}, plugins pinned to ${PINNED:-?}; origin/$PROD at ${PROD_VERSION:-?}):"
    NEXT=""
    for n in $(seq 2 "$LAST"); do
        if [ "$n" = 2 ]; then say "  Prepare, then the pull request into $PROD:"; fi
        if [ "$n" = 11 ]; then say "  Cut, once $PROD has it, from a $PROD checkout:"; fi
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
    say "The release is prepared: the pull request ${PR_URL:-from $RB} into $PROD."
    say "Once CI passes on it, merge it on GitHub with a merge commit (Create a merge commit, not"
    say "squash or rebase); the push to $PROD deploys clax.dev. Then, on a $PROD checkout at"
    say "origin/$PROD's tip (git switch $PROD; git pull --ff-only), run \`$RESUME\`"
    say "again: it tags the release, pins the plugins, carries both commits to main and publishes"
    say "the Pi package."
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
else
    say "  ok: cutting $V on $PROD at origin/$PROD ($GH_REPO), clean, gh signed in, $V newer than every tag, signing works"
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
printf '== step 1 of %d ran %ds\n' "$LAST" "$PREFLIGHT_S" >> "$LOG"
DONE_W="${WEIGHTS[1]}"
RUN_START="$(now)"
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
    printf '== step %d of %d ran %ds\n' "$n" "$LAST" $(($(now) - STEP_START)) >> "$LOG"
    DONE_W=$((DONE_W + WEIGHTS[n]))
done
say ""
say "[100%] Clax $V is released. This run took $(dur $(($(now) - RUN_START + PREFLIGHT_S)))."
