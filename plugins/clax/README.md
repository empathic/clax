# Clax for Codex

Publish HTML pages from Codex to a local Clax server, view them in a
browser, and get the comments people leave on them back into the session.

This directory is `plugins/clax` rather than `plugins/codex` because a Codex
marketplace entry must point at `./plugins/<plugin-name>`, and the plugin is
named `clax`. The marketplace is `.agents/plugins/marketplace.json` at the
repository root.

## Install

From a clone of the Clax repository (nothing needs building):
`codex plugin marketplace add <clone>`, then `codex plugin add clax@clax`,
and start a new session. `just install` in the clone instead builds and
installs `clax` into `~/.cargo/bin` and runs `clax init`, which registers
this plugin (the copy built into that binary, written to
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

`codex mcp list` then shows the `clax` server. Codex runs plugin hooks only
with `features.hooks = true` and after you trust them (see below).

## Working from a source checkout

`just dev codex` starts a session on a fresh build, on a separate home and
port; "Developing Clax" in the top-level README has the details. Codex cannot
load a plugin from a directory: it runs the copy `codex plugin add` put in
`$CODEX_HOME/plugins/cache/clax/clax/<version>/`, so `just dev codex` uses
your installed plugin. A change to the skill, the hooks or the wrapper
reaches Codex through `just install`, which reinstalls the plugin from the
copy built into the new binary. `clax doctor --agent codex` reports a stale
copy.

## When something is missing

If the tools are missing, a hook reports an error, or comments do not arrive,
run

```
clax doctor --agent codex
```

It checks each layer and names the fix for each failure: `binary` (this
`clax`, and the one the plugins run and why: `CLAX_BIN`, the `bin` setting,
or the pinned release), `upgrade`
(whether a failed upgrade keeps the daemon at an older version, until
when, and why), `plugin` (Codex's installed copy and whether its version and
wrapper match the binary), `skill` (whether the installed skill
states the binary's tool count and is the skill the binary was built with),
`mcp` (whether the daemon has a live Codex session, which the MCP server
registers), `hooks` (the latest Codex lines in `~/.clax/logs/hooks.log`),
`feedback` (each live session's watches and push state), and `codex_push` and
`codex_sessions` (native push, below).

`~/.clax/logs/hooks.log` (under `$CLAX_HOME` when set) has one line per
hook run (agent, event, binary, duration, exit code, and the start of any
error), one `launch` line per MCP server start (the binary, its version, and
any version warning), and one `launcher` line per wrapper run that could not
run `clax` (the reason and the binaries it tried). It rotates to
`hooks.log.1` past 1 MiB. The `status` tool reports `binary` (the `clax`
answering), `plugin_version`, and `skew: true` when the plugin and the
binary differ.

## What it adds

- The `clax` MCP server (`clax mcp --agent codex`): twenty-three tools,
  `publish`, `read`, `list`, `delete`, `open`, `pin`, `unpin`,
  `asset_upload`, `status`, `comments_read`, `comments_reply`,
  `comments_resolve`, `watch`, `wait_for_feedback`, `working`, and the data tools
  `db_get`, `db_list`, `db_query`, `db_set`, `db_update`, `db_delete`,
  `db_str_replace`, `db_batch`, which Codex names `mcp__clax__<tool>`. The first
  tool call starts the daemon when none is running.
- The `clax` skill: when to publish, the page contract, and the comment
  loop.
- Hooks (`hooks/hooks.json`), run as `clax hook --agent codex <event>`:
  `SessionStart` (`session-start`) registers the Codex session with the daemon
  and records its Codex session ID and `CODEX_HOME`, and adds any comments
  already waiting for the session to its context; `Stop` (`stop`, 10 s;
  gives up after 8 s) hands over comments sent to the session at the end of a
  turn, and when nothing is waiting ends the page's "working" status;
  `PostToolUse` (`scripts/tool-hook.sh codex`, 5 s) keeps that status alive,
  starting `clax` at most once a minute (not yet measured on Codex; see
  `docs/contract.md`); `SessionEnd` (`session-end`) ends the session. Hooks never start a
  daemon. They are optional; see below.

## The comment loop

People comment on a page in the browser and send a thread to the agent with
**Send to agent** or `@agent`. Those comments reach a Codex session:

- on the next clax tool result;
- when a session starts, from the `SessionStart` hook, if comments were
  already waiting for it;
- at the end of a turn, from the `Stop` hook, which blocks the stop with the
  comments as the reason so Codex continues the turn with them (only on
  artifacts the session watches with replies on);
- at once while the agent is in `wait_for_feedback`, which returns within
  Codex's 60 s tool-call limit and is called again;
- pushed with `codex queue` (native push, below).

The thread in the browser shows which of these it is waiting on.
`docs/contract.md` ("Comments and feedback") has the details.

### Tool approval

Codex asks before each MCP tool call unless the server's tools are approved.
To approve the clax tools, add to `~/.codex/config.toml`:

```toml
[plugins."clax@clax".mcp_servers.clax]
default_tools_approval_mode = "approve"
```

This approves every clax tool, including `delete`. `codex exec` runs with
approval policy `never` and refuses tool calls that would prompt, so
non-interactive use needs this setting.

### Hooks

Codex runs plugin hooks only when hooks are enabled:

```toml
[features]
hooks = true
```

and each hook is trusted. Codex asks you to review and trust the plugin's hooks
the first time it finds them in an interactive session; `codex exec` skips
untrusted hooks. Without hooks the tools still work: the MCP server registers
the session itself, and comments arrive on tool results and from
`wait_for_feedback`. With hooks, the session also carries Codex's own session
ID, and comments also arrive at the end of a turn and by native push.

### Native push

When a comment is sent to a Codex session, the daemon runs

```
codex queue --thread <Codex session ID> --message <the comments>
```

An idle Codex TUI with that session open starts a turn with the message at
once; a busy one runs it as its next turn. This needs:

- the hooks, enabled and trusted, so the daemon knows the Codex session ID
  (the `SessionStart` hook sends it) and the session's `CODEX_HOME` (passed
  through from the hook's environment when set; otherwise `codex` uses its
  default home);
- `codex` on the daemon's `PATH`, or named by `CLAX_CODEX_BIN` (an
  executable file). When `CLAX_CODEX_BIN` is set, `PATH` is not searched:
  a value that is not an executable file turns native push off with a reason
  naming it, and the empty string turns it off on purpose. The daemon reads both when it starts, so after changing them run
  `clax stop` and let the next tool call start it again;
- a Codex TUI attached to the session. `codex exec` sessions, and TUIs that
  have exited, hold queued messages until `codex resume <session ID>`; the
  daemon cannot tell, and those comments are resent on the next tool result
  or at the end of a turn once two minutes pass unacknowledged (see the
  resend rule in `docs/contract.md`).

Only artifacts the session watches with replies on are pushed, and not while
the agent is inside `wait_for_feedback` (which delivers them itself).
`clax doctor --agent codex` checks the first two: which `codex` the
daemon found and where from, and whether every live Codex session has its
Codex session ID. The `status` tool's `push` field says whether push is on for
the current session and why not. If `codex queue` fails (exits non-zero, is
killed, times out after 10 s, or cannot start), the session stays live, its
comments fall back to the other tiers, and `push.last_error` and
`push.last_error_at` say what happened.

## How the plugin finds its files

Codex 0.158 does not expand `${PLUGIN_ROOT}`, `${CLAUDE_PLUGIN_ROOT}`, or
`${CODEX_PLUGIN_ROOT}` in a plugin's `.mcp.json` `command`, `args`, or `env`,
and does not export them to MCP servers. It does resolve a relative `cwd`
against the installed plugin root, so `.mcp.json` sets `"cwd": "./"` and runs
`bash ./scripts/ensure-clax.sh`. Hooks are run by a shell with `PLUGIN_ROOT`
(and `CLAUDE_PLUGIN_ROOT`) exported, so `hooks/hooks.json` uses
`"${PLUGIN_ROOT}"`.

Codex starts MCP servers with a minimal environment (`HOME`, `PATH`, and a few
others), so `.mcp.json` forwards the Clax variables through `env_vars`:
`CLAX_HOME`, `CLAX_NO_OPEN`, `CLAX_BIN` and `CLAX_CODEX_BIN`. A daemon the MCP server starts inherits that environment,
including Codex's `PATH`.

## Maintaining

`scripts/ensure-clax.sh` here is a copy of `scripts/ensure-clax.sh` at the
repository root; `scripts/test-plugins.sh` fails when they differ. Update the
root copy and copy it over. `scripts/test-plugins.sh` also runs Codex's plugin
validator when `~/.codex/skills/.system/plugin-creator` is installed.

Codex installs a copy of the plugin under `$CODEX_HOME/plugins/cache`, from
the copy built into the binary; after changing files here, run
`just install`. The skill's tool list and plugin version are generated: after
adding a tool to `plugins/pi/test/fixtures/contract.json` or changing the
version, run `scripts/sync-skill-tools.py`.

`scripts/smoke-codex.sh` (manual, calls a model) installs the marketplace and
plugin into a scratch `CODEX_HOME`, runs `codex exec` to publish a page, and
checks the page and the registered session. It copies `~/.codex/auth.json` into
the scratch home for the run and deletes it afterwards. `--hooks` also enables
hooks for that run.
