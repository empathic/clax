# Grok Build Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Grok Build (`grok`, xAI's coding agent CLI) becomes Clax's fourth harness, alongside Claude Code, Codex and Pi. It gets delivery tiers 1, 2, 4 and 5, through a separate plugin, `clax-grok`, that `clax init` installs, including on a machine where Grok is the only harness. For tier 5, the agent starts Grok's `monitor` tool on a new, harness-neutral command, `clax feedback follow`, which prints one line per new comment. Grok also discovers the Clax Claude Code plugin (named `clax`) in `~/.claude/plugins`. When a person enables that copy in Grok as well, exactly one Clax MCP server and one set of Clax hooks still act in each Grok session.

**Architecture:** `plugins/clax-grok` is a Claude-format plugin with a `.grok-plugin/plugin.json` manifest. Its MCP server is named `clax_grok` and runs `ensure-clax.sh exec mcp --agent grok`. Its hooks run `clax hook --agent grok` for `SessionStart`, `Stop` and `SessionEnd`. The daemon accepts the harness `grok`, whose armed watches wait on the Stop hook (tier 2). For tier 5, the clax-grok skill has the agent start Grok's persistent `monitor` on `clax feedback follow` after its first publish. The command long-polls a new route, `GET /api/sessions/<sid>/notices`, and prints one notice line per comment, naming the artifact and thread and saying to call `comments_read`. Each line wakes the session. A notice delivers nothing: it stamps `feedback.notified_at`, so each comment is announced once, and the comment itself still arrives exactly once, through tier 1, 2 or 4. The shim keys a Grok session on `GROK_SESSION_ID` from the start. The hooks read Grok's camelCase envelope (`sessionId`, `stopHookActive`, `reason`). The dedupe rule is that **in a Grok session, only `--agent grok` acts**. The Claude Code copy stands down when Grok runs it. Its hooks exit 0 silently. Its MCP server completes the handshake and offers one `status` tool, which says Clax runs from clax-grok. The wrapper (`ensure-clax.sh`, now in four byte-identical copies) enforces this, and so does the binary, as a second layer for a stale wrapper or a stale binary. Grok runs a hook with `GROK_HOOK_EVENT` set, and an MCP server with `GROK_SESSION_ID` set. Claude Code sets `CLAUDE_PID` to its own PID, which is the MCP server's parent, so a Claude Code session nested in a Grok shell still acts. `clax init` installs clax-grok with `grok plugin install <dir> --trust`. It never runs a `grok` command that names `clax`. `just dev grok` runs like `just dev codex`: the installed plugin, with the fresh build first on `PATH`.

**Tech Stack:** Rust 2024 (clap 4, rmcp 3.5, serde, axum 0.8, rust-embed 8, assert_cmd), Bash 3.2-compatible shell, Python 3 (the plugin checks and `sync-skill-tools.py`), and the Grok Build CLI 1.0.45 as read from source at `xai-org/grok-build@2bdd1d6a` (`grok plugin install|uninstall|list --json`, `GROK_HOME`, `GROK_SESSION_ID`, `GROK_HOOK_EVENT`, `GROK_PLUGIN_ROOT`, the `monitor` tool). Tests use only a fake `grok`.

**Spec:** `docs/superpowers/specs/2026-09-28-clax-design.md`. It was written under the product's first name and renamed with the product (spec D15); the brief cites it by that first name. Task 1 amends §2 (new D17), §3, §4, §5, §6, §10 (including a new "Notices" subsection), §11, §12, §13 (a new Grok Build subsection), §16 and §18, and `docs/contract.md`. The owner's binding decisions are in `.superpowers/sdd/2026-10-01-grok/decisions.md`, including the 2026-10-01 decision that adds tier 5 through `monitor`, which supersedes "stop at tiers 1, 2 and 4". The research is `.superpowers/sdd/grok-build-research.md`. The unsettled questions are in `.superpowers/sdd/2026-10-01-grok/open-questions.md`. Each place this plan relies on a recommended answer from that file is marked **provisional (Qn)**.

**Precondition:** `git status --short -- docs/contract.md docs/superpowers/specs docs/follow-ups.md README.md plugins scripts justfile .claude-plugin .agents .grok-plugin crates/clax-cli crates/clax-mcp/src/shim.rs crates/clax-mcp/src/plugin.rs crates/clax-mcp/src/lib.rs crates/clax-hooks crates/clax-server/src/routes/sessions.rs crates/clax-core/src/store/feedback.rs` prints nothing. When this plan was written, other agents had uncommitted work in `crates/clax-hooks/tests/golden.rs` and elsewhere in the crates. If any of these paths shows up, stop and ask the controller. Do not stash, discard or commit someone else's changes.

## Global Constraints

- Rust edition 2024. `cargo clippy --workspace --all-targets -- -D warnings` and `RUSTFLAGS=-Dwarnings cargo check --workspace` pass.
- **Agents stage only. The controller commits.** Stage with `git add` and explicit paths, never `git add -A` or `git add .`. Never run `git commit`, `git stash`, `git reset` or `git checkout -- <path>`. Each task ends with its files staged and a proposed commit message in the report. The controller commits with plain `git commit`, which signs; never `--no-gpg-sign`.
- **Never bind or connect to port 7480 or 7481.** The owner's daemon or dev server may be there. Tests start daemons with `--port 0`, or discover a daemon that a test started with `--port 0` in a scratch `CLAX_HOME`.
- **Never read, write or delete a real home:** `~/.clax`, `~/.clax-dev`, `~/.claude`, `~/.codex`, `~/.grok`, `~/.pi` (Pi's settings), `~/.cargo/bin/clax`, `~/.local/bin/clax`. Every test sets `HOME`, `CLAX_HOME`, `CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `GROK_HOME` and `PI_CODING_AGENT_DIR` to scratch directories.
- **Agents never run a real harness CLI** (`grok`, `claude`, `codex`, `pi`). Every test that involves `grok` puts a fake `grok` first on `PATH` and checks that the fake is the one found before it runs anything. `scripts/smoke-grok.sh` is for the owner to run. Agents write it, `bash -n` it and `shellcheck` it, but never run it. Agents do not run `scripts/verify-harnesses.sh` either.
- **Tests clear the harness environment they inherit.** Agents run inside Claude Code, which sets `CLAUDE_PID`, `CLAUDE_CODE_SESSION_ID` and `CLAUDE_PLUGIN_ROOT`, and possibly inside other harnesses. Every test that runs the wrapper, `clax mcp` or `clax hook` removes `GROK_SESSION_ID`, `GROK_HOOK_EVENT`, `GROK_PLUGIN_ROOT`, `GROK_HOME`, `CLAUDE_PID`, `CLAUDE_CODE_SESSION_ID`, `CLAUDE_PLUGIN_ROOT`, `CLAUDE_PROJECT_DIR` and `CLAX_SESSION_ID`, then sets only what the case needs. Otherwise the stand-down guard would test the agent's own harness.
- No test reaches the network beyond `127.0.0.1`.
- In prose, comments, doc comments and commit messages, write "ID", never "id", except as a literal symbol in code (`session_id`, `"id"`).
- Doc comments and commit messages describe the contract or the change. They never describe the conversation that led to it, the history of a name, or how good the work is.
- Shell that the plugins run (`ensure-clax.sh`) is bash 3.2-compatible: no `mapfile`, no `${var,,}`, no `declare -A`, no `$BASHPID`. A possibly empty array expands as `${a[@]+"${a[@]}"}`.
- `scripts/ensure-clax.sh` and its plugin copies (`plugins/claude-code/scripts/`, `plugins/clax/scripts/` and, from Task 7, `plugins/clax-grok/scripts/`) stay byte-identical. `scripts/test-plugins.sh` checks this.
- The product's previous name must not appear literally in any file this plan creates or edits (the name gate in `scripts/test-plugins.sh`). Code that needs it assembles it from two halves, as the existing code does.
- A step marked **provisional (Qn)** follows the recommendation in `open-questions.md`. Implement it as written, and keep the code for it in the one place the step names, so a different answer changes one place.
- The owner confirmed Q1, Q2, Q3 and Q5, the `clax_grok` server name and the always-inert Claude Code copy on 2026-10-01; those markers now record where each decision lands. Q4 (live verification) is still settled by the owner's smoke run.
- Every task ends with `bash scripts/quality_gates.sh; echo "exit=$?"` printing `exit=0`. Check the status itself, not only the last line. The script takes a per-checkout lock, so a second run waits.

## Review Focus

1. **Exactly one Clax acts in a Grok session, in every combination.** The four combinations (Claude copy enabled or not, clax-grok enabled or not), with Grok's first-definition-wins server merge in both discovery orders, are each run by `crates/clax-cli/tests/grok_dedupe.rs` (Task 10). The wrapper's cases (`scripts/test-ensure-clax.sh`, "grok guard …") and the binary's (`crates/clax-cli/tests/standdown.rs`) cover each layer on its own.
2. **The guard never fires outside Grok.** Claude Code (`CLAUDE_PID` is the MCP server's parent), a Claude Code session nested in a Grok shell, Codex (`--agent codex`) and Pi (no wrapper) still act. A Grok session nested in a Claude Code shell has an inherited `CLAUDE_PID` that is not the parent, so the Claude copy stands down there. Tests: the guard cases in both layers.
3. **A standing-down server is a working MCP server, not a failure.** `initialize` succeeds with `instructions`. `tools/list` offers exactly `status`. `tools/call status` returns the explanation with `isError: false`. `ping` answers. It needs no `clax` binary, no daemon and no network. A standing-down hook reads its stdin to the end, prints nothing and exits 0.
4. **Grok's hooks never claim comments the agent will not see.** Grok discards `SessionStart` output and the output of an allowing `UserPromptSubmit`. So `session-start --agent grok` joins without asking for `prompt_hook` feedback, and `prompt --agent grok` does nothing. If either fetched `tier=prompt_hook`, the daemon would mark comments delivered that never reached the agent. `stop --agent grok` acts only on `reason == "end_turn"` (or no `reason`). Tests: `crates/clax-hooks/src/events.rs` unit tests and the `grok_*` cases in `crates/clax-hooks/tests/golden.rs`.
5. **`clax init` never touches the Grok registration named `clax`.** In Grok that name is the Claude Code plugin's install under `~/.claude`, and a `grok plugin uninstall clax` could delete Claude Code's files. Every Grok command names `clax-grok`. Init works when `grok` is the only harness on `PATH`. Tests: `crates/clax-cli/tests/init.rs` (`grok_*`).
6. **Normalising Grok's envelope does not change Claude Code or Codex parsing.** Grok sends both `sessionId` and `session_id`, and `stopHookActive` with no snake_case alias. Serde aliases would make that a duplicate-field error, so the parser copies the camelCase values after deserialising. Tests: `crates/clax-hooks/src/input.rs`.
7. **A monitor notice wakes the agent and never delivers.** A notice never sets `delivered_at`. Each row is announced at most once per target session, even with two followers, and never after a tier has delivered it. Notices pause while the session is in `wait_for_feedback`. A woken agent receives the comment through tier 1, 2 or 4, exactly once. `clax feedback follow` prints nothing but notice lines, survives a daemon restart, and exits 0 when its session ends. Tests: `crates/clax-core/src/store/feedback.rs` (`notices_*`), `crates/clax-server/tests/api_notices.rs`, `crates/clax-cli/tests/follow.rs`, and the monitor case in `grok_dedupe.rs`.

---

## Design decisions

These follow from `decisions.md`, the research and the Grok source. Each is binding for the tasks below.

### Dedupe: in a Grok session, only `--agent grok` acts

**What Grok does with two Clax plugins.** Grok discovers Claude Code's installed plugins (`~/.claude/plugins/installed_plugins.json` and `~/.claude/plugins/*/`) as User-scope plugins, which start disabled (research §2). A person can enable the one named `clax`. Grok merges MCP servers from every source into one map keyed by the plain server name. In its own words, "The first definition of a name wins". A later server with the same name is dropped, not renamed and not namespaced by plugin (`xai-grok-config/src/mcp_servers.rs` at `2bdd1d6a`: `non_toml_mcp_servers_with_origin`, "Callers keep the first entry for each name"; `plugin_oauth_configs` uses `entry(name).or_insert`). Hooks are not merged: every enabled plugin's hooks run.

**The rule.** Clax acts in a Grok session only through `--agent grok`. The Claude Code copy (`--agent claude`) stands down whenever Grok runs it, and that holds whether or not clax-grok is loaded:

- **Hook mode:** it reads stdin to the end, prints nothing, exits 0, and logs one `standdown` line.
- **MCP mode:** it serves a stand-down server whose one tool, `status`, returns this text (with `isError: false`):

  > This is the Clax plugin for Claude Code, which Grok Build also loads. In Grok, Clax runs from the clax-grok plugin, whose tools are named `clax_grok__<tool>` (for example `clax_grok__publish`); this server does nothing. If no `clax_grok` tools are listed, run `clax init --agent grok`. To remove this server from Grok, run `grok plugin disable clax`.

- **CLI mode** (the Claude copy's slash commands, which run `ensure-clax.sh exec open …`): unchanged. CLI commands carry no session, so they cannot duplicate anything.

**clax-grok's MCP server is named `clax_grok`.** If it were named `clax`, the same name as the Claude copy's server, Grok would keep whichever it saw first and drop the other. Research §5.1 suggests `~/.claude/...` sorts first. If the Claude copy were enabled, clax-grok's working server could then vanish behind a stand-down server. With distinct names, both load in any discovery order, and the dedupe depends only on the guard. Tools are therefore `clax_grok__<tool>` in Grok (`use_tool` with that qualified name), and the permission rule is `MCPTool(clax_grok__*)`. This choice is listed under "Raised by the plan" in `open-questions.md`.

**How a run knows Grok started it.** Both layers use the same test:

| Mode | "Grok runs this Claude copy" when | Why this signal |
|---|---|---|
| hook | `GROK_HOOK_EVENT` is non-empty (the binary also accepts Grok's envelope key `hookEventName` on stdin) | Grok sets it on every hook process (research §2, Hooks). It is not in the environment of Grok's shell tool, so a Claude Code session started from Grok's shell does not inherit it. |
| mcp | `GROK_SESSION_ID` is non-empty **and** `CLAUDE_PID` is not the process's parent PID | Grok sets `GROK_SESSION_ID` for every stdio MCP server it spawns. Claude Code sets `CLAUDE_PID` to its own PID and spawns the server directly, so under Claude Code `CLAUDE_PID == PPID`. This was checked in a Claude Code Bash tool: `CLAUDE_PID` equals the parent `claude` process. `CLAUDE_CODE_SESSION_ID` cannot be the signal, because Claude Code exports it to its Bash tool, so a Grok started there inherits it. |

Nesting, both ways:
- **Claude Code started from a Grok shell.** Claude's server has `GROK_SESSION_ID` (inherited) and `CLAUDE_PID == PPID`, so it acts. Its hooks have no `GROK_HOOK_EVENT`, so they act.
- **Grok started from a Claude Code shell.** The server Grok spawns for the Claude copy has a fresh `GROK_SESSION_ID` and an inherited `CLAUDE_PID` that is not its parent (Grok is), so it stands down. Hooks have `GROK_HOOK_EVENT`, so they stand down.

**Every combination in one Grok session:**

| Claude copy in Grok | clax-grok | MCP servers Grok starts | Acting MCP server | Acting hooks | What the person sees |
|---|---|---|---|---|---|
| disabled (Grok's default) | enabled | `clax_grok` | `clax_grok` | clax-grok's | normal |
| enabled | enabled | `clax` (stand-down) and `clax_grok` | `clax_grok` | clax-grok's; the Claude copy's exit 0 silently | normal, plus an idle `clax__status`; `doctor --agent grok` suggests `grok plugin disable clax` |
| enabled | not installed, or disabled | `clax` (stand-down) | none | none | `clax__status` says to run `clax init --agent grok`; the server shows as connected, not failed |
| disabled | not installed | none | none | none | no Clax; `doctor --agent grok` says the plugin is not installed |

**Why not a per-session lock in the shim?** A lock picks the first server to start, not the right one. Grok spawns servers concurrently, so the winner would vary between runs. Hooks have no lifetime that could hold a lock. And with distinct server names and the guard, no second `--agent grok` server exists: two plugins named `clax-grok` resolve to one by Grok's plugin-name conflict rule, and a second `clax_grok` server definition is dropped by the server merge.

**Why `clax init` does not disable the Claude copy in Grok.** It is already disabled by default. A person who enables it has chosen to, and re-disabling it on every `clax init` would undo that choice without explaining it. In Grok, the name `clax` refers to Claude Code's install, so Clax never runs a `grok plugin` command that names it (Review Focus 5). The guard is still needed for a person who enables the copy after `init`. `clax doctor --agent grok` reports the idle copy, with the command that removes it.

**Why a stand-down server rather than an exit.** A stdio server that exits during the handshake is a failed server. Clients show it as failed (the reasoning behind the wrapper's fallback server in the stable-install plan), and the research found nothing showing that Grok treats such a server differently. A completed handshake with one informational tool is visible and harmless. Grok shows MCP tools to the model only through `search_tool` and `use_tool`, so the idle tool costs nothing until something searches for it.

### Grok's tiers and hooks

Tiers 1, 2, 4 and 5 apply (`decisions.md`). Tier 3 does not exist in Grok: an allowing `UserPromptSubmit` hook's stdout is discarded, and `SessionStart` output is ignored. The daemon's `waiting_on` for an armed Grok watch is `stop_hook`.

**Tier 5 is a notice, not a delivery.** Grok has no CLI or socket that puts a message into someone else's session. Its `monitor` tool, which the agent starts, turns each line a command prints into a notification that wakes the session for a new turn (research §2, "Pushing a message into a running session"). The clax-grok skill has the agent start one persistent monitor per session, after its first publish (publishing arms a watch), on `"<binary.path>" feedback follow --agent grok --harness-session <session.harness_session_id>`, with both values taken from `clax_grok__status`. It starts none when `status.push.available` is already true. `clax feedback follow` long-polls `GET /api/sessions/<sid>/notices` and prints one line per comment:

> `[clax] New comment on "<title>" (<url>), thread <thread ID>. Call comments_read with url_or_id "<artifact ID>" and thread_id "<thread ID>" to read it; if you have already handled it, do nothing.`

The line is a pointer, not the payload. The daemon counts it as a notice: it sets `feedback.notified_at` on the row, only where that is still unset and the row is undelivered, and it never sets `delivered_at`. That avoids double delivery by construction. The comment is delivered once, by whichever of the existing tiers hands it over first: tier 1 when the woken agent calls `comments_read` (the tool result carries it in-band, acknowledged), tier 2 when the woken turn ends, or tier 4 inside `wait_for_feedback`. `notified_at` keeps two followers, or a restarted one, from announcing a row twice, and it is cleared when a row is retargeted to a new session. While the session is in `wait_for_feedback`, notices pause, as tier 5 does for Codex and Pi. A notice can reach a busy session after its Stop hook has already delivered the same comment (Grok queues the line until the turn ends), which is why the line ends `if you have already handled it, do nothing`. `status`'s `push` for Grok is `{"tier": "monitor", "available": <a follower is connected>, "reason": <why not, when not>}`.

The command is harness-neutral (`--session <Clax session ID>`, or `--agent <harness> --harness-session <ID>`), with Grok's `GROK_SESSION_ID` as the only environment default. A later Claude Code fallback can run it as a background command that exits after its first line (a `--once` flag to add then), so the exit wakes the session. This plan does not wire Claude Code.

| Hook | clax-grok `hooks.json` | `clax hook --agent grok` |
|---|---|---|
| `SessionStart` | `timeout: 5` | joins by `sessionId`, fills `cwd`, prints nothing, and does not fetch `prompt_hook` feedback |
| `Stop` | `timeout: 10` | as Claude's, but only when `reason` is `end_turn` or absent; `stopHookActive` read from the camelCase key |
| `SessionEnd` | `timeout: 2` | ends the row; gives up after 1.2 s (1 s per request), inside Grok's 1.5 s default `SessionEnd` budget |
| `UserPromptSubmit` | not wired | `prompt --agent grok`, if run by hand, does nothing |

### Installing, dev and doctor

- `clax init` (and `--agent grok`) runs `grok plugin uninstall clax-grok --confirm` (failure ignored), then `grok plugin install <home>/marketplace/plugins/clax-grok --trust` (required). It records `{"plugin": "clax-grok", "source": <dir>}` under `grok` in `registrations.json`. `clax uninit` runs the uninstall. The plugin directory is named after the plugin, as Codex requires for its own. Nothing depends on Grok marketplaces. The tree also carries a root `.grok-plugin/marketplace.json` listing clax-grok. Without it, a person who added `~/.clax/marketplace` to Grok as a marketplace would be offered the Claude Code plugin, because Grok falls back to `.claude-plugin/marketplace.json` (research §5.2). **Provisional (Q4 a, b):** the flags and the `plugin list --json` shape are read from the guide, not run.
- `just dev grok` runs the real `grok` with its real `GROK_HOME` and the installed clax-grok plugin, with the fresh build first on `PATH` and `CLAX_HOME=~/.clax-dev` (`decisions.md`: dev works as for the other harnesses; no single-run override for Grok).
- `clax doctor --agent grok` checks `binary`, `upgrade`, `plugin` (a `clax-grok` manifest under `$GROK_HOME`), `skill`, `mcp`, `hooks` (expected to have run, as for Claude Code), `feedback`, and two Grok checks: `grok` (its `--version`, with a warning below 1.0.45; **provisional (Q3)**) and `claude_copy` (the Claude copy's `standdown` lines in `hooks.log`).
- Tool approval: Clax writes no Grok permission rule. The README and the `mcp` check's detail give `[permission] allow = ["MCPTool(clax_grok__*)"]` (**provisional (Q1)**).
- Sandboxed Grok: no code. The README and the contract's Known limitations describe it (**provisional (Q2)**).
- PostToolUse: not added (**provisional (Q5)**).

## File Structure

| Path | Responsibility |
|---|---|
| `crates/clax-server/src/routes/sessions.rs` | `grok` in `HARNESSES`; Grok's `push` |
| `crates/clax-core/src/store/feedback.rs` | `waiting_on`: an armed Grok watch waits on `stop_hook` |
| `crates/clax-mcp/src/shim.rs` | `Harness::Grok`: `GROK_SESSION_ID`, the working directory |
| `crates/clax-mcp/src/plugin.rs` | `.grok-plugin/plugin.json`, `GROK_PLUGIN_ROOT` |
| `crates/clax-mcp/src/standdown.rs` (new) | The stand-down MCP server: one `status` tool |
| `crates/clax-cli/src/host.rs` (new) | Whether Grok runs this Claude copy; the `standdown` log line |
| `crates/clax-cli/src/commands/mcp.rs`, `hook.rs` | `--agent grok`; the stand-down guard |
| `crates/clax-hooks/src/input.rs`, `events.rs` | Grok's envelope; quiet join; `end_turn` filter |
| `crates/clax-core/src/store/feedback.rs`, `feedback.rs`, `migrations.rs` | `feedback.notified_at`; `take_notices`; the notice line |
| `crates/clax-server/src/routes/feedback.rs`, `feedback.rs` | `GET /api/sessions/<sid>/notices`; the follower registry |
| `crates/clax-cli/src/commands/feedback.rs` (new) | `clax feedback follow` |
| `scripts/ensure-clax.sh` (+ 3 copies) | The wrapper's stand-down guard and stand-down server |
| `plugins/clax-grok/` (new) | Manifest, `.mcp.json`, hooks, skill, README, wrapper copy |
| `.grok-plugin/marketplace.json` (new) | Grok marketplace index listing clax-grok |
| `crates/clax-cli/src/plugins.rs`, `build.rs` | Embed clax-grok and the Grok index |
| `crates/clax-cli/src/commands/init.rs` | The `grok` harness entry |
| `crates/clax-cli/src/commands/doctor_agent.rs` | `--agent grok`; `grok` and `claude_copy` checks |
| `scripts/dev.sh`, `scripts/test-dev.sh`, `justfile` | `just dev grok` |
| `crates/clax-cli/tests/grok_dedupe.rs` (new) | A fake Grok runs every combination end to end |
| `scripts/smoke-grok.sh` (new) | The owner's live check against real `grok` |
| `scripts/test-plugins.sh`, `check-version.sh`, `bump-version.sh`, `sync-skill-tools.py` | The fourth plugin in the checks |

---

### Task 1: Spec and contract amendments

**Files:**
- Modify: `docs/superpowers/specs/2026-09-28-clax-design.md` (§2, §3, §4, §10, §11, §12, §13, §16, §18)
- Modify: `docs/contract.md` (Tools, Sessions, Delivery tiers per harness, Installation and the wrapper, `clax doctor --agent`, Known limitations)

**Interfaces:**
- Produces: the binding text that Tasks 2–11 implement. No code.

- [ ] **Step 1: §2 Decisions: add D17 after D16**

```markdown
| D17 | Grok Build is a fourth harness, served by a separate plugin, `clax-grok`, whose MCP server is named `clax_grok`; it gets tiers 1, 2, 4 and 5, tier 5 being notices from `clax feedback follow` under Grok's agent-started `monitor` tool, which wake the session and deliver nothing. Grok also loads the Claude Code plugin, so in a Grok session only `--agent grok` acts: the Claude Code copy stands down (its hooks exit 0 silently; its MCP server offers one `status` tool naming clax-grok), detected by `GROK_HOOK_EVENT` for hooks and by `GROK_SESSION_ID` with a `CLAUDE_PID` that is not the server's parent for MCP | Grok discovers Claude Code plugins and keeps only the first MCP server of a name, so a shared plugin or server name would let one copy shadow the other; deciding by harness rather than by start order gives one acting copy in every combination. |
```

- [ ] **Step 2: §3 Architecture**

In the diagram, replace `│ harness: Claude Code | Codex | Pi        │` with `│ harness: Claude Code | Codex | Grok | Pi │`. In the Shim bullet, replace `` (`clax mcp --agent <claude|codex>`) `` with `` (`clax mcp --agent <claude|codex|grok>`) ``.

- [ ] **Step 3: §4 Repository layout**

After the `clax/` entry under `plugins/`, add:

```
  clax-grok/                       Grok Build plugin: .grok-plugin/plugin.json, .mcp.json, hooks/, skills/, scripts/
```

After the `.agents/plugins/marketplace.json` line, add:

```
.grok-plugin/marketplace.json      Grok Build marketplace index pointing at plugins/clax-grok
```

Replace `(copied into both plugins' scripts/)` with `(copied into each MCP plugin's scripts/)`.

- [ ] **Step 3b: §5 Storage and §6 HTTP API**

In §5, in the `feedback(…)` row list, after `untargeted_at, push_failed_at` add `, notified_at`, and after the sentence that ends `drive resends (§10).` add `` `notified_at` records when `clax feedback follow` announced the row to its target (a notice, §10 "Notices"); it is cleared when the row is retargeted. ``

In §6, after the Feedback bullet, add:

```markdown
- Notices: `GET /api/sessions/<sid>/notices?wait=<secs>` (W; long-poll,
  capped at 600 s; returns `{notices: [{feedback_id, comment_id,
  thread_id, artifact_id, title, url}], lines, waited_s}` for the
  session's armed rows that no tier has delivered and no follower has
  announced, and sets their `notified_at`; delivers nothing; answers
  empty at once while the session is in `wait_for_feedback`; 404 for an
  unknown session, 400 `unknown_session` for an ended one).
```

- [ ] **Step 4: §10 Delivery tiers**

In the tier table:
- Tier 1, Harnesses: `all three` → `all four`.
- Tier 2, Harnesses: replace the cell with `Claude Code, Codex and Grok Build (Claude Code and Codex verified: {"decision":"block","reason":...} on stdout with exit 0 continues the turn with the reason as input and the hook fires again with stop_hook_active: true; Grok from source: the same, with stopHookActive, and the hook acts only on reason end_turn)`, keeping the existing backticks around the JSON and field names.
- Tier 3, Harnesses: append `; Grok Build: none (it discards an allowing UserPromptSubmit hook's output and ignores SessionStart output)`.
- Tier 4, Harnesses: `all three` → `all four`; in Failure modes, after `(Codex defaults to 60 s)`, add `; Grok's default is 6000 s`.
- Tier 5, Harnesses: append ` Grok Build: notices from clax feedback follow, run by Grok's monitor tool, which the agent starts (below).` Latency: append `; Grok: at once when idle (each line starts a turn), after the current turn when busy (from source)`. Failure modes: append ` A Grok session is woken only once its agent has started the monitor, and never under headless grok -p.`

After the paragraph that begins `The same honesty applies to Claude Code`, add:

```markdown
Grok Build has no push either, but its `monitor` tool turns each line a
command prints into a turn. Once the agent has started a monitor on
`clax feedback follow` (the skill does so after its first publish), each
comment sent to it wakes an idle session with a notice (see "Notices").
Before that, and in headless `grok -p`, comments reach it at the end of
a turn (tier 2), on its next clax tool call (tier 1), or inside a
`wait_for_feedback` loop (tier 4).
```

After "### Delivery and acknowledgement", add a subsection:

```markdown
### Notices (Grok's monitor)

`clax feedback follow` long-polls `GET /api/sessions/<sid>/notices` and
prints one line per comment sent to the session, naming the artifact and
thread and saying to call `comments_read`. It never prints the comment.
The daemon announces a row only when no tier has delivered it, no
follower has announced it to this session (`notified_at` unset), and the
session watches the artifact with replies armed, and it sets
`notified_at` as it announces. A notice is not a delivery: the row still
waits for tiers 1, 2 and 4, which deliver it once under the rules above,
so a monitor never causes a second delivery. Retargeting a row clears
`notified_at`. While the session is inside `wait_for_feedback`, the
notices poll answers empty at once and announces nothing. The command
finds the session by `--session`, by `--agent` and `--harness-session`,
or by `GROK_SESSION_ID`; it never starts a daemon, follows the session
across daemon restarts, and exits 0 once the session has ended (at once
for `--session`; after 60 s with no live Clax session for a harness
session). `status`'s `push` for a Grok session is `{"tier": "monitor",
"available": <a follower polled within 15 s>, "reason": …}`.
```

- [ ] **Step 5: §11 Sessions and identity**

After the paragraph that ends `as toolpath's plugin relies on.`, add:

```markdown
Grok Build passes `GROK_SESSION_ID` to the stdio MCP servers it spawns
for a session, and to hooks (with the session ID in their input as
`sessionId` and `session_id`). The shim under `--agent grok` registers
with that ID and its own working directory (Grok's), so the row is keyed
by the Grok session ID from the start and hooks join by ID. In Grok's
default mode the shim's parent is `grok`; in leader mode it is the
leader process. Subagents share the parent session's MCP servers, so
their clax calls count as the parent session.
```

- [ ] **Step 6: §12 MCP tool surface**

Replace `Codex
as `mcp__clax__<name>`, and the Pi extension registers them as
`clax_<name>`.` with:

```markdown
Codex
as `mcp__clax__<name>`, Grok Build as `clax_grok__<name>` (reached
through its `search_tool` and `use_tool` meta-tools), and the Pi
extension registers them as `clax_<name>`.
```

- [ ] **Step 7: §13 Plugins: a Grok Build subsection before `### Pi`**

```markdown
### Grok Build (`plugins/clax-grok`)

The directory is named after the plugin. The plugin is not named `clax`,
because Grok also discovers the Claude Code plugin of that name
(`~/.claude/plugins`, User scope, disabled until the person enables it)
and resolves plugin-name conflicts before enabling.

- `.grok-plugin/plugin.json`: name `clax-grok`, version, description.
  Grok finds the other components by convention.
- `.mcp.json`: `{"mcpServers": {"clax_grok": {"command":
  "${GROK_PLUGIN_ROOT}/scripts/ensure-clax.sh", "args": ["exec", "mcp",
  "--agent", "grok"], "env": {"GROK_PLUGIN_ROOT": "${GROK_PLUGIN_ROOT}"}}}}`.
  The server is named `clax_grok`, not `clax`: Grok keeps the first MCP
  server definition of a name and drops the rest, so a shared name would
  let the Claude Code copy's server shadow this one. Grok passes its whole
  environment, plus `GROK_SESSION_ID`, to stdio servers.
- `hooks/hooks.json` in Claude Code's format, each command
  `"${GROK_PLUGIN_ROOT}/scripts/ensure-clax.sh" exec hook --agent grok
  <event>`: `SessionStart` → `session-start` (joins by `sessionId`, fills
  `cwd`, prints nothing: Grok ignores its output), `Stop` → `stop`
  (tier 2, timeout 10 s; acts only when `reason` is `end_turn` or absent,
  reads `stopHookActive`), `SessionEnd` → `session-end` (timeout 2 s;
  `session-end` gives up after 1.2 s, inside Grok's 1.5 s default). No
  `UserPromptSubmit` hook: Grok discards an allowing one's output.
- `skills/clax/SKILL.md`: the same skill as the other plugins, with tools
  named `clax_grok__<tool>` and called through `use_tool`, plus a "Live
  feedback in Grok" section: after its first publish in a session, unless
  `clax_grok__status` reports `push.available`, the agent starts Grok's
  `monitor` tool with `persistent: true` on `"<binary.path>" feedback
  follow --agent grok --harness-session <session.harness_session_id>`
  (both from `status`), and answers each line by calling
  `clax_grok__comments_read` on the thread it names.
- `scripts/ensure-clax.sh`: a copy of the wrapper.
- Installed by `clax init` (`grok plugin uninstall clax-grok --confirm`,
  then `grok plugin install ~/.clax/marketplace/plugins/clax-grok --trust`);
  `clax uninit` uninstalls it. Clax never runs a `grok` command naming
  `clax`, which in Grok is the Claude Code plugin's install. Root
  `.grok-plugin/marketplace.json` lists it, so a Grok marketplace added
  from `~/.clax/marketplace` offers clax-grok, not the Claude Code plugin.
- `just dev grok`, like `just dev codex`, runs the installed plugin with
  the fresh build first on `PATH`, on `~/.clax-dev`.

**One acting copy.** In a Grok session only `--agent grok` acts. The
wrapper and the binary both treat a `--agent claude` run as started by
Grok when, for a hook, `GROK_HOOK_EVENT` is set (or the input carries
Grok's `hookEventName`), or, for the MCP server, `GROK_SESSION_ID` is set
and `CLAUDE_PID` is not the server's parent process (Claude Code sets
`CLAUDE_PID` to its own PID, so a Claude Code session nested in a Grok
shell still acts). Such a hook reads its input, prints nothing and exits
0. Such an MCP server completes the handshake and offers one tool,
`status`, which says that Clax runs from clax-grok and how to install it
or disable the idle copy (`isError: false`). Each stand-down appends a
`standdown mode=<hook|mcp> agent=claude host=grok` line to
`~/.clax/logs/hooks.log`.

Person-side settings, documented in the plugin README rather than set by
Clax:

- Grok asks before each MCP tool call in its default `ask` mode;
  `[permission] allow = ["MCPTool(clax_grok__*)"]` in
  `~/.grok/config.toml` approves every Clax tool, including `delete`.
  Headless `grok -p` needs `--always-approve` or that rule.
- Grok's sandbox, when turned on, covers the MCP server, the hooks and any
  daemon they start. Start the daemon outside Grok (`clax serve`) or use a
  custom profile with `read_write = ["~/.clax"]`.
```

- [ ] **Step 8: §16 Testing**

In the Plugins bullet, replace `` against fake `claude`, `codex` and `pi` commands `` with `` against fake `claude`, `codex`, `grok` and `pi` commands ``. Before `; the Pi extension through a fake`, add `; a fake Grok (crates/clax-cli/tests/grok_dedupe.rs) that loads both Clax plugins with Grok's first-definition-wins server merge and checks that one copy acts in every combination; a manual Grok smoke test (scripts/smoke-grok.sh)`, with backticks around the two paths.

In the clax-hooks bullet, replace `including `stop_hook_active` and the Codex `stop` shape` with `including `stop_hook_active`, the Codex `stop` shape, and Grok's camelCase envelope`.

- [ ] **Step 9: §18 Open questions**

Append a bullet:

```markdown
- Grok Build: the tool-approval rule, sandboxed Grok, the minimum Grok
  version, the live checks, and a PostToolUse hand-over are recorded with
  recommendations in `.superpowers/sdd/2026-10-01-grok/open-questions.md`;
  the build follows the recommendations until the owner decides otherwise.
```

- [ ] **Step 10: Contract, Tools**

In the shim bullet, replace `` `clax mcp --agent <claude|codex>` `` with `` `clax mcp --agent <claude|codex|grok>` ``. In "Names as the model sees them", add a row after Codex's:

```markdown
| Grok Build | `clax_grok__<tool>`, through `search_tool` and `use_tool` |
```

- [ ] **Step 11: Contract, Sessions**

Replace `The shim takes `harness_session_id` from `CLAUDE_CODE_SESSION_ID` under Claude
Code, else from `CLAX_SESSION_ID` for any harness, else sends none.` with:

```markdown
The shim takes `harness_session_id` from `CLAUDE_CODE_SESSION_ID` under Claude
Code and from `GROK_SESSION_ID` under Grok Build, else from `CLAX_SESSION_ID`
for any harness, else sends none.
```

Replace `` (`claude`, `codex`, `pi`) `` with `` (`claude`, `codex`, `grok`, `pi`) ``, `only those three
harness names` with `only those four
harness names`, and `harness must be one of
claude, codex, pi` with `harness must be one of
claude, codex, grok, pi`. In the hooks paragraph, after `` `session-end` after 2.5 s (2 s per request, inside
Codex's 3 s `SessionEnd` cap) ``, add `` (under Grok, 1.2 s with 1 s per request, inside Grok's 1.5 s default) ``.

Add a subsection after `### Codex`:

```markdown
### Grok Build

Grok passes `GROK_SESSION_ID` to the MCP servers it starts for a session.
The shim registers with `harness_session_id` set to it and `cwd` set to its
own working directory (Grok starts servers in its own). The `SessionStart`
hook (`clax hook --agent grok session-start`) joins by the `sessionId` in
its input, fills an empty `cwd`, and prints nothing, because Grok ignores
`SessionStart` output. The `Stop` hook (`clax hook --agent grok stop`) hands
comments over at the end of a turn. It reads `stopHookActive` (Grok has no
snake_case alias for it), and does nothing when `reason` is present and is
not `end_turn`, which skips the Stop that Grok fires at session end. The
`SessionEnd` hook ends the row by ID. There is no prompt hook: Grok discards
an allowing `UserPromptSubmit` hook's output.

Grok also loads the Clax Claude Code plugin when the person enables it in
Grok. That copy stands down: see "The wrapper". Exactly one Clax MCP server
and one set of Clax hooks act in a Grok session.
```

- [ ] **Step 12: Contract, Delivery tiers per harness**

Replace the measurement line with `Measured on 2026-09-29 with Codex CLI 0.158.0 and Claude Code 2.1.284; Pi 0.73.1 from its source; Grok Build 1.0.45 from its source (not yet run live; scripts/smoke-grok.sh records the measured version).`, with backticks around the path. Add a Grok column to the table, between Codex and Pi:

| Tier | Grok Build |
|---|---|
| 1 | next clax tool call (shim) |
| 2 | end of the turn: the same shape continues the turn; the hook acts only on `reason` `end_turn` |
| 3 | none: an allowing `UserPromptSubmit` hook's output is discarded and `SessionStart` output is ignored |
| 4 | immediate while waiting; Grok's tool timeout defaults to 6000 s |
| 5 | once the agent has started the monitor (`clax feedback follow`): a notice line wakes an idle session at once and a busy one after its turn; it points at the comment, which then arrives through tier 1, 2 or 4 (from source; not run live) |

- [ ] **Step 12b: Contract, Comments and feedback**

After "### Acknowledgement and resends", add a "### Notices (Grok's monitor)" subsection with the same text as the spec's §10 "Notices" (Step 4), plus the line format:

```markdown
    [clax] New comment on "<title>" (<url>), thread <thread ID>. Call comments_read with url_or_id "<artifact ID>" and thread_id "<thread ID>" to read it; if you have already handled it, do nothing.

The title is put on one line, double quotes become single quotes, and it is
cut to 80 characters.
```

- [ ] **Step 13: Contract, Installation and the wrapper**

In "`clax init` and `clax uninit`", after `` `pi install ~/.clax/marketplace/plugins/pi` ``, add `` ; `grok plugin uninstall clax-grok --confirm` (failure ignored) and `grok plugin install ~/.clax/marketplace/plugins/clax-grok --trust` ``. After the paragraph that starts `When in doubt, a registration is kept.`, add:

```markdown
Grok registrations are removed by the name `clax-grok` only. In Grok, the
name `clax` is the Claude Code plugin that Grok discovers in
`~/.claude/plugins`, and Clax never runs a `grok` command that names it.
`clax uninit` keeps the marketplace while `grok plugin list --json` still
names a path under it, and also when `grok` is not on `PATH` but
`registrations.json` records a Grok registration.
```

In "The wrapper", replace `The Claude Code and Codex plugins start` with `The Claude Code, Codex and Grok plugins start`. Add at the end of the subsection:

```markdown
In a Grok session the wrapper stands the Claude Code copy down before it
looks for `clax`. A run with `--agent claude` counts as started by Grok
when it is a hook with `GROK_HOOK_EVENT` set, or the MCP server with
`GROK_SESSION_ID` set and a `CLAUDE_PID` that is not its parent process.
Such a hook reads its stdin and exits 0 with no output. Such an MCP server
answers `initialize` (with `instructions` stating the same text),
`tools/list` with one tool, `status`, whose call returns the text below
with `isError: false`, and `ping`; any other request gets JSON-RPC error
-32601. `clax mcp --agent claude` and `clax hook --agent claude` apply the
same rule themselves (a hook also counts as Grok's when its input has
`hookEventName`), so a stale wrapper or a stale binary still stands down.
Either layer appends `standdown mode=<hook|mcp> agent=claude host=grok` to
`hooks.log`. The text:

> This is the Clax plugin for Claude Code, which Grok Build also loads. In
> Grok, Clax runs from the clax-grok plugin, whose tools are named
> `clax_grok__<tool>` (for example `clax_grok__publish`); this server does
> nothing. If no `clax_grok` tools are listed, run `clax init --agent
> grok`. To remove this server from Grok, run `grok plugin disable clax`.
```

- [ ] **Step 14: Contract, `clax doctor --agent`**

Replace `` `clax doctor --agent <claude|codex|pi>` `` with `` `clax doctor --agent <claude|codex|grok|pi>` ``. In the `hooks` bullet, replace `or when no Claude Code hook has run` with `or when no Claude Code or Grok hook has run`. Add two bullets at the end of the list:

```markdown
- `grok` (Grok only): `grok --version`; failed when `grok` is not on
  `PATH` or reports a version older than 1.0.45.
- `claude_copy` (Grok only, never failed): whether the Claude Code plugin
  has stood down in a Grok session (its `standdown` lines in `hooks.log`),
  with `grok plugin disable clax` to remove its idle server.
```

- [ ] **Step 15: Contract, Known limitations**

Append:

```markdown
- Grok Build's sandbox, when turned on, covers the shim, the hooks and a
  daemon the shim starts. Under the `workspace`, `read-only` and `strict`
  profiles that daemon cannot write `~/.clax`; on Linux, `read-only` and
  `strict` may block loopback too. Start the daemon outside Grok
  (`clax serve`) or use a custom profile with `read_write = ["~/.clax"]`.
- A Grok session started from a Claude Code shell inherits `CLAUDE_PID`;
  that is not its MCP server's parent, so the Claude Code copy stands down
  there as it should. A Claude Code session whose `CLAUDE_PID` is unset
  but that inherits `GROK_SESSION_ID` from a Grok shell would stand its
  own Clax down.
- Grok's tier 5 needs the agent to start the monitor; a session whose agent
  never publishes, or skips the skill's step, is woken by nothing. Headless
  `grok -p` sessions end with the process, so they have no monitor.
- Whether Grok starts a new MCP server with the new `GROK_SESSION_ID` on
  `/new` or `/resume` within one process is not yet measured; until it
  is, a resumed Grok session may keep the Clax session of its first
  conversation.
```

- [ ] **Step 16: Check and stage**

Run `scripts/test-plugins.sh` (the name gate and the contract's `## Page contract` and tool list checks read these files). Expected: `plugin checks passed`. Run the quality gates. Stage the two files.

Proposed commit message: `Specify Grok Build as a fourth harness: the clax-grok plugin, tiers 1, 2, 4 and 5 (monitor notices), and one acting Clax copy per Grok session`

---

### Task 2: The `grok` harness in the daemon and the shim

**Files:**
- Modify: `crates/clax-server/src/routes/sessions.rs` (`HARNESSES`, `push_info`)
- Modify: `crates/clax-core/src/store/feedback.rs` (`waiting_on`, a test)
- Modify: `crates/clax-server/tests/api_sessions.rs`
- Modify: `crates/clax-mcp/src/shim.rs` (`Harness::Grok`, `registration`, tests)
- Modify: `crates/clax-mcp/src/plugin.rs` (`MANIFESTS`, `root_from_env`, tests)
- Modify: `crates/clax-cli/src/commands/mcp.rs` (`Agent::Grok`)
- Modify: `crates/clax-mcp/tests/shim.rs` (one case)

**Interfaces:**
- Consumes: Task 1's §11 and contract Sessions text.
- Produces: `POST /api/sessions` and `/api/sessions/join` accept `harness: "grok"`. `clax mcp --agent grok` registers `{harness: "grok", harness_session_id: $GROK_SESSION_ID, cwd: <its working directory>}`. `clax_mcp::shim::Harness::Grok`. `clax_mcp::plugin::MANIFESTS` includes `.grok-plugin/plugin.json`, and `root_from_env` reads `GROK_PLUGIN_ROOT`. Task 3 uses the harness name; Tasks 7 and 10 use `--agent grok`.

- [ ] **Step 1: Write the failing daemon tests**

In `crates/clax-server/tests/api_sessions.rs`, change `register_accepts_only_known_harnesses`: the message becomes `"harness must be one of claude, codex, grok, pi"`, and the accepted list becomes `["claude", "codex", "grok", "pi"]`. Add:

```rust
#[tokio::test]
async fn a_grok_session_has_no_native_push() {
    let ts = TestServer::spawn().await;
    let s = register(
        &ts,
        json!({"harness": "grok", "harness_session_id": "019a-grok", "cwd": "/w", "pid": 10, "parent_pid": 5}),
    )
    .await;
    assert_eq!(s["harness_session_id"], "019a-grok");
    let sess: Value = ts
        .get_authed(&format!("/api/sessions/{}", s["id"].as_str().unwrap()))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(sess["push"]["tier"], Value::Null);
    assert_eq!(sess["push"]["available"], false);
    assert!(
        sess["push"]["reason"]
            .as_str()
            .unwrap()
            .starts_with("Grok Build has no native push"),
        "{sess}"
    );
}
```

In `crates/clax-core/src/store/feedback.rs` `mod tests`, add (next to `feedback_state_follows_the_row_through_its_life`, using its helpers `store`, `session`, `artifact` and `thread`):

```rust
#[test]
fn an_armed_grok_watch_waits_on_the_stop_hook() {
    let (_d, st) = store();
    let grok = session(&st, "grok", "g1");
    let aid = artifact(&st, Some(&grok));
    let tid = thread(&st, &aid, "hi");
    st.send_to_agent(&tid).unwrap();
    assert_eq!(
        st.feedback_state(&tid, false).unwrap().unwrap().tier,
        Some(Tier::Piggyback),
        "unarmed: the next tool call"
    );
    st.ensure_watch(&grok, &aid).unwrap();
    assert_eq!(
        st.feedback_state(&tid, false).unwrap().unwrap().tier,
        Some(Tier::StopHook)
    );
}
```

Run: `cargo test -p clax-server --test api_sessions -- harness grok` and `cargo test -p clax-core an_armed_grok_watch`. Expected: FAIL (400 for `grok`; the tier is `Piggyback`).

- [ ] **Step 2: Accept `grok` in the daemon**

In `crates/clax-server/src/routes/sessions.rs`:

```rust
/// The harness names a session may carry.
pub const HARNESSES: [&str; 4] = ["claude", "codex", "grok", "pi"];
```

In `push_info`, add an arm before the catch-all (Task 6 replaces it with the monitor's state):

```rust
        "grok" => {
            json!({"tier": null, "available": false, "reason": "Grok Build has no native push; comments arrive at the end of a turn (Stop hook), on the next clax tool call, or during wait_for_feedback"})
        }
```

In `crates/clax-core/src/store/feedback.rs`:

```rust
fn waiting_on(harness: &str, has_hsid: bool, armed: bool, codex_push: bool) -> Tier {
    match (harness, armed) {
        ("codex", true) if has_hsid && codex_push => Tier::Queue,
        ("pi", true) => Tier::Inject,
        ("claude" | "codex" | "grok", true) => Tier::StopHook,
        _ => Tier::Piggyback,
    }
}
```

Search for any other place that lists the three harnesses: `grep -rn '"claude", "codex", "pi"\|claude, codex, pi' crates web/src docs`. Update each one that names accepted harnesses (the shell's harness label, if it has one, shows `Grok`). Leave test fixtures that list harnesses for other reasons as they are.

Run the Step 1 tests. Expected: PASS.

- [ ] **Step 3: Write the failing shim tests**

In `crates/clax-mcp/src/shim.rs` `mod tests`, add:

```rust
#[test]
fn grok_uses_its_session_id_and_working_directory() {
    let r = registration(
        Harness::Grok,
        env(&[
            ("GROK_SESSION_ID", "019a-g"),
            ("CLAUDE_CODE_SESSION_ID", "cc-1"),
            ("CLAUDE_PROJECT_DIR", "/claude"),
            ("CLAX_SESSION_ID", "ax-1"),
        ]),
        Some(PathBuf::from("/work")),
        Some(PathBuf::from("/parent")),
        7,
        3,
    );
    assert_eq!(r.harness, "grok");
    assert_eq!(r.harness_session_id.as_deref(), Some("019a-g"));
    assert_eq!(r.cwd, "/work");
}

#[test]
fn grok_without_its_session_id_falls_back_to_clax_session_id() {
    let r = registration(
        Harness::Grok,
        env(&[("GROK_SESSION_ID", ""), ("CLAX_SESSION_ID", "ax-1")]),
        None,
        None,
        7,
        3,
    );
    assert_eq!(r.harness_session_id.as_deref(), Some("ax-1"));
    assert_eq!(r.cwd, "");
}
```

The second argument list follows the existing tests' order: `env`, `current_dir`, `parent_cwd`, `pid`, `parent_pid`.

In `crates/clax-mcp/src/plugin.rs` `mod tests`, add:

```rust
#[test]
fn a_grok_plugin_root_and_manifest_are_found() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".grok-plugin")).unwrap();
    std::fs::write(
        dir.path().join(".grok-plugin/plugin.json"),
        r#"{"name": "clax-grok", "version": "0.3.0"}"#,
    )
    .unwrap();
    assert_eq!(manifest_version(dir.path()).as_deref(), Some("0.3.0"));
    let root = dir.path().display().to_string();
    let env = move |k: &str| (k == "GROK_PLUGIN_ROOT").then(|| root.clone());
    assert_eq!(root_from_env(env, None), Some(dir.path().to_path_buf()));
    assert_eq!(
        root_from_env(|_| None, Some(dir.path().to_path_buf())),
        Some(dir.path().to_path_buf()),
        "a working directory holding a Grok manifest is a plugin root"
    );
}
```

Run: `cargo test -p clax-mcp grok`. Expected: FAIL (no `Harness::Grok`).

- [ ] **Step 4: Implement the shim side**

`crates/clax-mcp/src/shim.rs`:

```rust
/// The harness that spawned the shim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Harness {
    Claude,
    Codex,
    Grok,
}

impl Harness {
    /// The name recorded as the session's `harness`.
    pub fn as_str(self) -> &'static str {
        match self {
            Harness::Claude => "claude",
            Harness::Codex => "codex",
            Harness::Grok => "grok",
        }
    }
}

/// The session registration for `harness`, from the environment variable lookup
/// `env`, the process's working directory `current_dir`, and the parent
/// process's working directory `parent_cwd`. The harness session ID is
/// `CLAUDE_CODE_SESSION_ID` under Claude Code and `GROK_SESSION_ID` under
/// Grok Build, else `CLAX_SESSION_ID`. The working directory is
/// `CLAUDE_PROJECT_DIR` under Claude Code, else `current_dir`; under Codex it
/// is `parent_cwd`, else empty, because Codex starts the shim in the plugin's
/// own directory; under Grok it is `current_dir`, because Grok starts servers
/// in its own working directory. Empty variables count as unset.
pub fn registration(
    harness: Harness,
    env: impl Fn(&str) -> Option<String>,
    current_dir: Option<PathBuf>,
    parent_cwd: Option<PathBuf>,
    pid: u32,
    parent_pid: u32,
) -> RegisterSession {
    let var = |name: &str| env(name).filter(|v| !v.is_empty());
    let own_id = match harness {
        Harness::Claude => var("CLAUDE_CODE_SESSION_ID"),
        Harness::Grok => var("GROK_SESSION_ID"),
        Harness::Codex => None,
    };
    let harness_session_id = own_id.or_else(|| var("CLAX_SESSION_ID"));
    let dir = match harness {
        Harness::Claude => var("CLAUDE_PROJECT_DIR").map(PathBuf::from).or(current_dir),
        Harness::Codex => parent_cwd,
        Harness::Grok => current_dir,
    };
    // ... the rest of the function is unchanged
}
```

`crates/clax-mcp/src/plugin.rs`:

```rust
/// The manifests that carry a plugin's version, relative to its root: Codex,
/// Claude Code, Grok Build, and the Pi package.
pub const MANIFESTS: [&str; 4] = [
    ".codex-plugin/plugin.json",
    ".claude-plugin/plugin.json",
    ".grok-plugin/plugin.json",
    "package.json",
];

/// The plugin root of a shim: `CLAUDE_PLUGIN_ROOT`, else `GROK_PLUGIN_ROOT`,
/// else `PLUGIN_ROOT` (empty counts as unset), else `cwd` when it holds a
/// harness plugin manifest (Codex starts the server in the plugin root and
/// exports none of the variables).
pub fn root_from_env(
    env: impl Fn(&str) -> Option<String>,
    cwd: Option<PathBuf>,
) -> Option<PathBuf> {
    ["CLAUDE_PLUGIN_ROOT", "GROK_PLUGIN_ROOT", "PLUGIN_ROOT"]
        .iter()
        .find_map(|k| env(k).filter(|v| !v.is_empty()).map(PathBuf::from))
        .or_else(|| cwd.filter(|d| MANIFESTS[..3].iter().any(|m| d.join(m).is_file())))
}
```

`CLAUDE_PLUGIN_ROOT` stays first. Grok sets it to the same value as `GROK_PLUGIN_ROOT` for hooks, and under Claude Code an inherited `GROK_PLUGIN_ROOT` must not win.

`crates/clax-cli/src/commands/mcp.rs`: add `Grok` to `Agent` (doc: `` /// The harness that spawns the shim. Pi has no MCP client; its extension calls the daemon directly. ``, unchanged), and map `Agent::Grok => Harness::Grok`.

- [ ] **Step 5: A shim integration case**

In `crates/clax-mcp/tests/shim.rs`, the `Shim::start_in` builder runs `clax --port 0 mcp --agent claude`. Give it an agent parameter, or add a `start_grok` constructor that runs `--agent grok` with `GROK_SESSION_ID=019a-shim` and the harness environment cleared (Global Constraints). Then add:

```rust
#[tokio::test]
async fn a_grok_shim_registers_by_its_session_id() {
    let shim = Shim::start_grok("019a-shim").await;
    let live = shim.live_sessions().await;
    assert_eq!(live.len(), 1, "{live:?}");
    assert_eq!(live[0]["harness"], "grok");
    assert_eq!(live[0]["harness_session_id"], "019a-shim");
    shim.finish().await;
}
```

Run: `cargo test -p clax-mcp --test shim grok` and `cargo test -p clax-mcp`. Expected: PASS.

- [ ] **Step 6: Gates and stage**

Run the quality gates. Stage the seven files.

Proposed commit message: `Accept the grok harness: sessions keyed on GROK_SESSION_ID, with Stop-hook delivery`

---

### Task 3: `clax hook --agent grok`

**Files:**
- Modify: `crates/clax-hooks/src/input.rs` (normalise Grok's envelope; `stop_reason`, `is_grok_envelope`)
- Modify: `crates/clax-hooks/src/events.rs` (`join`, `session_start_quiet`, Grok's Stop filter; tests)
- Modify: `crates/clax-cli/src/commands/hook.rs` (`Agent::Grok`, per-agent budgets, Grok's `prompt`)
- Create: `crates/clax-hooks/tests/fixtures/grok-session-start.json`, `grok-stop.json`, `grok-stop-active.json`, `grok-stop-shutdown.json`, `grok-session-end.json`
- Modify: `crates/clax-hooks/tests/golden.rs` (the `grok_*` cases)

**Interfaces:**
- Consumes: Task 2 (`grok` sessions).
- Produces: `clax hook --agent grok <session-start|stop|session-end|prompt>`. `HookInput::parse` fills `session_id`, `stop_hook_active`, `transcript_path` and `hook_event_name` from Grok's camelCase keys when the snake_case ones are absent. `HookInput::stop_reason() -> Option<&str>`. `HookInput::is_grok_envelope() -> bool`, which Task 4 uses. `events::join` and `events::session_start_quiet`.

- [ ] **Step 1: Fixtures**

These have Grok's envelope shape (research §2, Hooks: camelCase keys plus snake_case aliases for some; no alias for `stopHookActive`):

`grok-session-start.json`:
```json
{"hookEventName":"SessionStart","hook_event_name":"SessionStart","sessionId":"019a-grok-1","session_id":"019a-grok-1","cwd":"/tmp/project","workspaceRoot":"/tmp/project","timestamp":"2026-10-01T10:00:00Z","permissionMode":"ask","source":"startup"}
```

`grok-stop.json`:
```json
{"hookEventName":"Stop","hook_event_name":"Stop","sessionId":"019a-grok-1","session_id":"019a-grok-1","cwd":"/tmp/project","workspaceRoot":"/tmp/project","timestamp":"2026-10-01T10:01:00Z","promptId":"p1","reason":"end_turn","stopHookActive":false,"transcriptPath":"/tmp/g/updates.jsonl","transcript_path":"/tmp/g/updates.jsonl"}
```

`grok-stop-active.json`: the same as `grok-stop.json` with `"stopHookActive":true`.

`grok-stop-shutdown.json`: the same as `grok-stop.json` with `"reason":"channel_closed"`.

`grok-session-end.json`:
```json
{"hookEventName":"SessionEnd","hook_event_name":"SessionEnd","sessionId":"019a-grok-1","session_id":"019a-grok-1","cwd":"/tmp/project","workspaceRoot":"/tmp/project","timestamp":"2026-10-01T10:02:00Z"}
```

- [ ] **Step 2: Write the failing input tests**

In `crates/clax-hooks/src/input.rs` `mod tests`:

```rust
#[test]
fn grok_camel_case_keys_fill_the_known_fields() {
    let i = HookInput::parse(
        r#"{"hookEventName":"Stop","sessionId":"g1","cwd":"/w","stopHookActive":true,"transcriptPath":"/t","reason":"end_turn"}"#,
    );
    assert_eq!(i.session_id.as_deref(), Some("g1"));
    assert_eq!(i.stop_hook_active, Some(true));
    assert_eq!(i.transcript_path.as_deref(), Some("/t"));
    assert_eq!(i.hook_event_name.as_deref(), Some("Stop"));
    assert_eq!(i.stop_reason(), Some("end_turn"));
    assert!(i.is_grok_envelope());
}

#[test]
fn both_spellings_together_parse_and_snake_case_wins() {
    let i = HookInput::parse(r#"{"session_id":"s","sessionId":"g","stop_hook_active":false,"stopHookActive":true}"#);
    assert_eq!(i.session_id.as_deref(), Some("s"));
    assert_eq!(i.stop_hook_active, Some(false));
}

#[test]
fn claude_and_codex_input_is_not_a_grok_envelope() {
    let i = HookInput::parse(r#"{"session_id":"s","hook_event_name":"Stop","stop_hook_active":true}"#);
    assert!(!i.is_grok_envelope());
    assert_eq!(i.stop_reason(), None);
    assert_eq!(i.stop_hook_active, Some(true));
}
```

Run: `cargo test -p clax-hooks input`. Expected: FAIL (no methods; `session_id` is `None` for camelCase).

- [ ] **Step 3: Implement the normalisation**

```rust
impl HookInput {
    /// Parses leniently: anything that is not a JSON object of the expected
    /// shape yields the default (all fields absent). Grok Build's camelCase
    /// keys (`sessionId`, `stopHookActive`, `transcriptPath`,
    /// `hookEventName`) fill the fields whose snake_case key is absent; they
    /// are copied after deserialising, not declared as serde aliases,
    /// because Grok sends both spellings of some keys and an alias would
    /// make that a duplicate-field error. The camelCase keys stay in `rest`.
    pub fn parse(stdin: &str) -> HookInput {
        let mut i: HookInput = serde_json::from_str(stdin).unwrap_or_default();
        let s = |i: &HookInput, k: &str| i.rest.get(k).and_then(Value::as_str).map(str::to_string);
        if i.session_id.is_none() {
            i.session_id = s(&i, "sessionId");
        }
        if i.transcript_path.is_none() {
            i.transcript_path = s(&i, "transcriptPath");
        }
        if i.hook_event_name.is_none() {
            i.hook_event_name = s(&i, "hookEventName");
        }
        if i.stop_hook_active.is_none() {
            i.stop_hook_active = i.rest.get("stopHookActive").and_then(Value::as_bool);
        }
        i
    }

    /// Why Grok Build's Stop fired: `end_turn` at the end of a turn,
    /// `channel_closed` or `shutdown` at session end. `None` when the input
    /// has no `reason` string.
    pub fn stop_reason(&self) -> Option<&str> {
        self.rest.get("reason").and_then(Value::as_str)
    }

    /// Whether this is Grok Build's envelope, which always carries the
    /// camelCase `hookEventName`; Claude Code and Codex send only
    /// `hook_event_name`.
    pub fn is_grok_envelope(&self) -> bool {
        self.rest.contains_key("hookEventName")
    }
}
```

Run: `cargo test -p clax-hooks input`. Expected: PASS.

- [ ] **Step 4: Write the failing event tests**

In `crates/clax-hooks/src/events.rs` `mod tests` (the `Fake` daemon lists sessions `a`/claude/`s1`, `b`/codex/`s1`, `c`/claude/`s2`; extend its listing with `{"id": "g", "harness": "grok", "harness_session_id": "s1"}`):

```rust
#[test]
fn a_quiet_start_joins_and_prints_nothing() {
    let d = Fake::default();
    let out = session_start_quiet("grok", 42, &[7], &input("s1"), &d).unwrap();
    assert_eq!(out, HookOutput::none());
    let calls = d.calls.borrow();
    assert_eq!(calls.len(), 1, "no prompt_hook request: {calls:?}");
    assert_eq!(calls[0].1, "/api/sessions/join");
    assert_eq!(calls[0].2["harness"], "grok");
}
```

For the Stop filter, use `fake(text)`, the `FeedbackFake` the existing Stop tests use. It returns `text` for any feedback request and records each path in `seen`. Add `{"id": "G", "harness": "grok", "harness_session_id": "s1"}` to its `/api/sessions?` listing. The existing tests are unaffected, because they look up `claude` and `codex` rows:

```rust
#[test]
fn a_grok_stop_acts_only_at_the_end_of_a_turn() {
    let d = fake(Some("[clax] hi"));
    let grok = |reason: &str| {
        HookInput::parse(&format!(r#"{{"sessionId":"s1","reason":"{reason}"}}"#))
    };
    assert_eq!(stop("grok", &grok("channel_closed"), &d).unwrap(), HookOutput::none());
    assert_eq!(stop("grok", &grok("shutdown"), &d).unwrap(), HookOutput::none());
    assert!(d.seen.borrow().is_empty(), "no daemon request at session end");
    assert_eq!(stop("grok", &grok("end_turn"), &d).unwrap(), HookOutput::block("[clax] hi"));
    let no_reason = HookInput::parse(r#"{"sessionId":"s1"}"#);
    assert_eq!(stop("grok", &no_reason, &d).unwrap(), HookOutput::block("[clax] hi"));
}

#[test]
fn a_reason_does_not_filter_other_harnesses() {
    let d = fake(Some("[clax] hi"));
    let i = HookInput::parse(r#"{"session_id":"s1","reason":"anything"}"#);
    assert_eq!(stop("claude", &i, &d).unwrap(), HookOutput::block("[clax] hi"));
}

#[test]
fn grok_stop_hook_active_turns_resends_off() {
    let d = fake(Some("[clax] hi"));
    let i = HookInput::parse(r#"{"sessionId":"s1","reason":"end_turn","stopHookActive":true}"#);
    stop("grok", &i, &d).unwrap();
    assert!(d.seen.borrow().iter().any(|p| p == "/api/sessions/G/feedback?tier=stop_hook&resends=false"), "{:?}", d.seen.borrow());
}
```

Run: `cargo test -p clax-hooks events`. Expected: FAIL.

- [ ] **Step 5: Implement the events**

In `events.rs`, split the join out of `session_start`:

```rust
/// Joins the harness's session ID to the session registered for the same
/// harness process (`parent_pid` is the hook's parent; `ancestor_pids`,
/// nearest first, cover a wrapper shell between the hook and the harness),
/// filling `cwd` and recording `codex_home` when given. Returns the
/// daemon's answer, `{"session": …}`.
///
/// # Errors
/// When the input has no session ID or the daemon request fails.
pub fn join(
    harness: &str,
    parent_pid: u32,
    ancestor_pids: &[u32],
    input: &HookInput,
    codex_home: Option<&str>,
    daemon: &dyn Daemon,
) -> anyhow::Result<Value> {
    let Some(session_id) = input.session_id.as_deref().filter(|s| !s.is_empty()) else {
        bail!("hook input has no session_id");
    };
    let mut body = json!({
        "harness": harness,
        "parent_pid": parent_pid,
        "harness_session_id": session_id,
    });
    if !ancestor_pids.is_empty() {
        body["ancestor_pids"] = json!(ancestor_pids);
    }
    if let Some(cwd) = &input.cwd {
        body["cwd"] = json!(cwd);
    }
    if let Some(h) = codex_home {
        body["codex_home"] = json!(h);
    }
    daemon.post("/api/sessions/join", &body)
}
```

`session_start` keeps its doc comment and signature, and starts with `let joined = join(harness, parent_pid, ancestor_pids, input, codex_home, daemon)?;`. The rest is unchanged. Add:

```rust
/// [`session_start`] for a harness that ignores `SessionStart` output
/// (Grok Build): joins and prints nothing. It does not ask for
/// `prompt_hook` feedback: that request marks comments delivered, and
/// they would never reach the agent.
pub fn session_start_quiet(
    harness: &str,
    parent_pid: u32,
    ancestor_pids: &[u32],
    input: &HookInput,
    daemon: &dyn Daemon,
) -> anyhow::Result<HookOutput> {
    join(harness, parent_pid, ancestor_pids, input, None, daemon)?;
    Ok(HookOutput::none())
}
```

At the top of `stop`, before `live_session`:

```rust
    // Grok Build fires Stop again at session end (`channel_closed`,
    // `shutdown`) and ignores that run's output; only `end_turn` (or an
    // input without a reason) is the end of a turn.
    if harness == "grok" && input.stop_reason().is_some_and(|r| r != "end_turn") {
        return Ok(HookOutput::none());
    }
```

Add to `stop`'s doc comment: `For Grok Build, a Stop whose reason is not end_turn allows the stop without asking the daemon.`

Run: `cargo test -p clax-hooks`. Expected: PASS.

- [ ] **Step 6: The command**

In `crates/clax-cli/src/commands/hook.rs`:

```rust
/// Grok Build gives `SessionEnd` hooks 1.5 s by default, so `session-end
/// --agent grok` gives up after this long.
const GROK_END_DEADLINE: Duration = Duration::from_millis(1200);
/// Each Grok `session-end` daemon request is abandoned after this long.
const GROK_END_REQUEST_TIMEOUT: Duration = Duration::from_millis(1000);

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum Agent {
    Claude,
    Codex,
    Grok,
}
```

`Agent::harness` gains `Agent::Grok => "grok"`. Replace `impl Event { fn budget(self) … }` with a function over both:

```rust
/// The deadline for the whole invocation and for each daemon request.
fn budget(agent: Agent, event: Event) -> (Duration, Duration) {
    match (agent, event) {
        (Agent::Grok, Event::SessionEnd) => (GROK_END_DEADLINE, GROK_END_REQUEST_TIMEOUT),
        (_, Event::SessionStart) => (START_DEADLINE, START_REQUEST_TIMEOUT),
        (_, Event::SessionEnd) => (END_DEADLINE, END_REQUEST_TIMEOUT),
        (_, Event::Stop) => (STOP_DEADLINE, FEEDBACK_REQUEST_TIMEOUT),
        (_, Event::Prompt) => (PROMPT_DEADLINE, FEEDBACK_REQUEST_TIMEOUT),
    }
}
```

Update both callers (`run` and `handle`) to `budget(agent, event)`. In `handle`, before `Client::discover`:

```rust
    // Grok Build discards an allowing prompt hook's output, and a
    // prompt_hook request marks comments delivered, so Grok's prompt hook
    // does nothing (the plugin does not wire it).
    if matches!((agent, event), (Agent::Grok, Event::Prompt)) {
        return Ok(HookOutput::none());
    }
```

and in the `match event`:

```rust
        Event::SessionStart if matches!(agent, Agent::Grok) => events::session_start_quiet(
            agent.harness(),
            parent_pid,
            &ancestors(parent_pid),
            &input,
            &client,
        ),
```

before the existing `Event::SessionStart` arm.

- [ ] **Step 7: Golden cases**

In `crates/clax-hooks/tests/golden.rs`, following the file's existing Claude and Codex cases (a `Daemon::start()`, a registered session, a thread sent to the agent on a watched artifact; reuse its helpers for those), add:

- `grok_session_start_joins_by_session_id_and_prints_nothing`: register `{"harness": "grok", "harness_session_id": "019a-grok-1", "cwd": "", "pid": <a live pid>, "parent_pid": 1}`. Send a comment that is waiting for that session. Run `hook --agent grok session-start` with `grok-session-start.json`. Assert: stdout is empty, exit 0, the row's `cwd` is `/tmp/project`, and the comment's feedback is still undelivered (`GET /api/sessions/<sid>/feedback?tier=wait&timeout_s=0`, or the helper the file uses, still returns it).
- `grok_stop_blocks_once_at_the_end_of_a_turn`: with an armed watch and one sent comment, `grok-stop.json` prints `{"decision":"block","reason":"…"}` containing the comment's text. `grok-stop-active.json` then prints nothing.
- `grok_stop_at_session_end_does_nothing`: with a sent comment, `grok-stop-shutdown.json` prints nothing and leaves the comment undelivered.
- `grok_session_end_ends_the_row_within_its_budget`: `grok-session-end.json` ends the row. `elapsed` is under 1.5 s. With no daemon (a fresh home), it also exits 0 within 1.5 s.

Run `hook_env` with the harness environment cleared, as Global Constraints require. Run: `cargo test -p clax-hooks --test golden grok`. Expected: PASS. Then run all of `cargo test -p clax-hooks`.

- [ ] **Step 8: Gates and stage**

Run the quality gates. Stage the Task 3 files.

Proposed commit message: `Add clax hook --agent grok: read Grok's camelCase input, join quietly at session start, and hand comments over only at the end of a turn`

---

### Task 4: The binary stands the Claude Code copy down in Grok

**Files:**
- Create: `crates/clax-mcp/src/standdown.rs`
- Modify: `crates/clax-mcp/src/lib.rs` (`pub mod standdown;`)
- Create: `crates/clax-cli/src/host.rs`
- Modify: `crates/clax-cli/src/main.rs` (`mod host;`)
- Modify: `crates/clax-cli/src/commands/mcp.rs`, `crates/clax-cli/src/commands/hook.rs`
- Modify: `crates/clax-cli/src/hooklog.rs` (`tail_for` skips `standdown` lines)
- Create: `crates/clax-cli/tests/standdown.rs`

**Interfaces:**
- Consumes: `HookInput::is_grok_envelope` (Task 3).
- Produces: `clax_mcp::standdown::{GROK_STANDDOWN, StandDown, serve}`; `crate::host::{grok_runs_mcp, grok_runs_hook, log_standdown}` in clax-cli. `clax mcp --agent claude` and `clax hook --agent claude` stand down when Grok runs them. Task 5 copies `GROK_STANDDOWN` into the wrapper word for word, and Task 7's plugin check compares the two. Task 8 reads the `standdown` lines.

- [ ] **Step 1: Write the failing unit tests**

Create `crates/clax-cli/src/host.rs` with only the tests first, and declare `mod host;` in `main.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn env(vars: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |k| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    }

    #[test]
    fn grok_runs_an_mcp_server_it_gave_a_session_id_and_did_not_get_from_claude() {
        assert!(grok_runs_mcp(env(&[("GROK_SESSION_ID", "g")]), 500));
        assert!(
            grok_runs_mcp(env(&[("GROK_SESSION_ID", "g"), ("CLAUDE_PID", "77")]), 500),
            "Grok started from a Claude Code shell: CLAUDE_PID is inherited"
        );
        assert!(
            !grok_runs_mcp(env(&[("GROK_SESSION_ID", "g"), ("CLAUDE_PID", "500")]), 500),
            "Claude Code started from a Grok shell: Claude Code is the parent"
        );
        assert!(!grok_runs_mcp(env(&[("CLAUDE_PID", "500")]), 500));
        assert!(!grok_runs_mcp(env(&[("GROK_SESSION_ID", "")]), 500));
        assert!(!grok_runs_mcp(env(&[]), 500));
    }

    #[test]
    fn grok_runs_a_hook_with_its_event_variable_or_its_envelope() {
        let claude = HookInput::parse(r#"{"session_id":"s","hook_event_name":"Stop"}"#);
        let grok = HookInput::parse(r#"{"session_id":"s","sessionId":"s","hookEventName":"Stop"}"#);
        assert!(grok_runs_hook(env(&[("GROK_HOOK_EVENT", "Stop")]), &claude));
        assert!(grok_runs_hook(env(&[]), &grok));
        assert!(!grok_runs_hook(env(&[]), &claude));
        assert!(
            !grok_runs_hook(env(&[("GROK_SESSION_ID", "g")]), &claude),
            "a Claude Code hook in a session started from a Grok shell acts"
        );
    }
}
```

Run: `cargo test -p clax-cli host`. Expected: FAIL (the functions do not exist).

- [ ] **Step 2: Implement `host.rs`**

Above the tests:

```rust
//! Whether Grok Build started this run of the Claude Code plugin's copy.
//! Grok loads Claude Code plugins too; in a Grok session only `--agent
//! grok` acts (spec D17), so a Claude Code copy that Grok runs stands down.

use clax_core::Home;
use clax_hooks::input::HookInput;

fn var(env: &impl Fn(&str) -> Option<String>, k: &str) -> Option<String> {
    env(k).filter(|v| !v.is_empty())
}

/// True when Grok started this MCP server: `GROK_SESSION_ID` is set and
/// `CLAUDE_PID` is not `parent_pid`. Claude Code sets `CLAUDE_PID` to its
/// own PID and is the parent of the servers it starts, so a Claude Code
/// session started from a Grok shell, which inherits `GROK_SESSION_ID`,
/// still acts; a Grok started from a Claude Code shell inherits a
/// `CLAUDE_PID` that is not its server's parent.
pub fn grok_runs_mcp(env: impl Fn(&str) -> Option<String>, parent_pid: u32) -> bool {
    var(&env, "GROK_SESSION_ID").is_some()
        && var(&env, "CLAUDE_PID").and_then(|p| p.trim().parse::<u32>().ok()) != Some(parent_pid)
}

/// True when Grok runs this hook: `GROK_HOOK_EVENT` is set (Grok sets it
/// for every hook it runs, and its shell tool does not pass it on), or the
/// input is Grok's envelope (`hookEventName`).
pub fn grok_runs_hook(env: impl Fn(&str) -> Option<String>, input: &HookInput) -> bool {
    var(&env, "GROK_HOOK_EVENT").is_some() || input.is_grok_envelope()
}

/// Appends `<time> standdown mode=<mode> agent=claude host=grok` to
/// hooks.log, the line the wrapper writes for the same event.
pub fn log_standdown(home: &Home, mode: &str) {
    let at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    crate::hooklog::append(home, &format!("{at} standdown mode={mode} agent=claude host=grok"));
}
```

Run: `cargo test -p clax-cli host`. Expected: PASS.

- [ ] **Step 3: The stand-down server**

Create `crates/clax-mcp/src/standdown.rs`:

```rust
//! The MCP server a Clax plugin copy serves when another copy acts for the
//! harness session: it completes the handshake and offers one tool,
//! `status`, whose result says which copy acts. It needs no daemon.

use crate::tools::StatusArgs;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig,
};
use rmcp::transport::stdio;
use rmcp::{ErrorData as McpError, ServerHandler, ServiceExt, tool, tool_handler, tool_router};

/// What the Claude Code plugin's copy says when Grok Build runs it. The
/// wrapper (`scripts/ensure-clax.sh`, `GROK_STANDDOWN`) carries the same
/// text; `scripts/test-plugins.sh` checks that they match.
pub const GROK_STANDDOWN: &str = "This is the Clax plugin for Claude Code, which Grok Build also loads. In Grok, Clax runs from the clax-grok plugin, whose tools are named `clax_grok__<tool>` (for example `clax_grok__publish`); this server does nothing. If no `clax_grok` tools are listed, run `clax init --agent grok`. To remove this server from Grok, run `grok plugin disable clax`.";

/// The description of the stand-down server's `status` tool.
pub const STATUS_DESCRIPTION: &str =
    "Says which Clax plugin serves this Grok session; this server does nothing else.";

/// A server whose one tool, `status`, returns `text`.
#[derive(Clone)]
pub struct StandDown {
    text: &'static str,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl StandDown {
    pub fn new(text: &'static str) -> StandDown {
        StandDown {
            text,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Says which Clax plugin serves this Grok session; this server does nothing else."
    )]
    pub async fn status(
        &self,
        Parameters(_args): Parameters<StatusArgs>,
    ) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![ContentBlock::text(self.text)]))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for StandDown {
    fn get_info(&self) -> ServerConfig {
        let mut config = ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(self.text);
        config.server_info = Implementation::new("clax", env!("CARGO_PKG_VERSION"));
        config
    }
}

/// Serves [`StandDown`] with `text` on stdin/stdout until the client
/// closes the connection.
pub async fn serve(text: &'static str) -> anyhow::Result<()> {
    let service = StandDown::new(text).serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_offers_exactly_one_tool_named_status() {
        let names: Vec<String> = StandDown::new(GROK_STANDDOWN)
            .tool_router
            .list_all()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect();
        assert_eq!(names, ["status"]);
        assert!(GROK_STANDDOWN.contains("clax_grok__publish"));
        assert!(GROK_STANDDOWN.contains("clax init --agent grok"));
    }
}
```

The `#[tool]` description literal and `STATUS_DESCRIPTION` must be the same string (rmcp's attribute takes a literal). Add a test that compares them through `list_all()`'s `description`. Mirror `ClaxTools`'s `#[tool_handler]` usage in `tools.rs` if rmcp 3.5's signatures differ from the above (for example, if `with_instructions` takes a `String`, pass `self.text.to_string()`). Add `pub mod standdown;` to `crates/clax-mcp/src/lib.rs`.

Run: `cargo test -p clax-mcp standdown`. Expected: PASS.

- [ ] **Step 4: Write the failing integration tests**

Create `crates/clax-cli/tests/standdown.rs`. It drives `clax` over stdin and stdout one request at a time, in scratch homes, with the harness environment cleared:

```rust
//! The Claude Code plugin's copy stands down when Grok Build runs it:
//! `clax mcp --agent claude` serves the stand-down server and `clax hook
//! --agent claude` does nothing; everywhere else they act.

use assert_cmd::cargo::cargo_bin;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

/// Variables a test inherits from the harness it runs in; cleared first.
const HARNESS_VARS: &[&str] = &[
    "GROK_SESSION_ID", "GROK_HOOK_EVENT", "GROK_PLUGIN_ROOT", "GROK_HOME",
    "CLAUDE_PID", "CLAUDE_CODE_SESSION_ID", "CLAUDE_PLUGIN_ROOT", "CLAUDE_PROJECT_DIR",
    "CLAX_SESSION_ID", "CLAX_BIN",
];

fn clax(home: &std::path::Path, env: &[(&str, String)]) -> Command {
    let mut c = Command::new(cargo_bin("clax"));
    for k in HARNESS_VARS {
        c.env_remove(k);
    }
    c.env("CLAX_HOME", home)
        .env("HOME", home.parent().unwrap())
        .env("CLAX_CODEX_BIN", "")
        .env("CLAX_NO_OPEN", "1")
        .env("RUST_LOG", "error");
    for (k, v) in env {
        c.env(k, v);
    }
    c
}

/// An MCP server under test, answering one request at a time.
struct Mcp {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Mcp {
    fn start(home: &std::path::Path, agent: &str, env: &[(&str, String)]) -> Mcp {
        let mut child = clax(home, env)
            .args(["--port", "0", "mcp", "--agent", agent])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut m = Mcp { child, stdin, stdout };
        let init = m.request(0, "initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "fake-grok", "version": "1"}}));
        assert!(init["result"].is_object(), "{init}");
        m.notify("notifications/initialized");
        m
    }
    fn notify(&mut self, method: &str) {
        writeln!(self.stdin, "{}", json!({"jsonrpc": "2.0", "method": method})).unwrap();
    }
    fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        writeln!(self.stdin, "{}", json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})).unwrap();
        loop {
            let mut line = String::new();
            assert!(self.stdout.read_line(&mut line).unwrap() > 0, "server closed stdout");
            let v: Value = serde_json::from_str(&line).unwrap();
            if v["id"] == json!(id) {
                return v;
            }
        }
    }
    fn tool_names(&mut self) -> Vec<String> {
        let r = self.request(1, "tools/list", json!({}));
        r["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect()
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn hooks_log(home: &std::path::Path) -> String {
    std::fs::read_to_string(home.join("logs/hooks.log")).unwrap_or_default()
}

#[test]
fn a_claude_copy_that_grok_starts_serves_only_status() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("ax");
    let mut m = Mcp::start(&home, "claude", &[("GROK_SESSION_ID", "019a-g".into())]);
    assert_eq!(m.tool_names(), ["status"]);
    let call = m.request(2, "tools/call", json!({"name": "status", "arguments": {}}));
    assert_ne!(call["result"]["isError"], json!(true), "{call}");
    let text = call["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("clax_grok__publish"), "{text}");
    assert_eq!(m.request(3, "ping", json!({}))["result"], json!({}));
    assert!(!home.join("daemon.json").exists(), "a standing-down server starts no daemon");
    assert!(hooks_log(&home).contains(" standdown mode=mcp agent=claude host=grok"));
}

#[test]
fn an_inherited_claude_pid_that_is_not_the_parent_still_stands_down() {
    let dir = tempfile::tempdir().unwrap();
    let mut m = Mcp::start(&dir.path().join("ax"), "claude", &[
        ("GROK_SESSION_ID", "019a-g".into()),
        ("CLAUDE_PID", "1".into()),
    ]);
    assert_eq!(m.tool_names(), ["status"]);
}

#[test]
fn claude_code_as_the_parent_acts_even_with_a_grok_session_id() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("ax");
    // The test process is the server's parent, standing in for Claude Code.
    let mut m = Mcp::start(&home, "claude", &[
        ("GROK_SESSION_ID", "019a-g".into()),
        ("CLAUDE_PID", std::process::id().to_string()),
    ]);
    assert_eq!(m.tool_names().len(), 22);
    drop(m);
    let _ = clax(&home, &[]).arg("stop").status();
}

#[test]
fn the_grok_agent_acts() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("ax");
    let mut m = Mcp::start(&home, "grok", &[("GROK_SESSION_ID", "019a-g".into())]);
    assert_eq!(m.tool_names().len(), 22);
    drop(m);
    let _ = clax(&home, &[]).arg("stop").status();
}

/// Runs `clax hook --agent claude stop` with `stdin`; (stdout, stderr).
fn claude_stop(home: &std::path::Path, env: &[(&str, String)], stdin: &str) -> (String, String) {
    let mut child = clax(home, env)
        .args(["hook", "--agent", "claude", "stop"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    (String::from_utf8(out.stdout).unwrap(), String::from_utf8(out.stderr).unwrap())
}

#[test]
fn a_claude_hook_that_grok_runs_does_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("ax");
    // No daemon: a hook that acted would report "no clax daemon is running".
    let (out, err) = claude_stop(&home, &[("GROK_HOOK_EVENT", "Stop".into())], r#"{"session_id":"s1"}"#);
    assert_eq!((out.as_str(), err.as_str()), ("", ""));
    let (out, err) = claude_stop(&home, &[], r#"{"sessionId":"s1","session_id":"s1","hookEventName":"Stop","reason":"end_turn"}"#);
    assert_eq!((out.as_str(), err.as_str()), ("", ""));
    let log = hooks_log(&home);
    assert_eq!(log.matches(" standdown mode=hook agent=claude host=grok").count(), 2, "{log}");
    assert!(!log.contains(" hook agent=claude "), "{log}");
}

#[test]
fn a_claude_hook_outside_grok_acts() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("ax");
    let (_, err) = claude_stop(&home, &[("GROK_SESSION_ID", "g".into())], r#"{"session_id":"s1"}"#);
    assert!(err.contains("no clax daemon is running"), "{err}");
    assert!(hooks_log(&home).contains(" hook agent=claude event=stop "));
}
```

Run: `cargo test -p clax-cli --test standdown`. Expected: the stand-down cases FAIL; the acting cases PASS.

- [ ] **Step 5: Wire the guard**

`crates/clax-cli/src/commands/mcp.rs`, in `run`, after the `preflight` early return and before `tracing_subscriber`:

```rust
    // SAFETY: getppid has no preconditions and cannot fail.
    let parent_pid = unsafe { libc::getppid() } as u32;
    if matches!(a.agent, Agent::Claude)
        && crate::host::grok_runs_mcp(|k| std::env::var(k).ok(), parent_pid)
    {
        // Grok Build runs the Claude Code copy too; in a Grok session only
        // clax-grok's server acts (spec D17).
        crate::host::log_standdown(home, "mcp");
        let rt = tokio::runtime::Runtime::new()?;
        let result = rt.block_on(clax_mcp::standdown::serve(clax_mcp::standdown::GROK_STANDDOWN));
        rt.shutdown_timeout(Duration::from_millis(100));
        return result;
    }
```

`crates/clax-cli/src/commands/hook.rs`: `handle` returns `anyhow::Result<Option<HookOutput>>`, where `None` means it stood down. After `let input = HookInput::parse(&stdin);`:

```rust
    if matches!(agent, Agent::Claude) && crate::host::grok_runs_hook(|k| std::env::var(k).ok(), &input) {
        // Grok Build runs the Claude Code copy's hooks too; in a Grok
        // session only clax-grok's hooks act (spec D17).
        crate::host::log_standdown(home, "hook");
        return Ok(None);
    }
```

Wrap the other return values in `Some(...)`. Grok's `prompt` early return from Task 3 becomes `return Ok(Some(HookOutput::none()));`. In `run`:

```rust
    let error = match rx.recv_timeout(deadline) {
        // Stood down: logged by `handle`, nothing to print or log here.
        Ok(Ok(None)) => std::process::exit(0),
        Ok(Ok(Some(out))) => {
            if let Some(line) = out.to_line() {
                let _ = writeln!(std::io::stdout(), "{line}");
            }
            None
        }
        Ok(Err(e)) => Some(format!("{e:#}")),
        Err(_) => Some(format!("timed out after {deadline:?}")),
    };
```

Update `run`'s doc comment: `A Claude Code hook that Grok Build runs stands down: it prints nothing and logs one standdown line.`

`crates/clax-cli/src/hooklog.rs` `tail_for`: keep a line only when it contains the `agent=` needle **and** does not contain `" standdown "`. Add the clause `` Stand-down lines (`standdown … host=grok`) are left out: they are not that harness's hooks. `` to its doc comment, and add a unit test that appends one `standdown` line and one hook line for `claude` and gets only the hook line back.

Run: `cargo test -p clax-cli --test standdown` and `cargo test -p clax-cli`. Expected: PASS.

- [ ] **Step 6: Gates and stage**

Run the quality gates. Stage the Task 4 files.

Proposed commit message: `Stand the Claude Code plugin's copy down when Grok Build runs it: a status-only MCP server and silent hooks`

---

### Task 5: The wrapper stands the Claude Code copy down in Grok

**Files:**
- Modify: `scripts/ensure-clax.sh`, then copy it over `plugins/claude-code/scripts/ensure-clax.sh` and `plugins/clax/scripts/ensure-clax.sh`
- Modify: `scripts/test-ensure-clax.sh`

**Interfaces:**
- Consumes: `GROK_STANDDOWN` and `STATUS_DESCRIPTION` (Task 4), copied word for word.
- Produces: the wrapper's first-layer guard, which needs no `clax` binary. Task 7 copies the wrapper into `plugins/clax-grok/scripts/`.

- [ ] **Step 1: Write the failing tests**

In `scripts/test-ensure-clax.sh`, make `new_env` also clear the harness environment:

```bash
    unset GROK_SESSION_ID GROK_HOOK_EVENT GROK_PLUGIN_ROOT GROK_HOME CLAUDE_PID CLAUDE_CODE_SESSION_ID CLAUDE_PLUGIN_ROOT CLAUDE_PROJECT_DIR CLAX_SESSION_ID
```

Then add a section after the hook-mode cases:

```bash
# --- Grok guard: the Claude Code copy stands down in a Grok session -----------

# A fake clax that records each run, so a case can tell that none happened.
recording_clax() {
    mkdir -p "$1"
    printf '#!/bin/sh\nif [ "$1" = "--version" ]; then echo "clax %s"; exit 0; fi\necho "$*" >> "%s/ran"\necho "args: $*"\n' "$V" "$SANDBOX" > "$1/clax"
    chmod +x "$1/clax"
}
# Runs the wrapper as a child of a shell whose PID is exported as CLAUDE_PID,
# as Claude Code does for the MCP servers it starts.
run_under_claude() {
    OUT="$("$TOOLS/bash" -c 'export CLAUDE_PID=$$; "$1" "$2" "${@:3}"; exit $?' _ "$TOOLS/bash" "$SCRIPT" "$@" 2>"$SANDBOX/stderr" < /dev/null)"
    RC=$?; ERR="$(cat "$SANDBOX/stderr")"
}
standdown_text() {
    "$PY" - "$OUT" <<'PYEOF'
import json, sys
lines = [json.loads(l) for l in sys.argv[1].splitlines()]
assert [l["id"] for l in lines] == [0, 1, "call-2", 3, 4], lines
init = lines[0]["result"]
assert init["serverInfo"]["name"] == "clax" and "clax-grok" in init["instructions"], init
tools = lines[1]["result"]["tools"]
assert [t["name"] for t in tools] == ["status"], tools
call = lines[2]["result"]
assert call["isError"] is False, call
assert lines[3]["error"]["code"] == -32601 and lines[4]["result"] == {}, lines
print(call["content"][0]["text"])
PYEOF
}
mcp_as() { local agent="$1"; shift; OUT="$(printf '%s\n' "$REQS" | env "$@" "$TOOLS/bash" "$SCRIPT" exec mcp --agent "$agent" 2>"$SANDBOX/stderr")"; RC=$?; ERR="$(cat "$SANDBOX/stderr")"; }

new_env
recording_clax "$FAKEBIN"
mcp_as claude GROK_SESSION_ID=019a-g
if [ "$RC" = 0 ] && text="$(standdown_text)" && echo "$text" | grep -qF 'clax_grok__publish' \
    && [ ! -e "$SANDBOX/ran" ] && hooks_log | grep -q ' standdown mode=mcp agent=claude host=grok$'; then
    pass "grok guard: the Claude copy's MCP server under Grok serves status only and runs no clax"
else fail "grok guard: Claude copy MCP under Grok (rc=$RC out=$OUT err=$ERR ran=$(cat "$SANDBOX/ran" 2>/dev/null))"; fi

new_env
mcp_as claude GROK_SESSION_ID=019a-g
if [ "$RC" = 0 ] && standdown_text >/dev/null && ! hooks_log | grep -q launcher; then
    pass "grok guard: standing down needs no clax binary"
else fail "grok guard: standing down without clax (out=$OUT log=$(hooks_log))"; fi

new_env
recording_clax "$FAKEBIN"
mcp_as claude GROK_SESSION_ID=019a-g CLAUDE_PID=1
if standdown_text >/dev/null && [ ! -e "$SANDBOX/ran" ]; then
    pass "grok guard: Grok started from a Claude Code shell (inherited CLAUDE_PID) stands the copy down"
else fail "grok guard: inherited CLAUDE_PID (out=$OUT)"; fi

new_env
recording_clax "$FAKEBIN"
GROK_SESSION_ID=019a-g run_under_claude exec mcp --agent claude
if [ "$RC" = 0 ] && grep -q -- '--agent claude' "$SANDBOX/ran" 2>/dev/null; then
    pass "grok guard: Claude Code as the parent (CLAUDE_PID) runs clax even with GROK_SESSION_ID"
else fail "grok guard: CLAUDE_PID parent (rc=$RC out=$OUT err=$ERR)"; fi

for agent in grok codex; do
    new_env
    recording_clax "$FAKEBIN"
    mcp_as "$agent" GROK_SESSION_ID=019a-g
    if grep -q -- "mcp --agent $agent" "$SANDBOX/ran" 2>/dev/null && ! hooks_log | grep -q standdown; then
        pass "grok guard: --agent $agent is never stood down"
    else fail "grok guard: --agent $agent (out=$OUT log=$(hooks_log))"; fi
done

new_env
recording_clax "$FAKEBIN"
OUT="$(printf '{"sessionId":"g","hookEventName":"Stop"}' | GROK_HOOK_EVENT=Stop "$TOOLS/bash" "$SCRIPT" exec hook --agent claude stop 2>"$SANDBOX/stderr")"; RC=$?; ERR="$(cat "$SANDBOX/stderr")"
if [ "$RC" = 0 ] && [ -z "$OUT" ] && [ -z "$ERR" ] && [ ! -e "$SANDBOX/ran" ] \
    && hooks_log | grep -q ' standdown mode=hook agent=claude host=grok$'; then
    pass "grok guard: a Claude copy hook that Grok runs reads its input, prints nothing and exits 0"
else fail "grok guard: Claude copy hook under Grok (rc=$RC out=$OUT err=$ERR)"; fi

new_env
recording_clax "$FAKEBIN"
OUT="$(printf '{"session_id":"s"}' | GROK_SESSION_ID=g "$TOOLS/bash" "$SCRIPT" exec hook --agent claude stop 2>"$SANDBOX/stderr")"; RC=$?
if [ "$RC" = 0 ] && grep -q -- 'hook --agent claude stop' "$SANDBOX/ran" 2>/dev/null; then
    pass "grok guard: a Claude Code hook with only GROK_SESSION_ID (Claude Code started from a Grok shell) acts"
else fail "grok guard: hook without GROK_HOOK_EVENT (rc=$RC)"; fi

new_env
recording_clax "$FAKEBIN"
GROK_HOOK_EVENT=Stop GROK_SESSION_ID=g run exec status
if [ "$RC" = 0 ] && [ "$OUT" = "args: status" ]; then
    pass "grok guard: CLI mode is never stood down"
else fail "grok guard: CLI mode (rc=$RC out=$OUT)"; fi

# The wrapper and the binary say the same thing.
want="$(sed -n 's/^pub const GROK_STANDDOWN: &str = "\(.*\)";$/\1/p' "$HERE/../crates/clax-mcp/src/standdown.rs")"
new_env
mcp_as claude GROK_SESSION_ID=019a-g
if [ -n "$want" ] && [ "$(standdown_text)" = "$want" ]; then pass "grok guard: the wrapper's text is the binary's"
else fail "grok guard: the stand-down text differs from crates/clax-mcp/src/standdown.rs"; fi
```

Run: `scripts/test-ensure-clax.sh`. Expected: the "grok guard" cases FAIL; all others PASS.

- [ ] **Step 2: Implement the guard in the wrapper**

In `scripts/ensure-clax.sh`, add to the header comment, after the paragraph about MCP mode:

```bash
# In a Grok Build session, Clax acts only through --agent grok (the
# clax-grok plugin); Grok also loads this Claude Code plugin. A run with
# --agent claude that Grok started stands down before any clax is looked
# for: a hook (GROK_HOOK_EVENT set) reads its input and exits 0 silently;
# the MCP server (GROK_SESSION_ID set, and CLAUDE_PID not this script's
# parent) answers with a minimal server whose one tool, status, says so.
```

After `CLAX_VERSION`:

```bash
# What the Claude Code copy's MCP server says when Grok Build runs it; the
# same text as GROK_STANDDOWN in crates/clax-mcp/src/standdown.rs.
GROK_STANDDOWN="This is the Clax plugin for Claude Code, which Grok Build also loads. In Grok, Clax runs from the clax-grok plugin, whose tools are named \`clax_grok__<tool>\` (for example \`clax_grok__publish\`); this server does nothing. If no \`clax_grok\` tools are listed, run \`clax init --agent grok\`. To remove this server from Grok, run \`grok plugin disable clax\`."
```

A function:

```bash
# True when Grok Build started this Claude Code copy's hook or MCP server.
# Claude Code sets CLAUDE_PID to its own PID and starts MCP servers
# directly, so under Claude Code CLAUDE_PID is this script's parent, even
# when Claude Code was started from a Grok shell that passed on
# GROK_SESSION_ID. Grok sets GROK_HOOK_EVENT on every hook it runs.
grok_runs_claude_copy() {
    [ "$AGENT" = claude ] || return 1
    case "$MODE" in
        hook) [ -n "${GROK_HOOK_EVENT:-}" ] ;;
        mcp) [ -n "${GROK_SESSION_ID:-}" ] && [ "${CLAUDE_PID:-}" != "$PPID" ] ;;
        *) return 1 ;;
    esac
}
```

Generalise `serve_unavailable` into `serve_status`, keeping its parsing loop:

```bash
# A minimal MCP server on stdin/stdout whose one tool, status, is described
# by $2. Its instructions are $1. With $3 = true, a status call is an error
# that states $1, or that the cause is gone (status_text); with $3 = false
# it returns $1. Answers until stdin closes.
serve_status() {
    local text="$1" desc="$2" is_error="$3" line method id proto out
    while IFS= read -r line || [ -n "$line" ]; do
        # ... the existing parsing, unchanged ...
        case "$method" in
            initialize)
                reply "$id" "{\"protocolVersion\":$proto,\"capabilities\":{\"tools\":{}},\"serverInfo\":{\"name\":\"clax\",\"version\":\"$CLAX_VERSION\"},\"instructions\":$(json_string "$text")}"
                ;;
            tools/list)
                reply "$id" "{\"tools\":[{\"name\":\"status\",\"description\":$(json_string "$desc"),\"inputSchema\":{\"type\":\"object\",\"properties\":{}}}]}"
                ;;
            tools/call)
                if [ "$is_error" = true ]; then out="$(status_text "$text")"; else out="$text"; fi
                reply "$id" "{\"content\":[{\"type\":\"text\",\"text\":$(json_string "$out")}],\"isError\":$is_error}"
                ;;
            ping) reply "$id" "{}" ;;
            *) printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32601,"message":%s}}\n' "$id" "$(json_string "$text")" ;;
        esac
    done
}

# Clax cannot run: the reason, and the fix.
serve_unavailable() {
    serve_status "Clax is unavailable: $REASON (Details: ${CLAX_HOME:-~/.clax}/logs/hooks.log.)" \
        "Clax could not start. Call this tool for the reason and the fix." true
}

# This Claude Code copy in a Grok Build session: clax-grok acts instead.
serve_standdown() {
    serve_status "$GROK_STANDDOWN" "Says which Clax plugin serves this Grok session; this server does nothing else." false
}
```

At the start of `main`:

```bash
    if grok_runs_claude_copy; then
        hooks_log "standdown mode=$MODE agent=claude host=grok"
        case "$MODE" in
            # Read the hook's input, so Grok's write to stdin never fails.
            hook) cat > /dev/null 2>&1 || true ;;
            *) serve_standdown ;;
        esac
        exit 0
    fi
```

Copy the wrapper over both plugin copies (`cp scripts/ensure-clax.sh plugins/claude-code/scripts/ensure-clax.sh`, and the same for `plugins/clax`). Run: `scripts/test-ensure-clax.sh` and `scripts/test-plugins.sh`. Expected: every case passes, and the copies match. Run `bash -n` on the wrapper and, if it is installed, `shellcheck scripts/ensure-clax.sh`.

- [ ] **Step 3: Gates and stage**

Run the quality gates. Stage the four files.

Proposed commit message: `Stand the Claude Code plugin's copy down in the wrapper when Grok Build runs it, before any clax is looked for`

---

### Task 6: Tier 5 for Grok: `clax feedback follow` and the monitor

**Files:**
- Modify: `crates/clax-core/src/store/migrations.rs` (a new migration: `feedback.notified_at`)
- Modify: `crates/clax-core/src/store/feedback.rs` (`take_notices`; untargeting and retargeting clear `notified_at`)
- Modify: `crates/clax-core/src/feedback.rs` (`Notice`, `render_notice`)
- Modify: `crates/clax-core/src/lib.rs` (re-export `Notice` if the crate re-exports its feedback types)
- Modify: `crates/clax-server/src/feedback.rs` (`Followers`)
- Modify: `crates/clax-server/src/state.rs` (`followers`)
- Modify: `crates/clax-server/src/routes/feedback.rs` (`notices`), `crates/clax-server/src/routes/mod.rs` (the route)
- Modify: `crates/clax-server/src/routes/sessions.rs` (Grok's `push` reports a follower)
- Create: `crates/clax-server/tests/api_notices.rs`
- Create: `crates/clax-cli/src/commands/feedback.rs`
- Modify: `crates/clax-cli/src/commands/mod.rs`, `crates/clax-cli/src/main.rs` (`clax feedback follow`)
- Create: `crates/clax-cli/tests/follow.rs`

**Interfaces:**
- Consumes: Task 2 (`grok` sessions; `push_info`).
- Produces:
  - `GET /api/sessions/<sid>/notices?wait=<s>` (token): `{notices: [{feedback_id, comment_id, thread_id, artifact_id, title, url}], lines: [<string>], waited_s}`.
  - `clax feedback follow [--session <ID> | --agent <claude|codex|grok> --harness-session <ID>]`: one stdout line per notice.
  - Grok's `status.push` is `{"tier": "monitor", "available": <a follower is connected>, "reason": …}`.
  - Task 7's skill section, Task 10's end-to-end case and Task 11's smoke check use these.

**The delivery model.** A monitor line is a **notice**, not a delivery. It wakes the agent and points it at a comment. It does not carry the comment, and it does not mark the comment's feedback row delivered. The row keeps waiting for tiers 1, 2 and 4, exactly as before: the woken agent's `comments_read` call carries it in-band (tier 1), or the Stop hook at the end of the woken turn hands it over (tier 2), or a `wait_for_feedback` does (tier 4). Whichever comes first marks it delivered, once, under the existing rules. That is why the monitor cannot cause a double delivery: it never delivers. The daemon records a notice as `notified_at` on the row, which has three effects:
1. Each row is announced at most once per target session, however many followers run. The stamp is set with `UPDATE … WHERE notified_at IS NULL AND delivered_at IS NULL`, and a notice is emitted only when that update changed the row.
2. A row that any tier has already delivered is never announced.
3. When a row is retargeted to another session (its session ended), `notified_at` is cleared, so the new session's follower announces it.

A notice is emitted only for rows on watches with `replies_armed`, as for tiers 2 and 5. The skip rule that already holds for Codex and Pi also holds here: while the session is inside `wait_for_feedback`, the notices poll answers empty at once and stamps nothing, and the wait delivers. A notice can arrive after a Stop hook has already delivered the same comment (Grok queues a monitor's line while the agent is busy). The line therefore ends `if you have already handled it, do nothing`, and the woken `comments_read` shows the thread as handled. That costs one short turn and never duplicates the payload.

**Reuse by Claude Code later.** The command is harness-neutral. `--session` takes a Clax session ID, and `--agent X --harness-session ID` takes any harness's own ID. Only Grok's environment default is wired (`GROK_SESSION_ID`). A later Claude Code fallback can run `clax feedback follow --agent claude --harness-session "$CLAUDE_CODE_SESSION_ID"` as a background command, with an added `--once` flag that exits after the first line (that exit wakes Claude Code). This plan does not add `--once` or any Claude Code wiring.

- [ ] **Step 1: Write the failing store tests**

In `crates/clax-core/src/store/feedback.rs` `mod tests`:

```rust
#[test]
fn notices_announce_each_armed_undelivered_row_once() {
    let (_d, st) = store();
    let grok = session(&st, "grok", "g1");
    let aid = artifact(&st, Some(&grok));
    let tid = thread(&st, &aid, "hi");
    st.send_to_agent(&tid).unwrap();
    assert!(st.take_notices(&grok, "http://h:1").unwrap().is_empty(), "unarmed: no notice");
    st.ensure_watch(&grok, &aid).unwrap();
    let n = st.take_notices(&grok, "http://h:1").unwrap();
    assert_eq!(n.len(), 1);
    assert_eq!(n[0].thread_id, tid);
    assert_eq!(n[0].url, format!("http://h:1/a/{}", aid.as_str()));
    assert!(st.take_notices(&grok, "http://h:1").unwrap().is_empty(), "announced once");
    // The notice delivered nothing: the Stop hook still hands the row over, once.
    assert_eq!(take(&st, &grok, Tier::StopHook).len(), 1);
    assert!(take(&st, &grok, Tier::StopHook).is_empty());
}

#[test]
fn a_delivered_row_is_never_announced() {
    let (_d, st) = store();
    let grok = session(&st, "grok", "g1");
    let aid = artifact(&st, Some(&grok));
    st.ensure_watch(&grok, &aid).unwrap();
    let tid = thread(&st, &aid, "hi");
    st.send_to_agent(&tid).unwrap();
    take(&st, &grok, Tier::Piggyback);
    assert!(st.take_notices(&grok, "http://h:1").unwrap().is_empty());
}

#[test]
fn a_retargeted_row_is_announced_to_its_new_session() {
    let (_d, st) = store();
    let first = session(&st, "grok", "g1");
    let aid = artifact(&st, Some(&first));
    st.ensure_watch(&first, &aid).unwrap();
    let tid = thread(&st, &aid, "hi");
    st.send_to_agent(&tid).unwrap();
    assert_eq!(st.take_notices(&first, "http://h:1").unwrap().len(), 1);
    st.end_session(&first).unwrap();
    let next = session(&st, "grok", "g2");
    st.watch(&next, &aid, true).unwrap();
    st.retarget_untargeted(&aid, &next).unwrap();
    assert_eq!(st.take_notices(&next, "http://h:1").unwrap().len(), 1);
}
```

In `crates/clax-core/src/feedback.rs` `mod tests`:

```rust
#[test]
fn a_notice_is_one_line_and_carries_no_comment_text() {
    let n = Notice {
        feedback_id: "f".into(),
        comment_id: "c".into(),
        thread_id: "01J9T".into(),
        artifact_id: "7q3k9mzx2b4t".into(),
        title: "Quarterly\nReview".into(),
        url: "http://localhost:7480/a/7q3k9mzx2b4t".into(),
    };
    assert_eq!(
        render_notice(&n),
        "[clax] New comment on \"Quarterly Review\" (http://localhost:7480/a/7q3k9mzx2b4t), thread 01J9T. Call comments_read with url_or_id \"7q3k9mzx2b4t\" and thread_id \"01J9T\" to read it; if you have already handled it, do nothing."
    );
}
```

Run: `cargo test -p clax-core notice`. Expected: FAIL (nothing exists yet).

- [ ] **Step 2: The migration, the store and the line**

Append a migration to `MIGRATIONS` in `crates/clax-core/src/store/migrations.rs`, following the existing entries:

```sql
-- When `clax feedback follow` announced the row to its target session (a
-- notice, not a delivery); cleared when the row is retargeted.
ALTER TABLE feedback ADD COLUMN notified_at TEXT;
```

Other agents may have added migrations meanwhile. Append yours after the last entry in the file as it is when you start, and update any test that counts migrations.

In `crates/clax-core/src/feedback.rs`:

```rust
/// A comment announced to a session's follower (`clax feedback follow`):
/// where it is, not what it says. Announcing is not delivering.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Notice {
    pub feedback_id: String,
    pub comment_id: String,
    pub thread_id: String,
    pub artifact_id: String,
    pub title: String,
    /// The artifact's browser URL.
    pub url: String,
}

/// Longest title a notice line quotes, in characters.
const NOTICE_TITLE_CHARS: usize = 80;

/// The one line `clax feedback follow` prints for `n`: the artifact, the
/// thread, and the tool call that reads it. It never includes the comment.
pub fn render_notice(n: &Notice) -> String {
    let title: String = n
        .title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('"', "'")
        .chars()
        .take(NOTICE_TITLE_CHARS)
        .collect();
    format!(
        "[clax] New comment on \"{title}\" ({url}), thread {tid}. Call comments_read with url_or_id \"{aid}\" and thread_id \"{tid}\" to read it; if you have already handled it, do nothing.",
        url = n.url,
        tid = n.thread_id,
        aid = n.artifact_id,
    )
}
```

In `crates/clax-core/src/store/feedback.rs`, inside `impl Store`:

```rust
    /// Announces the session's rows that no tier has delivered and no
    /// follower has announced, on open threads of live artifacts that the
    /// session watches with replies armed: stamps `notified_at` and returns
    /// one notice per row, oldest first. A row is announced at most once per
    /// target; the stamp is set only where it is still unset, so concurrent
    /// followers never announce the same row twice. Delivery is unaffected:
    /// the rows still wait for tiers 1, 2 and 4.
    pub fn take_notices(&self, session_id: &str, browser_base: &str) -> Result<Vec<Notice>> {
        let base = browser_base.trim_end_matches('/').to_string();
        let now = Store::now();
        self.with_tx(|tx| {
            let mut stmt = tx.prepare(
                "SELECT f.id, f.comment_id, f.thread_id, t.artifact_id, a.title
                 FROM feedback f
                 JOIN threads t ON t.id = f.thread_id
                 JOIN artifacts a ON a.id = t.artifact_id
                 WHERE f.target_session_id = ?1
                   AND f.delivered_at IS NULL AND f.notified_at IS NULL
                   AND a.deleted_at IS NULL AND t.status = 'open'
                   AND EXISTS (SELECT 1 FROM watches w WHERE w.session_id = f.target_session_id
                               AND w.artifact_id = t.artifact_id AND w.replies_armed = 1)
                 ORDER BY f.created_at, f.id",
            )?;
            let rows = stmt
                .query_map(params![session_id], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?, r.get::<_, String>(4)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            drop(stmt);
            let mut out = Vec::new();
            for (feedback_id, comment_id, thread_id, artifact_id, title) in rows {
                let changed = tx.execute(
                    "UPDATE feedback SET notified_at = ?2
                     WHERE id = ?1 AND notified_at IS NULL AND delivered_at IS NULL",
                    params![feedback_id, now],
                )?;
                if changed == 1 {
                    let url = format!("{base}/a/{artifact_id}");
                    out.push(Notice { feedback_id, comment_id, thread_id, artifact_id, title, url });
                }
            }
            Ok(out)
        })
    }
```

Use the file's own helpers for the transaction and the timestamp if they differ from `with_tx` and `Store::now()`. In the statement at line 69 that untargets rows (`UPDATE feedback SET target_session_id = NULL, untargeted_at = ?2 WHERE id = ?1`), add `, notified_at = NULL`. Do the same in `retarget_untargeted`'s `UPDATE`, so a retargeted row is announced again to its new session.

Run: `cargo test -p clax-core`. Expected: PASS.

- [ ] **Step 3: Write the failing route tests**

Create `crates/clax-server/tests/api_notices.rs`, using `common::TestServer` and the helpers the feedback tests use to create an artifact, a watched thread and a send-to-agent (copy them from `api_feedback.rs` if they are local to it):

```rust
mod common;
use common::TestServer;
use serde_json::{Value, json};

// grok_session(ts) registers {"harness": "grok", "harness_session_id": "g1", ...} and returns its ID;
// sent_comment(ts, sid) publishes as sid (which watches with replies armed), opens a thread and sends it to the agent; returns (artifact ID, thread ID).

#[tokio::test]
async fn a_notice_points_at_the_comment_once_and_delivers_nothing() {
    let ts = TestServer::spawn().await;
    let sid = grok_session(&ts).await;
    let (aid, tid) = sent_comment(&ts, &sid).await;
    let v: Value = ts.get_authed(&format!("/api/sessions/{sid}/notices?wait=0")).await.json().await.unwrap();
    assert_eq!(v["notices"].as_array().unwrap().len(), 1, "{v}");
    assert_eq!(v["notices"][0]["thread_id"], tid);
    let line = v["lines"][0].as_str().unwrap();
    assert!(line.starts_with("[clax] New comment on ") && line.contains(&aid) && !line.contains('\n'), "{line}");
    let again: Value = ts.get_authed(&format!("/api/sessions/{sid}/notices?wait=0")).await.json().await.unwrap();
    assert!(again["notices"].as_array().unwrap().is_empty());
    // Still undelivered: the Stop hook's tier hands it over.
    let f: Value = ts.get_authed(&format!("/api/sessions/{sid}/feedback?tier=stop_hook")).await.json().await.unwrap();
    assert_eq!(f["feedback"].as_array().unwrap().len(), 1, "{f}");
}

#[tokio::test]
async fn a_waiting_notices_poll_wakes_on_a_new_comment() {
    // Start ?wait=10 in a task, then send a comment; it returns within 2 s with one notice.
}

#[tokio::test]
async fn notices_stay_quiet_while_the_session_waits_for_feedback() {
    // Start /feedback?tier=wait&wait=5 in a task; after it is in progress, send a comment;
    // /notices?wait=0 answers {"notices": [], "lines": [], "waited_s": 0}; the wait returns the comment.
}

#[tokio::test]
async fn grok_push_reports_whether_a_follower_is_connected() {
    let ts = TestServer::spawn().await;
    let sid = grok_session(&ts).await;
    let push = |v: Value| v["push"].clone();
    let before: Value = ts.get_authed(&format!("/api/sessions/{sid}")).await.json().await.unwrap();
    assert_eq!(push(before.clone())["tier"], "monitor");
    assert_eq!(push(before)["available"], false);
    // While a ?wait=5 notices poll is in progress (spawned task), available is true.
}

#[tokio::test]
async fn notices_of_an_unknown_or_ended_session_are_errors() {
    // 404 for an unknown session; 400 unknown_session for an ended one, before any wait.
}
```

Write each test body in full, following `api_feedback.rs`'s long-poll tests for the spawned-task pattern. Run: `cargo test -p clax-server --test api_notices`. Expected: FAIL (404 on the route).

- [ ] **Step 4: The route and the follower registry**

`crates/clax-server/src/feedback.rs`:

```rust
/// Sessions with a `clax feedback follow` connected: one in a notices
/// long-poll now, or whose last poll ended under [`Followers::RECENT`]
/// ago (the gap while it prints and polls again).
#[derive(Default)]
pub struct Followers {
    active: Mutex<HashMap<String, usize>>,
    last: Mutex<HashMap<String, std::time::Instant>>,
}

impl Followers {
    pub const RECENT: std::time::Duration = std::time::Duration::from_secs(15);

    /// Counts a notices poll of `session_id` until the guard drops.
    pub fn enter(self: &Arc<Self>, session_id: &str) -> FollowGuard {
        *self.active.lock().unwrap().entry(session_id.to_string()).or_default() += 1;
        FollowGuard { followers: self.clone(), session_id: session_id.to_string() }
    }

    /// Whether a follower of `session_id` is connected.
    pub fn is_following(&self, session_id: &str) -> bool {
        self.active.lock().unwrap().contains_key(session_id)
            || self.last.lock().unwrap().get(session_id).is_some_and(|t| t.elapsed() < Self::RECENT)
    }
}

pub struct FollowGuard {
    followers: Arc<Followers>,
    session_id: String,
}

impl Drop for FollowGuard {
    fn drop(&mut self) {
        let mut active = self.followers.active.lock().unwrap();
        if let Some(n) = active.get_mut(&self.session_id) {
            *n -= 1;
            if *n == 0 {
                active.remove(&self.session_id);
            }
        }
        self.followers.last.lock().unwrap().insert(self.session_id.clone(), std::time::Instant::now());
    }
}
```

Add `pub followers: Arc<crate::feedback::Followers>` to `AppState`, built with `Default`, next to `feedback_waiters`. Where an ended session is forgotten (`feedback_waiters.forget`), also remove its `last` entry.

`crates/clax-server/src/routes/feedback.rs`:

```rust
#[derive(Deserialize)]
pub struct NoticesQuery {
    #[serde(default)]
    wait: u64,
}

/// `GET /api/sessions/<sid>/notices?wait=<s>`: announces the session's
/// armed, undelivered comments that no follower has announced
/// (`Store::take_notices`), as soon as there are any or after `wait`
/// seconds (capped at 600): `{notices, lines, waited_s}`, one line per
/// notice. Announcing delivers nothing. While the session is inside
/// `wait_for_feedback`, answers empty at once and announces nothing. The
/// poll counts as a connected follower for `status`'s `push`. Unknown
/// session: 404; ended: 400 `unknown_session`, before any wait. A daemon
/// that begins shutting down answers empty at once.
pub async fn notices(
    State(s): State<AppState>,
    _t: RequireToken,
    sid: Result<Path<String>, PathRejection>,
    q: Result<Query<NoticesQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let sid = path(sid)?;
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let check = sid.clone();
    s.store_call(move |st| live_session(st, &check)).await?;
    let _following = s.followers.enter(&sid);
    let started = Instant::now();
    let deadline = started + Duration::from_secs(q.wait.min(MAX_WAIT_SECS));
    let notify = s.feedback_waiters.get(&sid);
    let mut shutdown = s.shutdown.clone();
    let stopping = async move {
        if shutdown.wait_for(|v| *v).await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    tokio::pin!(stopping);
    let empty = |waited: u64| Json(json!({"notices": [], "lines": [], "waited_s": waited}));
    loop {
        let notified = notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if s.feedback_waiters.is_waiting(&sid) {
            return Ok(empty(0));
        }
        let base = s.feedback_ctx().browser_base.clone();
        let who = sid.clone();
        let notices = s.store_call(move |st| st.take_notices(&who, &base)).await?;
        if !notices.is_empty() || Instant::now() >= deadline {
            let lines: Vec<String> = notices.iter().map(clax_core::feedback::render_notice).collect();
            return Ok(Json(json!({"notices": notices, "lines": lines, "waited_s": started.elapsed().as_secs()})));
        }
        tokio::select! {
            _ = &mut notified => {}
            _ = tokio::time::sleep_until(deadline) => {}
            _ = &mut stopping => return Ok(empty(started.elapsed().as_secs())),
        }
    }
}
```

Use the same `browser_base` the `poll` handler passes to `take_feedback` (`s.feedback_ctx()`'s field, or whatever `poll` reads). Register `.route("/api/sessions/{id}/notices", get(feedback::notices))` next to the feedback route. Check that send-to-agent and retargeting wake `feedback_waiters` for the target session, which they already do for the inject and wait polls. If a route needs it, add the wake there.

`crates/clax-server/src/routes/sessions.rs`: give `push_info` a `following: bool` parameter (the call site passes `s.followers.is_following(&session.id)`), and change the Grok arm from Task 2 to:

```rust
        "grok" => {
            let reason = (!following).then_some(
                "no clax feedback follow is running for this session; the clax-grok skill starts one with Grok's monitor tool after a publish. Meanwhile comments arrive at the end of a turn (Stop hook), on the next clax tool call, or during wait_for_feedback",
            );
            json!({"tier": "monitor", "available": following, "reason": reason})
        }
```

Update Task 2's test `a_grok_session_has_no_native_push` to match: `tier` is `"monitor"`, `available` is `false` with no follower, and the reason starts with `no clax feedback follow is running`. Rename it `a_grok_session_without_a_follower_has_no_push`.

Run: `cargo test -p clax-server`. Expected: PASS.

- [ ] **Step 5: Write the failing command tests**

Create `crates/clax-cli/tests/follow.rs`. It starts a daemon with `clax --port 0 serve` in a scratch home (as `crates/clax-hooks/tests/golden.rs` does), registers a Grok session over REST with the token from `daemon.json`, and spawns `clax feedback follow` with `stdout` piped, a `BufReader`, and a reader thread feeding a channel, so each assertion can wait with a timeout:

```rust
#[test]
fn it_prints_one_line_per_new_comment_and_nothing_else() {
    // Spawn `clax feedback follow --agent grok --harness-session g1 --poll-secs 2`.
    // Publish as the session (which arms a watch), open a thread, send it to the agent.
    // Within 5 s exactly one line arrives; it starts with "[clax] New comment on " and names the thread.
    // A second comment on another thread produces exactly one more line.
    // Nothing is written to stderr.
}

#[test]
fn it_finds_the_grok_session_from_the_environment() {
    // Same, with no flags and GROK_SESSION_ID=g1 in the environment.
}

#[test]
fn it_survives_a_daemon_restart() {
    // Start following; `clax stop`; start a daemon again on --port 0 in the same home and
    // register the session again; send a comment; the follower prints its line within 10 s.
}

#[test]
fn it_exits_cleanly_when_the_session_ends() {
    // `--grace-secs 1`; end the session with PATCH {"ended": true};
    // the process exits 0 within 5 s and printed nothing.
}

#[test]
fn it_exits_at_once_when_a_followed_clax_session_ends() {
    // `--session <clax session ID>`; end it; exit 0 within 3 s.
}

#[test]
fn without_a_session_it_is_a_usage_error() {
    // No flags and no GROK_SESSION_ID: exit 2, stderr names --session, --agent/--harness-session and GROK_SESSION_ID.
}
```

Write each in full. Clear the harness environment in every spawned command (Global Constraints). Run: `cargo test -p clax-cli --test follow`. Expected: FAIL (no `feedback` command).

- [ ] **Step 6: The command**

`crates/clax-cli/src/commands/feedback.rs`:

```rust
//! `clax feedback follow`: prints one line per comment sent to an agent
//! session, for a harness that turns a command's output lines into wake-ups
//! (Grok Build's `monitor` tool). Each line is a notice that points at the
//! comment; it delivers nothing (see `Store::take_notices`). The command
//! never starts a daemon: it waits for one, follows the session across
//! daemon restarts, and exits 0 once the session has ended.

use crate::client::Client;
use clax_core::Home;
use serde_json::Value;
use std::io::Write;
use std::time::{Duration, Instant};

#[derive(clap::Subcommand)]
pub enum Cmd {
    /// Print one line per comment sent to an agent session, as it arrives.
    ///
    /// Each line names the artifact and thread and says to call
    /// comments_read; it never contains the comment. Following does not
    /// deliver: the comment still reaches the agent through its next clax
    /// tool call, its Stop hook, or wait_for_feedback. The session is
    /// --session (a Clax session ID), or --agent with --harness-session (the
    /// harness's own ID), or, inside Grok Build, GROK_SESSION_ID. Exits 0
    /// once the session has ended.
    Follow(FollowArgs),
}

#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum FollowAgent {
    Claude,
    Codex,
    Grok,
}

impl FollowAgent {
    fn harness(self) -> &'static str {
        match self {
            FollowAgent::Claude => "claude",
            FollowAgent::Codex => "codex",
            FollowAgent::Grok => "grok",
        }
    }
}

#[derive(clap::Args)]
pub struct FollowArgs {
    /// The Clax session to follow (`status` reports it as session.id).
    #[arg(long, conflicts_with_all = ["agent", "harness_session"])]
    pub session: Option<String>,
    /// The harness whose session --harness-session names.
    #[arg(long, value_enum, requires = "harness_session")]
    pub agent: Option<FollowAgent>,
    /// The harness's own session ID (`status` reports it as
    /// session.harness_session_id).
    #[arg(long, requires = "agent")]
    pub harness_session: Option<String>,
    /// Seconds each daemon long-poll waits.
    #[arg(long, hide = true, default_value_t = 50)]
    pub poll_secs: u64,
    /// Seconds a followed harness session may have no live Clax session
    /// before the command exits.
    #[arg(long, hide = true, default_value_t = 60)]
    pub grace_secs: u64,
}

/// What to follow.
enum Target {
    /// A Clax session ID: following ends when it ends.
    Session(String),
    /// A harness session, resolved to its live Clax session on each poll,
    /// so a session re-registered after a daemon restart is followed too.
    Harness(&'static str, String),
}

fn target(a: &FollowArgs, env: impl Fn(&str) -> Option<String>) -> Option<Target> {
    if let Some(s) = &a.session {
        return Some(Target::Session(s.clone()));
    }
    if let (Some(agent), Some(id)) = (a.agent, &a.harness_session) {
        return Some(Target::Harness(agent.harness(), id.clone()));
    }
    env("GROK_SESSION_ID")
        .filter(|v| !v.is_empty())
        .map(|id| Target::Harness("grok", id))
}

/// The live Clax session of `(harness, id)`, if any.
fn live_session(c: &Client, harness: &str, id: &str) -> anyhow::Result<Option<String>> {
    let v = c.get("/api/sessions?live=true")?;
    Ok(v["sessions"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|s| s["harness"] == harness && s["harness_session_id"].as_str() == Some(id))
        .and_then(|s| s["id"].as_str())
        .map(str::to_string))
}

/// Whether a daemon error says the session is gone.
fn session_gone(e: &anyhow::Error) -> bool {
    let m = e.to_string();
    m.starts_with("unknown_session:") || m.starts_with("not_found:")
}

pub fn run(_cli: &crate::Cli, home: &Home, cmd: &Cmd) -> anyhow::Result<()> {
    let Cmd::Follow(a) = cmd;
    let Some(target) = target(a, |k| std::env::var(k).ok()) else {
        eprintln!("clax feedback follow: no session: pass --session, or --agent with --harness-session, or run it inside Grok Build (GROK_SESSION_ID)");
        std::process::exit(2);
    };
    let poll = Duration::from_secs(a.poll_secs.max(1));
    let grace = Duration::from_secs(a.grace_secs);
    let mut backoff = Duration::from_secs(1);
    let mut missing_since: Option<Instant> = None;
    let mut stdout = std::io::stdout();
    loop {
        let Some(client) = Client::discover(home) else {
            // No daemon: wait for one; another client starts it.
            std::thread::sleep(backoff);
            backoff = (backoff * 2).min(Duration::from_secs(30));
            continue;
        };
        let sid = match &target {
            Target::Session(s) => s.clone(),
            Target::Harness(h, id) => match live_session(&client, h, id) {
                Ok(Some(s)) => {
                    missing_since = None;
                    s
                }
                Ok(None) => {
                    let since = *missing_since.get_or_insert_with(Instant::now);
                    if since.elapsed() >= grace {
                        return Ok(());
                    }
                    std::thread::sleep(Duration::from_secs(1).min(grace));
                    continue;
                }
                Err(_) => {
                    std::thread::sleep(backoff);
                    backoff = (backoff * 2).min(Duration::from_secs(30));
                    continue;
                }
            },
        };
        let path = format!("/api/sessions/{sid}/notices?wait={}", poll.as_secs());
        match client.get_with_timeout(&path, poll + Duration::from_secs(10)) {
            Ok(v) => {
                backoff = Duration::from_secs(1);
                for line in v["lines"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                    writeln!(stdout, "{line}")?;
                }
                stdout.flush()?;
            }
            Err(e) if session_gone(&e) => {
                if let Target::Session(_) = target {
                    return Ok(());
                }
                // A harness session may be registered again; resolve it anew.
            }
            Err(_) => {
                std::thread::sleep(backoff);
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
        }
    }
}
```

`writeln!` on a closed stdout (the monitor was stopped) returns an error, and the command exits with it. That is the right outcome. Wire the command in `main.rs`:

```rust
    /// Follow comments sent to an agent session (one line per comment).
    #[command(subcommand)]
    Feedback(commands::feedback::Cmd),
```

with `Cmd::Feedback(c) => commands::feedback::run(&cli, &home, c)` in the dispatch, and add `pub mod feedback;` to `commands/mod.rs`. Add unit tests for `target` (flags win over the environment; `GROK_SESSION_ID` alone gives `Harness("grok", …)`; nothing gives `None`).

Run: `cargo test -p clax-cli --test follow` and `cargo test -p clax-cli feedback`. Expected: PASS.

- [ ] **Step 7: Gates and stage**

Run the quality gates. Stage the Task 6 files.

Proposed commit message: `Add clax feedback follow: one notice line per comment sent to a session, for Grok's monitor tool; notices wake the agent and deliver nothing`

---

### Task 7: The `clax-grok` plugin

**Files:**
- Create: `plugins/clax-grok/.grok-plugin/plugin.json`, `plugins/clax-grok/.mcp.json`, `plugins/clax-grok/hooks/hooks.json`, `plugins/clax-grok/skills/clax/SKILL.md`, `plugins/clax-grok/README.md`, `plugins/clax-grok/scripts/ensure-clax.sh` (a copy of the wrapper, mode 0755)
- Create: `.grok-plugin/marketplace.json`
- Modify: `crates/clax-cli/src/plugins.rs`, `crates/clax-cli/build.rs` (embed both)
- Modify: `scripts/check-version.sh`, `scripts/bump-version.sh`, `scripts/sync-skill-tools.py`, `scripts/test-plugins.sh`
- Modify: `plugins/claude-code/skills/clax/SKILL.md`, `plugins/clax/skills/clax/SKILL.md`, `plugins/pi/skills/clax/SKILL.md` (the shared "Comment loop" section names Grok; the Claude skill's intro says it is idle in Grok)
- Modify: `README.md` (the plugin list)

**Interfaces:**
- Consumes: Task 5's wrapper; Task 2's `--agent grok` shim; Task 3's hooks; Task 6's `clax feedback follow` and Grok's `push`.
- Produces: the plugin tree at `plugins/clax-grok`, written by `clax init` to `<home>/marketplace/plugins/clax-grok` (Task 7). Its server name is `clax_grok`; Tasks 8 and 10 rely on it.

- [ ] **Step 1: Write the failing plugin checks**

In `scripts/test-plugins.sh`:

1. Add `.grok-plugin/marketplace.json` to `json_files`, and add the clax-grok files to the "is missing" loop: `plugins/clax-grok/.grok-plugin/plugin.json plugins/clax-grok/.mcp.json plugins/clax-grok/hooks/hooks.json plugins/clax-grok/skills/clax/SKILL.md plugins/clax-grok/README.md`.
2. Extend `for plugin in plugins/claude-code plugins/clax; do` to `plugins/claude-code plugins/clax plugins/clax-grok`. The `.mcp.json` and SessionStart/SessionEnd checks then cover it.
3. Add the Grok checks:

```bash
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
```

4. Add `plugins/clax-grok/scripts/ensure-clax.sh` to the wrapper loop (`for wrapper in …`).
5. Add `plugins/clax-grok/skills/clax/SKILL.md` to `skill_copies`. Then, after the `section` function is defined, check the skill's own section:

```bash
# The Grok skill tells the agent to start one persistent monitor on
# clax feedback follow, with the values from clax_grok__status.
grok_skill=plugins/clax-grok/skills/clax/SKILL.md
live="$(section "$grok_skill" "Live feedback in Grok")"
if [ -n "$live" ] && echo "$live" | grep -qF 'feedback follow --agent grok --harness-session' \
    && echo "$live" | grep -qF 'persistent: true' && echo "$live" | grep -qF 'push.available' \
    && echo "$live" | grep -qF 'clax_grok__comments_read'; then
    pass "$grok_skill has the Live feedback in Grok section"
else fail "$grok_skill needs a '## Live feedback in Grok' section naming the monitor command, persistent: true, push.available and clax_grok__comments_read"; fi
```
6. Add a check that the wrapper's stand-down text matches the binary's, if Task 5's test does not already run in this script:

```bash
want="$(sed -n 's/^pub const GROK_STANDDOWN: &str = "\(.*\)";$/\1/p' crates/clax-mcp/src/standdown.rs)"
got="$(sed -n 's/^GROK_STANDDOWN="\(.*\)"$/\1/p' scripts/ensure-clax.sh | sed 's/\\`/`/g')"
if [ -n "$want" ] && [ "$want" = "$got" ]; then pass "the wrapper's GROK_STANDDOWN matches crates/clax-mcp/src/standdown.rs"
else fail "the wrapper's GROK_STANDDOWN differs from crates/clax-mcp/src/standdown.rs"; fi
```

Run: `scripts/test-plugins.sh`. Expected: FAIL (the plugin does not exist).

- [ ] **Step 2: The plugin files**

`plugins/clax-grok/.grok-plugin/plugin.json` (the version is the workspace version, `scripts/check-version.sh --print`):

```json
{
  "name": "clax-grok",
  "version": "0.3.0",
  "description": "Local artifacts with comment-driven development for Grok Build: publish HTML pages, view them, and get feedback back",
  "author": {
    "name": "Empathic"
  },
  "keywords": ["artifacts", "html", "preview", "comments"]
}
```

`plugins/clax-grok/.mcp.json`:

```json
{
  "mcpServers": {
    "clax_grok": {
      "command": "${GROK_PLUGIN_ROOT}/scripts/ensure-clax.sh",
      "args": ["exec", "mcp", "--agent", "grok"],
      "env": {"GROK_PLUGIN_ROOT": "${GROK_PLUGIN_ROOT}"}
    }
  }
}
```

`plugins/clax-grok/hooks/hooks.json`:

```json
{
  "hooks": {
    "SessionStart": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "\"${GROK_PLUGIN_ROOT}/scripts/ensure-clax.sh\" exec hook --agent grok session-start",
            "timeout": 5
          }
        ]
      }
    ],
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "\"${GROK_PLUGIN_ROOT}/scripts/ensure-clax.sh\" exec hook --agent grok stop",
            "timeout": 10
          }
        ]
      }
    ],
    "SessionEnd": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "\"${GROK_PLUGIN_ROOT}/scripts/ensure-clax.sh\" exec hook --agent grok session-end",
            "timeout": 2
          }
        ]
      }
    ]
  }
}
```

`plugins/clax-grok/scripts/ensure-clax.sh`: `cp scripts/ensure-clax.sh plugins/clax-grok/scripts/ensure-clax.sh && chmod 755 plugins/clax-grok/scripts/ensure-clax.sh`.

`plugins/clax-grok/skills/clax/SKILL.md`: copy `plugins/clax/skills/clax/SKILL.md`, then replace its two-line harness paragraph after the tools block (`In Codex the tools are named …`) with:

```markdown
In Grok Build the tools are reached through `use_tool` with the qualified
name `clax_grok__<tool>`, for example `clax_grok__publish`; `search_tool`
finds them. Grok asks before each call unless the person allows
`MCPTool(clax_grok__*)`.
```

Then add a section after "## Comment loop" (a Grok-only section, not shared with the other skills):

```markdown
## Live feedback in Grok

Grok can wake you when a comment is sent to you, through its `monitor`
tool. Start one monitor per session, after your first publish in it
(publishing watches the artifact with replies on):

1. Call `clax_grok__status`. If `push.available` is true, a monitor is
   already running for this session: do not start another.
2. Otherwise call Grok's `monitor` tool with `persistent: true` and the
   command `"<binary.path>" feedback follow --agent grok --harness-session
   <session.harness_session_id>`, with both values from that `status`
   result. It prints nothing until a comment arrives.

Each line it prints names an artifact and a thread. Call
`clax_grok__comments_read` with the `url_or_id` and `thread_id` it gives,
do what the comment asks, then reply and resolve as in "Comment loop". If
you have already handled that thread, do nothing. The line never contains
the comment itself; reading the thread is what hands it to you. The monitor
stops by itself when the session ends.
```

Leave the generated tools block for Step 4.

`.grok-plugin/marketplace.json`:

```json
{
  "name": "clax",
  "owner": {
    "name": "Empathic"
  },
  "plugins": [
    {
      "name": "clax-grok",
      "description": "Local artifacts with comment-driven development for Grok Build: publish HTML pages, view them, and get feedback back",
      "source": "./plugins/clax-grok"
    }
  ]
}
```

`plugins/clax-grok/README.md`. Model it on `plugins/clax/README.md`, with these sections:
- **Title and summary:** `# Clax for Grok Build`, then why the directory and plugin are named `clax-grok` (Grok discovers the Claude Code plugin named `clax`; plugin-name conflicts resolve before enabling) and why the server is `clax_grok` (Grok keeps the first MCP server definition of a name).
- **Install:** `just install` or `clax init` runs `grok plugin install ~/.clax/marketplace/plugins/clax-grok --trust`. Then start a new Grok session. It works when Grok is the only harness installed.
- **What it adds**, starting with the list line that `sync-skill-tools.py` checks: `- The `clax_grok` MCP server (`clax mcp --agent grok`): twenty-two tools,` followed by the same twenty-two names as `plugins/clax/README.md`, ending `which Grok names `clax_grok__<tool>` and reaches through `use_tool`.` Then the skill, and the three hooks with their timeouts and what each does (from spec §13).
- **The Claude Code plugin in Grok:** Grok lists the Claude Code plugin as a disabled User-scope plugin named `clax`. If you enable it, it stands down: its server offers only `clax__status`, and its hooks do nothing. `grok plugin disable clax` removes it. Include the combination table from this plan's Design decisions, in prose or as a table.
- **Settings you may want** (**provisional (Q1, Q2)**): the approval rule `[permission] allow = ["MCPTool(clax_grok__*)"]` in `~/.grok/config.toml` (it approves `delete` too); `--always-approve` for headless `grok -p`; the sandbox note from spec §13.
- **Feedback tiers:** tiers 1, 2, 4 and 5. Tier 5 is the monitor the skill starts: each comment wakes an idle session with a notice line, and the comment itself arrives with the next `comments_read`, at the end of the turn, or in `wait_for_feedback`. Explain why there is no tier 3, and that headless `grok -p` gets no monitor.
- **Working from a source checkout:** `just dev grok` runs the installed plugin with the fresh build first on `PATH`; run `just install` to try plugin changes.
- **Troubleshooting:** `clax doctor --agent grok`, and `~/.clax/logs/hooks.log` (`agent=grok` lines, and `standdown` lines for the Claude copy).

- [ ] **Step 3: Embed it**

`crates/clax-cli/src/plugins.rs`:
- add `#[include = "clax-grok/**"]` to `Plugins`;
- add `const GROK_MARKETPLACE: &str = include_str!("../../../.grok-plugin/marketplace.json");`;
- add `(".grok-plugin/marketplace.json".to_string(), GROK_MARKETPLACE.as_bytes().to_vec())` to the start of `files()`;
- update the module doc's layout sentence to name `.grok-plugin/marketplace.json` and `plugins/{claude-code,clax,clax-grok,pi}`;
- in the test that lists expected embedded paths, add `"plugins/clax-grok/.grok-plugin/plugin.json"`, `"plugins/clax-grok/.mcp.json"`, `"plugins/clax-grok/hooks/hooks.json"`, `"plugins/clax-grok/scripts/ensure-clax.sh"` and `".grok-plugin/marketplace.json"`. Also assert that `materialize` writes `plugins/clax-grok/scripts/ensure-clax.sh` with mode 0755, as the existing `*.sh` rule does.

`crates/clax-cli/build.rs`: add `"plugins/clax-grok"` to `DIRS` and `".grok-plugin/marketplace.json"` to `FILES`.

Run: `cargo test -p clax-cli plugins`. Expected: PASS.

- [ ] **Step 4: Versions, skills and doc lists**

`scripts/check-version.sh`: add `"plugins/clax-grok/.grok-plugin/plugin.json": load("plugins/clax-grok/.grok-plugin/plugin.json").get("version")` to `versions`, add `plugins/clax-grok/scripts/ensure-clax.sh` to the wrapper tuple, and `plugins/clax-grok/skills/clax/SKILL.md` to the skill tuple. In its header comment, `the three copies of ensure-clax.sh` becomes `the copies of ensure-clax.sh`.

`scripts/bump-version.sh`: add `"plugins/clax-grok/.grok-plugin/plugin.json"` to the manifest tuple, and `"plugins/clax-grok/scripts/ensure-clax.sh"` to the wrapper tuple. The manifest's `"version"` line must be indented two spaces, as the regex expects.

`scripts/sync-skill-tools.py`:
- add to `SKILLS`: `("plugins/clax-grok/skills/clax/SKILL.md", "plugins/clax-grok/.grok-plugin/plugin.json", "", "as the `clax_grok` MCP server")`;
- add to `DOC_LISTS`: `("plugins/clax-grok/README.md", r"^- The `clax_grok` MCP server [^\n]*?: ([a-z-]+) tools", "")`;
- in `names`, exclude the server name as well: `found = set(re.findall(r"`([a-z_]+)`", text)) - {"clax", "clax_grok"}`.

Run `python3 scripts/sync-skill-tools.py` (it writes the clax-grok skill's tools block), then `python3 scripts/sync-skill-tools.py --check`. Expected: no output, exit 0.

In the shared "Comment loop" section of all four skills, change `the watch has replies on (Claude Code, Codex).` to `the watch has replies on (Claude Code, Codex, Grok Build).` The section must stay identical in all four copies. In the Claude Code skill's intro paragraph (after the tools block, which is not a shared section), add:

```markdown
Grok Build also loads this plugin when it is enabled there; in Grok it does
nothing, and Clax runs from the clax-grok plugin instead.
```

`README.md`: in the harness list and the plugin-details sentence, add Grok Build and `[plugins/clax-grok/README.md](plugins/clax-grok/README.md)`.

- [ ] **Step 5: Run the checks**

Run: `scripts/test-plugins.sh`, `scripts/check-version.sh`, `cargo test -p clax-cli plugins`. Expected: all pass.

- [ ] **Step 6: Gates and stage**

Run the quality gates. Stage the new plugin tree, `.grok-plugin/marketplace.json`, and the modified files.

Proposed commit message: `Add the clax-grok plugin: a clax_grok MCP server and Session, Stop and SessionEnd hooks for Grok Build, embedded for clax init`

---

### Task 8: `clax init` and `clax uninit` for Grok

**Files:**
- Modify: `crates/clax-cli/src/commands/doctor_agent.rs` (`Dirs.grok_home`)
- Modify: `crates/clax-cli/src/commands/init.rs` (the `grok` harness)
- Modify: `crates/clax-cli/src/main.rs` (the `Init`/`Uninit` doc comments name Grok)
- Modify: `crates/clax-cli/tests/init.rs`

**Interfaces:**
- Consumes: the embedded `plugins/clax-grok` (Task 7).
- Produces: `clax init [--agent grok]` and `clax uninit [--agent grok]`; `Dirs::grok_home` (`$GROK_HOME`, else `~/.grok`), which Task 9 uses.

- [ ] **Step 1: Write the failing tests**

In `crates/clax-cli/tests/init.rs`, make `Env::cmd` also set `GROK_HOME` to `self.p("grok")` and remove `GROK_SESSION_ID`, `GROK_HOOK_EVENT` and `CLAUDE_PID`. Give the fake CLI one more behaviour: when its arguments are `plugin list --json`, it prints the file `{dir}/grok-list.json` if that exists, else `[]`. (Add `if [ "$*" = "plugin list --json" ]; then cat '{list}' 2>/dev/null || echo '[]'; fi` before the final `exit 0` of the script `Env::new` writes, and keep recording the call.) Then:

```rust
#[test]
fn a_machine_with_only_grok_registers_clax_grok() {
    let e = Env::new(&["grok"]);
    let (ok, v) = e.json(&["init"]);
    assert!(ok, "{v}");
    for other in ["claude", "codex", "pi"] {
        assert_eq!(status(&v, other), "skipped", "{v}");
    }
    assert_eq!(status(&v, "grok"), "registered", "{v}");
    let dir = e.root().join("plugins/clax-grok");
    assert_eq!(
        e.calls(),
        [
            "grok plugin uninstall clax-grok --confirm".to_string(),
            format!("grok plugin install {} --trust", dir.display()),
        ]
    );
    assert!(dir.join(".grok-plugin/plugin.json").is_file());
    assert!(e.root().join(".grok-plugin/marketplace.json").is_file());
    assert!(!e.p(".grok").exists() && !e.p("grok").exists(), "init writes nothing in a Grok home");
}

#[test]
fn no_grok_command_names_the_claude_code_plugin() {
    let e = Env::new(&["claude", "codex", "grok", "pi"]);
    assert!(e.json(&["init"]).0);
    assert!(e.json(&["uninit"]).0);
    for c in e.calls().iter().filter(|c| c.starts_with("grok ")) {
        assert!(
            !c.split_whitespace().any(|w| w == "clax" || w == "clax@clax"),
            "{c}: in Grok, `clax` is the Claude Code plugin's install"
        );
    }
}

#[test]
fn a_failed_grok_uninstall_is_ignored_and_a_failed_install_fails_grok_only() {
    let e = Env::new(&["claude", "grok"]);
    std::fs::write(
        e.p("fail"),
        format!(
            "grok plugin uninstall clax-grok --confirm\ngrok plugin install {} --trust\n",
            e.root().join("plugins/clax-grok").display()
        ),
    )
    .unwrap();
    let (ok, v) = e.json(&["init"]);
    assert!(!ok);
    assert_eq!(status(&v, "grok"), "failed");
    assert_eq!(status(&v, "claude"), "registered");
    assert!(detail(&v, "grok").contains("(ignored)"), "{v}");
}

#[test]
fn init_records_grok_and_uninit_removes_it_and_the_marketplace() {
    let e = Env::new(&["grok"]);
    assert!(e.json(&["init"]).0);
    let rec: serde_json::Value =
        serde_json::from_slice(&std::fs::read(e.p("ax/registrations.json")).unwrap()).unwrap();
    assert_eq!(rec["harnesses"]["grok"]["plugin"], "clax-grok");
    let (ok, v) = e.json(&["uninit"]);
    assert!(ok, "{v}");
    assert_eq!(status(&v, "grok"), "removed");
    assert!(e.calls().contains(&"grok plugin list --json".to_string()));
    assert!(!e.root().exists(), "{v}");
}

#[test]
fn uninit_keeps_the_marketplace_while_grok_still_lists_a_plugin_from_it() {
    let e = Env::new(&["grok"]);
    assert!(e.json(&["init"]).0);
    std::fs::write(
        e.p("grok-list.json"),
        serde_json::json!([{"name": "clax-grok", "source": e.root().join("plugins/clax-grok")}]).to_string(),
    )
    .unwrap();
    let (_, v) = e.json(&["uninit"]);
    assert!(e.root().exists());
    assert!(v["marketplace_detail"].as_str().unwrap().contains("grok still registers it"), "{v}");
}
```

Run: `cargo test -p clax-cli --test init grok`. Expected: FAIL (`grok` is not a harness; `--agent grok` is rejected).

- [ ] **Step 2: Implement**

`Dirs` in `doctor_agent.rs` gains a field and its lookup:

```rust
    /// `$GROK_HOME`, else `~/.grok`.
    pub grok_home: PathBuf,
```

```rust
            grok_home: var("GROK_HOME").unwrap_or_else(|| home.join(".grok")),
```

Then fix every place that builds a `Dirs` literal: the doctor tests' fixture constructs one.

`init.rs`, in the module doc, after the Claude Code and Codex sentence:

```rust
//! A Grok registration is removed by the name `clax-grok` only: in Grok,
//! `clax` names the Claude Code plugin that Grok discovers in
//! `~/.claude/plugins`, and no `grok` command here ever names it.
```

Add the harness to `HARNESSES` (between `codex` and `pi`):

```rust
    Harness {
        name: "grok",
        removals: grok_removals,
        additions: grok_additions,
        record: |root| json!({"plugin": GROK_PLUGIN, "source": grok_plugin_dir(root)}),
        uses: grok_uses,
        by_hand: |_| format!("grok plugin uninstall {GROK_PLUGIN} --confirm"),
    },
```

and its functions:

```rust
/// The Grok plugin's name. Never `clax`: that is the Claude Code plugin,
/// which Grok also discovers.
const GROK_PLUGIN: &str = "clax-grok";

/// The Grok plugin's directory inside the marketplace at `root`.
fn grok_plugin_dir(root: &Path) -> PathBuf {
    lexical(&root.join("plugins").join(GROK_PLUGIN))
}

fn grok_removals(_ctx: &Ctx) -> Actions {
    Actions {
        steps: vec![step(false, &["plugin", "uninstall", GROK_PLUGIN, "--confirm"])],
        notes: Vec::new(),
    }
}

fn grok_additions(root: &Path) -> Vec<Step> {
    let dir = grok_plugin_dir(root).display().to_string();
    vec![step(true, &["plugin", "install", dir.as_str(), "--trust"])]
}

/// Whether Grok still lists a plugin from under `root`: `grok plugin list
/// --json`, run in the home directory. Without `grok` on PATH, true when
/// `registrations.json` still records a Grok registration under `root`,
/// since Grok's own files are not read.
fn grok_uses(ctx: &Ctx, root: &Path) -> Result<bool, String> {
    if on_path("grok").is_none() {
        return Ok(ctx.recorded.get("grok").is_some_and(|r| {
            r["source"].as_str().is_some_and(|s| Path::new(s).starts_with(root))
        }));
    }
    let mut cmd = std::process::Command::new("grok");
    cmd.args(["plugin", "list", "--json"]).stdin(std::process::Stdio::null());
    if ctx.home.is_dir() {
        cmd.current_dir(&ctx.home);
    }
    let out = cmd.output().map_err(|e| format!("could not run `grok plugin list --json`: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "`grok plugin list --json` failed: {}",
            String::from_utf8_lossy(&out.stderr).lines().next().unwrap_or_default()
        ));
    }
    let v: Value = serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("could not parse `grok plugin list --json` ({e})"))?;
    Ok(json_mentions(&v, root, &ctx.home, &ctx.home))
}
```

**Provisional (Q4 a, b):** the flags and the list format come from Grok's user guide, not a live run. If the smoke run shows otherwise, only these three functions change.

Add `grok` to the `Init` doc comment's harness list in `main.rs`, if it names them.

Run: `cargo test -p clax-cli --test init`. Expected: PASS, including the existing cases, which do not put `grok` on `PATH` and so report it `skipped`. If an existing case asserts the exact list of `agents`, add `grok` to it.

- [ ] **Step 3: Gates and stage**

Run the quality gates. Stage the four files.

Proposed commit message: `Register clax-grok with Grok Build in clax init and remove it in uninit, never naming the Claude Code plugin's install`

---

### Task 9: `clax doctor --agent grok` and `just dev grok`

**Files:**
- Modify: `crates/clax-cli/src/commands/doctor_agent.rs` (`DoctorAgent::Grok`, plugin roots, the `grok` and `claude_copy` checks)
- Modify: `crates/clax-cli/src/commands/doctor.rs`, if it lists the agents
- Modify: `scripts/dev.sh`, `scripts/test-dev.sh`, `justfile`

**Interfaces:**
- Consumes: `Dirs::grok_home` (Task 8); the `standdown` lines (Tasks 4, 5); the plugin's skill (Task 7).
- Produces: `clax doctor --agent grok`; `just dev grok`.

- [ ] **Step 1: Write the failing doctor tests**

In `doctor_agent.rs` `mod tests`, using the existing fixture that builds a scratch `Dirs` (extended with `grok_home`):

```rust
#[test]
fn the_grok_plugin_is_found_anywhere_under_grok_home() {
    let f = Fixture::new();
    let root = f.dirs().grok_home.join("plugins/installed/clax-grok-0.3.0");
    std::fs::create_dir_all(root.join(".grok-plugin")).unwrap();
    std::fs::write(root.join(".grok-plugin/plugin.json"), r#"{"name": "clax-grok", "version": "0.3.0"}"#).unwrap();
    // A plugin of another name is not ours, and session trees are never walked.
    let other = f.dirs().grok_home.join("plugins/x");
    std::fs::create_dir_all(other.join(".grok-plugin")).unwrap();
    std::fs::write(other.join(".grok-plugin/plugin.json"), r#"{"name": "x", "version": "9.9.9"}"#).unwrap();
    assert_eq!(plugin_roots(DoctorAgent::Grok, &f.dirs()), vec![root]);
}

#[test]
fn grok_hooks_are_expected_and_stand_downs_are_not_grok_or_claude_hooks() {
    let home = /* a scratch Home, as the existing hooks_check tests make */;
    assert_eq!(hooks_check(DoctorAgent::Grok, &home)["ok"], false, "no grok hook has run");
    crate::hooklog::append(&home, "2026-10-01T10:00:00Z standdown mode=hook agent=claude host=grok");
    assert_eq!(hooks_check(DoctorAgent::Claude, &home)["ok"], false, "a stand-down is not a Claude Code hook run");
    let c = claude_copy_check(&home);
    assert_eq!(c["ok"], true);
    assert!(c["detail"].as_str().unwrap().contains("grok plugin disable clax"), "{c}");
}

#[test]
fn the_grok_version_check_warns_below_the_minimum() {
    assert_eq!(grok_version_check(Some("grok 1.0.45"))["ok"], true);
    assert_eq!(grok_version_check(Some("grok-build 1.1.0 (abc)"))["ok"], true);
    assert_eq!(grok_version_check(Some("grok 1.0.44"))["ok"], false);
    assert_eq!(grok_version_check(None)["ok"], false);
}
```

Run: `cargo test -p clax-cli doctor_agent`. Expected: FAIL.

- [ ] **Step 2: Implement the doctor**

`DoctorAgent` gains `Grok`. The `harness()` match gets `"grok"`, `display()` gets `"Grok Build"`, `reinstall()` gets `"reinstall with `clax init --agent grok`"`, and `built_skill()` gets `include_str!("../../../../plugins/clax-grok/skills/clax/SKILL.md")`. `plugin_roots` gets:

```rust
        DoctorAgent::Grok => grok_plugin_copies(&dirs.grok_home),
```

```rust
/// Directories under `grok_home`, at most five levels down and never inside
/// `sessions` or `logs`, whose `.grok-plugin/plugin.json` names clax-grok.
/// Grok's install layout is not documented, so this does not assume one.
fn grok_plugin_copies(grok_home: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![(grok_home.to_path_buf(), 0usize)];
    while let Some((d, depth)) = stack.pop() {
        if read_json(&d.join(".grok-plugin/plugin.json")).is_some_and(|m| m["name"] == "clax-grok") {
            out.push(d);
            continue;
        }
        if depth == 5 {
            continue;
        }
        for e in std::fs::read_dir(&d).into_iter().flatten().filter_map(Result::ok) {
            let name = e.file_name();
            if e.file_type().is_ok_and(|t| t.is_dir()) && name != "sessions" && name != "logs" {
                stack.push((e.path(), depth + 1));
            }
        }
    }
    out
}
```

`where_installed` gets `DoctorAgent::Grok => dirs.grok_home.display().to_string()`, and `plugin_check`'s install hint gets `DoctorAgent::Grok => "`clax init --agent grok`"`. `hooks_check` treats Grok like Claude Code: a missing line fails, with `no hook has run (<log> has no grok line); clax-grok's hooks run once the plugin is installed and trusted (clax init installs it with --trust)`. Every other `match` on `DoctorAgent` gets a Grok arm. Where an arm's text names the harness's MCP listing, Grok's is `` (`grok mcp list` shows whether Grok has the clax_grok server) ``. In `mcp_check`'s detail for Grok, add the approval rule: `Grok asks before each tool call; [permission] allow = ["MCPTool(clax_grok__*)"] in ~/.grok/config.toml approves them all, delete included` (**provisional (Q1)**).

Two new checks, run only for `--agent grok`, after `hooks`:

```rust
/// The oldest Grok Build release Clax is checked against
/// (open-questions Q3).
const MIN_GROK: semver::Version = semver::Version::new(1, 0, 45);

/// `grok`: the first `X.Y.Z` in `grok --version`'s first line; failed when
/// there is none, or it is older than [`MIN_GROK`].
pub fn grok_version_check(version_line: Option<&str>) -> Value {
    let Some(line) = version_line else {
        return check("grok", false, "grok is not on PATH, or `grok --version` failed");
    };
    let found = line
        .split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .find_map(|w| semver::Version::parse(w).ok());
    match found {
        Some(v) if v >= MIN_GROK => check("grok", true, line.to_string()),
        Some(v) => check("grok", false, format!("{line}: Clax is checked against Grok Build {MIN_GROK} and later; update Grok (v{v} is older)")),
        None => check("grok", false, format!("{line}: no version found")),
    }
}

/// `claude_copy`: never failed. Whether the Claude Code plugin has stood
/// down in a Grok session, from its `standdown` lines in hooks.log.
pub fn claude_copy_check(home: &Home) -> Value {
    let n = [home.hooks_log_path().with_extension("log.1"), home.hooks_log_path()]
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .map(|t| t.lines().filter(|l| l.contains(" standdown ") && l.contains(" host=grok")).count())
        .sum::<usize>();
    if n == 0 {
        check("claude_copy", true, "the Claude Code plugin has not run in a Grok session")
    } else {
        check("claude_copy", true, format!(
            "the Claude Code plugin is enabled in Grok and stood down {n} time(s): clax-grok acts instead. `grok plugin disable clax` removes its idle clax server"
        ))
    }
}
```

`grok --version` runs through the same `version_line` helper (with its 3 s timeout) that `clax_on_path` uses, on the `grok` found on `PATH`. Add `grok` to the `--agent` list in `doctor.rs` if that file enumerates the agents.

Run: `cargo test -p clax-cli doctor`. Expected: PASS.

- [ ] **Step 3: Write the failing dev tests**

In `scripts/test-dev.sh`:
- add `GROK_HOME="$T/grok-home"` to the exported harness homes and to the `mkdir -p`;
- extend the fake loop to `for h in claude codex grok pi; do`, and have each fake record `grok_home=${GROK_HOME:-}` too;
- add the case:

```bash
devrun grok --resume
line="$(cat "$T/calls")"
tmpdir="$(printf '%s' "$line" | sed -n 's#.*clax=\(.*\)/clax (.*#\1#p')"
if [ "$(wc -l < "$T/calls" | tr -d ' ')" = 1 ] \
    && echo "$line" | grep -qF "grok --resume | clax=$tmpdir/clax (clax 9.9.9-dev) home=$HOME/.clax-dev" \
    && echo "$line" | grep -qF "grok_home=$GROK_HOME" \
    && [ -n "$tmpdir" ] && [ ! -e "$tmpdir" ] && [ -z "$(ls -A "$GROK_HOME")" ] && grep -q 'just install' "$T/out"; then
    pass "just dev grok runs Grok once, with the build on PATH and ~/.clax-dev, its own GROK_HOME untouched and the installed plugin"
else fail "just dev grok ($line; $(cat "$T/err"))"; fi
```

The fakes' recorded-line format gains a field, so update the existing `grep -qF` patterns that match the whole line (the claude and codex cases) by appending it, or make the new field the last one on the line so existing prefix matches still hold. Run: `scripts/test-dev.sh`. Expected: the grok case FAILS (usage error).

- [ ] **Step 4: Implement `just dev grok`**

`scripts/dev.sh`:
- header comment: change `[claude|codex|pi]` to `[claude|codex|grok|pi]`, and add after the codex paragraph:

```bash
#   grok    runs the installed clax-grok plugin with the fresh build, like
#           codex: Grok's TUI has no flag that loads a plugin from a
#           directory for one run. Grok's own home and config are used as
#           they are; to try plugin changes in Grok, run `just install`.
```

- the `case "$harness"` check becomes `claude | codex | grok | pi) ;;`, and the comment there about Grok not being built yet goes;
- usage text: `usage: just dev [claude|codex|grok|pi] [harness arguments...]`;
- the run step:

```bash
    grok)
        echo "clax dev: Grok runs its installed Clax plugin (clax-grok); run \`just install\` to try plugin changes"
        grok "$@"
        ;;
```

- the bare-`just dev` hint: `` (`just dev claude|codex|grok|pi` starts a harness) ``.

`justfile`: in the `dev` recipe's comment, `(claude and pi load this checkout's plugin; codex uses the installed one)` becomes `(claude and pi load this checkout's plugin; codex and grok use the installed one)`.

Run: `scripts/test-dev.sh`. Expected: PASS.

- [ ] **Step 5: Gates and stage**

Run the quality gates. Stage the changed files.

Proposed commit message: `Add clax doctor --agent grok, with the Grok version and the idle Claude Code copy, and just dev grok`

---

### Task 10: A fake Grok runs every combination

**Files:**
- Create: `crates/clax-cli/tests/grok_dedupe.rs`

**Interfaces:**
- Consumes: everything above: the embedded plugins written by `clax init`'s `materialize` (or the repository's `plugins/` directly), the wrapper, `--agent grok`, the guard, the hooks, and `clax feedback follow`.
- Produces: the end-to-end proof for Review Focus 1, and the tier 5 path through a fake monitor.

**What the fake Grok does** (from the Grok source and guide, research §2): given an ordered list of enabled plugin directories, it
1. reads each plugin's `.mcp.json` (a bare map or `mcpServers`) and merges the servers into one map keyed by server name, keeping the **first** definition of a name (`mcp_servers.rs`);
2. spawns each surviving server with `${CLAUDE_PLUGIN_ROOT}` and `${GROK_PLUGIN_ROOT}` expanded in `command`, `args` and `env`, with its whole environment plus the server's `env` plus `GROK_SESSION_ID`, its working directory set to a scratch project, and the test process as its parent (a stand-in for `grok`, so `CLAUDE_PID` is unset);
3. speaks MCP to each: `initialize`, `notifications/initialized`, `tools/list`;
4. runs every enabled plugin's hooks for an event with `sh -c <command>`, with `GROK_HOOK_EVENT`, `GROK_SESSION_ID`, `GROK_PLUGIN_ROOT`, `CLAUDE_PLUGIN_ROOT` (the same value) and `CLAUDE_PROJECT_DIR` set, and Grok's camelCase envelope on stdin;
5. for tier 5, runs a "monitor": spawns a command through `sh -c` with the shell environment (including `GROK_SESSION_ID`) and collects its stdout lines.

- [ ] **Step 1: Write the test harness**

```rust
//! A fake Grok Build loads the Clax plugins the way Grok does: MCP servers
//! merged by name with the first definition kept, every enabled plugin's
//! hooks run, `GROK_SESSION_ID` and `GROK_HOOK_EVENT` set, and Grok's
//! camelCase hook input. Checks that exactly one Clax copy acts in every
//! combination of the Claude Code copy and clax-grok, and that a monitor
//! running `clax feedback follow` is told about a comment once.

use assert_cmd::cargo::cargo_bin;
use serde_json::{Map, Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

const REPO: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
const HARNESS_VARS: &[&str] = &[
    "GROK_SESSION_ID", "GROK_HOOK_EVENT", "GROK_PLUGIN_ROOT", "GROK_HOME",
    "CLAUDE_PID", "CLAUDE_CODE_SESSION_ID", "CLAUDE_PLUGIN_ROOT", "CLAUDE_PROJECT_DIR",
    "CLAX_SESSION_ID", "CLAX_BIN",
];

fn claude_copy() -> PathBuf { Path::new(REPO).join("plugins/claude-code") }
fn grok_copy() -> PathBuf { Path::new(REPO).join("plugins/clax-grok") }

/// A scratch world: a home, a project, a Clax home with a daemon on port 0.
struct World {
    dir: tempfile::TempDir,
}

impl World {
    fn new() -> World {
        let w = World { dir: tempfile::tempdir().unwrap() };
        std::fs::create_dir_all(w.project()).unwrap();
        let ok = w.cmd(&cargo_bin("clax")).args(["--port", "0", "serve"]).stdout(Stdio::null()).status().unwrap();
        assert!(ok.success());
        w
    }
    fn clax_home(&self) -> PathBuf { self.dir.path().join("ax") }
    fn project(&self) -> PathBuf { self.dir.path().join("project") }
    /// A command with the harness environment cleared and the scratch homes set.
    fn cmd(&self, program: &Path) -> Command {
        let mut c = Command::new(program);
        for k in HARNESS_VARS { c.env_remove(k); }
        c.env("HOME", self.dir.path())
            .env("CLAX_HOME", self.clax_home())
            .env("GROK_HOME", self.dir.path().join("grok-home"))
            .env("CLAUDE_CONFIG_DIR", self.dir.path().join("claude"))
            .env("CLAX_BIN", cargo_bin("clax"))
            .env("CLAX_CODEX_BIN", "")
            .env("CLAX_NO_OPEN", "1")
            .env("RUST_LOG", "error")
            .current_dir(self.project());
        c
    }
    fn daemon(&self) -> (String, String) {
        let info: Value = serde_json::from_slice(&std::fs::read(self.clax_home().join("daemon.json")).unwrap()).unwrap();
        (format!("http://127.0.0.1:{}", info["port"]), info["token"].as_str().unwrap().to_string())
    }
    fn api(&self, method: &str, path: &str, body: Option<Value>) -> Value {
        let (base, token) = self.daemon();
        let c = reqwest::blocking::Client::builder().no_proxy().build().unwrap();
        let mut r = c.request(method.parse().unwrap(), format!("{base}{path}")).bearer_auth(token);
        if let Some(b) = body { r = r.json(&b); }
        r.send().unwrap().json().unwrap_or(Value::Null)
    }
    fn live_sessions(&self) -> Vec<Value> {
        self.api("GET", "/api/sessions?live=true", None)["sessions"].as_array().unwrap().clone()
    }
    fn hooks_log(&self) -> String {
        std::fs::read_to_string(self.clax_home().join("logs/hooks.log")).unwrap_or_default()
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = self.cmd(&cargo_bin("clax")).arg("stop").status();
    }
}

/// `${CLAUDE_PLUGIN_ROOT}` and `${GROK_PLUGIN_ROOT}` expanded, as Grok does.
fn expand(s: &str, root: &Path) -> String {
    let r = root.display().to_string();
    s.replace("${CLAUDE_PLUGIN_ROOT}", &r).replace("${GROK_PLUGIN_ROOT}", &r)
}

/// Each plugin's servers, merged by name with the first definition kept.
fn merged_servers(plugins: &[PathBuf]) -> Vec<(String, PathBuf, Value)> {
    let mut out: Vec<(String, PathBuf, Value)> = Vec::new();
    for p in plugins {
        let d: Value = serde_json::from_slice(&std::fs::read(p.join(".mcp.json")).unwrap()).unwrap();
        let servers: Map<String, Value> = d.get("mcpServers").unwrap_or(&d).as_object().unwrap().clone();
        for (name, s) in servers {
            if !out.iter().any(|(n, _, _)| *n == name) {
                out.push((name, p.clone(), s));
            }
        }
    }
    out
}

/// A spawned MCP server, after the handshake.
struct Server {
    name: String,
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    instructions: String,
}

impl Server {
    fn spawn(w: &World, name: &str, root: &Path, def: &Value, session: &str) -> Server {
        let mut c = w.cmd(Path::new(&expand(def["command"].as_str().unwrap(), root)));
        for a in def["args"].as_array().into_iter().flatten() {
            c.arg(expand(a.as_str().unwrap(), root));
        }
        for (k, v) in def["env"].as_object().into_iter().flatten() {
            c.env(k, expand(v.as_str().unwrap(), root));
        }
        let mut child = c
            .env("GROK_SESSION_ID", session)
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
            .spawn().unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut s = Server { name: name.into(), child, stdin, stdout, instructions: String::new() };
        let init = s.request(0, "initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "fake-grok", "version": "1.0.45"}}));
        assert!(init["result"].is_object(), "{name} failed its handshake: {init}");
        s.instructions = init["result"]["instructions"].as_str().unwrap_or_default().to_string();
        writeln!(s.stdin, "{}", json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).unwrap();
        s
    }
    fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        writeln!(self.stdin, "{}", json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})).unwrap();
        loop {
            let mut line = String::new();
            assert!(self.stdout.read_line(&mut line).unwrap() > 0, "{} closed stdout", self.name);
            let v: Value = serde_json::from_str(&line).unwrap();
            if v["id"] == json!(id) { return v; }
        }
    }
    fn tools(&mut self) -> Vec<String> {
        self.request(1, "tools/list", json!({}))["result"]["tools"].as_array().unwrap()
            .iter().map(|t| t["name"].as_str().unwrap().to_string()).collect()
    }
    fn call(&mut self, id: u64, tool: &str, args: Value) -> Value {
        self.request(id, "tools/call", json!({"name": tool, "arguments": args}))["result"].clone()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Runs every enabled plugin's hooks for `event`; each one's stdout.
fn run_hooks(w: &World, plugins: &[PathBuf], event: &str, session: &str, extra: Value) -> Vec<(PathBuf, String)> {
    let mut envelope = json!({
        "hookEventName": event, "hook_event_name": event,
        "sessionId": session, "session_id": session,
        "cwd": w.project(), "workspaceRoot": w.project(),
        "timestamp": "2026-10-01T10:00:00Z", "permissionMode": "ask",
    });
    for (k, v) in extra.as_object().into_iter().flatten() { envelope[k] = v.clone(); }
    let mut out = Vec::new();
    for p in plugins {
        let hooks: Value = serde_json::from_slice(&std::fs::read(p.join("hooks/hooks.json")).unwrap()).unwrap();
        for entry in hooks["hooks"][event].as_array().into_iter().flatten() {
            for h in entry["hooks"].as_array().into_iter().flatten() {
                let mut child = w.cmd(Path::new("/bin/sh"))
                    .args(["-c", h["command"].as_str().unwrap()])
                    .env("GROK_HOOK_EVENT", event).env("GROK_SESSION_ID", session)
                    .env("GROK_PLUGIN_ROOT", p).env("CLAUDE_PLUGIN_ROOT", p)
                    .env("CLAUDE_PROJECT_DIR", w.project())
                    .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
                    .spawn().unwrap();
                child.stdin.take().unwrap().write_all(envelope.to_string().as_bytes()).unwrap();
                let o = child.wait_with_output().unwrap();
                assert!(o.status.success(), "{} {event} hook exited {:?}", p.display(), o.status);
                out.push((p.clone(), String::from_utf8(o.stdout).unwrap()));
            }
        }
    }
    out
}

/// One Grok session with `plugins` enabled, in that discovery order.
struct Session {
    servers: Vec<Server>,
    id: String,
}

fn start(w: &World, plugins: &[PathBuf], id: &str) -> Session {
    let servers = merged_servers(plugins).into_iter()
        .map(|(name, root, def)| Server::spawn(w, &name, &root, &def, id))
        .collect();
    let s = Session { servers, id: id.into() };
    run_hooks(w, plugins, "SessionStart", id, json!({"source": "startup"}));
    s
}

/// The servers that act (offer more than `status`), by name.
fn acting(s: &mut Session) -> Vec<String> {
    s.servers.iter_mut().filter_map(|srv| (srv.tools().len() > 1).then(|| srv.name.clone())).collect()
}
```

- [ ] **Step 2: The combinations**

```rust
#[test]
fn both_copies_enabled_one_acts_in_either_discovery_order() {
    for order in [[claude_copy(), grok_copy()], [grok_copy(), claude_copy()]] {
        let w = World::new();
        let mut s = start(&w, &order, "019a-both");
        let mut names: Vec<_> = s.servers.iter().map(|x| x.name.clone()).collect();
        names.sort();
        assert_eq!(names, ["clax", "clax_grok"], "distinct names: both load");
        assert_eq!(acting(&mut s), ["clax_grok"]);
        let idle = s.servers.iter_mut().find(|x| x.name == "clax").unwrap();
        assert_eq!(idle.tools(), ["status"]);
        let r = idle.call(2, "status", json!({}));
        assert_ne!(r["isError"], json!(true));
        assert!(r["content"][0]["text"].as_str().unwrap().contains("clax_grok__publish"));
        let live = w.live_sessions();
        assert_eq!(live.len(), 1, "{live:?}");
        assert_eq!((live[0]["harness"].as_str(), live[0]["harness_session_id"].as_str()), (Some("grok"), Some("019a-both")));
        let log = w.hooks_log();
        assert_eq!(log.matches(" hook agent=grok event=session-start ").count(), 1, "{log}");
        assert!(!log.contains(" hook agent=claude "), "{log}");
        assert!(log.contains(" standdown mode=hook agent=claude host=grok"), "{log}");
        assert!(log.contains(" standdown mode=mcp agent=claude host=grok"), "{log}");
    }
}

#[test]
fn only_clax_grok_acts_alone() {
    let w = World::new();
    let mut s = start(&w, &[grok_copy()], "019a-grok");
    assert_eq!(acting(&mut s), ["clax_grok"]);
    assert_eq!(w.live_sessions().len(), 1);
    assert!(!w.hooks_log().contains("standdown"));
}

#[test]
fn only_the_claude_copy_says_how_to_get_clax_grok_and_acts_nowhere() {
    let w = World::new();
    let mut s = start(&w, &[claude_copy()], "019a-claude");
    assert!(acting(&mut s).is_empty());
    assert_eq!(s.servers.len(), 1);
    assert!(s.servers[0].instructions.contains("clax init --agent grok"));
    assert!(w.live_sessions().is_empty(), "no session is registered");
    let out = run_hooks(&w, &[claude_copy()], "Stop", "019a-claude", json!({"reason": "end_turn", "stopHookActive": false}));
    assert!(out.iter().all(|(_, o)| o.is_empty()));
}

#[test]
fn neither_copy_means_no_clax() {
    let w = World::new();
    let s = start(&w, &[], "019a-none");
    assert!(s.servers.is_empty());
    assert!(w.live_sessions().is_empty());
}
```

- [ ] **Step 3: One Stop hand-over, and tier 5 through a fake monitor**

```rust
#[test]
fn with_both_enabled_a_comment_is_announced_once_and_handed_over_once() {
    let w = World::new();
    let both = [claude_copy(), grok_copy()];
    let mut s = start(&w, &both, "019a-loop");
    let grok = s.servers.iter_mut().find(|x| x.name == "clax_grok").unwrap();
    // Publishing watches the artifact with replies armed.
    let r = grok.call(10, "publish", json!({"title": "Loop", "html": "<title>Loop</title><p id=x>hi</p>"}));
    let aid = /* the artifact ID from r's JSON text */;
    // The monitor the skill starts: the command line from the skill, run through sh with the session's environment.
    let mut monitor = w.cmd(Path::new("/bin/sh"))
        .args(["-c", &format!("\"{}\" feedback follow --agent grok --harness-session 019a-loop --poll-secs 2 --grace-secs 1", cargo_bin("clax").display())])
        .env("GROK_SESSION_ID", "019a-loop")
        .stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().unwrap();
    // A viewer comments and sends it to the agent (the REST calls the shell makes; copy them from the server tests).
    let tid = /* create a thread on aid and POST its send-to-agent */;
    // Within 10 s the monitor prints exactly one line naming tid; read it on a thread with a timeout.
    // Stop: exactly one block, from clax-grok; the Claude copy's Stop prints nothing.
    let out = run_hooks(&w, &both, "Stop", "019a-loop", json!({"reason": "end_turn", "stopHookActive": false}));
    let blocks: Vec<_> = out.iter().filter(|(_, o)| o.contains("\"decision\":\"block\"")).collect();
    assert_eq!(blocks.len(), 1, "{out:?}");
    assert_eq!(blocks[0].0, grok_copy());
    assert!(blocks[0].1.contains(&tid));
    // The next Stop (stopHookActive) blocks nothing; the monitor prints nothing more within 4 s.
    let again = run_hooks(&w, &both, "Stop", "019a-loop", json!({"reason": "end_turn", "stopHookActive": true}));
    assert!(again.iter().all(|(_, o)| o.is_empty()), "{again:?}");
    // The session-end Stop does nothing either.
    let shutdown = run_hooks(&w, &both, "Stop", "019a-loop", json!({"reason": "shutdown"}));
    assert!(shutdown.iter().all(|(_, o)| o.is_empty()));
    // SessionEnd ends the row; the monitor then exits 0 within 5 s and wrote nothing to stderr.
    run_hooks(&w, &both, "SessionEnd", "019a-loop", json!({}));
    assert!(w.live_sessions().is_empty());
    let _ = monitor.kill();
}
```

Fill in the marked lines: the artifact ID from the `publish` result's JSON text, and the thread creation and send-to-agent REST calls copied from `crates/clax-server/tests/api_feedback.rs`. Read the monitor's stdout on a thread that feeds a channel, so each wait has a timeout, and assert the monitor's exit status after `SessionEnd`.

Run: `cargo test -p clax-cli --test grok_dedupe`. Expected: PASS once Tasks 2–7 are in. If a case fails, the failure is in the layer that case names. Fix that layer, not this test, unless the test's fake Grok contradicts the research.

- [ ] **Step 4: Gates and stage**

Run the quality gates. Stage the test.

Proposed commit message: `Test with a fake Grok that exactly one Clax copy acts in every combination, and that a monitor announces a comment once`

---

### Task 11: `scripts/smoke-grok.sh`, READMEs and follow-ups

**Files:**
- Create: `scripts/smoke-grok.sh` (mode 0755)
- Modify: `README.md`, `plugins/claude-code/README.md` (a short "In Grok Build" note), `docs/follow-ups.md`
- Modify: `docs/contract.md` (only the "Other commands and scripts" list)

**Interfaces:**
- Consumes: everything above.
- Produces: the owner's live check, which answers open-questions Q4. **Agents never run it.**

- [ ] **Step 1: The smoke script**

`scripts/smoke-grok.sh`, modelled on `scripts/smoke-codex.sh`. It is a manual end-to-end check of the Grok path. It is not a quality gate: it runs real `grok -p` sessions, which call a model and need `XAI_API_KEY` (or a logged-in Grok).

```bash
#!/usr/bin/env bash
# Manual end-to-end check of the Grok Build path. Not a quality gate: it runs
# real `grok -p` sessions, which call a model and need XAI_API_KEY (or Grok's
# own login, copied into the scratch GROK_HOME for the run and deleted on
# exit). The owner runs it; agents never do.
#
# Everything runs in a scratch root: HOME, GROK_HOME, CLAX_HOME and a free
# port, so ~/.grok, ~/.claude, ~/.clax and ports 7480/7481 are never touched.
# CLAX_BIN is this working tree's target/debug/clax.
#
# It checks, and prints a PASS/FAIL line for each (open-questions.md Q4):
#   a  clax init --agent grok installs clax-grok (`grok plugin install <dir> --trust`)
#      and uninit removes it; `grok plugin list --json` names its source
#   c  a session registers as harness grok with GROK_SESSION_ID; the hooks run
#   d  a sent comment blocks Stop once at the end of a turn (stopHookActive on the next)
#   e  with the Claude Code copy also enabled (scratch ~/.claude/plugins), only
#      clax_grok acts: one live session, standdown lines for the Claude copy
#   m  `clax feedback follow` under the monitor tool prints one line per comment
#      (interactive: the script prints the steps for an idle TUI and waits)
#   v  `grok --version`, recorded for docs/contract.md
# Usage: scripts/smoke-grok.sh [scratch-dir]
set -euo pipefail
```

Write the body:
- **Setup:** choose a free port with `python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])'` and refuse 7480 and 7481. Write it into `$CLAX_HOME/config.toml` as `[serve] port`. `cargo build -q -p clax-cli --bin clax`. Export `HOME`, `GROK_HOME`, `CLAX_HOME` and `CLAX_BIN` to the scratch root, with `PATH` keeping the real `grok`. If `$REAL_HOME/.grok/auth.json` exists and `XAI_API_KEY` is unset, copy it into the scratch `GROK_HOME` and delete the copy on exit, as `smoke-codex.sh` does for Codex.
- **a:** `"$CLAX_BIN" init --agent grok --json`. Expect `registered`, then `grok plugin list --json` naming the marketplace path.
- **c, d:** `grok -p "Publish a one-line page titled Smoke with clax, then stop." --always-approve`. Then query `GET /api/sessions` with the token: there is a `grok` row with a non-empty `harness_session_id`, and `hooks.log` has `agent=grok` session-start lines. Then post a comment and send it to the agent over REST, and run `grok -p --resume <ID> "Say done." --always-approve`. Confirm the turn received the payload: the transcript under `$GROK_HOME/sessions/` contains the comment text, and the feedback row is delivered with tier `stop_hook`.
- **e:** write a scratch `$HOME/.claude/plugins/installed_plugins.json` that points `clax@clax` at `<marketplace>/plugins/claude-code`, then run `grok plugin enable clax`. Repeat c. Expect one live `grok` row, no `claude` row, and `standdown` lines in `hooks.log`. Also record whether `search_tool` lists both `clax__status` and `clax_grok__publish`: ask the model to list the tools whose names start with `clax`, and print its answer.
- **m:** print instructions for the owner to run in a terminal: start `grok` (the TUI) with this environment, ask it to publish and follow the skill's live-feedback step, then press Enter in the script. The script then sends a comment over REST and asks the owner to confirm, y or n, that the idle TUI woke with the `[clax] New comment on …` line.
- **v:** print `grok --version`.
- **Report:** print the table, and the line to paste into `docs/contract.md`'s measurement sentence.

Run `bash -n scripts/smoke-grok.sh` and `shellcheck scripts/smoke-grok.sh`, if it is installed. **Do not run the script.**

- [ ] **Step 2: Docs**

- `README.md`: Grok Build in the harness list. One paragraph in Install: `clax init` installs clax-grok when `grok` is on `PATH`, including when Grok is the only harness. Add `just dev grok` to the dev list.
- `plugins/claude-code/README.md`: a short "In Grok Build" section. Grok discovers this plugin; enabled there, it stands down and points at clax-grok.
- `docs/contract.md`, "Other commands and scripts": add `clax feedback follow` (one line: what it prints, that it delivers nothing, and its exit rule) and `scripts/smoke-grok.sh` (manual, the owner runs it, scratch homes and port).
- `docs/follow-ups.md`: replace the Grok Build entry with:
  - the live checks still owed (open-questions Q4: a–g, plus the monitor wake);
  - the provisional answers Q1–Q3 and Q5, and what changes if the owner decides otherwise;
  - the Claude Code fallback for `clax feedback follow` (a `--once` flag, run as a background command whose exit wakes the session);
  - a PostToolUse hand-over for Claude Code and Grok together (Q5);
  - `scripts/verify-harnesses.sh` does not cover `grok` yet.

Run `scripts/test-plugins.sh` (the name gate and the README tool lists). Expected: pass.

- [ ] **Step 3: Gates and stage**

Run the quality gates. Stage the changed files.

Proposed commit message: `Add the owner's Grok smoke script and document Grok Build in the READMEs and follow-ups`

---

## Steps for the owner

These need a real Grok and are not run by agents.

1. `just install` (it runs `clax init`, which now installs clax-grok). `clax doctor --agent grok`: `plugin`, `skill` and `grok` pass.
2. `scripts/smoke-grok.sh`. Each FAIL names the provisional step to change (open-questions Q4). Paste its measured-version line into `docs/contract.md` and drop "not yet run live".
3. In a real Grok TUI session: publish a page, check that the agent starts the monitor (`/tasks` or Grok's background-task list shows `clax feedback follow`), leave the session idle, comment in the browser and press **Send to agent**. The session should wake with `[clax] New comment on …`, call `clax_grok__comments_read`, and reply.
4. If you enable the Claude Code plugin in Grok (`grok plugin enable clax`), `clax doctor --agent grok`'s `claude_copy` check reports it. Only `clax_grok__*` tools act.
5. Decide Q1, Q2, Q3 and Q5 in `.superpowers/sdd/2026-10-01-grok/open-questions.md`, or accept the recommendations.
