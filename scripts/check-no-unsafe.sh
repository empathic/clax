#!/usr/bin/env bash
# Fails, one line per finding, unless no Rust source under ROOT/crates
# (default: the repository) uses `unsafe` but the one exception AGENTS.md
# names, and nothing relaxes the `unsafe_code` lint:
#
#  - the workspace's Cargo.toml forbids `unsafe_code`;
#  - every crate's Cargo.toml takes the workspace's lints, but
#    crates/clax-ios's and the separate crates/clax-worker workspace's;
#  - crates/clax-ios/Cargo.toml, when there is one, denies `unsafe_code`;
#  - in crates/clax-ios/src/lib.rs the only `unsafe` is a line
#    `#[unsafe(no_mangle)]` directly after `#[allow(unsafe_code)]` and
#    directly before `pub extern "C" fn <name>(` on one line, with `<name>`
#    one of clax_start, clax_stop, clax_state and clax_trim_memory, each
#    once, and every parameter a `u16` or an `i32`; `unsafe_code` appears
#    nowhere else in that crate;
#  - no other source uses `unsafe`.
#
# A source line counts only where the word appears before any `//` comment,
# and not as the start of a hyphenated word such as CSP's `'unsafe-inline'`.
set -euo pipefail
cd "${1:-$(dirname "$0")/..}"

# shellcheck disable=SC2016 # Perl, not shell, expands these.
check='
use strict;
use warnings;

my $bad = 0;
sub finding { print "$_[0]\n"; $bad = 1 }
my $ONLY = "only the four exported functions may use unsafe";
my %EXPORT = map { $_ => 1 } qw(clax_start clax_stop clax_state clax_trim_memory);
my %OWN_LINTS = map { $_ => 1 } qw(crates/clax-ios/Cargo.toml crates/clax-worker/Cargo.toml);

# A manifest as {"table.key" => value}, for the plain TOML Cargo.toml uses:
# one key per line, [table] headers, # comments outside strings.
sub manifest {
    my ($path) = @_;
    my %keys;
    open(my $fh, "<", $path) or return undef;
    my $table = "";
    while (my $line = <$fh>) {
        $line =~ s/^\s+|\s+$//g;
        next if $line eq "" || $line =~ /^#/;
        if ($line =~ /^\[\[/) { $table = "[[array]]"; next }
        if ($line =~ /^\[\s*([^\]]+?)\s*\]/) { ($table = $1) =~ s/\s*\.\s*/./g; next }
        if ($line =~ /^([\w.\-" ]+?)\s*=\s*(.*?)\s*(?:#[^"]*)?$/) {
            (my $key = $1) =~ s/[\s"]//g;
            $keys{$table eq "" ? $key : "$table.$key"} = $2;
        }
    }
    return \%keys;
}

# The value of KEY, also when its parent is an inline table.
sub value {
    my ($keys, $key) = @_;
    return $keys->{$key} if exists $keys->{$key};
    my ($parent, $leaf) = $key =~ /^(.*)\.([^.]+)$/ or return undef;
    my $inline = value($keys, $parent) // return undef;
    return $inline =~ /^\{.*\b\Q$leaf\E\s*=\s*([^,}]+?)\s*[,}]/ ? $1 : undef;
}

# The level of a lint: "deny" or { level = "deny", ... }.
sub level {
    my $v = value(@_) // return "";
    return $v =~ /^"(\w+)"$/ ? $1 : $v =~ /\blevel\s*=\s*"(\w+)"/ ? $1 : "";
}

my $workspace = manifest("Cargo.toml");
finding("Cargo.toml: the workspace must forbid unsafe_code")
    unless $workspace && level($workspace, "workspace.lints.rust.unsafe_code") eq "forbid";

for my $path (sort glob("crates/*/Cargo.toml")) {
    next if $OWN_LINTS{$path};
    my $keys = manifest($path);
    my $takes = (value($keys, "lints.workspace") // "") eq "true"
        && !grep { /^lints\./ && $_ ne "lints.workspace" } keys %$keys;
    finding("$path: does not take the workspace\x27s lints") unless $takes;
}
if (-e "crates/clax-ios/Cargo.toml") {
    finding("crates/clax-ios/Cargo.toml: must deny unsafe_code")
        unless level(manifest("crates/clax-ios/Cargo.toml"), "lints.rust.unsafe_code") eq "deny";
}

for my $path (@ARGV) {
    open(my $fh, "<", $path) or die "$path: $!";
    my @text = <$fh>;
    close $fh;
    my @code = map { (my $c = $_) =~ s{//.*}{}; $c =~ s/^\s+|\s+$//g; $c } @text;
    my $ios = $path =~ m{^crates/clax-ios/};

    # The lines of the four exports: their `#[allow(unsafe_code)]` and
    # their `#[unsafe(no_mangle)]`.
    my (%granted, %named);
    if ($path eq "crates/clax-ios/src/lib.rs") {
        for my $i (1 .. $#code - 1) {
            next unless $code[$i] eq "#[unsafe(no_mangle)]" && $code[$i - 1] eq "#[allow(unsafe_code)]";
            my ($name, $params) = $code[$i + 1] =~ /^pub extern "C" fn (\w+)\(([^)]*)\)/ or next;
            next unless $EXPORT{$name} && !$named{$name}++;
            next if grep { !/^\s*\w+\s*:\s*(?:u16|i32)\s*$/ } grep { /\S/ } split(/,/, $params);
            $granted{$i} = $granted{$i - 1} = 1;
        }
    }
    for my $i (0 .. $#code) {
        next if $granted{$i};
        my $n = $i + 1;
        if ($code[$i] =~ /\bunsafe\b(?!-)/) {
            (my $line = $text[$i]) =~ s/\n$//;
            finding($path eq "crates/clax-ios/src/lib.rs" ? "$path:$n: $ONLY" : "$path:$n: $line");
        } elsif ($ios && $code[$i] =~ /\bunsafe_code\b/) {
            finding("$path:$n: $ONLY");
        }
    }
}
exit($bad ? 1 : 0);
'

sources=()
while IFS= read -r -d '' f; do sources+=("$f"); done \
    < <(find crates -name "*.rs" -not -path "*/target/*" -print0 | sort -z)
perl -e "$check" -- ${sources[@]+"${sources[@]}"}
