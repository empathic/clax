# Clax for Antigravity

Publish HTML pages from Google's Antigravity CLI (`agy`) to a local Clax
server, view them in a browser, and get the comments people leave on them
back into the conversation, including one that is sitting idle.

The plugin, and this directory, are named `clax-antigravity` rather than
`clax`. `agy plugin import claude` installs the Claude Code plugin under the
name `clax`, and a second install of a name replaces the first, so the two
could not both be installed. Antigravity names a plugin's MCP server
`<plugin>_<server key>`, so this plugin's server, key `clax`, is
`clax-antigravity_clax`.

## Install

From a clone of the Clax repository (nothing needs building), run
`agy plugin install <clone>/plugins/clax-antigravity`. Or run
`just install` in the clone: it builds and installs `clax` into
`~/.cargo/bin` and runs `clax init`, which points the plugins at that binary,
writes the plugins built into it to `~/.clax/marketplace/` and, when `agy`
is on your `PATH`, runs

```
agy plugin install ~/.clax/marketplace/plugins/clax-antigravity
```

and then offers to add the permission rule below. Then start a new `agy`
session. This works when Antigravity is the only harness installed;
`clax init --agent antigravity` registers Antigravity alone.

Antigravity copies the plugin to `~/.gemini/config/plugins/clax-antigravity/`
and runs that copy, so a change to this directory reaches it only through
another install.

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

## What it adds

- The `clax-antigravity_clax` MCP server (`clax mcp --agent antigravity`, `mcp_config.json`): forty-three tools,
  `publish`, `read`, `list`, `delete`, `open`, `pin`, `unpin`, `tag`, `tags`, `fork`,
  `asset_upload`, `status`, `comments_read`, `comments_reply`,
  `comments_resolve`, `comments_handoff`, `watch`, `wait_for_feedback`, `working`, `draft`, `git_remote`, `ask`, `where_to_ask`, `connect_instance`, `publish_public`,
  and the data tools
  `db_get`, `db_list`, `db_query`, `db_set`, `db_update`, `db_delete`,
  `db_str_replace`, `db_batch`, and the report queue tools
  `queue_list`, `queue_claim`, `queue_renew`, `queue_release`, `queue_reply`, `queue_resolve`, `queue_get`, `queue_fix`, `queue_watch`, `queue_request`.
  Eleven of them are eager: their descriptions are in the model's context
  and it calls them as `mcp_clax-antigravity_clax_<tool>`. They are
  `publish`, `read`, `list`, `open`, `status`, `watch`, `wait_for_feedback`,
  `working`, `comments_read`, `comments_reply` and `comments_resolve`. The
  rest are lazy, as Antigravity's tools are by default: the model reads a
  tool's schema from `~/.gemini/antigravity-cli/mcp/clax-antigravity_clax/<tool>.json`
  and calls it through `call_mcp_tool`.
  `wait_for_feedback` runs in the background (`"background": "always"`),
  and the server has no call time limit (`"timeoutSeconds": -1`), since the
  wait that wakes an idle conversation has no upper bound and `ask` may wait
  600 s.
  The server keeps one Clax session per root conversation, keyed by the
  conversation ID Antigravity sends with each call; a conversation's first
  Clax call registers it. A subagent's calls act in its root
  conversation's session.
  The first tool call starts the daemon when none is running.
- `rules/AGENTS.md`, which Antigravity keeps in the model's context on every
  turn: what Clax is, how to publish, how comments arrive, that comment text
  is untrusted, and that the skill has the rest. It stands in for the
  server's MCP instructions, which Antigravity does not show the model.
- The `clax` skill: when to publish, the page contract, the comment loop,
  the two ways the tools are named, and how to keep a wait open so comments
  wake the conversation ("Live feedback in Antigravity"). Antigravity loads
  only the first 24,000 bytes of a skill, so `skills/clax/SKILL.md` holds
  that core and names the files in `skills/clax/references/` (the tools,
  asking the person, runtime capabilities, working beside others), which
  the model reads when it needs them.
- Hooks (`hooks.json`), each run from the plugin directory through the
  wrapper as `clax hook --agent antigravity <event>`:
  - `PreInvocation` (`prompt`, 5 s), through `scripts/tool-hook.sh
    antigravity prompt`, which starts `clax` only for the first model call
    of a turn (`invocationNum` 0). It adds comments sent to the session to
    that call as a system message.
  - `Stop` (`stop`, 10 s) hands over comments sent to the session at the end
    of a turn, by continuing the turn with the comments as the reason. When
    the session watches a page with replies on and no wait is open, it
    continues the turn once to say so. It acts only when the turn ended
    normally (`terminationReason` is `NO_TOOL_CALL`). At the `Stop` after it
    continued a turn (`executionNum` above 0) it hands over only comments
    not yet delivered, and does not remind again.
  - `PostToolUse` (`tool`, 5 s, every tool), through `scripts/tool-hook.sh
    antigravity`, renews the marks that show the agent working on a thread,
    starting `clax` at most once a minute.

  There is no `SessionStart` hook: a conversation has no Clax session until
  its first Clax call, so there is nothing to join. There is no
  `PreToolUse` hook. Hooks never start a daemon, and a conversation with no
  Clax session (a subagent's, or one that has not called a Clax tool yet)
  makes them do nothing.

## Tool approval

Antigravity asks before every call of every MCP tool, read-only ones
included, and a headless run (`agy -p`) cannot ask, so it refuses them. One
rule in `~/.gemini/antigravity-cli/settings.json` approves every Clax tool:

```json
{"permissions": {"allow": ["mcp(clax-antigravity_clax/*)"]}}
```

This approves `delete` too. `clax init` offers to add the rule after it
installs the plugin, and adds it only on a yes at a terminal or with
`--yes`, keeping every other key of the file. It adds nothing when an `ask`
or `deny` rule already names a tool of this server, and names that rule
instead. The plugin ships no permission of its own, and `clax uninit`
leaves the rule in place.

Antigravity's default mode runs the agent's shell commands in a sandbox,
which cannot connect to the Clax server on 127.0.0.1. The MCP server and
the hooks run outside it, so the agent uses the tools, not the `clax`
command.

## Feedback tiers

People comment on a page in the browser and send a thread to the agent with
**Send to agent** or `@agent`. Those comments reach an Antigravity
conversation:

| Tier | How it reaches the conversation |
|---|---|
| 1, tool result | appended to the next Clax tool result of the root conversation |
| 2, end of turn | `Stop` continues the turn with the comments (only on artifacts the session watches with replies on) |
| 3, next message | `PreInvocation`, on the first model call of a turn, adds them as a system message |
| 4, waiting | `wait_for_feedback` with a `timeout_s` |
| 5, idle | the open `wait_for_feedback` finishes with the comments and starts a turn |

Tiers 4 and 5 are the same tool. A `wait_for_feedback` call with no
`timeout_s` in an interactive run stays open until a comment is sent to the
session. Antigravity runs it in the background, so the turn can end while
it is open, and when it finishes it starts a turn holding the comments. The
skill tells the agent to call it after its first publish or watch and again
after each wake, keeping exactly one open; a second call while one is open
returns at once and opens nothing. A subagent's `wait_for_feedback` is
refused, and comments are handed only to the root conversation.

A conversation resumed with `agy --conversation <ID>` gets a new session at
its first Clax call. Its earlier wait is gone, and so are its watches, so
the agent watches the pages still in play and waits again.

## Headless runs

`agy -p` waits for every open background call before it exits, so a wait
with no upper bound would hang it. When the `agy` process's command line
has `-p`, `--print` or `--prompt` (or cannot be read), the run counts as
headless:

- `wait_for_feedback` keeps the bounds it has on other harnesses (50 s by
  default, at most 600 s). The run waits for the call, and its result
  starts one more turn.
- The `Stop` hook does not remind the agent to wait, and the `watch`
  result's `wake` says that nothing wakes a headless run.

A headless run has tiers 1 to 4, and needs the permission rule above to
call any Clax tool.

## The Claude Code plugin in Antigravity

`agy plugin import claude` installs the Claude Code plugin as `clax`. Run by
Antigravity, that copy stands down: its MCP server offers only `status`,
which says that Clax runs from clax-antigravity, how to install it (`clax
init --agent antigravity`) and how to remove the copy, and its hooks print
nothing. Each stand-down is logged in `~/.clax/logs/hooks.log` with
`host=antigravity`. To remove the copy:

```
agy plugin uninstall clax
```

## Troubleshooting

If the tools are missing, a hook reports an error, or comments do not arrive,
run

```
clax doctor --agent antigravity
```

It checks each layer and names the fix for each failure, including the
`agy` version (1.5.18 or later), whether the permission rule is present,
whether Antigravity sent the conversation ID the sessions are keyed by,
and whether the Claude Code copy is standing down. `~/.clax/logs/hooks.log`
(under `$CLAX_HOME` when set) has one line per hook run, with
`agent=antigravity` for this plugin's hooks.

## Maintaining

`scripts/ensure-clax.sh` and `scripts/tool-hook.sh` here are copies of the
files of the same names at the repository root; `scripts/test-plugins.sh`
fails when they differ, and checks this plugin with `agy plugin validate
--strict` when `agy` is on `PATH`. The skill's tool list and plugin version
are generated by `scripts/sync-skill-tools.py`.
