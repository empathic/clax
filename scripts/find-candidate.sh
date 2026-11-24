#!/usr/bin/env bash
# Finds the release candidate a merge into prod ships (docs/prod.md,
# "Releasing"): the merge commit's second parent is the release branch's
# head, the commit staging tested, and the candidate is the latest
# successful staging.yml run on a push of that head.
#
# Usage: find-candidate.sh <merge commit>
#
# Prints, as `key=value` lines for $GITHUB_OUTPUT:
#   head=<the second parent>
#   run=<the staging.yml run's ID>
#   pr=<the release pull request's number>
#
# Exits 1, naming why, when the commit is no merge (no second parent), when
# its tree is not the head's (something besides the release branch entered
# with the merge: prod moved while the pull request was open), when no
# staging.yml run on a push of the head succeeded, or when GitHub names no
# pull request for the merge commit. It reads git in the current directory,
# which must hold the merge commit and its parents (a checkout with
# fetch-depth 2), and GitHub through gh, for GH_REPO (default: the
# current repository's).
set -euo pipefail
[ $# = 1 ] || { echo "usage: find-candidate.sh <merge commit>" >&2; exit 2; }
merge="$(git rev-parse --verify "$1^{commit}")"
repo="${GH_REPO:-$(gh repo view --json nameWithOwner --jq .nameWithOwner)}"

head="$(git rev-parse --verify --quiet "$merge^2^{commit}")" || {
    echo "find-candidate: $merge is not a merge commit: a release reaches prod by merging its pull request with a merge commit" >&2
    exit 1
}
merge_tree="$(git rev-parse "$merge^{tree}")"
head_tree="$(git rev-parse "$head^{tree}")"
if [ "$merge_tree" != "$head_tree" ]; then
    echo "find-candidate: $merge's tree ($merge_tree) is not the tested head $head's ($head_tree): prod moved while the release pull request was open; cut the release again" >&2
    exit 1
fi

run="$(gh run list --repo "$repo" --workflow staging.yml --event push --commit "$head" --status success \
    --limit 1 --json databaseId --jq '.[0].databaseId // ""')"
if [ -z "$run" ]; then
    echo "find-candidate: no staging.yml run on a push of $head succeeded: staging never passed on the commit this merge ships" >&2
    exit 1
fi

pr="$(gh api "repos/$repo/commits/$merge/pulls" --jq '[.[] | select(.base.ref == "prod")][0].number // ""')"
if [ -z "$pr" ]; then
    echo "find-candidate: GitHub names no pull request into prod for $merge" >&2
    exit 1
fi

echo "head=$head"
echo "run=$run"
echo "pr=$pr"
