#!/usr/bin/env bash
# Fails when a tracked source file or script makes a hard link: a call of
# `hard_link` (Rust), `link`/`linkSync` (Node), `os.link` (Python) or
# `linkat`, an `ln` (or a `$..._LN` variable naming it) without -s, or a
# `cp -l`. Clax makes no hard links, in its code, its tests or its scripts;
# a symbolic link does instead. With file arguments, checks those files
# (for trying the check out); without, every tracked file of the kinds
# below.
set -euo pipefail
cd "$(dirname "$0")/.."

if [ $# -gt 0 ]; then
    files=("$@")
else
    files=()
    while IFS= read -r -d '' f; do
        [ "$f" = scripts/check-no-hard-links.sh ] || files+=("$f")
    done < <(git ls-files -z -- '*.rs' '*.ts' '*.tsx' '*.js' '*.mjs' '*.cjs' '*.svelte' \
        '*.py' '*.sh' '*.bash' 'justfile' '**/justfile' 'Makefile' '**/Makefile')
fi

perl -ne '
    my $bad =
        # Rust std::fs::hard_link, libc/nix linkat, a hard-link crate.
        /\bhard_?link\b/i
        # Node fs.link / fs.linkSync / promises.link, Python os.link, nix
        # or libc link and linkat, and link imported from Node fs or Python
        # os; "symlinkSync" and "unlinkSync" have no word boundary before
        # "link".
        || /\b(?:fs|promises|os|unistd|libc)\s*(?:\.|::)\s*link(?:at)?\s*\(/
        || /\blinkSync\b|\blinkat\s*\(/
        || /\blink\b.*\bfrom\s+["\x27](?:node:)?fs(?:\/promises)?["\x27]/
        || /\bfrom\s+os\s+import\b.*\blink\b/
        # cp -l / cp --link.
        || /(?:^|[\s;&|(`"\x27])cp\s+(?:-[a-zA-Z]*l[a-zA-Z]*|--link)\b/;
    # ln, or a variable naming it, whose options do not include -s.
    while (!$bad && /(?:^|[\s;&|(`"\x27])(?:ln|\$\{?\w*_LN\}?)"?((?:\s+-[-\w]+)*)(?=\s|$)/g) {
        my $opts = $1;
        $bad = 1 unless $opts =~ /\s-[a-zA-Z]*s|--symbolic/;
    }
    if ($bad) { print "$ARGV:$.: $_"; $found = 1 }
    close ARGV if eof;
    END {
        if ($found) {
            print STDERR "hard links are not allowed in Clax; use a symbolic link\n";
            exit 1;
        }
    }
' "${files[@]}"
