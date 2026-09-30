# Stable Install Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Clax installs and develops the way `../clash` does. The plugins run the `clax` on `PATH` and never download anything. `just install` builds and installs `clax` from the checkout and registers its plugins with every harness (`clax init`). `just dev [claude|codex|pi]` runs a fresh build from a temporary directory on `PATH`, with the plugin loaded from the checkout, on a dev home and port of its own. `just watch` is the auto-reloading server. Releases and `install.sh` exist for other people, and nothing assumes the repository is public.

**Architecture:** `scripts/ensure-clax.sh`, copied into both plugins, shrinks to a thin wrapper. It runs `$CLAX_BIN` or the first `clax` on `PATH`, warns when that binary's version differs from the plugin's, and logs to `hooks.log`. When there is no `clax`, it answers the MCP client itself with a minimal server whose `status` tool says why, and it lets hooks exit 0. `clax init` writes the plugin tree embedded in the binary to `~/.clax/marketplace/` and registers it with Claude Code, Codex and Pi through their own CLIs. It also removes stale registrations, including those under the previous product name. `clax uninit` reverses it. The daemon records its executable, and a newer binary replaces an older daemon under the start lock on the same port. `clax doctor --agent` and the MCP `status` tool say which binary runs and whether it matches the plugin. A home's `config.toml` may set its daemon's port, which is how `~/.clax-dev` stays on 7481. A tag builds four native release binaries with a dry-run path. `install.sh` installs a release into `~/.local/bin` for people without a checkout.

**Tech Stack:** Bash 3.2-compatible shell (macOS `/bin/bash`), Rust 2024 (clap 4, `toml` 0.9, `rust-embed` 8, axum 0.8), TypeScript (Pi extension, Vitest), Python 3 (test-only fake servers, version scripts), GitHub Actions (`macos-15`, `macos-15-intel`, `ubuntu-24.04`, `ubuntu-24.04-arm`), and the harness CLIs `claude` (`plugin marketplace add|remove`, `plugin install|uninstall`, `--plugin-dir`, `--settings`), `codex` (`plugin marketplace add|remove`, `plugin add|remove`, `--enable`) and `pi` (`install`, `remove`, `-e`, `--skill`, `-ne`).

**Spec:** `docs/superpowers/specs/2026-09-28-clax-design.md`. Task 1 amends §2 (D15 note and new D16), §3, §4, §5, §7, §13, §14 and §16. The person's decisions are in `.superpowers/sdd/2026-09-30-stable-install/decisions.md`. Its last section, "REDESIGN", supersedes the earlier ones wherever they conflict, and it is binding.

**Precondition:** `git status --short -- docs/contract.md docs/superpowers/specs README.md plugins scripts justfile install.sh .github crates web/vite.shell.config.ts` prints nothing. A Task 1 run of the superseded plan may have left uncommitted D16 edits in the spec; if so, the person discards them first (see "Steps for the person", A). If anything else shows up, someone else has uncommitted work in files this plan edits: stop and ask. Do not stash, discard or commit another person's changes.

## Global Constraints

- Rust edition 2024. `cargo clippy --workspace --all-targets -- -D warnings` and `RUSTFLAGS=-Dwarnings cargo check --workspace` pass. `oxlint --deny-warnings` passes (`cd web && npm run lint`).
- Commit with plain `git commit`, which signs. Never pass `--no-gpg-sign`. After each commit, `git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed` prints `signed`. If signing fails, stop and report; do not retry in a loop. Stage with `git add` and explicit paths only.
- Never bind or connect to port 7480 or 7481. The person's daemon or dev server may be there. Tests start daemons with `--port 0`, and fake servers bind port 0.
- Never read, write or delete the real `~/.clax`, `~/.clax-dev`, `~/.claude`, `~/.codex`, `~/.pi`, `~/.cargo/bin/clax` or `~/.local/bin/clax`, nor the home directory Clax used before its rename (spec D15). Tests set `HOME` and `CLAX_HOME` to scratch directories. Tests that involve a harness set `CLAUDE_CONFIG_DIR`, `CODEX_HOME` and `PI_CODING_AGENT_DIR` to scratch directories and put fake `claude`, `codex` and `pi` scripts first on `PATH`. A test that runs a real harness CLI (only where a step says so) runs it with all four variables pointing at scratch directories.
- No test reaches GitHub or any other host. Downloads in tests go to `scripts/fake-release-server.py` on `127.0.0.1`.
- Agents never change the repository's visibility, tag, push, or publish a release. Nothing in the plan assumes the repository is public. `install.sh` and the release download work only once the person makes it public, and the docs say so.
- The plugins never download, build, or search anywhere but `$CLAX_BIN` and `PATH`.
- In prose, comments and commit messages, write "ID", never "id", except as a literal symbol in code.
- Shell that plugins or `install.sh` run is bash 3.2-compatible: no `mapfile`, no `${var,,}`, no `declare -A`, no `$BASHPID`. Expand a possibly empty array as `${a[@]+"${a[@]}"}`.
- `scripts/ensure-clax.sh` and its two plugin copies stay byte-identical (`scripts/test-plugins.sh` checks).
- The previous product name must not appear literally in any file this plan creates or edits, except the approved exceptions listed in `scripts/test-plugins.sh`. Code assembles it from two halves (`concat!("arti", "fax")`, `OLD="arti""fax"`), as the existing tests do.
- Every task ends with `bash scripts/quality_gates.sh; echo "exit=$?"` printing `exit=0`. Check the status itself, not only the last line of output.

## Review Focus

1. **The MCP client always gets an answer.** With no usable `clax` (none on `PATH`, or a bad `CLAX_BIN`), stdout carries valid JSON-RPC answering `initialize`, `tools/list`, `tools/call`, `ping` and unknown methods. It is never a closed pipe, and it never carries a stray log line. Tests: the "fallback …" cases in `scripts/test-ensure-clax.sh`.
2. **Hooks never fail their harness.** With no binary, or a binary that fails, hook mode exits 0 and logs one line. Tests: the "hook mode …" cases.
3. **`clax init` is safe to re-run and touches only what it names.** It removes and re-adds only the `clax` registrations and the previous name's, never another plugin or marketplace. It reports a harness CLI that is missing or failing instead of aborting the others. It never reads or writes the previous name's home. Tests: `crates/clax-cli/tests/init.rs`.
4. **`just dev` leaves nothing behind.** Its temporary directory is removed on exit. It never writes the real `~/.codex` (Codex runs on a dev `CODEX_HOME`). It stops only a dev-home daemon whose executable is gone. Tests: `scripts/test-dev.sh`.
5. **The daemon swap is clean.** The start lock is held from the shutdown until the new daemon answers. SSE streams end and reconnect to the same port. A newer daemon is never replaced by an older binary. Tests: `serve_replaces_an_older_daemon_on_its_port` and `serve_keeps_a_newer_daemon`.

---

## Design decisions

These settle the open questions. Each is binding for the tasks below.

**Plugins run `clax` from `PATH`, through a thin wrapper.** The Clash plugins call `clash hook …` directly. Clax keeps one small script between the harness and the binary, for two reasons the decisions require. A bare `clax mcp` in `.mcp.json` fails to spawn when `clax` is missing, and the client then shows a spawn error rather than a reason. A bare `clax hook …` exits 127 when `clax` is missing, which Claude Code reports as a hook error on every event. So `scripts/ensure-clax.sh` keeps its name (the hooks, slash commands and the agent-working plan's `tool-hook.sh` already call it), but all its discovery and download logic goes. It runs `$CLAX_BIN` (an explicit override for scripts and tests), else the first `clax` on `PATH` that reports itself as clax. The plugin's version is the wrapper's `CLAX_VERSION`, kept equal to the workspace version by `scripts/check-version.sh`. A binary of another version runs, with a warning on stderr and in `hooks.log`. `status` and `doctor --agent` report the mismatch too.

**The MCP failure surface is a minimal stdio server, not an error reply to `initialize`.** JSON-RPC 2.0 lets a server answer any request, `initialize` included, with an error, and the MCP lifecycle spec shows one for an unsupported protocol version. A client that gets that error treats the server as failed to start. Claude Code marks it failed in `/mcp`, and Codex reports a startup failure. The text lands in a status line or log that the agent never reads. A server that completes the handshake is visible to the person and the agent alike. Its `initialize` result carries `instructions` that state the reason. Its one tool, `status` (the name the skill already tells agents to call), returns the reason and the fix with `isError: true`. So when there is no `clax`, the wrapper serves that server itself, in about 40 lines of bash, and needs no binary. It echoes the client's `protocolVersion`, answers `ping`, and gives every other request JSON-RPC error -32601 with the same reason. Its `status` tool looks for `clax` again on every call, so once the person installs it, the tool says to reconnect.

**`clax init` registers an embedded copy of the plugins, not the checkout.** Registering the checkout path is what broke Codex when the checkout moved. It also cannot work for someone who installed with `install.sh` and has no checkout. The binary instead embeds the plugin tree it was built with (`rust-embed` over `plugins/`, plus the two marketplace manifests). `clax init` writes that tree to `<home>/marketplace/` (default `~/.clax/marketplace/`), in the checkout's layout: `.claude-plugin/marketplace.json`, `.agents/plugins/marketplace.json`, `plugins/claude-code`, `plugins/clax`, `plugins/pi`. It then registers that directory. The registered plugins therefore always match the installed binary exactly. Moving or deleting the checkout changes nothing, and nothing is pulled from GitHub. `just dev` is the path that loads plugins live from the checkout.

**How each harness is registered, found by running each CLI's `--help` and trying it against scratch config directories.** Codex 0.159.2 and Claude Code were run with a temporary `HOME`, `CODEX_HOME` and `CLAUDE_CONFIG_DIR`. Pi 0.73.1, from `plugins/pi/node_modules/.bin/pi`, was run with a temporary `HOME` and `PI_CODING_AGENT_DIR`.

| Harness | `clax init` | `clax uninit` | Load from a directory for `just dev` |
|---|---|---|---|
| Claude Code | `claude plugin uninstall clax@clax`, `claude plugin marketplace remove clax` (failures ignored), then `claude plugin marketplace add <root>`, `claude plugin install clax@clax` | the first two | `claude --plugin-dir <checkout>/plugins/claude-code`, with `--settings '{"enabledPlugins":{"clax@clax":false}}'` so an installed `clax@clax` does not load beside it |
| Codex | `codex plugin remove clax@clax`, `codex plugin marketplace remove clax` (failures ignored), then `codex plugin marketplace add <root>`, `codex plugin add clax@clax` | the first two | **Not supported.** Codex has no plugin-directory flag: `codex plugin add` copies the plugin into `$CODEX_HOME/plugins/cache/`. `just dev codex` therefore runs Codex on a dev `CODEX_HOME` (`~/.clax-dev/codex-home`). There it removes and re-adds the checkout as the `clax` marketplace and reinstalls the plugin before every start, so each start sees the checkout as it is. The real `~/.codex` is never touched, and the person logs in once in the dev `CODEX_HOME`. |
| Pi | `pi remove <each installed package named @empathic/clax-pi>`, then `pi install <root>/plugins/pi` | the removals | `pi -ne -e <checkout>/plugins/pi/src/clax.ts --skill <checkout>/plugins/pi/skills/clax`. `-ne` turns off extension discovery for that session, so an installed Clax package does not load twice. Other installed extensions are off for that session too. |

`pi install <path>` records the path in `settings.json` relative to the settings directory, and loads the package from there on every start. A Pi package that `clax init` wrote to `~/.clax/marketplace/plugins/pi` has no `node_modules`. The extension imports only Node built-ins, `typebox` and `@mariozechner/pi-coding-agent`, which Pi provides to extensions. "Steps for the person" confirms this on a real Pi.

**Stale registrations.** `clax init` and `clax uninit` also remove registrations under the previous product name: the marketplace and plugin `<old>` and `<old>@<old>` in Claude Code and Codex, and any Pi package whose `package.json` names `@empathic/<old>-pi`. They find these by reading the harnesses' own registries: `$CLAUDE_CONFIG_DIR/plugins/installed_plugins.json` and `known_marketplaces.json`, `$CODEX_HOME/config.toml`, and `$PI_CODING_AGENT_DIR/settings.json`. They remove each one through the harness's CLI. These are harness registrations, not Clax data. The previous name's home directory is still never read or touched (D15, amended in Task 1).

**The dev home and port.** A home's own `config.toml` may hold `[serve] port`, the port a daemon started for that home listens on. The spec's §5 already reserves `config.toml` for it. `just dev` and `just watch` create `~/.clax-dev` with `port = 7481`. So every daemon for that home, whether started by `just watch` or by a `just dev` session's shim, listens on 7481, and the agents' daemon on 7480 is never touched. `just dev` stops a dev-home daemon only when its recorded executable no longer exists, which means it came from an earlier `just dev` whose temporary directory is gone. A `just watch` daemon is left alone.

**Which daemon survives a version difference: newer wins.** A binary that finds an older daemon replaces it. One that finds a newer or equal daemon keeps it and warns once. Two harnesses whose plugins differ in version therefore converge on the newer daemon instead of restarting each other's at every reconnect. After a deliberate downgrade, the person runs `clax stop` once, and the README says so.

**The restart handshake.** `daemon.json` already carries `version`. It gains `exe`, the canonical path of the daemon's executable. A replacement runs in six steps:
1. It takes `daemon.lock`, the start lock every auto-start takes.
2. It re-reads `daemon.json`. If another client has already replaced the daemon acceptably, it uses that one.
3. It posts `/api/admin/shutdown`. SSE streams (`/api/events`) and `/mcp` streams end, and long polls (`wait_for_feedback`, the Stop hook's wait) return what they have, as on `clax stop`. In-flight requests get the existing 5 s drain, and one still running after that fails with a connection error that the agent can retry.
4. It waits up to 7 s for the old PID to exit.
5. It starts the new executable with `serve --foreground` on the old port and bind address, and waits for `/healthz`.
6. It releases the lock.

Browser tabs reconnect through `EventSource`'s retry to the same port. Shims get a 401 for the new token, refresh, and register again. Their session rows persist.

**`just install` and `just uninstall`.** `just install` runs `just web`, then `cargo install --locked --path crates/clax-cli` (to `~/.cargo/bin`), then `~/.cargo/bin/clax init`. It warns when the first `clax` on `PATH` is not the one it installed, for example a `~/.local/bin/clax` left by `install.sh`. `just uninstall` runs `clax uninit`, then `cargo uninstall clax-cli`. Nothing is pulled from GitHub.

**Releases, for other people only.** A `v*` tag builds four native release binaries: macOS arm64 on `macos-15`, macOS x86_64 on `macos-15-intel`, and Linux x86_64 and arm64 on `ubuntu-24.04` and `ubuntu-24.04-arm`. The Linux builds are static musl, with `musl-tools`. Each binary embeds the web UI and is smoke-tested. They are packed as `clax-<version>-<target>.tar.gz` and published with `SHA256SUMS` and `install.sh`. A manual run, or a pull request touching the release path, does everything except publish. `install.sh [version]` installs a release into `~/.local/bin` after checking its checksum, then says to run `clax init`. `github.com/empathic/clax` is private today, and GitHub serves a private repository's release files only to authenticated requests. So until the person makes the repository public, publishing works but `install.sh` gets 404. It says so, and the README says so. The first release is `v0.3.0` (Task 12 bumps the version; the person tags).

**macOS signing.** `curl` does not set `com.apple.quarantine`: only apps that opt into Launch Services quarantine do, such as browsers, Mail and AirDrop. `tar` sets it on extracted files only when the archive carries it, and `install.sh` fetches with `curl`. The archives are also built with `tar --no-mac-metadata --no-xattrs`, so no attribute travels inside them. Gatekeeper therefore never assesses the installed binary, and the arm64 linker's ad hoc signature suffices. `cargo install` builds locally and is never quarantined either. Two later channels would change this:
- A browser download of the archive is quarantined, and Gatekeeper then blocks an unsigned binary. It would need a Developer ID Application certificate, `codesign --options runtime --timestamp`, and `xcrun notarytool submit` of a zip. A bare Mach-O cannot be stapled, so Gatekeeper checks online.
- A Homebrew formula fetches with `curl` and does not quarantine. A Homebrew cask does, and would need the notarized binary.

"Steps for the person" checks `xattr` and `codesign` on a real download.

## File Structure

| Path | Responsibility |
|---|---|
| `scripts/ensure-clax.sh` (+ copies in `plugins/claude-code/scripts/`, `plugins/clax/scripts/`) | Thin wrapper: `$CLAX_BIN` or `clax` on `PATH`, version warning, fallback MCP server, hook exit 0, `hooks.log` |
| `scripts/test-ensure-clax.sh` | Wrapper tests with scratch homes and fake binaries |
| `crates/clax-core/src/config.rs` | A home's `config.toml`: `[serve] port` |
| `crates/clax-server/src/daemon.rs` | `DaemonInfo.exe` |
| `crates/clax-cli/src/client.rs` | `spawn_locked`, `replace`, newer-wins `connect_matching_version` |
| `crates/clax-cli/src/plugins.rs` | The embedded plugin tree and marketplace manifests; `materialize(root)` |
| `crates/clax-cli/src/commands/init.rs` | `clax init` / `clax uninit` |
| `crates/clax-cli/src/commands/doctor_agent.rs` | `binary` check: every `clax` on `PATH` |
| `crates/clax-mcp/src/tools.rs` | `status` reports `binary` |
| `plugins/pi/src/daemon.ts`, `plugins/pi/src/clax.ts` | Install hint, `binary` in `status` |
| `scripts/check-version.sh`, `bump-version.sh`, `package-release.sh`, `smoke-release-binary.sh`, `test-release.sh` | Versions and release packaging |
| `.github/workflows/release.yml` | Version check, four native builds, assemble + install through `install.sh`, publish on tag only |
| `install.sh`, `scripts/fake-release-server.py`, `scripts/test-install.sh` | The release installer for other people, and its tests |
| `scripts/dev.sh`, `scripts/watch.sh`, `scripts/dev-home.sh`, `scripts/test-dev.sh` | `just dev [harness]`, `just watch`, shared helpers, tests |

---

### Task 1: Spec amendments

**Files:**
- Modify: `docs/superpowers/specs/2026-09-28-clax-design.md` (§2, §3, §4, §5, §7, §13, §14, §16)

**Interfaces:**
- Produces: the binding text later tasks implement.

- [ ] **Step 1: §2 Decisions**

At the end of the D15 row's Decision cell, append: `; `clax init` and `clax uninit` do remove the harnesses' registrations of the first name's plugin and marketplace, which are harness settings, not Clax data`. After the D15 row, add:

```markdown
| D16 | The plugins run the `clax` on `PATH` (or `$CLAX_BIN`) through a thin wrapper and never download or build; `just install` installs `clax` from the checkout and `clax init` registers the plugins embedded in the binary with each harness; `just dev <harness>` runs a fresh build from a temporary directory on `PATH` with the plugin loaded from the checkout, on `~/.clax-dev` and port 7481; releases and `install.sh` serve people without a checkout | Local use never depends on a public repository or a release; a registered plugin always matches the installed binary; a moved checkout breaks nothing. |
```

- [ ] **Step 2: §3 Architecture**

Replace the Plugins bullet's `plus an installer script that finds or downloads the binary (toolpath's `ensure-path.sh` pattern)` with `plus a thin wrapper that runs the `clax` on `PATH` and explains when there is none (§13, D16)`.

- [ ] **Step 3: §4 Repository layout**

After the line `justfile, scripts/quality_gates.sh same gate style as toolpath`, add:

```
scripts/ensure-clax.sh             the plugins' wrapper (copied into both plugins' scripts/)
scripts/dev.sh, watch.sh           `just dev <harness>` and `just watch`
scripts/*release*, check-version.sh, bump-version.sh
                                   release packaging and version checks (.github/workflows/release.yml)
install.sh                         installs a release into ~/.local/bin, for people without a checkout
```

- [ ] **Step 4: §5 Storage and data model**

In the `~/.clax/` block, replace the `daemon.json` line and the `config.toml` line with:

```
  daemon.json            {port, pid, token, started_at, bind, version, exe}  mode 0600
```

and

```
  config.toml            [serve] port (a daemon started for this home listens there; default 7480);
                         later: bind address, sample provider, key env var name
  marketplace/           the plugins embedded in the binary, written and registered by `clax init`
```

- [ ] **Step 5: §7 Daemon discovery and lifecycle, item 3 and item 5**

In item 3, replace `binds `127.0.0.1:7480` by default` with `binds `127.0.0.1` on `--port`, else the home's `[serve] port`, else 7480`. Replace item 5 ("Version skew: …") with:

```markdown
5. Version skew: `daemon.json` records the daemon's `version` and `exe`
   (its executable's canonical path). A client that finds a daemon older
   than itself replaces it; a newer daemon, or one of the same version, is
   kept (a newer daemon serves older clients, and two plugins at different
   versions must not restart each other's daemon). A replacement holds
   `daemon.lock` throughout: it re-reads `daemon.json`, asks the daemon to
   shut down (SSE streams and long polls end, in-flight requests get 5 s),
   waits up to 7 s for its PID to exit, starts the new executable on the old
   port and bind address, and waits for `/healthz`. Storage migrations run
   on daemon start.
```

- [ ] **Step 6: §13 Plugins**

Replace the Claude Code bullet that begins `- `scripts/ensure-clax.sh`: toolpath's` with:

```markdown
- `scripts/ensure-clax.sh`: a thin wrapper. It runs `$CLAX_BIN`, else the
  first `clax` on `PATH` that reports itself as clax; it never downloads,
  builds, or looks anywhere else. A binary whose version differs from the
  plugin's (`CLAX_VERSION` in the wrapper) runs with a warning. With no
  binary, MCP mode answers the MCP client with a minimal server whose one
  tool, `status`, states the reason; hooks print one line and exit 0. Every
  failure and every MCP start is one line in `~/.clax/logs/hooks.log`.
- Installation: `clax init` writes the plugins embedded in the binary to
  `~/.clax/marketplace/` and registers them (`claude plugin marketplace
  add`, `claude plugin install clax@clax`); `clax uninit` removes them.
  `just dev claude` loads the checkout's plugin with `--plugin-dir`.
```

In the Codex section, replace the sentence that begins `It starts MCP servers with a minimal environment, so `env_vars`` through `shim starts inherits, §10).` with:

```markdown
It starts MCP servers with a minimal environment (which keeps `PATH`),
  so `env_vars` forwards `CLAX_HOME`, `CLAX_NO_OPEN`, `CLAX_BIN` and
  `CLAX_CODEX_BIN` (which a daemon the shim starts inherits, §10).
```

Replace the Codex bullet `- `scripts/ensure-clax.sh`: a copy of the Claude plugin's installer.` with `- `scripts/ensure-clax.sh`: a copy of the Claude plugin's wrapper.`, and replace `Install: `codex plugin marketplace add <repo>` then `codex plugin add clax@clax`.` with `Installed by `clax init` (`codex plugin marketplace add ~/.clax/marketplace`, `codex plugin add clax@clax`). Codex cannot load a plugin from a directory, so `just dev codex` runs on a dev `CODEX_HOME` and reinstalls the checkout's plugin there before each start.`

In the Pi section, after the first bullet, add:

```markdown
- The extension runs `$CLAX_BIN`, else `clax` on `PATH`, and never
  downloads. `clax init` runs `pi install ~/.clax/marketplace/plugins/pi`;
  `just dev pi` loads the checkout's extension and skill with `-e` and
  `--skill` (and `-ne`, so an installed copy does not load twice).
```

- [ ] **Step 7: §14 Security model**

Replace the bullet `- No telemetry, no outbound calls except `sample()` and release downloads` / `  by the installer script.` with:

```markdown
- No telemetry, no outbound calls except `sample()`. The plugins never
  download anything. `install.sh`, which a person runs by hand, downloads
  a release and checks it against the release's `SHA256SUMS`, which comes
  from the same place, so the check protects integrity, not authenticity.
```

- [ ] **Step 8: §16 Testing**

Replace `- **Plugins**: shell tests for `ensure-clax.sh`;` with:

```markdown
- **Plugins**: shell tests for `ensure-clax.sh` (the `PATH` lookup, the
  fallback MCP server, hooks), and tests of `clax init`/`uninit` and
  `just dev` against fake `claude`, `codex` and `pi` commands and scratch
  harness configuration directories;
```

and after the Plugins bullet add:

```markdown
- **Release**: `scripts/test-release.sh` checks the version, bump and
  packaging scripts; `scripts/test-install.sh` runs `install.sh` against a
  local fake release server; `.github/workflows/release.yml` builds,
  smoke-tests and packages every target on pull requests that touch it and
  on manual runs, and publishes only on a `v*` tag.
```

- [ ] **Step 9: Check and commit**

Run: `grep -c "D16" docs/superpowers/specs/2026-09-28-clax-design.md; bash scripts/test-plugins.sh | tail -1`
Expected: a count of at least 1, and `plugin checks passed`.

```bash
git add docs/superpowers/specs/2026-09-28-clax-design.md
git commit -m "Specify the clash-style install: clax on PATH, clax init, just dev per harness, releases for others"
git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed
```

---

### Task 2: A home's `config.toml` sets its daemon's port

**Files:**
- Create: `crates/clax-core/src/config.rs`
- Modify: `Cargo.toml` (workspace dependency `toml`), `crates/clax-core/Cargo.toml`, `crates/clax-core/src/lib.rs`, `crates/clax-cli/src/main.rs`, `crates/clax-cli/src/commands/{serve,status,tools,delete,pin,list,publish,mcp,open}.rs`, `crates/clax-cli/tests/cli.rs`

**Interfaces:**
- Produces: `clax_core::config::{FILE, HomeConfig}`; `HomeConfig::load(home_root) -> Result<HomeConfig>`, `.serve_port() -> Option<u16>`.
- Produces: `Cli::port_for(&self, home: &Home) -> u16`: `--port` when given, else the home's `[serve] port`, else 7480.

- [ ] **Step 1: Add the dependency**

In the root `Cargo.toml` `[workspace.dependencies]`, add `toml = "0.9"`. In `crates/clax-core/Cargo.toml` `[dependencies]`, add `toml.workspace = true`.

- [ ] **Step 2: Write the failing tests, then implement**

Create `crates/clax-core/src/config.rs` and add `pub mod config;` to `crates/clax-core/src/lib.rs`:

```rust
//! A home's `config.toml`.
//!
//! Read-only here. `[serve] port` is the port a daemon started for the home
//! listens on (7480 when absent); `just dev` and `just watch` write
//! `port = 7481` into `~/.clax-dev/config.toml`. Other tables are reserved
//! (spec §5) and ignored.

use crate::{CoreError, Result};
use std::path::Path;

/// The file name inside a home.
pub const FILE: &str = "config.toml";

/// A parsed `config.toml`.
#[derive(Clone, Debug, Default)]
pub struct HomeConfig {
    table: toml::Table,
}

impl HomeConfig {
    /// `<home_root>/config.toml`; a missing file is an empty config.
    ///
    /// # Errors
    /// `Invalid { code: "bad_config" }` naming the file when it does not
    /// parse; `Io` when it cannot be read.
    pub fn load(home_root: &Path) -> Result<HomeConfig> {
        let path = home_root.join(FILE);
        let table = match std::fs::read_to_string(&path) {
            Ok(text) => text.parse::<toml::Table>().map_err(|e| {
                CoreError::invalid("bad_config", format!("{}: {e}", path.display()))
            })?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => toml::Table::new(),
            Err(e) => return Err(e.into()),
        };
        Ok(HomeConfig { table })
    }

    /// `[serve] port`, when it is an integer in 1..=65535.
    pub fn serve_port(&self) -> Option<u16> {
        let p = self.table.get("serve")?.get("port")?.as_integer()?;
        u16::try_from(p).ok().filter(|p| *p > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(text: &str) -> HomeConfig {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(FILE), text).unwrap();
        HomeConfig::load(dir.path()).unwrap()
    }

    #[test]
    fn a_missing_file_has_no_port() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(HomeConfig::load(dir.path()).unwrap().serve_port(), None);
    }

    #[test]
    fn serve_port_is_read_and_other_tables_are_ignored() {
        assert_eq!(with("[sample]\napi_key_env = \"K\"\n\n[serve]\nport = 7481\n").serve_port(), Some(7481));
    }

    #[test]
    fn out_of_range_or_mistyped_ports_are_ignored() {
        for t in ["[serve]\nport = 0\n", "[serve]\nport = 70000\n", "[serve]\nport = \"7481\"\n"] {
            assert_eq!(with(t).serve_port(), None, "{t}");
        }
    }

    #[test]
    fn a_config_that_does_not_parse_is_an_error_naming_the_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(FILE), "[serve\n").unwrap();
        let e = HomeConfig::load(dir.path()).unwrap_err().to_string();
        assert!(e.contains("config.toml"), "{e}");
    }
}
```

Run: `cargo test -p clax-core config::`
Expected: PASS (4 tests).

- [ ] **Step 3: The CLI's default port comes from the home's config**

In `crates/clax-cli/src/main.rs`, change the `port` field of `Cli` to:

```rust
    /// Port to use when starting a daemon (0 = any free port). Default: the
    /// home's `[serve] port` in config.toml, else 7480.
    #[arg(long, global = true)]
    pub port: Option<u16>,
```

and add below `pub enum Cmd { … }`:

```rust
impl Cli {
    /// `--port` when given, else the home's `[serve] port`, else 7480.
    pub fn port_for(&self, home: &clax_core::Home) -> u16 {
        self.port
            .or_else(|| {
                clax_core::config::HomeConfig::load(home.root())
                    .ok()
                    .and_then(|c| c.serve_port())
            })
            .unwrap_or(clax_server::daemon::DEFAULT_PORT)
    }
}
```

In each command file listed under **Files**, replace `cli.port` with `cli.port_for(home)`. Afterwards, `grep -rn "cli\.port\b" crates/clax-cli/src` prints only `port_for` lines.

- [ ] **Step 4: CLI test for the home's port**

Append to `crates/clax-cli/tests/cli.rs`:

```rust
#[test]
fn a_daemon_started_without_port_uses_the_homes_serve_port() {
    let e = Env::new();
    // A port the kernel picks, recorded in the home's config.toml.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    std::fs::create_dir_all(e.dir.path().join("ax")).unwrap();
    std::fs::write(
        e.dir.path().join("ax/config.toml"),
        format!("[serve]\nport = {port}\n"),
    )
    .unwrap();
    let out = e.cmd().args(["serve", "--json"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["port"].as_u64().unwrap(), u64::from(port));
    e.stop();
}
```

Run: `cargo test -p clax-cli --test cli a_daemon_started_without_port_uses_the_homes_serve_port`
Expected: PASS.

- [ ] **Step 5: Gates and commit**

Run: `bash scripts/quality_gates.sh; echo "exit=$?"`
Expected: `exit=0`.

```bash
git add Cargo.toml Cargo.lock crates/clax-core/Cargo.toml crates/clax-core/src/config.rs crates/clax-core/src/lib.rs crates/clax-cli/src crates/clax-cli/tests/cli.rs
git commit -m "A home's config.toml [serve] port sets its daemon's default port"
git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed
```

---
### Task 3: The daemon's executable, and a clean replacement

**Files:**
- Modify: `crates/clax-server/src/daemon.rs`, `crates/clax-server/tests/daemon.rs`, `crates/clax-cli/src/client.rs`, `crates/clax-cli/src/commands/serve.rs`
- Create: `crates/clax-cli/tests/switch.rs`

**Interfaces:**
- Consumes: `Cli::port_for` (Task 2).
- Produces: `DaemonInfo.exe: Option<String>`.
- Produces: `Client::spawn_locked(home, exe: &Path, port, bind) -> Result<Client>`. The caller holds `DaemonLock`.
- Produces: `Client::replace(home, old: &Client, exe: &Path, accept: impl Fn(&DaemonInfo) -> bool) -> Result<Client>`.
- Produces: `connect_matching_version` keeps a newer or equal daemon and replaces an older one through `replace`, on the old port and bind. `clax serve` (background) now calls it too.

- [ ] **Step 1: `exe` in `daemon.json`**

In `crates/clax-server/src/daemon.rs`, add to `DaemonInfo` after `version`:

```rust
    /// Canonical path of the daemon's executable, so a client can tell which
    /// build serves (a `just dev` build and an installed one can share a
    /// version). Absent in records written before it existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exe: Option<String>,
```

In `serve`, set it when building `info`:

```rust
        exe: std::env::current_exe()
            .and_then(|p| p.canonicalize())
            .ok()
            .map(|p| p.display().to_string()),
```

Add `exe: None` to every other `DaemonInfo { … }` literal: `grep -rn "DaemonInfo {" crates` lists them (`crates/clax-server/tests/daemon.rs` and the `info()` helper in `crates/clax-cli/src/client.rs`). In `crates/clax-server/tests/daemon.rs`, add a test that a record without `exe` still parses:

```rust
#[test]
fn a_record_without_exe_still_parses() {
    let v: clax_server::daemon::DaemonInfo = serde_json::from_str(
        r#"{"port":1,"pid":2,"token":"t","started_at":"s","bind":"127.0.0.1","version":"0.2.0"}"#,
    )
    .unwrap();
    assert_eq!(v.exe, None);
}
```

- [ ] **Step 2: Write the failing switch test**

Create `crates/clax-cli/tests/switch.rs`:

```rust
//! A binary that finds an older daemon replaces it on the same port, holding
//! the start lock; a newer daemon is kept.

use assert_cmd::Command;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

struct Fake {
    port: u16,
    child: Arc<Mutex<Child>>,
    shutdown_seen: Arc<AtomicBool>,
}

/// A stand-in daemon of `version`: its PID is a `sleep` child, it answers
/// `/healthz`, and on `POST /api/admin/shutdown` it closes its listener and
/// kills the child, as a real daemon exits.
fn fake_daemon(home: &std::path::Path, version: &str) -> Fake {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let child = Arc::new(Mutex::new(
        std::process::Command::new("sleep").arg("60").spawn().unwrap(),
    ));
    let pid = child.lock().unwrap().id();
    let seen = Arc::new(AtomicBool::new(false));
    let (c2, s2, v) = (child.clone(), seen.clone(), version.to_string());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let mut buf = [0u8; 2048];
            let n = s.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let body = format!(r#"{{"version":"{v}","pid":{pid},"started_at":"s"}}"#);
            let _ = write!(
                s,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            if req.starts_with("POST /api/admin/shutdown") {
                s2.store(true, Ordering::SeqCst);
                drop(s);
                break; // drops the listener: the port is free again
            }
        }
        let mut c = c2.lock().unwrap();
        let _ = c.kill();
        let _ = c.wait();
    });
    std::fs::create_dir_all(home).unwrap();
    let info = serde_json::json!({
        "port": port, "pid": pid, "token": "t", "started_at": "s",
        "bind": "127.0.0.1", "version": version,
    });
    std::fs::write(home.join("daemon.json"), info.to_string()).unwrap();
    Fake { port, child, shutdown_seen: seen }
}

fn clax(dir: &std::path::Path) -> Command {
    let mut c = Command::cargo_bin("clax").unwrap();
    c.env("HOME", dir)
        .env("CLAX_HOME", dir.join("ax"))
        .env_remove("CLAX_CONFIG_DIR")
        .env("CLAX_CODEX_BIN", "");
    c
}

fn daemon_json(dir: &std::path::Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(dir.join("ax/daemon.json")).unwrap()).unwrap()
}

#[test]
fn serve_replaces_an_older_daemon_on_its_port() {
    let dir = tempfile::tempdir().unwrap();
    let fake = fake_daemon(&dir.path().join("ax"), "0.0.1");
    let out = clax(dir.path()).args(["serve", "--json", "--port", "0"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(fake.shutdown_seen.load(Ordering::SeqCst), "the old daemon was asked to shut down");
    let now = daemon_json(dir.path());
    assert_eq!(now["port"].as_u64().unwrap(), u64::from(fake.port), "same port");
    assert_eq!(now["version"], env!("CARGO_PKG_VERSION"));
    let exe = std::fs::canonicalize(env!("CARGO_BIN_EXE_clax")).unwrap();
    assert_eq!(now["exe"].as_str().unwrap(), exe.display().to_string());
    clax(dir.path()).arg("stop").assert().success();
}

#[test]
fn serve_keeps_a_newer_daemon() {
    let dir = tempfile::tempdir().unwrap();
    let fake = fake_daemon(&dir.path().join("ax"), "999.0.0");
    let out = clax(dir.path()).args(["serve", "--json", "--port", "0"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(!fake.shutdown_seen.load(Ordering::SeqCst));
    assert_eq!(daemon_json(dir.path())["version"], "999.0.0");
    let mut c = fake.child.lock().unwrap();
    let _ = c.kill();
    let _ = c.wait();
}
```

Run: `cargo test -p clax-cli --test switch`
Expected: `serve_replaces_an_older_daemon_on_its_port` FAILS, because `clax serve` returns the old daemon.

- [ ] **Step 3: Split the spawn out of `connect_with_bind`**

In `crates/clax-cli/src/client.rs`, move everything in `connect_with_bind` after the second `discover_with` check into:

```rust
    /// Starts `exe serve --foreground` for `home` on `port` and `bind` and
    /// waits up to 5 s for it to answer `/healthz`. The caller holds the
    /// start lock ([`DaemonLock`]).
    pub fn spawn_locked(
        home: &Home,
        exe: &std::path::Path,
        port: u16,
        bind: IpAddr,
    ) -> anyhow::Result<Client> {
        let probe = probe_client().context("building probe client")?;
        // (the log file, Command::new(exe) … and the readiness loop, moved
        // unchanged from connect_with_bind, with `std::env::current_exe()?`
        // replaced by `exe`)
    }
```

`connect_with_bind` then reads:

```rust
    pub fn connect_with_bind(home: &Home, port: u16, bind: IpAddr) -> anyhow::Result<Client> {
        let probe = probe_client().context("building probe client")?;
        if let Some(c) = Client::discover_with(home, &probe) {
            return Ok(c);
        }
        home.ensure_dirs()?;
        let _lock = DaemonLock::acquire(home).context("acquiring daemon lock")?;
        if let Some(c) = Client::discover_with(home, &probe) {
            return Ok(c);
        }
        Client::spawn_locked(home, &std::env::current_exe()?, port, bind)
    }
```

The moved body is code motion. Its comments (stdio, `setsid`, the zombie reaper) move with it unchanged.

- [ ] **Step 4: `replace` and the newer-wins rule**

Add to `impl Client`:

```rust
    /// Replaces the daemon `old` names with one started from `exe` on the
    /// same port and bind address. Holds the start lock throughout, so no
    /// other client starts a daemon in the gap. Under the lock it re-reads
    /// `daemon.json`: a different live daemon that `accept`s is used as it
    /// is (another client already replaced `old`). Otherwise it asks the
    /// daemon to shut down (SSE streams and long polls end; in-flight
    /// requests get the daemon's 5 s drain), waits up to 7 s for its PID to
    /// exit, and starts `exe`.
    pub fn replace(
        home: &Home,
        old: &Client,
        exe: &std::path::Path,
        accept: impl Fn(&DaemonInfo) -> bool,
    ) -> anyhow::Result<Client> {
        let bind: IpAddr = old
            .info
            .bind
            .parse()
            .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
        let _lock = DaemonLock::acquire(home).context("acquiring daemon lock")?;
        let current = Client::discover(home);
        if let Some(c) = &current
            && c.info.pid != old.info.pid
            && accept(&c.info)
        {
            return Ok(Client::from_info(c.info.clone()));
        }
        let target = current.unwrap_or_else(|| Client::from_info(old.info.clone()));
        if let Err(e) = target.shutdown() {
            tracing::info!(error = %e, "the old daemon did not take the shutdown request; waiting for it to exit");
        }
        let deadline = Instant::now() + Duration::from_secs(7);
        while Instant::now() < deadline && pid_alive(target.info.pid) {
            std::thread::sleep(Duration::from_millis(50));
        }
        if pid_alive(target.info.pid) {
            bail!(
                "clax daemon v{} (pid {}) did not exit within 7 s; stop it with `clax stop` and try again",
                target.info.version,
                target.info.pid
            );
        }
        Client::spawn_locked(home, exe, target.info.port, bind)
    }
```

Replace the body of `connect_matching_version` after the first `if !needs_replacing … { … return Ok(c); }` block with:

```rust
        tracing::info!(
            "replacing clax daemon v{} (pid {}) with v{ours} on port {}",
            c.info.version,
            c.info.pid,
            c.info.port
        );
        let exe = std::env::current_exe().context("finding this executable")?;
        Client::replace(home, &c, &exe, |info| !needs_replacing(info, ours))
```

Update its doc comment: "…a running daemon older than this binary is replaced through [`Client::replace`] on its own port and bind address; a newer or equal daemon is kept, with a warning logged once per process when the versions differ."

- [ ] **Step 5: `clax serve` upgrades an older daemon**

In `crates/clax-cli/src/commands/serve.rs`, in the background branch, replace `Client::connect_with_bind(home, cli.port_for(home), bind)` with the sequence below, so a newer binary's `serve` replaces an older daemon exactly as its shim would:

```rust
    let bind = a.bind.unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
    let c = match Client::discover(home) {
        Some(_) => Client::connect_matching_version(home, cli.port_for(home))?,
        None => Client::connect_with_bind(home, cli.port_for(home), bind)?,
    };
```

- [ ] **Step 6: Run the tests**

Run: `cargo test -p clax-cli --test switch && cargo test -p clax-cli --test cli && cargo test -p clax-server --test daemon`
Expected: PASS.

- [ ] **Step 7: Gates and commit**

Run: `bash scripts/quality_gates.sh; echo "exit=$?"`
Expected: `exit=0`.

```bash
git add crates/clax-server/src/daemon.rs crates/clax-server/tests/daemon.rs crates/clax-cli/src/client.rs crates/clax-cli/src/commands/serve.rs crates/clax-cli/tests/switch.rs
git commit -m "Record the daemon's executable; replace an older daemon under the start lock on its own port"
git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed
```

---

### Task 4: The plugins' wrapper: `clax` from `PATH`, and a readable failure

**Files:**
- Modify (rewrite): `scripts/ensure-clax.sh`, then copy it to `plugins/claude-code/scripts/ensure-clax.sh` and `plugins/clax/scripts/ensure-clax.sh`
- Modify (rewrite): `scripts/test-ensure-clax.sh`
- Modify: `plugins/clax/.mcp.json` (`env_vars`), `scripts/test-plugins.sh`

**Interfaces:**
- Produces: the wrapper specified in spec §13 (Task 1). Its `hooks.log` lines are:
  - `<ts> launch mode=mcp agent=<a> bin="<path>" version="<line>" warning="<text>"` on every MCP start.
  - `<ts> launcher mode=<m> agent=<a> exit=<code|fallback> reason="<text>" tried="<candidates>" argv="<args>"` on every failure. `clax doctor --agent`'s `hooks` check already treats a ` launcher ` line as a failure.
- Produces: the fallback MCP server (Design decisions).
- Produces: `CLAX_VERSION` in the wrapper, which `scripts/check-version.sh` (Task 7) reads. It replaces `MIN_VERSION`.

This task removes the checkout search, the `~/.local/bin` and `~/.clax/bin` lookups, the release download, `CLAX_SOURCE_DIR`, `CLAX_INSTALL_DIR`, `CLAX_CONFIG_DIR`, `CLAX_RELEASE_BASE_URL` and `CLAX_RELEASE_VERSION`, and every test of them.

- [ ] **Step 1: Write the new tests**

Replace `scripts/test-ensure-clax.sh` with:

```bash
#!/usr/bin/env bash
# Hermetic tests for ensure-clax.sh: a scratch HOME, and a PATH holding only
# the tools the wrapper needs plus fake `clax` binaries. No network, no real
# ~/.clax, no harness.
set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$(mktemp -d)" && pwd -P)"
trap 'rm -rf "$ROOT"' EXIT
mkdir -p "$ROOT/wrapper"
cp "$HERE/ensure-clax.sh" "$ROOT/wrapper/ensure-clax.sh"
SCRIPT="$ROOT/wrapper/ensure-clax.sh"
V="$(sed -n 's/^CLAX_VERSION="\(.*\)"$/\1/p' "$SCRIPT")"
# The interpreter itself, not a version-manager shim that needs the real PATH.
PY="$(python3 -c 'import sys; print(sys.executable)')"
ORIG_PATH="$PATH"
FAILED=0
pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1"; FAILED=1; }

# The tools the wrapper may call, linked into an otherwise empty directory
# (never a clax).
TOOLS="$ROOT/tools"
mkdir -p "$TOOLS"
for t in bash sh env awk head tail grep sed tr cat mktemp mv mkdir rm chmod date wc cp sleep ls; do
    if p="$(command -v "$t" 2>/dev/null)" && [ -x "$p" ]; then ln -sf "$p" "$TOOLS/$t"; fi
done

# A fake clax at $1/clax whose --version prints $2; any other run prints its
# arguments.
fake_clax() {
    mkdir -p "$1"
    printf '#!/bin/sh\nif [ "$1" = "--version" ]; then echo "%s"; exit 0; fi\necho "args: $*"\n' "$2" > "$1/clax"
    chmod +x "$1/clax"
}

new_env() {
    SANDBOX="$(mktemp -d "$ROOT/case.XXXXXX")"
    export HOME="$SANDBOX/home"
    mkdir -p "$HOME"
    FAKEBIN="$SANDBOX/fakebin"
    mkdir -p "$FAKEBIN"
    export PATH="$FAKEBIN:$TOOLS"
    unset CLAX_BIN CLAX_HOME CLAX_SOURCE_DIR CLAX_INSTALL_DIR CLAX_CONFIG_DIR CLAX_RELEASE_BASE_URL CLAX_RELEASE_VERSION
}
run() { OUT="$("$TOOLS/bash" "$SCRIPT" "$@" 2>"$SANDBOX/stderr" < /dev/null)"; RC=$?; ERR="$(cat "$SANDBOX/stderr")"; }
run_at() { local s="$1"; shift; OUT="$("$TOOLS/bash" "$s" "$@" 2>"$SANDBOX/stderr" < /dev/null)"; RC=$?; ERR="$(cat "$SANDBOX/stderr")"; }

# MCP requests as the clients send them: rmcp (Codex) puts the ID first, the
# TypeScript SDK (Claude Code) puts it last.
REQS='{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"method":"tools/list","params":{},"jsonrpc":"2.0","id":1}
{"jsonrpc":"2.0","id":"call-2","method":"tools/call","params":{"name":"status","arguments":{}}}
{"jsonrpc":"2.0","id":3,"method":"resources/list","params":{}}
{"jsonrpc":"2.0","id":4,"method":"ping"}'
mcp() { OUT="$(printf '%s\n' "$REQS" | "$TOOLS/bash" "$SCRIPT" exec mcp --agent codex 2>"$SANDBOX/stderr")"; RC=$?; ERR="$(cat "$SANDBOX/stderr")"; }
# Checks that $OUT is the fallback server's answer to $REQS; prints the text
# of its status tool. Fails (non-zero) otherwise.
fallback_text() {
    "$PY" - "$OUT" <<'PYEOF'
import json, sys
lines = [json.loads(l) for l in sys.argv[1].splitlines()]
assert [l["id"] for l in lines] == [0, 1, "call-2", 3, 4], lines
init = lines[0]["result"]
assert init["protocolVersion"] == "2025-06-18" and init["serverInfo"]["name"] == "clax", init
assert init["capabilities"] == {"tools": {}}, init
assert init["instructions"].startswith("Clax is unavailable: "), init
tools = lines[1]["result"]["tools"]
assert [t["name"] for t in tools] == ["status"] and tools[0]["inputSchema"]["type"] == "object", tools
call = lines[2]["result"]
assert call["isError"] is True and call["content"][0]["type"] == "text", call
assert lines[3]["error"]["code"] == -32601, lines[3]
assert lines[4]["result"] == {}, lines[4]
print(call["content"][0]["text"])
PYEOF
}
hooks_log() { cat "${CLAX_HOME:-$HOME/.clax}/logs/hooks.log" 2>/dev/null; }

# --- Resolution -------------------------------------------------------------

new_env
fake_clax "$FAKEBIN" "clax $V"
run
if [ "$RC" = 0 ] && [ "$OUT" = "$FAKEBIN/clax" ]; then pass "the clax on PATH is found"
else fail "the clax on PATH is found (rc=$RC out=$OUT err=$ERR)"; fi

new_env
fake_clax "$FAKEBIN" "somethingelse 1.0"
fake_clax "$SANDBOX/second" "clax $V"
PATH="$FAKEBIN:$SANDBOX/second:$TOOLS" run
if [ "$RC" = 0 ] && [ "$OUT" = "$SANDBOX/second/clax" ]; then pass "a foreign clax on PATH is skipped"
else fail "a foreign clax on PATH is skipped (rc=$RC out=$OUT)"; fi

new_env
fake_clax "$FAKEBIN" "clax $V"
fake_clax "$SANDBOX/x" "clax $V"
CLAX_BIN="$SANDBOX/x/clax" run exec status
if [ "$RC" = 0 ] && [ "$OUT" = "args: status" ]; then
    CLAX_BIN="$SANDBOX/x/clax" run
    if [ "$OUT" = "$SANDBOX/x/clax" ]; then pass "CLAX_BIN wins over PATH"; else fail "CLAX_BIN wins over PATH (out=$OUT)"; fi
else fail "CLAX_BIN wins over PATH (rc=$RC out=$OUT err=$ERR)"; fi

new_env
fake_clax "$FAKEBIN" "clax $V"
CLAX_BIN="$SANDBOX/missing" run exec status
if [ "$RC" = 1 ] && [ -z "$OUT" ] && echo "$ERR" | grep -q "CLAX_BIN is set to '$SANDBOX/missing', which is not a usable clax binary"; then
    pass "an unusable CLAX_BIN fails instead of falling back to PATH"
else fail "an unusable CLAX_BIN fails (rc=$RC out=$OUT err=$ERR)"; fi

new_env
fake_clax "$FAKEBIN" "clax 0.0.1"
mcp
if [ "$OUT" = "args: mcp --agent codex" ] && echo "$ERR" | grep -q "warning: $FAKEBIN/clax is clax 0.0.1, but this plugin is clax $V" \
    && hooks_log | grep -q "launch mode=mcp agent=codex bin=\"$FAKEBIN/clax\" version=\"clax 0.0.1\" warning=\"$FAKEBIN/clax is clax 0.0.1"; then
    pass "a clax of another version runs in MCP mode with a logged warning"
else fail "a clax of another version runs with a logged warning (out=$OUT err=$ERR log=$(hooks_log))"; fi

new_env
fake_clax "$FAKEBIN" "clax $V"
mcp
if [ "$OUT" = "args: mcp --agent codex" ] && [ -z "$ERR" ] && hooks_log | grep -q "launch mode=mcp agent=codex bin=\"$FAKEBIN/clax\" version=\"clax $V\" warning=\"\""; then
    pass "every MCP start is logged"
else fail "every MCP start is logged (out=$OUT err=$ERR log=$(hooks_log))"; fi

new_env
mcp
if text="$(fallback_text)" && echo "$text" | grep -q "no clax binary is on PATH" && echo "$text" | grep -q "just install" \
    && hooks_log | grep -q "launcher mode=mcp agent=codex exit=fallback reason=\"no clax binary is on PATH.*tried=\"PATH has no clax: $FAKEBIN:$TOOLS\""; then
    pass "no clax: the MCP client gets the reason from the fallback server"
else fail "no clax: the MCP client gets the reason (out=$OUT err=$ERR log=$(hooks_log))"; fi

new_env
CLAX_BIN="$SANDBOX/a\"b\\c" mcp
if text="$(fallback_text)" && echo "$text" | grep -qF "$SANDBOX/a\"b\\c"; then
    pass "the fallback escapes quotes and backslashes in its JSON"
else fail "the fallback escapes quotes and backslashes (out=$OUT)"; fi

new_env
OUT="$({
    printf '%s\n' "$(echo "$REQS" | head -1)"
    sleep 0.5
    fake_clax "$FAKEBIN" "clax $V"
    printf '%s\n' '{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"status","arguments":{}}}'
} | "$TOOLS/bash" "$SCRIPT" exec mcp --agent claude 2>/dev/null)"
if echo "$OUT" | tail -1 | grep -q "clax is now available at $FAKEBIN/clax. Reconnect"; then
    pass "the fallback's status tool notices a clax installed since"
else fail "the fallback's status tool notices a clax installed since (out=$OUT)"; fi

# Neither a checkout, ~/.cargo/bin off PATH, ~/.local/bin, nor a harness's
# configuration is searched.
new_env
mkdir -p "$SANDBOX/repo/plugins/clax/scripts" "$HOME/.codex"
printf '[workspace]\nmembers = [\n    "crates/clax-cli",\n]\n' > "$SANDBOX/repo/Cargo.toml"
cp "$SCRIPT" "$SANDBOX/repo/plugins/clax/scripts/ensure-clax.sh"
fake_clax "$SANDBOX/repo/target/debug" "clax $V"
fake_clax "$HOME/.cargo/bin" "clax $V"
fake_clax "$HOME/.local/bin" "clax $V"
printf '[marketplaces.clax]\nsource_type = "local"\nsource = "%s"\n' "$SANDBOX/repo" > "$HOME/.codex/config.toml"
CLAX_SOURCE_DIR="$SANDBOX/repo" CLAX_INSTALL_DIR="$HOME/.local/bin" run_at "$SANDBOX/repo/plugins/clax/scripts/ensure-clax.sh"
if [ "$RC" = 1 ] && [ -z "$OUT" ]; then pass "only PATH is searched: not a checkout, ~/.cargo/bin, ~/.local/bin or Codex's config"
else fail "only PATH is searched (rc=$RC out=$OUT)"; fi

new_env
fake_clax "$FAKEBIN" "clax $V"
run exec one "two words"
if [ "$RC" = 0 ] && [ "$OUT" = "args: one two words" ]; then pass "exec passes arguments through"
else fail "exec passes arguments through (rc=$RC out=$OUT)"; fi

new_env
run bogus
if [ "$RC" = 2 ] && echo "$ERR" | grep -q usage; then pass "an unknown mode prints usage"; else fail "an unknown mode prints usage (rc=$RC)"; fi

# --- Hooks never fail --------------------------------------------------------

new_env
run exec hook --agent codex session-start
if [ "$RC" = 0 ] && [ -z "$OUT" ] && [ "$(printf '%s\n' "$ERR" | grep -c .)" = 1 ] && echo "$ERR" | grep -q "no clax binary is on PATH" \
    && hooks_log | grep -q "launcher mode=hook agent=codex exit=0 reason=\"no clax binary is on PATH.*argv=\"exec hook --agent codex session-start\""; then
    pass "hook mode with no clax prints one line, logs it and exits 0"
else fail "hook mode with no clax (rc=$RC out=$OUT err=$ERR log=$(hooks_log))"; fi

new_env
CLAX_BIN="$SANDBOX/missing" run exec hook --agent claude stop
if [ "$RC" = 0 ] && [ -z "$OUT" ] && [ "$(printf '%s\n' "$ERR" | grep -c .)" = 1 ] && echo "$ERR" | grep -q "CLAX_BIN is set to"; then
    pass "hook mode with an unusable CLAX_BIN prints one line and exits 0"
else fail "hook mode with an unusable CLAX_BIN (rc=$RC err=$ERR)"; fi

new_env
printf '#!/bin/sh\nif [ "$1" = "--version" ]; then echo "clax %s"; exit 0; fi\necho "partial output"\necho "boom: daemon exploded" >&2\nexit 3\n' "$V" > "$FAKEBIN/clax"
chmod +x "$FAKEBIN/clax"
run exec hook --agent claude prompt
if [ "$RC" = 0 ] && [ -z "$OUT" ] && echo "$ERR" | grep -q "boom: daemon exploded" \
    && hooks_log | grep -q "launcher mode=hook agent=claude exit=3 reason=\"clax exited 3: boom: daemon exploded\""; then
    pass "a failing hook binary exits 0, drops its stdout and is logged with its stderr"
else fail "a failing hook binary (rc=$RC out=$OUT err=$ERR log=$(hooks_log))"; fi

new_env
fake_clax "$FAKEBIN" "clax 0.0.1"
run exec hook --agent codex stop
if [ "$RC" = 0 ] && [ "$OUT" = "args: hook --agent codex stop" ] && [ -z "$ERR" ] && [ -z "$(hooks_log)" ]; then
    pass "a succeeding hook passes its stdout through and logs nothing, whatever its version"
else fail "a succeeding hook passes its stdout through (rc=$RC out=$OUT err=$ERR log=$(hooks_log))"; fi

new_env
export CLAX_HOME="$SANDBOX/ax-home"
mkdir -p "$CLAX_HOME/logs"
awk 'BEGIN { for (i = 0; i < 20000; i++) print "0123456789012345678901234567890123456789012345678901234567890123" }' > "$CLAX_HOME/logs/hooks.log"
run exec hook --agent claude stop
if [ "$RC" = 0 ] && [ -s "$CLAX_HOME/logs/hooks.log.1" ] && [ "$(wc -l < "$CLAX_HOME/logs/hooks.log" | tr -d ' ')" = 1 ] \
    && grep -q "agent=claude" "$CLAX_HOME/logs/hooks.log"; then
    pass "hooks.log rotates to hooks.log.1 past 1 MiB, under CLAX_HOME"
else fail "hooks.log rotates past 1 MiB (rc=$RC)"; fi

new_env
export CLAX_HOME="$SANDBOX/not-a-dir"
echo file > "$CLAX_HOME"
run exec hook --agent codex stop
if [ "$RC" = 0 ] && [ -z "$OUT" ]; then pass "an unwritable log does not fail a hook"; else fail "an unwritable log does not fail a hook (rc=$RC err=$ERR)"; fi

# --- No alias for the previous name -------------------------------------------
# Its variables, its binary on PATH and its home are all ignored and left
# untouched. The name is assembled from two halves so the name gate finds no
# literal.
OLD="arti""fax"
OLD_UPPER="ARTI""FAX"
new_env
fake_clax "$SANDBOX/elsewhere" "clax $V"
printf '#!/bin/sh\necho "%s %s"\n' "$OLD" "$V" > "$FAKEBIN/$OLD"
chmod +x "$FAKEBIN/$OLD"
mkdir -p "$HOME/.$OLD/bin"
cp "$SANDBOX/elsewhere/clax" "$HOME/.$OLD/bin/clax"
export "${OLD_UPPER}_BIN=$SANDBOX/elsewhere/clax" "${OLD_UPPER}_HOME=$HOME/.$OLD"
run exec hook --agent codex stop
unset "${OLD_UPPER}_BIN" "${OLD_UPPER}_HOME"
if [ "$RC" = 0 ] && [ -z "$OUT" ] && echo "$ERR" | grep -q "no clax binary is on PATH" \
    && [ ! -e "$HOME/.$OLD/logs" ] && [ -s "$HOME/.clax/logs/hooks.log" ] \
    && [ "$(PATH="$ORIG_PATH" ls -A "$HOME/.$OLD")" = bin ]; then
    pass "the previous name's variables, binary and home are ignored and left untouched"
else fail "the previous name's variables, binary and home are ignored (rc=$RC out=$OUT err=$ERR)"; fi

[ "$FAILED" = 0 ] && echo "all wrapper tests passed" || echo "wrapper tests FAILED"
exit "$FAILED"
```

Run: `bash scripts/test-ensure-clax.sh`
Expected: FAIL. The old launcher searches outside `PATH` and has no fallback server.

- [ ] **Step 2: Write the wrapper**

Replace `scripts/ensure-clax.sh` with the script below. `CLAX_VERSION` is the workspace version (`0.2.0` until Task 12).

```bash
#!/usr/bin/env bash
# Runs the `clax` on PATH for the Clax plugins, and says why when it cannot.
#
# Usage:
#   ensure-clax.sh                   print the path of the clax that would run
#   ensure-clax.sh exec mcp <args>   run the MCP server
#   ensure-clax.sh exec hook <args>  run a hook (always exits 0)
#   ensure-clax.sh exec <args>       run any other clax command
#
# The binary is $CLAX_BIN when set (it must then be a usable clax), else the
# first `clax` on PATH whose --version names clax. This script never
# downloads, builds, or looks anywhere else.
#
# With no binary, MCP mode answers the MCP client itself with a minimal
# server whose one tool, `status`, states the reason; hook mode prints one
# line and exits 0; other modes print the reason and exit 1. A binary whose
# version is not $CLAX_VERSION (this plugin's) runs, with a warning.
#
# Every failure, and every MCP start, appends one line to
# ${CLAX_HOME:-~/.clax}/logs/hooks.log (rotated to hooks.log.1 past 1 MiB).

set -uo pipefail

# This plugin's Clax version; a clax of another version runs with a warning.
CLAX_VERSION="0.2.0"
LOG_MAX_BYTES=1048576
ARGV="$*"

case "${1:-}" in
    "") MODE=print ;;
    exec)
        case "${2:-}" in
            mcp) MODE=mcp ;;
            hook) MODE=hook ;;
            *) MODE=cli ;;
        esac
        ;;
    *) echo "usage: ensure-clax.sh [exec <clax arguments...>]" >&2; exit 2 ;;
esac
AGENT=-
prev=""
for a in "$@"; do
    if [ "$prev" = --agent ]; then AGENT="$a"; fi
    prev="$a"
done
BIN="" GOT_VERSION="" WARNING="" REASON="" TRIED=""

log() { echo "$@" >&2; }
oneline() { printf '%s' "$1" | tr '\n"' " '"; }

# Appends "<time> $1" to hooks.log, rotating it past LOG_MAX_BYTES. Never fails.
hooks_log() {
    {
        local dir size
        if [ -n "${CLAX_HOME:-}" ]; then dir="$CLAX_HOME/logs"
        elif [ -n "${HOME:-}" ]; then dir="$HOME/.clax/logs"
        else return 0; fi
        mkdir -p "$dir" || return 0
        if [ -f "$dir/hooks.log" ]; then
            size="$(wc -c < "$dir/hooks.log" | tr -d ' ')"
            if [ "${size:-0}" -gt "$LOG_MAX_BYTES" ]; then mv -f "$dir/hooks.log" "$dir/hooks.log.1"; fi
        fi
        printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" >> "$dir/hooks.log"
    } 2>/dev/null || true
}

# Logs this run's failure, with $1 as its exit status.
fail_line() {
    hooks_log "launcher mode=$MODE agent=$AGENT exit=$1 reason=\"$(oneline "$REASON")\" tried=\"$(oneline "$TRIED")\" argv=\"$(oneline "$ARGV")\""
}

# True when $1 is an executable file whose --version names clax; sets
# GOT_VERSION to that line.
check_bin() {
    GOT_VERSION=""
    [ -f "$1" ] && [ -x "$1" ] || return 1
    GOT_VERSION="$("$1" --version 2>/dev/null < /dev/null | head -1)"
    case "$GOT_VERSION" in "clax "*) return 0 ;; *) return 1 ;; esac
}

# Sets BIN (and GOT_VERSION, TRIED); on failure sets REASON and returns 1.
resolve() {
    local dir IFS=:
    TRIED=""
    if [ -n "${CLAX_BIN:-}" ]; then
        if check_bin "$CLAX_BIN"; then
            BIN="$CLAX_BIN"
            TRIED="CLAX_BIN=$CLAX_BIN: $GOT_VERSION"
            return 0
        fi
        TRIED="CLAX_BIN=$CLAX_BIN: not a usable clax"
        REASON="CLAX_BIN is set to '$CLAX_BIN', which is not a usable clax binary. Unset CLAX_BIN, or point it at a clax binary."
        return 1
    fi
    for dir in ${PATH:-}; do
        [ -n "$dir" ] || continue
        if check_bin "$dir/clax"; then
            BIN="$dir/clax"
            TRIED="${TRIED:+$TRIED; }$BIN: $GOT_VERSION"
            return 0
        fi
        if [ -e "$dir/clax" ]; then TRIED="${TRIED:+$TRIED; }$dir/clax: not clax"; fi
    done
    TRIED="${TRIED:+$TRIED; }PATH has no clax: ${PATH:-(empty)}"
    REASON="no clax binary is on PATH. Install it with \`just install\` in a Clax checkout (it puts clax in ~/.cargo/bin), or with the release installer (~/.local/bin), and start the harness from a shell whose PATH includes that directory."
    return 1
}

json_string() {
    local s="$1" out="" c i
    for (( i = 0; i < ${#s}; i++ )); do
        c="${s:i:1}"
        case "$c" in
            '"') out="$out\\\"" ;;
            '\') out="$out\\\\" ;;
            $'\n') out="$out\\n" ;;
            $'\t') out="$out\\t" ;;
            $'\r') out="$out\\r" ;;
            [[:cntrl:]]) ;;
            *) out="$out$c" ;;
        esac
    done
    printf '"%s"' "$out"
}
json_field() { printf '%s' "$2" | sed -nE "s/.*\"$1\"[[:space:]]*:[[:space:]]*\"([^\"]*)\".*/\\1/p" | head -1; }
reply() { printf '{"jsonrpc":"2.0","id":%s,"result":%s}\n' "$1" "$2"; }

# The status tool's text: the reason, or, once clax has appeared since, that
# it is there now.
status_text() {
    local found
    if found="$(resolve > /dev/null 2>&1 && echo "$BIN")" && [ -n "$found" ]; then
        echo "clax is now available at $found. Reconnect the clax MCP server (/mcp in Claude Code), or start a new session, to use it."
    else
        echo "$1"
    fi
}

# A minimal MCP server on stdin/stdout whose one tool, status, states why clax
# cannot run, so the client and the agent see the reason instead of a closed
# pipe. Answers until stdin closes.
serve_unavailable() {
    local text line method id proto
    text="Clax is unavailable: $REASON (Details: ${CLAX_HOME:-~/.clax}/logs/hooks.log.)"
    while IFS= read -r line || [ -n "$line" ]; do
        method="$(json_field method "$line")"
        id="$(printf '%s' "$line" | sed -nE 's/.*"id"[[:space:]]*:[[:space:]]*("([^"\\]|\\.)*"|-?[0-9]+).*/\1/p' | head -1)"
        [ -n "$method" ] && [ -n "$id" ] || continue
        case "$method" in
            initialize)
                proto="$(json_field protocolVersion "$line")"
                reply "$id" "{\"protocolVersion\":\"${proto:-2025-06-18}\",\"capabilities\":{\"tools\":{}},\"serverInfo\":{\"name\":\"clax\",\"version\":\"$CLAX_VERSION\"},\"instructions\":$(json_string "$text")}"
                ;;
            tools/list)
                reply "$id" "{\"tools\":[{\"name\":\"status\",\"description\":$(json_string "Clax could not start. Call this tool for the reason and the fix."),\"inputSchema\":{\"type\":\"object\",\"properties\":{}}}]}"
                ;;
            tools/call)
                reply "$id" "{\"content\":[{\"type\":\"text\",\"text\":$(json_string "$(status_text "$text")")}],\"isError\":true}"
                ;;
            ping) reply "$id" "{}" ;;
            *) printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32601,"message":%s}}\n' "$id" "$(json_string "$text")" ;;
        esac
    done
}

# Runs a hook with the binary and always exits 0: a hook must never fail its
# harness. Its stderr passes through; its stdout only when it exited 0. A
# non-zero exit is logged with the end of its stderr.
run_hook() {
    local bin="$1" out rc errfile tail=""
    shift
    errfile="$(mktemp 2>/dev/null)" || errfile=""
    if [ -n "$errfile" ]; then
        if out="$("$bin" "$@" 2>"$errfile")"; then rc=0; else rc=$?; fi
        cat "$errfile" >&2 2>/dev/null || true
        tail="$(tr '\n"' " '" < "$errfile" 2>/dev/null)" || tail=""
        rm -f "$errfile"
    else
        if out="$("$bin" "$@")"; then rc=0; else rc=$?; fi
    fi
    if [ "$rc" = 0 ]; then
        if [ -n "$out" ]; then printf '%s\n' "$out"; fi
    else
        tail="${tail% }"
        if [ "${#tail}" -gt 200 ]; then tail="${tail: -200}"; fi
        REASON="clax exited ${rc}: ${tail}"
        fail_line "$rc"
    fi
    exit 0
}

main() {
    if resolve; then
        if [ "$GOT_VERSION" != "clax $CLAX_VERSION" ]; then
            WARNING="$BIN is $GOT_VERSION, but this plugin is clax $CLAX_VERSION; run \`just install\` (or \`clax init\`) so the plugin and the binary match"
        fi
        case "$MODE" in
            print) echo "$BIN"; exit 0 ;;
            hook) shift; run_hook "$BIN" "$@" ;;
            mcp)
                if [ -n "$WARNING" ]; then log "clax: warning: $WARNING"; fi
                hooks_log "launch mode=mcp agent=$AGENT bin=\"$(oneline "$BIN")\" version=\"$GOT_VERSION\" warning=\"$(oneline "$WARNING")\""
                shift
                exec "$BIN" "$@"
                ;;
            *)
                if [ -n "$WARNING" ]; then log "clax: warning: $WARNING"; fi
                shift
                exec "$BIN" "$@"
                ;;
        esac
    fi
    case "$MODE" in
        hook)
            log "clax: $REASON"
            fail_line 0
            exit 0
            ;;
        mcp)
            fail_line fallback
            log "clax: $REASON"
            serve_unavailable
            exit 0
            ;;
        *)
            log "clax: $REASON"
            fail_line 1
            exit 1
            ;;
    esac
}

main "$@"
```

Copy it to both plugins and make all three executable:

```bash
cp scripts/ensure-clax.sh plugins/claude-code/scripts/ensure-clax.sh
cp scripts/ensure-clax.sh plugins/clax/scripts/ensure-clax.sh
chmod +x scripts/ensure-clax.sh plugins/claude-code/scripts/ensure-clax.sh plugins/clax/scripts/ensure-clax.sh
```

- [ ] **Step 3: Codex forwards only what the wrapper and the binary read**

In `plugins/clax/.mcp.json`, set `env_vars` to exactly `["CLAX_HOME", "CLAX_NO_OPEN", "CLAX_BIN", "CLAX_CODEX_BIN"]`. Codex passes `PATH` and `HOME` to MCP servers by default.

In `scripts/test-plugins.sh`:
- In the Python block of the "Codex MCP server and hooks use --agent codex" check, add before `sys.exit(0 if ok else 1)`:

```python
ok = ok and server.get("env_vars") == ["CLAX_HOME", "CLAX_NO_OPEN", "CLAX_BIN", "CLAX_CODEX_BIN"]
```

- In the "One version everywhere" block, change the installer's pattern from `r'^MIN_VERSION="([^"]+)"'` to `r'^CLAX_VERSION="([^"]+)"'`, and its label from `" MIN_VERSION"` to `" CLAX_VERSION"`. Task 7 replaces the whole block with `scripts/check-version.sh`.

- [ ] **Step 4: Run the tests**

Run: `bash scripts/test-ensure-clax.sh`
Expected: every line `PASS`, then `all wrapper tests passed`.

Run: `bash scripts/test-plugins.sh | tail -1 && cargo test -p clax-cli doctor_agent`
Expected: `plugin checks passed`, and the doctor tests pass. They compare the plugins' wrapper copies with the one the binary embeds, which Step 2 kept identical.

The smoke scripts (`scripts/smoke-claude.sh`, `smoke-codex.sh`, `smoke-pi.sh`) set `CLAX_BIN` to the working tree's build, which the wrapper still honours, so they need no change.

- [ ] **Step 5: Gates and commit**

Run: `bash scripts/quality_gates.sh; echo "exit=$?"`
Expected: `exit=0`.

```bash
git add scripts/ensure-clax.sh plugins/claude-code/scripts/ensure-clax.sh plugins/clax/scripts/ensure-clax.sh scripts/test-ensure-clax.sh plugins/clax/.mcp.json scripts/test-plugins.sh
git commit -m "Reduce the plugins' launcher to clax on PATH; a fallback MCP server states why clax cannot run"
git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed
```

---

### Task 5: Which binary runs: `doctor --agent` and `status`

**Files:**
- Modify: `crates/clax-cli/src/commands/doctor_agent.rs`, `crates/clax-mcp/src/tools.rs`, `crates/clax-mcp/tests/tools.rs`, `plugins/pi/src/daemon.ts`, `plugins/pi/src/clax.ts`, `plugins/pi/test/clax.test.ts`

**Interfaces:**
- Produces: `doctor_agent::clax_on_path(path: &OsStr) -> Vec<(PathBuf, Option<String>)>`. It returns every file named `clax` in the `PATH` value, in order, with the first line of its `--version` when that names clax.
- Produces: the `binary` check (`binary_check(exe, version, clax_bin, on_path)`). It names this executable and the binary the plugins' wrapper would run (`$CLAX_BIN`, else the first clax on `PATH`), and lists every `clax` on `PATH`. It fails when the wrapper would run no clax, or another one than this. The `plugin` check already fails on a plugin whose version differs from the binary's, so together they report a version mismatch.
- Produces: `status` gains `binary: {"path", "version"}`: this process's executable and version, in the shim and in Pi. `plugin_version` and `skew` are unchanged.
- Produces: Pi's install hint names `just install` and `install.sh`.

- [ ] **Step 1: Write the failing doctor tests**

Add to the test module of `crates/clax-cli/src/commands/doctor_agent.rs`:

```rust
    /// A script named clax in `dir` whose --version prints `line`.
    fn fake_clax(dir: &Path, line: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(dir).unwrap();
        let p = dir.join("clax");
        std::fs::write(&p, format!("#!/bin/sh\necho '{line}'\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p.canonicalize().unwrap()
    }

    #[test]
    fn clax_on_path_lists_every_clax_in_order() {
        let t = tempfile::tempdir().unwrap();
        let a = fake_clax(&t.path().join("a"), "other 1.0");
        let b = fake_clax(&t.path().join("b"), "clax 0.3.0");
        let path = std::env::join_paths([t.path().join("a"), t.path().join("none"), t.path().join("b")]).unwrap();
        let found = clax_on_path(&path);
        assert_eq!(found.len(), 2);
        assert_eq!((found[0].0.canonicalize().unwrap(), found[0].1.clone()), (a, None));
        assert_eq!((found[1].0.canonicalize().unwrap(), found[1].1.clone()), (b, Some("clax 0.3.0".into())));
    }

    #[test]
    fn binary_passes_when_the_plugins_run_this_clax() {
        let t = tempfile::tempdir().unwrap();
        let me = fake_clax(&t.path().join("me"), "clax 0.3.0");
        let other = fake_clax(&t.path().join("other"), "clax 0.2.0");
        let v = binary_check(&me, "0.3.0", None, &[(me.clone(), Some("clax 0.3.0".into())), (other.clone(), Some("clax 0.2.0".into()))]);
        assert_eq!(v["ok"], true, "{v}");
        let d = v["detail"].as_str().unwrap();
        assert!(d.contains(&format!("the plugins run: {} (clax 0.3.0)", me.display())), "{d}");
        assert!(d.contains(&format!("{} (clax 0.2.0)", other.display())), "{d}");
    }

    #[test]
    fn binary_fails_when_another_clax_comes_first_or_none_is_on_path() {
        let t = tempfile::tempdir().unwrap();
        let me = fake_clax(&t.path().join("me"), "clax 0.3.0");
        let first = fake_clax(&t.path().join("first"), "clax 0.2.0");
        let v = binary_check(&me, "0.3.0", None, &[(first.clone(), Some("clax 0.2.0".into())), (me.clone(), Some("clax 0.3.0".into()))]);
        assert_eq!(v["ok"], false);
        assert!(v["detail"].as_str().unwrap().contains("the plugins run another clax than this one"), "{v}");
        let v = binary_check(&me, "0.3.0", None, &[]);
        assert_eq!(v["ok"], false);
        assert!(v["detail"].as_str().unwrap().contains("the plugins run: nothing (no clax on PATH)"), "{v}");
        let v = binary_check(&me, "0.3.0", Some(me.to_str().unwrap()), &[]);
        assert_eq!(v["ok"], true, "CLAX_BIN names this binary: {v}");
    }
```

Run: `cargo test -p clax-cli doctor_agent`
Expected: FAIL (the functions do not exist in this form).

- [ ] **Step 2: Implement**

Replace `binary_check` in `doctor_agent.rs` with:

```rust
/// Every file named `clax` in the `PATH` value `path`, in order, with the
/// first line of its `--version` when that names clax.
pub fn clax_on_path(path: &std::ffi::OsStr) -> Vec<(PathBuf, Option<String>)> {
    std::env::split_paths(path)
        .map(|d| d.join("clax"))
        .filter(|p| p.is_file())
        .map(|p| {
            let v = std::process::Command::new(&p)
                .arg("--version")
                .stdin(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .output()
                .ok()
                .and_then(|o| String::from_utf8_lossy(&o.stdout).lines().next().map(str::to_string))
                .filter(|l| l.starts_with("clax "));
            (p, v)
        })
        .collect()
}

/// `binary`: this executable and its version, the binary the plugins'
/// wrapper runs (`$CLAX_BIN`, else the first clax on `PATH`), and every
/// clax on `PATH`; failed when the wrapper runs none, or another one.
pub fn binary_check(
    exe: &Path,
    version: &str,
    clax_bin: Option<&str>,
    on_path: &[(PathBuf, Option<String>)],
) -> Value {
    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let runs: Option<(PathBuf, String)> = match clax_bin.filter(|b| !b.is_empty()) {
        Some(b) => Some((PathBuf::from(b), "from CLAX_BIN".into())),
        None => on_path
            .iter()
            .find_map(|(p, v)| v.as_ref().map(|v| (p.clone(), v.clone()))),
    };
    let mut lines = vec![format!("this clax: {} (clax {version})", exe.display())];
    let ok = match &runs {
        Some((p, v)) => {
            lines.push(format!("the plugins run: {} ({v})", p.display()));
            canon(p) == canon(exe)
        }
        None => {
            lines.push("the plugins run: nothing (no clax on PATH)".into());
            false
        }
    };
    let listed: Vec<String> = on_path
        .iter()
        .map(|(p, v)| format!("{} ({})", p.display(), v.as_deref().unwrap_or("not clax")))
        .collect();
    lines.push(format!(
        "on PATH, in order: {}",
        if listed.is_empty() { "none".to_string() } else { listed.join("; ") }
    ));
    if !ok {
        lines.push(
            "the plugins run another clax than this one, or none: run `just install` in your Clax checkout (or install.sh), and put its directory first on the PATH your harness starts with".into(),
        );
    }
    check("binary", ok, lines.join("\n"))
}
```

In `checks`, replace `let mut out = vec![binary_check(&exe, version)];` with:

```rust
    let on_path = clax_on_path(&std::env::var_os("PATH").unwrap_or_default());
    let clax_bin = std::env::var("CLAX_BIN").ok();
    let mut out = vec![binary_check(&exe, version, clax_bin.as_deref(), &on_path)];
```

Update the module doc comment's `binary` line to: ``- `binary`: this `clax`, the one the plugins run (`$CLAX_BIN`, else the first on `PATH`), and every `clax` on `PATH`.`` Update any existing test that called the old `binary_check(exe, version)`.

- [ ] **Step 3: `status` reports the binary**

In `crates/clax-mcp/src/tools.rs`, in `status`, after the `plugin_version` block, add:

```rust
        // Which binary answers: the plugins run the clax on PATH.
        out["binary"] = json!({
            "path": std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default(),
            "version": env!("CARGO_PKG_VERSION"),
        });
```

In `crates/clax-mcp/tests/tools.rs`, in `status_reports_the_plugin_version_and_skew_when_known`, after the first `status` call, add:

```rust
    assert_eq!(s["binary"]["version"], env!("CARGO_PKG_VERSION"));
    assert!(!s["binary"]["path"].as_str().unwrap().is_empty(), "{s}");
```

- [ ] **Step 4: Pi**

In `plugins/pi/src/daemon.ts`, replace `INSTALL_HINT` with:

```ts
/** How to get a binary when none is found. Pi never downloads. */
export const INSTALL_HINT =
  "install clax with `just install` in a Clax checkout (it puts clax in ~/.cargo/bin), or with the release installer " +
  "(~/.local/bin), and start Pi from a shell whose PATH includes that directory; or set CLAX_BIN to a clax binary";
```

In `plugins/pi/src/clax.ts`, in `status`, after the `daemon_version` line, add:

```ts
    // Which binary the extension runs: CLAX_BIN, else the clax on PATH.
    try {
      out.binary = { path: findBinary(this.env), version: VERSION };
    } catch (e) {
      out.binary = { path: null, error: (e as Error).message };
    }
```

and add `findBinary` to its import from `./daemon.ts`. In `plugins/pi/test/clax.test.ts`, in the test "status reports daemon_version only when the daemon's version differs", add `expect(s.binary).toMatchObject({ path: expect.any(String) });`. The tests run with `CLAX_BIN` set, so `path` is the test daemon's binary.

- [ ] **Step 5: Run**

Run: `cargo test -p clax-cli doctor_agent && cargo test -p clax-mcp && (cd plugins/pi && npm run typecheck && npx vitest run)`
Expected: PASS.

- [ ] **Step 6: Gates and commit**

Run: `bash scripts/quality_gates.sh; echo "exit=$?"`
Expected: `exit=0`.

```bash
git add crates/clax-cli/src/commands/doctor_agent.rs crates/clax-mcp/src/tools.rs crates/clax-mcp/tests/tools.rs plugins/pi/src/daemon.ts plugins/pi/src/clax.ts plugins/pi/test/clax.test.ts
git commit -m "Say which clax the plugins run: every clax on PATH in doctor --agent, binary in status"
git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed
```

---

### Task 6: `clax init` and `clax uninit`

**Files:**
- Create: `crates/clax-cli/src/plugins.rs`, `crates/clax-cli/src/commands/init.rs`, `crates/clax-cli/tests/init.rs`
- Modify: `crates/clax-cli/Cargo.toml` (`rust-embed`, `toml`), `crates/clax-cli/src/main.rs`, `crates/clax-cli/src/commands/mod.rs`

**Interfaces:**
- Consumes: `doctor_agent::{Dirs, clax_on_path}` (Task 5).
- Produces: `plugins::files() -> Vec<(String, Vec<u8>)>` (the marketplace tree, relative paths) and `plugins::materialize(root) -> io::Result<()>`.
- Produces: `clax init [--agent claude|codex|pi]... [--json]` and `clax uninit [--agent …]... [--json]`. The default is every harness whose CLI (`claude`, `codex`, `pi`) is on `PATH`. JSON: `{"marketplace": "<root>", "agents": [{"agent", "status": "registered"|"removed"|"skipped"|"failed", "detail", "commands": ["claude plugin …", …]}]}`. The exit status is 1 when any harness failed; a skipped harness is not a failure.

- [ ] **Step 1: Dependencies**

In `crates/clax-cli/Cargo.toml` `[dependencies]`, add `rust-embed.workspace = true` and `toml.workspace = true`.

- [ ] **Step 2: Write the failing tests**

Create `crates/clax-cli/tests/init.rs`:

```rust
//! `clax init` / `clax uninit` against fake `claude`, `codex` and `pi`
//! commands and scratch harness configuration directories. The real
//! harnesses and their real configuration are never touched.

use assert_cmd::Command;
use std::path::{Path, PathBuf};

/// The previous name, assembled so the repository's name gate finds no literal.
const OLD: &str = concat!("arti", "fax");
const REPO: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    /// A scratch HOME with fake CLIs for `harnesses` in `fakebin`. Each fake
    /// appends "<name> <args>" to `calls`, and exits 1 when that line is
    /// listed in `fail`.
    fn new(harnesses: &[&str]) -> Env {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("fakebin");
        std::fs::create_dir_all(&bin).unwrap();
        for h in harnesses {
            let p = bin.join(h);
            std::fs::write(
                &p,
                format!(
                    "#!/bin/sh\nline=\"{h} $*\"\necho \"$line\" >> '{calls}'\nif grep -qxF \"$line\" '{fail}' 2>/dev/null; then echo \"$line failed\" >&2; exit 1; fi\nexit 0\n",
                    calls = dir.path().join("calls").display(),
                    fail = dir.path().join("fail").display(),
                ),
            )
            .unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        Env { dir }
    }
    fn p(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }
    fn root(&self) -> PathBuf {
        self.p("ax/marketplace")
    }
    fn cmd(&self) -> Command {
        let mut c = Command::cargo_bin("clax").unwrap();
        c.env("HOME", self.dir.path())
            .env("CLAX_HOME", self.p("ax"))
            .env("CLAUDE_CONFIG_DIR", self.p("claude"))
            .env("CODEX_HOME", self.p("codex"))
            .env("PI_CODING_AGENT_DIR", self.p("pi"))
            .env("PATH", format!("{}:/usr/bin:/bin", self.p("fakebin").display()))
            .env_remove("CLAX_BIN");
        c
    }
    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.p("calls"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
    fn json(&self, args: &[&str]) -> (bool, serde_json::Value) {
        let out = self.cmd().args(args).arg("--json").output().unwrap();
        let v = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)));
        (out.status.success(), v)
    }
}

fn status(v: &serde_json::Value, agent: &str) -> String {
    v["agents"].as_array().unwrap().iter().find(|a| a["agent"] == agent).unwrap()["status"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Every file under `dir`, relative, skipping `skip` top-level names.
fn tree(dir: &Path, skip: &[&str]) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            let rel = p.strip_prefix(dir).unwrap().to_string_lossy().to_string();
            if skip.iter().any(|s| rel == *s || rel.starts_with(&format!("{s}/"))) {
                continue;
            }
            if p.is_dir() { stack.push(p) } else { out.push((rel, std::fs::read(&p).unwrap())) }
        }
    }
    out.sort();
    out
}

#[test]
fn init_writes_the_embedded_plugins_as_a_marketplace() {
    let e = Env::new(&[]);
    let (ok, v) = e.json(&["init"]);
    assert!(ok, "{v}");
    let root = e.root();
    assert_eq!(v["marketplace"], root.display().to_string());
    let repo = Path::new(REPO);
    assert_eq!(tree(&root.join("plugins/claude-code"), &[]), tree(&repo.join("plugins/claude-code"), &[]));
    assert_eq!(tree(&root.join("plugins/clax"), &[]), tree(&repo.join("plugins/clax"), &[]));
    assert_eq!(
        tree(&root.join("plugins/pi"), &[]),
        tree(&repo.join("plugins/pi"), &["node_modules", "test", "tsconfig.json", "vitest.config.ts", "package-lock.json"])
    );
    for m in [".claude-plugin/marketplace.json", ".agents/plugins/marketplace.json"] {
        assert_eq!(std::fs::read(root.join(m)).unwrap(), std::fs::read(repo.join(m)).unwrap(), "{m}");
    }
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(root.join("plugins/clax/scripts/ensure-clax.sh")).unwrap().permissions().mode();
    assert_eq!(mode & 0o111, 0o111, "scripts are executable");
    // No harness CLI on PATH: every harness is skipped, which is not a failure.
    for a in ["claude", "codex", "pi"] {
        assert_eq!(status(&v, a), "skipped", "{v}");
    }
}

#[test]
fn init_registers_each_harness_on_path_through_its_cli() {
    let e = Env::new(&["claude", "codex", "pi"]);
    let (ok, v) = e.json(&["init"]);
    assert!(ok, "{v}");
    let r = e.root().display().to_string();
    assert_eq!(
        e.calls(),
        vec![
            "claude plugin uninstall clax@clax".to_string(),
            "claude plugin marketplace remove clax".into(),
            format!("claude plugin marketplace add {r}"),
            "claude plugin install clax@clax".into(),
            "codex plugin remove clax@clax".into(),
            "codex plugin marketplace remove clax".into(),
            format!("codex plugin marketplace add {r}"),
            "codex plugin add clax@clax".into(),
            format!("pi install {r}/plugins/pi"),
        ]
    );
    for a in ["claude", "codex", "pi"] {
        assert_eq!(status(&v, a), "registered", "{v}");
    }
}

#[test]
fn init_only_touches_the_harnesses_asked_for() {
    let e = Env::new(&["claude", "codex", "pi"]);
    let (ok, v) = e.json(&["init", "--agent", "pi"]);
    assert!(ok, "{v}");
    assert_eq!(e.calls(), vec![format!("pi install {}/plugins/pi", e.root().display())]);
    assert_eq!(v["agents"].as_array().unwrap().len(), 1);
}

#[test]
fn init_removes_stale_registrations_including_the_previous_names_and_nothing_else() {
    let e = Env::new(&["claude", "codex", "pi"]);
    let w = |rel: &str, text: String| {
        std::fs::create_dir_all(e.p(rel).parent().unwrap()).unwrap();
        std::fs::write(e.p(rel), text).unwrap();
    };
    w("claude/plugins/installed_plugins.json", format!(r#"{{"version":2,"plugins":{{"{OLD}@{OLD}":[{{}}],"other@other":[{{}}]}}}}"#));
    w("claude/plugins/known_marketplaces.json", format!(r#"{{"{OLD}":{{}},"other":{{}}}}"#));
    w("codex/config.toml", format!("[marketplaces.{OLD}]\nsource = \"/x\"\n\n[marketplaces.other]\nsource = \"/y\"\n\n[plugins.\"{OLD}@{OLD}\"]\nenabled = true\n"));
    w("oldpkg/package.json", format!(r#"{{"name":"@empathic/{OLD}-pi"}}"#));
    w("claxpkg/package.json", r#"{"name":"@empathic/clax-pi"}"#.into());
    w("otherpkg/package.json", r#"{"name":"@someone/else"}"#.into());
    w("pi/settings.json", r#"{"packages":["../oldpkg","../claxpkg","../otherpkg","npm:@x/y"]}"#.into());
    // The previous name's home, which must stay exactly as it is.
    w(&format!(".{OLD}/marker"), "keep".into());

    let (ok, v) = e.json(&["init"]);
    assert!(ok, "{v}");
    let calls = e.calls();
    for want in [
        format!("claude plugin uninstall {OLD}@{OLD}"),
        format!("claude plugin marketplace remove {OLD}"),
        format!("codex plugin remove {OLD}@{OLD}"),
        format!("codex plugin marketplace remove {OLD}"),
        // Pi packages are named by their canonical directory.
        format!("pi remove {}", e.p("oldpkg").canonicalize().unwrap().display()),
        format!("pi remove {}", e.p("claxpkg").canonicalize().unwrap().display()),
    ] {
        assert!(calls.contains(&want), "missing {want:?} in {calls:#?}");
    }
    assert!(!calls.iter().any(|c| c.contains("other")), "{calls:#?}");
    assert_eq!(std::fs::read_to_string(e.p(&format!(".{OLD}/marker"))).unwrap(), "keep");
    assert_eq!(std::fs::read_dir(e.p(&format!(".{OLD}"))).unwrap().count(), 1);
}

#[test]
fn a_failing_harness_is_reported_and_the_others_still_register() {
    let e = Env::new(&["claude", "codex", "pi"]);
    std::fs::write(e.p("fail"), format!("codex plugin marketplace add {}\n", e.root().display())).unwrap();
    let (ok, v) = e.json(&["init"]);
    assert!(!ok, "a failed harness makes init exit 1");
    assert_eq!(status(&v, "codex"), "failed");
    assert!(v["agents"][1]["detail"].as_str().unwrap().contains("failed"), "{v}");
    assert_eq!(status(&v, "claude"), "registered");
    assert_eq!(status(&v, "pi"), "registered");
}

#[test]
fn init_twice_does_the_same_again_and_uninit_removes_registrations_and_the_marketplace_only() {
    let e = Env::new(&["claude", "codex", "pi"]);
    assert!(e.json(&["init"]).0);
    let first = e.calls();
    std::fs::remove_file(e.p("calls")).unwrap();
    assert!(e.json(&["init"]).0);
    assert_eq!(e.calls(), first);
    std::fs::write(e.p("ax/clax.db"), "data").unwrap();
    std::fs::remove_file(e.p("calls")).unwrap();
    let (ok, v) = e.json(&["uninit"]);
    assert!(ok, "{v}");
    assert_eq!(
        e.calls(),
        vec![
            "claude plugin uninstall clax@clax".to_string(),
            "claude plugin marketplace remove clax".into(),
            "codex plugin remove clax@clax".into(),
            "codex plugin marketplace remove clax".into(),
        ]
    );
    assert!(!e.root().exists());
    assert_eq!(std::fs::read_to_string(e.p("ax/clax.db")).unwrap(), "data");
    assert_eq!(status(&v, "pi"), "removed");
}
```

The `pi install` in the second test finds no removal first, because the scratch `settings.json` does not exist yet. The uninit test's `pi` removes nothing for the same reason: the fakes do not write `settings.json`.

Run: `cargo test -p clax-cli --test init`
Expected: FAIL (`unrecognized subcommand 'init'`).

- [ ] **Step 3: The embedded plugin tree**

Create `crates/clax-cli/src/plugins.rs`:

```rust
//! The plugins this binary was built with, and writing them out as a
//! marketplace directory (`clax init`). The layout matches the repository:
//! `.claude-plugin/marketplace.json`, `.agents/plugins/marketplace.json` and
//! `plugins/{claude-code,clax,pi}`, so both manifests' `./plugins/<name>`
//! sources resolve. The Pi package carries what its `package.json` ships.

use rust_embed::RustEmbed;
use std::path::Path;

#[derive(RustEmbed)]
#[folder = "../../plugins/"]
#[include = "claude-code/**"]
#[include = "clax/**"]
#[include = "pi/package.json"]
#[include = "pi/README.md"]
#[include = "pi/src/**"]
#[include = "pi/skills/**"]
struct Plugins;

const CLAUDE_MARKETPLACE: &str = include_str!("../../../.claude-plugin/marketplace.json");
const CODEX_MARKETPLACE: &str = include_str!("../../../.agents/plugins/marketplace.json");

/// Every file of the marketplace tree: (path relative to its root, contents).
pub fn files() -> Vec<(String, Vec<u8>)> {
    let mut out = vec![
        (".claude-plugin/marketplace.json".to_string(), CLAUDE_MARKETPLACE.as_bytes().to_vec()),
        (".agents/plugins/marketplace.json".to_string(), CODEX_MARKETPLACE.as_bytes().to_vec()),
    ];
    for p in Plugins::iter() {
        let f = Plugins::get(&p).expect("an embedded file lists itself");
        out.push((format!("plugins/{p}"), f.data.into_owned()));
    }
    out.sort();
    out
}

/// Writes the tree to `root`, replacing what is there. It is built in a
/// sibling temporary directory and renamed into place, so a harness never
/// reads a half-written plugin. Scripts (`*.sh`) are 0755, other files 0644.
pub fn materialize(root: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let parent = root.parent().expect("the marketplace root has a parent");
    std::fs::create_dir_all(parent)?;
    let pid = std::process::id();
    let tmp = parent.join(format!(".marketplace.{pid}.tmp"));
    let old = parent.join(format!(".marketplace.{pid}.old"));
    let _ = std::fs::remove_dir_all(&tmp);
    for (rel, data) in files() {
        let path = tmp.join(&rel);
        std::fs::create_dir_all(path.parent().expect("a file has a parent"))?;
        std::fs::write(&path, data)?;
        let mode = if rel.ends_with(".sh") { 0o755 } else { 0o644 };
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))?;
    }
    if root.exists() {
        std::fs::rename(root, &old)?;
    }
    std::fs::rename(&tmp, root)?;
    let _ = std::fs::remove_dir_all(&old);
    Ok(())
}
```

Add `mod plugins;` to `crates/clax-cli/src/main.rs`. If `init_writes_the_embedded_plugins_as_a_marketplace` later shows that dotfiles such as `claude-code/.claude-plugin/plugin.json` are missing, add explicit `#[include = "claude-code/.claude-plugin/*"]` and `#[include = "clax/.codex-plugin/*"]` (and the `.mcp.json` files) rather than weakening the test.

- [ ] **Step 4: The commands**

Create `crates/clax-cli/src/commands/init.rs`:

```rust
//! `clax init` and `clax uninit`: register the plugins this binary embeds
//! with each harness through the harness's own CLI, and remove them.
//!
//! `init` writes the marketplace tree ([`crate::plugins`]) to
//! `<home>/marketplace`, then for each harness removes the existing `clax`
//! registration and any under the previous product name (found in the
//! harness's own registry), and adds the new one. `uninit` does the
//! removals and deletes the marketplace directory. Clax's data is never
//! touched, nor the previous name's home.

use super::doctor_agent::Dirs;
use clax_core::Home;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// The previous product name, assembled so the name gate finds no literal.
const OLD: &str = concat!("arti", "fax");
/// The Pi package's name.
const PI_PACKAGE: &str = "@empathic/clax-pi";

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Harness {
    Claude,
    Codex,
    Pi,
}

impl Harness {
    const ALL: [Harness; 3] = [Harness::Claude, Harness::Codex, Harness::Pi];
    fn cli(self) -> &'static str {
        match self {
            Harness::Claude => "claude",
            Harness::Codex => "codex",
            Harness::Pi => "pi",
        }
    }
}

#[derive(clap::Args)]
pub struct Args {
    /// Only this harness (repeatable). Default: every one whose CLI
    /// (`claude`, `codex`, `pi`) is on PATH.
    #[arg(long = "agent", value_enum)]
    pub agents: Vec<Harness>,
}

/// One harness command; a failure of a `required` one fails the harness.
struct Step {
    args: Vec<String>,
    required: bool,
}

fn step(required: bool, args: &[&str]) -> Step {
    Step { args: args.iter().map(|s| s.to_string()).collect(), required }
}

fn read_json(p: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()
}

/// Whether a JSON registry names `key`, at the top level or under `plugins`.
fn names(v: &Option<Value>, key: &str) -> bool {
    v.as_ref().is_some_and(|v| v.get(key).is_some() || v["plugins"].get(key).is_some())
}

/// The installed Pi packages (absolute directories) whose `package.json`
/// `name` is one of `wanted`, from `<pi dir>/settings.json` (local paths
/// there are relative to that directory).
fn pi_packages(pi_dir: &Path, wanted: &[String]) -> Vec<PathBuf> {
    let Some(v) = read_json(&pi_dir.join("settings.json")) else { return Vec::new() };
    v["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| p.as_str().or_else(|| p["source"].as_str()))
        .filter(|s| !s.contains(':'))
        .map(|s| {
            let p = Path::new(s);
            let abs = if p.is_absolute() { p.to_path_buf() } else { pi_dir.join(p) };
            abs.canonicalize().unwrap_or(abs)
        })
        .filter(|d| {
            read_json(&d.join("package.json"))
                .and_then(|v| v["name"].as_str().map(str::to_string))
                .is_some_and(|n| wanted.contains(&n))
        })
        .collect()
}

/// The commands that remove `h`'s Clax registration and any under the
/// previous name that its registry shows.
fn removals(h: Harness, dirs: &Dirs) -> Vec<Step> {
    let old_plugin = format!("{OLD}@{OLD}");
    let mut out = Vec::new();
    match h {
        Harness::Claude => {
            let installed = read_json(&dirs.claude_dir.join("plugins/installed_plugins.json"));
            let markets = read_json(&dirs.claude_dir.join("plugins/known_marketplaces.json"));
            if names(&installed, &old_plugin) {
                out.push(step(false, &["plugin", "uninstall", old_plugin.as_str()]));
            }
            if names(&markets, OLD) {
                out.push(step(false, &["plugin", "marketplace", "remove", OLD]));
            }
            out.push(step(false, &["plugin", "uninstall", "clax@clax"]));
            out.push(step(false, &["plugin", "marketplace", "remove", "clax"]));
        }
        Harness::Codex => {
            let cfg: Option<toml::Table> = std::fs::read_to_string(dirs.codex_home.join("config.toml"))
                .ok()
                .and_then(|t| t.parse().ok());
            let has = |table: &str, key: &str| {
                cfg.as_ref().and_then(|c| c.get(table)).and_then(|t| t.get(key)).is_some()
            };
            if has("plugins", &old_plugin) {
                out.push(step(false, &["plugin", "remove", old_plugin.as_str()]));
            }
            if has("marketplaces", OLD) {
                out.push(step(false, &["plugin", "marketplace", "remove", OLD]));
            }
            out.push(step(false, &["plugin", "remove", "clax@clax"]));
            out.push(step(false, &["plugin", "marketplace", "remove", "clax"]));
        }
        Harness::Pi => {
            let wanted = vec![format!("@empathic/{OLD}-pi"), PI_PACKAGE.to_string()];
            for d in pi_packages(&dirs.pi_dir, &wanted) {
                let d = d.display().to_string();
                out.push(step(false, &["remove", d.as_str()]));
            }
        }
    }
    out
}

/// The commands that register the marketplace at `root` with `h`.
fn additions(h: Harness, root: &Path) -> Vec<Step> {
    let r = root.display().to_string();
    let pi = format!("{r}/plugins/pi");
    match h {
        Harness::Claude => vec![
            step(true, &["plugin", "marketplace", "add", r.as_str()]),
            step(true, &["plugin", "install", "clax@clax"]),
        ],
        Harness::Codex => vec![
            step(true, &["plugin", "marketplace", "add", r.as_str()]),
            step(true, &["plugin", "add", "clax@clax"]),
        ],
        Harness::Pi => vec![step(true, &["install", pi.as_str()])],
    }
}

/// `name` in a directory of `PATH`, if any.
fn on_path(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(name)).find(|p| p.is_file())
}

/// Runs `steps` with `h`'s CLI; the harness's result as JSON.
fn run_steps(h: Harness, steps: Vec<Step>, done: &str) -> Value {
    let mut commands = Vec::new();
    let mut notes = Vec::new();
    let mut failed = false;
    for s in steps {
        let line = format!("{} {}", h.cli(), s.args.join(" "));
        commands.push(line.clone());
        let out = std::process::Command::new(h.cli())
            .args(&s.args)
            .stdin(std::process::Stdio::null())
            .output();
        let err = match out {
            Ok(o) if o.status.success() => continue,
            Ok(o) => String::from_utf8_lossy(&o.stderr).trim().to_string(),
            Err(e) => e.to_string(),
        };
        if s.required {
            failed = true;
            notes.push(format!("`{line}` failed: {err}"));
            break;
        }
    }
    let status = if failed { "failed" } else { done };
    json!({"agent": h.cli(), "status": status, "detail": notes.join("\n"), "commands": commands})
}

fn run(cli: &crate::Cli, home: &Home, a: &Args, install: bool) -> anyhow::Result<()> {
    let dirs = Dirs::from_env(|k| std::env::var(k).ok())
        .ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
    let root = home.root().join("marketplace");
    if install {
        crate::plugins::materialize(&root)?;
    }
    let chosen: Vec<Harness> = if a.agents.is_empty() { Harness::ALL.to_vec() } else { a.agents.clone() };
    let mut results = Vec::new();
    for h in chosen {
        if on_path(h.cli()).is_none() {
            results.push(json!({"agent": h.cli(), "status": "skipped", "detail": format!("{} is not on PATH", h.cli()), "commands": []}));
            continue;
        }
        let mut steps = removals(h, &dirs);
        if install {
            steps.extend(additions(h, &root));
        }
        results.push(run_steps(h, steps, if install { "registered" } else { "removed" }));
    }
    if !install && root.exists() {
        std::fs::remove_dir_all(&root)?;
    }
    let failed = results.iter().any(|r| r["status"] == "failed");
    let out = json!({"marketplace": root, "agents": results});
    super::print(cli, out, |j| {
        let mut lines = vec![format!("marketplace: {}", j["marketplace"].as_str().unwrap_or_default())];
        for r in j["agents"].as_array().into_iter().flatten() {
            let mut l = format!("{}: {}", r["agent"].as_str().unwrap_or_default(), r["status"].as_str().unwrap_or_default());
            if let Some(d) = r["detail"].as_str().filter(|d| !d.is_empty()) {
                l.push_str(&format!(" ({d})"));
            }
            lines.push(l);
        }
        if install {
            lines.push("Start a new session in each harness to load the plugin.".into());
        }
        lines.join("\n")
    });
    if install {
        let first = super::doctor_agent::clax_on_path(&std::env::var_os("PATH").unwrap_or_default())
            .into_iter()
            .find(|(_, v)| v.is_some())
            .map(|(p, _)| p);
        let me = std::env::current_exe().ok().and_then(|p| p.canonicalize().ok());
        if first.as_ref().and_then(|p| p.canonicalize().ok()) != me {
            eprintln!(
                "warning: the plugins run the first clax on PATH, which is {}, not this one ({}); put this one's directory first on PATH",
                first.map(|p| p.display().to_string()).unwrap_or_else(|| "none".into()),
                me.map(|p| p.display().to_string()).unwrap_or_default()
            );
        }
    }
    if failed {
        anyhow::bail!("a harness could not be {}", if install { "registered" } else { "unregistered" });
    }
    Ok(())
}

pub fn init(cli: &crate::Cli, home: &Home, a: &Args) -> anyhow::Result<()> {
    run(cli, home, a, true)
}

pub fn uninit(cli: &crate::Cli, home: &Home, a: &Args) -> anyhow::Result<()> {
    run(cli, home, a, false)
}
```

Add `pub mod init;` to `crates/clax-cli/src/commands/mod.rs`. In `crates/clax-cli/src/main.rs`, add to `Cmd`:

```rust
    /// Register the Clax plugins built into this binary with Claude Code,
    /// Codex and Pi (each one whose CLI is on PATH), replacing stale ones.
    Init(commands::init::Args),
    /// Remove the Clax plugin registrations from Claude Code, Codex and Pi.
    Uninit(commands::init::Args),
```

and to the `match`:

```rust
        Cmd::Init(a) => commands::init::init(&cli, &home, a),
        Cmd::Uninit(a) => commands::init::uninit(&cli, &home, a),
```

`Dirs` must be `pub` with `pub` fields in `doctor_agent.rs`, as it already is. `clax_on_path` is `pub` from Task 5.

- [ ] **Step 5: Run**

Run: `cargo test -p clax-cli --test init`
Expected: PASS (6 tests).

Then check the real CLIs accept the commands, in scratch directories only (skip any that is not installed):

```bash
T="$(mktemp -d)"; mkdir -p "$T/codex" "$T/claude" "$T/pi"
export HOME="$T" CLAX_HOME="$T/ax" CODEX_HOME="$T/codex" CLAUDE_CONFIG_DIR="$T/claude" PI_CODING_AGENT_DIR="$T/pi"
PATH="$PWD/plugins/pi/node_modules/.bin:$PATH" target/debug/clax init; echo "exit=$?"
cat "$T/codex/config.toml" "$T/claude/plugins/known_marketplaces.json" "$T/pi/settings.json"
PATH="$PWD/plugins/pi/node_modules/.bin:$PATH" target/debug/clax uninit; echo "exit=$?"
rm -rf "$T"
```

Expected: `exit=0` twice. Codex's `config.toml` and Claude's `known_marketplaces.json` name `$T/ax/marketplace`, and Pi's `settings.json` lists `…/ax/marketplace/plugins/pi`. Run this in a fresh shell afterwards, so the scratch variables do not stay set.

- [ ] **Step 6: Gates and commit**

Run: `bash scripts/quality_gates.sh; echo "exit=$?"`
Expected: `exit=0`.

```bash
git add crates/clax-cli/Cargo.toml Cargo.lock crates/clax-cli/src/plugins.rs crates/clax-cli/src/commands/init.rs crates/clax-cli/src/commands/mod.rs crates/clax-cli/src/main.rs crates/clax-cli/tests/init.rs
git commit -m "Add clax init and uninit: register the embedded plugins with each harness, removing stale registrations"
git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed
```

---
### Task 7: Version and packaging scripts

**Files:**
- Create: `scripts/check-version.sh`, `scripts/bump-version.sh`, `scripts/package-release.sh`, `scripts/smoke-release-binary.sh`, `scripts/test-release.sh`
- Modify: `scripts/test-plugins.sh`, `scripts/quality_gates.sh`

**Interfaces:**
- Produces: `scripts/check-version.sh [vX.Y.Z | --print]`. It exits 0 when every version agrees (and, given a tag, when the tag is `v<version>`). Otherwise it prints each source and its value and exits 1. `--print` prints the version.
- Produces: `scripts/bump-version.sh X.Y.Z`, which rewrites every source `check-version.sh` reads.
- Produces: `scripts/package-release.sh archive <version> <target> <binary> <outdir>` writes `<outdir>/clax-<version>-<target>.tar.gz`, which holds exactly `clax-<version>-<target>/clax`. It refuses a binary that does not report `clax <version>`. `scripts/package-release.sh sums <dir>` writes `<dir>/SHA256SUMS` for every other regular file in `<dir>`, in `sha256sum` format.
- Produces: `scripts/smoke-release-binary.sh <binary> <version>`, which exits 0 when the binary reports the version and serves the embedded web UI from a scratch home on a kernel-picked port.

- [ ] **Step 1: `scripts/check-version.sh`**

```bash
#!/usr/bin/env bash
# Checks that every place a Clax version is written agrees.
#   check-version.sh           exit 0 when they agree; print each one when not
#   check-version.sh v1.2.3    also require the tag to be v<version>
#   check-version.sh --print   print the version (after checking)
set -uo pipefail
cd "$(dirname "$0")/.."
exec python3 - "$@" <<'PY'
import json, re, sys

def first(pattern, text):
    m = re.search(pattern, text, re.M)
    return m.group(1) if m else None

def load(path):
    with open(path) as f:
        return json.load(f)

cargo = open("Cargo.toml").read()
ws = re.search(r'^\[workspace\.package\]\s*$(.*?)(?=^\[|\Z)', cargo, re.M | re.S)
lock = open("Cargo.lock").read()
market = load(".claude-plugin/marketplace.json")
pi_lock = load("plugins/pi/package-lock.json")
versions = {
    "Cargo.toml [workspace.package]": first(r'^version\s*=\s*"([^"]+)"', ws.group(1)) if ws else None,
    "plugins/claude-code/.claude-plugin/plugin.json": load("plugins/claude-code/.claude-plugin/plugin.json").get("version"),
    "plugins/clax/.codex-plugin/plugin.json": load("plugins/clax/.codex-plugin/plugin.json").get("version"),
    ".claude-plugin/marketplace.json version": market.get("version"),
    ".claude-plugin/marketplace.json plugins[clax]": next((p.get("version") for p in market.get("plugins", []) if p.get("name") == "clax"), None),
    "plugins/pi/package.json": load("plugins/pi/package.json").get("version"),
    "plugins/pi/package-lock.json version": pi_lock.get("version"),
    'plugins/pi/package-lock.json packages[""]': pi_lock.get("packages", {}).get("", {}).get("version"),
    "scripts/ensure-clax.sh CLAX_VERSION": first(r'^CLAX_VERSION="([^"]+)"', open("scripts/ensure-clax.sh").read()),
}
for crate in ("clax-core", "clax-server", "clax-cli", "clax-mcp", "clax-hooks"):
    versions[f"Cargo.lock {crate}"] = first(r'^name = "%s"\nversion = "([^"]+)"' % re.escape(crate), lock)

args = sys.argv[1:]
values = set(versions.values())
if None in values or len(values) != 1:
    print("versions differ:", file=sys.stderr)
    for k, v in versions.items():
        print(f"  {k}: {v}", file=sys.stderr)
    sys.exit(1)
version = values.pop()
if not re.fullmatch(r'\d+\.\d+\.\d+(-[0-9A-Za-z.]+)?', version):
    sys.exit(f"{version!r} is not a release version (X.Y.Z or X.Y.Z-pre)")
if args == ["--print"]:
    print(version)
elif len(args) == 1:
    if args[0] != f"v{version}":
        sys.exit(f"the tag {args[0]} does not match the version {version} (expected v{version})")
elif args:
    sys.exit("usage: check-version.sh [vX.Y.Z | --print]")
PY
```

- [ ] **Step 2: `scripts/bump-version.sh`**

```bash
#!/usr/bin/env bash
# Writes a new Clax version into every place scripts/check-version.sh reads,
# then runs it. Usage: bump-version.sh X.Y.Z
set -euo pipefail
cd "$(dirname "$0")/.."
[ $# = 1 ] || { echo "usage: bump-version.sh X.Y.Z" >&2; exit 2; }
python3 - "$1" <<'PY'
import re, sys, pathlib
new = sys.argv[1]
if not re.fullmatch(r'\d+\.\d+\.\d+(-[0-9A-Za-z.]+)?', new):
    sys.exit(f"{new!r} is not a release version (X.Y.Z or X.Y.Z-pre)")

def sub(path, pattern, count=1):
    p = pathlib.Path(path)
    text = p.read_text()
    out, n = re.subn(pattern, lambda m: m.group(1) + new + m.group(3), text, count=count, flags=re.M)
    if n != count:
        sys.exit(f"{path}: expected {count} version(s) matching {pattern!r}, found {n}")
    p.write_text(out)

sub("Cargo.toml", r'(\[workspace\.package\]\s*\nversion\s*=\s*")([^"]+)(")')
for crate in ("clax-core", "clax-server", "clax-cli", "clax-mcp", "clax-hooks"):
    sub("Cargo.lock", r'(^name = "%s"\nversion = ")([^"]+)(")' % re.escape(crate))
for f in ("plugins/claude-code/.claude-plugin/plugin.json", "plugins/clax/.codex-plugin/plugin.json", "plugins/pi/package.json"):
    sub(f, r'(^  "version": ")([^"]+)(")')
sub(".claude-plugin/marketplace.json", r'("version": ")([^"]+)(")', count=2)
sub("plugins/pi/package-lock.json", r'(^  "version": ")([^"]+)(")')
sub("plugins/pi/package-lock.json", r'("": \{\n      "name": "@empathic/clax-pi",\n      "version": ")([^"]+)(")')
for f in ("scripts/ensure-clax.sh", "plugins/claude-code/scripts/ensure-clax.sh", "plugins/clax/scripts/ensure-clax.sh"):
    sub(f, r'(^CLAX_VERSION=")([^"]+)(")')
PY
scripts/check-version.sh "v$1"
echo "bumped to $1"
```

- [ ] **Step 3: `scripts/package-release.sh`**

```bash
#!/usr/bin/env bash
# Packages release archives and their checksums.
#   package-release.sh archive <version> <target> <binary> <outdir>
#       writes <outdir>/clax-<version>-<target>.tar.gz, holding exactly
#       clax-<version>-<target>/clax; refuses a binary that does not report
#       "clax <version>"
#   package-release.sh sums <dir>
#       writes <dir>/SHA256SUMS for every other regular file in <dir>
set -euo pipefail

sha256() { if command -v sha256sum >/dev/null 2>&1; then sha256sum "$@"; else shasum -a 256 "$@"; fi; }

case "${1:-}" in
    archive)
        [ $# = 5 ] || { echo "usage: package-release.sh archive <version> <target> <binary> <outdir>" >&2; exit 2; }
        version="$2" target="$3" bin="$4" out="$5"
        name="clax-$version-$target"
        got="$("$bin" --version 2>/dev/null | head -1 || true)"
        [ "$got" = "clax $version" ] || { echo "$bin reports '$got', not 'clax $version'" >&2; exit 1; }
        stage="$(mktemp -d)"
        trap 'rm -rf "$stage"' EXIT
        mkdir "$stage/$name"
        cp "$bin" "$stage/$name/clax"
        chmod 755 "$stage/$name/clax"
        mkdir -p "$out"
        # No extended attributes or AppleDouble files travel in the archive,
        # so nothing (a quarantine flag included) is restored on extraction.
        if [ "$(uname -s)" = Darwin ]; then
            COPYFILE_DISABLE=1 tar --no-mac-metadata --no-xattrs -czf "$out/$name.tar.gz" -C "$stage" "$name"
        else
            tar -czf "$out/$name.tar.gz" -C "$stage" "$name"
        fi
        echo "$out/$name.tar.gz"
        ;;
    sums)
        [ $# = 2 ] || { echo "usage: package-release.sh sums <dir>" >&2; exit 2; }
        cd "$2"
        files=()
        for f in *; do
            if [ -f "$f" ] && [ "$f" != SHA256SUMS ]; then files+=("$f"); fi
        done
        [ ${#files[@]} -gt 0 ] || { echo "no files to sum in $2" >&2; exit 1; }
        sha256 "${files[@]}" > SHA256SUMS.tmp
        mv SHA256SUMS.tmp SHA256SUMS
        echo "$2/SHA256SUMS"
        ;;
    *)
        echo "usage: package-release.sh archive <version> <target> <binary> <outdir> | sums <dir>" >&2
        exit 2
        ;;
esac
```

- [ ] **Step 4: `scripts/smoke-release-binary.sh`**

```bash
#!/usr/bin/env bash
# Checks a built binary: it reports "clax <version>" and serves the embedded
# web UI. Uses a scratch home and a port the kernel picks.
# Usage: smoke-release-binary.sh <binary> <version>
set -euo pipefail
[ $# = 2 ] || { echo "usage: smoke-release-binary.sh <binary> <version>" >&2; exit 2; }
bin="$1" version="$2"
got="$("$bin" --version | head -1)"
[ "$got" = "clax $version" ] || { echo "$bin reports '$got', not 'clax $version'" >&2; exit 1; }
scratch="$(mktemp -d)"
export HOME="$scratch" CLAX_HOME="$scratch/home" CLAX_CODEX_BIN=""
unset CLAX_CONFIG_DIR
cleanup() { "$bin" stop >/dev/null 2>&1 || true; rm -rf "$scratch"; }
trap cleanup EXIT
info="$("$bin" serve --json --port 0)"
port="$(printf '%s' "$info" | python3 -c 'import json, sys; print(json.load(sys.stdin)["port"])')"
page="$(curl -fsS "http://127.0.0.1:$port/")"
case "$page" in
    *"/_clax/"*) ;;
    *) echo "$bin does not serve the embedded web UI: GET / names no /_clax/ asset (was web/dist built before cargo build --release?)" >&2; exit 1 ;;
esac
echo "ok: $bin is clax $version and serves the embedded web UI"
```

- [ ] **Step 5: `scripts/test-release.sh`**

```bash
#!/usr/bin/env bash
# Tests check-version.sh, bump-version.sh and package-release.sh in a scratch
# copy of the files they touch. No network.
set -uo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
FAILED=0
pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1"; FAILED=1; }

for f in Cargo.toml Cargo.lock .claude-plugin/marketplace.json plugins/claude-code/.claude-plugin/plugin.json \
    plugins/clax/.codex-plugin/plugin.json plugins/pi/package.json plugins/pi/package-lock.json \
    scripts/ensure-clax.sh plugins/claude-code/scripts/ensure-clax.sh plugins/clax/scripts/ensure-clax.sh \
    scripts/check-version.sh scripts/bump-version.sh scripts/package-release.sh; do
    mkdir -p "$T/$(dirname "$f")"
    cp "$HERE/$f" "$T/$f"
done
V="$("$HERE/scripts/check-version.sh" --print)"

if (cd "$T" && scripts/check-version.sh); then pass "the versions agree"; else fail "the versions agree"; fi
if (cd "$T" && scripts/check-version.sh "v$V"); then pass "the matching tag is accepted"; else fail "the matching tag is accepted"; fi
out="$(cd "$T" && scripts/check-version.sh v9.9.9 2>&1)"; rc=$?
if [ "$rc" = 1 ] && echo "$out" | grep -q "does not match the version $V"; then pass "another tag is refused"; else fail "another tag is refused ($out)"; fi

if (cd "$T" && scripts/bump-version.sh 9.8.7 >/dev/null) && [ "$(cd "$T" && scripts/check-version.sh --print)" = 9.8.7 ] \
    && grep -q '^CLAX_VERSION="9.8.7"$' "$T/plugins/clax/scripts/ensure-clax.sh" \
    && cmp -s "$T/scripts/ensure-clax.sh" "$T/plugins/claude-code/scripts/ensure-clax.sh"; then
    pass "bump-version writes every version, the launcher copies included"
else fail "bump-version writes every version"; fi

sed -i.bak 's/"version": "9.8.7"/"version": "9.8.6"/' "$T/plugins/clax/.codex-plugin/plugin.json"
out="$(cd "$T" && scripts/check-version.sh 2>&1)"; rc=$?
if [ "$rc" = 1 ] && echo "$out" | grep -q "plugins/clax/.codex-plugin/plugin.json: 9.8.6"; then pass "a stray version is named"
else fail "a stray version is named ($out)"; fi

if out="$(cd "$T" && scripts/bump-version.sh not-a-version 2>&1)"; then fail "a bad version is refused"
else echo "$out" | grep -q "is not a release version" && pass "a bad version is refused" || fail "a bad version is refused ($out)"; fi

mkdir -p "$T/bin"
printf '#!/bin/sh\necho "clax 1.2.3"\n' > "$T/bin/clax"
chmod +x "$T/bin/clax"
"$T/scripts/package-release.sh" archive 1.2.3 x86_64-unknown-linux-musl "$T/bin/clax" "$T/dist" >/dev/null
list="$(tar -tzf "$T/dist/clax-1.2.3-x86_64-unknown-linux-musl.tar.gz" | sed 's#/$##' | sort)"
if [ "$list" = "$(printf 'clax-1.2.3-x86_64-unknown-linux-musl\nclax-1.2.3-x86_64-unknown-linux-musl/clax')" ]; then
    pass "an archive holds exactly clax-<version>-<target>/clax"
else fail "an archive holds exactly clax-<version>-<target>/clax ($list)"; fi
if out="$("$T/scripts/package-release.sh" archive 1.2.4 x86_64-unknown-linux-musl "$T/bin/clax" "$T/dist" 2>&1)"; then
    fail "a binary of another version is refused"
else echo "$out" | grep -q "not 'clax 1.2.4'" && pass "a binary of another version is refused" || fail "a binary of another version is refused ($out)"; fi

echo "#!/bin/sh" > "$T/dist/install.sh"
"$T/scripts/package-release.sh" sums "$T/dist" >/dev/null
if (cd "$T/dist" && { sha256sum -c SHA256SUMS 2>/dev/null || shasum -a 256 -c SHA256SUMS; } >/dev/null) \
    && [ "$(wc -l < "$T/dist/SHA256SUMS" | tr -d ' ')" = 2 ] && ! grep -q SHA256SUMS "$T/dist/SHA256SUMS"; then
    pass "SHA256SUMS covers every other file and verifies"
else fail "SHA256SUMS covers every other file and verifies"; fi

[ "$FAILED" = 0 ] && echo "release script tests passed" || echo "release script tests FAILED"
exit "$FAILED"
```

- [ ] **Step 6: One version check, in one place**

In `scripts/test-plugins.sh`, replace the "One version everywhere" block (the `python3 - Cargo.toml … scripts/ensure-clax.sh` heredoc and its pass/fail, which Task 4 pointed at `CLAX_VERSION`) with:

```bash
# One version everywhere: see scripts/check-version.sh for the list.
if out="$(scripts/check-version.sh 2>&1)"; then pass "every written version agrees (scripts/check-version.sh)"
else fail "$out"; fi
```

In the Codex manifest check in the same file, replace `m.get("version") == "0.2.0"` with `m.get("version") == sys.argv[2]`, and pass `"$(scripts/check-version.sh --print)"` as the second argument to that `python3 -` call.

In `scripts/quality_gates.sh`, after the `installer` line, add:

```bash
run "release scripts"       scripts/test-release.sh
```

Make the new scripts executable: `chmod +x scripts/check-version.sh scripts/bump-version.sh scripts/package-release.sh scripts/smoke-release-binary.sh scripts/test-release.sh`.

- [ ] **Step 7: Run**

Run: `scripts/test-release.sh && scripts/test-plugins.sh | tail -1 && just web >/dev/null && cargo build --release -q -p clax-cli && scripts/smoke-release-binary.sh target/release/clax "$(scripts/check-version.sh --print)"`
Expected: `release script tests passed`, `plugin checks passed`, and `ok: target/release/clax is clax <version> and serves the embedded web UI`. The smoke runs the build in place because it is a test, not an agent; agents never run a binary from `target/`.

- [ ] **Step 8: Gates and commit**

Run: `bash scripts/quality_gates.sh; echo "exit=$?"`
Expected: `exit=0`.

```bash
git add scripts/check-version.sh scripts/bump-version.sh scripts/package-release.sh scripts/smoke-release-binary.sh scripts/test-release.sh scripts/test-plugins.sh scripts/quality_gates.sh
git commit -m "Add the version check, version bump, release packaging and release-binary smoke scripts"
git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed
```

---

### Task 8: `install.sh`, for people without a checkout

**Files:**
- Create: `install.sh`, `scripts/fake-release-server.py`, `scripts/test-install.sh`
- Modify: `scripts/quality_gates.sh`

**Interfaces:**
- Consumes: `scripts/package-release.sh` (Task 7) to build the fixture archives, so the installer and the packager agree on names and layout.
- Produces: `install.sh [version]`. With no version, it follows `https://github.com/empathic/clax/releases/latest` to the latest tag. It installs `clax` into `$CLAX_INSTALL_DIR`, else `~/.local/bin`, after checking the archive against `SHA256SUMS` and the binary's version, and moves it into place in one rename. It never runs `clax init`; it tells the person to. The environment variables `CLAX_RELEASE_BASE_URL`, `CLAX_RELEASE_LATEST_URL` and `CLAX_DOWNLOAD_TIMEOUT` exist for tests.
- Produces: `scripts/fake-release-server.py <root> <request log> <port file>`. It serves `<root>/good/<path>` at `/<mode>/<path>` for the modes `ok`, `none` (404), `badsum` (zeroed `SHA256SUMS`), `partial` (archives cut in half after a full `Content-Length`) and `slow` (a 60 s stall), and `<root>/wrong/<path>` at `/wrong/<path>`. `/<mode>/latest` redirects to `/<mode>/tag/v$FAKE_LATEST`. It logs every request path and binds `127.0.0.1:0`.

- [ ] **Step 1: The fake release server**

Create `scripts/fake-release-server.py`:

```python
#!/usr/bin/env python3
"""A stand-in for GitHub release downloads, for scripts/test-install.sh.

Usage: fake-release-server.py <root> <request log> <port file>

Serves <root>/good/<path> at /<mode>/<path> and <root>/wrong/<path> at
/wrong/<path>, and appends every request path to <request log>. Modes:
  ok       the file as it is; /ok/latest redirects to /ok/tag/v$FAKE_LATEST
  none     404 for everything
  badsum   SHA256SUMS with every checksum zeroed
  partial  archives: the full Content-Length, half the body, then a close
  slow     waits 60 s before answering
  wrong    the files under <root>/wrong
Binds 127.0.0.1 on a port the kernel picks and writes it to <port file>.
"""
import http.server
import os
import re
import sys
import threading
import time

ROOT, LOG, PORT_FILE = sys.argv[1:4]
LOG_LOCK = threading.Lock()


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_GET(self):
        with LOG_LOCK, open(LOG, "a") as f:
            f.write(self.path + "\n")
        mode, _, rest = self.path.lstrip("/").partition("/")
        if rest == "latest":
            self.send_response(302)
            self.send_header("Location", f"/{mode}/tag/v{os.environ.get('FAKE_LATEST', '0.0.0')}")
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        if rest.startswith("tag/"):
            self.send_response(200)
            self.send_header("Content-Length", "2")
            self.end_headers()
            self.wfile.write(b"ok")
            return
        tree = "wrong" if mode == "wrong" else "good"
        path = os.path.join(ROOT, tree, rest)
        if mode == "slow":
            time.sleep(60)
        if mode == "none" or ".." in rest or not os.path.isfile(path):
            self.send_error(404)
            return
        with open(path, "rb") as f:
            data = f.read()
        if mode == "badsum" and rest.endswith("SHA256SUMS"):
            data = re.sub(rb"^[0-9a-f]{64}", b"0" * 64, data, flags=re.M)
        self.send_response(200)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        if mode == "partial" and rest.endswith(".tar.gz"):
            self.wfile.write(data[: len(data) // 2])
            self.wfile.flush()
            self.close_connection = True
            return
        self.wfile.write(data)


server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
server.daemon_threads = True
with open(PORT_FILE + ".tmp", "w") as f:
    f.write(str(server.server_address[1]))
os.replace(PORT_FILE + ".tmp", PORT_FILE)
server.serve_forever()
```

- [ ] **Step 2: Write the failing tests**

Create `scripts/test-install.sh`:

```bash
#!/usr/bin/env bash
# Tests install.sh against scripts/fake-release-server.py on 127.0.0.1 (a
# port the kernel picks), with a scratch HOME. No network.
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
INSTALL="$(cd "$HERE/.." && pwd)/install.sh"
ROOT="$(cd "$(mktemp -d)" && pwd -P)"
SERVER_PID=""
cleanup() {
    if [ -n "$SERVER_PID" ]; then kill "$SERVER_PID" 2>/dev/null; wait "$SERVER_PID" 2>/dev/null; fi
    rm -rf "$ROOT"
    return 0
}
trap cleanup EXIT
# The interpreter itself, not a version-manager shim that needs the real PATH.
PY="$(python3 -c 'import sys; print(sys.executable)')"
ORIG_PATH="$PATH"
FAILED=0
pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1"; FAILED=1; }
V=0.3.0
TARGETS="aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-musl aarch64-unknown-linux-musl"

# Release trees: good (clax $V) and wrong (an archive holding clax 0.0.9).
mkdir -p "$ROOT/payload" "$ROOT/release/good/v$V" "$ROOT/release/wrong/v$V" "$ROOT/stage"
printf '#!/bin/sh\necho "clax %s"\n' "$V" > "$ROOT/payload/clax"
chmod +x "$ROOT/payload/clax"
for t in $TARGETS; do
    "$HERE/package-release.sh" archive "$V" "$t" "$ROOT/payload/clax" "$ROOT/release/good/v$V" >/dev/null
    mkdir -p "$ROOT/stage/clax-$V-$t"
    printf '#!/bin/sh\necho "clax 0.0.9"\n' > "$ROOT/stage/clax-$V-$t/clax"
    chmod +x "$ROOT/stage/clax-$V-$t/clax"
    (cd "$ROOT/stage" && tar -czf "$ROOT/release/wrong/v$V/clax-$V-$t.tar.gz" "clax-$V-$t")
done
"$HERE/package-release.sh" sums "$ROOT/release/good/v$V" >/dev/null
"$HERE/package-release.sh" sums "$ROOT/release/wrong/v$V" >/dev/null
REQLOG="$ROOT/requests.log"
: > "$REQLOG"
(FAKE_LATEST="$V" exec "$PY" "$HERE/fake-release-server.py" "$ROOT/release" "$REQLOG" "$ROOT/port") &
SERVER_PID=$!
for _ in $(seq 50); do [ -s "$ROOT/port" ] && break; sleep 0.1; done
BASE="http://127.0.0.1:$(cat "$ROOT/port")"

new_env() {
    SANDBOX="$(mktemp -d "$ROOT/case.XXXXXX")"
    export HOME="$SANDBOX/home"
    mkdir -p "$HOME" "$SANDBOX/bin"
    export PATH="$SANDBOX/bin:$ORIG_PATH"
    unset CLAX_INSTALL_DIR CLAX_DOWNLOAD_TIMEOUT
    : > "$REQLOG"
}
fake_uname() {
    printf '#!/bin/sh\ncase "$1" in -s) echo %s ;; -m) echo %s ;; esac\n' "$1" "$2" > "$SANDBOX/bin/uname"
    chmod +x "$SANDBOX/bin/uname"
}
inst() { # mode [args...]
    local mode="$1"; shift
    OUT="$(CLAX_RELEASE_BASE_URL="$BASE/$mode" CLAX_RELEASE_LATEST_URL="$BASE/$mode/latest" bash "$INSTALL" "$@" 2>&1)"; RC=$?
}

new_env
fake_uname Linux x86_64
inst ok
if [ "$RC" = 0 ] && [ "$("$HOME/.local/bin/clax" --version)" = "clax $V" ] && grep -qx "/ok/latest" "$REQLOG" \
    && grep -qx "/ok/v$V/clax-$V-x86_64-unknown-linux-musl.tar.gz" "$REQLOG" && echo "$OUT" | grep -q "clax init"; then
    pass "the latest release is found, checked and installed into ~/.local/bin"
else fail "the latest release is installed (rc=$RC out=$OUT)"; fi

for pair in Darwin/arm64/aarch64-apple-darwin Darwin/x86_64/x86_64-apple-darwin Linux/aarch64/aarch64-unknown-linux-musl Linux/arm64/aarch64-unknown-linux-musl; do
    new_env
    fake_uname "${pair%%/*}" "$(echo "$pair" | cut -d/ -f2)"
    CLAX_INSTALL_DIR="$SANDBOX/dest" inst ok "v$V"
    if [ "$RC" = 0 ] && [ -x "$SANDBOX/dest/clax" ] && grep -qx "/ok/v$V/clax-$V-${pair##*/}.tar.gz" "$REQLOG" && ! grep -q latest "$REQLOG"; then
        pass "a named version on ${pair%/*} fetches ${pair##*/}"
    else fail "a named version on ${pair%/*} (rc=$RC out=$OUT)"; fi
done

new_env
fake_uname Linux x86_64
inst none "$V"
if [ "$RC" = 1 ] && echo "$OUT" | grep -q "answered HTTP 404 (the release may not exist, or the repository may not be public yet)" && [ ! -e "$HOME/.local/bin/clax" ]; then
    pass "a missing release (or a private repository) is named"
else fail "a missing release (rc=$RC out=$OUT)"; fi

new_env
fake_uname Linux x86_64
inst badsum "$V"
if [ "$RC" = 1 ] && echo "$OUT" | grep -q "checksum mismatch" && [ ! -e "$HOME/.local/bin/clax" ]; then
    pass "a checksum mismatch installs nothing"
else fail "a checksum mismatch (rc=$RC out=$OUT)"; fi

new_env
fake_uname Linux x86_64
inst partial "$V"
if [ "$RC" = 1 ] && echo "$OUT" | grep -q "was cut short" && [ ! -e "$HOME/.local/bin/clax" ]; then
    pass "a partial download installs nothing"
else fail "a partial download (rc=$RC out=$OUT)"; fi

new_env
fake_uname Linux x86_64
start=$(date +%s)
CLAX_DOWNLOAD_TIMEOUT=2 inst slow "$V"
took=$(( $(date +%s) - start ))
if [ "$RC" = 1 ] && echo "$OUT" | grep -q "timed out after 2s" && [ "$took" -lt 10 ] && [ ! -e "$HOME/.local/bin/clax" ]; then
    pass "a stalled download times out within its bound"
else fail "a stalled download times out (took=${took}s rc=$RC out=$OUT)"; fi

new_env
fake_uname Linux x86_64
inst wrong "$V"
if [ "$RC" = 1 ] && echo "$OUT" | grep -q "does not hold clax $V" && [ ! -e "$HOME/.local/bin/clax" ]; then
    pass "an archive holding another version is refused"
else fail "an archive holding another version (rc=$RC out=$OUT)"; fi

new_env
fake_uname FreeBSD x86_64
inst ok "$V"
if [ "$RC" = 1 ] && echo "$OUT" | grep -q "no prebuilt clax for FreeBSD/x86_64" && [ ! -s "$REQLOG" ]; then
    pass "an unsupported platform says so without downloading"
else fail "an unsupported platform (rc=$RC out=$OUT)"; fi

new_env
fake_uname Linux x86_64
mkdir -p "$HOME/.local/bin"
printf '#!/bin/sh\necho "clax 0.0.1"\n' > "$HOME/.local/bin/clax"
chmod +x "$HOME/.local/bin/clax"
exec 3< "$HOME/.local/bin/clax"
inst ok "$V"
if [ "$RC" = 0 ] && [ "$("$HOME/.local/bin/clax" --version)" = "clax $V" ] && grep -q "0.0.1" <&3 \
    && ! ls -a "$HOME/.local/bin" | grep -q '^\.clax\.'; then
    pass "an existing clax is replaced by one rename; an open copy keeps the old file"
else fail "an existing clax is replaced by rename (rc=$RC out=$OUT)"; fi
exec 3<&-

[ "$FAILED" = 0 ] && echo "installer tests passed" || echo "installer tests FAILED"
exit "$FAILED"
```

Run: `bash scripts/test-install.sh`
Expected: FAIL (`install.sh` does not exist).

- [ ] **Step 3: `install.sh`**

Create `install.sh` at the repository root:

```bash
#!/usr/bin/env bash
# Installs a released clax into ~/.local/bin, for people without a checkout
# (with a checkout, run `just install` instead).
#
# Usage: install.sh [version]      (default: the latest release)
#   curl -fsSL https://github.com/empathic/clax/releases/latest/download/install.sh | bash
#
# It downloads clax-<version>-<target>.tar.gz and SHA256SUMS from the release,
# checks the archive's checksum (SHA256SUMS comes from the same place, so this
# protects integrity, not authenticity) and the binary's version, and moves it
# into place in one rename. The download needs no credentials only while the
# GitHub repository is public.
#
# Environment:
#   CLAX_INSTALL_DIR          where to install (default ~/.local/bin)
#   CLAX_RELEASE_BASE_URL     release download base (files come from
#                             <base>/v<version>/); for tests
#   CLAX_RELEASE_LATEST_URL   the URL that redirects to the latest release's
#                             tag; for tests
#   CLAX_DOWNLOAD_TIMEOUT     seconds each download may take (default 300)
set -euo pipefail

REPO="empathic/clax"
BASE="${CLAX_RELEASE_BASE_URL:-https://github.com/${REPO}/releases/download}"
LATEST_URL="${CLAX_RELEASE_LATEST_URL:-https://github.com/${REPO}/releases/latest}"
INSTALL_DIR="${CLAX_INSTALL_DIR:-$HOME/.local/bin}"
TIMEOUT="${CLAX_DOWNLOAD_TIMEOUT:-300}"
TMP=""
trap 'if [ -n "$TMP" ]; then rm -rf "$TMP"; fi' EXIT

die() { echo "clax install: $*" >&2; exit 1; }

target() {
    case "$(uname -s)/$(uname -m)" in
        Darwin/arm64 | Darwin/aarch64) echo aarch64-apple-darwin ;;
        Darwin/x86_64) echo x86_64-apple-darwin ;;
        Linux/x86_64 | Linux/amd64) echo x86_64-unknown-linux-musl ;;
        Linux/aarch64 | Linux/arm64) echo aarch64-unknown-linux-musl ;;
        *) die "there is no prebuilt clax for $(uname -s)/$(uname -m); build it from a checkout with \`just install\`" ;;
    esac
}

sha256() {
    if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{ print $1 }'
    else shasum -a 256 "$1" | awk '{ print $1 }'; fi
}

# Downloads $1 to $2, naming the failure.
fetch() {
    local code rc=0
    code="$(curl -sSL --connect-timeout 10 --max-time "$TIMEOUT" -o "$2" -w '%{http_code}' "$1" 2>/dev/null)" || rc=$?
    case "$rc" in
        0) ;;
        28) die "downloading $1 timed out after ${TIMEOUT}s" ;;
        18) die "the download of $1 was cut short" ;;
        6 | 7) die "cannot reach $1" ;;
        *) die "downloading $1 failed (curl exit $rc)" ;;
    esac
    [ "$code" = 200 ] || die "$1 answered HTTP $code (the release may not exist, or the repository may not be public yet)"
}

main() {
    local version="${1:-}" t name expected actual url
    for c in curl tar awk; do command -v "$c" >/dev/null 2>&1 || die "$c is required"; done
    command -v sha256sum >/dev/null 2>&1 || command -v shasum >/dev/null 2>&1 || die "sha256sum or shasum is required"
    t="$(target)"
    if [ -z "$version" ]; then
        url="$(curl -sSL -o /dev/null -w '%{url_effective}' --connect-timeout 10 --max-time 30 "$LATEST_URL" 2>/dev/null)" \
            || die "cannot reach $LATEST_URL"
        version="${url##*/}"
        case "$version" in v[0-9]*) ;; *) die "could not find the latest release at $LATEST_URL (the repository may not be public yet)" ;; esac
    fi
    version="${version#v}"
    name="clax-$version-$t"
    TMP="$(mktemp -d)"
    echo "clax install: downloading clax $version ($t)"
    fetch "$BASE/v$version/SHA256SUMS" "$TMP/SHA256SUMS"
    fetch "$BASE/v$version/$name.tar.gz" "$TMP/$name.tar.gz"
    expected="$(awk -v f="$name.tar.gz" '{ n = $2; sub(/^\*/, "", n) } n == f { print $1; exit }' "$TMP/SHA256SUMS")"
    [ -n "$expected" ] || die "SHA256SUMS of v$version does not list $name.tar.gz"
    actual="$(sha256 "$TMP/$name.tar.gz")"
    [ "$actual" = "$expected" ] || die "checksum mismatch for $name.tar.gz (SHA256SUMS says $expected, the download is $actual); nothing was installed"
    mkdir "$TMP/x"
    tar -xzf "$TMP/$name.tar.gz" -C "$TMP/x" 2>/dev/null || die "$name.tar.gz could not be unpacked"
    [ "$("$TMP/x/$name/clax" --version 2>/dev/null | head -1)" = "clax $version" ] \
        || die "$name.tar.gz does not hold clax $version"
    mkdir -p "$INSTALL_DIR"
    cp "$TMP/x/$name/clax" "$INSTALL_DIR/.clax.$$"
    chmod 755 "$INSTALL_DIR/.clax.$$"
    mv -f "$INSTALL_DIR/.clax.$$" "$INSTALL_DIR/clax"
    echo "clax install: installed clax $version at $INSTALL_DIR/clax"
    case ":$PATH:" in
        *":$INSTALL_DIR:"*) ;;
        *) echo "clax install: $INSTALL_DIR is not on PATH; add it: export PATH=\"$INSTALL_DIR:\$PATH\"" ;;
    esac
    echo "clax install: now run \`clax init\` to register the plugins with Claude Code, Codex and Pi"
}

main "$@"
```

`chmod +x install.sh scripts/fake-release-server.py scripts/test-install.sh`. In `scripts/quality_gates.sh`, after the `release scripts` line, add:

```bash
run "release installer"     scripts/test-install.sh
```

- [ ] **Step 4: Run**

Run: `bash scripts/test-install.sh`
Expected: every line `PASS`, then `installer tests passed`.

- [ ] **Step 5: Gates and commit**

Run: `bash scripts/quality_gates.sh; echo "exit=$?"`
Expected: `exit=0`.

```bash
git add install.sh scripts/fake-release-server.py scripts/test-install.sh scripts/quality_gates.sh
git commit -m "Add install.sh: a checksum-checked release install into ~/.local/bin for people without a checkout"
git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed
```

---

### Task 9: The release workflow, with a dry run

**Files:**
- Modify: `.github/workflows/release.yml`

**Interfaces:**
- Consumes: the Task 7 scripts and `install.sh` (Task 8).
- Produces: on a `v*` tag, a GitHub release `v<version>` holding `clax-<version>-{aarch64-apple-darwin,x86_64-apple-darwin,x86_64-unknown-linux-musl,aarch64-unknown-linux-musl}.tar.gz`, `install.sh` and `SHA256SUMS`. On `workflow_dispatch`, and on pull requests that touch the release path, it runs everything except the publish job and keeps the result as the workflow artifact `release-dist`.

- [ ] **Step 1: Replace the workflow**

```yaml
name: Release

# A v* tag builds, checks and publishes a release. A manual run, or a pull
# request that touches the release path, runs the same jobs without
# publishing; its result is the workflow artifact `release-dist`.
on:
  push:
    tags: ["v*"]
  workflow_dispatch: {}
  pull_request:
    paths:
      - ".github/workflows/release.yml"
      - "scripts/package-release.sh"
      - "scripts/check-version.sh"
      - "scripts/smoke-release-binary.sh"
      - "install.sh"
      - "rust-toolchain.toml"

permissions:
  contents: read

concurrency:
  group: release-${{ github.ref }}
  cancel-in-progress: false

env:
  CARGO_TERM_COLOR: always

jobs:
  version:
    name: Check versions
    runs-on: ubuntu-24.04
    outputs:
      version: ${{ steps.v.outputs.version }}
      publish: ${{ steps.v.outputs.publish }}
    steps:
      - uses: actions/checkout@v4
      - id: v
        shell: bash
        run: |
          if [ "$GITHUB_EVENT_NAME" = push ]; then
            scripts/check-version.sh "$GITHUB_REF_NAME"
            echo "publish=true" >> "$GITHUB_OUTPUT"
          else
            scripts/check-version.sh
            echo "publish=false" >> "$GITHUB_OUTPUT"
          fi
          echo "version=$(scripts/check-version.sh --print)" >> "$GITHUB_OUTPUT"

  build:
    name: Build ${{ matrix.target }}
    needs: version
    runs-on: ${{ matrix.os }}
    strategy:
      fail-fast: false
      matrix:
        include:
          # Every target builds natively; no cross toolchain.
          - { target: aarch64-apple-darwin, os: macos-15 }
          - { target: x86_64-apple-darwin, os: macos-15-intel }
          - { target: x86_64-unknown-linux-musl, os: ubuntu-24.04 }
          - { target: aarch64-unknown-linux-musl, os: ubuntu-24.04-arm }
    env:
      VERSION: ${{ needs.version.outputs.version }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with: { toolchain: 1.94.0, targets: "${{ matrix.target }}" }
      - uses: actions/setup-node@v4
        with: { node-version: 22, cache: npm, cache-dependency-path: web/package-lock.json }
      - name: Install musl-tools
        if: endsWith(matrix.target, '-linux-musl')
        run: sudo apt-get update && sudo apt-get install -y musl-tools
      - uses: extractions/setup-just@v2
      # A release build embeds web/dist, so the web UI is built first.
      - name: Build web bundle
        run: just web
      - name: Build
        # cc looks for aarch64-linux-musl-gcc, which musl-tools does not
        # install; on the arm64 runner the native musl-gcc targets it.
        env:
          CC_aarch64_unknown_linux_musl: musl-gcc
        run: cargo build --release --locked --target ${{ matrix.target }} -p clax-cli --bin clax
      - name: Smoke-test the binary
        shell: bash
        run: scripts/smoke-release-binary.sh "target/${{ matrix.target }}/release/clax" "$VERSION"
      - name: Package
        shell: bash
        run: scripts/package-release.sh archive "$VERSION" ${{ matrix.target }} "target/${{ matrix.target }}/release/clax" dist
      - uses: actions/upload-artifact@v4
        with:
          name: clax-${{ matrix.target }}
          path: dist/clax-${{ env.VERSION }}-${{ matrix.target }}.tar.gz
          if-no-files-found: error

  assemble:
    name: Assemble and install through install.sh
    needs: [version, build]
    runs-on: ubuntu-24.04
    env:
      VERSION: ${{ needs.version.outputs.version }}
    steps:
      - uses: actions/checkout@v4
      - uses: actions/download-artifact@v4
        with: { path: dist, pattern: "clax-*", merge-multiple: true }
      - name: Add install.sh and checksums
        shell: bash
        run: |
          cp install.sh dist/install.sh
          scripts/package-release.sh sums dist
          cat dist/SHA256SUMS
          test "$(ls dist | wc -l)" = 6
      - name: Install through install.sh from a local copy of the release
        shell: bash
        run: |
          serve="$RUNNER_TEMP/serve"
          mkdir -p "$serve/v$VERSION"
          cp dist/* "$serve/v$VERSION/"
          (cd "$serve" && exec python3 -u -m http.server 0 --bind 127.0.0.1 > "$RUNNER_TEMP/http.log" 2>&1) &
          for _ in $(seq 50); do
            port="$(sed -n 's#.*http://127.0.0.1:\([0-9]*\)/.*#\1#p' "$RUNNER_TEMP/http.log" | head -1)"
            [ -n "$port" ] && break
            sleep 0.2
          done
          export HOME="$RUNNER_TEMP/home" CLAX_RELEASE_BASE_URL="http://127.0.0.1:$port"
          bash dist/install.sh "$VERSION"
          test "$("$HOME/.local/bin/clax" --version)" = "clax $VERSION"
      - uses: actions/upload-artifact@v4
        with: { name: release-dist, path: dist/, if-no-files-found: error }

  publish:
    name: Publish ${{ github.ref_name }}
    needs: [version, assemble]
    if: needs.version.outputs.publish == 'true'
    runs-on: ubuntu-24.04
    permissions:
      contents: write
    steps:
      - uses: actions/download-artifact@v4
        with: { name: release-dist, path: dist }
      - name: Create the release
        env:
          GH_TOKEN: ${{ github.token }}
        run: |
          gh release create "$GITHUB_REF_NAME" dist/* \
            --repo "$GITHUB_REPOSITORY" \
            --title "Clax ${{ needs.version.outputs.version }}" \
            --generate-notes --verify-tag
```

`python3 -u -m http.server 0` prints the port it bound (`Serving HTTP on 127.0.0.1 port N (http://127.0.0.1:N/)`) without buffering, and the step reads it from that line.

- [ ] **Step 2: Lint the workflow locally**

Run: `python3 -c 'import yaml,sys; d=yaml.safe_load(open(".github/workflows/release.yml")); j=d["jobs"]; assert set(j)=={"version","build","assemble","publish"}; assert j["publish"]["if"]=="needs.version.outputs.publish == '"'"'true'"'"'"; assert len(j["build"]["strategy"]["matrix"]["include"])==4; print("ok")'`
Expected: `ok`. If PyYAML is missing, run `pip3 install --user pyyaml` first. If `actionlint` is installed, `actionlint .github/workflows/release.yml` must also print nothing.

Push nothing and dispatch nothing. When the person pushes this work, a pull request that touches `release.yml` runs the dry-run jobs. While the repository is private, a runner label the account lacks (most likely `ubuntu-24.04-arm` or `macos-15-intel`) fails that one job. "Steps for the person" gives the one-job fallback. The person runs the manual dispatch there too.

- [ ] **Step 3: Gates and commit**

Run: `bash scripts/quality_gates.sh; echo "exit=$?"`
Expected: `exit=0`.

```bash
git add .github/workflows/release.yml
git commit -m "Build, smoke-test and package four native release binaries; publish only on a tag"
git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed
```

---
### Task 10: `just install`, `just dev [harness]`, `just watch`

**Files:**
- Create: `scripts/dev-home.sh`, `scripts/dev.sh` (new content), `scripts/test-dev.sh`
- Rename: `scripts/dev.sh` → `scripts/watch.sh` (then edit)
- Modify: `justfile`, `scripts/test-justfile.sh`, `scripts/quality_gates.sh`, `web/vite.shell.config.ts`

**Interfaces:**
- Consumes: `clax init`/`uninit` (Task 6), `[serve] port` (Task 2), `DaemonInfo.exe` (Task 3).
- Produces:
  - `just install`: `just web`, `cargo install --locked --path crates/clax-cli`, then `~/.cargo/bin/clax init`, warning when the first `clax` on `PATH` is another one. `just uninstall`: `clax uninit`, then `cargo uninstall clax-cli`.
  - `just dev [claude|codex|pi] [harness arguments]` (default `claude`), as the Design decisions describe.
  - `just watch [--shared] [serve arguments]`: today's `just dev`. It serves `~/.clax-dev` on 7481 by default, and `~/.clax` on 7480 with `--shared`.
  - `just serve`, `just stop` and `just doctor` act on the dev home.

- [ ] **Step 1: `scripts/dev-home.sh`**

```bash
# Sourced by scripts/dev.sh, scripts/watch.sh and scripts/test-dev.sh.

# watch_settings [--shared] [args...]: sets DEV_HOME, DEV_PORT and DEV_ARGS
# (the remaining arguments, for `clax serve`). Without --shared: $CLAX_HOME,
# else ~/.clax-dev, on $CLAX_DEV_PORT, else 7481. With --shared: $CLAX_HOME,
# else ~/.clax (the home agents use), on 7480.
watch_settings() {
    local shared="" a
    DEV_ARGS=()
    for a in "$@"; do
        if [ "$a" = --shared ]; then shared=1; else DEV_ARGS+=("$a"); fi
    done
    if [ -n "$shared" ]; then
        DEV_HOME="${CLAX_HOME:-$HOME/.clax}"
        DEV_PORT=7480
    else
        DEV_HOME="${CLAX_HOME:-$HOME/.clax-dev}"
        DEV_PORT="${CLAX_DEV_PORT:-7481}"
    fi
}

# ensure_dev_home <home> <port>: creates the home (0700) and, when its
# config.toml has no [serve] table, records <port> there, so every daemon
# started for this home listens on it.
ensure_dev_home() {
    mkdir -p "$1"
    chmod 700 "$1"
    if ! grep -q '^\[serve\]' "$1/config.toml" 2>/dev/null; then
        printf '\n[serve]\nport = %s\n' "$2" >> "$1/config.toml"
    fi
}

# stop_orphan_daemon <home>: stops the home's daemon when the executable it
# recorded no longer exists (an earlier `just dev`, whose temporary directory
# is gone). A daemon whose executable exists, such as `just watch`'s, is left
# alone.
stop_orphan_daemon() {
    local exe pid
    [ -f "$1/daemon.json" ] || return 0
    exe="$(sed -n 's/.*"exe"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$1/daemon.json" | head -1 || true)"
    pid="$(sed -n 's/.*"pid"[[:space:]]*:[[:space:]]*\([0-9][0-9]*\).*/\1/p' "$1/daemon.json" | head -1 || true)"
    if [ -n "$exe" ] && [ ! -e "$exe" ] && [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
        echo "clax dev: stopping the daemon of $1 (pid $pid): its binary $exe is gone"
        kill "$pid"
    fi
}
```

- [ ] **Step 2: `just watch` is today's `just dev`**

Run `git mv scripts/dev.sh scripts/watch.sh`. In `scripts/watch.sh`, keep the shebang line. Replace everything after it, through the `fi` that closes the `CLAX_HOME` message (the header comment, `set -euo pipefail`, the `cd`, `PORT=7480`, `ARGS="$*"` and the message), with:

```bash
# `just watch`: runs the daemon and the web bundlers with auto-reload. By
# default it serves its own home ($CLAX_HOME, else ~/.clax-dev) on port 7481,
# so rebuilding never takes the agents' daemon down; `--shared` serves the
# agents' home (~/.clax) on 7480. Other arguments go to `clax serve`.
set -euo pipefail
cd "$(dirname "$0")/.."
. scripts/dev-home.sh
watch_settings "$@"
export CLAX_HOME="$DEV_HOME"
PORT="$DEV_PORT"
ARGS="${DEV_ARGS[*]-}"
if [ "$PORT" = 7480 ]; then
    echo "Clax watch: --shared: serving CLAX_HOME=$CLAX_HOME on port $PORT, the home your agents use. While a Rust change rebuilds, an agent may start its own daemon here."
else
    ensure_dev_home "$CLAX_HOME" "$PORT"
    echo "Clax watch: serving CLAX_HOME=$CLAX_HOME on port $PORT (agents keep their own daemon; \`just watch --shared\` serves theirs)"
fi
```

In the rest of the file, replace `Clax dev: http://` with `Clax watch: http://`. `CLAX_HOME` is now always exported, so the `cleanup` trap's `target/debug/clax stop` stops this server's daemon and never the agents' daemon.

- [ ] **Step 3: `scripts/dev.sh`, the clash-style harness launcher**

Create `scripts/dev.sh`:

```bash
#!/usr/bin/env bash
# `just dev [claude|codex|pi] [harness arguments...]`: builds clax, puts the
# build first on PATH from a temporary directory (removed on exit), and starts
# the harness with the Clax plugin loaded from this checkout, on the dev home
# ($CLAX_HOME, else ~/.clax-dev, whose daemon listens on 7481). The agents'
# own home, daemon and installed binary are untouched.
#   claude  claude --plugin-dir plugins/claude-code, with an installed
#           clax@clax disabled for the session (--settings)
#   pi      pi -ne -e plugins/pi/src/clax.ts --skill plugins/pi/skills/clax
#           (-ne: no extension is discovered, so an installed Clax package
#           does not load twice; other installed extensions are off too)
#   codex   Codex cannot load a plugin from a directory. The session runs on
#           a dev CODEX_HOME ($CLAX_DEV_CODEX_HOME, else
#           ~/.clax-dev/codex-home; log in there once), where this checkout
#           is re-added as the `clax` marketplace and the plugin reinstalled
#           before each start. ~/.codex is never read or written.
# CLAX_DEV_BIN=<binary> uses that binary instead of building (tests).
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT="$(pwd -P)"
. scripts/dev-home.sh

harness="${1:-claude}"
if [ $# -gt 0 ]; then shift; fi
case "$harness" in
    claude | codex | pi) ;;
    *) echo "usage: just dev [claude|codex|pi] [harness arguments...]" >&2; exit 2 ;;
esac
command -v "$harness" >/dev/null 2>&1 || { echo "clax dev: $harness is not on PATH" >&2; exit 1; }

if [ -n "${CLAX_DEV_BIN:-}" ]; then
    bin="$CLAX_DEV_BIN"
else
    [ -f web/dist/index.html ] || (cd web && npm ci --silent && npm run build)
    cargo build -q -p clax-cli --bin clax
    bin=target/debug/clax
fi
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
cp "$bin" "$tmp/clax"
export PATH="$tmp:$PATH"
export CLAX_HOME="${CLAX_HOME:-$HOME/.clax-dev}"
ensure_dev_home "$CLAX_HOME" 7481
stop_orphan_daemon "$CLAX_HOME"
echo "clax dev: $("$tmp/clax" --version) at $tmp/clax, CLAX_HOME=$CLAX_HOME"

case "$harness" in
    claude)
        claude --plugin-dir "$ROOT/plugins/claude-code" --settings '{"enabledPlugins":{"clax@clax":false}}' "$@"
        ;;
    pi)
        pi -ne -e "$ROOT/plugins/pi/src/clax.ts" --skill "$ROOT/plugins/pi/skills/clax" "$@"
        ;;
    codex)
        export CODEX_HOME="${CLAX_DEV_CODEX_HOME:-$HOME/.clax-dev/codex-home}"
        mkdir -p "$CODEX_HOME"
        if [ ! -f "$CODEX_HOME/auth.json" ]; then
            echo "clax dev: $CODEX_HOME has no login yet; Codex will ask you to log in once."
        fi
        codex plugin remove clax@clax >/dev/null 2>&1 || true
        codex plugin marketplace remove clax >/dev/null 2>&1 || true
        codex plugin marketplace add "$ROOT" >/dev/null
        codex plugin add clax@clax >/dev/null
        codex --enable hooks "$@"
        ;;
esac
```

`chmod +x scripts/dev.sh`.

- [ ] **Step 4: `scripts/test-dev.sh`**

```bash
#!/usr/bin/env bash
# Tests scripts/dev-home.sh and scripts/dev.sh with a scratch HOME, fake
# `claude`, `codex` and `pi` commands and a fake clax. No cargo build, no real
# harness, no daemon on 7480 or 7481.
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd -P)"
T="$(cd "$(mktemp -d)" && pwd -P)"
trap 'rm -rf "$T"' EXIT
FAILED=0
pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1"; FAILED=1; }
export HOME="$T/home"
mkdir -p "$HOME"
unset CLAX_HOME CLAX_DEV_PORT CLAX_DEV_CODEX_HOME CODEX_HOME CLAUDE_CONFIG_DIR PI_CODING_AGENT_DIR

. "$HERE/dev-home.sh"

watch_settings --bind 0.0.0.0
if [ "$DEV_HOME" = "$HOME/.clax-dev" ] && [ "$DEV_PORT" = 7481 ] && [ "${DEV_ARGS[*]}" = "--bind 0.0.0.0" ]; then
    pass "just watch defaults to ~/.clax-dev on 7481 and passes other arguments on"
else fail "just watch defaults ($DEV_HOME $DEV_PORT ${DEV_ARGS[*]-})"; fi
watch_settings --shared
if [ "$DEV_HOME" = "$HOME/.clax" ] && [ "$DEV_PORT" = 7480 ] && [ -z "${DEV_ARGS[*]-}" ]; then
    pass "just watch --shared serves ~/.clax on 7480"
else fail "just watch --shared ($DEV_HOME $DEV_PORT)"; fi

ensure_dev_home "$T/dh" 7481
ensure_dev_home "$T/dh" 9999
if [ "$(grep -c '^\[serve\]' "$T/dh/config.toml")" = 1 ] && grep -qx 'port = 7481' "$T/dh/config.toml" \
    && [ "$(stat -c %a "$T/dh" 2>/dev/null || stat -f %Lp "$T/dh")" = 700 ]; then
    pass "ensure_dev_home records the port once and keeps the home private"
else fail "ensure_dev_home ($(cat "$T/dh/config.toml"))"; fi

# A daemon whose recorded executable is gone is stopped; one whose executable
# exists is left alone.
sleep 60 &
orphan=$!
sleep 60 &
kept=$!
mkdir -p "$T/o1" "$T/o2"
printf '{\n  "port": 1,\n  "pid": %s,\n  "exe": "%s"\n}\n' "$orphan" "$T/gone/clax" > "$T/o1/daemon.json"
printf '{\n  "port": 1,\n  "pid": %s,\n  "exe": "%s"\n}\n' "$kept" "$HERE/dev.sh" > "$T/o2/daemon.json"
stop_orphan_daemon "$T/o1" >/dev/null
stop_orphan_daemon "$T/o2" >/dev/null
sleep 0.3
if ! kill -0 "$orphan" 2>/dev/null && kill -0 "$kept" 2>/dev/null; then
    pass "only a dev daemon whose binary is gone is stopped"
else fail "only a dev daemon whose binary is gone is stopped"; fi
kill "$orphan" "$kept" 2>/dev/null
wait 2>/dev/null

# Fake harnesses record what they were run with, and what `clax` was on PATH.
FAKE="$T/fake"
mkdir -p "$FAKE"
for h in claude codex pi; do
    cat > "$FAKE/$h" <<SH
#!/bin/sh
c="\$(command -v clax)"
echo "$h \$* | clax=\$c (\$(clax --version)) home=\${CLAX_HOME:-} codex_home=\${CODEX_HOME:-}" >> "$T/calls"
SH
    chmod +x "$FAKE/$h"
done
printf '#!/bin/sh\necho "clax 9.9.9-dev"\n' > "$T/clax-build"
chmod +x "$T/clax-build"
devrun() { : > "$T/calls"; CLAX_DEV_BIN="$T/clax-build" PATH="$FAKE:$PATH" "$HERE/dev.sh" "$@" >/dev/null 2>"$T/err"; }

devrun claude --resume
line="$(cat "$T/calls")"
tmpdir="$(printf '%s' "$line" | sed -n 's#.*clax=\(.*\)/clax (.*#\1#p')"
if echo "$line" | grep -qF "claude --plugin-dir $ROOT/plugins/claude-code --settings {\"enabledPlugins\":{\"clax@clax\":false}} --resume | clax=$tmpdir/clax (clax 9.9.9-dev) home=$HOME/.clax-dev" \
    && [ -n "$tmpdir" ] && [ ! -e "$tmpdir" ] && grep -qx 'port = 7481' "$HOME/.clax-dev/config.toml"; then
    pass "just dev claude runs the build from a removed-afterwards tmpdir, the checkout's plugin, and ~/.clax-dev on 7481"
else fail "just dev claude ($line; tmpdir=$tmpdir; $(cat "$T/err"))"; fi

devrun pi
if grep -qF "pi -ne -e $ROOT/plugins/pi/src/clax.ts --skill $ROOT/plugins/pi/skills/clax | clax=" "$T/calls"; then
    pass "just dev pi loads the checkout's extension and skill"
else fail "just dev pi ($(cat "$T/calls"))"; fi

devrun codex
dev_codex="$HOME/.clax-dev/codex-home"
expected="codex plugin remove clax@clax | clax=
codex plugin marketplace remove clax | clax=
codex plugin marketplace add $ROOT | clax=
codex plugin add clax@clax | clax=
codex --enable hooks | clax="
got="$(sed 's/ | clax=.*/ | clax=/' "$T/calls")"
if [ "$got" = "$expected" ] && ! grep -v "codex_home=$dev_codex\$" "$T/calls" | grep -q . && [ ! -e "$HOME/.codex" ]; then
    pass "just dev codex reinstalls the checkout's plugin in a dev CODEX_HOME and never touches ~/.codex"
else fail "just dev codex ($(cat "$T/calls"))"; fi

devrun bogus
if [ -z "$(cat "$T/calls")" ] && grep -q "usage: just dev" "$T/err"; then pass "an unknown harness prints usage"
else fail "an unknown harness prints usage ($(cat "$T/err"))"; fi

: > "$T/calls"
if CLAX_DEV_BIN="$T/clax-build" PATH="/usr/bin:/bin" "$HERE/dev.sh" claude >/dev/null 2>"$T/err"; then fail "a missing harness CLI fails"
else grep -q "claude is not on PATH" "$T/err" && pass "a missing harness CLI fails and says so" || fail "a missing harness CLI ($(cat "$T/err"))"; fi

[ "$FAILED" = 0 ] && echo "dev script tests passed" || echo "dev script tests FAILED"
exit "$FAILED"
```

`chmod +x scripts/test-dev.sh`. In `scripts/quality_gates.sh`, after the `release installer` line, add:

```bash
run "dev scripts"           scripts/test-dev.sh
```

- [ ] **Step 5: The justfile**

Replace the `dev`, `install`, `uninstall`, `serve`, `stop` and `doctor` recipes with:

```make
# Build clax and start a harness with the plugin from this checkout, on ~/.clax-dev and port 7481 (claude, codex or pi)
dev HARNESS="claude" *ARGS:
    ./scripts/dev.sh {{HARNESS}} {{ARGS}}

# Run the auto-reloading daemon and web UI (~/.clax-dev on 7481; --shared: ~/.clax on 7480)
watch *ARGS:
    ./scripts/watch.sh {{ARGS}}

# Install clax from this checkout into ~/.cargo/bin and register its plugins with each harness found
install: web
    cargo install --locked --path crates/clax-cli
    "${CARGO_HOME:-$HOME/.cargo}/bin/clax" init
    @b="${CARGO_HOME:-$HOME/.cargo}/bin/clax"; f="$(command -v clax || true)"; if [ "$f" != "$b" ]; then echo "warning: the first clax on PATH is ${f:-none}, not $b; the plugins run the first one, so put ${b%/clax} first on PATH" >&2; fi

# Remove the plugin registrations and the clax installed by `just install`
uninstall:
    -"${CARGO_HOME:-$HOME/.cargo}/bin/clax" uninit
    -cargo uninstall clax-cli

# Run the dev daemon in the foreground on ~/.clax-dev, port 7481 (extra args go to `clax serve`)
serve *ARGS:
    CLAX_HOME="${CLAX_HOME:-$HOME/.clax-dev}" cargo run -p clax-cli -- serve --foreground --port 7481 {{ARGS}}

# Stop the dev daemon (~/.clax-dev)
stop:
    CLAX_HOME="${CLAX_HOME:-$HOME/.clax-dev}" cargo run -q -p clax-cli -- stop

# Check the dev home and its daemon (~/.clax-dev)
doctor:
    CLAX_HOME="${CLAX_HOME:-$HOME/.clax-dev}" cargo run -q -p clax-cli -- doctor
```

In `scripts/test-justfile.sh`, before `exit "$missing"`, add:

```bash
recipes="$(just --summary)"
for r in dev watch install uninstall serve stop doctor; do
    case " $recipes " in *" $r "*) ;; *) echo "missing recipe: $r" >&2; missing=1 ;; esac
done
for r in dev-install dev-uninstall; do
    case " $recipes " in *" $r "*) echo "recipe $r should not exist" >&2; missing=1 ;; esac
done
```

- [ ] **Step 6: The Vite dev proxy follows the dev daemon**

In `web/vite.shell.config.ts`, change every `http://127.0.0.1:7480` in `server.proxy` to `http://127.0.0.1:7481`. No gate starts Vite's dev server. A person who does is proxied to `just watch`'s daemon.

- [ ] **Step 7: Run**

Run: `scripts/test-dev.sh && scripts/test-justfile.sh && echo justfile-ok`
Expected: `dev script tests passed` and `justfile-ok`.

Do not run `just install`, `just uninstall`, `just dev`, `just watch`, `just serve` or `just stop` here. They act on the person's `~/.cargo/bin`, harness registrations, `~/.clax` or `~/.clax-dev`. "Steps for the person" runs them.

- [ ] **Step 8: Gates and commit**

Run: `bash scripts/quality_gates.sh; echo "exit=$?"`
Expected: `exit=0`.

```bash
git add scripts/dev-home.sh scripts/dev.sh scripts/watch.sh scripts/test-dev.sh justfile scripts/test-justfile.sh scripts/quality_gates.sh web/vite.shell.config.ts
git commit -m "Add just install/uninstall with clax init, just dev per harness, and just watch on ~/.clax-dev"
git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed
```

---

### Task 11: Documentation

**Files:**
- Modify: `README.md`, `plugins/claude-code/README.md`, `plugins/clax/README.md`, `plugins/pi/README.md`, `docs/contract.md`, `docs/superpowers/plans/2026-09-29-svelte-port.md`, `docs/superpowers/plans/2026-09-30-agent-working.md`

**Interfaces:**
- Consumes: everything above. The docs describe only what Tasks 2–10 built.

- [ ] **Step 1: README, "Install from source" becomes "Install"**

Replace the "Install from source" section with:

````markdown
## Install

From a clone of this repository (Rust 1.94, Node 22 and `just`):

```
just install
```

It builds the web UI and `clax`, and installs `clax` into `~/.cargo/bin`.
Then it runs `clax init`, which registers the Clax plugins built into that
binary with each harness whose CLI is on your `PATH`: Claude Code, Codex and
Pi. The registration replaces any older one, including registrations under
Clax's previous name. Start a new session in each harness afterwards. The
plugins run the `clax` on the `PATH` the harness starts with, so
`~/.cargo/bin` must be on it. `just uninstall` removes the registrations and
the binary. Nothing is downloaded.

Without a clone, once the repository is public:

```
curl -fsSL https://github.com/empathic/clax/releases/latest/download/install.sh | bash
clax init
```

`install.sh` puts the release's `clax` in `~/.local/bin`, after checking it
against the release's `SHA256SUMS`.

When a plugin half works, `clax doctor --agent <claude|codex|pi>` checks each
layer. Its `binary` check shows which `clax` the plugins run and every `clax`
on `PATH`. If the MCP server cannot find `clax` at all, its one tool,
`status`, says why, and `~/.clax/logs/hooks.log` has the details.
````

In "Use from an agent", replace the three install bullets with one sentence, `` `clax init` registers the plugin with each harness (see Install); plugin details: [plugins/claude-code/README.md](plugins/claude-code/README.md), [plugins/clax/README.md](plugins/clax/README.md), [plugins/pi/README.md](plugins/pi/README.md). `` Replace the paragraph that begins `Build the binary first` with:

```markdown
The plugins run `clax` from `PATH` (or `CLAX_BIN`, for scripts) through
`scripts/ensure-clax.sh`, which never downloads or builds anything. A `clax`
whose version differs from the plugin's runs with a warning, and `status` and
`clax doctor --agent` report the difference; `just install` (or `clax init`)
brings them back in step.
```

- [ ] **Step 2: README, "Upgrading"**

Append to the "Upgrading" section:

```markdown
### From an earlier source install

Earlier versions guessed which `clax` to run: from `PATH`, `~/.local/bin`,
`~/.clax/bin`, or a checkout's `target/`. Now the plugins run the `clax` on
`PATH`, and `clax init` registers them. In the checkout, run `just install`.
It re-registers every harness from `~/.clax/marketplace/`, which replaces
registrations that pointed at an old or moved checkout, and removes those
under Clax's previous name. Then remove binaries that nothing should run:
check `which -a clax`, and delete an old `~/.local/bin/clax` or
`~/.clax/bin/clax`. Unset `CLAX_SOURCE_DIR` and `CLAX_INSTALL_DIR`
wherever you set them. Check with `clax doctor --agent <claude|codex|pi>`.

A newer daemon is never replaced by an older `clax`. After installing an
older version, run `clax stop` once.
```

- [ ] **Step 3: README, "Development" becomes "Developing Clax"**

Replace the "Development" section's first bullet (`just dev` …, with its two sub-bullets) with:

````markdown
## Developing Clax

Two loops, both on a home of their own, `~/.clax-dev`, whose daemon listens on
port 7481. Your agents' home, `~/.clax`, their daemon on 7480, and the `clax`
they have installed are never touched.

**The daemon and the web UI: `just watch`.** An auto-reloading server at
http://localhost:7481. A Rust change rebuilds and restarts the daemon; a web
change rebuilds `web/dist` (reload the browser). Extra arguments go to
`serve` (`just watch --bind 0.0.0.0`). `just watch --shared` serves the
agents' home on 7480 instead (stop their daemon first with `clax stop`).
`just stop`, `just serve` and `just doctor` act on `~/.clax-dev`. It needs
`cargo-watch` (`cargo install cargo-watch`).

**An agent on your working tree: `just dev [claude|codex|pi]`.** It builds
`clax`, copies it into a temporary directory put first on `PATH` (removed
when the session ends), and starts the harness with the plugin, skill and
hooks loaded from this checkout:

```
just dev                 # Claude Code (claude --plugin-dir plugins/claude-code)
just dev codex
just dev pi
just dev claude --resume # extra arguments go to the harness
```

- Claude Code loads `plugins/claude-code` straight from the checkout, and an
  installed `clax@clax` is disabled for that session.
- Pi loads `plugins/pi/src/clax.ts` and its skill from the checkout. It runs
  with `-ne` so an installed Clax package does not load twice, which turns
  off your other Pi extensions for that session too.
- Codex cannot load a plugin from a directory. `just dev codex` runs on its
  own `CODEX_HOME`, `~/.clax-dev/codex-home`, and reinstalls this checkout's
  plugin there before every start. Log in once there (Codex asks). Your
  `~/.codex` is never touched.

Edit the plugin, skill or hooks, then start a new `just dev` session to pick
them up. `just install` puts the working tree in front of your everyday
agents.
````

Keep the remaining "Development" bullets (`just check`, `just ci`) and the `scripts/quality_gates.sh` paragraph under the new heading. In that paragraph:
- Replace `the installer's `MIN_VERSION` carry one version` with `the plugins' wrapper's `CLAX_VERSION` carry one version (`scripts/check-version.sh`)`.
- Add `the release script, release installer and dev script tests (`scripts/test-release.sh`, `scripts/test-install.sh`, `scripts/test-dev.sh`), ` after `the justfile, installer, `.

- [ ] **Step 4: README, "Releasing"**

Add after "Developing Clax":

````markdown
## Releasing

Releases are for people without a checkout. Only a person cuts one, and only
once the repository is public: `install.sh` downloads without credentials.

```
scripts/bump-version.sh 0.4.0
python3 scripts/sync-skill-tools.py     # the skills state the plugin version
just ci
git commit -am "Release 0.4.0"
git tag -s v0.4.0 -m "Clax 0.4.0"
git push origin main v0.4.0
```

The tag runs `.github/workflows/release.yml`. It checks that the tag matches
every version, then builds macOS arm64 and x86_64 and Linux x86_64 and arm64
binaries on native runners, each with the web UI embedded. It smoke-tests each
binary and packs `clax-<version>-<target>.tar.gz`. It installs one with
`install.sh` from a local copy of the release, and publishes the archives,
`install.sh` and `SHA256SUMS`. Running the workflow by hand (Actions, Release,
Run workflow) does everything except publish, and keeps the result as the
`release-dist` artifact.
````

- [ ] **Step 5: The plugin READMEs**

In `plugins/claude-code/README.md` and `plugins/clax/README.md`, replace the "Install" section (through the paragraph that begins `Hooks never download anything`) with:

```markdown
## Install

From a clone of the Clax repository, run `just install`: it installs `clax`
into `~/.cargo/bin` and runs `clax init`, which registers this plugin (the
copy built into that binary, written to `~/.clax/marketplace/`). Then start a
new session.

The plugin runs `clax` from the `PATH` the harness starts with (or
`CLAX_BIN`), through a small wrapper, `scripts/ensure-clax.sh`. The wrapper
never downloads or builds anything. A `clax` whose version differs from the
plugin's runs with a warning in `~/.clax/logs/hooks.log`. Without any `clax`,
the MCP server still starts, with a single tool, `status`, that says why and
how to fix it. Hooks print one line, log it, and exit 0, so a missing binary
never fails a turn.
```

For `plugins/clax/README.md` only, add after that section: `` `codex mcp list` then shows the `clax` server. Codex runs plugin hooks only with `features.hooks = true` and after you trust them (see below). ``

Replace the "Working from a source checkout" section with:

```markdown
## Working from a source checkout

`just dev claude` (or `just dev codex`) starts a session on a fresh build
with this plugin loaded from the checkout, on a separate home and port;
"Developing Clax" in the top-level README has the details, including why
Codex runs on its own `CODEX_HOME` there. `just install` updates your
everyday install.
```

In "When something is missing", describe the `binary` check as `` `binary` (this `clax`, the one the plugins run, and every `clax` on `PATH`) ``, and drop any mention of the release download.

In `plugins/pi/README.md`, replace the paragraph on finding the `clax` binary (`CLAX_BIN` or `PATH`) with:

```markdown
The extension runs `CLAX_BIN`, else the `clax` on `PATH`, and never
downloads. `just install` in the checkout installs `clax` and runs
`clax init`, which `pi install`s this package from the copy built into the
binary. `just dev pi` loads it from the checkout instead. `status` reports
`binary`: the path it runs.
```

- [ ] **Step 6: `docs/contract.md`**

1. In "### status", add `"binary": {"path": "/Users/alex/.cargo/bin/clax", "version": "0.2.0"},` to the example after `"feedback": []`, and add after the `plugin_version` paragraph:

```markdown
`binary` is the executable answering and its version: the `clax` the plugin
ran (from `PATH`, or `CLAX_BIN`), or, under Pi, the one the extension runs
(`{"path": null, "error": "<why>"}` when it finds none).
```

2. Replace the "Version skew:" paragraph at the end of "### status" with:

```markdown
Version skew: a shim that finds a daemon older than itself replaces it on the
old daemon's port and bind address, holding the daemon's start lock
throughout, so no other client starts one in the gap. It asks the daemon to
shut down: SSE streams end (browsers reconnect to the same port) and long
polls return what they have. In-flight requests get 5 s, and one that is cut
off fails with a connection error the agent can retry. It then waits up to 7 s
for the old daemon to exit and starts its own. A newer daemon, one of the same
version, or one whose version does not parse, is kept. `daemon.json` records
the daemon's `version` and `exe`.
```

3. Add a section `## Installation and the wrapper` before `## Security model`:

```markdown
## Installation and the wrapper

`clax init` writes the plugins built into the binary to
`~/.clax/marketplace/` and registers them with each harness whose CLI is on
`PATH`: `claude plugin marketplace add` and `claude plugin install
clax@clax`; `codex plugin marketplace add` and `codex plugin add
clax@clax`; `pi install ~/.clax/marketplace/plugins/pi`. It first removes the
existing Clax registrations and any under Clax's previous name. `--agent`
limits it to named harnesses. `clax uninit` removes the registrations and the
marketplace directory. Neither touches Clax's data.

The Claude Code and Codex plugins start `clax` through
`scripts/ensure-clax.sh`, which runs `CLAX_BIN`, else the first `clax` on
`PATH` whose `--version` names clax. It never downloads, builds, or looks
anywhere else. A `clax` of another version than the plugin's runs; the MCP
server and other commands warn about it, hooks stay silent. Before it starts
the MCP server, the wrapper runs `clax mcp --preflight`, which reads the home,
its `config.toml` and the port without starting a daemon, and then replaces
itself with `clax mcp`, so the harness is its parent. When there is no usable
`clax`, or the preflight fails:

- The MCP server answers the MCP client itself. `initialize` succeeds, with
  `instructions` that start `Clax is unavailable:`. `tools/list` offers one
  tool, `status`, whose call returns the reason and the fix
  (`isError: true`), or says to reconnect once the cause is gone. `ping`
  answers `{}`. Any other request gets JSON-RPC error -32601 with the same
  reason. A `clax mcp` that exits later in a session is not relayed: the
  client sees the connection close.
- A hook (no usable `clax` only) prints one line to stderr and exits 0.
- Other commands print the reason and exit 1.

Every MCP start adds a `launch mode=mcp agent=<harness> bin="<path>"
version="<version>" warning="<text>"` line to `~/.clax/logs/hooks.log`
(under `CLAX_HOME` when set). Every failure adds a `launcher mode=<mode>
agent=<harness> exit=<status> reason="<why>" tried="<candidates>"
argv="<arguments>"` line.
```

4. In "## Security model", replace the "No telemetry" bullet with the §14 text from Task 1, Step 7.

5. In "## Known limitations", add:

```markdown
- The plugins run the `clax` on the `PATH` their harness starts with. A
  harness started from a desktop launcher may not have `~/.cargo/bin` on its
  `PATH`; `status`, the fallback server and `clax doctor --agent` say so.
- Sessions that were running when a daemon was replaced keep their shim's
  binary until they restart.
- If a replaced daemon's port is taken while it restarts, the new daemon
  binds one of the next 20 ports and open browser tabs must be reloaded.
- `install.sh` needs the repository to be public: GitHub serves a private
  repository's release files only to authenticated requests.
- Codex cannot load a plugin from a directory, so `just dev codex` reinstalls
  the checkout's plugin into a dev `CODEX_HOME` before each start.
```

- [ ] **Step 7: Other plans' port assumptions**

`just watch` (formerly `just dev`) and `just dev` now bind 7481. In `docs/superpowers/plans/2026-09-29-svelte-port.md`:
- In Global Constraints, replace `Never bind or connect to port 7480.` with `Never bind or connect to port 7480 or 7481 (the agents' daemon and the dev daemon).`
- In Task 10 Step 5, replace `never `just dev`, which binds 7480:` with `never `just watch` or `just dev`, which bind 7481 and serve `~/.clax-dev`:`
- In Task 11's `vite.shell.config.ts` block, replace each `http://127.0.0.1:7480` in `server.proxy` with `http://127.0.0.1:7481`, to match the file after this plan's Task 10. Add `(as the stable-install plan left it)` to the note that says the block is unchanged.
- Where it says `just dev` (CLAX_DEV=1) (the bridge parts' stable names), change `just dev` to `just watch`.

The timing harness (`web/perf/usable.perf.ts`) starts its daemons through `startDaemon()` with `--port 0`, and needs no change. The `7481` in `web/shell/src/frame-src-cases.json` is an example origin in a unit test, never bound, and needs no change either.

In `docs/superpowers/plans/2026-09-30-agent-working.md`, in Global Constraints, replace `Never bind or connect to port 7480.` with `Never bind or connect to port 7480 or 7481.` Its `tool-hook.sh` calls `ensure-clax.sh exec hook`, which Task 4 kept.

- [ ] **Step 8: Check the docs**

Run: `python3 scripts/sync-skill-tools.py --check && bash scripts/test-plugins.sh | tail -1 && git grep -n "MIN_VERSION\|CLAX_SOURCE_DIR\|CLAX_INSTALL_DIR\|dev-link\|dev-install\|cargo install --path crates/clax-cli" -- README.md plugins/*/README.md docs/contract.md plugins/pi/src`
Expected: the sync check passes, `plugin checks passed`, and the `git grep` prints nothing.

- [ ] **Step 9: Gates and commit**

Run: `bash scripts/quality_gates.sh; echo "exit=$?"`
Expected: `exit=0`.

```bash
git add README.md plugins/claude-code/README.md plugins/clax/README.md plugins/pi/README.md docs/contract.md docs/superpowers/plans/2026-09-29-svelte-port.md docs/superpowers/plans/2026-09-30-agent-working.md
git commit -m "Document just install and clax init, just dev and just watch, the wrapper, and releasing"
git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed
```

---

### Task 12: Version 0.3.0

**Files:**
- Modify: every file `scripts/bump-version.sh` writes, the three skills' generated blocks, `docs/contract.md` (`status` example), and the Rust tests that pin the current version

**Interfaces:**
- Produces: the workspace, both plugin manifests, the marketplace, the Pi package and the wrapper at `0.3.0`, ready for the person to tag `v0.3.0`. Agents do not tag.

- [ ] **Step 1: Bump**

```bash
scripts/bump-version.sh 0.3.0
python3 scripts/sync-skill-tools.py
```

Expected: `bumped to 0.3.0`, and the three `SKILL.md` blocks now read `This is Clax plugin 0.3.0.`.

- [ ] **Step 2: Tests that pinned 0.2.0**

Run: `git grep -n '0\.2\.0' -- crates scripts plugins docs/contract.md ':!**/package-lock.json'`. Each hit falls into one of two kinds, handled differently:
- A test that writes a manifest, skill block or cache path meant to *match* the binary (for example `.codex/plugins/cache/clax/clax/0.2.0` with a matching manifest in `doctor_agent.rs` and `tests/cli.rs`). It now uses `env!("CARGO_PKG_VERSION")` (Rust) or `$(scripts/check-version.sh --print)` (shell), so it never pins a version again.
- A test that means *another* version (a stale plugin, an older daemon in `needs_replacing`, `clean_break.rs`'s fake daemon). It keeps its literal.

In `docs/contract.md`, change the `status` example's `"version"` and `binary.version` to `"0.3.0"`.

- [ ] **Step 3: Gates and commit**

Run: `scripts/check-version.sh v0.3.0 && bash scripts/quality_gates.sh; echo "exit=$?"`
Expected: `exit=0`.

```bash
git add -u
git status --short   # only the files the bump, the skill sync and Step 2 changed
git commit -m "Version 0.3.0"
git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed
```

---

## Steps for the person

Agents stop at the end of Task 12. Everything below touches your real homes, harness registrations or GitHub, so only you do it.

### A. Before the plan runs

A Task 1 run of the superseded plan left uncommitted edits in the spec (a D16 row about `~/.clax/bin/<version>` and dev links). If they are still there, discard them, so Task 1 starts from the committed spec:

```bash
cd /Users/alex/Devel/empathic/clax
git diff --stat -- docs/superpowers/specs/2026-09-28-clax-design.md   # shows only that run's edits
git checkout -- docs/superpowers/specs/2026-09-28-clax-design.md
```

### B. Move this machine to the new install

```bash
cd /Users/alex/Devel/empathic/clax
git pull
just install
which -a clax        # ~/.cargo/bin/clax must come first
```

`just install` runs `clax init`. That re-registers Claude Code, Codex and Pi from `~/.clax/marketplace/`, replacing the Codex marketplace that pointed at the old checkout path, and removes registrations under the previous name. Its output lists each harness as `registered` or `skipped`. Then:

- Remove stale binaries that `which -a clax` shows ahead of or beside `~/.cargo/bin/clax`: an old `~/.local/bin/clax` or `~/.clax/bin/clax`. Unset `CLAX_SOURCE_DIR` and `CLAX_INSTALL_DIR` in your shell profile.
- Start a new session in each harness. Run `clax doctor --agent claude`, then `codex`, then `pi`: `binary` must say the plugins run `~/.cargo/bin/clax`, and `plugin` must pass. Ask the agent to call `status`; `binary.path` must be `~/.cargo/bin/clax`.
- Pi: check that the extension loads from `~/.clax/marketplace/plugins/pi`, which has no `node_modules` (`pi list`, then a session in which the `clax_*` tools appear). If it cannot resolve `typebox` there, tell the agents; the fix is to register the checkout's `plugins/pi` for Pi instead.
- The fallback: start `env PATH=/usr/bin:/bin claude` (a PATH without `clax`), open `/mcp`, and check that the `clax` server is connected with the single tool `status`, which says `clax` is not on `PATH`. Exit.

Try the dev loops:
- `just watch` serves http://localhost:7481 from `~/.clax-dev`. Your agents keep working on 7480.
- `just dev` (Claude Code): check in `/plugin` that only the checkout's Clax plugin is active. If an installed `clax@clax` still loads beside it, disable it for dev sessions with `claude plugin disable clax@clax`, and tell the agents.
- `just dev codex`: log in once when Codex asks. The login lives in `~/.clax-dev/codex-home` and never in `~/.codex`. To reuse your normal login, copy `~/.codex/auth.json` there yourself.
- `just dev pi`.

### C. Cut the first release, v0.3.0, when you choose to make the repository public

Releases are only for people without a checkout, and nothing local depends on them.

1. Make the repository public when you decide to (Settings, General, Change visibility). Until then, a release can be published, but `install.sh` gets 404.
2. Run the release workflow by hand on `main` (Actions, Release, Run workflow). All four builds and `assemble` must pass. If a runner label is unavailable, change that one job:
   - `x86_64-apple-darwin` builds on `macos-15` with the same `--target`. Its smoke step then runs under Rosetta (`softwareupdate --install-rosetta --agree-to-license`).
   - `aarch64-unknown-linux-musl` builds on `ubuntu-24.04` with `cargo install cargo-zigbuild`, `pip install ziglang` and `cargo zigbuild --release --locked --target aarch64-unknown-linux-musl -p clax-cli --bin clax`. Its smoke step cannot run there, so skip it for that target.
3. Download the `release-dist` artifact and check it holds four archives, `install.sh` and `SHA256SUMS`.
4. Tag and push:

```bash
scripts/check-version.sh v0.3.0
git tag -s v0.3.0 -m "Clax 0.3.0"
git push origin main v0.3.0
```

5. When the publish job has finished, and the repository is public, test the installer into a scratch directory:

```bash
T="$(mktemp -d)"
curl -fsSL https://github.com/empathic/clax/releases/latest/download/install.sh | CLAX_INSTALL_DIR="$T" bash
xattr -l "$T/clax"                           # prints nothing: no com.apple.quarantine
codesign -dv "$T/clax" 2>&1 | grep -i adhoc
"$T/clax" --version                          # clax 0.3.0
rm -rf "$T"
```
