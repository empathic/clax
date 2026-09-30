---
description: Watch an artifact for comments sent to you, or stop watching it
argument-hint: "[artifact ID or URL] [off]"
allowed-tools: Bash(${CLAUDE_PLUGIN_ROOT}/scripts/ensure-clax.sh:*), mcp__plugin_clax_clax__watch
---

## Context

- Artifacts (pinned first, then most recently updated): !`"${CLAUDE_PLUGIN_ROOT}/scripts/ensure-clax.sh" exec list --json`

## Your task

Call the `watch` tool of the `clax` MCP server.

User arguments: $ARGUMENTS

- The first argument is the artifact (ID, URL, or a title hint matched against the list above); without one, use the first artifact in the list.
- A trailing `off` means `on: false`; otherwise `on: true` with `replies: true`.

Report the artifact's title and whether it is now watched. When watched, say that comments the person sends to the agent will reach this session at the end of a turn, with the next message, or on the next clax tool call.
