#!/usr/bin/env bash
# Fails when a script that runs under `set -o pipefail` pipes into a reader
# that can stop before its input ends: `grep -q` (or -l, -L, -m, --quiet,
# --silent, --max-count, --files-with-matches), `head`, a `sed` script with a
# q or Q command, or an `awk` program with `exit`. Such a reader can exit
# while the writer before it is still writing; the writer then dies of
# SIGPIPE and pipefail fails the pipeline although the reader found what it
# looked for. Whether it happens depends on timing, so it passes on one
# machine and fails on another. Readers that read all their input do
# instead: `grep ... >/dev/null`, `sed -n 1p`, an awk program that keeps
# reading.
#
# A script counts as running under pipefail when it sets it, or when a
# script that sets it sources it (`. scripts/x.sh`). With file arguments,
# checks those files, each counted as running under pipefail (for trying
# the check out); without, every tracked shell script and justfile.
#
# A line ending in a pipe is read with the next. A reader behind `(`, `{`,
# variable assignments, `command`, `exec`, `env` or `timeout N` is found;
# `|&` counts as a pipe. Known gaps, none of them in the scripts today: a
# quoted `|` (as in `grep -E 'a|b'`) splits the command there; only a sed's
# or awk's first program is read (`sed -e p -e q` passes); `read` and
# python readers are not checked; and a script is counted as sourced only
# through a literal path (`. scripts/x.sh`, `. "$HERE/x.sh"`), not a computed
# one or through another sourced script.
set -euo pipefail
cd "$(dirname "$0")/.."

if [ $# -gt 0 ]; then
    exec python3 scripts/check-pipefail-pipes.py --all "$@"
fi
files=()
while IFS= read -r -d '' f; do
    case "$f" in scripts/check-pipefail-pipes.sh|scripts/test-check-pipefail-pipes.sh) ;; *) files+=("$f") ;; esac
done < <(git ls-files -z -- '*.sh' '*.bash' 'justfile' '**/justfile')
python3 scripts/check-pipefail-pipes.py "${files[@]}"
