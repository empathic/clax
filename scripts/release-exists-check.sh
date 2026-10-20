#!/usr/bin/env bash
# Decides, for the Release workflow on a pushed tag, whether its release
# exists already: .github/workflows/release.yml runs it before building.
#
#   release-exists-check.sh vX.Y.Z <commit>
#
# Prints `skip=false` when the tag has no release (build and publish as
# usual), and `skip=true` when the release exists and the earliest
# successful Release run on the tag built a commit with the same tree as
# <commit>: the tag was moved to a copy of its commit with the same files,
# as re-signing makes, so the release already holds this build. It fails,
# saying why, when the release exists and that run built another tree, or
# no successful run made it, or gh cannot tell. Trees are read through the
# GitHub API, as the earlier run's commit need not be in the checkout.
#
# Environment: GH_REPO (owner/name) and GH_TOKEN, as gh reads them.
set -euo pipefail
[ $# = 2 ] || { echo "usage: release-exists-check.sh vX.Y.Z <commit>" >&2; exit 2; }
tag="$1"
sha="$2"
: "${GH_REPO:?GH_REPO must name the repository}"

if ! out="$(gh release view "$tag" --json tagName 2>&1)"; then
    case "$out" in
        *"release not found"*) echo "skip=false"; exit 0 ;;
        *) echo "error: cannot tell whether the release $tag exists: $out" >&2; exit 1 ;;
    esac
fi

earliest="$(gh run list --workflow release.yml --event push --branch "$tag" --status success --limit 100 \
    --json databaseId,headSha,createdAt --jq 'sort_by(.createdAt) | .[0] // empty | "\(.databaseId) \(.headSha)"')"
if [ -z "$earliest" ]; then
    echo "error: the release $tag exists, but no successful Release run on $tag made it; delete it" \
        "(gh release delete $tag --yes) and run this again" >&2
    exit 1
fi
run="${earliest%% *}"
built="${earliest#* }"
want="$(gh api "repos/$GH_REPO/git/commits/$sha" --jq .tree.sha)"
have="$(gh api "repos/$GH_REPO/git/commits/$built" --jq .tree.sha)"
if [ -z "$want" ] || [ "$want" != "$have" ]; then
    echo "error: the release $tag exists, built by run $run from $built (tree $have), and $tag now names" \
        "$sha (tree ${want:-unknown}), another tree; delete the release (gh release delete $tag --yes)" \
        "and rerun this run to build it" >&2
    exit 1
fi
echo "The release $tag exists already, built by run $run from $built, whose tree ($have) $sha shares;" \
    "nothing is rebuilt or uploaded." >&2
echo "skip=true"
