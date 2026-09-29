---
description: Wait for comments sent to you and act on each as it arrives
argument-hint: "[artifact ID or URL]"
allowed-tools: Bash(${CLAUDE_PLUGIN_ROOT}/scripts/ensure-artifax.sh:*), mcp__plugin_artifax_artifax__wait_for_feedback, mcp__plugin_artifax_artifax__comments_read, mcp__plugin_artifax_artifax__comments_reply, mcp__plugin_artifax_artifax__comments_resolve, mcp__plugin_artifax_artifax__publish, mcp__plugin_artifax_artifax__read
---

## Context

- Artifacts (pinned first, then most recently updated): !`"${CLAUDE_PLUGIN_ROOT}/scripts/ensure-artifax.sh" exec list --json`

## Your task

Enter the live comment loop from the skill's "Comment loop" section.

User arguments: $ARGUMENTS

- An artifact given (ID, URL, or title hint): pass it as `url_or_id` to `wait_for_feedback`; otherwise wait on every artifact this session watches.
- Tell the person once that you are waiting for their comments and that they can press "Send to agent" or write `@agent` on a thread.
- Loop: call `wait_for_feedback`; when comments arrive, act on each (change, `comments_reply`, `comments_resolve`); when the result has `call_again: true`, call it again. Stop when the person tells you to.
