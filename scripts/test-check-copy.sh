#!/usr/bin/env bash
# Checks scripts/check-copy.py on fixtures, then runs it on the files given
# (Bazel passes //apps/clax_app:copy_sources): a straight apostrophe in a
# contraction or possessive fails in single-, double-, triple-quoted and raw
# Dart literals and in JS strings and templates; "..." fails; a catch
# variable interpolated into a literal fails; straight double quotes around
# words fail; curly copy, plain interpolations, htm attributes, JSON in a
# literal, comments, JS regular expressions and lines ending
# `// copy-check: allow` pass; each finding names its path, line and rule.
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1
d="$(mktemp -d)"
trap 'rm -rf "$d"' EXIT
fails=0
_fail() { echo "FAIL: $1" >&2; fails=$((fails + 1)); }

# _expect NAME pass|fail EXT CONTENT [OUTPUT-SUBSTRING]
_expect() {
    local name="$1" want="$2" ext="$3" content="$4" needle="${5:-}"
    local f="$d/$name.$ext" out code
    printf '%s\n' "$content" > "$f"
    out="$(python3 scripts/check-copy.py "$f" 2>&1)"
    code=$?
    if [ "$want" = pass ] && [ "$code" -ne 0 ]; then
        _fail "$name: expected pass, got exit $code: $out"
    elif [ "$want" = fail ] && [ "$code" -ne 1 ]; then
        _fail "$name: expected exit 1, got $code: $out"
    elif [ -n "$needle" ] && [[ "$out" != *"$needle"* ]]; then
        _fail "$name: output lacks '$needle': $out"
    fi
}

# Dart.
_expect escaped_apostrophe fail dart "Text('Don\\'t');" "escaped_apostrophe.dart:1: straight apostrophe"
_expect curly_apostrophe pass dart "Text('Don’t');"
_expect double_quoted_apostrophe fail dart 'Text("Clax'"'"'s service");'
_expect triple_quoted fail dart "final s = '''
It's here
''';" "triple_quoted.dart:2:"
_expect raw_string fail dart "final s = r\"You're in\";"
_expect ellipsis fail dart "Text('Loading...');" "three dots"
_expect curly_ellipsis pass dart "Text('Loading…');"
_expect catch_variable fail dart "Text('\$e');" "raw error"
_expect catch_braces fail dart "Text('Failed: \${error}');" "raw error"
_expect catch_member fail dart "Text('Failed: \${e.message}');" "raw error"
_expect count pass dart "Text('\$count apps');"
_expect nested_interpolation pass dart "Text('\${names.join(', ')} and \${n == 1 ? 'one' : 'more'}');"
_expect nested_literal fail dart "Text('\${ok ? 'Done' : 'Can\\'t'}');"
_expect allow pass dart "const hint = 'Tap Keep going'; // copy-check: allow
const old = 'It\\'s old'; // copy-check: allow"
_expect comment pass dart "// It's a comment, don't check it...
/* Nor this: can't. */
Text('Fine');"
_expect straight_quotes fail dart "Text('Ask \"Make me a timer\"');" "straight quotes"
_expect curly_quotes pass dart "Text('Ask “Make me a timer”');"
_expect json_literal pass dart "const j = '{\"op\": \"set\", \"id\": 1}';"
_expect spread pass dart "final xs = [...a, ...b];"
_expect log pass dart "debugPrint('clax home: \$e'); _log('it\\'s \$e...');"
_expect core_exception pass dart "throw const FormatException('a script is {\"runs\": [...]}');"
_expect app_error fail dart "throw AgentError(\"Clax couldn't start\");"
_expect lines fail dart "final a = 1;

Text(\"isn't\");" "lines.dart:3:"

# JavaScript.
_expect js_template fail js 'const t = `Can'"'"'t ${what}`;' "straight apostrophe"
_expect js_template_curly pass js 'const t = `Can’t ${what}`;'
_expect js_string fail js "const t = \"You're offline\";"
_expect js_htm_attribute pass js 'html`<${Empty} icon="compass" title="Nothing here" />`;'
_expect js_htm_text fail js 'html`<p>Clax'"'"'ll make it</p>`;'
_expect js_nested_template fail js 'html`<p>${ok ? html`<b>Don'"'"'t</b>` : null}</p>`;'
_expect js_attribute_hole pass js 'html`<${Tile} entry=${e} />`;'
_expect js_text_hole fail js 'html`<p>${err}</p>`;' "raw error"
_expect js_regex pass js "const r = /[\"'\`]s/g; const q = x.replace(/it's/, '');"
_expect js_division pass js "const half = a / 2; const t = 'fine' / 1;"
_expect js_comment pass js "// Don't check this...
/* or this: it's fine */
const t = 'ok';"
_expect js_ellipsis fail js "const t = 'Loading...';"
_expect js_allow pass js "const t = \"it's\"; // copy-check: allow"

# Several files, each finding listed.
printf "Text('a...');\n" > "$d/one.dart"
printf "const t = 'b...';\n" > "$d/two.js"
out="$(python3 scripts/check-copy.py "$d/one.dart" "$d/two.js" 2>&1)"
[[ "$out" == *"one.dart:1:"* && "$out" == *"two.js:1:"* ]] || _fail "several files: $out"
# A file of another type is refused.
printf 'x\n' > "$d/notes.txt"
python3 scripts/check-copy.py "$d/notes.txt" > /dev/null 2>&1
[ $? -eq 2 ] || _fail "a .txt file is not refused with exit 2"

# The tree.
if [ "$#" -gt 0 ]; then
    python3 scripts/check-copy.py "$@" || _fail "the app's copy has straight quotes, '...' or raw errors (above)"
fi

if [ "$fails" -ne 0 ]; then
    echo "$fails check(s) failed" >&2
    exit 1
fi
echo "check-copy: ok"
