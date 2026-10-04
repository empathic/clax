#!/usr/bin/env bash
# Builds the web UI (`npm run build` in web/), then gives each file in
# web/dist whose content the build did not change its previous modification
# time back. A release build embeds web/dist and cargo goes by modification
# times, so an identical web UI does not rebuild the release binary. When
# the build is done it touches web/node_modules/.clax-web-built, the build's
# time for checks that compare web/dist with its sources.
#
# Usage: scripts/build-web.sh
set -euo pipefail
cd "$(dirname "$0")/../web"
SNAP="$(mktemp "${TMPDIR:-/tmp}/clax-web-dist.XXXXXX")"
trap 'rm -f "$SNAP"' EXIT
# path, SHA-256 and modification time of every file under dist, one per line.
perl -MFile::Find -MDigest::SHA -MTime::HiRes=stat -e '
    find({ no_chdir => 1, wanted => sub {
        return unless -f $_ && !-l $_;
        my $sha = Digest::SHA->new(256)->addfile($_)->hexdigest;
        printf "%s\t%s\t%.9f\n", $_, $sha, (stat $_)[9];
    } }, "dist");' > "$SNAP"
npm run build
perl -MDigest::SHA -MTime::HiRes=utime -e '
    open my $f, "<", $ARGV[0] or die "$ARGV[0]: $!";
    while (<$f>) {
        chomp;
        my ($path, $sha, $mtime) = split /\t/;
        next unless -f $path && !-l $path;
        next unless Digest::SHA->new(256)->addfile($path)->hexdigest eq $sha;
        utime undef, $mtime, $path or die "$path: $!";
    }' "$SNAP"
touch node_modules/.clax-web-built
