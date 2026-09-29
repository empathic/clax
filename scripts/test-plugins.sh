#!/usr/bin/env bash
# Structure checks for the agent plugins and the marketplace manifests.
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
json_files+=(.claude-plugin/marketplace.json .agents/plugins/marketplace.json)
for f in "${json_files[@]}"; do
    if [ ! -f "$f" ]; then fail "$f is missing"; continue; fi
    if python3 -m json.tool "$f" >/dev/null 2>&1; then pass "$f is valid JSON"; else fail "$f is not valid JSON"; fi
done

for f in plugins/claude-code/.claude-plugin/plugin.json plugins/claude-code/.mcp.json plugins/claude-code/hooks/hooks.json \
    plugins/artifax/.codex-plugin/plugin.json plugins/artifax/.mcp.json plugins/artifax/hooks/hooks.json \
    plugins/artifax/skills/artifax/SKILL.md plugins/artifax/README.md; do
    [ -f "$f" ] || fail "$f is missing"
done

for plugin in plugins/claude-code plugins/artifax; do
    # .mcp.json: a bare map of servers or a mcpServers wrapper, each with a command.
    if python3 - "$plugin/.mcp.json" 2>/dev/null <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
servers = d.get("mcpServers", d) if isinstance(d, dict) else None
ok = isinstance(servers, dict) and servers and all(isinstance(v, dict) and v.get("command") for v in servers.values())
sys.exit(0 if ok else 1)
PY
    then pass "$plugin/.mcp.json servers have a command"; else fail "$plugin/.mcp.json must map server names to objects with a command"; fi

    # hooks.json: SessionStart and SessionEnd commands with numeric timeouts.
    if python3 - "$plugin/hooks/hooks.json" 2>/dev/null <<'PY'
import json, sys
hooks = json.load(open(sys.argv[1])).get("hooks", {})
def ok(event):
    entries = hooks.get(event) or []
    cmds = [h for e in entries for h in e.get("hooks", [])]
    return bool(cmds) and all(isinstance(h.get("timeout"), (int, float)) and not isinstance(h.get("timeout"), bool) for h in cmds)
sys.exit(0 if ok("SessionStart") and ok("SessionEnd") else 1)
PY
    then pass "$plugin/hooks/hooks.json has SessionStart and SessionEnd with numeric timeouts"; else fail "$plugin/hooks/hooks.json needs SessionStart and SessionEnd hooks with numeric timeout"; fi
done

# The Codex plugin's MCP server and hooks run the shim as the codex agent.
if python3 - plugins/artifax/.mcp.json plugins/artifax/hooks/hooks.json 2>/dev/null <<'PY'
import json, sys
server = json.load(open(sys.argv[1]))["mcpServers"]["artifax"]
hooks = json.load(open(sys.argv[2]))["hooks"]
argv = [server["command"], *server.get("args", [])]
cmds = [h["command"] for event in ("SessionStart", "SessionEnd") for entry in hooks[event] for h in entry["hooks"]]
ok = argv[-4:] == ["exec", "mcp", "--agent", "codex"] and all("exec hook --agent codex" in c for c in cmds)
# Codex expands no plugin-root variable in .mcp.json but resolves a relative cwd
# against the plugin root, so the script path is relative to that cwd.
ok = ok and server.get("cwd") == "./" and argv[1] == "./scripts/ensure-artifax.sh"
# Hooks run in a shell with PLUGIN_ROOT exported.
ok = ok and all('"${PLUGIN_ROOT}/scripts/ensure-artifax.sh"' in c for c in cmds)
sys.exit(0 if ok else 1)
PY
then pass "the Codex MCP server and hooks use --agent codex"; else fail "the Codex MCP server and hooks must run the shim with --agent codex"; fi

# Every Claude marketplace plugin source is an existing directory.
if python3 - .claude-plugin/marketplace.json <<'PY'
import json, os, sys
plugins = json.load(open(sys.argv[1])).get("plugins", [])
ok = bool(plugins) and all(isinstance(p.get("source"), str) and os.path.isdir(p["source"]) for p in plugins)
sys.exit(0 if ok else 1)
PY
then pass "marketplace plugin sources exist"; else fail "a marketplace plugin source is not an existing directory"; fi

# Every Codex marketplace entry is a local source at ./plugins/<name> that exists.
if python3 - .agents/plugins/marketplace.json 2>/dev/null <<'PY'
import json, os, sys
plugins = json.load(open(sys.argv[1])).get("plugins", [])
def ok(p):
    src = p.get("source") or {}
    return src.get("source") == "local" and src.get("path") == "./plugins/" + p.get("name", "") and os.path.isdir(src["path"])
sys.exit(0 if plugins and all(ok(p) for p in plugins) else 1)
PY
then pass "Codex marketplace sources are ./plugins/<name> and exist"; else fail "a Codex marketplace source is not ./plugins/<name> or does not exist"; fi

# The Codex manifest: pinned fields, companion paths that resolve, no top-level hooks.
if python3 - plugins/artifax/.codex-plugin/plugin.json 2>/dev/null <<'PY'
import json, os, sys
m = json.load(open(sys.argv[1]))
root = os.path.dirname(os.path.dirname(sys.argv[1]))
i = m.get("interface", {})
prompts = i.get("defaultPrompt")
ok = (m.get("name") == "artifax" and m.get("version") == "0.2.0"
      and m.get("mcpServers") == "./.mcp.json" and os.path.isfile(os.path.join(root, ".mcp.json"))
      and m.get("skills") == "./skills/" and os.path.isdir(os.path.join(root, "skills"))
      and "hooks" not in m
      and isinstance(prompts, list) and 1 <= len(prompts) <= 3
      and all(isinstance(p, str) and 0 < len(p) <= 128 for p in prompts))
sys.exit(0 if ok else 1)
PY
then pass "plugins/artifax/.codex-plugin/plugin.json has the pinned fields"; else fail "plugins/artifax/.codex-plugin/plugin.json is missing pinned fields"; fi

for installer in plugins/claude-code/scripts/ensure-artifax.sh plugins/artifax/scripts/ensure-artifax.sh; do
    if cmp -s scripts/ensure-artifax.sh "$installer"; then
        pass "$installer matches scripts/ensure-artifax.sh"
    else
        fail "$installer differs from scripts/ensure-artifax.sh (or is missing)"
    fi
    [ -x "$installer" ] || fail "$installer is not executable"
done

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

validator="$HOME/.codex/skills/.system/plugin-creator/scripts/validate_plugin.py"
if [ -f "$validator" ]; then
    if out="$(python3 "$validator" plugins/artifax 2>&1)"; then
        pass "the Codex plugin validator accepts plugins/artifax"
    else
        fail "the Codex plugin validator rejects plugins/artifax: $out"
    fi
else
    echo "SKIP: $validator not found; the Codex plugin validator was not run"
fi

if [ "$FAILED" -ne 0 ]; then echo "plugin checks failed"; exit 1; fi
echo "plugin checks passed"
