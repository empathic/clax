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

# .mcp.json: a bare map of servers or a mcpServers wrapper, each with a command.
if python3 - plugins/claude-code/.mcp.json <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
servers = d.get("mcpServers", d) if isinstance(d, dict) else None
ok = isinstance(servers, dict) and servers and all(isinstance(v, dict) and v.get("command") for v in servers.values())
sys.exit(0 if ok else 1)
PY
then pass ".mcp.json servers have a command"; else fail ".mcp.json must map server names to objects with a command"; fi

# hooks.json: SessionStart and SessionEnd commands with numeric timeouts.
if python3 - plugins/claude-code/hooks/hooks.json <<'PY'
import json, sys
hooks = json.load(open(sys.argv[1])).get("hooks", {})
def ok(event):
    entries = hooks.get(event) or []
    cmds = [h for e in entries for h in e.get("hooks", [])]
    return bool(cmds) and all(isinstance(h.get("timeout"), (int, float)) and not isinstance(h.get("timeout"), bool) for h in cmds)
sys.exit(0 if ok("SessionStart") and ok("SessionEnd") else 1)
PY
then pass "hooks.json has SessionStart and SessionEnd with numeric timeouts"; else fail "hooks.json needs SessionStart and SessionEnd hooks with numeric timeout"; fi

# Every marketplace plugin source is an existing directory.
if python3 - .claude-plugin/marketplace.json <<'PY'
import json, os, sys
plugins = json.load(open(sys.argv[1])).get("plugins", [])
ok = bool(plugins) and all(isinstance(p.get("source"), str) and os.path.isdir(p["source"]) for p in plugins)
sys.exit(0 if ok else 1)
PY
then pass "marketplace plugin sources exist"; else fail "a marketplace plugin source is not an existing directory"; fi

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
    if [ -n "$(frontmatter "$f" allowed-tools)" ]; then pass "$f has allowed-tools"; else fail "$f has no frontmatter allowed-tools"; fi
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
