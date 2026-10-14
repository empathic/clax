#!/usr/bin/env bash
# Fails when a Rust source under crates/ uses `unsafe` (the `unsafe_code`
# lint forbids it too); a line counts only where the word appears before
# any `//` comment, and not as the start of a hyphenated word such as CSP's
# `'unsafe-inline'`.
set -euo pipefail
cd "$(dirname "$0")/.."
find crates -name "*.rs" -not -path "*/target/*" -print0 |
    xargs -0 perl -ne '(my $c = $_) =~ s{//.*}{}; if ($c =~ /\bunsafe\b(?!-)/) { print "$ARGV:$.: $_"; $bad = 1 } close ARGV if eof; END { exit($bad ? 1 : 0) }'
