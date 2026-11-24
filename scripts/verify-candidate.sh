#!/usr/bin/env bash
# Checks that every file of a release candidate was made by this
# repository's workflows at the tested head commit, before Deploy prod
# ships it (docs/prod.md, "Releasing"): each file's attestation
# (actions/attest-build-provenance), through `gh attestation verify`.
#
# Usage: verify-candidate.sh <head commit> <dir>
#
# <dir> holds what find-candidate.sh's staging.yml run uploaded, one
# directory per artifact:
#   release-dist/*     the binaries, SHA256SUMS, install.sh and SOURCE_TREE,
#                      attested by release.yml (called by staging.yml)
#   worker/worker.tar  the Worker, attested by staging.yml
# Each must be attested as made by its workflow in GH_REPO (default: the
# current repository's) from <head commit>. Any other file, a missing
# artifact, or one file that fails refuses the whole candidate: exit 1,
# naming the first file that fails.
set -euo pipefail
[ $# = 2 ] || { echo "usage: verify-candidate.sh <head commit> <dir>" >&2; exit 2; }
head="$1"
dir="$2"
[[ $head =~ ^[0-9a-f]{40}$ ]] || { echo "verify-candidate: '$head' is not a full commit ID" >&2; exit 2; }
repo="${GH_REPO:-$(gh repo view --json nameWithOwner --jq .nameWithOwner)}"

[ -n "$(find "$dir/release-dist" -mindepth 1 -maxdepth 1 -type f 2>/dev/null)" ] \
    || { echo "verify-candidate: $dir/release-dist has no files" >&2; exit 1; }
[ -f "$dir/worker/worker.tar" ] || { echo "verify-candidate: no $dir/worker/worker.tar" >&2; exit 1; }

n=0
while IFS= read -r -d '' f; do
    rel="${f#"$dir"/}"
    [ ! -L "$f" ] || { echo "verify-candidate: $rel is a symbolic link, no file of a candidate" >&2; exit 1; }
    case "$rel" in
        release-dist/*) workflow=release.yml ;;
        worker/worker.tar) workflow=staging.yml ;;
        *) echo "verify-candidate: $rel is no file of a candidate" >&2; exit 1 ;;
    esac
    if ! out="$(gh attestation verify "$f" --repo "$repo" \
        --signer-workflow "$repo/.github/workflows/$workflow" --source-digest "$head" 2>&1)"; then
        echo "verify-candidate: $rel is not attested as $workflow's at $head:" >&2
        echo "$out" >&2
        exit 1
    fi
    echo "verified $rel ($workflow at ${head:0:12})"
    n=$((n + 1))
done < <(find "$dir" -mindepth 1 \( -type f -o -type l \) -print0 | LC_ALL=C sort -z)
echo "verify-candidate: $n files verified"
