#!/usr/bin/env bash
# check-ios-spawns.sh [ROOT]: fails on a process launch (`Command::new`,
# `posix_spawn`, `fork()`, `exec*()`) in the crates the iOS daemon links,
# since iOS lets an app start no other program, unless
# scripts/ios-spawns.allow names the function it is in. An entry is
# `<path> <fn>  # how it is kept off on iOS`, or `skip <path>  # why` for a
# module compiled out on iOS: its `mod` line must sit under a `#[cfg(not(…))]`
# naming `target_os = "ios"`. An entry with no launch left fails too.
# Test code is not scanned: files named `*_tests.rs` or `tests.rs`, and a
# top-level item under `#[cfg(test)]`. ROOT defaults to the repository.
set -euo pipefail
root="$(cd "${1:-$(dirname "$0")/..}" && pwd)"
cd "$root"
# clax-server and what it links outside its dev-dependencies, and clax-ios
# once it exists.
crates=(clax-core clax-embedded clax-http clax-ios clax-mcp clax-realtime clax-rt clax-sample clax-server clax-store)
files=()
for c in "${crates[@]}"; do
    [ -d "crates/$c/src" ] || continue
    while IFS= read -r -d '' f; do files+=("$f"); done \
        < <(find "crates/$c/src" -name '*.rs' ! -name '*_tests.rs' ! -name 'tests.rs' -print0)
done
perl - scripts/ios-spawns.allow "${files[@]}" <<'PERL'
use strict;
use warnings;
my $allow = shift;
my (%allow, %skip, %used, @bad);
open(my $a, '<', $allow) or die "$allow: cannot read: $!\n";
while (<$a>) {
    s/#.*//;
    s/^\s+|\s+$//g;
    next unless length;
    if (/^skip\s+(\S+)$/) { $skip{$1} = $. }
    elsif (/^(\S+)\s+(\w+)$/) { $allow{"$1 $2"} = $. }
    else { push @bad, "$allow:$.: neither `<path> <fn>` nor `skip <path>`" }
}
close $a;
for my $f (sort @ARGV) {
    next if $skip{$f};
    open(my $fh, '<', $f) or die "$f: cannot read: $!\n";
    my ($fn, $test, $pending) = ('(top level)', 0, 0);
    while (<$fh>) {
        if ($test) { $test = 0 if /^\}/; next }
        if (/^#\[cfg\(test\)\]/) { $pending = 1; next }
        if ($pending) {
            next if /^#\[/;
            $pending = 0;
            $test = 1 unless /;\s*$/;
            next;
        }
        (my $c = $_) =~ s{//.*}{};
        $fn = $1 if $c =~ /^\s*(?:pub(?:\([^)]*\))?\s+)?(?:const\s+)?(?:async\s+)?(?:unsafe\s+)?(?:extern\s+"C"\s+)?fn\s+(\w+)/;
        next unless $c =~ /\bCommand::new\b|\bposix_spawnp?\b|\bfork\s*\(\s*\)|\bexec(?:l|lp|le|v|vp|ve|vpe)\s*\(/;
        if ($allow{"$f $fn"}) { $used{"$f $fn"} = 1; next }
        (my $t = $_) =~ s/^\s+|\s+$//g;
        push @bad, "$f:$.: a process launch in `$fn`, which iOS refuses (gate it off on iOS and list it in $allow): $t";
    }
    close $fh;
}
for my $k (sort keys %allow) {
    next if $used{$k};
    my ($f, $fn) = split / /, $k;
    push @bad, "$allow:$allow{$k}: no process launch left in `$fn` of $f";
}
for my $f (sort keys %skip) {
    if (!-f $f) { push @bad, "$allow:$skip{$f}: no file $f"; next }
    (my $dir = $f) =~ s{/[^/]+$}{};
    (my $stem = $f) =~ s{^.*/|\.rs$}{}g;
    my $gated;
    for my $p (map { "$dir/$_" } qw(lib.rs main.rs mod.rs)) {
        open(my $ph, '<', $p) or next;
        my $prev = '';
        while (<$ph>) {
            if (/^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+\Q$stem\E\s*;/) {
                $gated = $prev =~ /^\s*#\[cfg\(not\(.*target_os\s*=\s*"ios"/ ? 1 : 0;
                last;
            }
            $prev = $_;
        }
        close $ph;
        last if defined $gated;
    }
    push @bad, "$f: skipped, but its `mod $stem;` is not under a `#[cfg(not(…))]` naming target_os = \"ios\"" unless $gated;
}
print "$_\n" for @bad;
exit(@bad ? 1 : 0);
PERL
