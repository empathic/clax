#!/usr/bin/env bash
# `scripts/perf-vis-baseline.sh <commit>`: measures the visibility budgets'
# baseline (scripts/perf-daemon-budget.json `vis_baseline`; spec
# 2026-10-08-multi-agent-multi-person §23) on the release build of <commit>,
# the commit before the visibility filter: the single-owner golden's
# collector commit, the branch's first.
#
# It checks <commit> out in a scratch worktree, builds its release `clax`
# there (in its own target directory, removed afterwards), and runs this
# checkout's scripts/perf-daemon.py --phase vis_baseline against it: the
# owner's agent reading an artifact's threads (200 comments) and polling 20
# feedback rows, on an install with no people, through routes <commit>
# already has. It prints a `VIS_BASELINE {...}` line; copy its two values
# into the budget file's `vis_baseline` with the commit and the machine.
# The budgets are that baseline plus their margins, so measure it on the
# machine the gate's budgets are judged on, and again whenever the branch is
# rebased onto a new main (pass the rebased collector commit).
#
# The release build embeds web/dist and web/dist-extension; the phase reads
# only the API, so this checkout's built ones are copied in (build them first:
# `cd web && pnpm run build`).
set -euo pipefail
cd "$(dirname "$0")/.."
[ $# -eq 1 ] || { echo "usage: scripts/perf-vis-baseline.sh <commit>" >&2; exit 2; }
commit="$(git rev-parse --verify "$1^{commit}")"
for d in web/dist web/dist-extension; do
    [ -d "$d" ] || { echo "perf-vis-baseline: $d is missing; run 'cd web && pnpm run build' first" >&2; exit 2; }
done
scratch="$(mktemp -d "${TMPDIR:-/tmp}/clax-vis-baseline.XXXXXX")"
cleanup() {
    git worktree remove --force "$scratch/tree" >/dev/null 2>&1 || true
    rm -rf "$scratch"
}
trap cleanup EXIT
git worktree add --detach "$scratch/tree" "$commit" >/dev/null
cp -R web/dist web/dist-extension "$scratch/tree/web/"
echo "perf-vis-baseline: building $commit (release)" >&2
(cd "$scratch/tree" && CARGO_TARGET_DIR="$scratch/target" CARGO_INCREMENTAL=0 cargo build -q --release -p clax-cli)
# A copy kept by its content, so macOS assesses it once (scripts/stable-bin.sh).
bin="$PWD/$(scripts/stable-bin.sh "$scratch/target/release/clax" target/clax-bin/release)"
echo "perf-vis-baseline: measuring $bin" >&2
python3 scripts/perf-daemon.py --phase vis_baseline "$bin" scripts/perf-daemon-budget.json
echo "perf-vis-baseline: commit $commit" >&2
