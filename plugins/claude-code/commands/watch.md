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

Then read the result's `wake` block and do what its `do` says before you report:

- `wake.when_idle` is true: start nothing; a comment the person sends to the agent wakes this session (`wake.via` says how).
- `wake.when_idle` is false and `wake.follow_command` is present: start `wake.follow_command` now with the Monitor tool (`timeout_ms` 1800000, and start it again when it expires); each line it prints is a comment notice. Without Monitor, run `wake.follow_once_command` with the Bash tool and `run_in_background: true`, and run it again after each comment. Keep exactly one running.
- `wake.when_idle` is false and there is no `follow_command`: nothing can wake this session while it is idle; say so plainly.

Report the artifact's title, whether it is now watched, and whether the person's next comment will wake this session (`replies_armed` alone does not mean it will). When it will not, say that their comments reach this session only at the end of a turn, with their next message, or on the next clax tool call.
