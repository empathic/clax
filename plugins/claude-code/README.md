# Artifax for Claude Code

Publish HTML pages from Claude Code to a local Artifax server, view them in a
browser, and get the comments people leave on them back into the session.

## Install

```
/plugin marketplace add empathic/artifax
/plugin install artifax@artifax
```

The plugin bundles a small wrapper (`scripts/ensure-artifax.sh`) that finds the
`artifax` binary on `PATH`, in `~/.local/bin`, or in `~/.artifax/bin`, and
otherwise downloads the latest release and installs it on first use. The download is checked against the release's `.sha256` file, which
comes from the same place as the tarball: the checksum protects integrity, not
authenticity. Set
`ARTIFAX_BIN` to run a specific build.

## What it adds

- The `artifax` MCP server (`artifax mcp --agent claude`): tools `publish`,
  `read`, `list`, `delete`, `open`, `pin`, `unpin`, `asset_upload`, `status`,
  `comments_read`, `comments_reply`, `comments_resolve`, `watch`,
  `wait_for_feedback`.
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
root copy and copy it over.

`scripts/smoke-claude.sh` (manual, calls a model) runs a real `claude -p`
session against a scratch daemon to check the whole path.
