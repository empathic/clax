# Clax for Claude Code

Publish HTML pages from Claude Code to a local Clax server, view them in a
browser, and get the comments people leave on them back into the session.

## Install

From a clone of the Clax repository, run `just install`: it installs `clax`
into `~/.cargo/bin` and runs `clax init`, which registers this plugin (the
copy built into that binary, written to `~/.clax/marketplace/`). Then start a
new session. Without a clone, the release installer (`install.sh`, see the
top-level README) puts `clax` in `~/.local/bin`; then run `clax init`.

The plugin runs `clax` from the `PATH` the harness starts with (or
`CLAX_BIN`), through a small wrapper, `scripts/ensure-clax.sh`. The wrapper
never downloads or builds anything. A `clax` whose version differs from the
plugin's runs with a warning in `~/.clax/logs/hooks.log`. Without any `clax`,
the MCP server still starts, with a single tool, `status`, that says why and
how to fix it. Hooks print one line, log it, and exit 0, so a missing binary
never fails a turn.

## Working from a source checkout

`just dev claude` starts a session on a fresh build with this plugin loaded
from the checkout (`claude --plugin-dir`), on a separate home and port;
"Developing Clax" in the top-level README has the details. `just install`
updates your everyday install: the installed plugin is the copy built into
the installed binary, so a change to the skill, the hooks or the wrapper
reaches it only through `just install`. `/clax:doctor` reports a stale copy.

## When something is missing

If the tools are missing, a hook reports an error, or comments do not arrive,
run `/clax:doctor`, or from a shell

```
clax doctor --agent claude
```

It checks each layer and names the fix for each failure: `binary` (this
`clax`, the one the plugins run, and every `clax` on `PATH`), `upgrade`
(whether a failed upgrade keeps the daemon at an older version, until
when, and why), `plugin` (the installed plugin and whether its version and
wrapper match the binary), `skill` (whether the installed skill
states the binary's tool count and is the skill the binary was built with),
`mcp` (whether the daemon has a live Claude Code session, which the MCP server
registers), `hooks` (the latest Claude Code lines in
`~/.clax/logs/hooks.log`), and `feedback` (each live session's watches and
push state).

`~/.clax/logs/hooks.log` (under `$CLAX_HOME` when set) has one line per
hook run (agent, event, binary, duration, exit code, and the start of any
error), one `launch` line per MCP server start (the binary, its version, and
any version warning), and one `launcher` line per wrapper run that could not
run `clax` (the reason and the binaries it tried). It rotates to
`hooks.log.1` past 1 MiB. The `status` tool reports `binary` (the `clax`
answering), `plugin_version`, and `skew: true` when the plugin and the
binary differ.

## What it adds

- The `clax` MCP server (`clax mcp --agent claude`): twenty-three tools,
  `publish`, `read`, `list`, `delete`, `open`, `pin`, `unpin`,
  `asset_upload`, `status`, `comments_read`, `comments_reply`,
  `comments_resolve`, `watch`, `wait_for_feedback`, `working`, and the data tools
  `db_get`, `db_list`, `db_query`, `db_set`, `db_update`, `db_delete`,
  `db_str_replace`, `db_batch`.
- Hooks (`hooks/hooks.json`), run as `clax hook --agent claude <event>`
  (`PostToolUse` through its gate script).
  Hooks never start a daemon (the first tool call does); with none running
  they do nothing.
  - `SessionStart` (`session-start`, 5 s): joins the Claude Code session to
    the one the MCP server registered and adds the daemon URL, and any
    comments already waiting, to the session context.
  - `UserPromptSubmit` (`prompt`, 5 s; gives up after 4 s): adds comments
    sent to this session to the person's next message as context.
  - `Stop` (`stop`, 10 s; gives up after 8 s): when comments sent to this
    session are waiting on an artifact it watches with replies on, blocks the
    stop with them as the reason, so the agent handles them before the turn
    ends. While Claude Code reports `stop_hook_active`, only comments never
    handed over before can block, so each comment blocks a stop at most once.
    When nothing blocks, the turn is over: the page stops showing the agent
    as working.
  - `PostToolUse` (`scripts/tool-hook.sh claude`, 5 s): keeps the page's
    "working" status alive while the agent uses tools. A shell check skips
    starting `clax` unless this session's stamp file
    (`~/.clax/run/tool-hook/`) is at least a minute old, so most tool calls
    cost only a few small shell commands. It always exits 0.
  - `SessionEnd` (`session-end`, 5 s): ends the session.
- The `clax` skill: when to publish, the page contract, and the comment
  loop.
- Commands: `/clax:open [ID]`, `/clax:list`,
  `/clax:serve [--bind 0.0.0.0|stop|status]`, `/clax:doctor`,
  `/clax:comments [ID]` (read an artifact's threads and act on those sent
  to the agent), `/clax:watch [ID] [off]` (follow an artifact, or stop),
  `/clax:wait [ID]` (wait for comments and act on each as it arrives).

## The comment loop

People comment on a page in the browser and send a thread to the agent with
**Send to agent** or `@agent`. Those comments reach the session on the next
clax tool result, at the end of a turn (the `Stop` hook), with the person's
next message (the `UserPromptSubmit` hook), or at once while the agent is in
`wait_for_feedback` (`/clax:wait`). Claude Code offers plugins no way to
wake an idle session, so nothing arrives between turns unless the agent is
waiting. The thread in the browser shows which of these it is waiting on.
`docs/contract.md` ("Comments and feedback") has the details.

## Maintaining

`scripts/ensure-clax.sh` here is a copy of `scripts/ensure-clax.sh` at the
repository root; `scripts/test-plugins.sh` fails when they differ. Update the
root copy and copy it over. The skill's tool list and plugin version are
generated: after adding a tool to `plugins/pi/test/fixtures/contract.json` or
changing the version, run `scripts/sync-skill-tools.py`.

`scripts/smoke-claude.sh` (manual, calls a model) runs a real `claude -p`
session against a scratch daemon to check the whole path.
