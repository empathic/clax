#!/usr/bin/env bash
# Checks scripts/sync-tokens.py on a copy of its inputs and outputs: the
# committed outputs are in step; an edited source makes the generated files
# stale; a contrast pair below its minimum fails both modes; an edited haiku
# copy is stale; an app-mode colour in web/shell/src/theme.css that differs
# from the source fails; a plain run is stable (a second run changes no
# byte); an edited value in kit.css's generated region is stale; a kit.css
# without its end marker fails; a color-mix() in kit.css outside an
# @supports block fails; an edited design/mark.css makes the home app's
# mark.css stale, an edited design/mark.json the Dart geometry; a mark
# path with a command other than M or A fails; a kit token the source maps
# that kit.css declares again outside its generated region fails (but not
# inside @supports); a color-mix() outside @supports in the home app's
# home.css or the map's stylesheet in map.js fails; a contrast pair
# below its minimum in a more-contrast palette fails; and a reel curve in
# web/reel/pace.ts that is not the motion curve fails.
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1
d="$(mktemp -d)"
trap 'rm -rf "$d"' EXIT
fails=0
_fail() { echo "FAIL: $1" >&2; fails=$((fails + 1)); }

INPUTS=(
    design/mark.css
    design/mark.json
    design/tokens.json
    scripts/sync-tokens.py
    web/reel/pace.ts
    web/shell/src/view/haiku.json
    web/shell/src/theme.css
)
OUTPUTS=(
    apps/clax_app/home/tokens.css
    apps/clax_app/lib/ui/tokens.g.dart
    apps/clax_app/assets/haiku.json
    apps/clax_app/home/mark.css
)
# The kit's stylesheet: only the text between its markers is generated.
KIT=apps/clax_app/assets/kit/kit.css
# Checked, never written: their color-mix() declarations.
HOME=apps/clax_app/home/home.css
MAP=apps/clax_app/assets/kit/map.js

_copy() {
    rm -rf "$d/w" && mkdir -p "$d/w"
    for f in "${INPUTS[@]}" "${OUTPUTS[@]}" "$KIT" "$HOME" "$MAP"; do
        [ -f "$f" ] || continue
        mkdir -p "$d/w/$(dirname "$f")" && cp "$f" "$d/w/$f"
    done
}
# _run [ARGS]: the copy's script on the copy; sets out and st. The script
# resolves its root from TEST_SRCDIR under Bazel, so the copy's run has it
# unset.
_run() {
    out="$(env -u TEST_SRCDIR -u TEST_WORKSPACE python3 "$d/w/scripts/sync-tokens.py" "$@" 2>&1)"
    st=$?
}
# _sub FILE FROM TO: replaces every FROM in FILE with TO.
_sub() {
    python3 - "$@" <<'EOF'
import sys
f, a, b = sys.argv[1:]
t = open(f).read()
assert a in t, a
open(f, "w").write(t.replace(a, b))
EOF
}
# _has TEXT: TEXT is in the last run's output.
_has() { grep -qF -- "$1" <<<"$out"; }

# 1. The committed outputs are what the source writes.
_copy
_run --check
[ "$st" = 0 ] || _fail "the committed outputs are in step: $out"

# 2. An edited colour makes both generated files stale.
_copy
_sub "$d/w/design/tokens.json" '#f7f7f5' '#f7f7f6'
_run --check
{ [ "$st" = 1 ] && _has apps/clax_app/home/tokens.css && _has apps/clax_app/lib/ui/tokens.g.dart; } ||
    _fail "an edited source makes the outputs stale: $out"

# 3. A pair below its minimum fails in both modes, naming it and its ratio.
for mode in --check write; do
    _copy
    python3 - "$d/w/design/tokens.json" <<'EOF'
import json, sys
f = sys.argv[1]
t = json.load(open(f))
t["color"]["muted"]["light"] = "#b0aea8"
open(f, "w").write(json.dumps(t, indent=2) + "\n")
EOF
    if [ "$mode" = write ]; then _run; else _run --check; fi
    ratio="$(sed -n 's/^contrast: muted on card (light) is \([0-9.]*\), below 4.5$/\1/p' <<<"$out")"
    { [ "$st" = 1 ] && [ -n "$ratio" ] && python3 -c "import sys; sys.exit(not float('$ratio') < 4.5)"; } ||
        _fail "low contrast fails ($mode): $out"
done

# 4. An edited haiku copy is stale.
_copy
echo "extra" >> "$d/w/apps/clax_app/assets/haiku.json"
_run --check
{ [ "$st" = 1 ] && _has apps/clax_app/assets/haiku.json; } || _fail "an edited haiku copy is stale: $out"

# 5. An app-mode colour in theme.css that differs from the source fails.
_copy
_sub "$d/w/web/shell/src/theme.css" '--you: #e0532f' '--you: #e0532e'
_run --check
{ [ "$st" = 1 ] && _has "theme.css: --you is #e0532e, design/tokens.json says #e0532f"; } ||
    _fail "a drifted app-mode colour fails: $out"

# 6. A plain run writes what --check accepts, and a second run changes no byte.
_copy
for f in "${OUTPUTS[@]}"; do rm -f "$d/w/$f"; done
_run
[ "$st" = 0 ] || _fail "a plain run succeeds: $out"
_run --check
[ "$st" = 0 ] || _fail "--check accepts a plain run's outputs: $out"
mkdir -p "$d/first"
for f in "${OUTPUTS[@]}"; do cp "$d/w/$f" "$d/first/$(basename "$f")"; done
_run
for f in "${OUTPUTS[@]}"; do
    cmp -s "$d/w/$f" "$d/first/$(basename "$f")" || _fail "a second run changed $f"
done
for f in "${OUTPUTS[@]}" "$KIT"; do
    [ ! -f "$f" ] || cmp -s "$d/w/$f" "$f" || _fail "the committed $f differs from a fresh run's"
done

# 7. A value edited in kit.css's generated region is stale, and a plain run
# puts it back.
_copy
_sub "$d/w/$KIT" '  --k-bg: #f7f7f5;' '  --k-bg: #f7f7f6;'
_run --check
{ [ "$st" = 1 ] && _has "$KIT"; } || _fail "an edited kit region is stale: $out"
_run
{ [ "$st" = 0 ] && cmp -s "$d/w/$KIT" "$KIT"; } || _fail "a plain run restores the kit region: $out"

# 8. A kit.css without its end marker fails in both modes.
for mode in --check write; do
    _copy
    _sub "$d/w/$KIT" '/* tokens:end */' ''
    if [ "$mode" = write ]; then _run; else _run --check; fi
    { [ "$st" = 1 ] && _has "kit.css: tokens:end marker missing"; } || _fail "a missing end marker fails ($mode): $out"
done

# 9. A color-mix() outside an @supports block fails: a declaration that uses
# var() is accepted when parsed, so a WebView without color-mix() drops it
# only when computing and a plain fallback before it never applies.
_copy
_sub "$d/w/$KIT" '/* Base. */' '.k-x { background: color-mix(in srgb, var(--k-accent) 10%, transparent); }
/* Base. */'
_run --check
{ [ "$st" = 1 ] && _has "kit.css: color-mix() outside @supports at line"; } || _fail "a bare color-mix fails: $out"

# 10. An edited design/mark.css makes the home app's copy stale, and a plain
# run copies it.
_copy
_sub "$d/w/design/mark.css" 'stroke-width: 2.6;' 'stroke-width: 2.4;'
_run --check
{ [ "$st" = 1 ] && _has "apps/clax_app/home/mark.css is stale"; } || _fail "an edited mark.css is stale: $out"
_run
{ [ "$st" = 0 ] && grep -qF 'stroke-width: 2.4;' "$d/w/apps/clax_app/home/mark.css"; } ||
    _fail "a plain run copies mark.css: $out"

# 11. An edited mark path makes the Dart geometry stale.
_copy
_sub "$d/w/design/mark.json" '0 0 1 0 11' '0 0 1 0 10'
_run --check
{ [ "$st" = 1 ] && _has "apps/clax_app/lib/ui/tokens.g.dart is stale" && _has "Offset(1.5, 12.5)"; } ||
    _fail "an edited mark path is stale: $out"

# 12. A mark path with a command other than M or A fails, naming it.
_copy
_sub "$d/w/design/mark.json" 'M1.5 2.5a5.5' 'M1.5 2.5l0 1a5.5'
_run --check
{ [ "$st" = 1 ] && _has "design/mark.json: you.d has a command other than M or A"; } ||
    _fail "a mark path with another command fails: $out"

# 13. A kit token the source maps, declared again below the generated
# region, overrides the brand: it fails, naming it and its line. Inside an
# @supports block (the tints that follow an app's own accent) it does not.
_copy
_sub "$d/w/$KIT" '/* Base. */' ':root { --k-bg: #ffffff; }
/* Base. */'
_run --check
{ [ "$st" = 1 ] && _has "kit.css: --k-bg is declared outside its generated region at line"; } ||
    _fail "a mapped kit token declared again fails: $out"
_copy
_sub "$d/w/$KIT" '/* Base. */' '@supports (color: color-mix(in srgb, red, red)) { :root { --k-accent-soft: color-mix(in srgb, var(--k-accent) 9%, transparent); } }
/* Base. */'
_run --check
[ "$st" = 0 ] || _fail "a mapped kit token inside @supports passes: $out"

# 14. A color-mix() outside @supports in home.css fails.
_copy
printf '%s\n' '.h-x { background: color-mix(in srgb, var(--fg) 10%, transparent); }' >> "$d/w/$HOME"
_run --check
{ [ "$st" = 1 ] && _has "home.css: color-mix() outside @supports at line"; } || _fail "a bare color-mix in home.css fails: $out"

# 15. A color-mix() outside @supports in map.js's stylesheet fails.
_copy
_sub "$d/w/$MAP" '.k-map:active {' '.k-map-x { background: color-mix(in srgb, red 10%, transparent); }
.k-map:active {'
_run --check
{ [ "$st" = 1 ] && _has "map.js: color-mix() outside @supports at line"; } || _fail "a bare color-mix in map.js fails: $out"

# 16. A pair below its minimum only with more contrast fails, naming the
# palette.
_copy
python3 - "$d/w/design/tokens.json" <<'EOF'
import json, sys
f = sys.argv[1]
t = json.load(open(f))
t["contrastMore"]["muted"] = "border"
open(f, "w").write(json.dumps(t, indent=2) + "\n")
EOF
_run --check
{ [ "$st" = 1 ] && _has "contrast: muted on bg (light, more contrast) is"; } || _fail "a more-contrast pair below its minimum fails: $out"

# 17. A reel curve that is not design/tokens.json's motion curve fails, naming both.
_copy
_sub "$d/w/web/reel/pace.ts" 'standard: [0.4, 0, 0.2, 1]' 'standard: [0.4, 0, 0.3, 1]'
_run --check
{ [ "$st" = 1 ] && _has "web/reel/pace.ts: CURVES is" && _has "motion.curves are"; } || _fail "a drifted reel curve fails: $out"

if [ "$fails" = 0 ]; then echo "sync tokens tests: all passed"; else echo "sync tokens tests: $fails failed"; exit 1; fi
