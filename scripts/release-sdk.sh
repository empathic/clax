#!/usr/bin/env bash
# Publishes the SDK package, @empathic/clax-client-sdk, to npm from main:
# the procedure docs/usage.md ("Releasing the SDK package") describes. The
# package has a version of its own, the version in web/sdk/package.json,
# not Clax's: a pull request to main raises it, and this publishes it from
# main, whether or not origin has a prod branch. A published SDK may
# therefore call report-queue routes that deployed instances (prod's tip)
# do not have yet.
#
#   scripts/release-sdk.sh X.Y.Z                            a dry run
#   scripts/release-sdk.sh X.Y.Z --i-know-what-i-am-doing   the publish
#
# The dry run, the default, runs the read-only checks and prints every step
# with whether it is done already; it changes nothing (it fetches origin's
# main, which moves only the remote-tracking ref). With
# --i-know-what-i-am-doing it acts, signing the tag with the checkout's
# signing key. It never changes the checkout's branch or work tree, so the
# checkout may be on any branch, in any state.
#
# The commit it publishes is the one the tag sdk-vX.Y.Z names, here or on
# origin, once the tag exists, and origin/main's tip until then.
#
#   1  preflight: gh signed in; git, gh and npm on PATH; X.Y.Z is a version
#      (X.Y.Z), the SDK version that commit carries, and newer than every
#      version npm has of the package (none counts as older); the tag, if
#      there is one, the same here and on origin and on origin/main; CI's
#      bazel job passed on the commit; commit signing works; the npm
#      environment allows main (a dry run only warns);
#      while the package is not on npm, npm signed in and pnpm on PATH. A
#      failure here changes nothing.
#   2  a signed tag sdk-vX.Y.Z, "Clax SDK X.Y.Z", on the commit
#   3  git push origin refs/tags/sdk-vX.Y.Z (the tag alone; no workflow
#      runs on it: release.yml and fleet-image.yml take v* tags)
#   4  gh workflow run publish-sdk.yml --ref main -f dry_run=false -f
#      sha=<the commit>, and wait for it. While the
#      package is not on npm (trusted publishing needs a package that
#      exists), its first version is published from here instead: in a
#      temporary worktree of the commit (target/release-sdk/X.Y.Z/publish),
#      pnpm install --frozen-lockfile, pnpm run build:sdk and pnpm run
#      test:sdk in web/, as publish-sdk.yml runs them, then npm publish
#      --access public from web/dist-sdk on this terminal, for npm's
#      prompts. Done once npm has
#      the version. After a first publish from here, every run ends with
#      the one-time trusted-publisher setup (and says it when step 4
#      fails), until a version publishes through publish-sdk.yml.
#
# A run that fails stops at that step, says which, and changes nothing after
# it. Run the same command again: each step's state is read from git,
# GitHub and npm (the tag here and on origin, the publish workflow's runs,
# the versions npm has), so the run resumes at the first step not done; a
# run of publish-sdk.yml under way is waited for, one that succeeded is
# taken as done (npm's view of a publish can lag it, so step 4 waits for
# npm to list the version), and a failed one is dispatched anew. A version
# npm has is refused, but for this command's own
# finished publish (npm has it, and origin has its tag), which reports every
# step done. It never force-pushes and never deletes a tag.
#
# It works from any directory: the checkout is the one holding this script.
#
# Environment:
#   GH_REPO                     the GitHub repository (default: origin's)
#   CLAX_RELEASE_NPM            the npm command (npm); for tests
#   CLAX_RELEASE_PNPM           the pnpm command (pnpm); for tests
#   CLAX_RELEASE_POLL_SECONDS   seconds between polls of a workflow run (20)
#   CLAX_RELEASE_MAX_POLLS      polls before a wait gives up (270, 90 min)
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
POLL="${CLAX_RELEASE_POLL_SECONDS:-20}"
MAX_POLLS="${CLAX_RELEASE_MAX_POLLS:-270}"
ACT_FLAG="--i-know-what-i-am-doing"
NPM="${CLAX_RELEASE_NPM:-npm}"
PNPM="${CLAX_RELEASE_PNPM:-pnpm}"
# The GitHub environment publish-sdk.yml's job runs in, which npm's trusted
# publisher names and the repository limits to main and prod.
NPM_ENV=npm
# The branch the package is published from.
BRANCH=main
WORKFLOW=publish-sdk.yml
# How publish-sdk.yml titles a run that publishes, before its commit.
RUN_TITLE="Publish SDK package"
# The file that holds the package's name and version.
SDK_MANIFEST=web/sdk/package.json
# Kept from a first publish from here (it holds that version) until a
# version publishes through publish-sdk.yml: while it exists, the
# trusted-publisher setup that workflow needs is said again.
FIRST_MARK="$ROOT/target/release-sdk/first-publish"
LAST=4

usage() {
    echo "usage: scripts/release-sdk.sh X.Y.Z [$ACT_FLAG]" >&2
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
TAG="sdk-v$V"
RESUME="scripts/release-sdk.sh $V $ACT_FLAG"

g() { git -C "$ROOT" "$@"; }
say() { printf '%s\n' "$*"; }
# Nothing shows a heartbeat here (scripts/lib/release-common.sh's detail).
detail() { :; }
PROBLEMS=()
problem() { PROBLEMS+=("$1"); }
NOTES=()
# shellcheck source=scripts/lib/release-common.sh
. "$ROOT/scripts/lib/release-common.sh"

# --- state, read from git, GitHub and npm -----------------------------------------

# sdk_field_at COMMIT FIELD: the top-level string FIELD (name, version) of
# COMMIT's web/sdk/package.json, or nothing. It reads the manifest's
# two-space, comma-ended lines, which web/sdk/package.test.ts pins.
sdk_field_at() {
    g show "$1:$SDK_MANIFEST" 2>/dev/null | sed -n "s/^  \"$2\": \"\(.*\)\",\$/\1/p"
}
sdk_version_at() { sdk_field_at "$1" version; }
is_signed_tag() { g cat-file tag "$TAG" 2>/dev/null | grep -E -- '^-----BEGIN [A-Z ]*SIGNATURE-----$' >/dev/null; }

# PKG is the package's npm name, from the commit published.
BRANCH_TIP="" LOCAL_TAG="" REMOTE_TAG="" COMMIT="" PKG="" ON_NPM=""
# Reads the state the steps are judged by; fails, saying why in
# REFRESH_ERR, when origin cannot be read.
refresh() {
    local out
    BRANCH_TIP="$(g rev-parse -q --verify "refs/remotes/origin/$BRANCH^{commit}" 2>/dev/null || true)"
    LOCAL_TAG="$(g rev-parse -q --verify "refs/tags/$TAG^{commit}" 2>/dev/null || true)"
    out="$(g ls-remote origin "refs/tags/$TAG" "refs/tags/$TAG^{}")" \
        || { REFRESH_ERR="cannot read origin (git ls-remote origin)"; return 1; }
    REMOTE_TAG="$(awk -v t="refs/tags/$TAG" '$2 == t "^{}" { p = $1 } $2 == t { q = $1 } END { print (p != "" ? p : q) }' <<< "$out")"
    # The tag's commit once there is a tag; the branch's tip until then.
    COMMIT="${LOCAL_TAG:-${REMOTE_TAG:-$BRANCH_TIP}}"
    PKG=""
    if [ -n "$COMMIT" ]; then PKG="$(sdk_field_at "$COMMIT" name)"; fi
}

# --- step 1: preflight ------------------------------------------------------------

check_tools() {
    local t
    for t in git gh "$NPM"; do
        command -v "$t" >/dev/null 2>&1 || problem "$t is not on PATH"
    done
}

# The version: the commit's, and the tag's commit on the branch.
check_version() {
    local at what
    if ! semver_ok "$V"; then
        problem "$V is not a version (X.Y.Z)"
        return 0
    fi
    # Step 3 pushes only a signed tag, and this never deletes one.
    if [ -n "$LOCAL_TAG" ] && [ -z "$REMOTE_TAG" ] && ! is_signed_tag; then
        problem "the tag $TAG here is not a signed tag: delete it (git tag -d $TAG) and run again"
        return 0
    fi
    [ -n "$BRANCH_TIP" ] || return 0
    if [ -n "$LOCAL_TAG" ] && [ -n "$REMOTE_TAG" ] && [ "$LOCAL_TAG" != "$REMOTE_TAG" ]; then
        problem "the tag $TAG tags $LOCAL_TAG here and $REMOTE_TAG on origin"
        return 0
    fi
    if [ "$COMMIT" != "$BRANCH_TIP" ] && ! g merge-base --is-ancestor "$COMMIT" "$BRANCH_TIP" 2>/dev/null; then
        problem "the tag $TAG tags $COMMIT, which is not on origin/$BRANCH"
        return 0
    fi
    at="$(sdk_version_at "$COMMIT")"
    [ "$at" != "$V" ] || return 0
    if [ "$COMMIT" = "$BRANCH_TIP" ] && [ -z "$LOCAL_TAG$REMOTE_TAG" ]; then
        what="origin/$BRANCH's tip ($COMMIT)"
        # The branch has moved past V: V is published, if at all, from the
        # older commit on it that carries V, which a tag names.
        if semver_ok "$at" && [ "$(ver_cmp "$at" "$V")" = 1 ]; then
            problem "$what carries SDK version $at, past $V: publish $at, or tag the commit on $BRANCH that carries $V (git tag -s $TAG -m \"Clax SDK $V\" <commit>) and run again"
            return 0
        fi
    else what="the tag $TAG's commit ($COMMIT)"; fi
    problem "$what carries SDK version ${at:-none}, not $V (the version in $SDK_MANIFEST): raise it on main by a pull request"
}

# The package on npm: X.Y.Z newer than every version npm has, but for this
# command's own finished publish. Trusted publishing (publish-sdk.yml) needs
# a package that exists, so while it does not, step 4 publishes the first
# version from here, which needs npm signed in and pnpm.
check_npm() {
    local err highest
    command -v "$NPM" >/dev/null 2>&1 && semver_ok "$V" || return 0
    if [ -z "$PKG" ]; then
        if [ -n "$COMMIT" ]; then problem "cannot read the package name from $COMMIT:$SDK_MANIFEST"; fi
        return 0
    fi
    err="$(mktemp)"
    if ! ON_NPM="$(on_npm "$PKG" 2>"$err")"; then
        problem "cannot ask npm about $PKG: $(cat "$err")"
        rm -f "$err"
        return 0
    fi
    rm -f "$err"
    if [ "$ON_NPM" = no ]; then
        if ! "$NPM" whoami >/dev/null 2>&1; then
            problem "$PKG is not on npm yet, so step 4 publishes its first version from here: sign in first (npm login; npm whoami fails)"
        fi
        command -v "$PNPM" >/dev/null 2>&1 || problem "pnpm is not on PATH (the first publish builds and tests the package)"
        NOTES+=("$PKG is not on npm yet: step 4 publishes its first version from this machine (npm publish, with npm's own prompts), then prints the trusted-publisher setup")
        return 0
    fi
    if has_version "$PKG" "$V"; then
        if [ -n "$REMOTE_TAG" ]; then NOTES+=("$PKG@$V is on npm and origin has $TAG: every step is done")
        else problem "npm has $PKG@$V already, and origin has no tag $TAG: a version npm has is not published again"; fi
        return 0
    fi
    if ! highest="$(highest_on_npm "$PKG")"; then
        problem "cannot read the versions npm has of $PKG ($NPM view $PKG versions)"
    elif [ -n "$highest" ] && [ "$(ver_cmp "$highest" "$V")" != -1 ]; then
        problem "npm has $PKG at $highest, not older than $V"
    fi
}
# The highest X.Y.Z of every version npm has of package $1, whatever its
# dist-tags say, or nothing. npm prints the versions as a JSON array, or as
# one string for a package with one version.
highest_on_npm() {
    local out v top=""
    out="$("$NPM" view "$1" versions --json 2>/dev/null)" || return 1
    for v in $(grep -oE '"[^"]*"' <<< "$out" | tr -d '"'); do
        semver_ok "${v%%-*}" || continue
        if [ -z "$top" ] || [ "$(ver_cmp "$v" "$top")" = 1 ]; then top="$v"; fi
    done
    echo "$top"
}

# CI's gate on the commit, as publish-sdk.yml checks it.
check_ci() {
    local id state=""
    [ -n "$COMMIT" ] && command -v gh >/dev/null 2>&1 || return 0
    id="$(find_run ci.yml push "$COMMIT" 2>/dev/null)" || id=""
    if [ -n "$id" ]; then state="$(ci_gate_state "$id" 2>/dev/null)" || state=""; fi
    [ "$state" = "completed success" ] && return 0
    problem "CI's bazel job has not passed on $COMMIT (${state:-no CI run on it}): publish-sdk.yml publishes only from a commit CI passed"
}

# The npm environment (npm_environment_problem): a dry run warns, a publish
# refuses, unless npm has the version already.
check_npm_environment() {
    local why
    command -v gh >/dev/null 2>&1 && [ -n "${GH_REPO:-}" ] && semver_ok "$V" || return 0
    if [ "$ON_NPM" = yes ] && has_version "$PKG" "$V"; then return 0; fi
    why="$(npm_environment_problem "$BRANCH" "$WORKFLOW runs in it")"
    [ -n "$why" ] || return 0
    if [ "$ACT" = 1 ]; then problem "$why"; else NOTES+=("WARN: $why"); fi
}

preflight() {
    check_tools
    command -v git >/dev/null 2>&1 || return 0
    resolve_repo
    if ! g fetch --quiet --no-tags origin "$BRANCH"; then
        problem "cannot fetch $BRANCH from origin: the SDK package is published from $BRANCH"
    fi
    refresh || problem "$REFRESH_ERR"
    [ -n "$BRANCH_TIP" ] || problem "origin has no $BRANCH branch"
    check_version
    if command -v gh >/dev/null 2>&1 && ! gh auth status >/dev/null 2>&1; then
        problem "gh is not signed in (gh auth login)"
    fi
    check_npm
    check_ci
    check_npm_environment
    check_signing
}

# --- steps 2-4 --------------------------------------------------------------------

TITLES=(
    [2]="Tag the commit"
    [3]="Push the tag"
    [4]="Publish the SDK package"
)
command_of() {
    local c="${COMMIT:-<the commit>}"
    case "$1" in
        2) echo "git tag -s $TAG -m \"Clax SDK $V\" $c" ;;
        3) echo "git push origin refs/tags/$TAG" ;;
        4) echo "gh workflow run $WORKFLOW --ref $BRANCH -f dry_run=false -f sha=$c, wait (the first version: npm publish from here)" ;;
    esac
}

done_2() { [ -n "$LOCAL_TAG" ] || [ -n "$REMOTE_TAG" ]; }
done_3() { [ -n "$REMOTE_TAG" ]; }
done_4() { has_version "$PKG" "$V"; }

step_2() {
    g tag -s "$TAG" -m "Clax SDK $V" "$COMMIT"
    is_signed_tag || { say "  the tag $TAG carries no signature"; return 1; }
}
step_3() {
    is_signed_tag || { say "  the tag $TAG carries no signature; not pushing it"; return 1; }
    g push origin "refs/tags/$TAG"
}
step_4() {
    local on
    on="$(on_npm "$PKG")" || { say "  cannot ask npm about $PKG"; return 1; }
    if [ "$on" = no ]; then first_publish; return; fi
    publish_through "$WORKFLOW" "$RUN_TITLE" "$COMMIT" "$BRANCH"
    await_output npm_version >/dev/null || { say "  npm does not list $PKG@$V after the publish run"; return 1; }
    # The workflow published: its trusted publisher is set up.
    rm -f "$FIRST_MARK"
}

# The package's first version, published from here: from a temporary
# worktree of COMMIT, removed after.
first_publish() {
    local dir="$ROOT/target/release-sdk/$V/publish" rc=0
    say "  $PKG is not on npm yet: publishing its first version from here"
    if [ -e "$dir" ]; then g worktree remove --force "$dir" 2>/dev/null || rm -rf "$dir"; fi
    g worktree prune
    mkdir -p "$(dirname "$dir")"
    g worktree add --quiet --detach "$dir" "$COMMIT"
    set +e
    (set -e; publish_from "$dir/web")
    rc=$?
    set -e
    g worktree remove --force "$dir"
    return "$rc"
}
# The checks publish-sdk.yml makes (the package builds and passes its test,
# at this version), then npm publish of web/dist-sdk, on this terminal for
# npm's sign-in and one-time-password prompts, then the wait for npm to list
# it. $1 is the worktree's web/.
publish_from() {
    local version
    cd "$1"
    say "  build and test: $PNPM install --frozen-lockfile && $PNPM run build:sdk && $PNPM run test:sdk"
    "$PNPM" install --frozen-lockfile
    "$PNPM" run build:sdk
    "$PNPM" run test:sdk
    version="$(sed -n 's/^  "version": "\(.*\)",$/\1/p' dist-sdk/package.json)"
    [ "$version" = "$V" ] || { say "  web/dist-sdk/package.json is at ${version:-no version}, not $V"; return 1; }
    cd dist-sdk
    say "  $NPM publish --access public (web/dist-sdk built at $COMMIT)"
    "$NPM" publish --access public
    # Kept as soon as npm took the package, so that the setup is said
    # again even if the wait below gives up, and on every later run.
    echo "$V" > "$FIRST_MARK"
    say "  $PKG@$V is on npm; the trusted-publisher setup later versions need is said at the end"
    await_output npm_version >/dev/null || { say "  npm does not list $PKG@$V after the publish"; return 1; }
}
# Prints the version once npm lists it.
npm_version() { if has_version "$PKG" "$V"; then echo "$V"; fi; }

# The one-time setup publish-sdk.yml needs once a first version was
# published from here (FIRST_MARK).
trusted_publisher_setup() {
    [ -e "$FIRST_MARK" ] || return 0
    say ""
    say "Once: $PKG's first version ($(cat "$FIRST_MARK")) was published from this machine, and later"
    say "versions publish through $WORKFLOW, which uses npm trusted publishing. On npmjs.com,"
    say "the package's Settings, Trusted Publisher, GitHub Actions: organization ${GH_REPO%%/*},"
    say "repository ${GH_REPO#*/}, workflow $WORKFLOW, environment $NPM_ENV; then Settings,"
    say "Publishing access: Require two-factor authentication and disallow tokens. (Said until a"
    say "version publishes through $WORKFLOW.)"
}

# --- main -------------------------------------------------------------------------

# Lists steps 2-4, done or to do; NEXT is the first step to do.
overview() {
    local n
    say ""
    say "The steps ($PKG $V from ${COMMIT:-?}; origin/$BRANCH at ${BRANCH_TIP:-?}):"
    NEXT=""
    for n in $(seq 2 "$LAST"); do
        if [ -z "$NEXT" ] && "done_$n" 2>/dev/null; then
            say "  $n. [done] ${TITLES[n]}: $(command_of "$n")"
        else
            NEXT="${NEXT:-$n}"
            say "  $n. [todo] ${TITLES[n]}: $(command_of "$n")"
        fi
    done
    say ""
}

if [ "$ACT" = 0 ]; then
    say "Dry run: nothing is changed. Run with $ACT_FLAG to publish."
    say ""
fi
say "Step 1: preflight"
preflight
if [ "${#PROBLEMS[@]}" -gt 0 ]; then
    for p in "${PROBLEMS[@]}"; do say "  FAIL: $p"; done
    if [ "$ACT" = 1 ] || ! semver_ok "$V"; then
        say "release-sdk.sh: preflight failed; nothing was changed."
        exit 1
    fi
else
    say "  ok: $PKG $V from $COMMIT on origin/$BRANCH ($GH_REPO), CI passed on it, gh signed in, signing works"
fi
for note in ${NOTES[@]+"${NOTES[@]}"}; do say "  note: $note"; done
overview

if [ "$ACT" = 0 ]; then
    if [ "${#PROBLEMS[@]}" -gt 0 ]; then
        say "Preflight failed: fix the above first. Nothing was changed."
        exit 1
    fi
    if [ -z "$NEXT" ]; then say "Every step is done. Nothing was changed."
    else say "A run with $ACT_FLAG starts at step $NEXT. Nothing was changed."; fi
    trusted_publisher_setup
    exit 0
fi

# The state preflight read holds until a step runs.
STALE=0
for n in $(seq 2 "$LAST"); do
    if [ "$STALE" = 1 ] && ! refresh; then
        say "release-sdk.sh: $REFRESH_ERR before step $n; run \`$RESUME\` again to resume."
        exit 1
    fi
    if "done_$n" 2>/dev/null; then
        say "step $n/$LAST ${TITLES[n]}: done already"
        continue
    fi
    say "step $n/$LAST ${TITLES[n]}: $(command_of "$n")"
    STALE=1
    set +e
    (set -e; "step_$n")
    rc=$?
    set -e
    if [ "$rc" != 0 ]; then
        say ""
        say "release-sdk.sh: step $n (${TITLES[n]}) failed."
        say "Fix the cause, then run \`$RESUME\` again: it resumes at the first step not done."
        # A workflow run fails without the trusted publisher.
        if [ "$n" = 4 ]; then trusted_publisher_setup; fi
        exit 1
    fi
done
say ""
say "$PKG $V is on npm, published from $COMMIT (the tag $TAG)."
trusted_publisher_setup
