# Artifax for Codex

Publish HTML pages from Codex to a local Artifax server, view them in a
browser, and (from phase 3) get comments back.

This directory is `plugins/artifax` rather than `plugins/codex` because a Codex
marketplace entry must point at `./plugins/<plugin-name>`, and the plugin is
named `artifax`. The marketplace is `.agents/plugins/marketplace.json` at the
repository root.

## Install

From a clone of this repository:

```
codex plugin marketplace add /path/to/artifax
codex plugin add artifax@artifax
```

`codex mcp list` then shows the `artifax` server. Start a new Codex session to
pick up the tools and the skill.

The plugin bundles a small wrapper (`scripts/ensure-artifax.sh`) that finds the
`artifax` binary on `PATH`, in `~/.local/bin`, or in `~/.artifax/bin`, and
otherwise downloads the latest release and installs it on first use. Set
`ARTIFAX_BIN` to an absolute path to run a specific build.

## What it adds

- The `artifax` MCP server (`artifax mcp --agent codex`): tools `publish`,
  `read`, `list`, `delete`, `open`, `pin`, `unpin`, `asset_upload`, `status`,
  which Codex names `mcp__artifax__<tool>`. The first tool call starts the
  daemon when none is running.
- The `artifax` skill: when to publish and the page contract.
- Hooks (`hooks/hooks.json`) that register the Codex session with the daemon on
  start and end it on exit. They are optional; see below.

### Tool approval

Codex asks before each MCP tool call unless the server's tools are approved.
To approve the artifax tools, add to `~/.codex/config.toml`:

```toml
[plugins."artifax@artifax".mcp_servers.artifax]
default_tools_approval_mode = "approve"
```

`codex exec` runs with approval policy `never` and refuses tool calls that
would prompt, so non-interactive use needs this setting.

### Hooks

Codex runs plugin hooks only when hooks are enabled:

```toml
[features]
hooks = true
```

and each hook is trusted. Codex asks you to review and trust the plugin's hooks
the first time it finds them in an interactive session; `codex exec` skips
untrusted hooks. Without hooks the tools still work: the MCP server registers
the session itself. With hooks, the session also carries Codex's own session
ID.

## How the plugin finds its files

Codex 0.158 does not expand `${PLUGIN_ROOT}`, `${CLAUDE_PLUGIN_ROOT}`, or
`${CODEX_PLUGIN_ROOT}` in a plugin's `.mcp.json` `command`, `args`, or `env`,
and does not export them to MCP servers. It does resolve a relative `cwd`
against the installed plugin root, so `.mcp.json` sets `"cwd": "./"` and runs
`bash ./scripts/ensure-artifax.sh`. Hooks are run by a shell with `PLUGIN_ROOT`
(and `CLAUDE_PLUGIN_ROOT`) exported, so `hooks/hooks.json` uses
`"${PLUGIN_ROOT}"`.

Codex starts MCP servers with a minimal environment (`HOME`, `PATH`, and a few
others), so `.mcp.json` forwards the Artifax variables through `env_vars`:
`ARTIFAX_HOME`, `ARTIFAX_NO_OPEN`, `ARTIFAX_BIN`, `ARTIFAX_INSTALL_DIR`,
`ARTIFAX_CONFIG_DIR`, `ARTIFAX_RELEASE_BASE_URL`, `ARTIFAX_RELEASE_VERSION`.

## Maintaining

`scripts/ensure-artifax.sh` here is a copy of `scripts/ensure-artifax.sh` at the
repository root; `scripts/test-plugins.sh` fails when they differ. Update the
root copy and copy it over. `scripts/test-plugins.sh` also runs Codex's plugin
validator when `~/.codex/skills/.system/plugin-creator` is installed.

Codex installs a copy of the plugin under `$CODEX_HOME/plugins/cache`; after
changing files here, run `codex plugin add artifax@artifax` again.

`scripts/smoke-codex.sh` (manual, calls a model) installs the marketplace and
plugin into a scratch `CODEX_HOME`, runs `codex exec` to publish a page, and
checks the page and the registered session. It copies `~/.codex/auth.json` into
the scratch home for the run and deletes it afterwards. `--hooks` also enables
hooks for that run.
