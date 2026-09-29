# Artifax for Codex

Publish HTML pages from Codex to a local Artifax server, view them in a
browser, and get the comments people leave on them back into the session.

This directory is `plugins/artifax` rather than `plugins/codex` because a Codex
marketplace entry must point at `./plugins/<plugin-name>`, and the plugin is
named `artifax`. The marketplace is `.agents/plugins/marketplace.json` at the
repository root.

## Install

No release has been published yet: install from a clone of this repository.

```
cd /path/to/artifax
just web                      # once: builds the web UI the binary embeds
cargo build -p artifax-cli    # builds target/debug/artifax
codex plugin marketplace add /path/to/artifax
codex plugin add artifax@artifax
```

`codex mcp list` then shows the `artifax` server. Start a new Codex session to
pick up the tools and the skill.

The plugin runs `artifax` through a small launcher, `scripts/ensure-artifax.sh`,
which uses the first of:

1. `ARTIFAX_BIN`, an absolute path to a build;
2. `artifax` on `PATH`;
3. `~/.local/bin/artifax` (or `$ARTIFAX_INSTALL_DIR/artifax`), then
   `~/.artifax/bin/artifax`;
4. the source checkout's `target/release/artifax` or `target/debug/artifax`,
   whichever is newer (see "Working from a source checkout");
5. a download of the latest release, checked against its `.sha256` file
   (which comes from the same place as the tarball, so it protects integrity,
   not authenticity). This is not available until the first release is
   published; until then it fails with a message naming the remedies.

Hooks never download anything: when no binary is found they print one line,
log it to `~/.artifax/logs/hooks.log`, and exit 0, so a missing binary never
fails a Codex turn.

## Working from a source checkout

- Build once with `cargo build -p artifax-cli`. Codex runs the plugin from its
  own copy (below), so the launcher finds the checkout through the marketplace
  source Codex recorded in `$CODEX_HOME/config.toml` (`[marketplaces.artifax]
  source`), and uses its `target/debug/artifax`, or `target/release/artifax`
  when that is newer. After a rebuild, new sessions use the new binary.
- Or run `cargo install --path crates/artifax-cli`, which puts `artifax` in
  `~/.cargo/bin`; when that is on the `PATH` Codex starts with, it wins over
  the checkout's build.
- Or set `ARTIFAX_BIN` to a binary, or `ARTIFAX_SOURCE_DIR` to a checkout, in
  the environment Codex starts with (`.mcp.json` forwards both to the MCP
  server).

Codex copies the plugin into `$CODEX_HOME/plugins/cache/artifax/artifax/<version>/`
when you run `codex plugin add`, and runs that copy. Changing the checkout (the
skill, the hooks, the launcher) does not change the copy: run
`codex plugin add artifax@artifax` again, then start a new session.
`artifax doctor --agent codex` reports a stale copy.

## When something is missing

If the tools are missing, a hook reports an error, or comments do not arrive,
run

```
artifax doctor --agent codex        # or /path/to/artifax/target/debug/artifax doctor --agent codex
```

It checks each layer and names the fix for each failure: `binary` (which
`artifax` and its version), `plugin` (Codex's installed copy and whether its
version and launcher match the binary), `skill` (whether the installed skill
states the binary's tool count and is the skill the binary was built with),
`mcp` (whether the daemon has a live Codex session, which the MCP server
registers), `hooks` (the latest Codex lines in `~/.artifax/logs/hooks.log`),
`feedback` (each live session's watches and push state), and `codex_push` and
`codex_sessions` (native push, below).

`~/.artifax/logs/hooks.log` (under `$ARTIFAX_HOME` when set) has one line per
hook run (agent, event, binary, duration, exit code, and the start of any
error) and one per launcher run that found no binary. It rotates to
`hooks.log.1` past 1 MiB. The `status` tool reports `plugin_version` and
`skew: true` when the plugin and the binary differ.

## What it adds

- The `artifax` MCP server (`artifax mcp --agent codex`): twenty-two tools,
  `publish`, `read`, `list`, `delete`, `open`, `pin`, `unpin`,
  `asset_upload`, `status`, `comments_read`, `comments_reply`,
  `comments_resolve`, `watch`, `wait_for_feedback`, and the data tools
  `db_get`, `db_list`, `db_query`, `db_set`, `db_update`, `db_delete`,
  `db_str_replace`, `db_batch`, which Codex names `mcp__artifax__<tool>`. The first
  tool call starts the daemon when none is running.
- The `artifax` skill: when to publish, the page contract, and the comment
  loop.
- Hooks (`hooks/hooks.json`), run as `artifax hook --agent codex <event>`:
  `SessionStart` (`session-start`) registers the Codex session with the daemon
  and records its Codex session ID and `CODEX_HOME`, and adds any comments
  already waiting for the session to its context; `Stop` (`stop`, 10 s;
  gives up after 8 s) hands over comments sent to the session at the end of a
  turn; `SessionEnd` (`session-end`) ends the session. Hooks never start a
  daemon. They are optional; see below.

## The comment loop

People comment on a page in the browser and send a thread to the agent with
**Send to agent** or `@agent`. Those comments reach a Codex session:

- on the next artifax tool result;
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
To approve the artifax tools, add to `~/.codex/config.toml`:

```toml
[plugins."artifax@artifax".mcp_servers.artifax]
default_tools_approval_mode = "approve"
```

This approves every artifax tool, including `delete`. `codex exec` runs with
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
- `codex` on the daemon's `PATH`, or named by `ARTIFAX_CODEX_BIN` (an
  executable file). When `ARTIFAX_CODEX_BIN` is set, `PATH` is not searched:
  a value that is not an executable file turns native push off with a reason
  naming it, and the empty string turns it off on purpose. The daemon reads both when it starts, so after changing them run
  `artifax stop` and let the next tool call start it again;
- a Codex TUI attached to the session. `codex exec` sessions, and TUIs that
  have exited, hold queued messages until `codex resume <session ID>`; the
  daemon cannot tell, and those comments are resent on the next tool result
  or at the end of a turn once two minutes pass unacknowledged (see the
  resend rule in `docs/contract.md`).

Only artifacts the session watches with replies on are pushed, and not while
the agent is inside `wait_for_feedback` (which delivers them itself).
`artifax doctor --agent codex` checks the first two: which `codex` the
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
`bash ./scripts/ensure-artifax.sh`. Hooks are run by a shell with `PLUGIN_ROOT`
(and `CLAUDE_PLUGIN_ROOT`) exported, so `hooks/hooks.json` uses
`"${PLUGIN_ROOT}"`.

Codex starts MCP servers with a minimal environment (`HOME`, `PATH`, and a few
others), so `.mcp.json` forwards the Artifax variables through `env_vars`:
`ARTIFAX_HOME`, `ARTIFAX_NO_OPEN`, `ARTIFAX_BIN`, `ARTIFAX_SOURCE_DIR`, `ARTIFAX_INSTALL_DIR`,
`ARTIFAX_CONFIG_DIR`, `ARTIFAX_RELEASE_BASE_URL`, `ARTIFAX_RELEASE_VERSION`,
`ARTIFAX_CODEX_BIN`. A daemon the MCP server starts inherits that environment,
including Codex's `PATH`.

## Maintaining

`scripts/ensure-artifax.sh` here is a copy of `scripts/ensure-artifax.sh` at the
repository root; `scripts/test-plugins.sh` fails when they differ. Update the
root copy and copy it over. `scripts/test-plugins.sh` also runs Codex's plugin
validator when `~/.codex/skills/.system/plugin-creator` is installed.

Codex installs a copy of the plugin under `$CODEX_HOME/plugins/cache`; after
changing files here, run `codex plugin add artifax@artifax` again. The skill's
tool list and plugin version are generated: after adding a tool to
`plugins/pi/test/fixtures/contract.json` or changing the version, run
`scripts/sync-skill-tools.py`.

`scripts/smoke-codex.sh` (manual, calls a model) installs the marketplace and
plugin into a scratch `CODEX_HOME`, runs `codex exec` to publish a page, and
checks the page and the registered session. It copies `~/.codex/auth.json` into
the scratch home for the run and deletes it afterwards. `--hooks` also enables
hooks for that run.
