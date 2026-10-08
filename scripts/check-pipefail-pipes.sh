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
set -euo pipefail
cd "$(dirname "$0")/.."

if [ $# -gt 0 ]; then
    files=("$@")
    sourced_by_pipefail=()
    all=1
else
    shell_files=()
    while IFS= read -r -d '' f; do
        case "$f" in scripts/check-pipefail-pipes.sh|scripts/test-check-pipefail-pipes.sh) ;; *) shell_files+=("$f") ;; esac
    done < <(git ls-files -z -- '*.sh' '*.bash' 'justfile' '**/justfile')
    files=("${shell_files[@]}")
    # The names of the scripts that pipefail scripts source.
    sourced_by_pipefail=()
    while IFS= read -r name; do sourced_by_pipefail+=("$name"); done < <(
        perl -ne '
            $pf = 1 if /pipefail/;
            push @names, $1 if /(?:^|[\s;&(-])(?:\.|source)\s+["\x27]?(?:\$\{?\w+\}?\/)?(?:[\w.-]+\/)*([\w.-]+\.(?:sh|bash))\b/;
            if (eof) { print "$_\n" for ($pf ? @names : ()); @names = (); $pf = 0; close ARGV }
        ' "${files[@]}" | sort -u)
    all=0
fi

ALL="$all" SOURCED="${sourced_by_pipefail[*]:-}" perl -ne '
    BEGIN { %sourced = map { $_ => 1 } split / /, ($ENV{SOURCED} // "") }
    # Read each file whole first, to know whether it runs under pipefail.
    push @lines, $_;
    next unless eof;
    my ($base) = $ARGV =~ m{([^/]+)$};
    my $pf = $ENV{ALL} || $sourced{$base} || grep { /pipefail/ } @lines;
    if ($pf) {
        for my $i (0 .. $#lines) {
            my $l = $lines[$i];
            next if $l =~ /^\s*#/;
            # Each command after a single pipe (not ||), up to the next one.
            while ($l =~ /(?<!\|)\|(?![|&])\s*([^|]*)/g) {
                my $c = $1;
                # grep options, up to a list operator or closing parenthesis.
                my ($g) = $c =~ /^[ef]?grep\b([^;&)]*)/;
                # The first quoted program of an awk or sed, or a bare sed
                # command such as `sed 1q`.
                my ($awk) = grep { defined } $c =~ /^awk\b[^\x27"]*(?:\x27([^\x27]*)\x27|"([^"]*)")/;
                my ($sed) = grep { defined } $c =~ /^sed\b[^\x27"]*(?:\x27([^\x27]*)\x27|"([^"]*)")/;
                $sed //= $1 if $c =~ /^sed\s+(?:-n\s+)?(\S+)/;
                my $bad =
                    (defined $g && $g =~ /\s-[a-zA-Z]*[qlLm]|\s--(?:quiet|silent|max-count|files-with(?:out)?-match)/)
                    || $c =~ /^head\b/
                    || (defined $awk && $awk =~ /\bexit\b/)
                    || (defined $sed && $sed =~ /(?:^|[\s;{}\d\/\$])[qQ]\d*\s*(?:[;}]|$)/);
                if ($bad) {
                    print "$ARGV:", $i + 1, ": $l";
                    $found = 1;
                    last;
                }
            }
        }
    }
    @lines = ();
    close ARGV;
    END {
        if ($found) {
            print STDERR "a reader that stops early (grep -q, head, sed q, awk exit) can fail a pipefail pipeline by SIGPIPE; read all the input instead (grep ... >/dev/null, sed -n 1p)\n";
            exit 1;
        }
    }
' "${files[@]}"
