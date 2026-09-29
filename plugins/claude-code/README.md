# Artifax for Claude Code

Publish HTML pages from Claude Code to a local Artifax server, view them in a
browser, and get the comments people leave on them back into the session.

## Install

No release has been published yet: install from a clone of this repository.
Build the binary, then add the clone as a marketplace:

```
cd /path/to/artifax
just web                                  # once: builds the web UI the binary embeds
cargo install --path crates/artifax-cli   # puts `artifax` in ~/.cargo/bin
```

```
/plugin marketplace add /path/to/artifax
/plugin install artifax@artifax
```

The plugin runs `artifax` through a small launcher, `scripts/ensure-artifax.sh`,
which uses the first of:

1. `ARTIFAX_BIN`, an absolute path to a build;
2. `artifax` on `PATH`;
3. `~/.local/bin/artifax` (or `$ARTIFAX_INSTALL_DIR/artifax`), then
   `~/.artifax/bin/artifax`;
4. a source checkout's `target/release/artifax` or `target/debug/artifax`,
   whichever is newer (see "Working from a source checkout");
5. a download of the latest release, checked against its `.sha256` file
   (which comes from the same place as the tarball, so it protects integrity,
   not authenticity). This is not available until the first release is
   published; until then it fails with a message naming the remedies.

Hooks never download anything: when no binary is found they print one line,
log it to `~/.artifax/logs/hooks.log`, and exit 0, so a missing binary never
fails a Claude Code turn.

## Working from a source checkout

- `cargo install --path crates/artifax-cli` (above) is the simplest: with
  `~/.cargo/bin` on the `PATH` Claude Code starts with, the launcher finds
  `artifax` there. Run it again after changing the Rust code.
- Or build with `cargo build -p artifax-cli` and let the launcher find the
  checkout's `target/debug/artifax` (or `target/release/artifax` when newer).
  It looks above the plugin directory, so this works when Claude Code runs the
  plugin from the checkout itself; when it runs its own copy (under
  `~/.claude/plugins`), set `ARTIFAX_SOURCE_DIR=/path/to/artifax` in the
  environment Claude Code starts with.
- Or set `ARTIFAX_BIN` to a binary.

Claude Code installs the plugin when you run `/plugin install`; after changing
the checkout (the skill, the hooks, the launcher), update the marketplace
(`/plugin marketplace update artifax`) and reinstall the plugin, then start a
new session. `/artifax:doctor` reports a stale copy.

## When something is missing

If the tools are missing, a hook reports an error, or comments do not arrive,
run `/artifax:doctor`, or from a shell

```
artifax doctor --agent claude       # or /path/to/artifax/target/debug/artifax doctor --agent claude
```

It checks each layer and names the fix for each failure: `binary` (which
`artifax` and its version), `plugin` (the installed plugin and whether its
version and launcher match the binary), `skill` (whether the installed skill
states the binary's tool count and is the skill the binary was built with),
`mcp` (whether the daemon has a live Claude Code session, which the MCP server
registers), `hooks` (the latest Claude Code lines in
`~/.artifax/logs/hooks.log`), and `feedback` (each live session's watches and
push state).

`~/.artifax/logs/hooks.log` (under `$ARTIFAX_HOME` when set) has one line per
hook run (agent, event, binary, duration, exit code, and the start of any
error) and one per launcher run that found no binary. It rotates to
`hooks.log.1` past 1 MiB. The `status` tool reports `plugin_version` and
`skew: true` when the plugin and the binary differ.

## What it adds

- The `artifax` MCP server (`artifax mcp --agent claude`): twenty-two tools,
  `publish`, `read`, `list`, `delete`, `open`, `pin`, `unpin`,
  `asset_upload`, `status`, `comments_read`, `comments_reply`,
  `comments_resolve`, `watch`, `wait_for_feedback`, and the data tools
  `db_get`, `db_list`, `db_query`, `db_set`, `db_update`, `db_delete`,
  `db_str_replace`, `db_batch`.
- Hooks (`hooks/hooks.json`), all run as `artifax hook --agent claude <event>`.
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
- The `artifax` skill: when to publish, the page contract, and the comment
  loop.
- Commands: `/artifax:open [ID]`, `/artifax:list`,
  `/artifax:serve [--bind 0.0.0.0|stop|status]`, `/artifax:doctor`,
  `/artifax:comments [ID]` (read an artifact's threads and act on those sent
  to the agent), `/artifax:watch [ID] [off]` (follow an artifact, or stop),
  `/artifax:wait [ID]` (wait for comments and act on each as it arrives).

## The comment loop

People comment on a page in the browser and send a thread to the agent with
**Send to agent** or `@agent`. Those comments reach the session on the next
artifax tool result, at the end of a turn (the `Stop` hook), with the person's
next message (the `UserPromptSubmit` hook), or at once while the agent is in
`wait_for_feedback` (`/artifax:wait`). Claude Code offers plugins no way to
wake an idle session, so nothing arrives between turns unless the agent is
waiting. The thread in the browser shows which of these it is waiting on.
`docs/contract.md` ("Comments and feedback") has the details.

## Maintaining

`scripts/ensure-artifax.sh` here is a copy of `scripts/ensure-artifax.sh` at the
repository root; `scripts/test-plugins.sh` fails when they differ. Update the
root copy and copy it over. The skill's tool list and plugin version are
generated: after adding a tool to `plugins/pi/test/fixtures/contract.json` or
changing the version, run `scripts/sync-skill-tools.py`.

`scripts/smoke-claude.sh` (manual, calls a model) runs a real `claude -p`
session against a scratch daemon to check the whole path.
