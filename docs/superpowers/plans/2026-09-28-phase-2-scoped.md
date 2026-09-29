# Artifax Phase 2: MCP Shim and Harness Plugins — Scoped Plan

> **Status:** scoped plan. Expand to step-level tasks (tests, code, commits) with the writing-plans skill when phase 1 has shipped, against the phase 1 code as it actually exists. The task boundaries, interfaces, and acceptance criteria below are the commitments; file-level code is not.
>
> **For agentic workers:** when expanded, use superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** Any agent in Claude Code, Codex, or Pi can publish, read, list, and update artifacts through MCP, with a stable session identity the daemon can address later.

**Architecture:** A stdio MCP shim (`artifax mcp --agent <x>`) per session proxies tools to the daemon over HTTP with the token and registers a session record. Hooks (`artifax hook --agent <x> <event>`) register the harness's own session ID. The daemon also serves the same tools over HTTP MCP. Three plugin directories package the shim, hooks, skills, and commands; an installer script finds or downloads the binary.

**Tech Stack:** `rmcp` (Rust MCP SDK) for stdio and streamable-HTTP transports; `reqwest`; harness JSON protocols per `clash/clash-codex/hooks.toml`, `clash/clash-pi/extensions/clash.ts`, and Claude Code's hook and plugin formats; bash for `ensure-artifax.sh`; TypeScript for the Pi extension.

**Spec:** `docs/superpowers/specs/2026-09-28-artifax-design.md` §3, §4, §11, §12 (artifact tools and `status`), §13, §17 "Phase 2", §18 (Codex manifest, Codex marketplace, Pi checklist, Claude session ID).

**Depends on:** phase 1 REST API, `Client`, `DaemonInfo`, `Home`.

## Global constraints (in addition to phase 1's)

- Tool names are exactly: `publish`, `read`, `list`, `delete`, `open`, `pin`, `unpin`, `asset_upload`, `status`. Claude Code surfaces them as `mcp__artifax__<name>`.
- Every tool result is a JSON object rendered as text; URLs in results are browser URLs (`http://localhost:<port>/a/<id>`).
- Hooks finish within 5 s and exit 0 with no output when the daemon is unreachable. They never block the harness because artifax is down.
- `ensure-artifax.sh` mirrors `toolpath/plugins/claude-code/scripts/ensure-path.sh`: `ARTIFAX_BIN`, `ARTIFAX_INSTALL_DIR`, checksum-verified release download, `exec` subcommand, no variables on command lines inside slash commands.
- The Pi adapter is unverified on this machine; its tasks start with the re-check list.

## Pre-flight checks (do these first, record answers in this file)

1. Codex `.codex-plugin/plugin.json`: does it accept an MCP servers key or a hooks key? Inspect `codex plugin --help`, `codex mcp add --help`, and any schema Codex 0.158 ships. Outcome decides Task 7's shape.
2. Codex marketplace manifest format for `codex plugin marketplace add <path|url>`. Outcome decides Task 7's marketplace file.
3. Whether Claude Code sets a session ID in the environment of MCP server processes it spawns. Outcome decides whether Task 4's parent-PID join is primary or fallback.
4. Pi installed? If not, install it, then run the seven-item checklist in spec §13 and write the answers here before Task 8.

## Tasks

### Task 1: Storage for sessions
- Migration 2: `sessions(id, harness, harness_session_id, cwd, pid, parent_pid, started_at, last_seen_at, ended_at)`; `versions.session_id` and `artifacts.owner_session_id` start being populated.
- `Store::register_session(RegisterSession) -> Session`, `Store::join_session_by_parent(harness, parent_pid, harness_session_id) -> Option<Session>`, `Store::heartbeat(id)`, `Store::end_session(id)`, `Store::list_sessions(live_only)`, `Store::reap_sessions(idle_for: Duration)` (no heartbeat for 5 min and dead PID → ended).
- Acceptance: unit tests for each; a shim record and a hook record with the same `(harness, parent_pid)` become one session; the reaper ends a dead-PID session and leaves a live one.

### Task 2: Session and MCP-over-HTTP routes
- `POST /api/sessions` (W), `PATCH /api/sessions/{id}` (W: heartbeat or `ended: true`), `GET /api/sessions`, `POST /api/sessions/{id}/join` (W, from hooks: `{harness_session_id}` by parent PID).
- Publish routes accept `X-Artifax-Session: <id>` and record it on the version and, on create, as `owner_session_id`.
- `/mcp` streamable-HTTP endpoint (rmcp) exposing the same tool set as Task 3, authenticated by the bearer token.
- Acceptance: integration tests; a publish with the header yields `owner_session_id` set; HTTP MCP `tools/list` matches the shim's.

### Task 3: `artifax-mcp` crate: the stdio shim
- `artifax mcp --agent <claude|codex|pi>`: ensure daemon (reuse `Client::connect`), register session `{harness, cwd, pid, parent_pid}`, heartbeat every 60 s, end the session on stdin EOF.
- Tools with JSON schemas mirroring the spec §12 artifact set. `publish` accepts `file_path` or `html`, `files` map (paths to local files or `{content, encoding}`), `url`/`id`, `if_version`, `title`, `description`, `icon`, `label`, `capabilities`. `read` returns the current `index.html` (or a named file) with size caps and a "truncated" flag. `list` returns the gallery order. `open` runs the platform opener on the daemon host. `asset_upload` posts multipart from local paths. `status` returns daemon URL, version, this session's ID and harness, and (phase 3) watches.
- Every result carries a trailing feedback block slot (empty in phase 2) so phase 3 adds tier 1 without changing shapes.
- Acceptance: spawn the shim against a test daemon and drive it with an MCP client over stdio; assert `tools/list`, a publish → read round trip, `if_version` conflict surfaced as a tool error with the current version, and that the session record ends when the client closes stdin.

### Task 4: `artifax-hooks` crate and `artifax hook`
- `artifax hook --agent claude|codex|pi session-start|session-end` reading the harness's stdin JSON (fixtures captured from each harness; Claude Code includes `session_id`, `cwd`, `hook_event_name`; Codex per `clash-codex`; Pi per `clash-pi`).
- `session-start`: join by parent PID or by `ARTIFAX_SESSION` env if set, print for Claude Code a `hookSpecificOutput.additionalContext` line naming the daemon URL; for others print nothing.
- `session-end`: end the session.
- Acceptance: fixture-driven golden tests per harness; unreachable daemon → exit 0 within 3 s with empty stdout.

### Task 5: `scripts/ensure-artifax.sh` and release workflow
- Adapt `ensure-path.sh` (env names, repo `empathic/artifax`, min version `0.1.0`, binary `artifax`).
- `.github/workflows/release.yml`: tag → build `aarch64-apple-darwin`, `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-gnu`, ship `artifax-<target>.tar.gz` + `.sha256`, with `web/dist` built and embedded first.
- Acceptance: bats or bash tests for resolution order (`ARTIFAX_BIN` wins, foreign `artifax` on PATH is skipped, fallback dir chosen when shadowed); a dry-run of the release job on a branch.

### Task 6: Claude Code plugin
- `plugins/claude-code/.claude-plugin/plugin.json`, `.mcp.json` pointing at `ensure-artifax.sh exec mcp --agent claude`, `hooks/hooks.json` with `SessionStart` and `SessionEnd` (Stop and UserPromptSubmit arrive in phase 3), `skills/artifax/SKILL.md` (page contract, publish workflow, tool reference, what phase 2 does not yet do), `commands/open.md`, `commands/serve.md`, `commands/doctor.md`, `README.md`. Root `.claude-plugin/marketplace.json`.
- Acceptance: `claude --plugin-dir plugins/claude-code` (or the local marketplace add) loads the plugin; a scripted session publishes and reads an artifact; `/artifax:open` opens it; hooks appear in `/hooks`.

### Task 7: Codex plugin
- `plugins/codex/.codex-plugin/plugin.json` with `skills`, `interface`; `skills/artifax/SKILL.md`; `skills/artifax-setup/SKILL.md` + `scripts/setup.sh` that runs `codex mcp add artifax -- <ensure path> exec mcp --agent codex` and appends `[hooks.session_start]`/`[hooks.stop]` entries idempotently; `config.snippet.toml`, `hooks.snippet.toml`. Marketplace file per pre-flight 2. If pre-flight 1 shows declarative support, replace the setup skill with the manifest keys and keep the snippets for manual installs.
- Acceptance: `setup.sh` is idempotent (run twice, one entry); a Codex session lists the artifax tools (`codex mcp list`) and publishes an artifact; hook trust prompt is documented.

### Task 8: Pi extension (unverified on this machine)
- Run the spec §13 checklist first; record results at the top of this file.
- `plugins/pi/package.json` (`@empathic/artifax-pi`, `pi.extensions`), `extensions/artifax.ts`: `session_start` → `artifax hook --agent pi session-start` with `event.sessionId`; tools via MCP registration if checklist item 3 is yes, else via the extension tool API wrapping `artifax <cmd> --json` (items 2); `tool_result` hook is a no-op until phase 3.
- Acceptance: vitest with a mocked `ExtensionAPI` covering event wiring and argument shapes; if Pi is installed, a manual publish from a Pi session.

### Task 9: Docs and gates
- `docs/contract.md` first version: tool reference and page contract. Gates: add `artifax-mcp` and `artifax-hooks` tests, shell tests for the installer, plugin smoke test for Claude Code.

## Ship criteria
An agent in Claude Code and in Codex publishes and updates an artifact through MCP and gets a browser URL; the daemon shows the publishing session on the card; the Pi extension passes its mocked tests, and its unverified items are recorded with answers or marked still open.
