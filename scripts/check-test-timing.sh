#!/usr/bin/env bash
# Prints each sleep in the test scripts that stands in for a condition (a
# `sleep <delay>` run in the foreground, or Python's `time.sleep(`), and
# each comparison that judges a clock reading (Python's `time.time()`,
# `time.monotonic()` or `time.perf_counter()`, JavaScript's `Date.now()` or
# `performance.now()`, the shell's `date +%s` or `$SECONDS`, or a name
# assigned one or a value computed from one): a budget on how long
# something took, or a deadline that ends a wait. The scripts are those
# every sh_test in scripts/BUILD.bazel runs (the gates, the tests, the
# comment-loop smoke), and scripts/test-*.sh, scripts/*.test.sh,
# scripts/fake-*, scripts/smoke-*.sh and scripts/*-smoke.sh, which run
# outside Bazel too; the Python and Node they embed (heredocs, `-c`, `-e`)
# are read with them. Not the perf benchmarks (perf_*_full, `manual`):
# they measure, on demand, and gate nothing.
#
# A test waits on the condition instead (scripts/lib/wait.sh's wait_for,
# whose own poll is not scanned, or the same bounded by a count of polls
# in Python), asserts the outcome rather than how long it took (printing
# a measurement is fine), and models a hang with a process that never
# ends by itself: an `exec sleep N` or a backgrounded `sleep N &`, killed
# by its PID, or a `read` from a FIFO.
#
# The one exemption is a comment `# timing: <why>` at the end of the line,
# or alone on the line before it, with its reason. An exemption on a line
# with nothing to exempt is a finding too.
#
# Report mode (docs/usage.md, "Developing Clax"): exits 0 whatever it
# prints, unless CLAX_TIMING=deny (bazel test --config=timing), when a
# finding fails it. scripts/check-test-timing.py's DEFAULT makes deny the
# default. No linter reads shell, so this is the shell's linter: a line
# scanner, as scripts/check-pipefail-pipes.sh is. With file arguments,
# checks those files instead; with --list, prints the files it checks.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"

if [ $# -gt 0 ] && [ "$1" != --list ]; then
    exec python3 "$here/check-test-timing.py" "$@"
fi
[ -f "$here/BUILD.bazel" ] || { echo "check-test-timing: $here/BUILD.bazel is missing" >&2; exit 2; }
names=()
while IFS= read -r src; do names+=("$src"); done < <(python3 "$here/check-test-timing.py" --sh-tests "$here/BUILD.bazel")
for f in "$here"/test-*.sh "$here"/*.test.sh "$here"/fake-* "$here"/smoke-*.sh "$here"/*-smoke.sh; do
    [ -f "$f" ] && names+=("${f##*/}")
done
files=()
while IFS= read -r name; do
    # The check and its test hold the patterns it looks for.
    case "$name" in check-test-timing.sh | test-check-test-timing.sh) ;; *) files+=("scripts/$name") ;; esac
done < <(printf '%s\n' "${names[@]}" | sort -u)
cd "$here/.."
if [ "${1:-}" = --list ]; then
    printf '%s\n' "${files[@]}"
    exit 0
fi
python3 "$here/check-test-timing.py" "${files[@]}"
