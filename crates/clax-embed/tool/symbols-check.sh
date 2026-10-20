#!/usr/bin/env bash
# symbols-check.sh NM LIB...: fails unless each shared library LIB defines
# exactly the four functions of include/clax_embed.h among the text symbols
# it exports (NM is the NDK's llvm-nm, run with -D --defined-only).
set -euo pipefail
nm="$1"
shift
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
done
exit "$status"
