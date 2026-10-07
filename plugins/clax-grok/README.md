# Clax for Grok Build

Publish HTML pages from Grok Build to a local Clax server, view them in a
browser, and get the comments people leave on them back into the session.

The plugin, and this directory, are named `clax-grok` rather than `clax`.
Grok also discovers the Claude Code plugin, which is named `clax`, and it
resolves plugin-name conflicts before it enables a plugin, so two plugins
named `clax` would not both load. Its MCP server is
named `clax_grok` for the same kind of reason: Grok merges the MCP servers of
every source into one map and keeps the first definition of a name, so a
server named `clax` could be hidden behind the Claude Code copy's server. The
repository root carries `.grok-plugin/marketplace.json`, which lists only
this plugin.

## Install

From a clone of the Clax repository (nothing needs building), run
`grok plugin install <clone>/plugins/clax-grok --trust`. Or run
`just install` in the clone: it builds and installs `clax` into
`~/.cargo/bin` and runs `clax init`, which points the plugins at that binary,
writes the plugins built into it to
`~/.clax/marketplace/` and, when `grok` is on your `PATH`, runs

```
grok plugin install ~/.clax/marketplace/plugins/clax-grok --trust
```

Then start a new Grok session. This works when Grok is the only harness
installed; `clax init --agent grok` registers Grok alone.

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

- The `clax_grok` MCP server (`clax mcp --agent grok`): twenty-four tools,
  `publish`, `read`, `list`, `delete`, `open`, `pin`, `unpin`,
  `asset_upload`, `status`, `comments_read`, `comments_reply`,
  `comments_resolve`, `watch`, `wait_for_feedback`, `working`, `ask`, and the data tools
  `db_get`, `db_list`, `db_query`, `db_set`, `db_update`, `db_delete`,
  `db_str_replace`, `db_batch`, which Grok names `clax_grok__<tool>` and reaches through `use_tool`.
  The first tool call starts the daemon when none is running.
- The `clax` skill: when to publish, the page contract, the comment loop,
  and how to start the monitor that wakes the session for comments ("Live
  feedback in Grok").
- Hooks (`hooks/hooks.json`), each run as `clax hook --agent grok <event>`
  through the wrapper:
  - `SessionStart` (`session-start`, 5 s) joins the Grok session to the
    daemon's by Grok's session ID and records its working directory. It
    prints nothing: Grok ignores `SessionStart` output.
  - `Stop` (`stop`, 10 s) hands over comments sent to the session at the end
    of a turn, by blocking the stop with the comments as the reason. It acts
    only when the turn ended normally (`reason` is `end_turn` or absent) and
    honours Grok's `stopHookActive`.
  - `SessionEnd` (`session-end`, 2 s) ends the session. It gives up after
    1.2 s, inside Grok's default 1.5 s budget for that event.

  There is no `UserPromptSubmit` hook: Grok discards an allowing one's
  output, so it could not add comments to a prompt. Hooks never start a
  daemon.

## The Claude Code plugin in Grok

Grok lists the Claude Code plugin as a disabled User-scope plugin named
`clax`. If you enable it, it stands down: its MCP server offers only
`clax__status`, which says that Clax runs from clax-grok, and its hooks read
their input and exit 0 without doing anything. Clax never disables it for
you, since in Grok the name `clax` is Claude Code's install. To remove it
from Grok:

```
grok plugin disable clax
```

What one Grok session gets in each combination:

| Claude Code plugin in Grok | clax-grok | MCP servers Grok starts | Acting server and hooks | What you see |
|---|---|---|---|---|
| disabled (Grok's default) | enabled | `clax_grok` | clax-grok's | normal |
| enabled | enabled | `clax` (stand-down) and `clax_grok` | clax-grok's | normal, plus an idle `clax__status`; `clax doctor --agent grok` suggests `grok plugin disable clax` |
| enabled | not installed, or disabled | `clax` (stand-down) | none | `clax__status` says to run `clax init --agent grok`; the server shows as connected, not failed |
| disabled | not installed | none | none | no Clax; `clax doctor --agent grok` says the plugin is not installed |

## Settings you may want

These are provisional: they are read from Grok's guide and source, and not
yet confirmed against a release (Q1, Q2 in the Grok Build open questions).
Clax sets none of them.

- **Tool approval.** Grok asks before each MCP tool call in its default
  mode. To approve every Clax tool, add to `~/.grok/config.toml`:

  ```toml
  [permission]
  allow = ["MCPTool(clax_grok__*)"]
  ```

  This approves `delete` too. Headless `grok -p` needs this rule or
  `--always-approve`, since it cannot ask.
- **Sandbox.** When Grok's sandbox is on, it covers the MCP server, the
  hooks and any daemon they start. Start the daemon outside Grok
  (`clax serve`), or use a custom sandbox profile with
  `read_write = ["~/.clax"]`.

## Feedback tiers

People comment on a page in the browser and send a thread to the agent with
**Send to agent** or `@agent`. Those comments reach a Grok session:

- **Tier 1:** on the next Clax tool result.
- **Tier 2:** at the end of a turn, from the `Stop` hook (only on artifacts
  the session watches with replies on).
- **Tier 4:** at once while the agent is in `wait_for_feedback`.
- **Tier 5:** through the monitor the skill starts. After its first publish
  in a session, the agent runs Grok's `monitor` tool on `clax feedback
  follow`, which prints one notice line per comment. Each line wakes an idle
  session (a busy one sees it after the current turn). The line names the
  artifact and thread but not the comment; the comment itself arrives with
  the next `comments_read`, at the end of the turn, or in
  `wait_for_feedback`, so it is delivered once.

There is no tier 3 (comments added to the person's next message): Grok
discards an allowing `UserPromptSubmit` hook's output and ignores
`SessionStart` output. Headless `grok -p` gets no monitor, so it relies on
tiers 1, 2 and 4.

## Working from a source checkout

`just dev grok` runs Grok with the installed clax-grok plugin, the fresh
build named in `CLAX_BIN`, and `CLAX_HOME=~/.clax-dev`. Grok runs its installed
copy of the plugin, so to try changes to the skill, the hooks or the wrapper,
run `just install`, which reinstalls the plugin from the copy built into the
new binary.

## Troubleshooting

If the tools are missing, a hook reports an error, or comments do not arrive,
run

```
clax doctor --agent grok
```

It checks each layer and names the fix for each failure, including whether
the Claude Code plugin is enabled in Grok. `~/.clax/logs/hooks.log` (under
`$CLAX_HOME` when set) has one line per hook run, with `agent=grok` for this
plugin's hooks, and one `standdown` line each time the Claude Code copy
stands down in Grok.

## Maintaining

`scripts/ensure-clax.sh` here is a copy of `scripts/ensure-clax.sh` at the
repository root; `scripts/test-plugins.sh` fails when they differ. The
skill's tool list and plugin version are generated by
`scripts/sync-skill-tools.py`.
