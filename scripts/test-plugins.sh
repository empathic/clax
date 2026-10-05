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
while IFS= read -r f; do json_files+=("$f"); done < <(find plugins -name node_modules -prune -o -type f -name '*.json' -print | sort)
json_files+=(.claude-plugin/marketplace.json .agents/plugins/marketplace.json .grok-plugin/marketplace.json)
for f in "${json_files[@]}"; do
    if [ ! -f "$f" ]; then fail "$f is missing"; continue; fi
    if python3 -m json.tool "$f" >/dev/null 2>&1; then pass "$f is valid JSON"; else fail "$f is not valid JSON"; fi
done

for f in plugins/claude-code/.claude-plugin/plugin.json plugins/claude-code/.mcp.json plugins/claude-code/hooks/hooks.json \
    plugins/clax/.codex-plugin/plugin.json plugins/clax/.mcp.json plugins/clax/hooks/hooks.json \
    plugins/clax/skills/clax/SKILL.md plugins/clax/README.md \
    plugins/clax-grok/.grok-plugin/plugin.json plugins/clax-grok/.mcp.json plugins/clax-grok/hooks/hooks.json \
    plugins/clax-grok/skills/clax/SKILL.md plugins/clax-grok/README.md; do
    [ -f "$f" ] || fail "$f is missing"
done

for plugin in plugins/claude-code plugins/clax plugins/clax-grok; do
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
if python3 - plugins/clax/.mcp.json plugins/clax/hooks/hooks.json 2>/dev/null <<'PY'
import json, sys
server = json.load(open(sys.argv[1]))["mcpServers"]["clax"]
hooks = json.load(open(sys.argv[2]))["hooks"]
argv = [server["command"], *server.get("args", [])]
cmds = [h["command"] for event in ("SessionStart", "SessionEnd", "Stop") for entry in hooks[event] for h in entry["hooks"]]
ok = argv[-4:] == ["exec", "mcp", "--agent", "codex"] and all("exec hook --agent codex" in c for c in cmds)
# Codex expands no plugin-root variable in .mcp.json but resolves a relative cwd
# against the plugin root, so the script path is relative to that cwd.
ok = ok and server.get("cwd") == "./" and argv[1] == "./scripts/ensure-clax.sh"
# Hooks run in a shell with PLUGIN_ROOT exported.
ok = ok and all('"${PLUGIN_ROOT}/scripts/ensure-clax.sh"' in c for c in cmds)
ok = ok and server.get("env_vars") == ["CLAX_HOME", "CLAX_NO_OPEN", "CLAX_BIN", "CLAX_CODEX_BIN"]
sys.exit(0 if ok else 1)
PY
then pass "the Codex MCP server and hooks use --agent codex"; else fail "the Codex MCP server and hooks must run the shim with --agent codex"; fi

# The Grok plugin: its own name (Grok also discovers the Claude Code plugin,
# named clax), a server named clax_grok run as the grok agent, and hooks for
# SessionStart, Stop and SessionEnd only, all through the quoted plugin root.
if python3 - plugins/clax-grok "$(scripts/check-version.sh --print)" 2>/dev/null <<'PY'
import json, os, sys
root, version = sys.argv[1], sys.argv[2]
m = json.load(open(os.path.join(root, ".grok-plugin/plugin.json")))
servers = json.load(open(os.path.join(root, ".mcp.json")))["mcpServers"]
hooks = json.load(open(os.path.join(root, "hooks/hooks.json")))["hooks"]
ok = m.get("name") == "clax-grok" and m.get("version") == version
ok = ok and list(servers) == ["clax_grok"]
s = servers["clax_grok"]
ok = ok and s["command"] == "${GROK_PLUGIN_ROOT}/scripts/ensure-clax.sh"
ok = ok and s.get("args") == ["exec", "mcp", "--agent", "grok"]
ok = ok and s.get("env") == {"GROK_PLUGIN_ROOT": "${GROK_PLUGIN_ROOT}"}
ok = ok and sorted(hooks) == ["SessionEnd", "SessionStart", "Stop"]
want = {"SessionStart": ("session-start", 5), "Stop": ("stop", 10), "SessionEnd": ("session-end", 2)}
prefix = '"${GROK_PLUGIN_ROOT}/scripts/ensure-clax.sh" exec hook --agent grok '
for event, (name, timeout) in want.items():
    cmds = [h for e in hooks[event] for h in e["hooks"]]
    ok = ok and len(cmds) == 1 and cmds[0]["command"] == prefix + name and cmds[0]["timeout"] == timeout
sys.exit(0 if ok else 1)
PY
then pass "plugins/clax-grok: name clax-grok, server clax_grok as --agent grok, three hooks"
else fail "plugins/clax-grok's manifest, .mcp.json or hooks.json is not as specified (spec §13, Grok Build)"; fi

# No two plugins that Grok can load share an MCP server name: Grok keeps the
# first definition of a name and drops the rest, so a shared name would let
# one Clax copy hide the other.
if python3 - plugins/claude-code/.mcp.json plugins/clax-grok/.mcp.json 2>/dev/null <<'PY'
import json, sys
names = []
for p in sys.argv[1:]:
    d = json.load(open(p))
    names += list(d.get("mcpServers", d))
sys.exit(0 if len(names) == len(set(names)) else 1)
PY
then pass "the Claude Code and Grok plugins' MCP server names differ"
else fail "the Claude Code and Grok plugins declare the same MCP server name"; fi

# The Grok marketplace index lists clax-grok at ./plugins/clax-grok.
if python3 - .grok-plugin/marketplace.json 2>/dev/null <<'PY'
import json, os, sys
plugins = json.load(open(sys.argv[1])).get("plugins", [])
ok = [p.get("name") for p in plugins] == ["clax-grok"] and plugins[0].get("source") == "./plugins/clax-grok" and os.path.isdir("plugins/clax-grok")
sys.exit(0 if ok else 1)
PY
then pass ".grok-plugin/marketplace.json lists clax-grok"
else fail ".grok-plugin/marketplace.json must list only clax-grok at ./plugins/clax-grok"; fi

# The wrapper's stand-down text is the binary's, word for word.
want="$(sed -n 's/^pub const GROK_STANDDOWN: &str = "\(.*\)";$/\1/p' crates/clax-mcp/src/standdown.rs)"
got="$(sed -n 's/^GROK_STANDDOWN="\(.*\)"$/\1/p' scripts/ensure-clax.sh | sed 's/\\`/`/g')"
if [ -n "$want" ] && [ "$want" = "$got" ]; then pass "the wrapper's GROK_STANDDOWN matches crates/clax-mcp/src/standdown.rs"
else fail "the wrapper's GROK_STANDDOWN differs from crates/clax-mcp/src/standdown.rs"; fi

# The Claude Code hooks quote the plugin root, which may contain spaces.
if python3 - plugins/claude-code/hooks/hooks.json 2>/dev/null <<'PY'
import json, sys
hooks = json.load(open(sys.argv[1]))["hooks"]
cmds = [h["command"] for event in ("SessionStart", "SessionEnd", "UserPromptSubmit", "Stop") for entry in hooks[event] for h in entry["hooks"]]
prefix = '"${CLAUDE_PLUGIN_ROOT}/scripts/ensure-clax.sh" exec hook --agent claude '
sys.exit(0 if cmds and all(c.startswith(prefix) for c in cmds) else 1)
PY
then pass "the Claude Code hooks quote \${CLAUDE_PLUGIN_ROOT} and use --agent claude"
else fail "the Claude Code hooks must run \"\${CLAUDE_PLUGIN_ROOT}/scripts/ensure-clax.sh\" exec hook --agent claude"; fi

# The Claude Code plugin hands feedback over at Stop and prompt submit; the
# Codex plugin at Stop. Stop hooks get 10 s. Both renew working records
# through the PostToolUse gate (scripts/tool-hook.sh), which gets 5 s.
if python3 - plugins/claude-code/hooks/hooks.json plugins/clax/hooks/hooks.json 2>/dev/null <<'PY'
import json, sys
claude = json.load(open(sys.argv[1]))["hooks"]
codex = json.load(open(sys.argv[2]))["hooks"]
def cmds(hooks, event):
    return [h for e in hooks.get(event, []) for h in e.get("hooks", [])]
ok = all(h["command"].endswith("exec hook --agent claude stop") and h["timeout"] == 10 for h in cmds(claude, "Stop")) and cmds(claude, "Stop")
ok = ok and all(h["command"].endswith("exec hook --agent claude prompt") and isinstance(h["timeout"], int) for h in cmds(claude, "UserPromptSubmit")) and cmds(claude, "UserPromptSubmit")
ok = ok and all('"${PLUGIN_ROOT}/scripts/ensure-clax.sh" exec hook --agent codex stop' in h["command"] and h["timeout"] == 10 for h in cmds(codex, "Stop")) and cmds(codex, "Stop")
ok = ok and [h["command"] for h in cmds(claude, "PostToolUse")] == ['"${CLAUDE_PLUGIN_ROOT}/scripts/tool-hook.sh" claude'] and all(h["timeout"] == 5 for h in cmds(claude, "PostToolUse"))
ok = ok and [h["command"] for h in cmds(codex, "PostToolUse")] == ['bash "${PLUGIN_ROOT}/scripts/tool-hook.sh" codex'] and all(h["timeout"] == 5 for h in cmds(codex, "PostToolUse"))
sys.exit(0 if ok else 1)
PY
then pass "Stop, prompt and PostToolUse hooks are wired"; else fail "the Claude Stop/UserPromptSubmit/PostToolUse or Codex Stop/PostToolUse hooks are missing or misconfigured"; fi

for f in plugins/claude-code/commands/comments.md plugins/claude-code/commands/watch.md plugins/claude-code/commands/wait.md; do
    [ -f "$f" ] || fail "$f is missing"
done

# One version everywhere: see scripts/check-version.sh for the list.
if out="$(scripts/check-version.sh 2>&1)"; then pass "every written version agrees (scripts/check-version.sh)"
else fail "$out"; fi

# The Rust tools and the Pi extension carry the same twenty-three tool descriptions,
# word for word (plugins/pi/test/fixtures/contract.json lists them).
if out="$(python3 - plugins/pi/test/fixtures/contract.json crates/clax-mcp/src/tools.rs plugins/pi/src/clax.ts 2>&1 <<'PY'
import json, sys
tools = json.load(open(sys.argv[1]))["tools"]
missing = []
for src in sys.argv[2:4]:
    text = open(src).read()
    for t in tools:
        # Both sources write the description as one double-quoted literal.
        quoted = '"' + t["description"].replace("\\", "\\\\").replace('"', '\\"') + '"'
        if quoted not in text:
            missing.append(f"{src}: {t['name']}")
if len(tools) != 23 or missing:
    print("; ".join(missing) or f"{len(tools)} tools in the fixture, not 23")
    sys.exit(1)
PY
)"; then pass "the twenty-three tool descriptions match in tools.rs and clax.ts"
else fail "tool descriptions differ from plugins/pi/test/fixtures/contract.json: $out"; fi

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
if python3 - plugins/clax/.codex-plugin/plugin.json "$(scripts/check-version.sh --print)" 2>/dev/null <<'PY'
import json, os, sys
m = json.load(open(sys.argv[1]))
root = os.path.dirname(os.path.dirname(sys.argv[1]))
i = m.get("interface", {})
prompts = i.get("defaultPrompt")
ok = (m.get("name") == "clax" and m.get("version") == sys.argv[2]
      and m.get("mcpServers") == "./.mcp.json" and os.path.isfile(os.path.join(root, ".mcp.json"))
      and m.get("skills") == "./skills/" and os.path.isdir(os.path.join(root, "skills"))
      and "hooks" not in m
      and isinstance(prompts, list) and 1 <= len(prompts) <= 3
      and all(isinstance(p, str) and 0 < len(p) <= 128 for p in prompts))
sys.exit(0 if ok else 1)
PY
then pass "plugins/clax/.codex-plugin/plugin.json has the pinned fields"; else fail "plugins/clax/.codex-plugin/plugin.json is missing pinned fields"; fi

# The Claude Code manifest declares the clax channel; no plugin or crate ever
# declares the channel permission relay (comment authors never approve tool use).
if python3 - plugins/claude-code/.claude-plugin/plugin.json <<'PY'
import json, sys
m = json.load(open(sys.argv[1]))
sys.exit(0 if m.get("channels") == [{"server": "clax"}] else 1)
PY
then pass "the Claude Code manifest declares the clax channel"; else fail "plugins/claude-code/.claude-plugin/plugin.json lacks channels: [{\"server\": \"clax\"}]"; fi

relay='claude/channel'"/permission"
if grep -rqF "$relay" crates plugins; then
    fail "permission relay must never be declared: $(grep -rlF "$relay" crates plugins | tr '\n' ' ')"
else
    pass "no permission relay"
fi

for wrapper in plugins/claude-code/scripts/ensure-clax.sh plugins/clax/scripts/ensure-clax.sh plugins/clax-grok/scripts/ensure-clax.sh plugins/pi/scripts/ensure-clax.sh; do
    if cmp -s scripts/ensure-clax.sh "$wrapper"; then
        pass "$wrapper matches scripts/ensure-clax.sh"
    else
        fail "$wrapper differs from scripts/ensure-clax.sh (or is missing)"
    fi
    [ -x "$wrapper" ] || fail "$wrapper is not executable"
done

# The plugins run the newest release: the pin (PINNED_VERSION in the wrapper,
# which scripts/pin-release.sh writes) is the newest v* tag, with a checksum
# for every target. With no tag yet, nothing may be pinned. The tags come
# from git, so a shallow clone (CI must fetch them) cannot be judged.
pin="$(sed -n 's/^PINNED_VERSION="\(.*\)"$/\1/p' scripts/ensure-clax.sh)"
if [ "$(git rev-parse --is-shallow-repository 2>/dev/null)" = true ]; then
    fail "the pin cannot be checked against the release tags in a shallow clone; fetch the history and tags (actions/checkout with fetch-depth: 0)"
else
    newest="$(git tag -l 'v*' | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | sed 's/^v//' | sort -t. -k1,1n -k2,2n -k3,3n | tail -1)"
    if [ -z "$newest" ] && [ -z "$pin" ]; then
        pass "no release is tagged yet, and none is pinned"
    elif [ -z "$newest" ]; then
        fail "the wrapper pins clax $pin, but no v* tag exists; pin a published release with scripts/pin-release.sh"
    elif [ "$pin" = "$newest" ]; then
        if grep -q '^SHA256_[A-Z0-9_]*=""$' scripts/ensure-clax.sh; then
            fail "the wrapper pins clax $pin but lacks a target's SHA256; run scripts/pin-release.sh v$pin"
        else pass "the plugins pin the newest release, v$pin"; fi
    else
        fail "the plugins pin '${pin:-nothing}', but the newest release tag is v$newest; run scripts/pin-release.sh v$newest"
    fi
fi

# Pi runs the same wrapper, which its package must ship.
if python3 - plugins/pi/package.json 2>/dev/null <<'PY'
import json, sys
sys.exit(0 if "scripts/" in json.load(open(sys.argv[1])).get("files", []) else 1)
PY
then pass "plugins/pi/package.json ships scripts/ (the wrapper)"; else fail "plugins/pi/package.json must list scripts/ in files"; fi

for gate in plugins/claude-code/scripts/tool-hook.sh plugins/clax/scripts/tool-hook.sh; do
    if cmp -s scripts/tool-hook.sh "$gate"; then
        pass "$gate matches scripts/tool-hook.sh"
    else
        fail "$gate differs from scripts/tool-hook.sh (or is missing)"
    fi
    [ -x "$gate" ] || fail "$gate is not executable"
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

# The four skill copies (Claude Code, Codex, Pi, Grok Build) share the page
# contract word for word (docs/contract.md carries the same section), and
# "Comment loop" and "What is not yet available" are the same in all four.
skill_copies=(plugins/claude-code/skills/clax/SKILL.md plugins/clax/skills/clax/SKILL.md plugins/pi/skills/clax/SKILL.md plugins/clax-grok/skills/clax/SKILL.md)
# The lines from "## <heading>" up to, not including, the next "## " heading.
section() {
    awk -v h="## $2" '
        $0 == h { on = 1; print; next }
        on && /^## / { exit }
        on { print }
    ' "$1"
}
# The Grok skill tells the agent to start one persistent monitor on the
# shell-quoted push.follow_command from clax_grok__status.
grok_skill=plugins/clax-grok/skills/clax/SKILL.md
live="$(section "$grok_skill" "Live feedback in Grok")"
if [ -n "$live" ] && echo "$live" | grep -qF 'push.follow_command' \
    && echo "$live" | grep -qF 'persistent: true' && echo "$live" | grep -qF 'push.available' \
    && echo "$live" | grep -qF 'clax_grok__comments_read'; then
    pass "$grok_skill has the Live feedback in Grok section"
else fail "$grok_skill needs a '## Live feedback in Grok' section naming the monitor command, persistent: true, push.available and clax_grok__comments_read"; fi
for f in "${skill_copies[@]}"; do
    if [ ! -f "$f" ]; then fail "$f is missing"; continue; fi
    if [ "$(frontmatter "$f" name)" = "clax" ]; then pass "$f is named clax"; else fail "$f frontmatter name is not clax"; fi
    if [ -n "$(section "$f" "Page contract")" ]; then pass "$f has a Page contract section"; else fail "$f has no '## Page contract' section"; fi
done
same_section() {
    local heading="$1"; shift
    local first="$1" f
    [ -f "$first" ] || return
    for f in "${@:2}"; do
        if [ ! -f "$f" ]; then fail "$f is missing"; continue; fi
        if cmp -s <(section "$first" "$heading") <(section "$f" "$heading"); then
            pass "'## $heading' in $f matches $first"
        else
            fail "'## $heading' in $f differs from $first"
            diff <(section "$first" "$heading") <(section "$f" "$heading") | head -20
        fi
    done
}
# Pi reads only the resources its package.json manifest lists once it has one,
# so the skills directory must be listed and shipped.
if python3 - plugins/pi/package.json 2>/dev/null <<'PY2'
import json, sys
d = json.load(open(sys.argv[1]))
ok = "skills" in d.get("pi", {}).get("skills", []) and "skills/" in d.get("files", [])
sys.exit(0 if ok else 1)
PY2
then pass "plugins/pi/package.json lists and ships skills/"; else fail "plugins/pi/package.json must list skills in pi.skills and skills/ in files"; fi
same_section "Page contract" "${skill_copies[@]}" docs/contract.md
same_section "What is not yet available" "${skill_copies[@]}"
for f in "${skill_copies[@]}"; do
    if [ -n "$(section "$f" "Comment loop")" ]; then pass "$f has a Comment loop section"; else fail "$f has no '## Comment loop' section"; fi
done
same_section "Comment loop" "${skill_copies[@]}"
# The runtime capabilities section is the same in the four skills and in
# docs/contract.md, and it points at the contract files the daemon serves
# (built in from web/contract/0.2.61/, which must hold all of them).
for f in "${skill_copies[@]}" docs/contract.md; do
    if [ -n "$(section "$f" "Runtime capabilities")" ]; then pass "$f has a Runtime capabilities section"; else fail "$f has no '## Runtime capabilities' section"; fi
done
same_section "Runtime capabilities" "${skill_copies[@]}" docs/contract.md
# The section documents room and sample (phase 5), and no copy still says
# they are not available.
rc="$(section "${skill_copies[0]}" "Runtime capabilities")"
if echo "$rc" | tr '\n' ' ' | grep -qF '`comments`, `assets`, `room`, `sample`)'; then pass "the Runtime capabilities contract list names room and sample"
else fail "the Runtime capabilities contract list does not name \`room\` and \`sample\`"; fi
if echo "$rc" | grep -q '^- `room`'; then pass "the Runtime capabilities section has a room bullet"
else fail "the Runtime capabilities section has no '- \`room\`' bullet"; fi
if echo "$rc" | grep -q '^- `sample`'; then pass "the Runtime capabilities section has a sample bullet"
else fail "the Runtime capabilities section has no '- \`sample\`' bullet"; fi
if grep -qF 'Rooms and `sample()` (phase 5)' "${skill_copies[0]}"; then fail "${skill_copies[0]} still says rooms and sample() are not available"
else pass "${skill_copies[0]} no longer says rooms and sample() are not available"; fi
if section docs/contract.md "Runtime capabilities" | grep -q '/_clax/contract/0.2.61/<name>.d.ts'; then pass "the Runtime capabilities section points at /_clax/contract/0.2.61/"; else fail "the Runtime capabilities section does not point at the daemon's /_clax/contract/0.2.61/<name>.d.ts"; fi
for name in claude permissions artifact self assets comments db downloads user files mcp room sample; do
    if [ -f "web/contract/0.2.61/$name.d.ts" ]; then pass "web/contract/0.2.61/$name.d.ts exists"; else fail "web/contract/0.2.61/$name.d.ts is missing"; fi
done
for f in "${skill_copies[@]}"; do
    if [ -n "$(section "$f" "Data (db)")" ]; then pass "$f has a Data (db) section"; else fail "$f has no '## Data (db)' section"; fi
done
same_section "Data (db)" "${skill_copies[@]}"

# Every tool list names exactly the tools in plugins/pi/test/fixtures/contract.json:
# the generated block in each skill intro, and the hand-written lists in
# docs/contract.md and the READMEs, whose spelled-out count must match too.
if out="$(python3 scripts/sync-skill-tools.py --check 2>&1)"; then
    pass "the skills, docs/contract.md and the READMEs list exactly the fixture's tools"
else fail "$out"; fi

# The previous name appears only in the approved exceptions listed in
# docs/superpowers/plans/2026-09-29-clax-rename.md: the plans written before
# the rename, this rename's plan and the Svelte port plan written alongside
# it, and the spec's name-history note. It is assembled from two halves so
# this file is not an exception.
OLD="arti""fax"
name_exceptions=(
    docs/superpowers/plans/2026-09-28-phase-1-daemon-publish-viewer.md
    docs/superpowers/plans/2026-09-28-phase-2-mcp-and-plugins.md
    docs/superpowers/plans/2026-09-28-phase-2-scoped.md
    docs/superpowers/plans/2026-09-28-phase-3-comments-and-feedback.md
    docs/superpowers/plans/2026-09-28-phase-4-runtime-capabilities.md
    docs/superpowers/plans/2026-09-28-phase-5-room-and-sample.md
    docs/superpowers/plans/2026-09-29-clax-rename.md
    docs/superpowers/plans/2026-09-29-svelte-port.md
    docs/superpowers/specs/2026-09-28-clax-design.md
)
excludes=()
for f in "${name_exceptions[@]}"; do excludes+=(":(exclude)$f"); done
stray="$(git grep -il "$OLD" -- . "${excludes[@]}"; git ls-files | grep -i "$OLD")"
if [ -z "$stray" ]; then pass "the previous name appears only in the approved exceptions"
else fail "the previous name remains in: $(echo $stray | head -c 2000)"; fi
spec=docs/superpowers/specs/2026-09-28-clax-design.md
stray="$(awk -v old="$OLD" '
    /<!-- name-history:begin -->/ { on = 1; next }
    /<!-- name-history:end -->/ { on = 0; next }
    !on && index(tolower($0), old) { print NR }
' "$spec" 2>/dev/null)"
if [ -f "$spec" ] && [ -z "$stray" ]; then pass "$spec names the previous name only in its name-history note"
else fail "$spec is missing or names the previous name outside its name-history note (lines: $(echo $stray))"; fi

validator="$HOME/.codex/skills/.system/plugin-creator/scripts/validate_plugin.py"
if [ -f "$validator" ]; then
    if out="$(python3 "$validator" plugins/clax 2>&1)"; then
        pass "the Codex plugin validator accepts plugins/clax"
    else
        fail "the Codex plugin validator rejects plugins/clax: $out"
    fi
else
    echo "SKIP: $validator not found; the Codex plugin validator was not run"
fi

if [ "$FAILED" -ne 0 ]; then echo "plugin checks failed"; exit 1; fi
echo "plugin checks passed"
