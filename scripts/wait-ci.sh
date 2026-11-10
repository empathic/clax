#!/usr/bin/env bash
# Waits for CI (ci.yml) on a commit and says whether it passed: the gate is
# its bazel job, as publish-pi.yml judges it (the run's own conclusion also
# counts perf-log, which is cancelled when pushes pile up).
#
# Usage: wait-ci.sh <commit> <event>      event: push or pull_request
#
# Exits 0 once the newest ci.yml run of <event> on <commit> has a bazel job
# that succeeded; 1 when that job ends any other way, or when the wait
# gives up. Reads GitHub through gh, for GH_REPO (default: the current
# repository's).
#
# Environment:
#   CLAX_WAIT_POLL_SECONDS   seconds between polls (30)
#   CLAX_WAIT_MAX_POLLS      polls before giving up (120: an hour)
set -euo pipefail
[ $# = 2 ] || { echo "usage: wait-ci.sh <commit> <event>" >&2; exit 2; }
commit="$1"
event="$2"
repo="${GH_REPO:-$(gh repo view --json nameWithOwner --jq .nameWithOwner)}"
poll="${CLAX_WAIT_POLL_SECONDS:-30}"
max="${CLAX_WAIT_MAX_POLLS:-120}"

for ((i = 1; i <= max; i++)); do
    run="$(gh run list --repo "$repo" --workflow ci.yml --event "$event" --commit "$commit" \
        --limit 1 --json databaseId --jq '.[0].databaseId // ""')"
    state=""
    if [ -n "$run" ]; then
        state="$(gh run view "$run" --repo "$repo" --json jobs \
            --jq '.jobs[] | select(.name == "bazel") | "\(.status) \(.conclusion)"')"
    fi
    case "$state" in
        "completed success")
            echo "wait-ci: CI's bazel job passed on $commit (run $run)"
            exit 0 ;;
        completed\ *)
            echo "wait-ci: CI's bazel job on $commit ended ${state#completed }: see run $run" >&2
            exit 1 ;;
    esac
    if [ -n "$state" ]; then now="$state"; elif [ -n "$run" ]; then now="run $run, no bazel job yet"; else now="no run yet"; fi
    echo "wait-ci: CI's bazel job on $commit: $now ($i/$max)"
    sleep "$poll"
done
echo "wait-ci: CI's bazel job on $commit has not finished after $max polls" >&2
exit 1
