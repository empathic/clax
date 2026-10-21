#!/usr/bin/env bash
# symbols-check.sh NM LIB...: fails unless each shared library LIB defines
# exactly the four functions of include/clax_embed.h among the text symbols
# it exports (NM is the NDK's llvm-nm, run with -D --defined-only), and
# unless every PT_LOAD segment of LIB is aligned to at least 16 KB (0x4000),
# which a phone with 16 KB pages needs to load it and Google Play needs of an
# app targeting SDK 35 or later. The alignments are read with the NDK's
# llvm-readelf, in NM's directory.
set -euo pipefail
nm="$1"
shift
readelf="$(dirname "$nm")/llvm-readelf"
want="clax_embed_call
clax_embed_close
clax_embed_open
clax_embed_subscribe"
status=0
for lib in "$@"; do
  syms="$("$nm" -D --defined-only "$lib")"
  got="$(awk '$2 == "T" && $3 ~ /^clax_embed_/ { print $3 }' <<<"$syms" | sort)"
  if [ "$got" != "$want" ]; then
    echo "$lib: exports $(tr '\n' ' ' <<<"$got")instead of $(tr '\n' ' ' <<<"$want")" >&2
    status=1
  else
    echo "$lib: the four clax_embed_ functions"
  fi
  headers="$("$readelf" -lW "$lib")"
  aligns="$(awk '$1 == "LOAD" { print $NF }' <<<"$headers")"
  low="$(while read -r a; do if [ -z "$a" ] || (( a < 0x4000 )); then echo "${a:-none}"; fi; done <<<"$aligns")"
  if [ -z "$aligns" ] || [ -n "$low" ]; then
    echo "$lib: PT_LOAD alignments $(tr '\n' ' ' <<<"$aligns")are not all at least 0x4000" >&2
    status=1
  else
    echo "$lib: every PT_LOAD aligned to at least 16 KB ($(tr '\n' ' ' <<<"$aligns" | sed 's/ $//'))"
  fi
done
exit "$status"
