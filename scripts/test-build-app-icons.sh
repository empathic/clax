#!/usr/bin/env bash
# Checks scripts/build-app-icons.sh on a copy of its sources and outputs: the
# committed outputs are in step; an edited mark, or an edited colour the
# icons use, makes them stale while a colour they do not use does not; a
# hand-edited generated file, a missing hash and a missing render are
# stale; --check writes nothing and works from any directory; a run without
# rsvg-convert fails naming it before writing anything; a run renders every
# listed PNG at its size, writes what --check accepts, and a second run
# changes no generated text; and, where rsvg-convert is found, the real
# renders have the sizes their names give.
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1
ROOT="$PWD"
d="$(mktemp -d)"
trap 'rm -rf "$d"' EXIT
fails=0
_fail() { echo "FAIL: $1" >&2; fails=$((fails + 1)); }
# shellcheck source=scripts/fake-exe.sh
. scripts/fake-exe.sh

SCRIPT=scripts/build-app-icons.sh
INPUTS=(design/mark.json design/tokens.json "$SCRIPT")
HASH=apps/clax_app/icons/SOURCE.sha256

# Every output, as the script lists it: TEXT (compared byte for byte) and
# PNG (rendered; only the source hash covers them).
listed="$(env -u TEST_SRCDIR -u TEST_WORKSPACE bash "$SCRIPT" --list)" || {
    echo "FAIL: $SCRIPT --list" >&2
    exit 1
}
TEXTS=()
PNGS=()
while read -r kind path size; do
    case "$kind" in
        text) TEXTS+=("$path") ;;
        png) PNGS+=("$path $size") ;;
    esac
done <<<"$listed"
PNG_PATHS=()
for p in "${PNGS[@]}"; do PNG_PATHS+=("${p% *}"); done
[ "${#TEXTS[@]}" -gt 0 ] && [ "${#PNGS[@]}" -gt 0 ] || _fail "--list names text and PNG outputs: $listed"

_copy() {
    rm -rf "$d/w" && mkdir -p "$d/w"
    for f in "${INPUTS[@]}" "${TEXTS[@]}" "${PNG_PATHS[@]}"; do
        [ -f "$f" ] || continue
        mkdir -p "$d/w/$(dirname "$f")" && cp "$f" "$d/w/$f"
    done
}
# _run [ARGS]: the copy's script on the copy; sets out and st.
_run() {
    out="$(env -u TEST_SRCDIR -u TEST_WORKSPACE RSVG_CONVERT="${RSVG:-}" bash "$d/w/$SCRIPT" "$@" 2>&1)"
    st=$?
}
_has() { grep -qF -- "$1" <<<"$out"; }
# _json FILE PYTHON: runs PYTHON on FILE's JSON (as t) and writes it back.
_json() {
    python3 - "$1" "$2" <<'EOF'
import json, sys
f, code = sys.argv[1:]
t = json.load(open(f))
exec(code)
open(f, "w").write(json.dumps(t, indent=2) + "\n")
EOF
}
_tree_sum() { (cd "$d/w" && find . -type f | sort | xargs shasum -a 256); }

# 1. The committed outputs are what the source writes.
_copy
_run --check
[ "$st" = 0 ] || _fail "the committed outputs are in step: $out"

# 2. An edited mark makes the hash and the vector drawables stale.
_copy
_json "$d/w/design/mark.json" 't["dot"]["r"] = 1.8'
_run --check
{ [ "$st" = 1 ] && _has "$HASH" && _has ic_launcher_fg.xml && _has "run $SCRIPT"; } ||
    _fail "an edited mark is stale: $out"

# 3. An edited colour the icons use is stale; one they do not use is not.
_copy
_json "$d/w/design/tokens.json" 't["color"]["you"]["dark"] = "#ff7a58"'
_run --check
{ [ "$st" = 1 ] && _has "$HASH"; } || _fail "an edited icon colour is stale: $out"
_copy
_json "$d/w/design/tokens.json" 't["color"]["danger"]["light"] = "#b3261f"'
_run --check
[ "$st" = 0 ] || _fail "a colour the icons do not use changes nothing: $out"

# 4. A hand-edited generated file is stale, naming it.
_copy
mono=apps/clax_app/android/app/src/main/res/drawable/ic_launcher_mono.xml
printf '<!-- edited -->\n' >> "$d/w/$mono"
_run --check
{ [ "$st" = 1 ] && _has "$mono"; } || _fail "a hand-edited output is stale: $out"

# 5. A missing hash, or a missing render, is stale.
_copy
rm -f "$d/w/$HASH"
_run --check
{ [ "$st" = 1 ] && _has "$HASH"; } || _fail "a missing hash is stale: $out"
_copy
rm -f "$d/w/${PNG_PATHS[0]}"
_run --check
{ [ "$st" = 1 ] && _has "${PNG_PATHS[0]} (missing)"; } || _fail "a missing render is stale: $out"

# 6. --check writes nothing, and works from another directory.
_copy
_json "$d/w/design/mark.json" 't["stroke"] = 2.7'
before="$(_tree_sum)"
out="$(cd / && env -u TEST_SRCDIR -u TEST_WORKSPACE bash "$d/w/$SCRIPT" --check 2>&1)"
st=$?
[ "$st" = 1 ] || _fail "--check from / sees the copy's stale outputs: $out"
[ "$(_tree_sum)" = "$before" ] || _fail "--check wrote a file"
_copy
out="$(cd / && env -u TEST_SRCDIR -u TEST_WORKSPACE bash "$d/w/$SCRIPT" --check 2>&1)"
st=$?
[ "$st" = 0 ] || _fail "--check works from /: $out"

# 7. Without rsvg-convert a run fails naming it, and writes nothing.
_copy
_json "$d/w/design/mark.json" 't["stroke"] = 2.7'
before="$(_tree_sum)"
RSVG=/nonexistent/rsvg-convert _run
{ [ "$st" = 1 ] && _has rsvg-convert; } || _fail "a run without rsvg-convert names it: $out"
[ "$(_tree_sum)" = "$before" ] || _fail "a run without rsvg-convert wrote a file"

# 8. A run renders every PNG at its size (a stand-in renderer writes the
# size it was asked for), writes what --check accepts, and a second run
# changes no generated text.
fake_exe "$d/bin/rsvg-convert" <<'EOF'
#!/bin/sh
# rsvg-convert -w W -h H -o OUT IN: writes "W H" into OUT.
w= h= o=
while [ $# -gt 1 ]; do
    case "$1" in
        -w) w="$2"; shift 2 ;;
        -h) h="$2"; shift 2 ;;
        -o) o="$2"; shift 2 ;;
        *) shift ;;
    esac
done
printf '%s %s\n' "$w" "$h" > "$o"
EOF
_copy
for f in "${TEXTS[@]}" "${PNG_PATHS[@]}"; do rm -f "$d/w/$f"; done
RSVG="$d/bin/rsvg-convert" _run
[ "$st" = 0 ] || _fail "a run succeeds: $out"
for p in "${PNGS[@]}"; do
    path="${p% *}" size="${p##* }"
    got="$(cat "$d/w/$path" 2>/dev/null)"
    [ "$got" = "$size $size" ] || _fail "$path rendered at $size (got: ${got:-nothing})"
done
_run --check
[ "$st" = 0 ] || _fail "--check accepts a run's outputs: $out"
mkdir -p "$d/first"
for f in "${TEXTS[@]}"; do cp "$d/w/$f" "$d/first/$(echo "$f" | tr / _)"; done
RSVG="$d/bin/rsvg-convert" _run
for f in "${TEXTS[@]}"; do
    cmp -s "$d/w/$f" "$d/first/$(echo "$f" | tr / _)" || _fail "a second run changed $f"
    [ ! -f "$f" ] || cmp -s "$d/w/$f" "$f" || _fail "the committed $f differs from a fresh run's"
done

# 9. The real renderer, where the script finds one, writes PNGs of the
# listed sizes.
_copy
RSVG="" _run
if [ "$st" = 1 ] && _has "rsvg-convert not found"; then
    echo "SKIP: no rsvg-convert, so no real render"
else
    [ "$st" = 0 ] || _fail "a real run succeeds: $out"
    for p in "${PNGS[@]}"; do
        path="${p% *}" size="${p##* }"
        got="$(python3 - "$d/w/$path" <<'EOF'
import struct, sys
b = open(sys.argv[1], "rb").read(24)
print(*struct.unpack(">II", b[16:24])) if b[:8] == b"\x89PNG\r\n\x1a\n" else print("not a PNG")
EOF
)"
        [ "$got" = "$size $size" ] || _fail "$path is ${size}x$size (got: $got)"
    done
fi

[ "$fails" = 0 ] && echo "PASS: build-app-icons.sh"
exit "$fails"
