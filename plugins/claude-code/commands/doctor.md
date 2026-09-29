---
description: Check the Artifax installation, storage, and daemon health
argument-hint: "[--fix]"
allowed-tools: Bash(${CLAUDE_PLUGIN_ROOT}/scripts/ensure-artifax.sh:*)
---

## Context

- Artifax CLI: !`"${CLAUDE_PLUGIN_ROOT}/scripts/ensure-artifax.sh"`
- Doctor: !`"${CLAUDE_PLUGIN_ROOT}/scripts/ensure-artifax.sh" exec doctor --agent claude --json`

## Your task

Summarize the doctor report above: list each check with its result, and explain the likely fix for any that failed. The report is read-only.

User arguments: $ARGUMENTS

If the user passed `--fix` and a check failed, explain what `--fix` repairs (stray staging and temp files, unaccounted version directories, zero-version artifact rows, and assets and corrupt rows of deleted artifacts; it never deletes live artifacts' rows). `--fix` refuses while a daemon is running, and the daemon normally is (the artifax tools start it), so after the user confirms: run `"${CLAUDE_PLUGIN_ROOT}/scripts/ensure-artifax.sh" exec stop`, then `"${CLAUDE_PLUGIN_ROOT}/scripts/ensure-artifax.sh" exec doctor --fix --json`, and tell the user the daemon will restart on the next artifax tool call.
