# shellcheck shell=bash
# Sourced, not run: what scripts/release.sh and scripts/release-sdk.sh
# share. Versions, commit signing, the repository's npm environment, the
# packages on npm, GitHub's workflow runs and the waits for them, and a
# publish through a workflow.
#
# The script that sources it sets ROOT (its checkout), NPM (the npm
# command), NPM_ENV (the GitHub environment the publish workflows run in),
# POLL (seconds between polls) and MAX_POLLS (polls before a wait gives
# up), and GH_REPO, or calls
# resolve_repo for it. It defines g (git in ROOT), say (prints a line),
# problem (records a preflight failure) and detail (what a heartbeat may
# tell of the step under way, or nothing). await_run also writes the run it
# waits for to $HB_DIR/run while HB_DIR names a directory.

# --- versions --------------------------------------------------------------------

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

# --- the repository ---------------------------------------------------------------

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
            g commit-tree -S -p HEAD -m "release signing check" "HEAD^{tree}" 2>"$scratch.err")" \
        && body="$(GIT_OBJECT_DIRECTORY="$scratch" GIT_ALTERNATE_OBJECT_DIRECTORIES="$objects" g cat-file commit "$sha")" \
        && grep -E '^gpgsig(-sha256)? ' >/dev/null <<< "$body"; then
        ok=1
    fi
    if [ "$ok" = 0 ]; then
        problem "commit signing does not work here (git commit -S): $(tr '\n' ' ' < "$scratch.err")"
    fi
    rm -rf "$scratch" "$scratch.err"
}

# The branches the npm environment may allow: prod, which publish-pi.yml is
# dispatched on, and main, which publish-sdk.yml is.
NPM_ENV_BRANCHES="main prod"

# npm_environment_problem BRANCH WHAT: the npm environment, read-only: it
# must exist, allow BRANCH, the branch the caller's publish workflow is
# dispatched on, and allow no branch but those in NPM_ENV_BRANCHES, or a
# run dispatched on another ref could get npm's token. Prints what is wrong
# with it, then WHAT (which workflows run in it) and how to set it up, or
# nothing when it is right. Creating it is a repository setting the owner
# makes; this only says how.
npm_environment_problem() {
    local branch="$1" what="$2" err policy branches b why=""
    err="$(mktemp)"
    if ! policy="$(gh api "repos/$GH_REPO/environments/$NPM_ENV" \
        --jq '.deployment_branch_policy | if . == null then "none" else "\(.protected_branches) \(.custom_branch_policies)" end' 2>"$err")"; then
        case "$(cat "$err")" in
            *404* | *"Not Found"*) why="the repository has no environment $NPM_ENV" ;;
            *) why="cannot read the environment $NPM_ENV: $(tr '\n' ' ' < "$err")" ;;
        esac
    elif [ "$policy" != "false true" ]; then
        why="the environment $NPM_ENV does not limit its deployment branches to ${NPM_ENV_BRANCHES// / and }"
    elif ! branches="$(gh api "repos/$GH_REPO/environments/$NPM_ENV/deployment-branch-policies" \
        --jq '[.branch_policies[] | "\(.type // "branch"):\(.name)"] | join(",")' 2>"$err")"; then
        why="cannot read the environment $NPM_ENV's deployment branches: $(tr '\n' ' ' < "$err")"
    else
        case ",$branches," in
            *",branch:$branch,"*) ;;
            *) why="the environment $NPM_ENV allows ${branches:-no branch}, not $branch" ;;
        esac
        for b in ${branches//,/ }; do
            case " $NPM_ENV_BRANCHES " in
                *" ${b#branch:} "*) [ "${b%%:*}" = branch ] && continue ;;
            esac
            why="${why:+$why; }the environment $NPM_ENV allows $b, which is not one of ${NPM_ENV_BRANCHES// / and }"
        done
    fi
    rm -f "$err"
    [ -n "$why" ] || return 0
    echo "$why. $what; in GitHub, the repository's Settings, Environments, New environment (or $NPM_ENV), name $NPM_ENV; Deployment branches and tags: Selected branches and tags; Add deployment branch or tag rule: branch $branch (scripts/setup-prod-github.sh allows main and prod)"
}

# --- npm ----------------------------------------------------------------------------

# Whether package $1 exists on npm: prints yes or no; fails, saying npm's
# error, when npm cannot tell.
on_npm() {
    local out
    if out="$("$NPM" view "$1" version 2>&1)"; then echo yes; return 0; fi
    case "$out" in
        *E404* | *"404 Not Found"*) echo no ;;
        *) echo "$out" | tr '\n' ' ' >&2; return 1 ;;
    esac
}
# Whether npm has package $1 at version $2.
has_version() {
    local out
    out="$("$NPM" view "$1@$2" version 2>/dev/null)" || return 1
    [ "$out" = "$2" ]
}

# --- workflow runs ------------------------------------------------------------------

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
    if [ -n "${HB_DIR:-}" ]; then printf '%s\n' "$id" > "$HB_DIR/run"; fi
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

# --- a publish through a workflow ----------------------------------------------------

# The newest run of publish workflow $1, whose runs are titled "$2 <commit>",
# that publishes (not a dry run) from commit $3, or nothing.
publish_run() {
    gh run list --workflow "$1" --event workflow_dispatch --limit 50 \
        --json databaseId,displayTitle --jq '.[] | "\(.databaseId)\t\(.displayTitle)"' \
        | awk -F '\t' -v t="$2 $3" '$2 == t { print $1 }' | sed -n 1p
}
# That run, when it is not run $4 (the newest before a dispatch).
new_publish_run() {
    local id
    id="$(publish_run "$1" "$2" "$3")" || return 1
    if [ "$id" != "$4" ]; then echo "$id"; fi
}
# publish_through WORKFLOW TITLE COMMIT BRANCH: publishes from COMMIT
# through WORKFLOW (dispatched on BRANCH with dry_run off), whose runs are
# titled "TITLE <commit>": a run already under way for COMMIT is waited
# for; one that succeeded is done (it published, though npm's view may not
# list the version yet: the caller, which found npm without it, waits for
# npm); none, or one that failed, is dispatched anew.
publish_through() {
    local workflow="$1" title="$2" commit="$3" branch="$4" id state
    id="$(gh_retry publish_run "$workflow" "$title" "$commit")" || return 1
    state=""
    if [ -n "$id" ]; then state="$(gh_retry run_state "$id")" || return 1; fi
    case "$state" in
        "completed success")
            say "  $title: run $id succeeded already"
            return 0 ;;
        "" | completed\ *)
            gh workflow run "$workflow" --ref "$branch" -f dry_run=false -f "sha=$commit"
            detail "dispatched; waiting for the run to start"
            id="$(await_output new_publish_run "$workflow" "$title" "$commit" "$id")" \
                || { say "  no $title run for $commit appeared"; return 1; } ;;
    esac
    await_run "$id" "$title"
}
