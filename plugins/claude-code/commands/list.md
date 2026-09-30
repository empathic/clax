---
description: List published artifacts with their URLs
allowed-tools: Bash(${CLAUDE_PLUGIN_ROOT}/scripts/ensure-clax.sh:*)
---

## Context

- Artifacts: !`"${CLAUDE_PLUGIN_ROOT}/scripts/ensure-clax.sh" exec list --json`

## Your task

Show the artifacts from the context above as a compact table: title, ID, version, pinned, last updated, and URL. Order is as listed (pinned first, then most recently updated). If there are none, say so and mention that publishing a page with the `clax` tools creates one.

Do not call any tool; the data is already in the context.
