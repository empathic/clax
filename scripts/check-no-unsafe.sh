#!/usr/bin/env bash
# Fails, one line per finding, unless no Rust source under ROOT/crates
# (default: the repository) uses `unsafe` but the one exception AGENTS.md
# names, and nothing relaxes the `unsafe_code` lint:
#
#  - the workspace's Cargo.toml forbids `unsafe_code`;
#  - every crate's Cargo.toml takes the workspace's lints, but
#    crates/clax-embed's and the separate crates/clax-worker workspace's;
#  - crates/clax-embed/Cargo.toml, where there is one, denies
#    `unsafe_code` and forbids `unsafe_op_in_unsafe_fn`, so an unsafe
#    operation in the body of its `unsafe fn` exports needs an `unsafe`
#    block, which this check sees;
#  - in crates/clax-embed/src/ffi.rs the only `unsafe` is, for each of
#    clax_embed_open, clax_embed_call, clax_embed_subscribe and
#    clax_embed_close, once: a line `#[unsafe(no_mangle)]` directly after
#    `#[allow(unsafe_code)]` and directly before `pub unsafe extern "C" fn
#    <name>(` (open and call, which take pointers) or `pub extern "C" fn
#    <name>(` (subscribe and close), every parameter (on that line or the
#    lines up to its `)`) a `u64`, `i64`, `i32`, `usize`, `*const u8` or
#    `ClaxPost`, the grant on that signature line ending at its `(` (the
#    rest of the line is checked as any other); and, in open's and call's
#    bodies, exactly one line
#    `let <a> = unsafe { std::slice::from_raw_parts(<p>, <n>) };` directly
#    after a `// SAFETY:` comment line; `unsafe_code` appears nowhere else in
#    that crate;
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
# The exports of clax-embed: 1 for those declared `unsafe` (they take pointers).
my %EMBED = (clax_embed_open => 1, clax_embed_call => 1, clax_embed_subscribe => 0, clax_embed_close => 0);
my %EMBED_PARAM = map { $_ => 1 } ("u64", "i64", "i32", "usize", "*const u8", "ClaxPost");
my $SLICE = qr/^let \w+ = unsafe \{ std::slice::from_raw_parts\(\w+, \w+\) \};$/;
my %OWN_LINTS = map { $_ => 1 } qw(crates/clax-embed/Cargo.toml crates/clax-worker/Cargo.toml);

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
if (-e "crates/clax-embed/Cargo.toml") {
    finding("crates/clax-embed/Cargo.toml: must deny unsafe_code")
        unless level(manifest("crates/clax-embed/Cargo.toml"), "lints.rust.unsafe_code") eq "deny";
    finding("crates/clax-embed/Cargo.toml: must forbid unsafe_op_in_unsafe_fn")
        unless level(manifest("crates/clax-embed/Cargo.toml"), "lints.rust.unsafe_op_in_unsafe_fn") eq "forbid";
}

for my $path (@ARGV) {
    open(my $fh, "<", $path) or die "$path: $!";
    my @text = <$fh>;
    close $fh;
    my @code = map { (my $c = $_) =~ s{//.*}{}; $c =~ s/^\s+|\s+$//g; $c } @text;
    my $embed = $path =~ m{^crates/clax-embed/};

    # The lines of the four exports: their `#[allow(unsafe_code)]`, their
    # `#[unsafe(no_mangle)]` and the slices their bodies read.
    my (%granted, %named);
    if ($path eq "crates/clax-embed/src/ffi.rs") {
        for my $i (1 .. $#code - 1) {
            next unless $code[$i] eq "#[unsafe(no_mangle)]" && $code[$i - 1] eq "#[allow(unsafe_code)]";
            my ($marked, $name) = $code[$i + 1] =~ /^pub (unsafe )?extern "C" fn (\w+)\(/ or next;
            next unless exists $EMBED{$name} && ($marked ? 1 : 0) == $EMBED{$name} && !$named{$name}++;
            # The signature, from its line to the one closing its parameters.
            my $end = $i + 1;
            my $sig = $code[$end];
            while ($sig !~ /\)/ && $end < $#code) { $sig .= " " . $code[++$end] }
            my ($params) = $sig =~ /\(([^)]*)\)/ or next;
            next if grep { !/^\s*\w+\s*:\s*(.+?)\s*$/ || !$EMBED_PARAM{$1} } grep { /\S/ } split(/,/, $params);
            my %body;
            if ($EMBED{$name}) {
                # The body runs to the first line that is a lone `}`.
                my $close = $end;
                $close++ while $close < $#code && $text[$close] !~ /^\}\s*$/;
                my @slices = grep {
                    $code[$_] =~ $SLICE && $code[$_ - 1] eq "" && $text[$_ - 1] =~ m{^\s*// SAFETY:}
                } ($end + 1 .. $close - 1);
                next unless @slices;
                $body{$slices[0]} = 1;
            }
            $granted{$_} = 1 for ($i - 1, $i, keys %body);
            # The signature line up to its `(`; what follows is checked.
            $code[$i + 1] =~ s/^[^(]*\(//;
        }
    }
    for my $i (0 .. $#code) {
        next if $granted{$i};
        my $n = $i + 1;
        if ($code[$i] =~ /\bunsafe\b(?!-)/) {
            (my $line = $text[$i]) =~ s/\n$//;
            finding($embed ? "$path:$n: $ONLY" : "$path:$n: $line");
        } elsif ($embed && $code[$i] =~ /\bunsafe_code\b/) {
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
