#!/usr/bin/env bash
# Checks scripts/check-pipefail-pipes.sh against pipelines that end in a
# reader that stops early and ones that only look like it.
set -euo pipefail
cd "$(dirname "$0")/.."
d="$(mktemp -d)"; trap 'rm -rf "$d"' EXIT
fails=0
expect() { # expect flagged|pass FILE CONTENT
    printf '%s\n' "$3" > "$d/$2"
    if scripts/check-pipefail-pipes.sh "$d/$2" >/dev/null 2>&1; then got=pass; else got=flagged; fi
    if [ "$got" = "$1" ]; then echo "PASS: $2 $1"; else echo "FAIL: $2 expected $1, got $got"; fails=1; fi
}
expect flagged grep-q.sh 'if echo "$out" | grep -q "done"; then :; fi'
expect flagged grep-qF.sh 'cmd | grep -qF -- "$x" || die'
expect flagged grep-quiet.sh 'cmd | grep --quiet x'
expect flagged grep-l.sh 'cmd | grep -l x'
expect flagged grep-m.sh 'cmd | grep -m1 x'
expect flagged head.sh 'v="$(cmd --version | head -1)"'
expect flagged head-n.sh 'cmd | head -n 20'
expect flagged sed-q.sh 'cmd | sed -n "1p;q"'
expect flagged sed-1q.sh 'cmd | sed 1q'
expect flagged awk-exit.sh "cmd | awk 'NF { print; exit }'"
expect flagged continued.sh 'cmd \
    | grep -q x'
expect flagged trailing.sh 'cmd |
    grep -q x'
expect flagged trailing-cont.sh 'cmd | \
    head -1'
expect flagged subshell.sh 'cmd | ( grep -q x )'
expect flagged group.sh 'cmd | { grep -q x; }'
expect flagged assign.sh 'cmd | LC_ALL=C grep -q x'
expect flagged command.sh 'cmd | command grep -q x'
expect flagged timeout.sh 'cmd | timeout 5 head -1'
expect flagged pipe-amp.sh 'cmd |& grep -q x'
expect pass grep-null.sh 'if echo "$out" | grep -F "done" >/dev/null; then :; fi'
expect pass grep-file.sh 'grep -q x "$file" && cmd | grep -c y'
expect pass grep-then-lt.sh 'while ! cmd | grep x >/dev/null && [ "$i" -lt 9 ]; do :; done'
expect pass or.sh 'cmd || head -1 file'
expect pass sed.sh 'cmd | sed -n 1p | sed "s/q/x/"'
expect pass awk.sh "cmd | awk 'NF && !seen { print; seen = 1 }' || exit 1"
expect pass comment.sh '# end in grep >/dev/null, not | grep -q'
exit "$fails"
