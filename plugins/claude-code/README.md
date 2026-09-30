# Clax for Claude Code

Publish HTML pages from Claude Code to a local Clax server, view them in a
browser, and get the comments people leave on them back into the session.

## Install

No release has been published yet: install from a clone of this repository.
Build the binary, then add the clone as a marketplace:

```
cd /path/to/clax
just web                                  # once: builds the web UI the binary embeds
cargo install --path crates/clax-cli      # puts `clax` in ~/.cargo/bin
```

```
/plugin marketplace add /path/to/clax
/plugin install clax@clax
```

The plugin runs `clax` through a small launcher, `scripts/ensure-clax.sh`,
which uses the first of:

1. `CLAX_BIN`, an absolute path to a build;
2. `clax` on `PATH`;
3. `~/.local/bin/clax` (or `$CLAX_INSTALL_DIR/clax`), then
   `~/.clax/bin/clax`;
4. a source checkout's `target/release/clax` or `target/debug/clax`,
   whichever is newer (see "Working from a source checkout");
5. a download of the latest release, checked against its `.sha256` file
   (which comes from the same place as the tarball, so it protects integrity,
   not authenticity). This is not available until the first release is
   published; until then it fails with a message naming the remedies.

Hooks never download anything: when no binary is found they print one line,
log it to `~/.clax/logs/hooks.log`, and exit 0, so a missing binary never
fails a Claude Code turn.

## Working from a source checkout

- `cargo install --path crates/clax-cli` (above) is the simplest: with
  `~/.cargo/bin` on the `PATH` Claude Code starts with, the launcher finds
  `clax` there. Run it again after changing the Rust code.
- A plugin installed from a marketplace (`/plugin install`) runs from Claude
  Code's own copy under `~/.claude/plugins/cache`, not from the checkout, so
  the launcher cannot find the checkout's build by itself: install with
  `cargo install` (above), or set `CLAX_SOURCE_DIR=/path/to/clax` in the
  environment Claude Code starts with.
- Only when Claude Code loads the plugin straight from the checkout
  (`claude --plugin-dir /path/to/clax/plugins/claude-code`) does
  `cargo build -p clax-cli` suffice: the launcher looks above the plugin
  directory and uses the checkout's `target/debug/clax` (or
  `target/release/clax` when newer).
- Or set `CLAX_BIN` to a binary.

Claude Code installs the plugin when you run `/plugin install`; after changing
the checkout (the skill, the hooks, the launcher), update the marketplace
(`/plugin marketplace update clax`) and reinstall the plugin, then start a
new session. `/clax:doctor` reports a stale copy.

## When something is missing

If the tools are missing, a hook reports an error, or comments do not arrive,
run `/clax:doctor`, or from a shell

```
clax doctor --agent claude       # or /path/to/clax/target/debug/clax doctor --agent claude
```

It checks each layer and names the fix for each failure: `binary` (which
`clax` and its version), `plugin` (the installed plugin and whether its
version and launcher match the binary), `skill` (whether the installed skill
states the binary's tool count and is the skill the binary was built with),
`mcp` (whether the daemon has a live Claude Code session, which the MCP server
registers), `hooks` (the latest Claude Code lines in
`~/.clax/logs/hooks.log`), and `feedback` (each live session's watches and
push state).

`~/.clax/logs/hooks.log` (under `$CLAX_HOME` when set) has one line per
hook run (agent, event, binary, duration, exit code, and the start of any
error) and one per launcher run that found no binary. It rotates to
`hooks.log.1` past 1 MiB. The `status` tool reports `plugin_version` and
`skew: true` when the plugin and the binary differ.

## What it adds

- The `clax` MCP server (`clax mcp --agent claude`): twenty-two tools,
  `publish`, `read`, `list`, `delete`, `open`, `pin`, `unpin`,
  `asset_upload`, `status`, `comments_read`, `comments_reply`,
  `comments_resolve`, `watch`, `wait_for_feedback`, and the data tools
  `db_get`, `db_list`, `db_query`, `db_set`, `db_update`, `db_delete`,
  `db_str_replace`, `db_batch`.
- Hooks (`hooks/hooks.json`), all run as `clax hook --agent claude <event>`.
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
