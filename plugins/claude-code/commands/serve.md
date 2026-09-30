---
description: Start, stop, or check the Clax daemon
argument-hint: "[--bind 0.0.0.0 | stop | status]"
allowed-tools: Bash(${CLAUDE_PLUGIN_ROOT}/scripts/ensure-clax.sh:*)
---

## Context

- Daemon status: !`"${CLAUDE_PLUGIN_ROOT}/scripts/ensure-clax.sh" exec status --json`

## Your task

Manage the local Clax daemon.

User arguments: $ARGUMENTS

Always invoke the CLI through the wrapper, with literal arguments and no shell variables:

```
"${CLAUDE_PLUGIN_ROOT}/scripts/ensure-clax.sh" exec <clax arguments...>
```

- No arguments: run `exec serve` to start the daemon in the background (a no-op when it already runs) and report its URL.
- `--bind <address>` (for example `0.0.0.0` for LAN access): run `exec serve --bind <address>`. Warn that binding beyond localhost exposes the artifacts to that network. If the daemon already runs on another address the command fails; tell the user to run `stop` first.
- `stop`: run `exec stop`.
- `status`: the context above already answers it; report whether the daemon runs, its URL, pid, and version.
