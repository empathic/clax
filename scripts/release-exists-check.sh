#!/usr/bin/env bash
# Which source tree a Clax release was built from, the one rule both the
# Release workflow and scripts/release.sh judge a release by.
#
#   release-exists-check.sh --tree vX.Y.Z
#       prints the tree the release vX.Y.Z was built from, or nothing when
#       there is no such release
#   release-exists-check.sh vX.Y.Z <commit>
#       for .github/workflows/release.yml before it builds: prints
#       `skip=false` when the tag has no release (build and publish as
#       usual), and `skip=true` when its release was built from <commit>'s
#       tree (the tag was moved to a copy of its commit with the same files,
#       as re-signing makes: the release already holds this build); fails,
#       saying why, when the release was built from another tree
#
# The tree is the release's SOURCE_TREE asset, which the Release workflow
# writes (`git rev-parse HEAD^{tree}`) and SHA256SUMS covers. A release made
# before that asset existed is judged by the newest successful Release run
# on the tag created before the release was: the run that uploaded it. It
# fails, saying why, when neither tells, or gh cannot. Trees of commits are
# read through the GitHub API, since a run's commit need not be local.
#
# Environment: GH_REPO (owner/name) and GH_TOKEN, as gh reads them.
set -euo pipefail
: "${GH_REPO:?GH_REPO must name the repository}"

# tree_of COMMIT: the commit's tree, from GitHub.
tree_of() { gh api "repos/$GH_REPO/git/commits/$1" --jq .tree.sha; }

# release_tree TAG: prints the tree, nothing when there is no release.
release_tree() {
    local tag="$1" out created tree built
    if ! out="$(gh release view "$tag" --json createdAt --jq .createdAt 2>&1)"; then
        case "$out" in
            *"release not found"*) return 0 ;;
            *) echo "error: cannot tell whether the release $tag exists: $out" >&2; return 1 ;;
        esac
    fi
    created="$out"
    if tree="$(gh release download "$tag" --pattern SOURCE_TREE --output - 2>/dev/null)" && [ -n "$tree" ]; then
        echo "$tree"
        return 0
    fi
    built="$(gh run list --workflow release.yml --event push --branch "$tag" --status success --limit 100 \
        --json headSha,createdAt \
        --jq "[.[] | select(.createdAt <= \"$created\")] | sort_by(.createdAt) | last | .headSha // empty")"
    if [ -z "$built" ]; then
        echo "error: the release $tag (published $created) has no SOURCE_TREE and no successful Release run" \
            "on $tag before it, so which build it holds is unknown; delete it (gh release delete $tag --yes)" \
            "and rerun the Release run" >&2
        return 1
    fi
    tree_of "$built"
}

case "${1:-}" in
    --tree)
        [ $# = 2 ] || { echo "usage: release-exists-check.sh --tree vX.Y.Z" >&2; exit 2; }
        release_tree "$2"
        ;;
    *)
        [ $# = 2 ] || { echo "usage: release-exists-check.sh vX.Y.Z <commit> | --tree vX.Y.Z" >&2; exit 2; }
        tag="$1"
        sha="$2"
        have="$(release_tree "$tag")"
        if [ -z "$have" ]; then echo "skip=false"; exit 0; fi
        want="$(tree_of "$sha")"
        if [ -z "$want" ] || [ "$want" != "$have" ]; then
            echo "error: the release $tag exists, built from tree $have, and $tag now names $sha" \
                "(tree ${want:-unknown}), another tree; delete the release (gh release delete $tag --yes)" \
                "and rerun this run to build it" >&2
            exit 1
        fi
        echo "The release $tag exists already, built from tree $have, which $sha shares;" \
            "nothing is rebuilt or uploaded." >&2
        echo "skip=true"
        ;;
esac
