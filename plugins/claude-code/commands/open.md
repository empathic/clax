---
description: Open an artifact in the browser
argument-hint: "[artifact ID or URL]"
allowed-tools: Bash(${CLAUDE_PLUGIN_ROOT}/scripts/ensure-clax.sh:*)
---

## Context

- Artifacts (newest first, pinned first): !`"${CLAUDE_PLUGIN_ROOT}/scripts/ensure-clax.sh" exec list --json`

## Your task

Open an artifact in the user's browser with the `open` tool of the `clax` MCP server.

User arguments: $ARGUMENTS

- An artifact ID or URL given: call `open` with it as `url_or_id`.
- No argument: call `open` with the first artifact in the list above (the top one, pinned or most recently updated). If the list is empty, say there are no artifacts yet.
- Free text that is not an ID or URL is a hint about which artifact: match it against the titles in the list. If no artifact matches confidently, show the closest candidates and ask which one.

Report the artifact's title and URL. If `opened` is false, give the URL so the user can open it themselves.
