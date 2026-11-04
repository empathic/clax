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
#   1  cherry-picks (-x) onto origin/main, in a temporary worktree, each
#      commit of the release branch that neither main nor prod before the
#      merge has (the "Release X.Y.Z" commit, and any fix made on the
#      branch); a conflict stops it, naming the commit
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
    picks=()
    while IFS= read -r c; do picks+=("$c"); done < <(git -C "$ROOT" rev-list --reverse --no-merges "$merge^2" ^"$main" ^"$merge^1")
    [ "${#picks[@]}" -gt 0 ] || { echo "carry-release: $merge's release branch has no commit main lacks" >&2; exit 1; }

    wt="$(mktemp -d)"
    trap 'git -C "$ROOT" worktree remove --force "$wt" >/dev/null 2>&1 || true; rm -rf "$wt"' EXIT
    git -C "$ROOT" worktree add --quiet --detach "$wt" "$main"
    g() { git -C "$wt" -c user.name="Clax release" -c user.email=release@invalid -c commit.gpgsign=false "$@"; }
    for c in "${picks[@]}"; do
        if ! g cherry-pick -x "$c" >/dev/null 2>&1; then
            g cherry-pick --abort >/dev/null 2>&1 || true
            echo "carry-release: $(git -C "$ROOT" log -1 --format='%h "%s"' "$c") does not apply to main's tip ${main:0:12}: carry it by hand (docs/prod.md, \"Releasing\")" >&2
            exit 1
        fi
    done
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

    body="$(printf '%s\n\nThe release branch'"'"'s commits, cherry-picked from %s:\n\n%s\n\nand the plugins pinned to v%s (scripts/pin-release.sh).\n' \
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
