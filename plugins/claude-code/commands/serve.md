---
description: Start, stop, or check the Clax daemon
argument-hint: "[--bind 0.0.0.0 | restart | stop | status]"
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
- `restart`: run `exec stop`, then `exec serve`, and report its URL.
- `stop`: run `exec stop`.
- `status`: the context above already answers it; report whether the daemon runs, its URL, pid, and version, and whether a browser is signed in (`browser_signed_in`).

After starting or restarting the daemon, make sure the user's browser is signed in to it, so the links you give them open: run `exec status --json`, and when `browser_signed_in` is `false`, run `exec open`. It signs their default browser in and opens Clax there. Say that you did, and that for another browser `exec login` prints a link that signs it in when opened within two minutes. When `browser_signed_in` is `true` or absent, do nothing more.
