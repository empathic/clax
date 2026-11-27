# Clax for Antigravity: a plugin for Google's Antigravity CLI

Status: design for the owner's review (2026-11-26). Every statement about
Antigravity in section 2 was observed on `agy` 1.5.18 on macOS unless it is
marked as documented only; section 12 lists what was not verified.

## 1. Intent

A person who works in Google's Antigravity CLI (`agy`) gets the same Clax
loop the other harnesses have: the agent publishes pages with the Clax
tools, the person comments in the browser, and the comments reach the
conversation, including one that is sitting idle. Antigravity becomes the
fifth harness beside Claude Code (`plugins/claude-code`), Codex
(`plugins/clax`), Grok Build (`plugins/clax-grok`) and Pi (`plugins/pi`).

The plugin is native to Antigravity. `agy plugin import claude` can convert
the Claude Code plugin, but the converted copy still runs `--agent claude`:
its MCP server acts as a Claude Code session and its hooks do nothing
(section 5), so it is not the product; section 6.8 makes that copy stand
down on purpose and say so.

## 2. How Antigravity works (verified on agy 1.5.18)

Each fact was observed with a throwaway plugin whose MCP server and hooks
logged what they received, in one-shot (`agy -p`) and interactive runs.
"Documented" means the fact comes from https://antigravity.google/docs or
the docs bundled with the CLI and was not run.

### 2.1 Plugins

- A plugin is a directory with a manifest at `plugin.json` or
  `.antigravity-plugin/plugin.json` (both load), and optional
  `mcp_config.json`, `hooks.json` (at the plugin root, never
  `hooks/hooks.json`), `rules/`, `skills/<name>/SKILL.md`. A `commands/`
  directory is not loaded (documented).
- `agy plugin install <dir>` copies the directory to
  `~/.gemini/config/plugins/<name>/`, keeps executable bits, and on a
  second install replaces the whole directory. `agy plugin uninstall
  <name>` removes it. `agy plugin list --json` prints `name`, `enabled`,
  `version`, `source`, `path` and the component names; `path` is the
  installed copy, not the directory it was installed from.
- `agy plugin validate <dir> [--strict] [--json]` checks a plugin the way
  the loader reads it, without installing.
- A plugin in `<workspace>/.agents/plugins/<name>/` loads with no install,
  but its MCP server starts only when the first prompt is sent, and the
  model has no MCP tools on that first turn. An installed plugin's server
  starts when `agy` launches and its tools work on the first turn.
- Installed plugins get `PLUGIN_ROOT=~/.gemini/config/plugins/<name>` and
  `PLUGIN_DATA=~/.gemini/antigravity-cli/plugin_data/<name>`. The data
  directory survives a reinstall.
- A plugin's `rules/AGENTS.md` (no frontmatter) and any `rules/*.md` with
  `trigger: always_on` are in the model's context on every turn. Skill
  names and descriptions are listed; a skill body loads on demand.

### 2.2 MCP servers

- One stdio server process per `agy` process. The same process served the
  first conversation, a `/new` conversation and a subagent. Its parent is
  the `agy` process; its working directory is the plugin directory; it
  inherits the whole environment `agy` was started with, plus
  `PLUGIN_ROOT` and `PLUGIN_DATA`. It gets no `ANTIGRAVITY_*` variable.
- `${PLUGIN_ROOT}` is expanded in `args`.
- The client opens with `server/discover` (protocol 2026-07-28) and falls
  back to `initialize` (2025-11-25) when refused. `clientInfo.name` is
  `antigravity-client`. It advertises `elicitation` and `roots`, not
  sampling.
- The installed shim (`clax mcp --agent codex`, clax 0.7.1, behind a
  logging proxy, against a scratch daemon) accepts the discover, so
  Antigravity runs it on 2026-07-28: `tools/list`, `status` and a
  background `wait_for_feedback` with `timeout_s: 5` all worked, and the
  conversation keys below were present beside the protocol's own `_meta`
  keys. The probe plugin refused the discover and ran on 2025-11-25; the
  facts in this section hold on both except where section 12 says a fact
  was seen on one only.
- Every `tools/call` carries `_meta` with
  `antigravity.google/conversation_id`,
  `antigravity.google/root_conversation_id`,
  `antigravity.google/artifacts_dir`, and for a subagent
  `antigravity.google/parent_conversation_id`. Appendix A has examples.
- The server is named `<plugin name>_<server key>` at run time. For a
  plugin `clax-antigravity` with server key `clax` it is
  `clax-antigravity_clax`.
- Tools are lazy by default: the system prompt lists only their names, and
  the model reads `~/.gemini/antigravity-cli/mcp/<server>/<tool>.json`
  before calling one through the generic `call_mcp_tool` tool
  (`ServerName`, `ToolName`, `Arguments`). A tool marked `"eager": true`
  in `mcp_config.json` has its description and parameters in context and
  is called under its own name, `mcp_<server>_<tool>`, for example
  `mcp_clax-antigravity_clax_publish`.
- The server's MCP `instructions` are written to
  `~/.gemini/antigravity-cli/mcp/<server>/instructions.md` and are not in
  the model's context.
- `timeoutSeconds` on a server replaces both the per-call limit (default
  180 s) and the connect limit (default 300 s); a negative value turns both
  off (a 200 s call completed under `-1`). When a call passes the limit the
  client sends `notifications/cancelled` and the call fails.
- A one-shot run waits for the server to connect before its first model
  call (observed with an 8 s delay).

### 2.3 Background tool calls

- A tool marked `"background": "always"` returns to the model at once as a
  task. The turn can end with the call still open (`Stop` reports
  `fullyIdle: false`). When the call finishes, an idle conversation starts
  a new turn with the result. If the conversation is busy, the result
  arrives as a message at its next model call.
- A call that passes `timeoutSeconds` wakes the conversation with an error.
- A background shell command behaves the same on exit; its output lines
  wake nothing.
- Escape does not cancel an open background call. `/new` leaves it open,
  and when it finishes the earlier conversation runs a turn in the
  background. `/exit` sends the server SIGTERM.
- While a background call is open the interactive footer shows a running
  task.
- One-shot: `agy -p` without `--print-timeout` waits for every background
  task before it exits. No environment variable tells a one-shot run from
  an interactive one; the `agy` process's command line does (`-p`,
  `--print`, `--prompt`).

### 2.4 Hooks

- Events: `SessionStart`, `PreInvocation`, `PostInvocation`, `PreToolUse`,
  `PostToolUse`, `Stop`. There is no prompt-submit event and no
  session-end event.
- `hooks.json` is a map of named groups. `SessionStart`, `PreInvocation`,
  `PostInvocation` and `Stop` take a flat list of handlers;
  `PreToolUse` and `PostToolUse` take a list of `{matcher, hooks}`.
- A hook runs with the plugin directory as its working directory, with
  `PLUGIN_ROOT`, `PLUGIN_DATA` and `ANTIGRAVITY_CONVERSATION_ID` set, and
  the same parent process as the MCP server. It can reach `127.0.0.1`.
- Input is camelCase JSON on stdin. Common fields: `conversationId`,
  `hookEventName`, `cwd` (the workspace), `workspacePaths`,
  `transcriptPath`, `artifactDirectoryPath`, `modelName`, `lastUserInput`.
  No field names a parent or root conversation.
- `SessionStart` fires once per conversation, at its first prompt, and also
  for each subagent. It does not fire when a conversation is resumed
  (`agy --conversation <id>`), and the conversation ID stays the same.
  `{"injectSteps":[{"systemMessage":{"systemMessage":"…"}}]}` reaches the
  model.
- `PreInvocation` fires before every model call, with `invocationNum`,
  which is 0 for the first call of each turn (a turn started by a person's
  message or by a background result). The same `injectSteps` output reaches
  the model.
- `PostToolUse` carries `toolCall.name`, `toolCall.args`, `toolCall.id`,
  and `error`. A lazy MCP call has the name `call_mcp_tool`; an eager one
  has `mcp_<server>_<tool>`. For a background call it fires when the call
  finishes.
- `Stop` carries `terminationReason` (`NO_TOOL_CALL` for a normal end),
  `executionNum`, `fullyIdle`, `continuationActive`.
  `{"decision":"continue","reason":"…"}` continues the turn with the
  reason as a system message. `continuationActive` stayed `false` after a
  continue. `executionNum` is 1 at the `Stop` after a continue and 0 at
  the first `Stop` of every later turn, whether a person's message or a
  background result started it.
- A hook that prints nothing is accepted for every event. For
  `PreToolUse`, printing `{}` denies the call and exiting non-zero fails
  the call. For the other events a non-zero exit did not stop the turn.
- No hook fires when the conversation is left (`/new`) or the CLI exits,
  except `PostToolUse` for a background call the exit cut off.

### 2.5 Permissions and the sandbox

- Every MCP tool call asks the person first, including tools annotated
  read-only. The prompt names `<server>/<tool>` and offers allow or deny
  once, for the conversation, or persisted to settings.
- A headless run cannot ask and denies the call.
- A rule `mcp(<server>/*)` in `permissions.allow` of
  `~/.gemini/antigravity-cli/settings.json` allows every tool of the
  server, eager names included. A `PreToolUse` hook answering `allow` does
  not.
- The CLI starts in `auto` mode by default (documented), which runs the
  agent's shell commands in a sandbox. A sandboxed command could not
  connect to `127.0.0.1`; the agent can ask the person to run a command
  outside the sandbox. MCP servers and hooks are not sandboxed.
- Commands the agent runs get `ANTIGRAVITY_AGENT=1` and
  `ANTIGRAVITY_CONVERSATION_ID`, among other `ANTIGRAVITY_*` variables.

### 2.6 What cannot wake a conversation from outside

`agy agentapi send-message <conversation ID> <text>` exists but needs
`ANTIGRAVITY_LS_ADDRESS` and a CSRF token that only the agent's own shell
commands receive, and it failed inside the sandbox. There is no equivalent
of `codex queue` for the daemon to call.

## 3. How Clax wires a harness today (verified in the code)

- **Shim.** `clax mcp --agent <claude|codex|grok>`
  (`crates/clax-commands/src/commands/mcp.rs:13`) runs
  `crates/clax-mcp/src/shim.rs`. It registers one session per process
  before serving (`shim.rs:189-212`, `registration` at `shim.rs:52`), holds
  that one session in `DaemonClient`'s state
  (`crates/clax-mcp/src/client.rs:198-209`), heartbeats it every 60 s and
  ends it when stdin closes or SIGTERM arrives (`shim.rs:249-282`).
- **Per-session tools exist.** `ClaxTools::in_session`
  (`crates/clax-mcp/src/tools.rs:1644`) binds the tools to a given session;
  the daemon's `/mcp` HTTP adapter uses it for one session per
  `Mcp-Session-Id` (`crates/clax-server/src/routes/mcp_http.rs`).
- **Tool calls.** `ServerHandler::call_tool` (`tools.rs:5154`) receives the
  rmcp `RequestContext` and passes known tools to `invoke` without it. No
  tool reads `_meta` today.
- **Feedback on tool results.** `finish` (`tools.rs:2843`) appends
  `piggyback()` (`tools.rs:2804`), a take of `tier=piggyback` for the
  session.
- **`wait_for_feedback`.** `do_wait` (`tools.rs:4228`) long-polls
  `GET /api/sessions/<sid>/feedback?tier=wait&wait=<secs>` with `secs`
  clamped to 1..600 (`tools.rs:238-244`; the server caps at 600 in
  `crates/clax-server/src/routes/feedback.rs:36`). While the poll is open
  the daemon marks the session waiting and skips native push for it
  (`routes/feedback.rs:377`).
- **Hooks.** `clax hook --agent <claude|codex|grok> <event>`
  (`crates/clax-commands/src/commands/hook.rs:108,171`) parses stdin with
  `HookInput` (`crates/clax-hooks/src/input.rs`), which already reads
  Grok's camelCase keys, and prints Claude Code's shapes from
  `crates/clax-hooks/src/output.rs` (`additional_context`, `block`).
  `session-start` joins a session (`events.rs:62`); the other events find
  the live session by harness and harness session ID
  (`events.rs:305-321`). With no daemon a hook prints nothing and exits 0.
- **Where the person is.** Claude Code's `prompt` hook polls with
  `prompt=true`, which records that the person typed in the terminal
  (`events.rs:624`, `note_poll` in `crates/clax-http/src/surface.rs:230`).
  Codex and Grok have no such hook and are in `INFERRED`
  (`surface.rs:219`): a turn in which nothing came from Clax stands for
  one the person typed, and any poll but the Codex queue's counts as
  coming from inside a turn.
- **Watches after an exit.** A registration with a known harness session
  ID gets its watches back only when the earlier session was ended by the
  reaper as idle (`crates/clax-store/src/sessions.rs:1392`), not when its
  shim ended it on exit.
- **The turn-end reminder.** With nothing to hand over, `stop` may block
  once to tell the agent how to be woken (`unwoken`, `events.rs:483-557`),
  using the wake block from `crates/clax-core/src/wake.rs:74`.
- **Push state.** `push_info`
  (`crates/clax-server/src/routes/sessions.rs:424-469`) reports per
  harness how an idle session is woken. The app's sessions use tier
  `wait`.
- **Sessions in the store.** Registration and join match on
  `(harness, harness_session_id)` first
  (`crates/clax-store/src/sessions.rs:576-599,759-819`). The reaper ends a
  local session unseen for 300 s whose process is gone
  (`sessions.rs:489-503`). `HARNESSES` in `routes/sessions.rs:23` lists the
  accepted harness names.
- **Install.** `clax init` has one table entry per harness
  (`crates/clax-commands/src/commands/init.rs:77-122`), writes the embedded
  plugins to `~/.clax/marketplace/` and runs the harness's own install
  command. For Codex it offers to add tool approvals to Codex's config
  (`crates/clax-commands/src/codex_approvals.rs`, `init.rs:800-911`).
- **Stand-down.** A Claude Code copy that Grok runs does nothing
  (`crates/clax-commands/src/host.rs`, `crates/clax-mcp/src/standdown.rs`,
  and the same check in `scripts/ensure-clax.sh:650-660`).

## 4. Goals, non-goals, anti-goals

Goals

- G1. `clax init` installs the plugin when `agy` is on `PATH`; `clax init
  --agent antigravity` installs it alone; `clax uninit` removes it.
- G2. All Clax tools work from an Antigravity conversation, each call
  acting in the Clax session of the conversation that made it.
- G3. Comments reach a conversation on the next Clax tool result, at the
  start of its next turn, at the end of a turn, and, when it is idle,
  through a background wait that wakes it with the comments.
- G4. `clax doctor --agent antigravity` names what is wrong at each layer.
- G5. A Claude Code copy of Clax that someone imported into Antigravity
  does nothing and says so.

Non-goals

- The Antigravity desktop app and IDE. They share the plugin format but
  were not tested.
- Asking the person through Antigravity's own question tool, as the
  Claude Code plugin does with `AskUserQuestion`. It would need a
  `PreToolUse` hook, which section 2.4 shows is unsafe. The `ask` tool
  works as a tool.
- Slash commands. Antigravity does not load `commands/`; the Codex and
  Grok plugins have none either.
- Antigravity connected straight to a hosted Clax's `/mcp`. Such a
  session keeps today's handling of an unknown MCP client; the wake of
  section 6.4 exists only through the plugin's shim.
- Session transcript links for Antigravity conversations.
- A marketplace listing. `clax init` and `agy plugin install <dir>` are
  the install paths.

Anti-goals: it fails, even working, if

- a comment is marked delivered to a conversation that never showed it to
  the model (a subagent's context, or a hook output Antigravity ignored);
- a hook failure breaks a tool call or a turn;
- the person is asked to approve each of forty-odd tools one at a time
  with no way offered to approve them together.

## 5. Approaches

**A. Import the Claude Code plugin** (`agy plugin import claude`, or `agy
plugin install plugins/claude-code`). The converter was run: it keeps
`--agent claude`, keeps only the `SessionStart`, `Stop` and `PostToolUse`
hooks, drops `UserPromptSubmit`, `SessionEnd` and the `AskUserQuestion`
hooks, and turns the eight commands into `clax-cmd-*` skills that still
contain `${CLAUDE_PLUGIN_ROOT}`. Its hooks would do nothing: Antigravity's
input has `hookEventName`, which today's Grok stand-down check takes for
Grok's envelope (`crates/clax-commands/src/host.rs:26`,
`crates/clax-hooks/src/input.rs:54`), so they exit silently and are logged
as `host=grok`. Its MCP server would register one Claude Code session per
`agy` process and give Claude Code's wake instructions. Comments would
arrive on tool results only. Not taken.

**B. The bare MCP server** (`agy mcp add`), with no plugin. Tools only: no
skill, no hooks, no wake. Every session would also be one Clax session per
`agy` process, wrong as soon as there is a second conversation. Not taken.

**C. A native plugin** with its own agent name, hooks that speak
Antigravity's contract, and a shim that keeps one Clax session per
conversation. Taken.

Within C, three ways to wake an idle conversation were tested:

- a background shell command running `clax feedback follow --once`, whose
  exit wakes the conversation. The default sandbox blocks its connection
  to the daemon. Not taken.
- `agy agentapi send-message` from the daemon. The daemon has no way to
  reach the conversation's language server (section 2.6). Not taken.
- a background MCP tool call that stays open until there are comments.
  It needs no shell, is not sandboxed, and its result carries the comments
  themselves. Taken (section 6.4).

## 6. Design

### 6.1 The plugin directory

`plugins/clax-antigravity/`:

```
.antigravity-plugin/plugin.json   name clax-antigravity, version, description, author, keywords
mcp_config.json
hooks.json
rules/AGENTS.md
skills/clax/SKILL.md
scripts/ensure-clax.sh            copy of scripts/ensure-clax.sh
scripts/tool-hook.sh              copy of scripts/tool-hook.sh
README.md
```

The plugin is named `clax-antigravity`, not `clax`: an imported Claude
Code copy installs as `clax`, and a second install of that name replaces
the first.

`mcp_config.json`:

```json
{
  "mcpServers": {
    "clax": {
      "command": "bash",
      "args": ["${PLUGIN_ROOT}/scripts/ensure-clax.sh", "exec", "mcp", "--agent", "antigravity"],
      "timeoutSeconds": -1,
      "tools": {
        "wait_for_feedback": {"background": "always", "eager": true},
        "publish": {"eager": true}
      }
    }
  }
}
```

with `"eager": true` also on `read`, `list`, `open`, `status`, `watch`,
`working`, `comments_read`, `comments_reply` and `comments_resolve`. The
other tools stay lazy. `timeoutSeconds` is `-1` because the wait in
section 6.4 has no upper bound and `ask` may wait 600 s.

`hooks.json`:

```json
{
  "clax": {
    "PreInvocation": [
      {"type": "command", "command": "./scripts/tool-hook.sh antigravity prompt", "timeout": 5}
    ],
    "Stop": [
      {"type": "command", "command": "./scripts/ensure-clax.sh exec hook --agent antigravity stop", "timeout": 10}
    ],
    "PostToolUse": [
      {"matcher": "*", "hooks": [
        {"type": "command", "command": "./scripts/tool-hook.sh antigravity", "timeout": 5}
      ]}
    ]
  }
}
```

There is no `SessionStart` hook and no `PreToolUse` hook (sections 6.3 and
2.4).

### 6.2 Sessions: one shim, many conversations

`clax mcp --agent antigravity` differs from the other shims in one way:
it keeps a Clax session per Antigravity conversation instead of one per
process.

- **No registration at startup.** `agy` starts the server at launch,
  before any conversation exists.
- **The session of a call.** `call_tool` reads
  `antigravity.google/root_conversation_id` from the request's `_meta`,
  falling back to `antigravity.google/conversation_id`. That root
  conversation ID is the session's harness session ID. The first call of a
  conversation registers it (`POST /api/sessions` with harness
  `antigravity`, that ID, the shim's `pid`, the `agy` process as
  `parent_pid`, and the working directory, repository and branch of the
  `agy` process, found as the Codex shim finds its parent's). The call
  then runs with tools bound to that session, as
  `ClaxTools::in_session` does for the `/mcp` adapter.
- **Subagents.** A subagent's calls carry the root conversation's ID and
  so act in the root's session. Feedback is handed only to the root
  conversation: a call whose `conversation_id` differs from its
  `root_conversation_id` gets no feedback appended to its result, and
  `wait_for_feedback` from it is refused with a message saying to call it
  from the main conversation.
- **A call with no conversation in `_meta`.** It registers as the Codex
  shim does, with no harness session ID. The store's existing rule
  (`crates/clax-store/src/sessions.rs:576-603`) then gives it the newest
  live session of the same `agy` process, or a new one. So if Antigravity
  stops sending the keys, the plugin degrades to one session per `agy`
  process and the tools keep working; nothing in the store changes for
  this. `clax doctor` reports a shim-registered `antigravity` session with
  no harness session ID (section 6.9).
- **Liveness.** The shim heartbeats every session it registered, at the
  existing interval, and re-registers one the daemon no longer knows, as
  today. On SIGTERM or stdin close it ends all of them within the existing
  end timeout. A conversation left with `/new` stays a live session until
  the process exits; section 2.3 shows it can still be woken and answer.
- **Resume.** A resumed conversation has the same ID and a new shim. Its
  first Clax call registers it again. Nothing reaches it before that call,
  because `SessionStart` does not fire on resume. As for every harness,
  its watches come back only if the earlier session was reaped as idle
  (a crash); after a normal exit the new session watches nothing until
  the model publishes or calls `watch` again, and the skill says so
  (section 6.6). This design does not change that rule.

### 6.3 Hooks

`clax hook --agent antigravity <event>` reads Antigravity's input
(`conversationId` as the session ID, `cwd`, `hookEventName`,
`transcriptPath`) and prints Antigravity's shapes. No event joins or
creates a session: each finds the live session with harness `antigravity`
and harness session ID equal to `conversationId`, and does nothing when
there is none. A subagent's hooks therefore do nothing, since its
conversation ID is never a session's.

| Clax event | Antigravity event | What it does | Output |
|---|---|---|---|
| `prompt` | `PreInvocation`, first model call of a turn | takes `tier=prompt_hook` feedback, without `prompt=true` | `{"injectSteps":[{"systemMessage":{"systemMessage":"<text>"}}]}`, or nothing |
| `stop` | `Stop` | hands over pending feedback, or gives the turn-end reminder (section 6.4), or ends the working marks | `{"decision":"continue","reason":"<text>"}`, or nothing |
| `tool` | `PostToolUse`, every tool | renews working marks, at most once a minute | nothing |

- `scripts/tool-hook.sh` gains the `antigravity` agent. It takes the
  session ID from `conversationId`. In its `prompt` mode it exits 0 unless
  the input's `invocationNum` is 0, so `clax` starts once per turn, not
  once per model call.
- The first model call of a turn is not always the person's: a background
  result starts a turn too. So `prompt` does not claim the person typed.
  `antigravity` joins `INFERRED` instead, and the daemon infers it as it
  does for Codex and Grok: the turn-start poll after a turn in which
  nothing came from Clax stands for a prompt the person typed, and a turn
  the open wait started does not. One exception is needed: an
  `antigravity` session's `tier=wait` polls come from outside a turn, as
  the Codex queue's do, and are not counted as turn activity.
- `stop` acts only when `terminationReason` is `NO_TOOL_CALL`. An
  `executionNum` above 0 is treated as Claude Code's `stop_hook_active`:
  resends off, and no second reminder. It is 0 again at the next turn
  (section 2.4).
- `session-start` and `session-end` are not wired. A new conversation has
  no session until its first Clax call, so there is nothing to join and
  nothing waiting; `rules/AGENTS.md` tells the model Clax is there.
  Sessions end when the shim exits.
- Every hook prints nothing and exits 0 when no `clax` can run or no
  daemon is running, as today.

### 6.4 Waking an idle conversation

The wake is `wait_for_feedback`, marked as a background tool.

- **Unbounded.** Under `--agent antigravity`, when the call gives no
  `timeout_s`, the shim keeps polling the daemon (`tier=wait`, the server's 600 s cap per
  poll) until there is feedback, an answer or a withheld notice to hand
  over, or the client cancels the call. It then returns the handover as
  today, which the daemon counts as acknowledged.
- **With a `timeout_s`.** The call is a background task too, since the
  setting is per tool: it returns to the model at once and finishes at the
  timeout with `call_again: true`, as today. The skill says to call it
  again only when the person asked the agent to keep waiting.
- **What the model does.** After its first publish or watch in a
  conversation it calls `wait_for_feedback` and ends its turn. Antigravity
  returns at once with a task; the conversation goes idle; when a person
  sends a comment the call finishes and the conversation starts a turn
  holding the comments. The model acts, replies, and calls
  `wait_for_feedback` again.
- **Push state.** For an `antigravity` session the daemon reports
  `push.tier: "wait"` and `available: true` while its wait is open, and
  otherwise `available: false` with a reason saying no wait is open. It
  reads unavailable for an instant between two polls; a second wait
  called while one is open returns at once and opens nothing.
- **The wake block** (`clax_core::wake::block`) says, when available,
  "Start nothing: your `wait_for_feedback` call is open and a comment sent
  to you finishes it. End your turn when you are done."; and otherwise
  "A comment will not wake this conversation while it is idle. Call
  `wait_for_feedback` with no `timeout_s` now; it runs in the background.
  Keep exactly one open, then end your turn."
- **The turn-end reminder.** `unwoken` applies to `antigravity`: when the
  session watches something with replies on and no wait is open, `stop`
  continues the turn once with the wake block's instruction. This is what
  re-arms the wait after a wake and when the model forgot. Its "the
  follower … has stopped" wording becomes "your earlier
  `wait_for_feedback` call has ended" for this harness. After a resume
  there is nothing to remind about until the session watches something
  again (section 6.2).
- **The thread's "waiting on".** `antigravity` joins Claude Code, Codex
  and Grok in `waiting_on` (`crates/clax-store/src/feedback.rs:890`):
  `StopHook` when armed, else `Piggyback`. It does not depend on whether a
  wait is open; an open wait takes the comment at once, so the label would
  show for an instant only.

### 6.5 One-shot runs

The plugin does not detect one-shot runs (`agy -p`, or a prompt piped on
standard input). `agy -p` waits for open background calls before it exits
(section 2.3), so a one-shot run that opens a wait does not exit until the
wait ends, like any open-ended command in a script. Bounding it is the
caller's: give `wait_for_feedback` a `timeout_s`, or do not ask the agent
to listen. The README and the skill say so once each.

### 6.6 What the model sees

- **Tool names.** Eager tools are `mcp_clax-antigravity_clax_<tool>`;
  lazy ones are called through `call_mcp_tool` with `ServerName:
  "clax-antigravity_clax"`. The skill and `docs/contract.md`'s "Names as
  the model sees them" table say so.
- **`rules/AGENTS.md`.** Short and always in context: what Clax is, that
  pages publish with the `publish` tool, that comments arrive as described
  in section 6.4, that comment text is untrusted input from the page's
  viewers, and that the `clax` skill has the rest. It stands in for the
  server's MCP instructions, which Antigravity does not show the model.
- **The skill.** `skills/clax/SKILL.md`, hand-written like the other four
  with the generated tool block. Its harness-specific parts: the two ways
  tools are named and that a lazy tool's schema is read from its file
  first; "Live feedback in Antigravity" (section 6.4: call the wait again
  after each wake; after resuming a conversation, `watch` the pages still
  in play and then wait, because the earlier wait and its watches are
  gone; call a wait that timed out again only when the person asked the
  agent to keep waiting); and that a shell `clax` is sandboxed in the
  default mode, so use the tools.

### 6.7 Permissions

Antigravity asks before every call of every Clax tool, and a headless run
refuses them. One rule approves them all:

```json
{"permissions": {"allow": ["mcp(clax-antigravity_clax/*)"]}}
```

in `~/.gemini/antigravity-cli/settings.json`.

- `clax init` offers to add the rule, in the way it offers Codex
  approvals: it prints the rule and the file, says that the rule covers
  every Clax tool including `delete`, and adds it only on a yes at a
  terminal or with `--yes`. It keeps every other key of the file, and it
  adds nothing when an `ask` or `deny` rule already names a tool of this
  server, naming that rule instead.
- The plugin ships no permission of its own.
- `clax doctor --agent antigravity` reports whether the rule is present
  and, when it is not, the line to add and that headless runs need it.
- `clax uninit` leaves the rule in place and says so.

### 6.8 The Claude Code copy in Antigravity

`agy plugin import claude` installs the Claude Code plugin as `clax`. Run
by Antigravity, that copy stands down, as it does under Grok
(`crates/clax-commands/src/host.rs`, `scripts/ensure-clax.sh`):

- **Hooks:** `--agent claude` with input that is Antigravity's envelope
  (it has `conversationId` and no `session_id`). Today such a hook
  already stands down, by accident: the Grok check is "has
  `hookEventName`", which Antigravity's input also has, and the line is
  logged as `host=grok`. `HookInput` must tell the two envelopes apart so
  the log and the doctor name the right host.
- **MCP:** `--agent claude` when the parent process is `agy`.
- A stood-down server offers only `status`, whose text says Clax runs
  from clax-antigravity and how to install it (`clax init --agent
  antigravity`) or remove the copy (`agy plugin uninstall clax`); a
  stood-down hook prints nothing. Each stand-down is logged with
  `host=antigravity`, and doctor counts those lines.

The wrapper's own copy of the check runs before any `clax` is found, so it
needs the same two rules in shell.

### 6.9 Install, doctor, development and build

- **`clax init`.** A `HARNESSES` entry `antigravity`, between `grok` and
  `pi`: removal `agy plugin uninstall clax-antigravity` (not required to
  succeed), addition `agy plugin install
  <marketplace>/plugins/clax-antigravity` (required), `uses` true when
  `agy plugin list --json` lists `clax-antigravity` (the listing gives the
  installed copy's path, not its source), or from `registrations.json`
  when `agy` is not on `PATH`. The harness is skipped when `agy` is not on
  `PATH`. Section 6.7's offer runs after a successful install.
- **`clax doctor --agent antigravity`.** The common checks (`binary`,
  `upgrade`, `plugin`, `skill`, `mcp`, `hooks`, `feedback`), with `plugin`
  reading `~/.gemini/config/plugins/clax-antigravity`, and:
  - `antigravity`: `agy --version` is at least 1.5.18, the version this
    design was verified on;
  - `antigravity_permissions`: section 6.7;
  - `antigravity_sessions`: a live `antigravity` session that a shim
    registered with no harness session ID (section 6.2), which means
    Antigravity did not send the conversation ID;
  - `claude_copy`: stand-down lines with `host=antigravity`, with the
    command that removes the copy.
- **`just dev antigravity`.** Like Codex and Grok: `agy` runs its
  installed plugin with the fresh build in `CLAX_BIN` and the dev home in
  `CLAX_HOME` (the MCP server and hooks inherit `agy`'s environment), and
  plugin changes reach it through `just install`. A workspace plugin is
  not used because its tools are missing on the first turn.
- **Embedding and build.** `clax-antigravity` joins `PLUGIN_DIRS` in
  `crates/clax-embedded/build.rs`, `plugin_files()` in
  `crates/clax-embedded/src/plugins.rs`, the globs in
  `plugins/BUILD.bazel`, and `.antigravity-plugin/plugin.json` joins
  `MANIFESTS` in `crates/clax-mcp/src/plugin.rs`. There is no marketplace
  file.
- **Scripts.** The lists in `scripts/test-plugins.sh`,
  `scripts/sync-skill-tools.py`, `scripts/bump-version.sh`,
  `scripts/check-version.sh`, `scripts/pin-release.sh`,
  `scripts/test-release.files`, `scripts/test-release.sh`,
  `scripts/dev.sh` and `scripts/test-dev.sh` gain the plugin.
  `scripts/test-plugins.sh` also runs `agy plugin validate --strict` on
  the plugin when `agy` is on `PATH`. `scripts/probe-harness-env.sh`
  prints `ANTIGRAVITY*` names, with values for `ANTIGRAVITY_AGENT` and
  `ANTIGRAVITY_CONVERSATION_ID` only.
- **CLI commands in the agent's shell.** `ANTIGRAVITY_AGENT=1` joins
  `HARNESS_VARIABLES` (`crates/clax-commands/src/channel.rs:34`) and
  `ANTIGRAVITY_CONVERSATION_ID` joins `SESSION_VARIABLES`
  (`crates/clax-commands/src/commands/remotes.rs:71`).

### 6.10 Daemon

- `antigravity` joins `HARNESSES` (`routes/sessions.rs:23`). No migration:
  the harness is a string.
- `push_info` and `wakes_when_idle` gain the `antigravity` arm of
  section 6.4.
- `waiting_on` treats `antigravity` as it treats Claude Code, Codex and
  Grok (section 6.4).
- `antigravity` joins `INFERRED`, and `note_poll` skips an `antigravity`
  session's `tier=wait` polls as it skips the Codex queue's (section 6.3).
- The display names in `crates/clax-store/src/toolpath/mod.rs` gain
  Antigravity (company Google).
- The `/mcp` adapter's naming of a harness from `clientInfo.name`
  (`routes/mcp_http.rs:191-202`) is left as it is (section 4, non-goals).

### 6.11 Docs

`docs/contract.md`: the shim's agent list, "Names as the model sees them",
a `### Antigravity` section under "Sessions", the "Harness shell
environments" paragraph with the variables section 2.5 observed, a column
in "Delivery tiers per harness", and `clax init`. `docs/usage.md` and
`README.md`: install, init, doctor and `just dev` lists. The plugin's
README follows the Grok plugin's.

## 7. Feedback tiers for Antigravity

| Tier | How it reaches an Antigravity conversation |
|---|---|
| 1, tool result | appended to the next Clax tool result of the root conversation |
| 2, end of turn | `Stop` continues the turn with the comments |
| 3, next message | `PreInvocation` on the first model call of a turn adds them as a system message |
| 4, waiting | `wait_for_feedback` with a `timeout_s` |
| 5, idle | the open `wait_for_feedback` finishes with the comments and starts a turn |

Tiers 4 and 5 are the same call; tier 5 is the form with no `timeout_s`.

## 8. Tests

Fast, no sleeps, injected clocks, helpers from `clax-testkit`.

- **Hooks** (`crates/clax-hooks/tests/golden.rs`): fixtures taken from
  Appendix A for `PreInvocation` (0 and 1), `Stop` (normal, continued,
  other termination reasons) and `PostToolUse` (lazy and eager names).
  Cases: output shapes; a conversation with no session prints nothing;
  the reminder is given once and not when `executionNum` is above 0; no
  daemon prints nothing.
- **Shim** (`crates/clax-mcp/tests/shim.rs`): no session registered before
  the first call; two root conversations get two sessions; a subagent's
  call acts in the root's session, gets no feedback appended and cannot
  wait; a call with no `_meta` registers with no harness session ID; every
  session is heartbeated and all are ended on exit; `wait_for_feedback`
  with no `timeout_s` outlasts one server poll and returns on feedback and
  on cancel; with a `timeout_s` it is bounded. The shim's tests run the
  2026-07-28 handshake, which is what Antigravity uses with it.
- **Daemon:** `push_info` and `wakes_when_idle` for an `antigravity`
  session with and without an open wait; `waiting_on`; and where the
  person is: a turn the person started records terminal activity, a turn
  the open wait started does not, and an idle wait's polls record nothing.
- **Init** (`crates/clax-cli/tests/init.rs`): a fake `agy`; installs only
  `clax-antigravity`; the permissions offer adds the rule on yes, keeps
  other keys, and adds nothing beside an `ask` or `deny` rule; `uninit`.
- **Stand-down** (`crates/clax-cli/tests/standdown.rs`,
  `scripts/test-ensure-clax.sh`): the Claude Code copy under Antigravity;
  Claude Code itself with Antigravity's variables inherited still acts;
  Grok's stand-down is unchanged by the envelope change.
- **Plugin files** (`scripts/test-plugins.sh`): manifest name, server key
  and args, the background and eager settings, `timeoutSeconds`, hook
  commands, no `PreToolUse` and no `SessionStart`, wrapper copies equal,
  skill sections.
- **Manual smoke** (`scripts/smoke-antigravity.sh`, calls a model):
  install, publish from `agy -p` with the permission rule, check the page
  and the session's harness session ID; then the interactive wake of
  section 6.4 by hand, including a wait open for more than ten minutes.

## 9. Decisions for the owner

None open; section 10 lists what the owner settled.

## 10. Settled

Settled by the owner (2026-11-26):
- `clax init` offers to write the permission rule (section 6.7), rather
  than only printing it as the Grok plugin does.
- `wait_for_feedback` is the wake tool, unbounded under Antigravity
  (section 6.4), instead of a new forty-fourth tool; one
  tool's default differs by harness, as `ask`'s timeout does for Codex.
- CLI only. The Antigravity app and IDE are out of scope until tested.
- Names: agent and harness `antigravity`; plugin `clax-antigravity`;
  server key `clax`.
- The plugin does not detect one-shot runs; bounding a one-shot run's wait
  is the caller's (section 6.5).

Settled by the designer (calls to check):
- Sessions are keyed by the root conversation, and subagents act in it
  but are never handed feedback.
- No `SessionStart` hook; registration happens at the first Clax call.
- `PreInvocation` is used, gated in shell to the first model call of a
  turn, and it does not claim the person typed: whether they did is
  inferred as for Codex and Grok.
- A conversation resumed after a normal exit watches nothing until the
  model watches again, as on the other harnesses.
- A wait with a `timeout_s` is a background task too.
- The shim serves every MCP revision it knows; Antigravity uses
  2026-07-28 with it.
- Eleven tools are eager (section 6.1); the rest are lazy.
- `timeoutSeconds: -1`, which also removes the connect limit.
- The turn-end reminder is what re-arms the wait.
- The minimum supported `agy` is 1.5.18.
- `clax uninit` leaves the permission rule.

## 11. Left to the builder

- How the shim holds its sessions and how `call_tool` passes the session
  to `invoke`.
- How the shim tracks that a conversation's unbounded wait is open.
- The exact wording of `rules/AGENTS.md`, the wake block and the
  stand-down text.
- Whether `tool-hook.sh` reads `invocationNum` itself or a second small
  script does.
- Whether `agy plugin validate --strict` needs a logo, and adding one.
- How `clax init` edits `settings.json` while keeping its other keys.

## 12. Not verified, and standing risks

- A wait open for more than 200 s, and across machine sleep. The manual
  smoke covers ten minutes.
- `--print-timeout` expiring with a call open (the CLI's changelog says
  exit code 3).
- Seen on MCP 2025-11-25 only, with the probe plugin: the wake of an idle
  interactive conversation, `notifications/cancelled` at a timeout, and a
  subagent's `_meta`. On 2026-07-28, with the real shim, a one-shot
  background `wait_for_feedback` and the root conversation's `_meta` were
  seen. The unbounded wait must end when the client cancels on either.
- A turn started by something other than a person or Clax (a background
  shell command finishing, a scheduled timer) is inferred as one the
  person typed.
- What Antigravity does when a hook exceeds its `timeout`.
- The environment of an imported Claude Code copy's MCP server; the
  stand-down rule for MCP relies on the parent process, not on it.
- Linux.
- Working marks while only a subagent is using tools: the root's
  `PostToolUse` does not fire until the subagent returns, so a long
  subagent run can let the root's working mark lapse.
- The conversation ID in `_meta` and the wake when a background call
  finishes are observed behaviour that Google does not document. The
  doctor's version check and section 6.2's degraded mode are the guards.

## 13. Alternatives not taken (to record as qualifier `alternative`s)

- **Importing the Claude Code plugin** (section 5, A). `revisit:` the
  importer translates hook output and agent names.
- **A dedicated wake tool** beside `wait_for_feedback`. Rejected: a
  forty-fourth tool for every harness, or a tool list that differs by
  harness. `revisit:` another harness needs an unbounded background wait
  with different semantics.
- **A background shell follower** (`clax feedback follow --once`).
  Rejected: sandboxed in the default mode. `revisit:` Antigravity lets a
  plugin grant its commands localhost access.
- **Daemon-side push with `agy agentapi send-message`.** Rejected: the
  daemon cannot reach a conversation's language server. `revisit:`
  Antigravity documents an address and credential a plugin process may
  use.
- **One Clax session per subagent.** Rejected: hooks cannot tell a
  subagent from a root conversation, and comments handed to a subagent
  may never reach the person's conversation. `revisit:` hook input names
  the parent conversation.
- **A `SessionStart` hook that joins the session**, as the other harnesses
  do. Rejected: it fires for every subagent and not on resume, so it would
  create sessions that never act and miss the case it exists for.
- **A `PreToolUse` hook** (to approve Clax tools, or to mirror
  `AskUserQuestion`). Rejected: an empty JSON answer denies and a failure
  breaks the call.
- **Shipping the permission rule in the plugin.** Rejected, as for Codex:
  it would approve `delete` without asking the person.
- **All tools eager.** Rejected: forty-odd schemas in every turn's
  context. `revisit:` models skip reading lazy schemas and call tools
  wrongly.
- **`prompt=true` from `PreInvocation`.** Rejected: a turn the open wait
  started would be recorded as the person typing in the terminal.
  `revisit:` hook input says what started the turn.
- **A "waiting on" label that follows the open wait.** Rejected: the
  store cannot see open waits, and the label would show only until the
  wait takes the comment.
- **Restoring watches after a normal exit and resume.** Not taken here:
  it is the same on every harness. `revisit:` people resume conversations
  and expect their pages' comments to follow.
- **Detecting one-shot runs** (from the `agy` command line and whether its
  standard input is a terminal) to bound their waits and drop the
  reminder. Rejected by the owner: a one-shot run that opens a wait is the
  caller's to bound (section 6.5).
- **Holding the shim to MCP 2025-11-25 for Antigravity.** Rejected: the
  real shim was run under `agy` on 2026-07-28 and worked. `revisit:` a
  behaviour section 12 lists as unseen on 2026-07-28 fails there.

## Appendix A: captured payloads

Paths shortened; IDs are examples.

`tools/call` `_meta`, root conversation and subagent, on MCP 2025-11-25.
On 2026-07-28 the same `antigravity.google/…` keys appear beside
`io.modelcontextprotocol/clientCapabilities`,
`io.modelcontextprotocol/clientInfo` and
`io.modelcontextprotocol/protocolVersion`.

```json
{"antigravity.google/artifacts_dir": "~/.gemini/antigravity-cli/brain/b7bfc3b1-e88c-423f-b4bb-ff457c34ea29",
 "antigravity.google/conversation_id": "b7bfc3b1-e88c-423f-b4bb-ff457c34ea29",
 "antigravity.google/root_conversation_id": "b7bfc3b1-e88c-423f-b4bb-ff457c34ea29",
 "progressToken": "854a6d31-8517-438c-9333-afa2dbed502f:4"}

{"antigravity.google/artifacts_dir": "~/.gemini/antigravity-cli/brain/41d3821b-c092-48c5-ae20-b1a9a38369e0",
 "antigravity.google/conversation_id": "41d3821b-c092-48c5-ae20-b1a9a38369e0",
 "antigravity.google/parent_conversation_id": "b7bfc3b1-e88c-423f-b4bb-ff457c34ea29",
 "antigravity.google/root_conversation_id": "b7bfc3b1-e88c-423f-b4bb-ff457c34ea29"}
```

Hook input, common fields (`PreInvocation` shown):

```json
{"conversationId": "b7bfc3b1-e88c-423f-b4bb-ff457c34ea29",
 "hookEventName": "PreInvocation",
 "cwd": "/path/to/workspace",
 "workspacePaths": ["/path/to/workspace"],
 "transcriptPath": "~/.gemini/antigravity-cli/brain/b7bfc3b1-…/.system_generated/logs/transcript_full.jsonl",
 "artifactDirectoryPath": "~/.gemini/antigravity-cli/brain/b7bfc3b1-…",
 "modelName": "gemini-3.8-flash-low",
 "lastUserInput": "Reply with just the word HELLO.",
 "invocationNum": 0,
 "initialNumSteps": 1}
```

`PostToolUse`, a lazy MCP call and an eager one, as captured from the
probe plugin (plugin `claxprobe`, server key `probe`, tools `whoami` and
`wait_bg`). Antigravity adds a `toolSummary` argument to an eager tool's
parameters; the server received only `{"seconds": 2}`.

```json
{"toolCall": {"id": "call_946817164294668622", "name": "call_mcp_tool",
              "args": {"ServerName": "claxprobe_probe", "ToolName": "whoami", "Arguments": {}}},
 "stepIdx": 4, "error": ""}

{"toolCall": {"id": "call_15898288830536712374", "name": "mcp_claxprobe_probe_wait_bg",
              "args": {"seconds": 2, "toolSummary": "Call wait_bg probe"}},
 "stepIdx": 5, "error": ""}
```

`Stop`, at a normal end with a background call open, and after a continue:

```json
{"terminationReason": "NO_TOOL_CALL", "executionNum": 0, "fullyIdle": false, "continuationActive": false, "error": ""}
{"terminationReason": "NO_TOOL_CALL", "executionNum": 1, "fullyIdle": true, "continuationActive": false, "error": ""}
```

What the model receives when a background call finishes, and when it
passes its limit:

```
Task id "<conversation ID>/task-7" finished with result:
<the tool's result text>

Task id "<conversation ID>/task-5" failed with result:
Encountered error in tool execution: MCP tool call to server "<server>" timed out after 15s: context deadline exceeded
```

What a resumed conversation's model is told about a wait its earlier
process left open: the task failed with `connection closed: calling
"tools/call": EOF`, and background tasks were stopped by a server restart.
