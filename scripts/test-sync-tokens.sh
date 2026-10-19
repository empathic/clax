#!/usr/bin/env bash
# Checks scripts/sync-tokens.py on a copy of its inputs and outputs: the
# committed outputs are in step; an edited source makes the generated files
# stale; a contrast pair below its minimum fails both modes; an edited haiku
# copy is stale; an app-mode colour in web/shell/src/theme.css that differs
# from the source fails; and a plain run is stable (a second run changes no
# byte).
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1
d="$(mktemp -d)"
trap 'rm -rf "$d"' EXIT
fails=0
_fail() { echo "FAIL: $1" >&2; fails=$((fails + 1)); }

INPUTS=(
    design/tokens.json
    scripts/sync-tokens.py
    web/shell/src/view/haiku.json
    web/shell/src/theme.css
)
OUTPUTS=(
    apps/clax_app/home/tokens.css
    apps/clax_app/lib/ui/tokens.g.dart
    apps/clax_app/assets/haiku.json
)

_copy() {
    rm -rf "$d/w" && mkdir -p "$d/w"
    for f in "${INPUTS[@]}" "${OUTPUTS[@]}" apps/clax_app/assets/kit/kit.css; do
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
rm -f "$d/w/${OUTPUTS[0]}" "$d/w/${OUTPUTS[1]}" "$d/w/${OUTPUTS[2]}"
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
for f in "${OUTPUTS[@]}"; do
    [ ! -f "$f" ] || cmp -s "$d/w/$f" "$f" || _fail "the committed $f differs from a fresh run's"
done

if [ "$fails" = 0 ]; then echo "sync tokens tests: all passed"; else echo "sync tokens tests: $fails failed"; exit 1; fi
