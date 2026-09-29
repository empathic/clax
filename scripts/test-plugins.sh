#!/usr/bin/env bash
# Structure checks for the agent plugins and the marketplace manifest.
set -uo pipefail
cd "$(dirname "$0")/.."

FAILED=0
fail() { echo "FAIL: $1"; FAILED=1; }
pass() { echo "PASS: $1"; }

# The value of a top-level frontmatter key in a markdown file, empty when absent.
frontmatter() {
    awk -v key="$2" '
        NR == 1 { if ($0 != "---") exit; next }
        $0 == "---" { exit }
        index($0, key ":") == 1 { sub(/^[^:]*:[ \t]*/, ""); print; exit }
    ' "$1"
}

json_files=()
while IFS= read -r f; do json_files+=("$f"); done < <(find plugins -type f -name '*.json' | sort)
json_files+=(.claude-plugin/marketplace.json)
for f in "${json_files[@]}"; do
    if [ ! -f "$f" ]; then fail "$f is missing"; continue; fi
    if python3 -m json.tool "$f" >/dev/null 2>&1; then pass "$f is valid JSON"; else fail "$f is not valid JSON"; fi
done

for f in plugins/claude-code/.claude-plugin/plugin.json plugins/claude-code/.mcp.json plugins/claude-code/hooks/hooks.json; do
    [ -f "$f" ] || fail "$f is missing"
done

installer=plugins/claude-code/scripts/ensure-artifax.sh
if cmp -s scripts/ensure-artifax.sh "$installer"; then
    pass "$installer matches scripts/ensure-artifax.sh"
else
    fail "$installer differs from scripts/ensure-artifax.sh (or is missing)"
fi
[ -x "$installer" ] || fail "$installer is not executable"

commands=(plugins/claude-code/commands/*.md)
[ -f "${commands[0]}" ] || fail "plugins/claude-code/commands has no commands"
for f in "${commands[@]}"; do
    [ -f "$f" ] || continue
    if [ -n "$(frontmatter "$f" description)" ]; then pass "$f has a description"; else fail "$f has no frontmatter description"; fi
done

skills=(plugins/*/skills/*/SKILL.md)
[ -f "${skills[0]}" ] || fail "no SKILL.md under plugins/*/skills"
for f in "${skills[@]}"; do
    [ -f "$f" ] || continue
    for key in name description; do
        if [ -n "$(frontmatter "$f" "$key")" ]; then pass "$f has $key"; else fail "$f has no frontmatter $key"; fi
    done
done

if [ "$FAILED" -ne 0 ]; then echo "plugin checks failed"; exit 1; fi
echo "plugin checks passed"
