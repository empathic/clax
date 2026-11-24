#!/usr/bin/env bash
# Checks that a pull request into prod comes from a release branch of this
# repository: CI's `version` job runs it on every pull request into prod.
#   check-release-head.sh <head ref> <head repository> <repository>
# Exits 0 when the head ref is release/X.Y.Z and the head repository is the
# repository; otherwise 1, with a one-line reason on stderr. Releases are
# cut onto release/X.Y.Z by `just release` (scripts/release.sh), and only
# the repository admins may push those branches (the ruleset `release`,
# scripts/setup-prod-github.sh). The pull request's own workflow files run
# this, so it guards against mistakes, not against a writer: that is the
# ruleset's and the staging environment's.
set -uo pipefail
[ $# = 3 ] || { echo "usage: check-release-head.sh <head ref> <head repository> <repository>" >&2; exit 2; }
head="$1" from="$2" repo="$3"
no() { echo "check-release-head: $*" >&2; exit 1; }

[ "$from" = "$repo" ] || no "the pull request comes from $from, not $repo: a release is cut onto a branch of $repo (just release X.Y.Z)"
[[ $head =~ ^release/[0-9]+\.[0-9]+\.[0-9]+$ ]] \
    || no "$head is not a release branch (release/X.Y.Z): a pull request into prod is a release, cut by just release X.Y.Z"
exit 0
