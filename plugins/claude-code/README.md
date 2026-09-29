# Artifax for Claude Code

Publish HTML pages from Claude Code to a local Artifax server, view them in a
browser, and (from phase 3) get comments back.

## Install

```
/plugin marketplace add empathic/artifax
/plugin install artifax@artifax
```

The plugin bundles a small wrapper (`scripts/ensure-artifax.sh`) that finds the
`artifax` binary on `PATH`, in `~/.local/bin`, or in `~/.artifax/bin`, and
otherwise downloads the latest release and installs it on first use. Set
`ARTIFAX_BIN` to run a specific build.

## What it adds

- The `artifax` MCP server (`artifax mcp --agent claude`): tools `publish`,
  `read`, `list`, `delete`, `open`, `pin`, `unpin`, `asset_upload`, `status`.
- Hooks that register the Claude Code session with the daemon on start and end
  it on exit. The session-start context includes the daemon URL only when a daemon is already running; hooks never start one, the first tool call does.
- The `artifax` skill: when to publish and the page contract.
- Commands: `/artifax:open [id]`, `/artifax:list`,
  `/artifax:serve [--bind 0.0.0.0|stop|status]`, `/artifax:doctor`.

## Maintaining

`scripts/ensure-artifax.sh` here is a copy of `scripts/ensure-artifax.sh` at the
repository root; `scripts/test-plugins.sh` fails when they differ. Update the
root copy and copy it over.

`scripts/smoke-claude.sh` (manual, calls a model) runs a real `claude -p`
session against a scratch daemon to check the whole path.
