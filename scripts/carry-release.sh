#!/usr/bin/env bash
# Carries a merged release to main (docs/prod.md, "Releasing"), as Deploy
# prod's carry job runs it with the release App's token: the release
# branch's own commits and the plugins' pin, in one pull request into main,
# merged once main's CI passes on it, so the marketplaces see the version
# and its pin in one push, after the release they pin exists.
#
# Usage: carry-release.sh X.Y.Z <merge commit>
#
# In a checkout holding origin/main and the merge commit with its history
# (fetch-depth 0), it:
#   1  merges the merge commit into origin/main, in a temporary worktree,
#      without committing: all that prod has and main lacks, which is the
#      release branch's own commits (the "Release X.Y.Z" commit, and any
#      fix made on the branch) and those of any earlier release merged
#      into prod whose carry never reached main (its Deploy prod run failed
#      first). A conflict stops it, naming the files
#   2  checks the versions agree at X.Y.Z (scripts/check-version.sh)
#   3  pins the plugins to the published vX.Y.Z (scripts/pin-release.sh)
#   4  makes that tree one commit on main's tip through GitHub's Git Data
#      API (blobs, a tree on main's, the commit), which GitHub signs as the
#      App's; it refuses a commit GitHub does not mark verified, since
#      main's ruleset takes signed commits only
#   5  points carry/X.Y.Z at it and opens the pull request into main
#   6  waits for CI's bazel job on it (scripts/wait-ci.sh), then squash
#      merges it (main takes linear, signed history; GitHub signs the
#      squash), and prints `commit=<main's new commit>`
# Run again, it finds a carry pull request already merged and prints its
# commit; an open one it waits for and merges.
#
# Environment: GH_REPO (owner/name) and GH_TOKEN, as gh reads them; the
# token must be the App's: a workflow's own token starts no CI on what it
# pushes. CLAX_WAIT_* as scripts/wait-ci.sh reads them.
set -euo pipefail
[ $# = 2 ] || { echo "usage: carry-release.sh X.Y.Z <merge commit>" >&2; exit 2; }
version="$1"
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "carry-release: '$version' is not X.Y.Z" >&2; exit 2; }
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
: "${GH_REPO:?GH_REPO must name the repository}"
branch="carry/$version"
merge="$(git -C "$ROOT" rev-parse --verify "$2^{commit}")"
subject="chore(release): $version, with the plugins pinned to it"

merged_commit() {
    gh pr list --repo "$GH_REPO" --head "$branch" --base main --state merged \
        --json mergeCommit --jq '.[0].mergeCommit.oid // ""'
}
open_pr() {
    gh pr list --repo "$GH_REPO" --head "$branch" --base main --state open \
        --json number,headRefOid --jq '.[0] // empty | "\(.number) \(.headRefOid)"'
}

done_commit="$(merged_commit)"
if [ -n "$done_commit" ]; then
    echo "carry-release: $branch is merged into main already"
    echo "commit=$done_commit"
    exit 0
fi

existing="$(open_pr)"
if [ -z "$existing" ]; then
    main="$(git -C "$ROOT" rev-parse --verify origin/main)"
    main_tree="$(git -C "$ROOT" rev-parse "$main^{tree}")"
    # What the carry brings, for its message: the commits of prod main
    # lacks, back to the newest commit of prod's first-parent line that
    # merges into main changing nothing (a release already carried, or
    # prod's beginning on main).
    carried=""
    while IFS= read -r p; do
        if [ "$(git -C "$ROOT" merge-tree --write-tree "$main" "$p" 2>/dev/null)" = "$main_tree" ]; then
            carried="$p"
            break
        fi
    done < <(git -C "$ROOT" rev-list --first-parent --max-count=50 "$merge^1")
    picks=()
    while IFS= read -r c; do picks+=("$c"); done < <(git -C "$ROOT" rev-list --reverse --no-merges "$merge" ^"$main" ${carried:+^"$carried"})
    [ "${#picks[@]}" -gt 0 ] || { echo "carry-release: prod's $merge has no commit main lacks" >&2; exit 1; }
    wt="$(mktemp -d)"
    trap 'git -C "$ROOT" worktree remove --force "$wt" >/dev/null 2>&1 || true; rm -rf "$wt"' EXIT
    git -C "$ROOT" worktree add --quiet --detach "$wt" "$main"
    g() { git -C "$wt" -c user.name="Clax release" -c user.email=release@invalid -c commit.gpgsign=false "$@"; }
    if ! g merge --no-commit --no-ff "$merge" >/dev/null 2>&1; then
        conflicts="$(g diff --name-only --diff-filter=U | tr '\n' ' ')"
        echo "carry-release: prod's ${merge:0:12} does not merge into main's tip ${main:0:12}: ${conflicts:-it }conflicts: carry it by hand (docs/prod.md, \"Releasing\")" >&2
        exit 1
    fi
    have="$("$wt/scripts/check-version.sh" --print)"
    [ "$have" = "$version" ] || { echo "carry-release: main with the release's commits is at $have, not $version" >&2; exit 1; }
    "$wt/scripts/pin-release.sh" "v$version"
    "$wt/scripts/check-version.sh"
    g add -A

    # The tree, on main's: a blob for each path that changed, with its mode;
    # a deleted path's entry has no blob.
    entries="$(mktemp)"
    while IFS= read -r -d '' path; do
        if [ -e "$wt/$path" ] || [ -L "$wt/$path" ]; then
            mode="$(git -C "$wt" ls-files --stage -- "$path" | cut -d' ' -f1)"
            blob="$(base64 < "$wt/$path" | tr -d '\n' \
                | jq -Rs '{content: ., encoding: "base64"}' \
                | gh api "repos/$GH_REPO/git/blobs" --method POST --input - --jq .sha)"
            jq -nc --arg p "$path" --arg m "$mode" --arg s "$blob" '{path: $p, mode: $m, type: "blob", sha: $s}'
        else
            jq -nc --arg p "$path" '{path: $p, mode: "100644", type: "blob", sha: null}'
        fi
    done < <(git -C "$wt" diff --cached --name-only --no-renames -z "$main") > "$entries"
    base_tree="$(git -C "$ROOT" rev-parse "$main^{tree}")"
    tree="$(jq -s --arg b "$base_tree" '{base_tree: $b, tree: .}' "$entries" \
        | gh api "repos/$GH_REPO/git/trees" --method POST --input - --jq .sha)"
    want_tree="$(g write-tree)"
    [ "$tree" = "$want_tree" ] || { echo "carry-release: GitHub made tree $tree, not the carry's $want_tree" >&2; exit 1; }

    body="$(printf '%s\n\nWhat prod has and main lacked, merged from %s:\n\n%s\n\nand the plugins pinned to v%s (scripts/pin-release.sh).\n' \
        "$subject" "${merge:0:12}" "$(git -C "$ROOT" log --no-walk=unsorted --format='- %h %s' "${picks[@]}")" "$version")"
    made="$(jq -n --arg m "$body" --arg t "$tree" --arg p "$main" '{message: $m, tree: $t, parents: [$p]}' \
        | gh api "repos/$GH_REPO/git/commits" --method POST --input - --jq '"\(.sha) \(.verification.verified)"')"
    commit="${made% *}"
    [ "${made#* }" = true ] || { echo "carry-release: GitHub did not sign $commit: main takes signed commits only" >&2; exit 1; }

    if gh api "repos/$GH_REPO/git/ref/heads/$branch" >/dev/null 2>&1; then
        jq -n --arg s "$commit" '{sha: $s, force: true}' \
            | gh api "repos/$GH_REPO/git/refs/heads/$branch" --method PATCH --input - >/dev/null
    else
        jq -n --arg r "refs/heads/$branch" --arg s "$commit" '{ref: $r, sha: $s}' \
            | gh api "repos/$GH_REPO/git/refs" --method POST --input - >/dev/null
    fi
    gh pr create --repo "$GH_REPO" --base main --head "$branch" --title "$subject" \
        --body "Carries Clax $version, merged into prod as $merge, to main with its pin. Deploy prod merges it once CI passes."
    existing="$(open_pr)"
fi

number="${existing% *}"
head="${existing#* }"
"$ROOT/scripts/wait-ci.sh" "$head" pull_request
gh pr merge "$number" --repo "$GH_REPO" --squash --subject "$subject (#$number)" --match-head-commit "$head"
done_commit="$(merged_commit)"
[ -n "$done_commit" ] || { echo "carry-release: #$number is not merged" >&2; exit 1; }
echo "commit=$done_commit"
