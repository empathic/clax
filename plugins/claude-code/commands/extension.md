---
description: Set up the Clax Chrome extension, to comment on any web page (such as your dev server)
allowed-tools: Bash(${CLAUDE_PLUGIN_ROOT}/scripts/ensure-clax.sh:*)
---

## Context

- Install: !`"${CLAUDE_PLUGIN_ROOT}/scripts/ensure-clax.sh" exec extension install --json`

## Your task

Tell the user the result above: which browsers got the native messaging host (`hosts`, with `status` `installed`), and the `load_unpacked` instruction, which they follow once in Chrome. If `status` is `no_browser`, say that Chrome, Chromium, Brave or Edge must be installed first. A browser whose entry is `conflict` is registered to another Clax home; explain that and that `clax extension install --force` replaces it, and run it only if the user asks. If it failed, explain the error. Then remind them to click the Clax toolbar button on their dev server's tab to start commenting.

Last, when you know the page's URL (the user named it, or you run its dev server), call the `watch` tool of the `clax` MCP server with it, read the result's `wake` block, and do what its `do` says: when `wake.when_idle` is false and `wake.follow_command` is present, start `wake.follow_command` now with the Monitor tool (`timeout_ms` 1800000, started again when it expires), or without Monitor run `wake.follow_once_command` with the Bash tool and `run_in_background: true` and again after each comment. Tell the user whether their next comment will wake you. When you do not know the URL, say that you can `watch` it once they tell you.
