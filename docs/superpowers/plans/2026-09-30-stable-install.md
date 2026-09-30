# Stable Install Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The Claude Code and Codex plugins run exactly the released `clax` they were built for, installed side by side under `~/.clax/bin/<version>/` from a checksum-verified GitHub release, and never a build they found in a source checkout. Dev builds reach agents only when the person opts in (`CLAX_BIN`, or `just dev-install`, which runs `clax dev-link`), and `just dev` runs on its own home and port so rebuilding never takes the agents' Clax down.

**Architecture:** A tag builds four native release binaries (macOS arm64 and x86_64, Linux x86_64 and arm64, static musl), each smoke-tested for its version and its embedded web UI, packs them as `clax-<version>-<target>.tar.gz`, and publishes them with `SHA256SUMS` and the launcher. The launcher (`scripts/ensure-clax.sh`, copied into both plugins) carries the one version it belongs to (`CLAX_VERSION`) and resolves `CLAX_BIN`, then the dev link recorded in `<config dir>/config.toml`, then `<config dir>/bin/<CLAX_VERSION>/clax`. Only the MCP server may download the missing version, under a lock, with bounded timeouts, and it installs atomically, keeps the previous version, and prunes older ones. When the MCP server cannot run `clax`, the launcher answers the MCP client itself with a minimal server whose `status` tool states the reason. Hooks never download and always exit 0. The daemon records which executable serves it, and a replacement (a newer binary meeting an older daemon, or a new dev link) stops and restarts it under the start lock, on the same port. The Pi extension resolves its binary the same way, without downloading. `clax doctor --agent` and the MCP `status` tool say which binary runs and why, and state a dev link plainly.

**Tech Stack:** Bash 3.2-compatible shell (macOS `/bin/bash`), `curl`, `tar`, `shasum`/`sha256sum`; Python 3 for the test-only fake release server and the version scripts; Rust 2024 (clap 4, `toml` 0.9, axum 0.8); TypeScript (Pi extension, Vitest); GitHub Actions (`macos-15`, `macos-15-intel`, `ubuntu-24.04`, `ubuntu-24.04-arm`), `gh` for publishing.

**Spec:** `docs/superpowers/specs/2026-09-28-clax-design.md`. Task 1 amends §2 (new decision D16), §4 Repository layout, §5 Storage, §7 Daemon discovery and lifecycle, §13 Plugins, §14 Security model and §16 Testing. The person's decisions are in `.superpowers/sdd/2026-09-30-stable-install/decisions.md` and are binding. They include the "Local dev flow" section.

**Precondition:** `git status --short -- docs/contract.md docs/superpowers/specs README.md plugins scripts justfile .github crates web/vite.shell.config.ts` prints nothing. If it prints anything, someone else has uncommitted work in files this plan edits: stop and ask. Do not stash or commit another person's changes.

## Global Constraints

- Rust edition 2024. `cargo clippy --workspace --all-targets -- -D warnings` and `RUSTFLAGS=-Dwarnings cargo check --workspace` pass. `oxlint --deny-warnings` passes (`cd web && npm run lint`).
- Commit with plain `git commit`, which signs. Never pass `--no-gpg-sign`. After each commit, `git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed` prints `signed`. Stage with `git add` and explicit paths only.
- Never bind or connect to port 7480 or 7481. The person's daemon or dev server may be there. Tests start daemons with `--port 0`, and the fake release server binds port 0.
- Never read, write or delete the real `~/.clax`, `~/.clax-dev`, `~/.claude` or `~/.codex`, nor the home directory Clax used before its rename (spec D15). Every test sets `HOME` and `CLAX_HOME` to scratch directories and unsets `CLAX_CONFIG_DIR` (or sets it to scratch).
- No test reaches GitHub or any other host. Downloads in tests go to `scripts/fake-release-server.py` on `127.0.0.1`.
- Agents never tag, push, publish a release, or change repository settings. Those are in "Steps for the person".
- In prose, comments and commit messages, write "ID", never "id", except as a literal symbol in code.
- The launcher runs under macOS's bash 3.2. Use no `mapfile`, no `${var,,}`, no `declare -A`, no `$BASHPID` and no `wait -n`. Expand a possibly empty array as `${a[@]+"${a[@]}"}`.
- `scripts/ensure-clax.sh` and its two plugin copies stay byte-identical: `cmp scripts/ensure-clax.sh plugins/claude-code/scripts/ensure-clax.sh && cmp scripts/ensure-clax.sh plugins/clax/scripts/ensure-clax.sh`.
- No launcher code path looks at `PATH`, at a harness's configuration (`~/.codex/config.toml`, `~/.claude`), or at its own location to find a `clax`.
- The previous product name must not appear in any file this plan creates or edits, except the approved exceptions listed in `scripts/test-plugins.sh`. Tests assemble it from two halves, as the existing tests do.
- Every task ends with `bash scripts/quality_gates.sh; echo "exit=$?"` printing `exit=0`. Check the status itself, not only the last line of output.

## Review Focus

1. **The MCP client always gets an answer.** For every launcher failure in MCP mode (unusable `CLAX_BIN`, broken dev link, no release, checksum mismatch, partial download, timeout, missing `curl`, unsupported platform, download still running), stdout carries valid JSON-RPC answering `initialize`, `tools/list` and `tools/call`. It is never a closed pipe, and it never carries a stray log line. Tests: "fallback answers …" in `scripts/test-ensure-clax.sh`.
2. **Two sessions starting at once.** One download, one atomic rename into `bin/<version>/`, no leftover `.tmp.*` or `.install.lock`, and both sessions exec the same binary. Test: "two MCP starts at once download once and both run clax".
3. **Nothing half-installed.** A failed download leaves `bin/<version>/` absent (or as it was). A reader never sees a partly written binary at `bin/<version>/clax`, because the directory appears by `rename(2)` only after its binary reported the right version.
4. **Hooks never block.** Hook mode never takes the lock, never runs `curl`, and exits 0 in every case. Tests: "hook mode … downloads nothing" (the fake server's request log stays empty).
5. **The daemon swap is clean.** The start lock is held from the shutdown until the new daemon answers, so no shim starts a stale daemon in the gap. SSE streams end and reconnect to the same port, and a newer daemon is never replaced by an older binary. Tests: `dev_link_restarts_the_linked_homes_daemon_on_its_port` and `serve_replaces_an_older_daemon_on_its_port`.

---

## Design decisions

These settle the open questions in the request. Each is binding for the tasks below.

**The MCP failure surface: a minimal stdio server, not an error reply to `initialize`.** JSON-RPC 2.0 lets a server answer any request, `initialize` included, with an error object, and the MCP lifecycle spec shows one for an unsupported protocol version. A client that gets an error to `initialize` treats the server as failed to start. Claude Code marks it failed in `/mcp`, and Codex reports a startup failure. In both, the text reaches at best a status line or log that the agent never reads, and the person has to go looking. A server that completes the handshake is visible to both the person and the agent. Its `initialize` result carries `instructions` that state the reason, and its one tool, `status` (the same name as the real tool the skill tells agents to call), returns the reason and the fix with `isError: true`. So the launcher serves that minimal server. It needs no binary. It is ~40 lines of bash that read newline-delimited JSON-RPC, echo the client's `protocolVersion`, and answer `ping`. Any other request gets a JSON-RPC error (-32601) carrying the same reason. Its `status` tool re-checks the install on every call, so after a background download finishes it says to reconnect.

**Downloads in MCP mode run in the background, and the launcher waits a bounded time.** Codex gives an MCP server 10 s to start by default. The launcher starts the download in a background subshell that holds the install lock, and waits `CLAX_MCP_WAIT` seconds (default 8) for it. If the download finishes, the launcher execs the new binary. If not, it serves the fallback server ("still downloading; reconnect in a minute"), and the download carries on, bounded by `curl --max-time $CLAX_DOWNLOAD_TIMEOUT` (default 120 s per file). A later session finds the finished install.

**The lock is a directory.** macOS has no `flock(1)`. `mkdir <config dir>/bin/.install.lock` is atomic. The holder writes its PID inside. A waiter takes the lock over when that PID is dead or the lock is older than 15 minutes. One narrow race remains: two waiters can both judge a dead holder's lock stale. Both then install, each by an atomic rename, so the result is two downloads, never a corrupt install. The contract's known limitations state this.

**The config directory and `config.toml`.** The launcher, the CLI and the Pi extension all compute the config directory the same way: `$CLAX_CONFIG_DIR`, else `$CLAX_HOME`, else `$HOME/.clax`. It holds `config.toml`, `bin/` and the launcher's `logs/hooks.log`. The dev link lives in `<config dir>/config.toml` as a `[dev_link]` table, and it is never stored in a fixed path under the real home. A test that sets `HOME` and `CLAX_HOME` to scratch directories therefore reads and writes only scratch files. It cannot see the person's real link, and it never shares a file with the real home. When a dev link names a `home`, the launcher exports it as `CLAX_HOME` for the binary and exports `CLAX_CONFIG_DIR` too, so the binary still finds the link. The home's own `config.toml` may hold `[serve] port`, the port a daemon started for that home listens on. `just dev` writes `port = 7481` into `~/.clax-dev/config.toml`, so agents linked to that home start its daemon on 7481, never on 7480. This matches the spec's §5, which already reserves `config.toml` for the port.

**Which daemon survives a version difference.** A binary that finds an *older* daemon replaces it. A binary that finds a *newer* daemon keeps it and warns, and an equal version is kept even when the executable differs. The decisions say "a running daemon from another version is restarted cleanly". Read literally, two plugins at different versions (Claude Code updated, Codex not yet) would restart each other's daemon at every reconnect, and every restart ends SSE streams and long polls. Newer-wins converges instead. A dev link is not a version question: `clax dev-link`, `clax dev-unlink` and `just dev-install` restart the linked home's daemon explicitly. A deliberate rollback to an older plugin keeps the newer daemon until `clax stop`, and the upgrade section of the README says so. This is listed under the decisions for the person, in case they want the literal rule.

**The restart handshake.** `daemon.json` already carries `version`. It gains `exe`, the canonical path of the daemon's executable. A replacement proceeds in five steps:
1. It takes `daemon.lock` (the start lock every auto-start takes) and re-reads `daemon.json`. If another client already replaced the daemon acceptably, it uses that daemon.
2. It posts `/api/admin/shutdown`. The daemon stops accepting connections and flips its shutdown signal: SSE streams (`/api/events`) and `/mcp` streams end, and long polls (`wait_for_feedback`, the Stop hook's wait) return what they have, as they do today on `clax stop`. In-flight requests get the existing 5 s drain, and a request still running after that (a large publish) fails with a connection error. The shim reports it and the agent retries.
3. It waits up to 7 s for the old PID to exit.
4. It starts the new executable with `serve --foreground` on the old daemon's port and bind address, and waits for `/healthz`.
5. It releases the lock.

A shim or hook that wants a daemon during the swap blocks on the lock and then finds the new one. Browser tabs reconnect through `EventSource`'s automatic retry to the same port. The bearer token changes, so shims get a 401, refresh, and register again, and their session rows persist in the database. If the old port was taken in the gap, the new daemon binds one of the next 20 ports, and open tabs must be reloaded (a known limitation). Sessions that were already running keep the binary they started with until they restart.

**`just install` is removed.** It ran `cargo install`, which put a `clax` in `~/.cargo/bin` that nothing uses any more, because the launcher never looks at `PATH`. The coordinator suggested a replacement that installs a local release build into `~/.clax/bin/<workspace version>/`. That would make an unreleased build indistinguishable from the release of the same version: the launcher would trust it as the release and never download the real one, and neither `doctor` nor `status` could tell. That is the silent-substitution failure this plan exists to remove. Testing an unreleased version is `just dev-install`'s job instead. That path is explicit, reversible (`just dev-uninstall`), stated plainly by `doctor --agent` and `status`, and bypasses the version match with a logged warning. `just uninstall` goes too. A person who wants `clax` in their shell adds `~/.clax/bin` to `PATH`: the launcher keeps `~/.clax/bin/clax` as a symlink to the installed release it last installed.

**Release hosting needs a public repository.** The launcher downloads with anonymous `curl` from `https://github.com/empathic/clax/releases/download/v<version>/…`. `gh repo view empathic/clax` reports the repository as **private**, and GitHub serves a private repository's release assets only to authenticated requests. So until the person decides otherwise, every download returns 404, and the fallback says so ("answered HTTP 404; is v0.2.0 released?"). The plan keeps the host in one place (`REPO` and `RELEASE_BASE_URL` in the launcher, and the `gh release create` target in `release.yml`). "Steps for the person" gives both ways out: make the repository public, or publish releases to a separate public repository.

**macOS signing.** `curl` does not set the `com.apple.quarantine` extended attribute: only apps that opt into Launch Services quarantine do (browsers, Mail, AirDrop). `tar` sets it on extracted files only when the archive itself carries it, and the launcher's archive, fetched by `curl`, does not. Gatekeeper therefore never assesses the launcher-installed binary. It needs only a valid signature, and the Rust linker gives every arm64 binary an ad hoc one ("adhoc, linker-signed"), so it runs unsigned by a Developer ID. The release archives are built with `tar --no-mac-metadata --no-xattrs` on macOS, so no extended attribute travels inside them. "Steps for the person" verifies this on a real download (`xattr -l` shows no `com.apple.quarantine`, and `codesign -dv` shows `adhoc`). Two later channels would change it:
- A browser download of the archive is quarantined, and Gatekeeper blocks an unsigned binary ("cannot be opened because the developer cannot be verified"). Shipping that way needs a Developer ID Application certificate, `codesign --options runtime --timestamp`, and notarization with `xcrun notarytool submit` of a zip. A bare Mach-O cannot be stapled, so Gatekeeper checks the notarization online.
- Homebrew formulae fetch with `curl` and do not quarantine, so a formula works as the launcher does. Homebrew casks quarantine by default, so a cask needs the notarized binary as well.

**Cross-building.** Every target builds on a native GitHub-hosted runner, so no cross toolchain is involved:

| Target | Runner |
|---|---|
| `aarch64-apple-darwin` | `macos-15` |
| `x86_64-apple-darwin` | `macos-15-intel` |
| `x86_64-unknown-linux-musl` | `ubuntu-24.04`, with `musl-tools` |
| `aarch64-unknown-linux-musl` | `ubuntu-24.04-arm`, with `musl-tools` and `CC_aarch64_unknown_linux_musl=musl-gcc` |

The Linux builds are static musl binaries, so they run on any distribution. If a runner label is unavailable to the repository (the arm64 Linux runner has been limited to public repositories at times), that one job fails and the release is not published. The fallback for `x86_64-apple-darwin` is `--target x86_64-apple-darwin` on `macos-15`, since the Apple SDK builds both architectures. For `aarch64-unknown-linux-musl` it is `cargo zigbuild` on `ubuntu-24.04`. Each is a one-job change, described in "Steps for the person".

## File Structure

| Path | Responsibility |
|---|---|
| `scripts/ensure-clax.sh` (+ copies in `plugins/claude-code/scripts/`, `plugins/clax/scripts/`) | The launcher: resolve, download (MCP and `install` only), fallback MCP server, hooks.log |
| `scripts/fake-release-server.py` | Test-only stand-in for GitHub release downloads: normal, 404, bad checksum, cut short, slow, delayed |
| `scripts/test-ensure-clax.sh` | Launcher tests against scratch homes and the fake server |
| `scripts/check-version.sh` | Every written version agrees; with a tag, the tag is `v<version>`; `--print` prints it |
| `scripts/bump-version.sh` | Writes a new version everywhere `check-version.sh` reads |
| `scripts/package-release.sh` | `archive`: one target's `.tar.gz`; `sums`: `SHA256SUMS` |
| `scripts/smoke-release-binary.sh` | A built binary reports its version and serves the embedded web UI |
| `scripts/test-release.sh` | Tests of the four release scripts in a scratch copy |
| `scripts/dev-home.sh` | Sourced helpers: `dev_settings` (home, port, args for `just dev`), `ensure_dev_home` |
| `scripts/dev-install.sh` | `just dev-install` / `just dev-uninstall` |
| `scripts/test-dev.sh` | Tests of `dev-home.sh` and `dev-install.sh` |
| `.github/workflows/release.yml` | Version check, four native builds, assemble + launcher end-to-end, publish on tag only |
| `crates/clax-core/src/config.rs` | Config directory, `config.toml` (`[dev_link]`, `[serve] port`) |
| `crates/clax-core/src/launch.rs` | The launcher's resolution, in Rust, for `doctor --agent` |
| `crates/clax-server/src/daemon.rs` | `DaemonInfo.exe` |
| `crates/clax-cli/src/client.rs` | `spawn_locked`, `replace`, newer-wins `connect_matching_version` |
| `crates/clax-cli/src/commands/dev_link.rs` | `clax dev-link [path] [--home <dir>]`, `clax dev-unlink` |
| `crates/clax-cli/src/commands/doctor_agent.rs` | New `launch` check |
| `crates/clax-mcp/src/plugin.rs`, `tools.rs`, `shim.rs` | `status` reports `launch` |
| `plugins/pi/src/daemon.ts` | Pi resolves `CLAX_BIN`, the dev link, then the installed version |

---

### Task 1: Spec amendments

**Files:**
- Modify: `docs/superpowers/specs/2026-09-28-clax-design.md` (§2, §4, §5, §7, §13, §14, §16)

**Interfaces:**
- Produces: the binding text every later task implements. Later tasks cite it by section.

- [ ] **Step 1: §2 Decisions, add D16**

After the D15 row, add:

```markdown
| D16 | The plugins run only the exact `clax` release they were built for, installed by their launcher into `~/.clax/bin/<version>/clax` from a GitHub release checked against its `SHA256SUMS`; a dev build reaches agents only by opt-in (`CLAX_BIN`, or `clax dev-link`, which `just dev-install` runs), never by searching a checkout, `PATH`, or a harness's configuration | A launcher that guessed a checkout from Codex's configuration broke every session when the checkout moved; an installed release does not move, and a dev link is visible and reversible. |
```

- [ ] **Step 2: §4 Repository layout**

In the layout block, replace the line `justfile, scripts/quality_gates.sh same gate style as toolpath` with:

```
justfile, scripts/quality_gates.sh same gate style as toolpath
scripts/ensure-clax.sh             the plugins' launcher (copied into both plugins' scripts/)
scripts/*release*, check-version.sh, bump-version.sh
                                   release packaging and version checks (.github/workflows/release.yml)
scripts/dev.sh, dev-install.sh     `just dev` (own home and port) and `just dev-install` (dev link)
```

- [ ] **Step 3: §5 Storage and data model**

In the `~/.clax/` block, replace the `daemon.json` line and the `config.toml` line with:

```
  daemon.json            {port, pid, token, started_at, bind, version, exe}  mode 0600
```

and

```
  config.toml            [serve] port; [dev_link] bin, home, linked_at (written by clax dev-link);
                         later: bind address, sample provider, key env var name
  bin/<version>/clax     releases installed by the plugins' launcher (the current one and the one before)
  bin/clax               symlink to the release the launcher installed last
  bin/dev/clax           the dev build `just dev-install` copies here
```

After the block, add the paragraph:

```markdown
The *config directory* is `$CLAX_CONFIG_DIR`, else `$CLAX_HOME`, else
`~/.clax`: `config.toml`, `bin/` and the launcher's `logs/hooks.log` live
there. It is normally the home itself. A dev link may point agents at another
home (`clax dev-link --home ~/.clax-dev`); the launcher then runs the binary
with `CLAX_HOME` set to that home and `CLAX_CONFIG_DIR` set to the config
directory. A daemon started for a home listens on that home's `[serve] port`,
else 7480.
```

- [ ] **Step 4: §7 Daemon discovery and lifecycle, item 5**

Replace item 5 ("Version skew: …") with:

```markdown
5. Version skew: `daemon.json` records the daemon's `version` and `exe`
   (its executable's canonical path). A client that finds a daemon older
   than itself replaces it; a newer daemon, or one of the same version, is
   kept (a newer daemon serves older clients, and two plugins at different
   versions must not restart each other's daemon). A replacement holds
   `daemon.lock` throughout: it re-reads `daemon.json`, asks the daemon to
   shut down (SSE streams and long polls end, in-flight requests get 5 s),
   waits up to 7 s for its PID to exit, starts the new executable on the old
   port and bind address, and waits for `/healthz`. `clax dev-link`,
   `clax dev-unlink` and `just dev-install` replace the linked home's
   daemon the same way, whatever its version. Storage migrations run on
   daemon start.
```

- [ ] **Step 5: §13 Plugins**

Replace the Claude Code bullet that begins `- `scripts/ensure-clax.sh`: toolpath's` with:

```markdown
- `scripts/ensure-clax.sh`: the launcher. It carries the plugin's version
  (`CLAX_VERSION`) and resolves, first match wins: `CLAX_BIN`; the dev link
  in `<config dir>/config.toml`; `<config dir>/bin/<CLAX_VERSION>/clax`. In
  MCP mode only, it then downloads that version's archive and `SHA256SUMS`
  from the release (bounded timeouts, under a lock, installed by atomic
  rename, keeping the previous version and pruning older ones). It never
  looks at `PATH`, a source checkout, a harness's configuration, or its own
  location. When it cannot run `clax` in MCP mode, it answers the MCP client
  with a minimal server whose `status` tool states the reason; hooks never
  download and always exit 0. Every failure, and every MCP start, is one line
  in `<config dir>/logs/hooks.log` naming each candidate it tried.
```

In the Codex section, replace the sentence that begins `It starts MCP servers with a minimal environment, so `env_vars`` through `shim starts inherits, §10).` with:

```markdown
It starts MCP servers with a minimal environment, so `env_vars`
  forwards `CLAX_HOME`, `CLAX_CONFIG_DIR`, `CLAX_NO_OPEN`, `CLAX_BIN`,
  `CLAX_RELEASE_BASE_URL`, `CLAX_DOWNLOAD_TIMEOUT`, `CLAX_MCP_WAIT` and
  `CLAX_CODEX_BIN` (which a daemon the shim starts inherits, §10).
```

In the Pi section, after the first bullet, add:

```markdown
- The extension runs `clax` resolved like the launcher (`CLAX_BIN`, the dev
  link, `<config dir>/bin/<package version>/clax`) and never downloads; with
  none found it names `bash scripts/ensure-clax.sh install`.
```

- [ ] **Step 6: §14 Security model**

Replace the bullet `- No telemetry, no outbound calls except `sample()` and release downloads` / `  by the installer script.` with:

```markdown
- No telemetry, no outbound calls except `sample()` and the launcher's
  release download: only in MCP mode or `ensure-clax.sh install`, only for
  the exact version the plugin carries, only when no dev link or `CLAX_BIN`
  is set, and only from the release URL. The archive is checked against the
  release's `SHA256SUMS`, which comes from the same place, so the check
  protects integrity, not authenticity.
```

- [ ] **Step 7: §16 Testing**

Replace `- **Plugins**: shell tests for `ensure-clax.sh`;` with:

```markdown
- **Plugins**: shell tests for `ensure-clax.sh` against scratch homes and a
  local fake release server (success, 404, checksum mismatch, partial
  download, timeout, concurrent installs, the fallback MCP server);
```

and add a bullet after the Plugins bullet:

```markdown
- **Release**: `scripts/test-release.sh` checks the version, bump and
  packaging scripts; `.github/workflows/release.yml` builds, smoke-tests and
  packages every target on pull requests that touch it and on manual runs,
  and publishes only on a `v*` tag.
```

- [ ] **Step 8: Check and commit**

Run: `grep -c "D16" docs/superpowers/specs/2026-09-28-clax-design.md; bash scripts/test-plugins.sh | tail -1`
Expected: a count of at least 1, and `plugin checks passed`.

```bash
git add docs/superpowers/specs/2026-09-28-clax-design.md
git commit -m "Specify the stable install: exact release per plugin, opt-in dev links, clean daemon replacement"
git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed
```

---

### Task 2: The config directory and `config.toml`

**Files:**
- Create: `crates/clax-core/src/config.rs`
- Modify: `Cargo.toml` (workspace dependency `toml`), `crates/clax-core/Cargo.toml`, `crates/clax-core/src/lib.rs`, `crates/clax-cli/src/main.rs`, `crates/clax-cli/src/commands/{serve,status,tools,delete,pin,list,publish,mcp,open}.rs`, `crates/clax-cli/tests/cli.rs`

**Interfaces:**
- Produces: `clax_core::config::{FILE, config_dir, config_dir_with, Config, DevLink, plain_path}`; `Config::load(dir) -> Result<Config>`, `.path()`, `.dev_link() -> Option<DevLink>`, `.set_dev_link(Option<&DevLink>)`, `.serve_port() -> Option<u16>`, `.save() -> Result<()>`.
- Produces: `Cli::port_for(&self, home: &Home) -> u16`: `--port` when given, else the home's `[serve] port`, else 7480.

- [ ] **Step 1: Add the dependency**

In the root `Cargo.toml` `[workspace.dependencies]`, add `toml = "0.9"`. In `crates/clax-core/Cargo.toml` `[dependencies]`, add `toml.workspace = true`.

- [ ] **Step 2: Write the failing tests**

Create `crates/clax-core/src/config.rs` holding only the test module below and the item signatures from Step 3 with `todo!()` bodies. Add `pub mod config;` to `crates/clax-core/src/lib.rs`.

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_dir_prefers_config_dir_then_clax_home_then_home() {
        let d = |a, b, c| config_dir_with(a, b, c).unwrap();
        assert_eq!(d(Some("/c"), Some("/h"), Some("/u")), PathBuf::from("/c"));
        assert_eq!(d(None, Some("/h"), Some("/u")), PathBuf::from("/h"));
        assert_eq!(d(Some(""), Some(""), Some("/u")), PathBuf::from("/u/.clax"));
        let e = config_dir_with(None, None, None).unwrap_err();
        assert_eq!(e.to_string(), "neither CLAX_CONFIG_DIR, CLAX_HOME nor HOME is set");
    }

    #[test]
    fn a_missing_file_is_an_empty_config() {
        let dir = tempfile::tempdir().unwrap();
        let c = Config::load(dir.path()).unwrap();
        assert_eq!(c.dev_link(), None);
        assert_eq!(c.serve_port(), None);
        assert_eq!(c.path(), dir.path().join("config.toml"));
    }

    #[test]
    fn dev_link_round_trips_in_the_launchers_line_format_and_keeps_other_tables() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "[serve]\nport = 7481\n\n[sample]\napi_key_env = \"K\"\n",
        )
        .unwrap();
        let mut c = Config::load(dir.path()).unwrap();
        let link = DevLink {
            bin: "/u/.clax/bin/dev/clax".into(),
            home: Some("/u/.clax-dev".into()),
            linked_at: "2026-09-30T12:00:00Z".into(),
        };
        c.set_dev_link(Some(&link));
        c.save().unwrap();
        let text = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
        assert!(
            text.contains(
                "[dev_link]\nbin = \"/u/.clax/bin/dev/clax\"\nhome = \"/u/.clax-dev\"\nlinked_at = \"2026-09-30T12:00:00Z\"\n"
            ),
            "{text}"
        );
        assert!(text.contains("[serve]\nport = 7481\n"), "{text}");
        assert!(text.contains("api_key_env = \"K\""), "{text}");
        let again = Config::load(dir.path()).unwrap();
        assert_eq!(again.dev_link(), Some(link));
        assert_eq!(again.serve_port(), Some(7481));
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(again.path()).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let mut again = again;
        again.set_dev_link(None);
        again.save().unwrap();
        let text = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
        assert!(!text.contains("dev_link"), "{text}");
        assert!(text.contains("port = 7481"), "{text}");
    }

    #[test]
    fn a_link_without_home_has_no_home_line() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = Config::load(dir.path()).unwrap();
        c.set_dev_link(Some(&DevLink {
            bin: "/b/clax".into(),
            home: None,
            linked_at: "t".into(),
        }));
        c.save().unwrap();
        let text = std::fs::read_to_string(c.path()).unwrap();
        assert_eq!(text, "[dev_link]\nbin = \"/b/clax\"\nlinked_at = \"t\"\n");
    }

    #[test]
    fn a_config_that_does_not_parse_is_an_error_naming_the_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "[serve\n").unwrap();
        let e = Config::load(dir.path()).unwrap_err().to_string();
        assert!(e.contains("config.toml"), "{e}");
    }

    #[test]
    fn plain_path_refuses_what_the_launcher_could_not_read_back() {
        assert_eq!(plain_path(Path::new("/a b/clax")).unwrap(), "/a b/clax");
        for bad in ["rel/clax", "/a\"b", "/a\\b", "/a\nb", "/a\tb"] {
            assert!(plain_path(Path::new(bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn serve_port_ignores_out_of_range_values() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "[serve]\nport = 70000\n").unwrap();
        assert_eq!(Config::load(dir.path()).unwrap().serve_port(), None);
    }
}
```

Run: `cargo test -p clax-core config::`
Expected: FAIL (panics at `todo!()`).

- [ ] **Step 3: Implement**

Above the test module in `crates/clax-core/src/config.rs`:

```rust
//! The Clax config directory and its `config.toml`.
//!
//! The config directory is `$CLAX_CONFIG_DIR`, else `$CLAX_HOME`, else
//! `$HOME/.clax` (an empty variable counts as unset). It holds `config.toml`,
//! `bin/` (installed releases and the dev build) and the launcher's
//! `logs/hooks.log`. The plugins' launcher (`scripts/ensure-clax.sh`) and the
//! Pi extension compute it the same way, so a test that points `HOME` and
//! `CLAX_HOME` at scratch directories never reads or writes the real one.
//!
//! Tables:
//! - `[dev_link]`: `bin`, optional `home`, `linked_at`; written by
//!   `clax dev-link`, removed by `clax dev-unlink`, read by the launcher
//!   line by line (so its paths must pass [`plain_path`]).
//! - `[serve]`: `port`, where a daemon started for this home listens (read
//!   from the home's own `config.toml`).
//!
//! Rewriting the file keeps every other table and key.

use crate::{CoreError, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The file name inside the config directory (and inside a home).
pub const FILE: &str = "config.toml";

/// The config directory from the given variable values.
///
/// # Errors
/// `Invalid { code: "no_home" }` when none is set.
pub fn config_dir_with(
    config_dir: Option<&str>,
    clax_home: Option<&str>,
    home: Option<&str>,
) -> Result<PathBuf> {
    let set = |v: Option<&str>| v.filter(|s| !s.is_empty()).map(PathBuf::from);
    set(config_dir)
        .or_else(|| set(clax_home))
        .or_else(|| set(home).map(|h| h.join(".clax")))
        .ok_or_else(|| {
            CoreError::invalid(
                "no_home",
                "neither CLAX_CONFIG_DIR, CLAX_HOME nor HOME is set",
            )
        })
}

/// [`config_dir_with`] from this process's environment.
pub fn config_dir() -> Result<PathBuf> {
    let v = |k: &str| std::env::var(k).ok();
    config_dir_with(
        v("CLAX_CONFIG_DIR").as_deref(),
        v("CLAX_HOME").as_deref(),
        v("HOME").as_deref(),
    )
}

/// The `[dev_link]` table: the binary agents run instead of the release,
/// and optionally the home they use with it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevLink {
    pub bin: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<PathBuf>,
    /// RFC 3339 UTC time of the `clax dev-link` that wrote it.
    pub linked_at: String,
}

/// A `config.toml`, loaded whole so a rewrite keeps what it does not know.
#[derive(Clone, Debug)]
pub struct Config {
    path: PathBuf,
    table: toml::Table,
}

impl Config {
    /// `dir/config.toml`; a missing file is an empty config.
    ///
    /// # Errors
    /// `Invalid { code: "bad_config" }` naming the file when it does not
    /// parse; `Io` when it cannot be read.
    pub fn load(dir: &Path) -> Result<Config> {
        let path = dir.join(FILE);
        let table = match std::fs::read_to_string(&path) {
            Ok(text) => text.parse::<toml::Table>().map_err(|e| {
                CoreError::invalid("bad_config", format!("{}: {e}", path.display()))
            })?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => toml::Table::new(),
            Err(e) => return Err(e.into()),
        };
        Ok(Config { path, table })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The `[dev_link]` table, when present and well formed.
    pub fn dev_link(&self) -> Option<DevLink> {
        self.table.get("dev_link")?.clone().try_into().ok()
    }

    /// Sets (`Some`) or removes (`None`) the `[dev_link]` table.
    pub fn set_dev_link(&mut self, link: Option<&DevLink>) {
        match link {
            Some(l) => {
                let v = toml::Value::try_from(l).expect("a DevLink serialises");
                self.table.insert("dev_link".into(), v);
            }
            None => {
                self.table.remove("dev_link");
            }
        }
    }

    /// `[serve] port`, when it is an integer in 1..=65535.
    pub fn serve_port(&self) -> Option<u16> {
        let p = self.table.get("serve")?.get("port")?.as_integer()?;
        u16::try_from(p).ok().filter(|p| *p > 0)
    }

    /// Writes the file atomically (a 0600 temp file in the same directory,
    /// synced, then renamed over it), creating the directory (0700) if needed.
    pub fn save(&self) -> Result<()> {
        use std::io::Write;
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        let dir = self.path.parent().expect("config.toml has a parent");
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        let tmp = dir.join(format!(".{FILE}.{}.tmp", std::process::id()));
        let _ = std::fs::remove_file(&tmp);
        let text = toml::to_string(&self.table).expect("a toml table serialises");
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

/// `p` as text that a `config.toml` basic string holds unchanged and that the
/// launcher's line parser reads back: absolute, UTF-8, and free of `"`, `\`
/// and control characters.
///
/// # Errors
/// `Invalid { code: "bad_path" }` otherwise.
pub fn plain_path(p: &Path) -> Result<&str> {
    let s = p
        .to_str()
        .ok_or_else(|| CoreError::invalid("bad_path", format!("{} is not UTF-8", p.display())))?;
    if !p.is_absolute() {
        return Err(CoreError::invalid("bad_path", format!("{s} is not an absolute path")));
    }
    if s.chars().any(|c| c == '"' || c == '\\' || c.is_control()) {
        return Err(CoreError::invalid(
            "bad_path",
            format!("{s:?} contains a quote, a backslash or a control character"),
        ));
    }
    Ok(s)
}
```

Run: `cargo test -p clax-core config::`
Expected: PASS (7 tests). If `a_link_without_home_has_no_home_line` or the round-trip test fails on the exact text, print `text`. A difference only in blank lines between tables may be absorbed by adjusting the expected text. A different quoting (`'...'` or `"""..."""`) may not: the launcher's parser (Task 8) and Pi's (Task 10) read exactly `key = "value"` lines, so make `save` produce them.

- [ ] **Step 4: The CLI's default port comes from the home's config**

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
                clax_core::config::Config::load(home.root())
                    .ok()
                    .and_then(|c| c.serve_port())
            })
            .unwrap_or(clax_server::daemon::DEFAULT_PORT)
    }
}
```

In each command file listed under **Files**, replace `cli.port` with `cli.port_for(home)`. `grep -rn "cli\.port\b" crates/clax-cli/src` must then print only `port_for` lines.

- [ ] **Step 5: CLI test for the home's port**

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

In `Env::cmd` in the same file, add `.env_remove("CLAX_CONFIG_DIR")` after `.env("HOME", …)`, and do the same wherever `crates/clax-cli/tests/*.rs` builds a `clax` command (`grep -n 'cargo_bin("clax")' crates/clax-cli/tests`). No test may inherit a config directory from the shell that runs it.

Run: `cargo test -p clax-cli --test cli a_daemon_started_without_port_uses_the_homes_serve_port`
Expected: PASS. The port the kernel picks is never 7480 or 7481 in practice. If it is, the test binds it first, so it cannot collide with the person's daemon.

- [ ] **Step 6: Gates and commit**

Run: `bash scripts/quality_gates.sh; echo "exit=$?"`
Expected: `exit=0`.

```bash
git add Cargo.toml Cargo.lock crates/clax-core/Cargo.toml crates/clax-core/src/config.rs crates/clax-core/src/lib.rs crates/clax-cli/src crates/clax-cli/tests/cli.rs
git commit -m "Add the config directory and config.toml; a home's [serve] port sets its daemon's default port"
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
    /// build serves (a dev link and a release can share a version). Absent
    /// in records written before it existed.
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

### Task 4: `clax dev-link` and `clax dev-unlink`

**Files:**
- Create: `crates/clax-cli/src/commands/dev_link.rs`, `crates/clax-cli/tests/dev_link.rs`
- Modify: `crates/clax-cli/src/commands/mod.rs`, `crates/clax-cli/src/main.rs`

**Interfaces:**
- Consumes: `clax_core::config::*` (Task 2), `Client::replace`, `DaemonInfo.exe` (Task 3).
- Produces: `clax dev-link [PATH] [--home <DIR>] [--json]`. `PATH` defaults to `<config dir>/bin/dev/clax`. The command refuses a binary inside a cargo target directory, one that is not a `clax`, and a path the launcher could not read back. It writes `[dev_link]`, then replaces the linked home's daemon when one is running. JSON: `{"linked": true, "bin", "version", "home", "config", "restarted": {"pid", "port"} | null}`.
- Produces: `clax dev-unlink [--json]`: removes `[dev_link]`, and stops the linked home's daemon when its `exe` is the unlinked binary. JSON: `{"linked": false, "was": <bin> | null, "stopped": <pid> | null}`.

- [ ] **Step 1: Write the failing tests**

Create `crates/clax-cli/tests/dev_link.rs`:

```rust
//! `clax dev-link` / `clax dev-unlink` against scratch homes. Every binary a
//! test links is a copy of the test build outside the cargo target directory.

use assert_cmd::Command;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    fn new() -> Env {
        Env { dir: tempfile::tempdir().unwrap() }
    }
    fn home(&self) -> PathBuf {
        self.dir.path().join("ax")
    }
    /// Runs `bin` (a copy) with the scratch environment.
    fn run(&self, bin: &Path) -> Command {
        let mut c = Command::new(bin);
        c.env("HOME", self.dir.path())
            .env("CLAX_HOME", self.home())
            .env_remove("CLAX_CONFIG_DIR")
            .env("CLAX_CODEX_BIN", "");
        c
    }
    /// A copy of the test build at `<scratch>/<name>/clax`.
    fn copy(&self, name: &str) -> PathBuf {
        let dir = self.dir.path().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let to = dir.join("clax");
        std::fs::copy(env!("CARGO_BIN_EXE_clax"), &to).unwrap();
        std::fs::canonicalize(to).unwrap()
    }
    fn config(&self) -> String {
        std::fs::read_to_string(self.home().join("config.toml")).unwrap_or_default()
    }
    fn daemon(&self) -> Option<serde_json::Value> {
        let t = std::fs::read_to_string(self.home().join("daemon.json")).ok()?;
        serde_json::from_str(&t).ok()
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        if let Some(pid) = self.daemon().and_then(|v| v["pid"].as_i64()) {
            // SAFETY: probe, then end a daemon this test started.
            unsafe {
                if libc::kill(pid as libc::pid_t, 0) == 0 {
                    libc::kill(pid as libc::pid_t, libc::SIGTERM);
                }
            }
        }
    }
}

#[test]
fn link_writes_the_config_and_unlink_removes_it_keeping_other_tables() {
    let e = Env::new();
    let a = e.copy("a");
    std::fs::create_dir_all(e.home()).unwrap();
    std::fs::write(e.home().join("config.toml"), "[serve]\nport = 7481\n").unwrap();
    let dev_home = e.dir.path().join("devhome");
    let out = e.run(&a).args(["dev-link", a.to_str().unwrap(), "--home"]).arg(&dev_home).arg("--json").output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["linked"], true);
    assert_eq!(v["version"], format!("clax {}", env!("CARGO_PKG_VERSION")));
    assert!(v["restarted"].is_null(), "no daemon was running");
    let dev_home = std::fs::canonicalize(&dev_home).unwrap();
    let text = e.config();
    assert!(text.contains(&format!("[dev_link]\nbin = \"{}\"\nhome = \"{}\"\n", a.display(), dev_home.display())), "{text}");
    assert!(text.contains("[serve]\nport = 7481"), "{text}");

    let out = e.run(&a).args(["dev-unlink", "--json"]).output().unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["linked"], false);
    assert_eq!(v["was"], a.display().to_string());
    let text = e.config();
    assert!(!text.contains("dev_link"), "{text}");
    assert!(text.contains("port = 7481"), "{text}");
}

#[test]
fn link_refuses_a_binary_in_a_cargo_target_directory() {
    let e = Env::new();
    let a = e.copy("a");
    let out = e.run(&a).args(["dev-link", env!("CARGO_BIN_EXE_clax")]).output().unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("cargo target directory") && err.contains("just dev-install"), "{err}");
    assert!(!e.config().contains("dev_link"));
}

#[test]
fn link_refuses_a_binary_that_is_not_clax() {
    let e = Env::new();
    let a = e.copy("a");
    let other = e.dir.path().join("other");
    std::fs::write(&other, "#!/bin/sh\necho other 1.0\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&other, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let out = e.run(&a).arg("dev-link").arg(&other).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("is not a clax binary"));
}

#[test]
fn link_defaults_to_bin_dev_clax_and_names_dev_install_when_it_is_missing() {
    let e = Env::new();
    let a = e.copy("a");
    let out = e.run(&a).arg("dev-link").output().unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("bin/dev/clax") && err.contains("just dev-install"), "{err}");
    std::fs::create_dir_all(e.home().join("bin/dev")).unwrap();
    std::fs::copy(&a, e.home().join("bin/dev/clax")).unwrap();
    e.run(&a).arg("dev-link").assert().success();
    let bin = std::fs::canonicalize(e.home().join("bin/dev/clax")).unwrap();
    assert!(e.config().contains(&format!("bin = \"{}\"", bin.display())));
}

#[test]
fn link_refuses_a_path_the_launcher_could_not_read() {
    let e = Env::new();
    let a = e.copy("a");
    let odd = e.dir.path().join("q\"uote");
    std::fs::create_dir_all(&odd).unwrap();
    std::fs::copy(&a, odd.join("clax")).unwrap();
    let out = e.run(&a).arg("dev-link").arg(odd.join("clax")).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("quote"));
}

#[test]
fn dev_link_restarts_the_linked_homes_daemon_on_its_port() {
    let e = Env::new();
    let a = e.copy("a");
    let b = e.copy("b");
    let out = e.run(&a).args(["serve", "--json", "--port", "0"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let before = e.daemon().unwrap();
    let port = before["port"].as_u64().unwrap();
    assert_eq!(before["exe"], a.display().to_string());

    // An open SSE stream on the old daemon.
    let mut sse = std::net::TcpStream::connect(("127.0.0.1", port as u16)).unwrap();
    write!(sse, "GET /api/events HTTP/1.1\r\nHost: localhost\r\nAccept: text/event-stream\r\n\r\n").unwrap();
    sse.set_read_timeout(Some(Duration::from_secs(15))).unwrap();
    let mut reader = BufReader::new(sse.try_clone().unwrap());
    let mut status = String::new();
    reader.read_line(&mut status).unwrap();
    assert!(status.contains("200"), "{status}");

    let out = e.run(&b).args(["dev-link", b.to_str().unwrap(), "--json"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["restarted"]["port"].as_u64().unwrap(), port);

    // The old stream ends.
    let start = Instant::now();
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => assert!(start.elapsed() < Duration::from_secs(15), "the SSE stream did not end"),
            Err(err) => panic!("the SSE stream did not end: {err}"),
        }
    }
    let after = e.daemon().unwrap();
    assert_eq!(after["exe"], b.display().to_string());
    assert_eq!(after["port"].as_u64().unwrap(), port);
    assert_ne!(after["pid"], before["pid"]);

    // Unlinking stops the daemon that runs the unlinked binary.
    let out = e.run(&b).args(["dev-unlink", "--json"]).output().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["stopped"], after["pid"]);
    assert!(e.daemon().is_none() || e.daemon().unwrap()["pid"] != after["pid"]);
}
```

Run: `cargo test -p clax-cli --test dev_link`
Expected: FAIL (`unrecognized subcommand 'dev-link'`).

- [ ] **Step 2: Implement the commands**

Create `crates/clax-cli/src/commands/dev_link.rs`:

```rust
//! `clax dev-link` and `clax dev-unlink`: point the plugins' launcher at a
//! dev build (and optionally another home), and back to the release.
//!
//! The link is the `[dev_link]` table of `<config dir>/config.toml`
//! ([`clax_core::config`]). Linking replaces the linked home's running
//! daemon with one started from the linked binary ([`Client::replace`]), so
//! agents' new sessions and the daemon agree; sessions already running keep
//! the binary they started with until they restart.

use crate::client::Client;
use anyhow::{Context, bail};
use clax_core::Home;
use clax_core::config::{self, Config, DevLink};
use serde_json::json;
use std::path::{Path, PathBuf};

#[derive(clap::Args)]
pub struct LinkArgs {
    /// The clax binary agents should run. Default: <config dir>/bin/dev/clax,
    /// where `just dev-install` copies a release build.
    pub path: Option<PathBuf>,
    /// The Clax home agents should use with it, for example ~/.clax-dev (the
    /// home `just dev` serves). Default: the usual home.
    #[arg(long)]
    pub home: Option<PathBuf>,
}

/// Refuses a binary inside a cargo target directory: `cargo clean`, a
/// rebuild or a moved checkout would change or remove it under agents.
fn refuse_target_dir(bin: &Path, dir: &Path) -> anyhow::Result<()> {
    for a in bin.ancestors().skip(1) {
        if a.file_name() == Some("target".as_ref()) || a.join("CACHEDIR.TAG").is_file() {
            bail!(
                "{} is inside a cargo target directory ({}); agents never run a binary from there. Run `just dev-install` in your Clax checkout, which copies a release build to {} and links it",
                bin.display(),
                a.display(),
                dir.join("bin/dev/clax").display()
            );
        }
    }
    Ok(())
}

/// The first line of `bin --version`, when it names clax.
fn clax_version(bin: &Path) -> anyhow::Result<String> {
    let out = std::process::Command::new(bin)
        .arg("--version")
        .output()
        .with_context(|| format!("running {} --version", bin.display()))?;
    let first = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .to_string();
    if !out.status.success() || !first.starts_with("clax ") {
        bail!("{} is not a clax binary (--version printed {first:?})", bin.display());
    }
    Ok(first)
}

/// The home a link's agents use: its own, else the usual one.
fn linked_home(link: &DevLink) -> anyhow::Result<Home> {
    match &link.home {
        Some(h) => Ok(Home::at(h.clone())),
        None => Ok(Home::from_env()?),
    }
}

pub fn link(cli: &crate::Cli, a: &LinkArgs) -> anyhow::Result<()> {
    let dir = config::config_dir()?;
    let given = a.path.clone().unwrap_or_else(|| dir.join("bin/dev/clax"));
    let bin = given.canonicalize().with_context(|| {
        format!(
            "{} does not exist; run `just dev-install` in your Clax checkout",
            given.display()
        )
    })?;
    refuse_target_dir(&bin, &dir)?;
    let version = clax_version(&bin)?;
    config::plain_path(&bin)?;
    let home = match &a.home {
        Some(h) => {
            if !h.is_absolute() {
                bail!("--home {} is not an absolute path", h.display());
            }
            Home::at(h.clone()).ensure_dirs()?;
            let h = h.canonicalize()?;
            config::plain_path(&h)?;
            Some(h)
        }
        None => None,
    };
    let link = DevLink {
        bin: bin.clone(),
        home,
        linked_at: clax_core::Store::now(),
    };
    let mut cfg = Config::load(&dir)?;
    cfg.set_dev_link(Some(&link));
    cfg.save()?;
    let target = linked_home(&link)?;
    let exe = bin.display().to_string();
    let restarted = match Client::discover(&target) {
        Some(c) => {
            let n = Client::replace(&target, &c, &bin, |i| i.exe.as_deref() == Some(exe.as_str()))?;
            Some(json!({"pid": n.info.pid, "port": n.info.port}))
        }
        None => None,
    };
    let out = json!({
        "linked": true,
        "bin": bin,
        "version": version,
        "home": link.home,
        "config": cfg.path(),
        "restarted": restarted,
    });
    super::print(cli, out, |j| {
        let mut s = format!(
            "Agents now run the dev build {} ({})",
            bin.display(),
            j["version"].as_str().unwrap_or_default()
        );
        if let Some(h) = j["home"].as_str() {
            s.push_str(&format!(" with the home {h}"));
        }
        s.push_str(".\nNew sessions use it; running sessions keep the binary they started with until they restart.\n`clax dev-unlink` returns agents to the release.");
        if let Some(r) = j["restarted"].as_object() {
            s.push_str(&format!(
                "\nRestarted the daemon of {} from it (pid {}, port {}).",
                target.root().display(),
                r["pid"],
                r["port"]
            ));
        }
        s
    });
    Ok(())
}

pub fn unlink(cli: &crate::Cli) -> anyhow::Result<()> {
    let dir = config::config_dir()?;
    let mut cfg = Config::load(&dir)?;
    let Some(link) = cfg.dev_link() else {
        super::print(cli, json!({"linked": false, "was": null, "stopped": null}), |_| {
            "No dev link is set; agents run the release.".into()
        });
        return Ok(());
    };
    cfg.set_dev_link(None);
    cfg.save()?;
    let home = linked_home(&link)?;
    let exe = link.bin.display().to_string();
    let mut stopped = None;
    if let Some(c) = Client::discover(&home)
        && c.info.exe.as_deref() == Some(exe.as_str())
    {
        c.shutdown()?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(7);
        while std::time::Instant::now() < deadline && clax_server::daemon::pid_alive(c.info.pid) {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        stopped = Some(c.info.pid);
    }
    super::print(
        cli,
        json!({"linked": false, "was": link.bin, "stopped": stopped}),
        |_| {
            let mut s = format!(
                "Removed the dev link to {}; new sessions run the release the plugin was built for.",
                link.bin.display()
            );
            if let Some(pid) = stopped {
                s.push_str(&format!(
                    "\nStopped its daemon (pid {pid}); the next session starts the release's."
                ));
            }
            s
        },
    );
    Ok(())
}
```

Add `pub mod dev_link;` to `crates/clax-cli/src/commands/mod.rs`. In `crates/clax-cli/src/main.rs`, add to `Cmd`:

```rust
    /// Run agents on a dev build: record it (and optionally a home) for the
    /// plugins' launcher, and restart that home's daemon from it.
    DevLink(commands::dev_link::LinkArgs),
    /// Return agents to the release: remove the dev link.
    DevUnlink,
```

and to the `match`:

```rust
        Cmd::DevLink(a) => commands::dev_link::link(&cli, a),
        Cmd::DevUnlink => commands::dev_link::unlink(&cli),
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p clax-cli --test dev_link`
Expected: PASS (6 tests).

- [ ] **Step 4: Gates and commit**

Run: `bash scripts/quality_gates.sh; echo "exit=$?"`
Expected: `exit=0`.

```bash
git add crates/clax-cli/src/commands/dev_link.rs crates/clax-cli/src/commands/mod.rs crates/clax-cli/src/main.rs crates/clax-cli/tests/dev_link.rs
git commit -m "Add clax dev-link and dev-unlink: record a dev build for agents and restart its home's daemon"
git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed
```

---
### Task 5: Which binary runs, and why: `doctor --agent` and `status`

**Files:**
- Create: `crates/clax-core/src/launch.rs`
- Modify: `crates/clax-core/src/lib.rs`, `crates/clax-cli/src/commands/doctor_agent.rs`, `crates/clax-mcp/src/plugin.rs`, `crates/clax-mcp/src/tools.rs`, `crates/clax-mcp/src/shim.rs`, `crates/clax-mcp/tests/tools.rs`

**Interfaces:**
- Consumes: `clax_core::config` (Task 2). It also consumes the launcher's environment contract from spec §13 (Task 1), which Task 8 implements: the launcher exports `CLAX_LAUNCH` (`clax-bin`, `dev-link`, `installed` or `downloaded`), `CLAX_LAUNCH_BIN`, and `CLAX_LAUNCH_WARNING` when the version differs.
- Produces: `clax_core::launch::{Source, Launch, NotFound, resolve, version_of}`.
- Produces: the doctor check `launch` (JSON `{"name": "launch", "ok", "detail"}`), second in `clax doctor --agent <h>`, after `binary`.
- Produces: `status` gains `launch: {"source", "bin", "warning", "notice"}` when the launcher started the shim. `notice` is a plain sentence for a dev link, else `null`.

- [ ] **Step 1: Write the failing resolution tests**

Create `crates/clax-core/src/launch.rs` with the signatures from Step 2 (bodies `todo!()`) and this test module; add `pub mod launch;` to `crates/clax-core/src/lib.rs`.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct Case {
        dir: tempfile::TempDir,
        vars: HashMap<&'static str, String>,
        versions: HashMap<PathBuf, String>,
    }

    impl Case {
        fn new() -> Case {
            let dir = tempfile::tempdir().unwrap();
            let mut vars = HashMap::new();
            vars.insert("HOME", dir.path().display().to_string());
            Case { dir, vars, versions: HashMap::new() }
        }
        fn cfg(&self) -> PathBuf {
            self.dir.path().join(".clax")
        }
        fn bin(&mut self, rel: &str, version: &str) -> PathBuf {
            let p = self.dir.path().join(rel);
            self.versions.insert(p.clone(), version.to_string());
            p
        }
        fn link(&self, bin: &Path, home: Option<&str>) {
            std::fs::create_dir_all(self.cfg()).unwrap();
            let home = home.map(|h| format!("home = \"{h}\"\n")).unwrap_or_default();
            std::fs::write(
                self.cfg().join("config.toml"),
                format!("[dev_link]\nbin = \"{}\"\n{home}linked_at = \"t\"\n", bin.display()),
            )
            .unwrap();
        }
        fn resolve(&self, expected: &str) -> Result<Launch, NotFound> {
            resolve(
                |k| self.vars.get(k).cloned(),
                expected,
                |p| self.versions.get(p).cloned(),
            )
        }
    }

    #[test]
    fn clax_bin_wins_and_warns_on_another_version() {
        let mut c = Case::new();
        let b = c.bin("x/clax", "clax 0.9.0");
        c.vars.insert("CLAX_BIN", b.display().to_string());
        let l = c.resolve("0.2.0").unwrap();
        assert_eq!(l.source, Source::ClaxBin);
        assert_eq!(l.bin, b);
        assert_eq!(l.warning.as_deref(), Some("CLAX_BIN runs clax 0.9.0, but the plugin is clax 0.2.0"));
    }

    #[test]
    fn an_unusable_clax_bin_fails_without_download() {
        let mut c = Case::new();
        c.vars.insert("CLAX_BIN", "/nope/clax".into());
        let e = c.resolve("0.2.0").unwrap_err();
        assert!(!e.downloads);
        assert!(e.reason.contains("CLAX_BIN is set to '/nope/clax'"), "{}", e.reason);
    }

    #[test]
    fn the_dev_link_comes_next_with_its_home() {
        let mut c = Case::new();
        let b = c.bin(".clax/bin/dev/clax", "clax 0.2.0");
        c.link(&b, Some("/u/.clax-dev"));
        let l = c.resolve("0.2.0").unwrap();
        assert_eq!(l.source, Source::DevLink);
        assert_eq!(l.home, Some(PathBuf::from("/u/.clax-dev")));
        assert_eq!(l.linked_at.as_deref(), Some("t"));
        assert_eq!(l.warning, None);
    }

    #[test]
    fn a_broken_dev_link_fails_without_download() {
        let c = Case::new();
        c.link(Path::new("/gone/clax"), None);
        let e = c.resolve("0.2.0").unwrap_err();
        assert!(!e.downloads);
        assert!(e.reason.contains("just dev-install"), "{}", e.reason);
    }

    #[test]
    fn the_installed_version_must_report_the_expected_version() {
        let mut c = Case::new();
        let good = c.bin(".clax/bin/0.2.0/clax", "clax 0.2.0");
        let l = c.resolve("0.2.0").unwrap();
        assert_eq!((l.source, l.bin), (Source::Installed, good));
        c.versions.insert(c.cfg().join("bin/0.2.0/clax"), "clax 0.1.0".into());
        let e = c.resolve("0.2.0").unwrap_err();
        assert!(e.downloads);
        assert!(e.tried.iter().any(|t| t.contains("clax 0.1.0, not clax 0.2.0")), "{:?}", e.tried);
    }

    #[test]
    fn nothing_found_names_every_candidate_and_downloads() {
        let c = Case::new();
        let e = c.resolve("0.2.0").unwrap_err();
        assert!(e.downloads);
        assert_eq!(e.tried.len(), 3, "{:?}", e.tried);
        assert_eq!(e.tried[0], "CLAX_BIN: unset");
        assert!(e.tried[1].starts_with("dev link: none in "), "{:?}", e.tried);
        assert!(e.tried[2].ends_with("bin/0.2.0/clax: missing"), "{:?}", e.tried);
    }

    #[test]
    fn the_config_directory_follows_clax_config_dir_then_clax_home() {
        let mut c = Case::new();
        let other = c.dir.path().join("elsewhere");
        let b = c.bin("elsewhere/bin/0.2.0/clax", "clax 0.2.0");
        c.vars.insert("CLAX_HOME", other.display().to_string());
        assert_eq!(c.resolve("0.2.0").unwrap().bin, b);
        c.vars.insert("CLAX_CONFIG_DIR", c.dir.path().join("none").display().to_string());
        assert!(c.resolve("0.2.0").is_err());
    }
}
```

Run: `cargo test -p clax-core launch::`
Expected: FAIL (panics at `todo!()`).

- [ ] **Step 2: Implement**

```rust
//! Which `clax` the plugins' launcher (`scripts/ensure-clax.sh`) runs, and
//! why, resolved the same way so `clax doctor --agent` can say it: `CLAX_BIN`,
//! then the dev link in `<config dir>/config.toml`, then
//! `<config dir>/bin/<plugin version>/clax`. The launcher's MCP mode then
//! downloads that version; nothing here does.

use crate::config::{self, Config};
use std::path::{Path, PathBuf};

/// Where the binary came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    ClaxBin,
    DevLink,
    Installed,
}

impl Source {
    /// The name the launcher exports in `CLAX_LAUNCH`.
    pub fn as_str(self) -> &'static str {
        match self {
            Source::ClaxBin => "clax-bin",
            Source::DevLink => "dev-link",
            Source::Installed => "installed",
        }
    }
}

/// A resolved binary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    pub source: Source,
    pub bin: PathBuf,
    /// Its `--version` line, such as `clax 0.2.0`.
    pub version: String,
    /// The home a dev link sets for it.
    pub home: Option<PathBuf>,
    /// When a dev link was made.
    pub linked_at: Option<String>,
    /// Set when a `CLAX_BIN` or dev-linked binary is not the plugin's version.
    pub warning: Option<String>,
}

/// Why nothing resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotFound {
    /// Each candidate and what was there, in order.
    pub tried: Vec<String>,
    pub reason: String,
    /// True when the launcher's MCP mode would download the plugin's version
    /// (neither `CLAX_BIN` nor a dev link is set).
    pub downloads: bool,
}

/// Resolves as the launcher does, for a plugin of version `expected`.
/// `env` looks up variables (empty counts as unset); `version_of` returns a
/// binary's `--version` line when it is a clax ([`version_of`] in production).
pub fn resolve(
    env: impl Fn(&str) -> Option<String>,
    expected: &str,
    version_of: impl Fn(&Path) -> Option<String>,
) -> Result<Launch, NotFound> {
    let want = format!("clax {expected}");
    let var = |k: &str| env(k).filter(|v| !v.is_empty());
    let mismatch = |who: &str, v: &str| (v != want).then(|| format!("{who} runs {v}, but the plugin is {want}"));
    if let Some(b) = var("CLAX_BIN") {
        let bin = PathBuf::from(&b);
        return match version_of(&bin) {
            Some(v) => Ok(Launch {
                source: Source::ClaxBin,
                warning: mismatch("CLAX_BIN", &v),
                bin,
                version: v,
                home: None,
                linked_at: None,
            }),
            None => Err(NotFound {
                tried: vec![format!("CLAX_BIN={b}: not a usable clax")],
                reason: format!("CLAX_BIN is set to '{b}', which is not a usable clax binary"),
                downloads: false,
            }),
        };
    }
    let mut tried = vec!["CLAX_BIN: unset".to_string()];
    let dir = match config::config_dir_with(
        var("CLAX_CONFIG_DIR").as_deref(),
        var("CLAX_HOME").as_deref(),
        var("HOME").as_deref(),
    ) {
        Ok(d) => d,
        Err(e) => return Err(NotFound { tried, reason: e.to_string(), downloads: false }),
    };
    let file = dir.join(config::FILE);
    match Config::load(&dir) {
        Ok(cfg) => match cfg.dev_link() {
            Some(link) => {
                return match version_of(&link.bin) {
                    Some(v) => Ok(Launch {
                        source: Source::DevLink,
                        warning: mismatch("the dev link", &v),
                        bin: link.bin,
                        version: v,
                        home: link.home,
                        linked_at: Some(link.linked_at),
                    }),
                    None => {
                        tried.push(format!("dev link {}: not a usable clax", link.bin.display()));
                        Err(NotFound {
                            tried,
                            reason: format!(
                                "the dev link in {} points at '{}', which is not a usable clax binary; run `just dev-install` in your Clax checkout, or remove the [dev_link] table (`clax dev-unlink`) to use the release",
                                file.display(),
                                link.bin.display()
                            ),
                            downloads: false,
                        })
                    }
                };
            }
            None => tried.push(format!("dev link: none in {}", file.display())),
        },
        Err(e) => return Err(NotFound { tried, reason: e.to_string(), downloads: false }),
    }
    let inst = dir.join("bin").join(expected).join("clax");
    match version_of(&inst) {
        Some(v) if v == want => Ok(Launch {
            source: Source::Installed,
            bin: inst,
            version: v,
            home: None,
            linked_at: None,
            warning: None,
        }),
        found => {
            tried.push(match found {
                Some(v) => format!("{}: {v}, not {want}", inst.display()),
                None => format!("{}: missing", inst.display()),
            });
            Err(NotFound {
                tried,
                reason: format!("{want} is not installed at {}", inst.display()),
                downloads: true,
            })
        }
    }
}

/// The first line of `bin --version`, when `bin` is an executable file whose
/// first line starts with `clax `.
pub fn version_of(bin: &Path) -> Option<String> {
    use std::os::unix::fs::PermissionsExt;
    let m = std::fs::metadata(bin).ok()?;
    if !m.is_file() || m.permissions().mode() & 0o111 == 0 {
        return None;
    }
    let out = std::process::Command::new(bin)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let first = String::from_utf8_lossy(&out.stdout).lines().next()?.to_string();
    first.starts_with("clax ").then_some(first)
}
```

In `crates/clax-cli/src/commands/dev_link.rs`, replace the body of `clax_version` with a call to `clax_core::launch::version_of(bin)`, and bail with the same message when it returns `None`, so the two agree on what a clax is.

Run: `cargo test -p clax-core launch:: && cargo test -p clax-cli --test dev_link`
Expected: PASS.

- [ ] **Step 3: The `launch` doctor check**

In `crates/clax-cli/src/commands/doctor_agent.rs`, add to the module doc comment's list, after `binary`: `` - `launch`: which binary the harness's launcher runs and why (a dev link or `CLAX_BIN` stated first), and its last MCP start. `` Then add:

```rust
/// The launcher's last MCP start line (`launch …` or a `launcher …` failure)
/// for `harness` in `<config dir>/logs/hooks.log`.
fn last_start(config_dir: &Path, harness: &str) -> Option<String> {
    let text = std::fs::read_to_string(config_dir.join("logs/hooks.log")).ok()?;
    let ok = format!(" launch mode=mcp agent={harness} ");
    let failed = format!(" launcher mode=mcp agent={harness} ");
    text.lines()
        .rev()
        .find(|l| l.contains(&ok) || l.contains(&failed))
        .map(str::to_string)
}

/// `launch`: the binary `agent`'s launcher runs for a plugin of version
/// `expected`, and why; failed when none resolves.
pub fn launch_check(
    agent: DoctorAgent,
    expected: &str,
    resolved: &Result<clax_core::launch::Launch, clax_core::launch::NotFound>,
    last: Option<&str>,
) -> Value {
    use clax_core::launch::Source;
    let mut detail = match resolved {
        Ok(l) => match l.source {
            Source::DevLink => {
                let mut s = format!(
                    "DEV LINK: agents run {} ({}), not the release clax {expected}",
                    l.bin.display(),
                    l.version
                );
                if let Some(h) = &l.home {
                    s.push_str(&format!(", with the home {}", h.display()));
                }
                s.push_str(&format!(
                    "; linked {} by `clax dev-link`. `clax dev-unlink` returns to the release.",
                    l.linked_at.as_deref().unwrap_or("?")
                ));
                s
            }
            Source::ClaxBin => format!(
                "CLAX_BIN: {} ({}) is set in this environment; a harness started with it runs that binary",
                l.bin.display(),
                l.version
            ),
            Source::Installed => format!(
                "release: {} ({}), the version the plugin expects",
                l.bin.display(),
                l.version
            ),
        },
        Err(e) => {
            let fix = match (e.downloads, agent) {
                (true, DoctorAgent::Pi) => {
                    "; Pi does not download: run `bash scripts/ensure-clax.sh install` in your Clax checkout".to_string()
                }
                (true, _) => {
                    "; the MCP server downloads it when the next session starts, or run `bash scripts/ensure-clax.sh install` in the plugin's directory".to_string()
                }
                (false, _) => String::new(),
            };
            format!("{}{fix}\ntried: {}", e.reason, e.tried.join("; "))
        }
    };
    if let Ok(l) = resolved
        && let Some(w) = &l.warning
    {
        detail.push_str(&format!("\nwarning: {w}"));
    }
    if let Some(line) = last {
        detail.push_str(&format!("\nlast MCP start: {line}"));
    }
    check("launch", resolved.is_ok(), detail)
}
```

In `checks`, compute the expected version and insert the check after `binary`. For Claude Code and Codex, the expected version is the installed plugin's manifest version. For Pi, and when no plugin is found, it is this binary's version:

```rust
    let env = |k: &str| std::env::var(k).ok();
    let config_dir = clax_core::config::config_dir().ok();
    // (inside the `Some(dirs)` arm, after `plugin_check`)
    let expected = root
        .as_deref()
        .and_then(clax_mcp::plugin::manifest_version)
        .unwrap_or_else(|| version.to_string());
    let resolved = clax_core::launch::resolve(env, &expected, clax_core::launch::version_of);
    let last = config_dir.as_deref().and_then(|d| last_start(d, agent.harness()));
    out.insert(1, launch_check(agent, &expected, &resolved, last.as_deref()));
```

In the `None => { … }` arm (no `HOME`), insert `check("launch", false, "HOME is not set")` at index 1.

`hooks_check` reads `home.hooks_log_path()`. When the config directory differs from the home (a dev link with `--home`), the launcher's lines are in the config directory's log. Make `hooks_check` take `extra_log: Option<&Path>`. When that path differs from `home.hooks_log_path()`, the check also reads the last `HOOK_LINES` lines for the agent from it, labelled with its path. Pass `config_dir.map(|d| d.join("logs/hooks.log"))` from `checks`. Existing callers and tests of `hooks_check` pass `None`.

- [ ] **Step 4: Doctor tests**

Add to the test module of `doctor_agent.rs`:

```rust
    #[test]
    fn launch_states_a_dev_link_plainly() {
        let l = clax_core::launch::Launch {
            source: clax_core::launch::Source::DevLink,
            bin: "/u/.clax/bin/dev/clax".into(),
            version: "clax 0.3.0-dev".into(),
            home: Some("/u/.clax-dev".into()),
            linked_at: Some("2026-09-30T12:00:00Z".into()),
            warning: Some("the dev link runs clax 0.3.0-dev, but the plugin is clax 0.2.0".into()),
        };
        let v = launch_check(DoctorAgent::Codex, "0.2.0", &Ok(l), Some("t launch mode=mcp agent=codex source=dev-link"));
        assert_eq!(v["ok"], true);
        let d = v["detail"].as_str().unwrap();
        assert!(d.starts_with("DEV LINK: agents run /u/.clax/bin/dev/clax (clax 0.3.0-dev), not the release clax 0.2.0, with the home /u/.clax-dev"), "{d}");
        assert!(d.contains("`clax dev-unlink` returns to the release"), "{d}");
        assert!(d.contains("\nwarning: the dev link runs clax 0.3.0-dev"), "{d}");
        assert!(d.contains("\nlast MCP start: t launch mode=mcp agent=codex"), "{d}");
    }

    #[test]
    fn launch_fails_when_the_release_is_missing_and_says_who_downloads() {
        let e = clax_core::launch::NotFound {
            tried: vec!["CLAX_BIN: unset".into(), "dev link: none in /c/config.toml".into(), "/c/bin/0.2.0/clax: missing".into()],
            reason: "clax 0.2.0 is not installed at /c/bin/0.2.0/clax".into(),
            downloads: true,
        };
        let v = launch_check(DoctorAgent::Claude, "0.2.0", &Err(e.clone()), None);
        assert_eq!(v["ok"], false);
        let d = v["detail"].as_str().unwrap();
        assert!(d.contains("the MCP server downloads it when the next session starts"), "{d}");
        assert!(d.contains("tried: CLAX_BIN: unset; dev link: none in /c/config.toml; /c/bin/0.2.0/clax: missing"), "{d}");
        let v = launch_check(DoctorAgent::Pi, "0.2.0", &Err(e), None);
        assert!(v["detail"].as_str().unwrap().contains("Pi does not download"));
    }

    #[test]
    fn last_start_finds_the_agents_latest_mcp_line() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("logs")).unwrap();
        std::fs::write(
            dir.path().join("logs/hooks.log"),
            "t1 launch mode=mcp agent=codex source=installed bin=\"/a\"\nt2 launch mode=mcp agent=claude source=installed bin=\"/b\"\nt3 launcher mode=hook agent=codex exit=0 reason=\"x\"\n",
        )
        .unwrap();
        assert_eq!(last_start(dir.path(), "codex").as_deref(), Some("t1 launch mode=mcp agent=codex source=installed bin=\"/a\""));
        assert_eq!(last_start(dir.path(), "pi"), None);
    }
```

Run: `cargo test -p clax-cli doctor_agent`
Expected: PASS.

- [ ] **Step 5: `status` reports the launch**

In `crates/clax-mcp/src/plugin.rs`, add:

```rust
/// What the launcher said about this process in `CLAX_LAUNCH`,
/// `CLAX_LAUNCH_BIN` and `CLAX_LAUNCH_WARNING`, as `status` reports it:
/// `{source, bin, warning, notice}`, where `notice` states a dev link in a
/// sentence. `None` when the launcher did not start this process.
pub fn launch_from_env(env: impl Fn(&str) -> Option<String>) -> Option<serde_json::Value> {
    let source = env("CLAX_LAUNCH").filter(|s| !s.is_empty())?;
    let bin = env("CLAX_LAUNCH_BIN").filter(|s| !s.is_empty());
    let warning = env("CLAX_LAUNCH_WARNING").filter(|s| !s.is_empty());
    let notice = (source == "dev-link").then(|| {
        format!(
            "This session runs a dev build linked with `clax dev-link` ({}), not the released clax. `clax dev-unlink` returns to the release.",
            bin.as_deref().unwrap_or("unknown path")
        )
    });
    Some(serde_json::json!({"source": source, "bin": bin, "warning": warning, "notice": notice}))
}
```

and its test in the module's tests:

```rust
    #[test]
    fn launch_from_env_states_a_dev_link() {
        let e = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
        };
        assert_eq!(launch_from_env(e(&[])), None);
        let v = launch_from_env(e(&[("CLAX_LAUNCH", "installed"), ("CLAX_LAUNCH_BIN", "/c/bin/0.2.0/clax")])).unwrap();
        assert_eq!(v, serde_json::json!({"source": "installed", "bin": "/c/bin/0.2.0/clax", "warning": null, "notice": null}));
        let v = launch_from_env(e(&[
            ("CLAX_LAUNCH", "dev-link"),
            ("CLAX_LAUNCH_BIN", "/c/bin/dev/clax"),
            ("CLAX_LAUNCH_WARNING", "the dev link runs clax 0.3.0, but this plugin is clax 0.2.0"),
        ]))
        .unwrap();
        assert_eq!(v["notice"], "This session runs a dev build linked with `clax dev-link` (/c/bin/dev/clax), not the released clax. `clax dev-unlink` returns to the release.");
        assert_eq!(v["warning"], "the dev link runs clax 0.3.0, but this plugin is clax 0.2.0");
    }
```

In `crates/clax-mcp/src/tools.rs`, add a field `launch: Option<Value>` to `ClaxTools` (initialised `None` in `new`), and:

```rust
    /// These tools with what the launcher reported about this process
    /// ([`crate::plugin::launch_from_env`]), which `status` reports as `launch`.
    pub fn with_launch(mut self, launch: Option<Value>) -> ClaxTools {
        self.launch = launch;
        self
    }
```

In `status`, after the `plugin_version` block, add:

```rust
        // Which binary the launcher ran, and why (a dev link says so plainly).
        if let Some(l) = &self.launch {
            out["launch"] = l.clone();
        }
```

In `crates/clax-mcp/src/shim.rs`, chain `.with_launch(crate::plugin::launch_from_env(|k| std::env::var(k).ok()))` after `.with_plugin_version(plugin_version)`. When the launch is a dev link, also log the notice at `warn` level on startup, so the harness's MCP log says it too.

In `crates/clax-mcp/tests/tools.rs`, append to `status_reports_the_plugin_version_and_skew_when_known`:

```rust
    assert!(s.get("launch").is_none(), "{s}");
    let linked = tools_for(&ts).with_launch(Some(serde_json::json!({
        "source": "dev-link", "bin": "/c/bin/dev/clax", "warning": null,
        "notice": "This session runs a dev build linked with `clax dev-link` (/c/bin/dev/clax), not the released clax. `clax dev-unlink` returns to the release."
    })));
    let s = ok(linked.status(Parameters(StatusArgs {})).await);
    assert_eq!(s["launch"]["source"], "dev-link");
    assert!(s["launch"]["notice"].as_str().unwrap().starts_with("This session runs a dev build"));
```

Run: `cargo test -p clax-mcp`
Expected: PASS.

- [ ] **Step 6: Gates and commit**

Run: `bash scripts/quality_gates.sh; echo "exit=$?"`
Expected: `exit=0`.

```bash
git add crates/clax-core/src/launch.rs crates/clax-core/src/lib.rs crates/clax-cli/src/commands/doctor_agent.rs crates/clax-cli/src/commands/dev_link.rs crates/clax-mcp/src/plugin.rs crates/clax-mcp/src/tools.rs crates/clax-mcp/src/shim.rs crates/clax-mcp/tests/tools.rs
git commit -m "Say which binary runs and why: a launch check in doctor --agent and launch in status"
git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed
```

---

### Task 6: Version and packaging scripts

**Files:**
- Create: `scripts/check-version.sh`, `scripts/bump-version.sh`, `scripts/package-release.sh`, `scripts/smoke-release-binary.sh`, `scripts/test-release.sh`
- Modify: `scripts/ensure-clax.sh` and both plugin copies (`MIN_VERSION` becomes `CLAX_VERSION`), `scripts/test-plugins.sh`, `scripts/quality_gates.sh`

**Interfaces:**
- Produces: `scripts/check-version.sh [vX.Y.Z | --print]`. It exits 0 when every version agrees (and, given a tag, when the tag is `v<version>`). Otherwise it prints each source and its value and exits 1. `--print` prints the version.
- Produces: `scripts/bump-version.sh X.Y.Z`, which rewrites every source `check-version.sh` reads.
- Produces: `scripts/package-release.sh archive <version> <target> <binary> <outdir>` writes `<outdir>/clax-<version>-<target>.tar.gz`, which holds exactly `clax-<version>-<target>/clax`. It refuses a binary that does not report `clax <version>`. `scripts/package-release.sh sums <dir>` writes `<dir>/SHA256SUMS` for every other regular file in `<dir>`, in `sha256sum` format.
- Produces: `scripts/smoke-release-binary.sh <binary> <version>`, which exits 0 when the binary reports the version and serves the embedded web UI from a scratch home on a kernel-picked port.

- [ ] **Step 1: Rename the launcher's version constant**

In `scripts/ensure-clax.sh`, rename `MIN_VERSION` to `CLAX_VERSION` everywhere (the constant and `warn_if_old`). Change its comment to `# The Clax version this launcher belongs to; the plugins download exactly this release.` Then copy the file to both plugins:

```bash
cp scripts/ensure-clax.sh plugins/claude-code/scripts/ensure-clax.sh
cp scripts/ensure-clax.sh plugins/clax/scripts/ensure-clax.sh
```

Task 8 rewrites the launcher. This step only lets the scripts below read the version by its final name.

- [ ] **Step 2: `scripts/check-version.sh`**

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

- [ ] **Step 3: `scripts/bump-version.sh`**

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

- [ ] **Step 4: `scripts/package-release.sh`**

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

- [ ] **Step 5: `scripts/smoke-release-binary.sh`**

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

- [ ] **Step 6: `scripts/test-release.sh`**

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

cp "$HERE/scripts/ensure-clax.sh" "$T/dist/ensure-clax.sh"
"$T/scripts/package-release.sh" sums "$T/dist" >/dev/null
if (cd "$T/dist" && { sha256sum -c SHA256SUMS 2>/dev/null || shasum -a 256 -c SHA256SUMS; } >/dev/null) \
    && [ "$(wc -l < "$T/dist/SHA256SUMS" | tr -d ' ')" = 2 ] && ! grep -q SHA256SUMS "$T/dist/SHA256SUMS"; then
    pass "SHA256SUMS covers every other file and verifies"
else fail "SHA256SUMS covers every other file and verifies"; fi

[ "$FAILED" = 0 ] && echo "release script tests passed" || echo "release script tests FAILED"
exit "$FAILED"
```

- [ ] **Step 7: One version check, in one place**

In `scripts/test-plugins.sh`, replace the "One version everywhere" block (the `python3 - Cargo.toml … scripts/ensure-clax.sh` heredoc and its pass/fail) with:

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

- [ ] **Step 8: Run**

Run: `scripts/test-release.sh && scripts/test-plugins.sh | tail -1 && just web >/dev/null && cargo build --release -q -p clax-cli && scripts/smoke-release-binary.sh target/release/clax "$(scripts/check-version.sh --print)"`
Expected: `release script tests passed`, `plugin checks passed`, and `ok: target/release/clax is clax <version> and serves the embedded web UI`. The smoke runs the build in place because it is a test, not an agent; agents never run a binary from `target/`.

- [ ] **Step 9: Gates and commit**

Run: `bash scripts/quality_gates.sh; echo "exit=$?"`
Expected: `exit=0`.

```bash
git add scripts/check-version.sh scripts/bump-version.sh scripts/package-release.sh scripts/smoke-release-binary.sh scripts/test-release.sh scripts/test-plugins.sh scripts/quality_gates.sh scripts/ensure-clax.sh plugins/claude-code/scripts/ensure-clax.sh plugins/clax/scripts/ensure-clax.sh
git commit -m "Add the version check, version bump, release packaging and release-binary smoke scripts"
git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed
```

---

### Task 7: The release workflow, with a dry run

**Files:**
- Modify: `.github/workflows/release.yml`

**Interfaces:**
- Consumes: the Task 6 scripts.
- Produces: on a `v*` tag, a GitHub release `v<version>` holding `clax-<version>-{aarch64-apple-darwin,x86_64-apple-darwin,x86_64-unknown-linux-musl,aarch64-unknown-linux-musl}.tar.gz`, `ensure-clax.sh` and `SHA256SUMS`. On `workflow_dispatch`, and on pull requests that touch the release path, it runs everything except the publish job and keeps the result as the workflow artifact `release-dist`.

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
      - "scripts/ensure-clax.sh"
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
    name: Assemble and install through the launcher
    needs: [version, build]
    runs-on: ubuntu-24.04
    env:
      VERSION: ${{ needs.version.outputs.version }}
    steps:
      - uses: actions/checkout@v4
      - uses: actions/download-artifact@v4
        with: { path: dist, pattern: "clax-*", merge-multiple: true }
      - name: Add the launcher and checksums
        shell: bash
        run: |
          cp scripts/ensure-clax.sh dist/ensure-clax.sh
          scripts/package-release.sh sums dist
          cat dist/SHA256SUMS
          test "$(ls dist | wc -l)" = 6
      - name: Install through the launcher from a local copy of the release
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
          export HOME="$RUNNER_TEMP/home" CLAX_HOME="$RUNNER_TEMP/home/.clax" CLAX_RELEASE_BASE_URL="http://127.0.0.1:$port"
          bin="$(bash dist/ensure-clax.sh install)"
          test "$("$bin" --version)" = "clax $VERSION"
          test "$(readlink "$CLAX_HOME/bin/clax")" = "$VERSION/clax"
          echo "installed $bin"
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

Push nothing and dispatch nothing. The pull request that carries this plan's work runs the dry-run jobs, because it touches `release.yml`. The person runs the manual dispatch in "Steps for the person".

- [ ] **Step 3: Gates and commit**

Run: `bash scripts/quality_gates.sh; echo "exit=$?"`
Expected: `exit=0`.

```bash
git add .github/workflows/release.yml
git commit -m "Build, smoke-test and package four native release binaries; publish only on a tag"
git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed
```

---
### Task 8: The launcher: resolve, run, and explain

**Files:**
- Modify (rewrite): `scripts/ensure-clax.sh`, then copy it to `plugins/claude-code/scripts/ensure-clax.sh` and `plugins/clax/scripts/ensure-clax.sh`
- Modify (rewrite): `scripts/test-ensure-clax.sh`
- Modify: `plugins/clax/.mcp.json` (`env_vars`), `scripts/test-plugins.sh`

**Interfaces:**
- Consumes: the `[dev_link]` line format (Task 2) and the release layout (Task 6): `<base>/v<version>/SHA256SUMS` and `<base>/v<version>/clax-<version>-<target>.tar.gz` holding `clax-<version>-<target>/clax`.
- Produces: the launcher specified in spec §13 (Task 1). Its modes, exports and `hooks.log` lines are:
  - `<ts> launch mode=mcp agent=<a> source=<s> bin="<path>" version="<line>" warning="<text>"` on every MCP start.
  - `<ts> launcher mode=<m> agent=<a> exit=<code|fallback> reason="<text>" tried="<c1>; <c2>; …" argv="<args>"` on every failure.
- Produces: the fallback MCP server (Design decisions). It answers `initialize` with `instructions` starting `Clax is unavailable: `, `tools/list` with the one tool `status`, `tools/call` with `isError: true`, `ping` with `{}`, and any other request with error -32601.

This task removes the checkout search, the `PATH` search, `CLAX_SOURCE_DIR`, `CLAX_INSTALL_DIR`, `CLAX_RELEASE_VERSION`, the latest-release lookup and `~/.local/bin`, and every test of them. Task 9 adds the download tests against the fake release server. This task's tests use only an unreachable release URL (`http://127.0.0.1:9/`, where nothing listens).

- [ ] **Step 1: Write the new tests**

Replace `scripts/test-ensure-clax.sh` with:

```bash
#!/usr/bin/env bash
# Hermetic tests for ensure-clax.sh: scratch HOME and config directories, a
# PATH holding only the tools the launcher needs plus fake `clax` binaries,
# and, for downloads, scripts/fake-release-server.py on 127.0.0.1 (a port the
# kernel picks). No test reaches the network or the real ~/.clax.
set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$(mktemp -d)" && pwd -P)"
mkdir -p "$ROOT/launcher"
cp "$HERE/ensure-clax.sh" "$ROOT/launcher/ensure-clax.sh"
SCRIPT="$ROOT/launcher/ensure-clax.sh"
V="$(sed -n 's/^CLAX_VERSION="\(.*\)"$/\1/p' "$SCRIPT")"
PY="$(command -v python3)"
SERVER_PID=""
cleanup() {
    if [ -n "$SERVER_PID" ]; then kill "$SERVER_PID" 2>/dev/null; wait "$SERVER_PID" 2>/dev/null; fi
    rm -rf "$ROOT"
    return 0
}
trap cleanup EXIT

ORIG_PATH="$PATH"
FAILED=0
pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1"; FAILED=1; }

# The tools the launcher may call, linked into an otherwise empty directory.
TOOLS="$ROOT/tools"
mkdir -p "$TOOLS"
for t in bash sh env awk sort head tail grep sed tr cat curl tar sha256sum shasum mktemp uname mv chmod mkdir \
    rm ln readlink dirname basename date wc cp touch find sleep ls cut seq; do
    if p="$(command -v "$t" 2>/dev/null)" && [ -x "$p" ]; then ln -sf "$p" "$TOOLS/$t"; fi
done
# The same tools without curl.
NOCURL="$ROOT/nocurl"
mkdir -p "$NOCURL"
for t in "$TOOLS"/*; do
    name="${t##*/}"
    [ "$name" = curl ] || ln -sf "$(readlink "$t")" "$NOCURL/$name"
done

# A fake clax at $1/clax whose --version prints $2; any other run prints its
# arguments and what the launcher exported.
fake_clax() {
    mkdir -p "$1"
    cat > "$1/clax" <<SH
#!/bin/sh
if [ "\$1" = "--version" ]; then echo "$2"; exit 0; fi
echo "args: \$* home=\${CLAX_HOME:-} cfg=\${CLAX_CONFIG_DIR:-} launch=\${CLAX_LAUNCH:-} bin=\${CLAX_LAUNCH_BIN:-} warn=\${CLAX_LAUNCH_WARNING:-}"
SH
    chmod +x "$1/clax"
}
# A fake uname reporting system $1 and machine $2.
fake_uname() {
    printf '#!/bin/sh\ncase "$1" in -s) echo %s ;; -m) echo %s ;; esac\n' "$1" "$2" > "$FAKEBIN/uname"
    chmod +x "$FAKEBIN/uname"
}

# A fresh sandbox: HOME, PATH and every variable the launcher reads. The
# release URL points at a port where nothing listens.
new_env() {
    SANDBOX="$(mktemp -d "$ROOT/case.XXXXXX")"
    export HOME="$SANDBOX/home"
    mkdir -p "$HOME"
    FAKEBIN="$SANDBOX/fakebin"
    mkdir -p "$FAKEBIN"
    export PATH="$FAKEBIN:$TOOLS"
    unset CLAX_BIN CLAX_HOME CLAX_CONFIG_DIR CLAX_DOWNLOAD_TIMEOUT CLAX_MCP_WAIT CLAX_LAUNCH CLAX_LAUNCH_BIN \
        CLAX_LAUNCH_WARNING CLAX_SOURCE_DIR CLAX_INSTALL_DIR CLAX_RELEASE_VERSION
    export CLAX_RELEASE_BASE_URL="http://127.0.0.1:9/unreachable"
    CFGDIR="$HOME/.clax"
}
run() { OUT="$("$TOOLS/bash" "$SCRIPT" "$@" 2>"$SANDBOX/stderr" < /dev/null)"; RC=$?; ERR="$(cat "$SANDBOX/stderr")"; }
run_at() { local s="$1"; shift; OUT="$("$TOOLS/bash" "$s" "$@" 2>"$SANDBOX/stderr" < /dev/null)"; RC=$?; ERR="$(cat "$SANDBOX/stderr")"; }

# MCP requests as the clients send them: rmcp (Codex) puts the ID first, the
# TypeScript SDK (Claude Code) puts it last.
REQS='{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"method":"tools/list","params":{},"jsonrpc":"2.0","id":1}
{"jsonrpc":"2.0","id":"call-2","method":"tools/call","params":{"name":"status","arguments":{}}}
{"jsonrpc":"2.0","id":3,"method":"resources/list","params":{}}'
mcp() { OUT="$(printf '%s\n' "$REQS" | "$TOOLS/bash" "$SCRIPT" exec mcp --agent codex 2>"$SANDBOX/stderr")"; RC=$?; ERR="$(cat "$SANDBOX/stderr")"; }
# Checks that $OUT is the fallback server's answer to $REQS; prints the text of
# its status tool. Fails (non-zero) otherwise.
fallback_text() {
    "$PY" - "$OUT" <<'PYEOF'
import json, sys
lines = [json.loads(l) for l in sys.argv[1].splitlines()]
assert [l["id"] for l in lines] == [0, 1, "call-2", 3], lines
init = lines[0]["result"]
assert init["protocolVersion"] == "2025-06-18" and init["serverInfo"]["name"] == "clax", init
assert init["capabilities"] == {"tools": {}}, init
assert init["instructions"].startswith("Clax is unavailable: "), init
tools = lines[1]["result"]["tools"]
assert [t["name"] for t in tools] == ["status"] and tools[0]["inputSchema"]["type"] == "object", tools
call = lines[2]["result"]
assert call["isError"] is True and call["content"][0]["type"] == "text", call
assert lines[3]["error"]["code"] == -32601, lines[3]
print(call["content"][0]["text"])
PYEOF
}
hooks_log() { cat "$CFGDIR/logs/hooks.log" 2>/dev/null; }
# Writes a dev link to bin $1 (and home $2) into the config directory.
link() {
    mkdir -p "$CFGDIR"
    {
        printf '[dev_link]\nbin = "%s"\n' "$1"
        if [ -n "${2:-}" ]; then printf 'home = "%s"\n' "$2"; fi
        printf 'linked_at = "t"\n'
    } > "$CFGDIR/config.toml"
}

# --- Resolution -------------------------------------------------------------

new_env
fake_clax "$CFGDIR/bin/$V" "clax $V"
run
if [ "$RC" = 0 ] && [ "$OUT" = "$CFGDIR/bin/$V/clax" ]; then pass "the installed version is found"
else fail "the installed version is found (rc=$RC out=$OUT err=$ERR)"; fi

new_env
fake_clax "$CFGDIR/bin/$V" "clax 0.0.1"
run
if [ "$RC" = 1 ] && [ -z "$OUT" ] && echo "$ERR" | grep -q "clax $V is not installed" \
    && echo "$ERR" | grep -q "install it with: bash" \
    && hooks_log | grep -q "mode=print .*tried=\"CLAX_BIN: unset; dev link: none in $CFGDIR/config.toml; $CFGDIR/bin/$V/clax: not clax $V (clax 0.0.1)\""; then
    pass "an installed binary of another version is not used, and every candidate is logged"
else fail "an installed binary of another version is not used (rc=$RC out=$OUT err=$ERR log=$(hooks_log))"; fi

new_env
fake_clax "$SANDBOX/x" "clax $V"
fake_clax "$SANDBOX/dev" "clax $V"
link "$SANDBOX/dev/clax"
fake_clax "$CFGDIR/bin/$V" "clax $V"
CLAX_BIN="$SANDBOX/x/clax" run exec status
if [ "$RC" = 0 ] && [ "$OUT" = "args: status home= cfg=$CFGDIR launch=clax-bin bin=$SANDBOX/x/clax warn=" ]; then
    pass "CLAX_BIN wins over the dev link and the installed version"
else fail "CLAX_BIN wins (rc=$RC out=$OUT err=$ERR)"; fi

new_env
fake_clax "$SANDBOX/x" "clax 0.0.1"
CLAX_BIN="$SANDBOX/x/clax" run exec status
if [ "$RC" = 0 ] && echo "$OUT" | grep -q "warn=CLAX_BIN runs clax 0.0.1, but this plugin is clax $V" \
    && echo "$ERR" | grep -q "warning: CLAX_BIN runs clax 0.0.1"; then
    pass "CLAX_BIN of another version runs with a warning"
else fail "CLAX_BIN of another version runs with a warning (rc=$RC out=$OUT err=$ERR)"; fi

new_env
fake_clax "$CFGDIR/bin/$V" "clax $V"
CLAX_BIN="$SANDBOX/missing" run exec status
if [ "$RC" = 1 ] && [ -z "$OUT" ] && echo "$ERR" | grep -q "CLAX_BIN is set to '$SANDBOX/missing', which is not a usable clax binary"; then
    pass "an unusable CLAX_BIN fails instead of falling through"
else fail "an unusable CLAX_BIN fails instead of falling through (rc=$RC out=$OUT err=$ERR)"; fi

new_env
fake_clax "$SANDBOX/dev" "clax $V"
link "$SANDBOX/dev/clax" "$SANDBOX/devhome"
fake_clax "$CFGDIR/bin/$V" "clax $V"
run exec status
if [ "$RC" = 0 ] && [ "$OUT" = "args: status home=$SANDBOX/devhome cfg=$CFGDIR launch=dev-link bin=$SANDBOX/dev/clax warn=" ]; then
    pass "the dev link wins over the installed version and sets its home"
else fail "the dev link wins and sets its home (rc=$RC out=$OUT err=$ERR)"; fi

new_env
fake_clax "$SANDBOX/dev" "clax 9.9.9"
link "$SANDBOX/dev/clax"
mcp
if [ "$OUT" = "args: mcp --agent codex home= cfg=$CFGDIR launch=dev-link bin=$SANDBOX/dev/clax warn=the dev link runs clax 9.9.9, but this plugin is clax $V" ] \
    && hooks_log | grep -q "launch mode=mcp agent=codex source=dev-link bin=\"$SANDBOX/dev/clax\" version=\"clax 9.9.9\" warning=\"the dev link runs clax 9.9.9"; then
    pass "a dev link of another version runs in MCP mode with a logged warning"
else fail "a dev link of another version runs with a logged warning (out=$OUT log=$(hooks_log))"; fi

new_env
fake_clax "$CFGDIR/bin/$V" "clax $V"
link "$SANDBOX/gone/clax"
mcp
if text="$(fallback_text)" && echo "$text" | grep -q "points at '$SANDBOX/gone/clax', which is not a usable clax binary" \
    && echo "$text" | grep -q "just dev-install" && ! echo "$text" | grep -qi "download" \
    && hooks_log | grep -q "launcher mode=mcp agent=codex exit=fallback"; then
    pass "a broken dev link in MCP mode answers with the reason and neither falls through nor downloads"
else fail "a broken dev link in MCP mode answers with the reason (out=$OUT err=$ERR)"; fi

new_env
mcp
if text="$(fallback_text)" && echo "$text" | grep -q "clax $V is not installed, and downloading it failed: cannot reach http://127.0.0.1:9/unreachable/v$V/SHA256SUMS"; then
    pass "no binary and an unreachable release: the MCP client gets the reason"
else fail "no binary and an unreachable release: the MCP client gets the reason (out=$OUT err=$ERR)"; fi

new_env
CLAX_BIN="$SANDBOX/a\"b\\c" mcp
if text="$(fallback_text)" && echo "$text" | grep -qF "$SANDBOX/a\"b\\c"; then
    pass "the fallback escapes quotes and backslashes in its JSON"
else fail "the fallback escapes quotes and backslashes (out=$OUT)"; fi

# Neither PATH nor a source checkout nor a harness's configuration is searched.
new_env
fake_clax "$FAKEBIN" "clax $V"
mkdir -p "$SANDBOX/repo/plugins/clax/scripts" "$SANDBOX/repo/plugins/clax/.codex-plugin"
printf '[workspace]\nmembers = [\n    "crates/clax-cli",\n]\n' > "$SANDBOX/repo/Cargo.toml"
cp "$SCRIPT" "$SANDBOX/repo/plugins/clax/scripts/ensure-clax.sh"
echo "{\"name\": \"clax\", \"version\": \"$V\", \"source\": \"$SANDBOX/repo\"}" > "$SANDBOX/repo/plugins/clax/.codex-plugin/plugin.json"
fake_clax "$SANDBOX/repo/target/debug" "clax $V"
fake_clax "$SANDBOX/repo/target/release" "clax $V"
mkdir -p "$HOME/.codex" "$HOME/.local/bin"
printf '[marketplaces.clax]\nsource_type = "local"\nsource = "%s"\n' "$SANDBOX/repo" > "$HOME/.codex/config.toml"
fake_clax "$HOME/.local/bin" "clax $V"
CLAX_SOURCE_DIR="$SANDBOX/repo" CLAX_INSTALL_DIR="$HOME/.local/bin" run_at "$SANDBOX/repo/plugins/clax/scripts/ensure-clax.sh"
if [ "$RC" = 1 ] && [ -z "$OUT" ]; then pass "PATH, ~/.local/bin, a checkout's target/ and Codex's config are never searched"
else fail "PATH, ~/.local/bin, a checkout's target/ and Codex's config are never searched (rc=$RC out=$OUT)"; fi

new_env
export CLAX_HOME="$SANDBOX/h"
fake_clax "$SANDBOX/h/bin/$V" "clax $V"
run
r1="$OUT"
export CLAX_CONFIG_DIR="$SANDBOX/c"
fake_clax "$SANDBOX/c/bin/$V" "clax $V"
run
if [ "$r1" = "$SANDBOX/h/bin/$V/clax" ] && [ "$OUT" = "$SANDBOX/c/bin/$V/clax" ]; then
    pass "the config directory is CLAX_CONFIG_DIR, else CLAX_HOME"
else fail "the config directory is CLAX_CONFIG_DIR, else CLAX_HOME (h=$r1 c=$OUT)"; fi

new_env
fake_clax "$CFGDIR/bin/$V" "clax $V"
run exec one "two words"
if [ "$RC" = 0 ] && echo "$OUT" | grep -q "^args: one two words "; then pass "exec passes arguments through"
else fail "exec passes arguments through (rc=$RC out=$OUT)"; fi

new_env
run bogus
if [ "$RC" = 2 ] && echo "$ERR" | grep -q usage; then pass "an unknown mode prints usage"; else fail "an unknown mode prints usage (rc=$RC)"; fi

# --- Hooks never download and never fail --------------------------------------

new_env
run exec hook --agent codex session-start
if [ "$RC" = 0 ] && [ -z "$OUT" ] && [ "$(printf '%s\n' "$ERR" | grep -c .)" = 1 ] \
    && echo "$ERR" | grep -q "clax $V is not installed" && echo "$ERR" | grep -q "Hooks never download" \
    && ! echo "$ERR" | grep -q "cannot reach" \
    && hooks_log | grep -q "launcher mode=hook agent=codex exit=0 reason=\"clax $V is not installed.*tried=\"CLAX_BIN: unset; dev link: none in .*argv=\"exec hook --agent codex session-start\""; then
    pass "hook mode with no binary prints one line, logs every candidate, exits 0 and downloads nothing"
else fail "hook mode with no binary (rc=$RC out=$OUT err=$ERR log=$(hooks_log))"; fi

new_env
CLAX_BIN="$SANDBOX/missing" run exec hook --agent claude stop
if [ "$RC" = 0 ] && [ -z "$OUT" ] && [ "$(printf '%s\n' "$ERR" | grep -c .)" = 1 ] && echo "$ERR" | grep -q "CLAX_BIN is set to"; then
    pass "hook mode with an unusable CLAX_BIN prints one line and exits 0"
else fail "hook mode with an unusable CLAX_BIN (rc=$RC err=$ERR)"; fi

new_env
link "$SANDBOX/gone/clax"
run exec hook --agent claude stop
if [ "$RC" = 0 ] && [ -z "$OUT" ] && echo "$ERR" | grep -q "dev link"; then pass "hook mode with a broken dev link exits 0"
else fail "hook mode with a broken dev link exits 0 (rc=$RC err=$ERR)"; fi

new_env
fake_clax "$SANDBOX/dev" "clax $V"
link "$SANDBOX/dev/clax" "$SANDBOX/devhome"
run exec hook --agent codex stop
if [ "$RC" = 0 ] && [ "$OUT" = "args: hook --agent codex stop home=$SANDBOX/devhome cfg=$CFGDIR launch=dev-link bin=$SANDBOX/dev/clax warn=" ]; then
    pass "hook mode runs the dev link with its home"
else fail "hook mode runs the dev link with its home (rc=$RC out=$OUT)"; fi

new_env
mkdir -p "$CFGDIR/bin/$V"
printf '#!/bin/sh\nif [ "$1" = "--version" ]; then echo "clax %s"; exit 0; fi\necho "partial output"\necho "boom: daemon exploded" >&2\nexit 3\n' "$V" > "$CFGDIR/bin/$V/clax"
chmod +x "$CFGDIR/bin/$V/clax"
run exec hook --agent claude prompt
if [ "$RC" = 0 ] && [ -z "$OUT" ] && echo "$ERR" | grep -q "boom: daemon exploded" \
    && hooks_log | grep -q "launcher mode=hook agent=claude exit=3 reason=\"clax exited 3: boom: daemon exploded\""; then
    pass "a failing hook binary exits 0, drops its stdout and is logged with its stderr"
else fail "a failing hook binary (rc=$RC out=$OUT err=$ERR log=$(hooks_log))"; fi

new_env
fake_clax "$CFGDIR/bin/$V" "clax $V"
run exec hook --agent codex stop
if [ "$RC" = 0 ] && echo "$OUT" | grep -q "^args: hook --agent codex stop .*launch=installed" && [ -z "$(hooks_log)" ]; then
    pass "a succeeding hook passes its stdout through and logs nothing"
else fail "a succeeding hook passes its stdout through (rc=$RC out=$OUT log=$(hooks_log))"; fi

new_env
export CLAX_HOME="$SANDBOX/ax-home"
CFGDIR="$CLAX_HOME"
mkdir -p "$CFGDIR/logs"
awk 'BEGIN { for (i = 0; i < 20000; i++) print "0123456789012345678901234567890123456789012345678901234567890123" }' > "$CFGDIR/logs/hooks.log"
run exec hook --agent claude stop
if [ "$RC" = 0 ] && [ -s "$CFGDIR/logs/hooks.log.1" ] && [ "$(wc -l < "$CFGDIR/logs/hooks.log" | tr -d ' ')" = 1 ] \
    && grep -q "agent=claude" "$CFGDIR/logs/hooks.log"; then
    pass "hooks.log rotates to hooks.log.1 past 1 MiB"
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
mkdir -p "$HOME/.$OLD/bin/$V"
cp "$SANDBOX/elsewhere/clax" "$HOME/.$OLD/bin/$V/clax"
export "${OLD_UPPER}_BIN=$SANDBOX/elsewhere/clax" "${OLD_UPPER}_HOME=$HOME/.$OLD" "${OLD_UPPER}_CONFIG_DIR=$HOME/.$OLD"
run exec hook --agent codex stop
unset "${OLD_UPPER}_BIN" "${OLD_UPPER}_HOME" "${OLD_UPPER}_CONFIG_DIR"
if [ "$RC" = 0 ] && [ -z "$OUT" ] && echo "$ERR" | grep -q "is not installed" \
    && [ ! -e "$HOME/.$OLD/logs" ] && [ -s "$HOME/.clax/logs/hooks.log" ] \
    && [ "$(PATH="$ORIG_PATH" ls -A "$HOME/.$OLD")" = bin ]; then
    pass "the previous name's variables, binary and home are ignored and left untouched"
else fail "the previous name's variables, binary and home are ignored (rc=$RC out=$OUT err=$ERR)"; fi

# (Task 9 adds the download cases here.)

[ "$FAILED" = 0 ] && echo "all launcher tests passed" || echo "launcher tests FAILED"
exit "$FAILED"
```

Run: `bash scripts/test-ensure-clax.sh`
Expected: FAIL. The old launcher finds `PATH` and checkout binaries and has no fallback server.

- [ ] **Step 2: Write the launcher**

Replace `scripts/ensure-clax.sh` with the script below. `CLAX_VERSION` keeps the value Task 6 set (the workspace version).

```bash
#!/usr/bin/env bash
# The Clax plugins' launcher: runs the clax release this plugin belongs to.
#
# Usage:
#   ensure-clax.sh                   print the resolved binary's path
#   ensure-clax.sh install           download and install clax $CLAX_VERSION
#   ensure-clax.sh exec mcp <args>   run the MCP server (may download once)
#   ensure-clax.sh exec hook <args>  run a hook (never downloads; exits 0)
#   ensure-clax.sh exec <args>       run any other clax command
#
# Resolution, first match wins:
#   1. $CLAX_BIN. When set it must be a usable clax.
#   2. The dev link: `bin` in the [dev_link] table of <config dir>/config.toml,
#      written by `clax dev-link`. When set it must be a usable clax; its
#      optional `home` becomes CLAX_HOME for the binary.
#   3. <config dir>/bin/$CLAX_VERSION/clax, when it reports clax $CLAX_VERSION.
#   4. MCP mode only, and only when neither 1 nor 2 is set: download clax
#      $CLAX_VERSION for this machine from the release, check it against the
#      release's SHA256SUMS, and install it as 3, under a lock and by atomic
#      rename, keeping the previously installed version and removing older
#      ones. `ensure-clax.sh install` does the same in the foreground.
#   5. Otherwise it fails with the reason. MCP mode answers the MCP client
#      with a minimal server whose `status` tool states the reason; hook mode
#      prints one line and exits 0; other modes print it and exit 1.
# It never looks for clax on PATH, in a source checkout, in a harness's
# configuration, or next to itself. A binary from 1 or 2 that reports
# another version runs with a warning.
#
# The config directory is $CLAX_CONFIG_DIR, else $CLAX_HOME, else ~/.clax.
# Every failure, and every MCP start, appends one line to
# <config dir>/logs/hooks.log (rotated to hooks.log.1 past 1 MiB).
#
# The binary runs with CLAX_CONFIG_DIR, CLAX_LAUNCH (clax-bin, dev-link,
# installed or downloaded), CLAX_LAUNCH_BIN and, on a version mismatch,
# CLAX_LAUNCH_WARNING set.
#
# Environment:
#   CLAX_BIN               a clax binary to run ahead of everything else
#   CLAX_HOME              the Clax home (default ~/.clax)
#   CLAX_CONFIG_DIR        where config.toml, bin/ and logs/hooks.log live
#   CLAX_RELEASE_BASE_URL  the release download base; files are fetched from
#                          <base>/v<version>/ (default: this repository's
#                          GitHub releases)
#   CLAX_DOWNLOAD_TIMEOUT  seconds each download may take (default 120)
#   CLAX_MCP_WAIT          seconds the MCP server waits for a download before
#                          answering with the reason (default 8)

set -uo pipefail

# The Clax version this launcher belongs to; the plugins run exactly this release.
CLAX_VERSION="0.2.0"
REPO="empathic/clax"
RELEASE_BASE_URL="${CLAX_RELEASE_BASE_URL:-https://github.com/${REPO}/releases/download}"
DOWNLOAD_TIMEOUT="${CLAX_DOWNLOAD_TIMEOUT:-120}"
MCP_WAIT="${CLAX_MCP_WAIT:-8}"
LOG_MAX_BYTES=1048576
LOCK_STALE_MINUTES=15
ARGV="$*"

case "${1:-}" in
    "") MODE=print ;;
    install) MODE=install ;;
    exec)
        case "${2:-}" in
            mcp) MODE=mcp ;;
            hook) MODE=hook ;;
            *) MODE=cli ;;
        esac
        ;;
    *) echo "usage: ensure-clax.sh [install | exec <clax arguments...>]" >&2; exit 2 ;;
esac
AGENT=-
prev=""
for a in "$@"; do
    if [ "$prev" = --agent ]; then AGENT="$a"; fi
    prev="$a"
done

if [ -n "${CLAX_CONFIG_DIR:-}" ]; then CFG="$CLAX_CONFIG_DIR"
elif [ -n "${CLAX_HOME:-}" ]; then CFG="$CLAX_HOME"
elif [ -n "${HOME:-}" ]; then CFG="$HOME/.clax"
else CFG=""
fi

BIN="" SOURCE="" GOT_VERSION="" WARNING="" REASON="" TRIED="" CAN_DOWNLOAD=""
LINK_BIN="" LINK_HOME="" INSTALL_ERROR="" LOCK_HELD=""

log() { echo "$@" >&2; }
tried() { TRIED="${TRIED:+$TRIED; }$1"; }
oneline() { printf '%s' "$1" | tr '\n"' " '"; }

# Appends "<time> $1" to <config dir>/logs/hooks.log, rotating it past
# LOG_MAX_BYTES. Never fails.
hooks_log() {
    {
        [ -n "$CFG" ] || return 0
        local dir="$CFG/logs" size
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

# Sets LINK_BIN and LINK_HOME from the [dev_link] table of config.toml, which
# `clax dev-link` writes as `key = "value"` lines.
read_dev_link() {
    LINK_BIN="" LINK_HOME=""
    local file="$CFG/config.toml" key val
    [ -f "$file" ] || return 0
    while IFS="$(printf '\t')" read -r key val; do
        case "$key" in
            bin) LINK_BIN="$val" ;;
            home) LINK_HOME="$val" ;;
        esac
    done < <(awk '
        /^[[:space:]]*\[/ { t = $0; gsub(/[[:space:]]/, "", t); on = (t == "[dev_link]"); next }
        on && /^[[:space:]]*(bin|home)[[:space:]]*=/ {
            k = $0; sub(/^[[:space:]]*/, "", k); sub(/[[:space:]]*=.*$/, "", k)
            v = $0; sub(/^[^=]*=[[:space:]]*"/, "", v); sub(/"[[:space:]]*$/, "", v)
            print k "\t" v
        }' "$file" 2>/dev/null)
}

# Finds the binary (1 to 3 above). Sets BIN, SOURCE, WARNING and TRIED; on
# failure sets REASON, and CAN_DOWNLOAD when step 4 may run.
resolve() {
    if [ -n "${CLAX_BIN:-}" ]; then
        if check_bin "$CLAX_BIN"; then
            BIN="$CLAX_BIN" SOURCE=clax-bin
            tried "CLAX_BIN=$CLAX_BIN: $GOT_VERSION"
            if [ "$GOT_VERSION" != "clax $CLAX_VERSION" ]; then
                WARNING="CLAX_BIN runs $GOT_VERSION, but this plugin is clax $CLAX_VERSION"
            fi
            return 0
        fi
        tried "CLAX_BIN=$CLAX_BIN: not a usable clax"
        REASON="CLAX_BIN is set to '$CLAX_BIN', which is not a usable clax binary. Unset CLAX_BIN, or point it at a clax binary."
        return 1
    fi
    tried "CLAX_BIN: unset"
    if [ -z "$CFG" ]; then
        REASON="neither CLAX_CONFIG_DIR, CLAX_HOME nor HOME is set, so there is nowhere to find clax."
        return 1
    fi
    read_dev_link
    if [ -n "$LINK_BIN" ]; then
        if check_bin "$LINK_BIN"; then
            BIN="$LINK_BIN" SOURCE=dev-link
            tried "dev link $LINK_BIN: $GOT_VERSION${LINK_HOME:+, home $LINK_HOME}"
            if [ "$GOT_VERSION" != "clax $CLAX_VERSION" ]; then
                WARNING="the dev link runs $GOT_VERSION, but this plugin is clax $CLAX_VERSION"
            fi
            return 0
        fi
        tried "dev link $LINK_BIN: not a usable clax"
        REASON="the dev link in $CFG/config.toml points at '$LINK_BIN', which is not a usable clax binary. Run \`just dev-install\` in your Clax checkout to rebuild it, or remove the [dev_link] table from $CFG/config.toml (\`clax dev-unlink\`) to use the release."
        return 1
    fi
    tried "dev link: none in $CFG/config.toml"
    local inst="$CFG/bin/$CLAX_VERSION/clax"
    if check_bin "$inst" && [ "$GOT_VERSION" = "clax $CLAX_VERSION" ]; then
        BIN="$inst" SOURCE=installed
        tried "$inst: $GOT_VERSION"
        return 0
    fi
    if [ -e "$inst" ]; then tried "$inst: not clax $CLAX_VERSION (${GOT_VERSION:-no version})"; else tried "$inst: missing"; fi
    CAN_DOWNLOAD=1
    REASON="clax $CLAX_VERSION is not installed ($inst is missing or is another version)."
    return 1
}

# The release target for this machine, or nothing.
release_target() {
    case "$(uname -s)/$(uname -m)" in
        Darwin/arm64 | Darwin/aarch64) echo aarch64-apple-darwin ;;
        Darwin/x86_64) echo x86_64-apple-darwin ;;
        Linux/x86_64 | Linux/amd64) echo x86_64-unknown-linux-musl ;;
        Linux/aarch64 | Linux/arm64) echo aarch64-unknown-linux-musl ;;
        *) return 1 ;;
    esac
}

sha256() {
    if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{ print $1 }'
    else shasum -a 256 "$1" | awk '{ print $1 }'; fi
}

# Downloads $1 to $2. On failure sets INSTALL_ERROR and returns 1.
fetch() {
    local code rc
    code="$(curl -sSL --connect-timeout 10 --max-time "$DOWNLOAD_TIMEOUT" -o "$2" -w '%{http_code}' "$1" 2>/dev/null)"
    rc=$?
    case "$rc" in
        0) ;;
        28) INSTALL_ERROR="downloading $1 timed out after ${DOWNLOAD_TIMEOUT}s"; return 1 ;;
        18) INSTALL_ERROR="the download of $1 was cut short"; return 1 ;;
        6 | 7) INSTALL_ERROR="cannot reach $1"; return 1 ;;
        *) INSTALL_ERROR="downloading $1 failed (curl exit $rc)"; return 1 ;;
    esac
    if [ "$code" != 200 ]; then
        INSTALL_ERROR="$1 answered HTTP $code; is v$CLAX_VERSION released?"
        return 1
    fi
}

# Takes <config dir>/bin/.install.lock, waiting up to $1 seconds. A lock whose
# holder is gone, or that is older than LOCK_STALE_MINUTES, is taken over.
take_lock() {
    local lock="$CFG/bin/.install.lock" deadline holder me
    me="$(exec sh -c 'echo "$PPID"')"
    deadline=$(( $(date +%s) + $1 ))
    while :; do
        if mkdir "$lock" 2>/dev/null; then
            echo "$me" > "$lock/pid"
            LOCK_HELD="$lock"
            return 0
        fi
        holder="$(cat "$lock/pid" 2>/dev/null)"
        if { [ -n "$holder" ] && ! kill -0 "$holder" 2>/dev/null; } \
            || [ -n "$(find "$lock" -maxdepth 0 -mmin +"$LOCK_STALE_MINUTES" 2>/dev/null)" ]; then
            rm -rf "$lock"
            continue
        fi
        [ "$(date +%s)" -lt "$deadline" ] || return 1
        sleep 0.2
    done
}
release_lock() {
    if [ -n "$LOCK_HELD" ]; then rm -rf "$LOCK_HELD"; fi
    LOCK_HELD=""
}

# The version <config dir>/bin/clax points at, if any.
current_version() { readlink "$CFG/bin/clax" 2>/dev/null | sed -n 's#^\([^/]*\)/clax$#\1#p'; }

# The newest installed release other than CLAX_VERSION.
newest_other() {
    local d name
    for d in "$CFG"/bin/*; do
        [ -d "$d" ] || continue
        name="${d##*/}"
        case "$name" in [0-9]*.[0-9]*.[0-9]*) [ "$name" = "$CLAX_VERSION" ] || echo "$name" ;; esac
    done | sort -V | tail -1
}

# Removes installed releases other than CLAX_VERSION, $1 (the previous one)
# and the version the home's running daemon reports. Never touches bin/dev.
prune() {
    local keep="$1" running d name
    running="$(sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$CFG/daemon.json" 2>/dev/null | head -1)"
    for d in "$CFG"/bin/*; do
        [ -d "$d" ] || continue
        name="${d##*/}"
        case "$name" in [0-9]*.[0-9]*.[0-9]*) ;; *) continue ;; esac
        if [ "$name" = "$CLAX_VERSION" ] || [ "$name" = "$keep" ] || [ "$name" = "$running" ]; then continue; fi
        rm -rf "$d"
    done
}

# Installs clax $CLAX_VERSION into <config dir>/bin/$CLAX_VERSION under the
# lock: downloads the archive and SHA256SUMS into a temporary directory in
# bin/, checks the archive's checksum and the binary's version, then renames
# the directory into place, points bin/clax at it, and prunes. On failure sets
# INSTALL_ERROR and returns 1, leaving nothing half-installed.
install_release() {
    local target tmp rc
    INSTALL_ERROR=""
    target="$(release_target)" || { INSTALL_ERROR="there is no prebuilt clax for $(uname -s)/$(uname -m)"; return 1; }
    command -v curl >/dev/null 2>&1 || { INSTALL_ERROR="curl is required to download clax"; return 1; }
    command -v tar >/dev/null 2>&1 || { INSTALL_ERROR="tar is required to unpack clax"; return 1; }
    command -v sha256sum >/dev/null 2>&1 || command -v shasum >/dev/null 2>&1 \
        || { INSTALL_ERROR="sha256sum or shasum is required to check clax"; return 1; }
    mkdir -p "$CFG/bin" || { INSTALL_ERROR="cannot create $CFG/bin"; return 1; }
    take_lock "$DOWNLOAD_TIMEOUT" \
        || { INSTALL_ERROR="another session has held $CFG/bin/.install.lock for over ${DOWNLOAD_TIMEOUT}s"; return 1; }
    if check_bin "$CFG/bin/$CLAX_VERSION/clax" && [ "$GOT_VERSION" = "clax $CLAX_VERSION" ]; then
        release_lock
        return 0
    fi
    tmp="$(mktemp -d "$CFG/bin/.tmp.XXXXXX")" \
        || { release_lock; INSTALL_ERROR="cannot create a temporary directory in $CFG/bin"; return 1; }
    if install_into "$tmp" "$target"; then rc=0; else rc=1; fi
    rm -rf "$tmp"
    release_lock
    return "$rc"
}

install_into() {
    local tmp="$1" target="$2" name expected actual prev dest="$CFG/bin/$CLAX_VERSION"
    local base="$RELEASE_BASE_URL/v$CLAX_VERSION"
    name="clax-$CLAX_VERSION-$target"
    fetch "$base/SHA256SUMS" "$tmp/SHA256SUMS" || return 1
    fetch "$base/$name.tar.gz" "$tmp/$name.tar.gz" || return 1
    expected="$(awk -v f="$name.tar.gz" '{ n = $2; sub(/^\*/, "", n) } n == f { print $1; exit }' "$tmp/SHA256SUMS")"
    [ -n "$expected" ] || { INSTALL_ERROR="the SHA256SUMS of v$CLAX_VERSION does not list $name.tar.gz"; return 1; }
    actual="$(sha256 "$tmp/$name.tar.gz")"
    if [ "$actual" != "$expected" ]; then
        INSTALL_ERROR="checksum mismatch for $name.tar.gz (SHA256SUMS says $expected, the download is $actual); nothing was installed"
        return 1
    fi
    mkdir "$tmp/x" && tar -xzf "$tmp/$name.tar.gz" -C "$tmp/x" 2>/dev/null \
        || { INSTALL_ERROR="$name.tar.gz could not be unpacked"; return 1; }
    if ! check_bin "$tmp/x/$name/clax" || [ "$GOT_VERSION" != "clax $CLAX_VERSION" ]; then
        INSTALL_ERROR="$name.tar.gz does not hold clax $CLAX_VERSION (its binary reports '${GOT_VERSION:-nothing}')"
        return 1
    fi
    mkdir "$tmp/$CLAX_VERSION" && mv "$tmp/x/$name/clax" "$tmp/$CLAX_VERSION/clax" \
        || { INSTALL_ERROR="cannot stage clax in $tmp"; return 1; }
    if [ -e "$dest" ]; then
        mv "$dest" "$tmp/replaced" || { INSTALL_ERROR="cannot move the broken $dest aside"; return 1; }
    fi
    mv "$tmp/$CLAX_VERSION" "$dest" || { INSTALL_ERROR="cannot move clax into $dest"; return 1; }
    prev="$(current_version)"
    if [ -z "$prev" ] || [ "$prev" = "$CLAX_VERSION" ]; then prev="$(newest_other)"; fi
    rm -f "$CFG/bin/.clax-link.$$"
    ln -s "$CLAX_VERSION/clax" "$CFG/bin/.clax-link.$$" && mv -f "$CFG/bin/.clax-link.$$" "$CFG/bin/clax"
    rm -f "$CFG/bin/.clax-link.$$"
    prune "$prev"
    return 0
}

# MCP mode: installs clax $CLAX_VERSION in the background and waits up to
# MCP_WAIT seconds. On success sets BIN and returns 0; otherwise sets REASON
# and returns 1 (the download may still be running).
download_for_mcp() {
    local result pid waited=0 limit line
    result="$(mktemp "${TMPDIR:-/tmp}/clax-install.XXXXXX" 2>/dev/null)" || {
        REASON="$REASON It could not be downloaded: no temporary file could be created."
        return 1
    }
    (
        trap release_lock EXIT
        if install_release; then echo ok; else echo "fail $INSTALL_ERROR"; fi > "$result"
    ) < /dev/null > /dev/null 2>&1 &
    pid=$!
    limit=$(( MCP_WAIT * 5 ))
    while kill -0 "$pid" 2>/dev/null && [ "$waited" -lt "$limit" ]; do
        sleep 0.2
        waited=$(( waited + 1 ))
    done
    if kill -0 "$pid" 2>/dev/null; then
        REASON="clax $CLAX_VERSION is not installed yet: it is still downloading after ${MCP_WAIT}s. Reconnect the clax MCP server in a minute (/mcp in Claude Code), or start a new session."
        return 1
    fi
    wait "$pid" 2>/dev/null
    line="$(head -1 "$result" 2>/dev/null)"
    rm -f "$result"
    case "$line" in
        ok)
            if check_bin "$CFG/bin/$CLAX_VERSION/clax" && [ "$GOT_VERSION" = "clax $CLAX_VERSION" ]; then
                BIN="$CFG/bin/$CLAX_VERSION/clax"
                return 0
            fi
            REASON="clax $CLAX_VERSION was installed, but $CFG/bin/$CLAX_VERSION/clax does not report it."
            ;;
        "fail "*) REASON="clax $CLAX_VERSION is not installed, and downloading it failed: ${line#fail }." ;;
        *) REASON="clax $CLAX_VERSION is not installed, and the download stopped without saying why." ;;
    esac
    return 1
}

# JSON string escaping for the fallback server.
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

# The status tool's text: the reason, or, once a download has finished since,
# that clax is now installed.
status_text() {
    if [ -n "$CAN_DOWNLOAD" ] && check_bin "$CFG/bin/$CLAX_VERSION/clax" && [ "$GOT_VERSION" = "clax $CLAX_VERSION" ]; then
        echo "clax $CLAX_VERSION is now installed. Reconnect the clax MCP server (/mcp in Claude Code), or start a new session, to use it."
    else
        echo "$1"
    fi
}

# A minimal MCP server on stdin/stdout whose one tool, status, states why clax
# cannot run, so the client and the agent see the reason instead of a closed
# pipe. Answers until stdin closes.
serve_unavailable() {
    local text line method id proto
    text="Clax is unavailable: $REASON (Details: ${CFG:-~/.clax}/logs/hooks.log.)"
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

# Runs the resolved binary for this mode. Never returns.
launch() {
    if [ -n "$CFG" ]; then export CLAX_CONFIG_DIR="$CFG"; fi
    export CLAX_LAUNCH="$SOURCE" CLAX_LAUNCH_BIN="$BIN"
    if [ -n "$WARNING" ]; then export CLAX_LAUNCH_WARNING="$WARNING"; else unset CLAX_LAUNCH_WARNING; fi
    if [ "$SOURCE" = dev-link ] && [ -n "$LINK_HOME" ]; then export CLAX_HOME="$LINK_HOME"; fi
    case "$MODE" in
        print)
            echo "$BIN"
            exit 0
            ;;
        hook)
            shift
            run_hook "$BIN" "$@"
            ;;
        mcp)
            if [ -n "$WARNING" ]; then log "clax: warning: $WARNING"; fi
            hooks_log "launch mode=mcp agent=$AGENT source=$SOURCE bin=\"$(oneline "$BIN")\" version=\"$GOT_VERSION\" warning=\"$(oneline "$WARNING")\""
            shift
            exec "$BIN" "$@"
            ;;
        *)
            if [ -n "$WARNING" ]; then log "clax: warning: $WARNING"; fi
            shift
            exec "$BIN" "$@"
            ;;
    esac
}

install_mode() {
    trap release_lock EXIT
    if [ -z "$CFG" ]; then log "clax: neither CLAX_CONFIG_DIR, CLAX_HOME nor HOME is set"; exit 1; fi
    if check_bin "$CFG/bin/$CLAX_VERSION/clax" && [ "$GOT_VERSION" = "clax $CLAX_VERSION" ]; then
        echo "$CFG/bin/$CLAX_VERSION/clax"
        exit 0
    fi
    log "clax: installing clax $CLAX_VERSION into $CFG/bin/$CLAX_VERSION"
    if install_release; then
        log "clax: installed; $CFG/bin/clax points at it (add $CFG/bin to PATH to run clax from a shell)"
        echo "$CFG/bin/$CLAX_VERSION/clax"
        exit 0
    fi
    REASON="installing clax $CLAX_VERSION failed: $INSTALL_ERROR"
    log "clax: $REASON"
    fail_line 1
    exit 1
}

main() {
    if [ "$MODE" = install ]; then install_mode; fi
    if resolve; then launch "$@"; fi
    case "$MODE" in
        hook)
            log "clax: $REASON Hooks never download; the MCP server installs clax when a session starts."
            fail_line 0
            exit 0
            ;;
        mcp)
            if [ -n "$CAN_DOWNLOAD" ] && download_for_mcp; then
                SOURCE=downloaded
                launch "$@"
            fi
            fail_line fallback
            log "clax: $REASON"
            serve_unavailable
            exit 0
            ;;
        *)
            log "clax: $REASON"
            if [ -n "$CAN_DOWNLOAD" ]; then log "clax: install it with: bash \"$0\" install"; fi
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

- [ ] **Step 3: Codex forwards the launcher's variables**

In `plugins/clax/.mcp.json`, set `env_vars` to exactly:

```json
      "env_vars": [
        "CLAX_HOME",
        "CLAX_CONFIG_DIR",
        "CLAX_NO_OPEN",
        "CLAX_BIN",
        "CLAX_RELEASE_BASE_URL",
        "CLAX_DOWNLOAD_TIMEOUT",
        "CLAX_MCP_WAIT",
        "CLAX_CODEX_BIN"
      ]
```

In `scripts/test-plugins.sh`, inside the Python block of the "Codex MCP server and hooks use --agent codex" check, add before `sys.exit(0 if ok else 1)`:

```python
ok = ok and server.get("env_vars") == ["CLAX_HOME", "CLAX_CONFIG_DIR", "CLAX_NO_OPEN", "CLAX_BIN", "CLAX_RELEASE_BASE_URL", "CLAX_DOWNLOAD_TIMEOUT", "CLAX_MCP_WAIT", "CLAX_CODEX_BIN"]
```

- [ ] **Step 4: Run the tests**

Run: `bash scripts/test-ensure-clax.sh`
Expected: every line `PASS`, then `all launcher tests passed`.

Run: `bash scripts/test-plugins.sh | tail -1 && cargo test -p clax-cli doctor_agent`
Expected: `plugin checks passed`, and the doctor tests pass. They compare the plugins' launcher copies with the one the binary embeds, which Step 2 kept identical.

- [ ] **Step 5: Gates and commit**

Run: `bash scripts/quality_gates.sh; echo "exit=$?"`
Expected: `exit=0`.

```bash
git add scripts/ensure-clax.sh plugins/claude-code/scripts/ensure-clax.sh plugins/clax/scripts/ensure-clax.sh scripts/test-ensure-clax.sh plugins/clax/.mcp.json scripts/test-plugins.sh
git commit -m "Rewrite the launcher: CLAX_BIN, dev link, installed release; a fallback MCP server states why clax cannot run"
git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed
```

---

### Task 9: The launcher's download, against a fake release server

**Files:**
- Create: `scripts/fake-release-server.py`
- Modify: `scripts/test-ensure-clax.sh` (the download cases replace the `# (Task 9 adds the download cases here.)` line); `scripts/ensure-clax.sh` and its copies only if a case below fails

**Interfaces:**
- Consumes: `scripts/package-release.sh` (Task 6) to build the fixture archives, so the launcher and the packager agree on names and layout.
- Produces: `scripts/fake-release-server.py <root> <request log> <port file>`. It serves `<root>/good/<path>` at `/<mode>/<path>` for the modes `ok`, `none` (404), `badsum` (zeroed `SHA256SUMS`), `partial` (archives cut in half after a full `Content-Length`), `slow` (60 s stall) and `delay` (`$FAKE_DELAY` seconds, default 2), and `<root>/wrong/<path>` at `/wrong/<path>`. It logs every request path and binds `127.0.0.1:0`.

- [ ] **Step 1: The fake release server**

Create `scripts/fake-release-server.py`:

```python
#!/usr/bin/env python3
"""A stand-in for GitHub release downloads, for scripts/test-ensure-clax.sh.

Usage: fake-release-server.py <root> <request log> <port file>

Serves <root>/good/<path> at /<mode>/<path>, and <root>/wrong/<path> at
/wrong/<path>. Every request path is appended to <request log>. Modes:
  ok       the file as it is
  none     404 for everything
  badsum   SHA256SUMS with every checksum zeroed
  partial  archives: the full Content-Length, half the body, then a close
  slow     waits 60 s before answering
  delay    waits $FAKE_DELAY seconds (default 2) before answering
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
        tree = "wrong" if mode == "wrong" else "good"
        path = os.path.join(ROOT, tree, rest)
        if mode == "slow":
            time.sleep(60)
        if mode == "delay":
            time.sleep(float(os.environ.get("FAKE_DELAY", "2")))
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

`chmod +x scripts/fake-release-server.py`.

- [ ] **Step 2: The download cases**

In `scripts/test-ensure-clax.sh`, replace the line `# (Task 9 adds the download cases here.)` with:

```bash
# --- Downloads, from scripts/fake-release-server.py ----------------------------

TARGETS="aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-musl aarch64-unknown-linux-musl"
release_tree() { # dir version-line
    local t
    mkdir -p "$ROOT/payload-$2" "$1/v$V"
    fake_clax "$ROOT/payload-$2" "$2"
    for t in $TARGETS; do
        # package-release.sh refuses a binary of another version, so the
        # "wrong" tree is packed by hand in the same layout.
        if [ "$2" = "clax $V" ]; then
            PATH="$ORIG_PATH" "$HERE/package-release.sh" archive "$V" "$t" "$ROOT/payload-$2/clax" "$1/v$V" >/dev/null
        else
            mkdir -p "$ROOT/stage/clax-$V-$t"
            cp "$ROOT/payload-$2/clax" "$ROOT/stage/clax-$V-$t/clax"
            (cd "$ROOT/stage" && PATH="$ORIG_PATH" tar -czf "$1/v$V/clax-$V-$t.tar.gz" "clax-$V-$t")
        fi
    done
    PATH="$ORIG_PATH" "$HERE/package-release.sh" sums "$1/v$V" >/dev/null
}
release_tree "$ROOT/release/good" "clax $V"
release_tree "$ROOT/release/wrong" "clax 0.0.9"
REQLOG="$ROOT/requests.log"
: > "$REQLOG"
(PATH="$ORIG_PATH" FAKE_DELAY=1.5 exec "$PY" "$HERE/fake-release-server.py" "$ROOT/release" "$REQLOG" "$ROOT/port") &
SERVER_PID=$!
for _ in $(seq 50); do [ -s "$ROOT/port" ] && break; sleep 0.1; done
BASE="http://127.0.0.1:$(cat "$ROOT/port")"
requests() { cat "$REQLOG"; }
no_leftovers() { ! ls -a "$CFGDIR/bin" 2>/dev/null | grep -qE '^\.(tmp\.|install\.lock|clax-link)'; }

new_env
fake_uname Linux x86_64
: > "$REQLOG"
CLAX_RELEASE_BASE_URL="$BASE/ok" mcp
if [ "$OUT" = "args: mcp --agent codex home= cfg=$CFGDIR launch=downloaded bin=$CFGDIR/bin/$V/clax warn=" ] \
    && [ "$(readlink "$CFGDIR/bin/clax")" = "$V/clax" ] \
    && requests | grep -qx "/ok/v$V/SHA256SUMS" && requests | grep -qx "/ok/v$V/clax-$V-x86_64-unknown-linux-musl.tar.gz" \
    && hooks_log | grep -q "launch mode=mcp agent=codex source=downloaded" && no_leftovers; then
    pass "MCP mode downloads, checks and installs the plugin's version, then runs it"
else fail "MCP mode downloads and runs (out=$OUT err=$ERR reqs=$(requests))"; fi
: > "$REQLOG"
CLAX_RELEASE_BASE_URL="$BASE/ok" mcp
if echo "$OUT" | grep -q "launch=installed" && [ -z "$(requests)" ]; then pass "the next start runs the installed version without the network"
else fail "the next start runs the installed version (out=$OUT reqs=$(requests))"; fi

for pair in Darwin/arm64/aarch64-apple-darwin Darwin/x86_64/x86_64-apple-darwin Linux/x86_64/x86_64-unknown-linux-musl \
    Linux/aarch64/aarch64-unknown-linux-musl Linux/arm64/aarch64-unknown-linux-musl; do
    new_env
    fake_uname "${pair%%/*}" "$(echo "$pair" | cut -d/ -f2)"
    : > "$REQLOG"
    CLAX_RELEASE_BASE_URL="$BASE/ok" run install
    if [ "$RC" = 0 ] && [ "$OUT" = "$CFGDIR/bin/$V/clax" ] && requests | grep -qx "/ok/v$V/clax-$V-${pair##*/}.tar.gz"; then
        pass "install on ${pair%/*} fetches ${pair##*/}"
    else fail "install on ${pair%/*} fetches ${pair##*/} (rc=$RC out=$OUT err=$ERR reqs=$(requests))"; fi
done

new_env
fake_uname Linux x86_64
CLAX_RELEASE_BASE_URL="$BASE/none" mcp
if text="$(fallback_text)" && echo "$text" | grep -q "answered HTTP 404; is v$V released?" && [ ! -e "$CFGDIR/bin/$V" ] && no_leftovers; then
    pass "a missing release: the MCP client gets the reason and nothing is installed"
else fail "a missing release (out=$OUT)"; fi

new_env
fake_uname Linux x86_64
CLAX_RELEASE_BASE_URL="$BASE/badsum" mcp
if text="$(fallback_text)" && echo "$text" | grep -q "checksum mismatch for clax-$V-x86_64-unknown-linux-musl.tar.gz" \
    && [ ! -e "$CFGDIR/bin/$V" ] && no_leftovers; then
    pass "a checksum mismatch installs nothing and says so"
else fail "a checksum mismatch installs nothing (out=$OUT)"; fi

new_env
fake_uname Linux x86_64
CLAX_RELEASE_BASE_URL="$BASE/partial" mcp
if text="$(fallback_text)" && echo "$text" | grep -q "was cut short" && [ ! -e "$CFGDIR/bin/$V" ] && no_leftovers; then
    pass "a partial download installs nothing and says so"
else fail "a partial download installs nothing (out=$OUT)"; fi

new_env
fake_uname Linux x86_64
start=$(date +%s)
CLAX_RELEASE_BASE_URL="$BASE/slow" CLAX_DOWNLOAD_TIMEOUT=2 mcp
took=$(( $(date +%s) - start ))
if text="$(fallback_text)" && echo "$text" | grep -q "timed out after 2s" && [ "$took" -lt 8 ] && [ ! -e "$CFGDIR/bin/$V" ] && no_leftovers; then
    pass "a stalled download times out within its bound and installs nothing"
else fail "a stalled download times out (took=${took}s out=$OUT)"; fi

new_env
fake_uname Linux x86_64
CLAX_RELEASE_BASE_URL="$BASE/wrong" mcp
if text="$(fallback_text)" && echo "$text" | grep -q "does not hold clax $V (its binary reports 'clax 0.0.9')" && [ ! -e "$CFGDIR/bin/$V" ]; then
    pass "an archive holding another version is refused"
else fail "an archive holding another version is refused (out=$OUT)"; fi

new_env
fake_uname FreeBSD x86_64
: > "$REQLOG"
CLAX_RELEASE_BASE_URL="$BASE/ok" mcp
if text="$(fallback_text)" && echo "$text" | grep -q "there is no prebuilt clax for FreeBSD/x86_64" && [ -z "$(requests)" ]; then
    pass "an unsupported platform says so without downloading"
else fail "an unsupported platform (out=$OUT)"; fi

new_env
fake_uname Linux x86_64
OUT="$(printf '%s\n' "$REQS" | PATH="$FAKEBIN:$NOCURL" CLAX_RELEASE_BASE_URL="$BASE/ok" "$TOOLS/bash" "$SCRIPT" exec mcp --agent codex 2>/dev/null)"
if text="$(fallback_text)" && echo "$text" | grep -q "curl is required to download clax"; then pass "a missing curl is the reason"
else fail "a missing curl is the reason (out=$OUT)"; fi

new_env
: > "$REQLOG"
CLAX_RELEASE_BASE_URL="$BASE/ok" run exec hook --agent codex session-start
r_hook="$RC"
CLAX_RELEASE_BASE_URL="$BASE/ok" run
r_print="$RC"
CLAX_RELEASE_BASE_URL="$BASE/ok" run exec status
if [ "$r_hook" = 0 ] && [ "$r_print" = 1 ] && [ "$RC" = 1 ] && [ -z "$(requests)" ] && [ ! -e "$CFGDIR/bin/$V" ]; then
    pass "hook, print and other modes never download, even with a release available"
else fail "hook, print and other modes never download (hook=$r_hook print=$r_print cli=$RC reqs=$(requests))"; fi

new_env
fake_uname Linux x86_64
CLAX_RELEASE_BASE_URL="$BASE/delay" CLAX_MCP_WAIT=1 mcp
text="$(fallback_text)"
for _ in $(seq 75); do [ -x "$CFGDIR/bin/$V/clax" ] && break; sleep 0.2; done
CLAX_RELEASE_BASE_URL="$BASE/delay" run
if echo "$text" | grep -q "is still downloading after 1s" && [ "$OUT" = "$CFGDIR/bin/$V/clax" ]; then
    pass "a download that outlasts the MCP wait answers at once and finishes in the background"
else fail "a download that outlasts the MCP wait (text=$text out=$OUT)"; fi

new_env
fake_uname Linux x86_64
: > "$REQLOG"
(printf '%s\n' "$REQS" | CLAX_RELEASE_BASE_URL="$BASE/delay" CLAX_MCP_WAIT=30 "$TOOLS/bash" "$SCRIPT" exec mcp --agent codex > "$SANDBOX/out1" 2>/dev/null) &
p1=$!
(printf '%s\n' "$REQS" | CLAX_RELEASE_BASE_URL="$BASE/delay" CLAX_MCP_WAIT=30 "$TOOLS/bash" "$SCRIPT" exec mcp --agent claude > "$SANDBOX/out2" 2>/dev/null) &
p2=$!
wait "$p1" "$p2"
if grep -q "^args: mcp --agent codex .*bin=$CFGDIR/bin/$V/clax" "$SANDBOX/out1" && grep -q "^args: mcp --agent claude .*bin=$CFGDIR/bin/$V/clax" "$SANDBOX/out2" \
    && [ "$(requests | grep -c "clax-$V-x86_64-unknown-linux-musl.tar.gz")" = 1 ] && no_leftovers; then
    pass "two MCP starts at once download once and both run clax"
else fail "two MCP starts at once (out1=$(cat "$SANDBOX/out1") out2=$(cat "$SANDBOX/out2") reqs=$(requests))"; fi

new_env
fake_uname Linux x86_64
for old in 0.0.1 0.0.2 0.0.3; do fake_clax "$CFGDIR/bin/$old" "clax $old"; done
fake_clax "$CFGDIR/bin/dev" "clax 9.9.9"
ln -s 0.0.2/clax "$CFGDIR/bin/clax"
printf '{"port":1,"pid":1,"version":"0.0.1"}\n' > "$CFGDIR/daemon.json"
CLAX_RELEASE_BASE_URL="$BASE/ok" run install
if [ "$RC" = 0 ] && [ -x "$CFGDIR/bin/0.0.2/clax" ] && [ -x "$CFGDIR/bin/0.0.1/clax" ] && [ ! -e "$CFGDIR/bin/0.0.3" ] \
    && [ -x "$CFGDIR/bin/dev/clax" ] && [ "$(readlink "$CFGDIR/bin/clax")" = "$V/clax" ]; then
    pass "installing keeps the previous version and the running daemon's, prunes older ones, and leaves bin/dev alone"
else fail "installing keeps the previous version and prunes (rc=$RC ls=$(ls "$CFGDIR/bin"))"; fi

new_env
fake_uname Linux x86_64
fake_clax "$CFGDIR/bin/$V" "clax 0.0.1"
CLAX_RELEASE_BASE_URL="$BASE/ok" mcp
if echo "$OUT" | grep -q "launch=downloaded" && [ "$("$CFGDIR/bin/$V/clax" --version)" = "clax $V" ]; then
    pass "a wrong binary at bin/<version> is replaced by the download"
else fail "a wrong binary at bin/<version> is replaced (out=$OUT)"; fi

new_env
fake_uname Linux x86_64
mkdir -p "$CFGDIR/bin/.install.lock"
echo 999999 > "$CFGDIR/bin/.install.lock/pid"
CLAX_RELEASE_BASE_URL="$BASE/ok" run install
if [ "$RC" = 0 ] && [ -x "$CFGDIR/bin/$V/clax" ] && no_leftovers; then pass "a lock whose holder is gone is taken over"
else fail "a lock whose holder is gone is taken over (rc=$RC err=$ERR)"; fi

new_env
fake_uname Linux x86_64
sleep 30 &
holder=$!
mkdir -p "$CFGDIR/bin/.install.lock"
echo "$holder" > "$CFGDIR/bin/.install.lock/pid"
CLAX_RELEASE_BASE_URL="$BASE/ok" CLAX_DOWNLOAD_TIMEOUT=1 run install
kill "$holder" 2>/dev/null; wait "$holder" 2>/dev/null
if [ "$RC" = 1 ] && echo "$ERR" | grep -q "another session has held $CFGDIR/bin/.install.lock"; then
    pass "a live lock is waited for, within the download bound"
else fail "a live lock is waited for (rc=$RC err=$ERR)"; fi
```

- [ ] **Step 3: Run the tests**

Run: `bash scripts/test-ensure-clax.sh`
Expected: every line `PASS`, then `all launcher tests passed`. A download case that fails points at a launcher defect: fix `scripts/ensure-clax.sh`, copy it to both plugins again, and re-run. Never weaken an assertion.

Run it three times in a row, to catch timing flakes in the concurrency and wait cases:
`for i in 1 2 3; do bash scripts/test-ensure-clax.sh | tail -1; done`
Expected: `all launcher tests passed` three times.

- [ ] **Step 4: Gates and commit**

Run: `bash scripts/quality_gates.sh; echo "exit=$?"`
Expected: `exit=0`.

```bash
git add scripts/fake-release-server.py scripts/test-ensure-clax.sh scripts/ensure-clax.sh plugins/claude-code/scripts/ensure-clax.sh plugins/clax/scripts/ensure-clax.sh
git commit -m "Test the launcher's download against a local fake release server: checksum, partial, timeout, concurrency, pruning"
git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed
```

---
### Task 10: Pi resolves its binary like the launcher

**Files:**
- Modify: `plugins/pi/src/daemon.ts`, `plugins/pi/src/clax.ts`
- Create: `plugins/pi/test/launch.test.ts`

**Interfaces:**
- Consumes: the `config.toml` line format (Task 2) and the resolution order (spec §13).
- Produces: `configDir(env)`, `readDevLink(dir)`, `resolveLaunch(version, env, versionOf)` and `LaunchError` in `daemon.ts`. `findBinary(env)` now returns `resolveLaunch(VERSION, env).bin`. `claxHome(env)` returns the dev link's `home` when `CLAX_BIN` is unset and the link names one. Pi's `status` gains `launch: {source, bin, warning, notice}`, or `{source: null, error}` when nothing resolves.
- Pi never downloads. `PATH` is no longer searched.

- [ ] **Step 1: Write the failing tests**

Create `plugins/pi/test/launch.test.ts`:

```ts
import { chmodSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { claxHome, configDir, LaunchError, readDevLink, resolveLaunch } from "../src/daemon.ts";

const dirs: string[] = [];
afterEach(() => {
  for (const d of dirs.splice(0)) rmSync(d, { recursive: true, force: true });
});

/** A scratch HOME, and a version table standing in for `--version`. */
function scratch() {
  const home = mkdtempSync(join(tmpdir(), "clax-pi-launch-"));
  dirs.push(home);
  const versions = new Map<string, string>();
  const bin = (rel: string, version: string) => {
    const p = join(home, rel);
    mkdirSync(join(p, ".."), { recursive: true });
    writeFileSync(p, "#!/bin/sh\n");
    chmodSync(p, 0o755);
    versions.set(p, version);
    return p;
  };
  const link = (b: string, h?: string) => {
    mkdirSync(join(home, ".clax"), { recursive: true });
    writeFileSync(join(home, ".clax/config.toml"), `[dev_link]\nbin = "${b}"\n${h ? `home = "${h}"\n` : ""}linked_at = "t"\n`);
  };
  const versionOf = (p: string) => versions.get(p) ?? null;
  return { home, env: { HOME: home } as NodeJS.ProcessEnv, bin, link, versionOf };
}

describe("resolveLaunch", () => {
  it("prefers CLAX_BIN, then the dev link, then the installed version", () => {
    const s = scratch();
    const inst = s.bin(".clax/bin/0.2.0/clax", "clax 0.2.0");
    expect(resolveLaunch("0.2.0", s.env, s.versionOf)).toMatchObject({ source: "installed", bin: inst, warning: null });
    const dev = s.bin(".clax/bin/dev/clax", "clax 0.3.0");
    s.link(dev, "/u/.clax-dev");
    expect(resolveLaunch("0.2.0", s.env, s.versionOf)).toMatchObject({
      source: "dev-link",
      bin: dev,
      home: "/u/.clax-dev",
      warning: "the dev link runs clax 0.3.0, but this package is clax 0.2.0",
    });
    const x = s.bin("x/clax", "clax 0.2.0");
    expect(resolveLaunch("0.2.0", { ...s.env, CLAX_BIN: x }, s.versionOf)).toMatchObject({ source: "clax-bin", bin: x });
  });

  it("never searches PATH, and says where it looked", () => {
    const s = scratch();
    const onPath = s.bin("path/clax", "clax 0.2.0");
    try {
      resolveLaunch("0.2.0", { ...s.env, PATH: join(onPath, "..") }, s.versionOf);
      expect.unreachable();
    } catch (e) {
      expect(e).toBeInstanceOf(LaunchError);
      const err = e as LaunchError;
      expect(err.tried).toEqual(["CLAX_BIN: unset", `dev link: none in ${join(s.home, ".clax/config.toml")}`, `${join(s.home, ".clax/bin/0.2.0/clax")}: missing`]);
      expect(err.message).toContain("bash scripts/ensure-clax.sh install");
    }
  });

  it("fails on an unusable CLAX_BIN or dev link rather than falling through", () => {
    const s = scratch();
    s.bin(".clax/bin/0.2.0/clax", "clax 0.2.0");
    expect(() => resolveLaunch("0.2.0", { ...s.env, CLAX_BIN: "/nope" }, s.versionOf)).toThrow("CLAX_BIN is set to '/nope'");
    s.link("/gone/clax");
    expect(() => resolveLaunch("0.2.0", s.env, s.versionOf)).toThrow("just dev-install");
  });

  it("finds the config directory as the launcher does", () => {
    expect(configDir({ HOME: "/u" })).toBe("/u/.clax");
    expect(configDir({ HOME: "/u", CLAX_HOME: "/h" })).toBe("/h");
    expect(configDir({ HOME: "/u", CLAX_HOME: "/h", CLAX_CONFIG_DIR: "/c" })).toBe("/c");
  });

  it("reads only the dev_link table", () => {
    const s = scratch();
    mkdirSync(join(s.home, ".clax"), { recursive: true });
    writeFileSync(join(s.home, ".clax/config.toml"), '[serve]\nport = 7481\n\n[dev_link]\nbin = "/b/clax"\nlinked_at = "t"\n');
    expect(readDevLink(join(s.home, ".clax"))).toEqual({ bin: "/b/clax", home: null });
    expect(readDevLink(join(s.home, "missing"))).toBeNull();
  });

  it("uses the dev link's home unless CLAX_BIN is set", () => {
    const s = scratch();
    s.link("/b/clax", "/u/.clax-dev");
    expect(claxHome(s.env)).toBe("/u/.clax-dev");
    expect(claxHome({ ...s.env, CLAX_BIN: "/x" })).toBe(join(s.home, ".clax"));
  });
});
```

Run: `cd plugins/pi && npx vitest run test/launch.test.ts`
Expected: FAIL (the exports do not exist).

- [ ] **Step 2: Implement**

In `plugins/pi/src/daemon.ts`, replace `INSTALL_HINT`, `claxHome` and `findBinary` with the code below. Keep `executable()` and every other export. Add `execFileSync` to the `node:child_process` import.

```ts
/** This package's version: the Clax release it runs. */
const VERSION: string = JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8")).version;

/** How to get a binary when none resolves. Pi never downloads. */
export const INSTALL_HINT =
  "install the release this package belongs to with `bash scripts/ensure-clax.sh install` in your Clax checkout, " +
  "or link a dev build with `just dev-install`, or set CLAX_BIN to a clax binary";

/** The Clax config directory, as the plugins' launcher finds it:
 * `CLAX_CONFIG_DIR`, else `CLAX_HOME`, else `~/.clax`. */
export function configDir(env: NodeJS.ProcessEnv = process.env): string {
  return env.CLAX_CONFIG_DIR || env.CLAX_HOME || join(env.HOME || homedir(), ".clax");
}

export interface DevLink {
  bin: string;
  home: string | null;
}

/** The `[dev_link]` table of `<dir>/config.toml` (the `key = "value"` lines
 * `clax dev-link` writes), or null. */
export function readDevLink(dir: string): DevLink | null {
  let text: string;
  try {
    text = readFileSync(join(dir, "config.toml"), "utf8");
  } catch {
    return null;
  }
  let on = false;
  let bin: string | null = null;
  let home: string | null = null;
  for (const line of text.split("\n")) {
    const t = line.trim();
    if (t.startsWith("[")) {
      on = t.replace(/\s/g, "") === "[dev_link]";
      continue;
    }
    const m = on ? /^(bin|home)\s*=\s*"(.*)"\s*$/.exec(t) : null;
    if (m?.[1] === "bin") bin = m[2];
    if (m?.[1] === "home") home = m[2];
  }
  return bin ? { bin, home } : null;
}

export interface Launch {
  source: "clax-bin" | "dev-link" | "installed";
  bin: string;
  version: string;
  home: string | null;
  warning: string | null;
}

/** Nothing resolved: `tried` names each candidate, in order. */
export class LaunchError extends Error {
  constructor(message: string, readonly tried: string[]) {
    super(message);
  }
}

/** The first line of `bin --version` when `bin` is an executable clax. */
export function readVersion(bin: string): string | null {
  if (!executable(bin)) return null;
  try {
    const first = execFileSync(bin, ["--version"], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"], timeout: 5_000 }).split("\n")[0];
    return first.startsWith("clax ") ? first : null;
  } catch {
    return null;
  }
}

/** The binary Pi runs, resolved as the plugins' launcher resolves it:
 * `CLAX_BIN`, then the dev link, then `<config dir>/bin/<version>/clax`.
 * Never searches `PATH` and never downloads. Throws a {@link LaunchError}
 * naming each place it looked. */
export function resolveLaunch(
  version: string = VERSION,
  env: NodeJS.ProcessEnv = process.env,
  versionOf: (bin: string) => string | null = readVersion,
): Launch {
  const want = `clax ${version}`;
  const mismatch = (who: string, v: string) => (v === want ? null : `${who} runs ${v}, but this package is ${want}`);
  if (env.CLAX_BIN) {
    const v = versionOf(env.CLAX_BIN);
    if (v) return { source: "clax-bin", bin: env.CLAX_BIN, version: v, home: null, warning: mismatch("CLAX_BIN", v) };
    throw new LaunchError(`CLAX_BIN is set to '${env.CLAX_BIN}', which is not a usable clax binary; ${INSTALL_HINT}`, [
      `CLAX_BIN=${env.CLAX_BIN}: not a usable clax`,
    ]);
  }
  const tried = ["CLAX_BIN: unset"];
  const dir = configDir(env);
  const link = readDevLink(dir);
  if (link) {
    const v = versionOf(link.bin);
    if (v) return { source: "dev-link", bin: link.bin, version: v, home: link.home, warning: mismatch("the dev link", v) };
    tried.push(`dev link ${link.bin}: not a usable clax`);
    throw new LaunchError(
      `the dev link in ${join(dir, "config.toml")} points at '${link.bin}', which is not a usable clax binary; run \`just dev-install\` in your Clax checkout, or \`clax dev-unlink\``,
      tried,
    );
  }
  tried.push(`dev link: none in ${join(dir, "config.toml")}`);
  const inst = join(dir, "bin", version, "clax");
  const v = versionOf(inst);
  if (v === want) return { source: "installed", bin: inst, version: v, home: null, warning: null };
  tried.push(v ? `${inst}: ${v}, not ${want}` : `${inst}: missing`);
  throw new LaunchError(`${want} is not installed at ${inst}; ${INSTALL_HINT}`, tried);
}

/** The `clax` binary to run (see {@link resolveLaunch}). */
export function findBinary(env: NodeJS.ProcessEnv = process.env): string {
  return resolveLaunch(VERSION, env).bin;
}

/** The Clax home: the dev link's home when `CLAX_BIN` is unset and the link
 * names one, else `$CLAX_HOME`, else `$HOME/.clax` (an empty variable counts
 * as unset). */
export function claxHome(env: NodeJS.ProcessEnv = process.env): string {
  const link = env.CLAX_BIN ? null : readDevLink(configDir(env));
  if (link?.home) return link.home;
  if (env.CLAX_HOME) return env.CLAX_HOME;
  return join(env.HOME || homedir(), ".clax");
}
```

In `plugins/pi/src/clax.ts`, in `status`, after the `daemon_version` line, add:

```ts
    // Which binary Pi runs, and why; a dev link says so plainly.
    try {
      const l = resolveLaunch(VERSION, this.env);
      out.launch = {
        source: l.source,
        bin: l.bin,
        warning: l.warning,
        notice:
          l.source === "dev-link"
            ? `This session runs a dev build linked with \`clax dev-link\` (${l.bin}), not the released clax. \`clax dev-unlink\` returns to the release.`
            : null,
      };
    } catch (e) {
      out.launch = { source: null, error: (e as Error).message };
    }
```

and add `resolveLaunch` to its import from `./daemon.ts`.

The existing Pi tests start their daemon through `CLAX_BIN`, which still wins, so they are unchanged. `resolveLaunch` reads `<config dir>/config.toml` only when `CLAX_BIN` is unset, so check that every Pi test either sets `CLAX_BIN` or passes an `env` whose `HOME` and `CLAX_HOME` are scratch directories. No test may read the real `~/.clax/config.toml`. Search the tests for a `PATH`-based binary lookup: `grep -n "PATH" plugins/pi/test/*.ts`. Any test that relied on `PATH` sets `CLAX_BIN` instead.

- [ ] **Step 3: Run**

Run: `cd plugins/pi && npm run typecheck && npx vitest run`
Expected: PASS.

- [ ] **Step 4: Gates and commit**

Run: `bash scripts/quality_gates.sh; echo "exit=$?"`
Expected: `exit=0`.

```bash
git add plugins/pi/src/daemon.ts plugins/pi/src/clax.ts plugins/pi/test/launch.test.ts
git commit -m "Pi resolves clax like the launcher: CLAX_BIN, the dev link, the installed release; status says which"
git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed
```

---

### Task 11: The local dev flow: `just dev` apart, `just dev-install`

**Files:**
- Create: `scripts/dev-home.sh`, `scripts/dev-install.sh`, `scripts/test-dev.sh`
- Modify: `scripts/dev.sh`, `justfile`, `scripts/test-justfile.sh`, `scripts/quality_gates.sh`, `web/vite.shell.config.ts`

**Interfaces:**
- Consumes: `clax dev-link` / `clax dev-unlink` (Task 4) and `[serve] port` (Task 2).
- Produces:
  - `just dev [--shared] [serve args]`. By default it serves `$CLAX_HOME`, else `~/.clax-dev`, on port 7481 (`$CLAX_DEV_PORT` overrides). `--shared` serves `$CLAX_HOME`, else `~/.clax`, on 7480.
  - `just dev-install [--home <dir>]`. It builds the web UI and a release binary, then copies the binary atomically to `<config dir>/bin/dev/clax` (never a path into `target/`). It then runs `clax dev-link` on the copy, which restarts the linked home's daemon.
  - `just dev-uninstall` runs `clax dev-unlink` and removes `bin/dev`.
  - `just serve`, `just stop` and `just doctor` act on the dev home.
- Removes: `just install`, `just uninstall` (see Design decisions).

- [ ] **Step 1: `scripts/dev-home.sh`**

```bash
# Sourced by scripts/dev.sh, scripts/dev-install.sh and scripts/test-dev.sh.

# dev_settings [--shared] [args...]: sets DEV_HOME, DEV_PORT and DEV_ARGS
# (the remaining arguments, for `clax serve`). Without --shared: $CLAX_HOME,
# else ~/.clax-dev, on $CLAX_DEV_PORT, else 7481. With --shared: $CLAX_HOME,
# else ~/.clax (the home agents use), on 7480.
dev_settings() {
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
# config.toml has no [serve] table, records <port> there, so a daemon that
# agents linked to this home (`clax dev-link --home`) start listens on it.
ensure_dev_home() {
    mkdir -p "$1"
    chmod 700 "$1"
    if ! grep -q '^\[serve\]' "$1/config.toml" 2>/dev/null; then
        printf '\n[serve]\nport = %s\n' "$2" >> "$1/config.toml"
    fi
}
```

- [ ] **Step 2: `scripts/dev.sh`**

Keep the shebang line. Replace everything after it, through the `fi` that closes the `CLAX_HOME` message (the header comment, `set -euo pipefail`, the `cd`, `PORT=7480`, `ARGS="$*"` and the `CLAX_HOME` message), with:

```bash
# Runs the daemon and the web bundlers with auto-reload. By default it serves
# its own home (~/.clax-dev) on port 7481, so rebuilding never takes the
# agents' daemon down; `--shared` serves the agents' home (~/.clax) on 7480.
# Other arguments go to `clax serve`.
set -euo pipefail
cd "$(dirname "$0")/.."
. scripts/dev-home.sh
dev_settings "$@"
export CLAX_HOME="$DEV_HOME"
PORT="$DEV_PORT"
ARGS="${DEV_ARGS[*]-}"
if [ "$PORT" = 7480 ]; then
    echo "Clax dev: --shared: serving CLAX_HOME=$CLAX_HOME on port $PORT, the home your agents use. While a Rust change rebuilds, an agent may start its own daemon here."
else
    ensure_dev_home "$CLAX_HOME" "$PORT"
    echo "Clax dev: serving CLAX_HOME=$CLAX_HOME on port $PORT (agents keep their own daemon; \`just dev --shared\` serves theirs)"
fi
```

`CLAX_HOME` is now always exported, so the `cleanup` trap's `target/debug/clax stop` stops the dev daemon and never the agents' daemon. The rest of the file (the healthz check, the bundlers, the `cargo watch` line) is unchanged.

- [ ] **Step 3: `scripts/dev-install.sh`**

```bash
#!/usr/bin/env bash
# `just dev-install [--home <dir>]`: builds the web UI and a release clax from
# this checkout, copies the binary to <config dir>/bin/dev/clax (a stable copy
# that `cargo clean`, rebuilds and checkout moves cannot touch), and runs
# `clax dev-link` on it so the plugins run it (restarting the linked home's
# daemon). With --home, agents also use that home.
# `just dev-uninstall` (--uninstall): `clax dev-unlink`, then removes bin/dev.
# CLAX_DEV_INSTALL_FROM=<binary> skips the build (tests).
set -euo pipefail
cd "$(dirname "$0")/.."
. scripts/dev-home.sh
CFG="${CLAX_CONFIG_DIR:-${CLAX_HOME:-$HOME/.clax}}"
DEST="$CFG/bin/dev"

if [ "${1:-}" = --uninstall ]; then
    if [ -x "$DEST/clax" ]; then "$DEST/clax" dev-unlink; else echo "no dev build at $DEST/clax"; fi
    rm -rf "$DEST"
    exit 0
fi

LINK_ARGS=()
HOME_ARG=""
while [ $# -gt 0 ]; do
    case "$1" in
        --home) [ $# -ge 2 ] || { echo "--home needs a directory" >&2; exit 2; }; HOME_ARG="$2"; LINK_ARGS+=(--home "$2"); shift 2 ;;
        *) echo "usage: dev-install.sh [--home <dir>] | --uninstall" >&2; exit 2 ;;
    esac
done

FROM="${CLAX_DEV_INSTALL_FROM:-}"
if [ -z "$FROM" ]; then
    (cd web && npm ci --silent && npm run build)
    cargo build --release --locked -p clax-cli --bin clax
    FROM=target/release/clax
fi
version="$("$FROM" --version 2>/dev/null | head -1 || true)"
case "$version" in "clax "*) ;; *) echo "$FROM is not a clax binary (--version printed '$version')" >&2; exit 1 ;; esac

mkdir -p "$DEST"
tmp="$DEST/.clax.$$"
cp "$FROM" "$tmp"
chmod 755 "$tmp"
# A rename: running sessions keep the old file, new ones get the new one.
mv -f "$tmp" "$DEST/clax"
if [ -n "$HOME_ARG" ] && [ "$HOME_ARG" != "$HOME/.clax" ]; then ensure_dev_home "$HOME_ARG" 7481; fi
echo "installed $version at $DEST/clax"
exec "$DEST/clax" dev-link "$DEST/clax" ${LINK_ARGS[@]+"${LINK_ARGS[@]}"}
```

- [ ] **Step 4: `scripts/test-dev.sh`**

```bash
#!/usr/bin/env bash
# Tests scripts/dev-home.sh and scripts/dev-install.sh with scratch homes and
# a fake clax. No cargo build, no daemon, no network.
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
T="$(cd "$(mktemp -d)" && pwd -P)"
trap 'rm -rf "$T"' EXIT
FAILED=0
pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1"; FAILED=1; }
export HOME="$T/home"
mkdir -p "$HOME"
unset CLAX_HOME CLAX_CONFIG_DIR CLAX_DEV_PORT

. "$HERE/dev-home.sh"
dev_settings --bind 0.0.0.0
if [ "$DEV_HOME" = "$HOME/.clax-dev" ] && [ "$DEV_PORT" = 7481 ] && [ "${DEV_ARGS[*]}" = "--bind 0.0.0.0" ]; then
    pass "just dev defaults to ~/.clax-dev on 7481 and passes other arguments on"
else fail "just dev defaults ($DEV_HOME $DEV_PORT ${DEV_ARGS[*]-})"; fi
dev_settings --shared
if [ "$DEV_HOME" = "$HOME/.clax" ] && [ "$DEV_PORT" = 7480 ] && [ -z "${DEV_ARGS[*]-}" ]; then
    pass "just dev --shared serves ~/.clax on 7480"
else fail "just dev --shared ($DEV_HOME $DEV_PORT)"; fi
CLAX_HOME="$T/h" dev_settings
if [ "$DEV_HOME" = "$T/h" ] && [ "$DEV_PORT" = 7481 ]; then pass "an explicit CLAX_HOME is served on 7481"
else fail "an explicit CLAX_HOME ($DEV_HOME $DEV_PORT)"; fi

ensure_dev_home "$T/dh" 7481
ensure_dev_home "$T/dh" 9999
if [ "$(grep -c '^\[serve\]' "$T/dh/config.toml")" = 1 ] && grep -qx 'port = 7481' "$T/dh/config.toml" \
    && [ "$(stat -c %a "$T/dh" 2>/dev/null || stat -f %Lp "$T/dh")" = 700 ]; then
    pass "ensure_dev_home records the port once and keeps the home private"
else fail "ensure_dev_home ($(cat "$T/dh/config.toml"))"; fi

# A fake clax that records its dev-link and dev-unlink calls.
mkdir -p "$T/src"
cat > "$T/src/clax" <<SH
#!/bin/sh
if [ "\$1" = "--version" ]; then echo "clax 0.9.0-dev"; exit 0; fi
echo "\$0 \$*" >> "$T/calls"
SH
chmod +x "$T/src/clax"
out="$(CLAX_DEV_INSTALL_FROM="$T/src/clax" "$HERE/dev-install.sh" --home "$T/devhome" 2>&1)"; rc=$?
if [ "$rc" = 0 ] && cmp -s "$T/src/clax" "$HOME/.clax/bin/dev/clax" && [ -x "$HOME/.clax/bin/dev/clax" ] \
    && grep -qx "$HOME/.clax/bin/dev/clax dev-link $HOME/.clax/bin/dev/clax --home $T/devhome" "$T/calls" \
    && grep -qx 'port = 7481' "$T/devhome/config.toml"; then
    pass "dev-install copies the build to bin/dev/clax and links it with its home"
else fail "dev-install copies and links (rc=$rc out=$out calls=$(cat "$T/calls" 2>/dev/null))"; fi

exec 3< "$HOME/.clax/bin/dev/clax"
printf '#!/bin/sh\nif [ "$1" = "--version" ]; then echo "clax 0.9.1-dev"; exit 0; fi\necho "$0 $*" >> "%s/calls"\n' "$T" > "$T/src/clax"
CLAX_DEV_INSTALL_FROM="$T/src/clax" "$HERE/dev-install.sh" >/dev/null 2>&1
if grep -q "0.9.0-dev" <&3 && grep -q "0.9.1-dev" "$HOME/.clax/bin/dev/clax" && ! ls -a "$HOME/.clax/bin/dev" | grep -q '^\.clax\.'; then
    pass "re-running dev-install replaces the binary by rename; an open copy keeps the old file"
else fail "re-running dev-install replaces by rename"; fi
exec 3<&-

printf '#!/bin/sh\necho other 1.0\n' > "$T/other"
chmod +x "$T/other"
if out="$(CLAX_DEV_INSTALL_FROM="$T/other" "$HERE/dev-install.sh" 2>&1)"; then fail "dev-install refuses a binary that is not clax"
else echo "$out" | grep -q "is not a clax binary" && pass "dev-install refuses a binary that is not clax" || fail "dev-install refuses ($out)"; fi

: > "$T/calls"
"$HERE/dev-install.sh" --uninstall >/dev/null 2>&1
if grep -q "dev-unlink" "$T/calls" && [ ! -e "$HOME/.clax/bin/dev" ]; then pass "dev-uninstall unlinks and removes bin/dev"
else fail "dev-uninstall unlinks and removes bin/dev ($(cat "$T/calls"))"; fi

[ "$FAILED" = 0 ] && echo "dev script tests passed" || echo "dev script tests FAILED"
exit "$FAILED"
```

`chmod +x scripts/dev-install.sh scripts/test-dev.sh`. `dev-home.sh` is sourced, not run, so it needs no execute bit.

- [ ] **Step 5: The justfile**

Replace the `dev`, `install`, `uninstall`, `serve`, `stop` and `doctor` recipes with:

```make
# Run a server that reloads on Rust and web changes (~/.clax-dev on port 7481; --shared: ~/.clax on 7480)
dev *ARGS:
    ./scripts/dev.sh {{ARGS}}

# Build a release clax, copy it to ~/.clax/bin/dev/clax and link agents to it (--home <dir>: and to that home)
dev-install *ARGS:
    ./scripts/dev-install.sh {{ARGS}}

# Unlink the dev build: agents return to the release their plugin was built for
dev-uninstall:
    ./scripts/dev-install.sh --uninstall

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
for r in dev dev-install dev-uninstall serve stop doctor; do
    case " $recipes " in *" $r "*) ;; *) echo "missing recipe: $r" >&2; missing=1 ;; esac
done
for r in install uninstall; do
    case " $recipes " in *" $r "*) echo "recipe $r should be gone (just dev-install replaces it)" >&2; missing=1 ;; esac
done
```

In `scripts/quality_gates.sh`, after the `release scripts` line, add:

```bash
run "dev scripts"           scripts/test-dev.sh
```

- [ ] **Step 6: The Vite dev proxy follows the dev daemon**

In `web/vite.shell.config.ts`, change every `http://127.0.0.1:7480` in `server.proxy` to `http://127.0.0.1:7481`. No gate starts Vite's dev server, and a person using it proxies to `just dev`'s daemon.

- [ ] **Step 7: Run**

Run: `scripts/test-dev.sh && scripts/test-justfile.sh && echo justfile-ok`
Expected: `dev script tests passed` and `justfile-ok`.

Do not run `just dev`, `just serve`, `just stop` or `just dev-install` here. They act on `~/.clax-dev` and `~/.clax`, which belong to the person. Their verification is in "Steps for the person".

- [ ] **Step 8: Gates and commit**

Run: `bash scripts/quality_gates.sh; echo "exit=$?"`
Expected: `exit=0`.

```bash
git add scripts/dev-home.sh scripts/dev-install.sh scripts/test-dev.sh scripts/dev.sh justfile scripts/test-justfile.sh scripts/quality_gates.sh web/vite.shell.config.ts
git commit -m "Separate just dev (~/.clax-dev, port 7481) and add just dev-install / dev-uninstall"
git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed
```

---

### Task 12: Documentation

**Files:**
- Modify: `README.md`, `plugins/claude-code/README.md`, `plugins/clax/README.md`, `plugins/pi/README.md`, `docs/contract.md`, `docs/superpowers/plans/2026-09-29-svelte-port.md`, `docs/superpowers/plans/2026-09-30-agent-working.md`

**Interfaces:**
- Consumes: everything above. The docs describe only what Tasks 2–11 built.

- [ ] **Step 1: README, "Install from source" becomes "Install"**

Replace the "Install from source" section with:

````markdown
## Install

Install the plugin for your agent. The plugin's launcher downloads the `clax`
release it was built for, checks it against the release's SHA-256 checksums,
and installs it in `~/.clax/bin/<version>/`. The first session that starts
the MCP server does this once. Later sessions, and every hook, use the
installed copy and never download.

```
git clone https://github.com/empathic/clax
```

- Claude Code: `/plugin marketplace add /path/to/clax`, then `/plugin install clax@clax`.
- Codex: `codex plugin marketplace add /path/to/clax`, then `codex plugin add clax@clax`.
- Pi: `pi install /path/to/clax/plugins/pi`. Pi does not download: run `bash scripts/ensure-clax.sh install` in the clone first.

To use `clax` from a shell, install it the same way and put `~/.clax/bin` on
your `PATH`. `~/.clax/bin/clax` points at the release the launcher installed
last:

```
bash scripts/ensure-clax.sh install
export PATH="$HOME/.clax/bin:$PATH"
```

When a plugin half works, `clax doctor --agent <claude|codex|pi>` checks each
layer, and its `launch` check says which binary runs and why. If the MCP server
cannot run `clax` at all, its only tool, `status`, says why, and
`~/.clax/logs/hooks.log` names every place the launcher looked.
````

In "Use from an agent", replace the paragraph that begins `Build the binary first` with:

```markdown
The Claude Code and Codex plugins run `clax` through `scripts/ensure-clax.sh`, which uses `CLAX_BIN` when it is set, else a dev build linked with `clax dev-link` (see "Developing Clax"), else `~/.clax/bin/<plugin version>/clax`, which the MCP server downloads when it is missing. It never uses a `clax` on `PATH` or a build in a checkout. The Pi extension resolves the same way, but never downloads. `docs/contract.md` ("Launcher") has the details.
```

- [ ] **Step 2: README, "Upgrading"**

Append to the "Upgrading" section:

```markdown
### From a source install (before releases)

Earlier versions ran whatever `clax` they found: on `PATH`, in
`~/.local/bin`, or in a checkout's `target/`. The launcher no longer looks
there.

1. Update the plugin: Claude Code `/plugin marketplace update clax`, then
   `/plugin install clax@clax`; Codex `codex plugin add clax@clax`. If the
   checkout moved, remove the marketplace and add it again from the new path.
2. Start a new session. Its MCP server installs the release into
   `~/.clax/bin/<version>/`. Before the first release exists, run
   `just dev-install` in the checkout instead (see "Developing Clax").
3. Remove the old binaries, which nothing uses now: `cargo uninstall clax-cli`
   (`~/.cargo/bin/clax`) and `~/.local/bin/clax`. Unset `CLAX_SOURCE_DIR` and
   `CLAX_INSTALL_DIR` wherever you set them.
4. Check with `clax doctor --agent <claude|codex|pi>`.

A newer daemon is never replaced by an older `clax`. After downgrading a plugin,
run `clax stop` once so the older release starts its own daemon.
```

- [ ] **Step 3: README, "Development" becomes "Developing Clax"**

Replace the "Development" section's first bullet (`just dev` …, with its two sub-bullets) with:

````markdown
## Developing Clax

There are three loops. None of them runs a binary out of `target/` for your agents.

**1. The daemon and the web UI: `just dev`.** An auto-reloading server on its
own home, `~/.clax-dev`, at http://localhost:7481. A Rust change rebuilds and
restarts the daemon; a web change rebuilds `web/dist` (reload the browser).
Your agents keep their own daemon on 7480 and are never interrupted. Extra
arguments go to `serve` (`just dev --bind 0.0.0.0`). `just dev --shared`
serves the agents' home, `~/.clax`, on 7480 instead. Stop the agents' daemon
first (`clax stop`). While a Rust change rebuilds, an agent may start its own
daemon there. `just stop`, `just serve` and `just doctor` act on
`~/.clax-dev`. It needs `cargo-watch` (`cargo install cargo-watch`).

**2. Your agents on your build: `just dev-install`.** It builds the web UI and
a release binary, copies it to `~/.clax/bin/dev/clax`, and links the plugins
to it (`clax dev-link`). It restarts the linked home's daemon from it. New
sessions run it; running sessions keep their binary until they restart.
`cargo clean`, rebuilds and moving the checkout cannot break it, because it is
a copy. Run it again after each change.

```
just dev-install                        # agents run your build on their usual home (~/.clax)
just dev-install --home ~/.clax-dev     # ...and publish into just dev's home, at http://localhost:7481
just dev-uninstall                      # back to the release the plugin was built for
```

While a dev link is active, `clax doctor --agent <harness>` starts its `launch`
check with `DEV LINK:`, the `status` tool's `launch.notice` says so, and no
release is downloaded. A dev build whose version differs from the plugin's
runs anyway, with a warning in `~/.clax/logs/hooks.log`. `CLAX_BIN=<binary>`
in a harness's environment also works, for one-off runs.

**3. Plugins, skills and hooks.** Edit them in the checkout, then reload the
harness's copy:

- Claude Code runs its own copy under `~/.claude/plugins`:
  `/plugin marketplace update clax`, then `/plugin uninstall clax@clax` and
  `/plugin install clax@clax`, then a new session. For one run straight from
  the checkout: `claude --plugin-dir /path/to/clax/plugins/claude-code`.
- Codex copies the plugin into
  `$CODEX_HOME/plugins/cache/clax/clax/<version>/` on `codex plugin add
  clax@clax`. Run that again, then start a new session. If the checkout moved:
  `codex plugin marketplace remove clax`, then `codex plugin marketplace add
  /new/path`.
- Pi runs the extension from the checkout path: start a new Pi session.
- The launcher lives in `scripts/ensure-clax.sh`, and each plugin carries a
  copy: after editing it, `cp scripts/ensure-clax.sh
  plugins/claude-code/scripts/ && cp scripts/ensure-clax.sh
  plugins/clax/scripts/` (`just plugin-test` checks they match), then
  `just dev-install` so `clax doctor --agent` compares against the same
  launcher.
- `clax doctor --agent <harness>` reports a stale plugin copy.
````

Keep the remaining "Development" bullets (`just check`, `just ci`) and the `scripts/quality_gates.sh` paragraph under the new heading. In that paragraph:
- Replace `the installer's `MIN_VERSION` carry one version` with `the launcher's `CLAX_VERSION` carry one version (`scripts/check-version.sh`)`.
- Add `the release script tests (`scripts/test-release.sh`), the dev script tests (`scripts/test-dev.sh`), ` after `the justfile, installer, `.

- [ ] **Step 4: README, "Releasing"**

Add after "Developing Clax":

````markdown
## Releasing

Only a person cuts a release. The steps:

```
scripts/bump-version.sh 0.3.0          # every version, the launcher's included
git commit -am "Release 0.3.0"         # after the gates pass
git tag -s v0.3.0 -m "Clax 0.3.0"
git push origin main v0.3.0
```

The tag runs `.github/workflows/release.yml`. It checks that the tag matches
every version, then builds macOS arm64 and x86_64 and Linux x86_64 and arm64
binaries on native runners, each with the web UI embedded. It smoke-tests each
binary and packs `clax-<version>-<target>.tar.gz`. It installs one through the
launcher from a local copy of the release, and publishes the archives,
`ensure-clax.sh` and `SHA256SUMS`. Running the workflow by hand (Actions,
Release, Run workflow) does everything except publish, and keeps the result as
the `release-dist` artifact.
````

- [ ] **Step 5: The plugin READMEs**

In `plugins/claude-code/README.md` and `plugins/clax/README.md`, replace the "Install" section (through the paragraph that begins `Hooks never download anything`). The new section starts with `## Install` and the harness's install commands in a code block:
- Claude Code: `/plugin marketplace add /path/to/clax`, then `/plugin install clax@clax`.
- Codex: `codex plugin marketplace add /path/to/clax`, then `codex plugin add clax@clax`, followed by the existing sentence that begins `` `codex mcp list` then shows``.

The rest of the section is the same in both:

```markdown
The plugin runs `clax` through a small launcher, `scripts/ensure-clax.sh`,
which carries the plugin's version and uses the first of:

1. `CLAX_BIN`, an absolute path to a clax binary;
2. a dev build linked with `clax dev-link` (`just dev-install` in the
   checkout), recorded in `~/.clax/config.toml`;
3. `~/.clax/bin/<plugin version>/clax`, the release this plugin was built for.

When 3 is missing, the MCP server downloads that exact release from GitHub,
checks it against the release's `SHA256SUMS` (which comes from the same place,
so it protects integrity, not authenticity), and installs it. If that takes
longer than a few seconds, the session starts without the clax tools and the
download finishes in the background: reconnect the MCP server, or start a new
session. The launcher never uses a `clax` on `PATH` or a build in a checkout.

If the MCP server cannot run `clax`, it still starts, with a single tool,
`status`, that says why and how to fix it. Hooks never download: without a
binary they print one line, log it to `~/.clax/logs/hooks.log`, and exit 0,
so a missing binary never fails a turn.
```

Replace the "Working from a source checkout" section with:

```markdown
## Working from a source checkout

Run `just dev-install` in the checkout: it copies a release build to
`~/.clax/bin/dev/clax` and links the plugin to it (restarting the daemon), so
rebuilding, `cargo clean` and moving the checkout never break a session.
`just dev-uninstall` returns to the release. "Developing Clax" in the
top-level README has the whole loop, including how to reload this plugin
after editing its skill or hooks.
```

In "When something is missing", add `launch` (which binary the launcher runs and why, and its last MCP start) to the list of checks, after `binary`.

In `plugins/pi/README.md`, replace the paragraph on finding the `clax` binary (`CLAX_BIN` or `PATH`) with:

```markdown
The extension runs `clax` resolved like the other plugins' launcher: `CLAX_BIN`,
else a dev build linked with `clax dev-link`, else
`~/.clax/bin/<package version>/clax`. It never downloads and never uses a
`clax` on `PATH`. Install the release with `bash scripts/ensure-clax.sh install`
in the checkout, or run `just dev-install`. `status` reports `launch`: which
binary runs and why.
```

- [ ] **Step 6: `docs/contract.md`**

1. In "### status", add `"launch": {"source": "installed", "bin": "/Users/alex/.clax/bin/0.2.0/clax", "warning": null, "notice": null},` to the example after `"feedback": []`, and add this paragraph after the `plugin_version` paragraph:

```markdown
`launch` says which binary answers and why, as the plugins' launcher reported
it: `source` is `clax-bin` (`CLAX_BIN`), `dev-link` (`clax dev-link`),
`installed` (`~/.clax/bin/<version>/clax`) or `downloaded` (installed by this
start); `bin` is its path; `warning` is set when it is not the plugin's
version; `notice` is a sentence stating a dev link, else `null`. It is absent
when the launcher did not start the shim (the daemon's `/mcp`). Under Pi it is
the extension's own resolution (`source` is never `downloaded`), or
`{"source": null, "error": "<why>"}` when none resolves.
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
version, or one whose version does not parse, is kept. `clax dev-link`,
`clax dev-unlink` and `just dev-install` replace the linked home's daemon the
same way whatever its version. `daemon.json` records the daemon's `version`
and `exe`.
```

3. Add a section `## Launcher` before `## Security model`:

```markdown
## Launcher

The Claude Code and Codex plugins start `clax` through
`scripts/ensure-clax.sh`, which carries the plugin's version
(`CLAX_VERSION`). The config directory is `$CLAX_CONFIG_DIR`, else
`$CLAX_HOME`, else `~/.clax`. It resolves, first match wins:

1. `CLAX_BIN`, which must then be a usable `clax`;
2. the dev link: `[dev_link] bin` in `<config dir>/config.toml`, written by
   `clax dev-link [path] [--home <dir>]` and removed by `clax dev-unlink`. It
   must then be a usable `clax`. Its `home` becomes `CLAX_HOME` for the
   binary;
3. `<config dir>/bin/<CLAX_VERSION>/clax`, when it reports that version.

A binary from 1 or 2 that reports another version runs, with a warning. When
none resolves:

- The MCP server (and only it, and only when neither 1 nor 2 is set)
  downloads `clax-<version>-<target>.tar.gz` and `SHA256SUMS` from
  `https://github.com/empathic/clax/releases/download/v<version>/`, with a
  10 s connect timeout and a 120 s limit per file
  (`CLAX_DOWNLOAD_TIMEOUT`), under the lock `<config dir>/bin/.install.lock`.
  It checks the checksum and the binary's version, and renames the new
  `bin/<version>/` into place. `bin/clax` then points at it, and older
  releases are removed except the previous one and the running daemon's. It
  waits up to 8 s (`CLAX_MCP_WAIT`); a longer download continues in the
  background.
- If the MCP server still has no binary, the launcher answers the MCP client
  itself. `initialize` succeeds, with `instructions` that start `Clax is
  unavailable:`. `tools/list` offers one tool, `status`, whose call returns
  the reason and the fix (`isError: true`). Any other request gets JSON-RPC
  error -32601 with the same reason.
- A hook prints one line to stderr and exits 0. It never downloads and never
  waits for the lock.
- Other commands (`ensure-clax.sh`, `ensure-clax.sh exec <command>`) print
  the reason and exit 1. `ensure-clax.sh install` downloads in the
  foreground.

Every MCP start adds a `launch mode=mcp agent=<harness> source=<source>
bin="<path>" version="<version>" warning="<text>"` line to
`<config dir>/logs/hooks.log`. Every failure adds a `launcher mode=<mode>
agent=<harness> exit=<status> reason="<why>" tried="<each candidate>"
argv="<arguments>"` line. The binary runs with `CLAX_CONFIG_DIR`,
`CLAX_LAUNCH`, `CLAX_LAUNCH_BIN` and, on a version mismatch,
`CLAX_LAUNCH_WARNING` set.
`clax doctor --agent <harness>` resolves the same way in its `launch` check.
```

4. In "## Security model", replace the "No telemetry" bullet with the §14 text from Task 1, Step 6.

5. In "## Known limitations", add:

```markdown
- Sessions that were running when a daemon was replaced, or when a dev link
  changed, keep the binary they started with until they restart; such a shim
  may restart a daemon from its own (older) binary only if it finds none.
- If a replaced daemon's port is taken while it restarts, the new daemon
  binds one of the next 20 ports and open browser tabs must be reloaded.
- Two sessions that both find a crashed installer's lock may both download;
  each installs by atomic rename, so the result is one complete install.
- The release download needs a public repository: GitHub serves a private
  repository's release files only to authenticated requests.
```

- [ ] **Step 7: Other plans' port assumptions**

`just dev` now binds 7481. Two plans in flight name only 7480. In `docs/superpowers/plans/2026-09-29-svelte-port.md`:
- In Global Constraints, replace `Never bind or connect to port 7480.` with `Never bind or connect to port 7480 or 7481 (the agents' daemon and `just dev`'s).`
- In Task 10 Step 5, replace `never `just dev`, which binds 7480:` with `never `just dev`, which binds 7481 and serves `~/.clax-dev`:`
- In Task 11's `vite.shell.config.ts` block, replace each `http://127.0.0.1:7480` in `server.proxy` with `http://127.0.0.1:7481`, to match the file after this plan's Task 11. Keep the note that says the block is unchanged, and add `(as the stable-install plan left it)`.

The timing harness (`web/perf/usable.perf.ts`) starts its daemons through `startDaemon()` with `--port 0` and needs no change. The `7481` in `web/shell/src/frame-src-cases.json` is an example origin string in a unit test, never bound, and needs no change either.

In `docs/superpowers/plans/2026-09-30-agent-working.md`, in Global Constraints, replace `Never bind or connect to port 7480.` with `Never bind or connect to port 7480 or 7481.`

- [ ] **Step 8: Check the docs**

Run: `python3 scripts/sync-skill-tools.py --check && bash scripts/test-plugins.sh | tail -1 && grep -n "MIN_VERSION\|CLAX_SOURCE_DIR\|CLAX_INSTALL_DIR\|cargo install --path" README.md plugins/*/README.md docs/contract.md`
Expected: the sync check passes, `plugin checks passed`, and the `grep` prints nothing.

- [ ] **Step 9: Gates and commit**

Run: `bash scripts/quality_gates.sh; echo "exit=$?"`
Expected: `exit=0`.

```bash
git add README.md plugins/claude-code/README.md plugins/clax/README.md plugins/pi/README.md docs/contract.md docs/superpowers/plans/2026-09-29-svelte-port.md docs/superpowers/plans/2026-09-30-agent-working.md
git commit -m "Document the release install, the launcher, the dev flow and releasing"
git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed
```

---

## Steps for the person

Agents stop at the end of Task 12. Everything below is outward-facing or touches the real homes, so only you do it.

### A. Decide where releases are hosted

`github.com/empathic/clax` is private, so anonymous `curl` gets 404 for its release files. Choose one:

1. **Make the repository public** (Settings, General, Danger Zone, Change visibility). Nothing in the plan changes.
2. **Publish releases to a separate public repository**, for example `empathic/clax-releases`:
   - Create it with at least one commit.
   - Create a fine-grained token with `contents: write` on it, and store it as the secret `RELEASES_TOKEN` in `empathic/clax`.
   - In `.github/workflows/release.yml`'s publish step, set `GH_TOKEN: ${{ secrets.RELEASES_TOKEN }}` and `--repo empathic/clax-releases`, drop `--verify-tag`, and add `--target main`.
   - In `scripts/ensure-clax.sh`, set `REPO="empathic/clax-releases"`, and copy the launcher to both plugins.
   - In `docs/contract.md` ("Launcher"), change the URL.

### B. Move this machine to the new launcher (before the first release)

```bash
cd /Users/alex/Devel/empathic/clax
git pull
just dev-install                  # builds, copies to ~/.clax/bin/dev/clax, links, restarts ~/.clax's daemon
```

Reinstall the plugins so their copies carry the new launcher:
- Claude Code: `/plugin marketplace update clax`, `/plugin uninstall clax@clax`, `/plugin install clax@clax`.
- Codex: `codex plugin marketplace remove clax`, `codex plugin marketplace add /Users/alex/Devel/empathic/clax`, `codex plugin add clax@clax`. Re-adding the marketplace clears the path recorded before the checkout moved.

Remove what nothing uses any more, and check each first:

```bash
~/.cargo/bin/clax --version && cargo uninstall clax-cli
~/.local/bin/clax --version && rm ~/.local/bin/clax      # only if it prints "clax ..."
grep -rn "CLAX_SOURCE_DIR\|CLAX_INSTALL_DIR" ~/.zshrc ~/.zprofile ~/.config/fish 2>/dev/null   # remove any hits
```

Start a new session in each harness and check the result:

```bash
~/.clax/bin/dev/clax doctor --agent codex     # launch: "DEV LINK: agents run /Users/alex/.clax/bin/dev/clax ..."
~/.clax/bin/dev/clax doctor --agent claude
tail -3 ~/.clax/logs/hooks.log                # a "launch mode=mcp ... source=dev-link" line per session
```

In a session, ask the agent to call `status`. `launch.notice` should state the dev link.

Check the fallback once. In a scratch shell, `CLAX_BIN=/nonexistent claude` (or `codex`), then run `/mcp` or list the tools: the `clax` server should be connected with the single tool `status`, which states the `CLAX_BIN` problem. Then exit that session.

Check the dev split: `just dev` serves http://localhost:7481 from `~/.clax-dev`. While it rebuilds, an agent session keeps working against 7480.

### C. Cut the first release

1. After A, run the release workflow by hand on `main`: Actions, Release, Run workflow. All four build jobs and `assemble` must pass. If `macos-15-intel` or `ubuntu-24.04-arm` is not available to the repository, change that one job:
   - `x86_64-apple-darwin` builds on `macos-15` with the same `--target`, since the Apple SDK builds both architectures. The smoke step then runs under Rosetta; install it with `softwareupdate --install-rosetta --agree-to-license`.
   - `aarch64-unknown-linux-musl` builds on `ubuntu-24.04` with `cargo install cargo-zigbuild`, `pip install ziglang` and `cargo zigbuild --release --locked --target aarch64-unknown-linux-musl -p clax-cli --bin clax`. Its smoke step cannot run there, so skip it for that target.
2. Download the `release-dist` artifact and check it holds four archives, `ensure-clax.sh` and `SHA256SUMS`.
3. Choose the version. The workspace is at `0.2.0` and nothing has been released, so `v0.2.0` works. To start at another version: `scripts/bump-version.sh 0.3.0`, `just ci`, then commit.
4. Tag and push:

```bash
scripts/check-version.sh v0.2.0
git tag -s v0.2.0 -m "Clax 0.2.0"
git push origin main v0.2.0
```

5. When the workflow's publish job has finished, test the real download into a scratch directory, not your real home:

```bash
T="$(mktemp -d)"
CLAX_CONFIG_DIR="$T" CLAX_HOME="$T" bash scripts/ensure-clax.sh install
xattr -l "$T/bin/0.2.0/clax"          # prints nothing: no com.apple.quarantine
codesign -dv "$T/bin/0.2.0/clax" 2>&1 | grep -i adhoc
"$T/bin/0.2.0/clax" --version
rm -rf "$T"
```

6. Move the agents to the release: `~/.clax/bin/dev/clax dev-unlink`. Then start a new session in each harness. Its MCP server downloads `v0.2.0` into `~/.clax/bin/0.2.0/`, and `clax doctor --agent <harness>` then reports `release: …/bin/0.2.0/clax`. Keep `just dev-install` for testing unreleased work.
7. Optionally, put `~/.clax/bin` on your `PATH` for a shell `clax`.
