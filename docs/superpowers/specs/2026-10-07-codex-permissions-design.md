# Codex permissions: no approval prompts mid-task

Date: 2026-10-07
Status: implemented on branch codex-permissions

## 1. Problem and goal

Codex stalls on approval prompts while using the Clax plugin: it asks before
calling a Clax MCP tool, often while the person is away in the browser, and
`codex exec` refuses such calls outright. Shell use of the `clax` command
inside Codex's sandbox fails or asks too.

Goal: once the person installs the plugin and approves once, at setup time,
Codex never stops on a Clax permission prompt mid-task. Nothing is granted
silently.

Everything below rests on experiments against codex-cli 0.160.1 (§2), each
in a throwaway `CODEX_HOME`. Decisions cite them.

## 2. Findings

### 2.1 Approval modes

A stdio MCP server with seven tools that differ only in annotations logged
every `tools/call` it received, so "ran" means the server saw the call.
`codex exec` always runs with approval policy `never` (even given `-c
approval_policy="on-request"`), so a call that needs approval fails with
`MCP tool call requires approval, but approval policy is never` and never
reaches the server: an exact detector of "would ask".

The binary's approval enum has four values: `auto`, `prompt`, `writes`,
`approve`. Set per server (`default_tools_approval_mode`) or per tool
(`tools.<name>.approval_mode`); unset means `auto`.

| tool annotations | auto | writes | prompt | approve |
|---|---|---|---|---|
| none | asks | asks | asks | runs |
| readOnlyHint true (with or without openWorldHint false) | runs | runs | asks | runs |
| readOnly false, destructive false, openWorld false | runs | asks | asks | runs |
| readOnly false, destructive true, openWorld false | asks | asks | asks | runs |
| readOnly false, destructive false, openWorld true | asks | asks | asks | runs |

So `auto` asks iff `destructiveHint == true`, or the tool is not read-only and
not `openWorldHint: false`. A tool without annotations asks; this is why every
Clax tool asked on first use.

The TUI (approval policy on-request), driven through tmux, showed the same
decision as a dialog: `Allow the fake MCP server to run tool "t_none"?` with
Allow / Allow for this session / Always allow / Cancel. **Always allow**
wrote `[mcp_servers.fake.tools.t_none] approval_mode = "approve"`: the
source of the owner's three per-tool `approve` entries.

### 2.2 Precedence

A tool's own `approval_mode` overrides the server default both ways (server
`approve` + tool `prompt` asks for that tool; no server default + tool
`approve` runs it). The server default covers all of its tools.

### 2.3 Plugin-shipped defaults

Codex honors `default_tools_approval_mode` and `tools.<name>.approval_mode`
in a plugin's `.mcp.json`, with no notice at install. The person's
`[plugins."<plugin>@<marketplace>".mcp_servers.<server>]` settings in
`config.toml` override them.

`codex plugin remove <plugin>` deletes the plugin's whole table, including
the person's `mcp_servers` approval settings; adding the plugin again does
not restore them. `clax init` removes and re-adds the plugin on every run.

### 2.4 The `clax` command in the shell sandbox

The default sandbox is `workspace-write` with network off. From a sandboxed
shell, `clax list` and `clax publish` fail with `acquiring daemon lock:
Operation not permitted`: the connection to the daemon on 127.0.0.1 is
blocked, so the client tries to start a daemon, which needs writes to the
Clax home. With `sandbox_workspace_write.network_access = true` both succeed
against a running daemon; `writable_roots = [<clax home>]` alone does not
help. `network_access` opens all outbound network for every shell command; no
loopback-only setting was found in this version.

MCP servers run outside the shell sandbox: a server wrote to a directory the
sandboxed shell could not.

### 2.5 Hook output

A plugin `SessionStart` hook's `systemMessage` is shown to the person in the
TUI transcript (`↳ Hook · <message>`). Untrusted hooks do not run.

## 3. Design

### 3.1 Annotations (§2.1)

Every Clax MCP tool is annotated, with `openWorldHint: false` on all (each
acts on this machine's Clax home and daemon):

- read-only: `read`, `list`, `status`, `comments_read`, `wait_for_feedback`,
  `db_get`, `db_list`, `db_query`. Reading acknowledges delivered comments,
  as every tool result does; that is delivery bookkeeping, not a change to
  artifacts or data.
- destructive: `delete`, `db_set`, `db_update`, `db_delete`,
  `db_str_replace`, `db_batch`. They replace or remove data no version keeps
  (page documents have no history).
- not destructive: `publish` (keeps every earlier version), `asset_upload`,
  `comments_reply`, `comments_resolve`, `watch`, `working`, `pin`, `unpin`,
  `open`.

Under `auto`, Codex then runs 17 of the 23 tools without asking and with no
setting at all. The stand-down server's `status` is read-only too.

The contract fixture (`plugins/pi/test/fixtures/contract.json`) lists each
tool's annotations; a `clax-mcp` test checks names, descriptions and
annotations against it.

### 3.2 No approval settings in the plugin (§2.3)

Codex would apply them silently. The six destructive tools are the person's
call, made in their own config.

### 3.3 Setup: `clax init` (§2.2, §2.3)

After registering the Codex plugin, `clax init`:

1. Puts back the person's `[plugins."clax@clax"]` settings that its own
   `codex plugin remove` deleted (read before removal; keys the re-added
   table lacks are copied back, recursively; nothing present is changed).
2. Works out which tools Codex would ask about, from the effective mode
   (tool, else server, else `auto`) and the annotations, skipping tools
   `enabled_tools`/`disabled_tools` hide and a disabled plugin.
3. For those without an `approval_mode` of their own, prints the exact lines
   (one `[plugins."clax@clax".mcp_servers.clax.tools.<tool>]` table with
   `approval_mode = "approve"` each) and asks on the terminal; `--yes` adds
   them without asking; with neither, nothing is added and the result says
   how. Per-tool entries rather than a server default: they grant exactly the
   tools the person saw, and a future tool is assessed afresh.

Edits use `toml_edit`: comments and layout are kept, a symbolic link is
followed, the file's mode is kept, the write is atomic, nothing is removed or
changed, and adding again is a no-op. A failure here is reported under the
Codex entry's `approvals` and never fails `init`.

### 3.4 Reporting

- `clax doctor --agent codex` adds `codex_approvals`: passes when no tool
  asks; warns naming `clax init --agent codex` when tools would ask; names
  tools whose own `approval_mode` makes them ask; fails when the config
  cannot be read or parsed.
- The `SessionStart` hook (§2.5) shows the person a one-line
  `systemMessage` when tools would ask: which tools and `clax init --agent
  codex` (or, with no `clax` on `PATH`, the setting to add). Once per set of
  tools, recorded in `<clax home>/run/codex-approvals-notice`; only when the
  plugin is registered in Codex's config; it never writes that config.

### 3.5 The shell (§2.4)

The Codex skill tells Codex to use the MCP tools and never the `clax` command,
since the tools run outside the sandbox. Clax does not set `network_access`:
it would open all network access to every shell command.

## 4. Rejected

- Server-level `default_tools_approval_mode = "approve"` written by setup:
  grants tools the person never saw, including future ones. Documented as the
  person's own option.
- Approvals in the plugin's `.mcp.json`: silent (§2.3).
- `network_access = true` or `writable_roots` from setup: widens the sandbox
  for everything to serve a path the MCP tools already cover.
- Writing Codex's config from the hook: not a moment the person chose.

## 5. Not verified

- Codex versions other than 0.160.1. Older ones may not read annotations;
  the doctor check assumes 0.160.1's rules.
- A profile layer (`codex -p <name>`, `$CODEX_HOME/<name>.config.toml`) is
  not read by the assessment.
- The interactive confirmation itself (a terminal) is exercised only by hand;
  tests cover `--yes` and the no-terminal path.
