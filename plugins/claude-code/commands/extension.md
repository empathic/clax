---
description: Set up the Clax Chrome extension, to comment on any web page (such as your dev server)
allowed-tools: Bash(${CLAUDE_PLUGIN_ROOT}/scripts/ensure-clax.sh:*)
---

## Context

- Install: !`"${CLAUDE_PLUGIN_ROOT}/scripts/ensure-clax.sh" exec extension install --json`

## Your task

Tell the user the result above: which browsers got the native messaging host (`hosts`, with `status` `installed`), and the `load_unpacked` instruction, which they follow once in Chrome. If no browser was installed, say that Chrome, Chromium, Brave or Edge must be installed first. If it failed, explain the error. Then remind them to click the Clax toolbar button on their dev server's tab to start commenting, and that you can `watch` that page's URL to receive their comments.
