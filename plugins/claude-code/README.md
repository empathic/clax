# Clax for Claude Code

Publish HTML pages from Claude Code to a local Clax server, view them in a
browser, and get the comments people leave on them back into the session.

## Install

`/plugin marketplace add empathic/clax`, then `/plugin install clax@clax`.
Once Claude Code reports the plugin active, its tools work in that
session; the `SessionStart` hook runs from the next session on. From a clone of the Clax repository, `just install`
instead builds and installs `clax` into `~/.cargo/bin` and runs `clax init`,
which registers this plugin (the copy built into that binary, written to
`~/.clax/marketplace/`) and points it at that binary.

The plugin needs no separate install of `clax`. Its wrapper,
`scripts/ensure-clax.sh`, runs `CLAX_BIN` when set; else the `bin` setting in
`~/.clax/config.toml` (`clax bin set <path>`, which `clax init` sets to
itself); else the Clax release the plugin pins, which the MCP server's first
start downloads into `~/.clax/bin/<version>/`, checks against the checksum
the plugin carries, and runs. `PATH` is never consulted. Hooks never
download: until the release is installed they exit 0 and do nothing. A
binary named by `CLAX_BIN` or the `bin` setting whose version differs from
the plugin's runs with a warning in `~/.clax/logs/hooks.log`. When no
`clax` can run, the MCP server still starts, with a single tool, `status`,
that says why and how to fix it, and the hooks exit 0, so a missing binary
never fails a turn.

## Allowing the tools

In Claude Code's Manual mode (`default`) every MCP tool asks before it
runs, and a plugin cannot pre-approve its own tools, so each Clax call,
even `status`, prompts until you allow it (in auto mode the classifier
decides instead). Add allow rules to `permissions.allow` in your user
settings (`~/.claude/settings.json`, every project), a project's
`.claude/settings.json` (shared with the repository) or its
`.claude/settings.local.json` (yours alone). At least the tools that only
read:

```json
{
  "permissions": {
    "allow": [
      "mcp__plugin_clax_clax__status",
      "mcp__plugin_clax_clax__list",
      "mcp__plugin_clax_clax__read",
      "mcp__plugin_clax_clax__comments_read",
      "mcp__plugin_clax_clax__db_get",
      "mcp__plugin_clax_clax__db_list",
      "mcp__plugin_clax_clax__db_query"
    ]
  }
}
```

`"mcp__plugin_clax_clax__*"` allows every Clax tool, publishing and
deleting included (Claude Code names a plugin's tools
`mcp__plugin_<plugin>_<server>__<tool>`; here both are `clax`).
`/permissions` adds the same rules from inside a session.

Behind an LLM gateway or proxy (`ANTHROPIC_BASE_URL`), auto mode's
classifier may get no verdict and deny tool calls. Allow rules settle
Clax's tools before the classifier is asked; Manual mode (Shift+Tab) also
works.

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
`clax`, and the one the plugins run and why: `CLAX_BIN`, the `bin` setting,
or the pinned release), `upgrade`
(whether a failed upgrade keeps the daemon at an older version, until
when, and why), `plugin` (the installed plugin and whether its version and
wrapper match the binary), `skill` (whether the installed skill
states the binary's tool count and is the skill the binary was built with),
`mcp` (whether the daemon has a live Claude Code session, which the MCP server
registers), `hooks` (the latest Claude Code lines in
`~/.clax/logs/hooks.log`), and `feedback` (each live session's watches and
push state).

When another program already listens on Clax's port (7480 unless
`[serve] port` in `~/.clax/config.toml` or `CLAX_PORT` says otherwise), the
daemon takes the next free port of the following 20, and the URLs it
returns carry that port. Only when all 21 are held, or the port you set
with `CLAX_PORT` is, does the MCP server start with just `status`, which
names the port and a free one to set instead. Clax never stops what holds a
port. After changing the setting, reconnect the server with `/mcp`.

`~/.clax/logs/hooks.log` (under `$CLAX_HOME` when set) has one line per
hook run (agent, event, binary, duration, exit code, and the start of any
error), one `launch` line per MCP server start (the binary, its version, and
any version warning), and one `launcher` line per wrapper run that could not
run `clax` (the reason and the binaries it tried). It rotates to
`hooks.log.1` past 1 MiB. The `status` tool reports `binary` (the `clax`
answering), `plugin_version`, and `skew: true` when the plugin and the
binary differ.

## What it adds

- The `clax` MCP server (`clax mcp --agent claude`): twenty-four tools,
  `publish`, `read`, `list`, `delete`, `open`, `pin`, `unpin`,
  `asset_upload`, `status`, `comments_read`, `comments_reply`,
  `comments_resolve`, `watch`, `wait_for_feedback`, `working`, `ask`, and the data tools
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
  `/clax:extension` sets up the Clax Chrome extension (see below).

## The Chrome extension

The Clax Chrome extension puts the comment overlay on any web page, such as
your dev server, so you can comment there and the agent can `watch` that
page's URL. `clax init` installs it; a plugin-only install sets it up with
`/clax:extension`, which runs `clax extension install` through the wrapper.
That writes the extension to `~/.clax/extension/` and registers its native
messaging host (`dev.empathic.clax`, which pairs the extension with the
local daemon) with each installed Chrome, Chromium, Brave and Edge on macOS
and Linux. Then, once, open chrome://extensions, turn on Developer mode,
choose Load unpacked, and pick `~/.clax/extension`. `clax extension status`
and `/clax:doctor` report the setup; `clax uninit` (or `clax extension
uninstall`) removes it. Snap and Flatpak Chromium on Linux cannot run the
native host, so the extension cannot pair there.

Until a Chrome Web Store listing exists, the extension's ID comes from where
it is installed, or from the public key in `web/extension/key/key.pub.b64`
once the owner commits one. Signing is the owner's alone and never runs in
CI: the private key lives only in the owner's 1Password, created there
directly (nothing on disk), and `CLAX_EXTENSION_KEY_REF` holds its `op://`
reference. `scripts/extension-pubkey.sh` writes the public key and prints
the ID (commit the file, rebuild and reinstall clax with `just install`
since the key is built in, then run `clax init`; the ID changes once), and
`scripts/pack-extension.sh` builds the Web Store zip (its manifest without
`key`, which the store refuses), with `--first-upload` (the private key as
`key.pem` at the zip's root) for the listing's first upload only. The
steps are in `docs/verification.md` §8.4.

## The comment loop

People comment on a page in the browser and send a thread to the agent with
**Send to agent** or `@agent`. Those comments reach the session on the next
clax tool result, at the end of a turn (the `Stop` hook), with the person's
next message (the `UserPromptSubmit` hook), or at once while the agent is in
`wait_for_feedback` (`/clax:wait`). The thread in the browser shows which of
these it is waiting on.

Neither of the following is needed for comments to arrive; they only wake
a session that is idle, so a comment does not wait for your next message.
An idle session wakes on a new comment in one of two ways:

- **The Clax channel.** Launch Claude Code with

  ```sh
  claude --dangerously-load-development-channels plugin:clax@clax
  ```

  and accept the development-channels warning; the startup screen then says
  messages from `plugin:clax@clax` inject into the session. Clax sends one
  notice through the channel per comment, which starts a turn when the
  session is idle and joins the next turn when it is busy. Channels are a
  Claude Code research preview: CLI only, with a claude.ai or Console login,
  and on Team and Enterprise an Owner must turn them on. Clax never asks for
  permission relay, so people who comment cannot approve tool use.
- **The background follower.** Without the channel, the skill has the agent
  run `clax feedback follow --once` in the background after it publishes,
  and start it again after each exit. The command exits when a comment
  arrives, and its exit wakes the session.

A notice only points at the comment. The comment itself still arrives once,
through the next clax tool result, the end of the turn, or
`wait_for_feedback`. `status` (its `push` field) and
`clax doctor --agent claude` show which path a session uses.
`docs/contract.md` ("Comments and feedback") has the details.

## In Grok Build

Grok Build discovers the plugins Claude Code has installed, this one
included. Clax runs in Grok from its own plugin, clax-grok (`clax init
--agent grok`), whose server is `clax_grok`. When Grok runs this plugin, it
stands down: its MCP server offers only `status`, which points at the
`clax_grok__*` tools and at `clax init --agent grok` when they are missing,
and its hooks read their input and exit without acting. It never acts as
Grok's Clax, whether or not clax-grok is loaded. To drop it from Grok, run
`grok plugin disable clax`; `clax doctor --agent grok` reports it as
`claude_copy`.

## Maintaining

`scripts/ensure-clax.sh` here is a copy of `scripts/ensure-clax.sh` at the
repository root; `scripts/test-plugins.sh` fails when they differ. Update the
root copy and copy it over. The skill's tool list and plugin version are
generated: after adding a tool to `plugins/pi/test/fixtures/contract.json` or
changing the version, run `scripts/sync-skill-tools.py`.

`scripts/smoke-claude.sh` (manual, calls a model) runs a real `claude -p`
session against a scratch daemon to check the whole path.
