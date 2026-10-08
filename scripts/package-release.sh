#!/usr/bin/env bash
# Packages release archives and their checksums.
#   package-release.sh archive <version> <target> <binary> <outdir>
#       writes <outdir>/clax-<version>-<target>.tar.gz, holding exactly
#       clax-<version>-<target>/clax; refuses a binary that does not report
#       "clax <version>"
#   package-release.sh sums <dir>
#       writes <dir>/SHA256SUMS for every other regular file in <dir>
set -euo pipefail

sha256() { if command -v sha256sum >/dev/null 2>&1; then sha256sum "$@"; else shasum -a 256 "$@"; fi; }

case "${1:-}" in
    archive)
        [ $# = 5 ] || { echo "usage: package-release.sh archive <version> <target> <binary> <outdir>" >&2; exit 2; }
        version="$2" target="$3" bin="$4" out="$5"
        name="clax-$version-$target"
        got="$("$bin" --version 2>/dev/null | sed -n 1p || true)"
        [ "$got" = "clax $version" ] || { echo "$bin reports '$got', not 'clax $version'" >&2; exit 1; }
        stage="$(mktemp -d)"
        trap 'rm -rf "$stage"' EXIT
        mkdir "$stage/$name"
        cp "$bin" "$stage/$name/clax"
        chmod 755 "$stage/$name/clax"
        mkdir -p "$out"
        # No extended attributes or AppleDouble files travel in the archive,
        # so nothing (a quarantine flag included) is restored on extraction.
        if [ "$(uname -s)" = Darwin ]; then
            COPYFILE_DISABLE=1 tar --no-mac-metadata --no-xattrs -czf "$out/$name.tar.gz" -C "$stage" "$name"
        else
            tar -czf "$out/$name.tar.gz" -C "$stage" "$name"
        fi
        echo "$out/$name.tar.gz"
        ;;
    sums)
        [ $# = 2 ] || { echo "usage: package-release.sh sums <dir>" >&2; exit 2; }
        cd "$2"
        files=()
        for f in *; do
            if [ -f "$f" ] && [ "$f" != SHA256SUMS ]; then files+=("$f"); fi
        done
        [ ${#files[@]} -gt 0 ] || { echo "no files to sum in $2" >&2; exit 1; }
        sha256 "${files[@]}" > SHA256SUMS.tmp
        mv SHA256SUMS.tmp SHA256SUMS
        echo "$2/SHA256SUMS"
        ;;
    *)
        echo "usage: package-release.sh archive <version> <target> <binary> <outdir> | sums <dir>" >&2
        exit 2
        ;;
esac
