# Clax Rename Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rename Artifax to Clax everywhere in the repository (prose, binary, crates, packages, plugins, skill, home directory, environment variables, wire prefix, routes, globals, log paths), with a clean break: no alias for the old name anywhere, and nothing that reads, migrates or deletes `~/.artifax`.

**Architecture:** One mechanical commit: the spec is amended, a permanent name gate is added to `scripts/test-plugins.sh` and seen failing, every directory and file whose path holds the old name is moved with `git mv`, one case-preserving substitution runs over the tracked files outside the approved exceptions, and the generated files (Cargo.lock, both package-lock.json files, the skills' tool blocks, `web/dist`) are regenerated, never hand-edited. Regression tests pin the clean break (the old home, variables, binary, plugin cache and cookie are ignored). A second task verifies the result from the outside: the exact exception list, a rename-only diff check, the built binary, a hermetic daemon run, and every quality gate.

**Tech Stack:** Rust 2024 workspace (cargo, clippy, rustfmt), bash, perl, python3, Node (npm, Vite, Vitest, oxlint, Playwright), git.

**Spec:** `docs/superpowers/specs/2026-09-28-artifax-design.md`, which Task 1 moves to `docs/superpowers/specs/2026-09-28-clax-design.md`.

## Global Constraints

- Rust edition 2024; `cargo clippy --workspace --all-targets -- -D warnings` and `RUSTFLAGS=-Dwarnings cargo check --workspace` stay clean.
- `oxlint --deny-warnings` (the `web lint` gate) stays clean.
- The Playwright e2e suite runs in both frame modes (subdomain and sandbox), as `npm run e2e` does today.
- Commit with `git commit --no-gpg-sign`; stage with `git add` naming explicit paths (after `git mv`, the moves are already staged; stage edits with `git add -u -- <paths>` or explicit paths, never `git add -A` or `git add .`).
- Never touch port 7480, the real `~/.artifax`, `~/.clax`, `~/.claude` or `~/.codex`. Every command that starts a daemon sets `CLAX_HOME` (or `ARTIFAX_HOME` before the rename) to a temp directory and passes `--port 0`. Do not run `scripts/smoke-claude.sh`, `scripts/smoke-codex.sh` or `scripts/smoke-pi.sh` (they drive the real harnesses).
- Clean break: no alias, fallback, migration or cleanup for the old name. The code never reads, writes, migrates or deletes `~/.artifax`, never reads an `ARTIFAX_*` variable, and never looks for an `artifax` binary, plugin, marketplace, cookie or storage key.
- The checkout directory (`/Users/alex/Devel/empathic/artifax`) and the git remote are not renamed; that is the person's job.
- In prose, "ID" is the short form of identifier; lowercase `id` only as a literal code symbol.
- Doc comments and commit messages describe the contract or the change, never this conversation.
- Version stays `0.2.0` in `Cargo.toml [workspace.package]`, both plugin manifests, both marketplace entries, `plugins/pi/package.json` and the installer's `MIN_VERSION`. No release has been published under either name, so there is no older Clax to warn about, and `scripts/test-plugins.sh` requires the five to agree.
- Each task ends with `bash scripts/quality_gates.sh; echo "exit=$?"` printing `all gates passed` and `exit=0`.

## Review Focus

Most likely to bite a person first:

1. **Old variables still exported in the person's shell** (`ARTIFAX_HOME`, `ARTIFAX_BIN`, …): Clax must ignore them and use `~/.clax` and its own resolution. Pinned by `crates/clax-cli/tests/clean_break.rs` and the no-alias case in `scripts/test-ensure-clax.sh` (Task 1, Steps 11 and 12).
2. **The old daemon still running on 7480, with its `~/.artifax/daemon.json`**: Clax must never follow that file or talk to that daemon, and must start its own on the next free port (`bind_first_free`). Pinned by `clean_break.rs` (it serves a live-looking daemon from the old home and asserts zero connections) (Task 1, Step 11).
3. **Old plugin copies still in the harness caches** (`<codex home>/plugins/cache/artifax/artifax/<version>`, `artifax@artifax` in Claude Code's installed plugins): `clax doctor --agent` must not report them as the Clax plugin. Pinned by `a_cache_copy_under_the_previous_name_is_not_this_plugin` in `crates/clax-cli/src/commands/doctor_agent.rs` (Task 1, Step 13).
4. **An `artifax` binary on PATH or in `~/.artifax/bin`**: the installer must not accept it as the Clax CLI. Pinned by the no-alias case in `scripts/test-ensure-clax.sh` (Task 1, Step 12).
5. **Browser state shared on localhost**: cookies ignore the port, so the old daemon's `artifax_viewer` cookie reaches Clax's daemon. Clax must mint its own `clax_viewer` and never adopt the old one. Pinned by `only_the_clax_viewer_cookie_names_a_viewer` in `crates/clax-server/src/viewer.rs` (Task 1, Step 14). The old `artifax.origin-ok` sessionStorage key is likewise ignored because the shell now reads `clax.origin-ok` only.

Also for the reviewer: the release download URL becomes `https://github.com/empathic/clax/releases/download`. It works only once the person renames the GitHub repository (GitHub redirects an old repository name to the new one, never the reverse). No release is published yet, so nothing that works today breaks.

---

## Inventory

Recompute at the start of Task 1 with:

```bash
git grep -c -i artifax -- . ':(exclude)docs/superpowers/plans'
git ls-files | grep -i artifax
```

Counts below are case-insensitive matches per file, taken at `0f2b97e` plus the in-flight edits to `docs/contract.md`, the spec and `web/shell/src/*` then in the working tree. Totals: 3,708 `artifax`, 364 `Artifax`, 439 `ARTIFAX` across the tracked tree; 2,225 matches in 189 files outside `docs/superpowers/plans`; the six historical phase plans hold 1,342 more. No other casing (such as `ArtiFax`) occurs.

### Paths that hold the name (moved with `git mv`)

| From | To |
|---|---|
| `crates/artifax-cli/` (21 files) | `crates/clax-cli/` |
| `crates/artifax-core/` (23 files) | `crates/clax-core/` |
| `crates/artifax-hooks/` (17 files, incl. 11 JSON fixtures) | `crates/clax-hooks/` |
| `crates/artifax-mcp/` (13 files) | `crates/clax-mcp/` |
| `crates/artifax-server/` (48 files) | `crates/clax-server/` |
| `plugins/artifax/` (6 files) | `plugins/clax/` |
| `plugins/clax/skills/artifax/` (after the move above) | `plugins/clax/skills/clax/` |
| `plugins/claude-code/skills/artifax/` | `plugins/claude-code/skills/clax/` |
| `plugins/pi/skills/artifax/` | `plugins/pi/skills/clax/` |
| `scripts/ensure-artifax.sh` | `scripts/ensure-clax.sh` |
| `plugins/clax/scripts/ensure-artifax.sh` | `plugins/clax/scripts/ensure-clax.sh` |
| `plugins/claude-code/scripts/ensure-artifax.sh` | `plugins/claude-code/scripts/ensure-clax.sh` |
| `scripts/test-ensure-artifax.sh` | `scripts/test-ensure-clax.sh` |
| `plugins/pi/src/artifax.ts` | `plugins/pi/src/clax.ts` |
| `plugins/pi/test/artifax.test.ts` | `plugins/pi/test/clax.test.ts` |
| `docs/superpowers/specs/2026-09-28-artifax-design.md` | `docs/superpowers/specs/2026-09-28-clax-design.md` |

### Cargo package and lib names

- Packages `artifax-cli`, `artifax-core`, `artifax-hooks`, `artifax-mcp`, `artifax-server` become `clax-*`; lib paths `artifax_core`, `artifax_server`, `artifax_mcp`, `artifax_hooks` become `clax_*` (146, 27, 22 and 3 uses).
- Binary: `[[bin]] name = "artifax"` becomes `"clax"`; `Command::cargo_bin("artifax")` and `CARGO_BIN_EXE_artifax` follow.
- Files: `Cargo.toml` (6: members, repository URL), `crates/artifax-cli/Cargo.toml` (6), `crates/artifax-core/Cargo.toml` (1), `crates/artifax-hooks/Cargo.toml` (1), `crates/artifax-mcp/Cargo.toml` (3), `crates/artifax-server/Cargo.toml` (5).
- Generated: `Cargo.lock` (14).

### Rust identifiers and strings

Identifiers: `ArtifaxTools` (45), `ArtifaxOptions` (3), `is_artifax` (15, shell), `install_artifax` (6, shell), `fake_artifax` (28), `artifax_bin` (8), `artifaxBin` (8), `artifax_home` (2), `artifaxHome` (3), `artifax_codex_bin` (2), test names such as `from_env_prefers_artifax_home`, `from_env_fails_without_artifax_home_or_home`, `claude_falls_back_to_artifax_session_id_and_current_dir`, `open_returns_the_browser_url_without_opening_under_artifax_no_open`, `artifax_codex_bin_overrides_path_and_empty_disables`, `pi_packages_are_local_paths_in_settings_naming_the_artifax_package`, `missing_artifax_home_and_home_is_an_error`, `artifax_foreign`.

Files (count): `crates/artifax-cli/src/client.rs` (8), `commands/asset.rs` (2), `commands/delete.rs` (1), `commands/doctor.rs` (11), `commands/doctor_agent.rs` (57), `commands/hook.rs` (6), `commands/list.rs` (1), `commands/mcp.rs` (3), `commands/open.rs` (3), `commands/pin.rs` (1), `commands/publish.rs` (2), `commands/read.rs` (4), `commands/serve.rs` (5), `commands/status.rs` (1), `commands/stop.rs` (6), `commands/tools.rs` (4), `hooklog.rs` (5), `main.rs` (5); `crates/artifax-core/src/anchor.rs` (3), `feedback.rs` (20), `home.rs` (14), `lib.rs` (1), `model.rs` (1), `store/viewers.rs` (1), `wrap.rs` (19); `crates/artifax-hooks/src/events.rs` (4), `input.rs` (1); `crates/artifax-mcp/src/client.rs` (4), `lib.rs` (2), `plugin.rs` (5), `render.rs` (1), `shim.rs` (13), `tools.rs` (29); `crates/artifax-server/src/blocking.rs` (2), `daemon.rs` (5), `db_caller.rs` (5), `error.rs` (2), `feedback.rs` (4), `host.rs` (3), `lib.rs` (1), `push.rs` (14), `routes/artifacts.rs` (10), `routes/assets.rs` (2), `routes/content.rs` (5), `routes/docs.rs` (5), `routes/events.rs` (3), `routes/feedback.rs` (3), `routes/mcp.rs` (3), `routes/mod.rs` (5), `routes/sessions.rs` (3), `routes/shell.rs` (5), `routes/threads.rs` (13), `routes/viewers.rs` (4), `state.rs` (1), `testing.rs` (8), `viewer.rs` (7).

### Environment variables (all ten become `CLAX_*`)

`ARTIFAX_HOME` (147), `ARTIFAX_BIN` (93), `ARTIFAX_CODEX_BIN` (71), `ARTIFAX_NO_OPEN` (25), `ARTIFAX_RELEASE_VERSION` (20), `ARTIFAX_INSTALL_DIR` (19), `ARTIFAX_SOURCE_DIR` (18), `ARTIFAX_RELEASE_BASE_URL` (17), `ARTIFAX_CONFIG_DIR` (14), `ARTIFAX_SESSION_ID` (10). (`ARTIFAX_SESSION`, `ARTIFAX_E…` and `ARTIFAX_SMOKE_UNSET_KEY` occur only in the historical plans.) The Codex `.mcp.json` `env_vars` list names nine of them.

### Wire prefixes, headers, cookie, storage, globals

- postMessage types (19, in `web/bridge/src/protocol.ts`): `artifax:hello`, `artifax:welcome`, `artifax:comment-mode`, `artifax:pick-start`, `artifax:pick`, `artifax:cancel`, `artifax:hover`, `artifax:key`, `artifax:focus`, `artifax:anchors`, `artifax:resolve-anchors`, `artifax:scroll-to`, `artifax:hash`, `artifax:navigate`, `artifax:use`, `artifax:use-result`, `artifax:call`, `artifax:call-result`, `artifax:event`. All become `clax:*`.
- HTTP headers: `x-artifax-session` (65), `x-artifax-via` (13), `x-artifax-test-delay-ms` (3) become `x-clax-*`.
- Cookie `artifax_viewer` (24) becomes `clax_viewer`; sessionStorage key `artifax.origin-ok` becomes `clax.origin-ok`.
- Globals: `window.__artifax` (14, plus the `.oxlintrc.json` `no-underscore-dangle` allow entry) becomes `window.__clax`; the Vite IIFE name `artifaxBridge` becomes `claxBridge`; the bridge's overlay custom element `artifax-overlay` becomes `clax-overlay`; e2e-only page globals `artifaxMsgs`, `artifaxClips`, `artifaxHeard`, `artifaxActivated` become `clax*`.
- Hook and feedback text: `[artifax] …` becomes `[clax] …`; console prefixes `artifax: …` become `clax: …`.
- Files: `web/bridge/src/anchor.ts` (2), `bridge.ts` (19), `caps/artifact.ts` (2), `channel.ts` (2), `meta.ts` (7), `protocol.ts` (24), `rpc.ts` (6), `text-walk.ts` (1); `web/shell/src/artifact.tsx` (19), `bridge-link.ts` (1), `caps/artifact.ts` (6), `caps/assets.ts` (1), `caps/comments.ts` (4), `caps/db.ts` (3), `caps/downloads.ts` (2), `caps/grants.ts` (2), `caps/host.ts` (7), `caps/user.ts` (1), `gallery.tsx` (2), `origin.ts` (1), `waiting.ts` (1); `web/.oxlintrc.json` (1).

### Routes and on-disk paths

- `/_artifax/{*path}` becomes `/_clax/{*path}` (`routes/mod.rs`, `routes/shell.rs` `BRIDGE`, `host.rs` pass-through); `/_artifax/bridge.js` (55 uses, `BRIDGE_PATH` and `BRIDGE_START` in `wrap.rs`) becomes `/_clax/bridge.js`.
- Build output `web/dist/_artifax/` becomes `web/dist/_clax/` (`web/vite.bridge.config.ts` outDir, `web/vite.shell.config.ts` assetsDir, `justfile` `web` and `clean` recipes).
- Home: `~/.artifax` becomes `~/.clax`; `artifax.db` becomes `clax.db`; `daemon.json` and `logs/hooks.log` keep their names under the new home; the installer fallback `~/.artifax/bin/artifax` becomes `~/.clax/bin/clax`; `~/.local/bin/artifax` becomes `~/.local/bin/clax`.
- Temp prefixes: `artifax-e2e-` (web/e2e/fixtures.ts), `artifax-loop.` (smoke-comment-loop.sh).

### Plugin manifests and commands

- `.agents/plugins/marketplace.json` (4): marketplace `name`, `displayName`, plugin `name`, `path: ./plugins/clax`.
- `.claude-plugin/marketplace.json` (3): marketplace and plugin `name`, description.
- `plugins/artifax/.codex-plugin/plugin.json` (3), `.mcp.json` (11: server key, script path, `env_vars`), `hooks/hooks.json` (3), `README.md` (47).
- `plugins/claude-code/.claude-plugin/plugin.json` (1), `.mcp.json` (2), `hooks/hooks.json` (4), `README.md` (39), `commands/comments.md` (3), `doctor.md` (5), `list.md` (3), `open.md` (3), `serve.md` (5), `wait.md` (2), `watch.md` (4). Commands become `/clax:comments`, `/clax:doctor`, `/clax:list`, `/clax:open`, `/clax:serve`, `/clax:wait`, `/clax:watch`.
- Plugin and marketplace IDs `artifax` become `clax`, so `artifax@artifax` becomes `clax@clax`.
- MCP tool names: `mcp__artifax__<tool>` becomes `mcp__clax__<tool>`; `mcp__plugin_artifax_artifax__<tool>` becomes `mcp__plugin_clax_clax__<tool>`; Pi's `artifax_<tool>` (22 tools) becomes `clax_<tool>`.

### Skill copies

`plugins/artifax/skills/artifax/SKILL.md` (12), `plugins/claude-code/skills/artifax/SKILL.md` (13), `plugins/pi/skills/artifax/SKILL.md` (18): frontmatter `name: clax`, prose, the generated tool block.

### The contract fixture and the Pi package

- `plugins/pi/test/fixtures/contract.json` (2): the `_comment` and the `status` description ("Report the Clax daemon's URL …"). The description must stay word-for-word equal in `crates/clax-mcp/src/tools.rs` and `plugins/pi/src/clax.ts`; one sweep changes all three.
- `plugins/pi/package.json` (4: `@empathic/artifax-pi`, description, repository URL, `pi.extensions` path), `plugins/pi/src/artifax.ts` (83, `artifaxExtension`), `src/client.ts` (5), `src/daemon.ts` (20), `test/artifax.test.ts` (159), `test/daemon-fixture.ts` (5), `README.md` (34).

### Lockfiles (generated)

`Cargo.lock` (14), `web/package-lock.json` (2: `artifax-web`), `plugins/pi/package-lock.json` (2: `@empathic/artifax-pi`).

### Tests and fixtures that assert literal strings

`crates/artifax-cli/tests/cli.rs` (45), `crates/artifax-hooks/tests/golden.rs` (23), `crates/artifax-mcp/tests/comments.rs` (11), `db.rs` (9), `open.rs` (7), `open_status.rs` (5), `shim.rs` (37), `tools.rs` (18), `crates/artifax-server/tests/api_artifacts.rs` (1), `api_content.rs` (15), `api_docs.rs` (4), `api_events.rs` (2), `api_feedback.rs` (1), `api_host.rs` (1), `api_push.rs` (3), `api_sessions.rs` (4), `api_threads.rs` (27), `api_timeout.rs` (1), `api_watches.rs` (2), `bridge_dev.rs` (6), `common/mod.rs` (1), `daemon.rs` (2); `web/bridge/test/anchor.test.ts` (2), `area.test.ts` (2), `bridge-area.test.ts` (12), `bridge-framed.test.ts` (8), `bridge.test.ts` (16), `channel.test.ts` (5), `comment-mode.test.ts` (7), `rpc.test.ts` (9); `web/shell/src/artifact.test.tsx` (119), `bridge-link.test.ts` (6), `caps/artifact.test.ts` (1), `caps/comments.test.ts` (4), `caps/db.test.ts` (1), `caps/host.test.ts` (21), `gallery.test.tsx` (1), `origin.test.ts` (1), `waiting.test.ts` (1); `web/e2e/area.spec.ts` (24), `artifact.spec.ts` (10), `bridge-comment.spec.ts` (12), `caching.spec.ts` (2), `comment-again.spec.ts` (1), `comment-loop.spec.ts` (2), `comment-targets.spec.ts` (9), `comments-capability.spec.ts` (2), `comments.spec.ts` (1), `fixtures.ts` (18), `gesture.spec.ts` (9), `subpages.spec.ts` (2), `viewer.spec.ts` (1), `wrap.spec.ts` (2). The in-tree unit tests in `doctor_agent.rs`, `plugin.rs`, `hooklog.rs`, `home.rs`, `wrap.rs`, `events.rs` and `anchor.rs` assert literal strings too. The hook JSON fixtures under `crates/artifax-hooks/tests/fixtures/` hold no occurrence (they move with their crate).

### Scripts, build config, CI

`scripts/dev.sh` (8), `ensure-artifax.sh` (71, ×3 identical copies), `quality_gates.sh` (1), `smoke-claude.sh` (14), `smoke-codex.sh` (20), `smoke-comment-loop.sh` (19), `smoke-pi.sh` (15), `sync-skill-tools.py` (13), `test-ensure-artifax.sh` (121), `test-plugins.sh` (27); `justfile` (12); `web/package.json` (1: `artifax-web`), `web/shell/index.html` (1: `<title>`), `web/vite.bridge.config.ts` (2), `web/vite.shell.config.ts` (1); `.github/workflows/release.yml` (7: `-p artifax-cli --bin artifax`, tarball and artifact names `artifax-<target>.tar.gz`).

### Docs prose

`README.md` (31), `docs/contract.md` (54), the spec (122), the plugin READMEs listed above.

---

## Traps

- **Hash-pinned fixtures.** None contains the name. The only pinned hashes are the `html_hash` fixture values `sha256:00` and `sha256:ab` (`crates/artifax-core/src/store/mod.rs`, `anchor.rs`, `crates/artifax-server/src/testing.rs`), which are unaffected. The bridge's `?v=<hash>` is computed from the built bundle at runtime, so it changes by itself when the bundle's strings change.
- **Tests that assert literal strings** (list above): the sweep changes the code and its assertions together. Where a string's length matters, it is still consistent: `hooklog.rs` counts `x` characters only after `stderr="`, so `/b/clax` in the line does not disturb it.
- **The plugin gate's word-for-word sections.** `scripts/test-plugins.sh` requires "Page contract" to match across the three skills and `docs/contract.md`, and "Comment loop", "Data (db)", "What is not yet available" to match across the three skills. One sweep changes all four files identically. Do not hand-edit one copy.
- **`scripts/sync-skill-tools.py --check`.** The tool block is `textwrap`-wrapped at 78 columns, so the shorter name rewraps it: run `python3 scripts/sync-skill-tools.py` (write mode) after the sweep. Its `names()` excludes the skill's own name (the sweep turns `{"artifax"}` into `{"clax"}`), and its `DOC_LISTS` regexes match "The `clax` MCP server" in the READMEs after the sweep. `crates/clax-mcp/src/plugin.rs` parses "This is Clax plugin <version>" from that block; the sweep changes the parser and the generator together.
- **The installer's `MIN_VERSION`** stays `0.2.0`; `test-plugins.sh` checks it equals the workspace, both manifests and the Pi package.
- **The Pi package name.** `@empathic/artifax-pi` becomes `@empathic/clax-pi` in `plugins/pi/package.json`, its lockfile, and the `PI_PACKAGE` constant in `crates/clax-cli/src/commands/doctor_agent.rs` that `clax doctor --agent pi` looks for in Pi's settings.
- **MCP tool names** change with the server key (`mcp__clax__*`) and the plugin ID (`mcp__plugin_clax_clax__*`); the Claude Code commands' `allowed-tools` frontmatter and the skill texts name them.
- **The Claude Code plugin cache layout.** `doctor_agent.rs` finds copies at `<claude dir>/plugins/cache/<marketplace>/clax/<version>` and in `installed_plugins.json` entries keyed `clax@*`; the Codex copy is `<codex home>/plugins/cache/<marketplace>/clax/<version>`. Old `artifax` copies are not found (a test pins it).
- **`git mv` for directories**, so history follows; move the directories before the sweep, then the sweep sees the new paths through `git ls-files`.
- **`include_str!` paths** in `doctor_agent.rs` (`../../../../scripts/ensure-artifax.sh`, the three `skills/artifax/SKILL.md` copies, `plugins/artifax/…`) must name the moved files; the sweep rewrites them to the paths the moves created.
- **Hard-coded paths** in `scripts/test-plugins.sh`, `scripts/sync-skill-tools.py`, `plugins/*/hooks/hooks.json` and `plugins/*/.mcp.json` (`./scripts/ensure-artifax.sh`, `${PLUGIN_ROOT}/scripts/ensure-artifax.sh`, `${CLAUDE_PLUGIN_ROOT}/scripts/ensure-artifax.sh`) must match the moved script names; the sweep does it, and the gate checks it.
- **The three installer copies** must stay byte-identical (`cmp` gate): edit `scripts/ensure-clax.sh` only, then copy it over the two plugin copies.
- **Generated files.** `Cargo.lock`: regenerate with `cargo update --workspace` (it renames the workspace members only and changes no dependency). `web/package-lock.json` and `plugins/pi/package-lock.json`: the sweep renames their `name` fields; `npm install --package-lock-only --ignore-scripts` must then leave them unchanged. The skills' tool blocks: `sync-skill-tools.py`. `web/dist`: untracked build output, rebuilt by the gates.
- **The stale `web/dist/_artifax/` directory** (over 100 old shell bundles) stays on disk after the rename and would be embedded by `rust-embed` into a release binary. Delete it before building.
- **`.claude/worktrees/`** holds three other checkouts with thousands of matches. Never use `grep -r`, `find` or `sed` over the tree; list files with `git ls-files` / `git grep`, which see tracked files of this checkout only.
- **The other agent's in-flight edits.** Run nothing in Task 1 until `git status --porcelain` is empty; if it is not, stop and report.
- **Articles.** "an Artifax" would become "an Clax". There are none today (`git grep -niE '\ban artifax'` is empty); Task 1 checks again after the sweep.
- **Aligned columns.** The installer's header documents the variables in a two-column layout; the shorter names shift the description column on five lines. Re-pad them (Task 1, Step 8). The contract's markdown tables are not padded and need nothing.
- **Release asset names** in `.github/workflows/release.yml` (`clax-<target>.tar.gz`) must match what `ensure-clax.sh` downloads (`clax-${target}.tar.gz`); the sweep changes both.
- **Cookies and storage on localhost ignore the port.** The rename of `artifax_viewer` and `artifax.origin-ok` keeps the two products' browser state apart.
- **The historical plans** keep naming `docs/superpowers/specs/2026-09-28-artifax-design.md`, which no longer exists after the move. That is accepted: they record what was executed.

## Approved exceptions

After this plan, `git grep -il artifax` lists exactly these nine files, and no tracked path contains the name:

1. `docs/superpowers/plans/2026-09-28-phase-1-daemon-publish-viewer.md` (historical, unchanged)
2. `docs/superpowers/plans/2026-09-28-phase-2-mcp-and-plugins.md` (historical, unchanged)
3. `docs/superpowers/plans/2026-09-28-phase-2-scoped.md` (historical, unchanged)
4. `docs/superpowers/plans/2026-09-28-phase-3-comments-and-feedback.md` (historical, unchanged)
5. `docs/superpowers/plans/2026-09-28-phase-4-runtime-capabilities.md` (historical, unchanged)
6. `docs/superpowers/plans/2026-09-28-phase-5-room-and-sample.md` (historical, unchanged)
7. `docs/superpowers/plans/2026-09-29-clax-rename.md` (this plan)
8. `docs/superpowers/plans/2026-09-29-svelte-port.md` (the Svelte port plan, written alongside this one)
9. `docs/superpowers/specs/2026-09-28-clax-design.md`, and within it only the lines between `<!-- name-history:begin -->` and `<!-- name-history:end -->`.

The tests that must spell the old name to prove it is ignored (`scripts/test-plugins.sh`, `scripts/test-ensure-clax.sh`, `crates/clax-cli/tests/clean_break.rs`, `crates/clax-cli/src/commands/doctor_agent.rs`, `crates/clax-server/src/viewer.rs`) assemble it from two halves (`"arti""fax"` in bash, `concat!("arti", "fax")` in Rust), so they are not exceptions and the gate stays exact.

---

### Task 1: Rename to Clax with a clean break

**Files:**
- Move (`git mv`): every path in the Inventory's "Paths that hold the name" table.
- Modify: every tracked file listed in the Inventory (by the sweep), plus by hand `docs/superpowers/specs/2026-09-28-clax-design.md`, `scripts/test-plugins.sh`, `scripts/test-ensure-clax.sh`, `scripts/ensure-clax.sh` (and its two copies), `crates/clax-cli/src/commands/doctor_agent.rs`, `crates/clax-server/src/viewer.rs`.
- Create: `crates/clax-cli/tests/clean_break.rs`.
- Regenerate: `Cargo.lock`, `plugins/*/skills/clax/SKILL.md` tool blocks.

**Interfaces:**
- Consumes: the tree at the commit this task starts from (call it `BASE`).
- Produces: binary `clax`; crates `clax-cli`, `clax-core`, `clax-hooks`, `clax-mcp`, `clax-server` (libs `clax_core`, `clax_server`, `clax_mcp`, `clax_hooks`); `ClaxTools`; home `~/.clax` with `clax.db`, `daemon.json`, `logs/hooks.log`; variables `CLAX_HOME`, `CLAX_BIN`, `CLAX_CODEX_BIN`, `CLAX_NO_OPEN`, `CLAX_SESSION_ID`, `CLAX_SOURCE_DIR`, `CLAX_INSTALL_DIR`, `CLAX_CONFIG_DIR`, `CLAX_RELEASE_BASE_URL`, `CLAX_RELEASE_VERSION`; postMessage prefix `clax:`; routes `/_clax/*`; header `x-clax-session`; cookie `clax_viewer`; global `window.__clax`; plugin and marketplace ID `clax`; skill `clax`; Pi package `@empathic/clax-pi` with tools `clax_<tool>`; installer `scripts/ensure-clax.sh`. Task 2 and the Svelte port plan rely on these names.

- [ ] **Step 1: Start from a clean tree**

Run:
```bash
cd /Users/alex/Devel/empathic/artifax
git status --porcelain
git rev-parse HEAD
```
Expected: `git status --porcelain` prints nothing. If it prints anything (the other agent's edits to `web/shell/src/caps/*`, `artifact.tsx`, `docs/contract.md`, the spec, or e2e files), stop and report; do not stash or discard them. Note the printed commit as `BASE` for Task 2.

- [ ] **Step 2: Move the spec and rename it throughout**

```bash
git mv docs/superpowers/specs/2026-09-28-artifax-design.md docs/superpowers/specs/2026-09-28-clax-design.md
perl -pi -e 's/ARTIFAX/CLAX/g; s/Artifax/Clax/g; s/artifax/clax/g' docs/superpowers/specs/2026-09-28-clax-design.md
git grep -c -i artifax -- docs/superpowers/specs/2026-09-28-clax-design.md
```
Expected: the last command prints nothing (exit 1).

- [ ] **Step 3: Add the name-history note and decision D15 to the spec**

Edit `docs/superpowers/specs/2026-09-28-clax-design.md`. Directly after the `Status:` line near the top, insert:

```markdown

<!-- name-history:begin -->
**Name history.** Clax was named Artifax until 2026-09-29, when it was
renamed with a clean break (D15): no command, crate, package, plugin, skill,
variable, route, message prefix, header, cookie or file keeps the old name or
answers to it, and Clax never reads, migrates or deletes `~/.artifax`. The
plans in `docs/superpowers/plans/2026-09-28-*.md` predate the rename and use
the old name.
<!-- name-history:end -->
```

In "## 2. Decisions", append this row after D14:

```markdown
| D15 | The product is Clax: binary `clax`, crates `clax-*`, home `~/.clax`, variables `CLAX_*`, message prefix `clax:`, routes `/_clax/`, plugin and skill `clax`; renamed from its first name with a clean break (no aliases, no migration; see the name-history note) | One name everywhere; nothing was released under the first name, so there is nothing to carry over. |
```

Then check the spec's remaining occurrences are inside the note only:
```bash
awk '/<!-- name-history:begin -->/{on=1;next} /<!-- name-history:end -->/{on=0;next} !on && tolower($0) ~ /artifax/ {print NR": "$0}' docs/superpowers/specs/2026-09-28-clax-design.md
```
Expected: no output.

- [ ] **Step 4: Write the name gate in `scripts/test-plugins.sh` and see it fail**

In `scripts/test-plugins.sh`, insert before the `validator=` line:

```bash
# The previous name appears only in the approved exceptions listed in
# docs/superpowers/plans/2026-09-29-clax-rename.md: the plans written before
# the rename, this rename's plan and the Svelte port plan written alongside
# it, and the spec's name-history note. It is assembled from two halves so
# this file is not an exception.
OLD="arti""fax"
name_exceptions=(
    docs/superpowers/plans/2026-09-28-phase-1-daemon-publish-viewer.md
    docs/superpowers/plans/2026-09-28-phase-2-mcp-and-plugins.md
    docs/superpowers/plans/2026-09-28-phase-2-scoped.md
    docs/superpowers/plans/2026-09-28-phase-3-comments-and-feedback.md
    docs/superpowers/plans/2026-09-28-phase-4-runtime-capabilities.md
    docs/superpowers/plans/2026-09-28-phase-5-room-and-sample.md
    docs/superpowers/plans/2026-09-29-clax-rename.md
    docs/superpowers/plans/2026-09-29-svelte-port.md
    docs/superpowers/specs/2026-09-28-clax-design.md
)
excludes=()
for f in "${name_exceptions[@]}"; do excludes+=(":(exclude)$f"); done
stray="$(git grep -il "$OLD" -- . "${excludes[@]}"; git ls-files | grep -i "$OLD")"
if [ -z "$stray" ]; then pass "the previous name appears only in the approved exceptions"
else fail "the previous name remains in: $(echo $stray | head -c 2000)"; fi
spec=docs/superpowers/specs/2026-09-28-clax-design.md
stray="$(awk -v old="$OLD" '
    /<!-- name-history:begin -->/ { on = 1; next }
    /<!-- name-history:end -->/ { on = 0; next }
    !on && index(tolower($0), old) { print NR }
' "$spec" 2>/dev/null)"
if [ -f "$spec" ] && [ -z "$stray" ]; then pass "$spec names the previous name only in its name-history note"
else fail "$spec is missing or names the previous name outside its name-history note (lines: $(echo $stray))"; fi
```

Run: `bash scripts/test-plugins.sh | grep -E 'previous name|name-history'`
Expected: `FAIL: the previous name remains in: …` listing the tree's files, and `PASS: docs/superpowers/specs/2026-09-28-clax-design.md names the previous name only in its name-history note`.

- [ ] **Step 5: Move every path that holds the name**

```bash
for c in cli core hooks mcp server; do git mv "crates/artifax-$c" "crates/clax-$c"; done
git mv plugins/artifax plugins/clax
git mv plugins/clax/skills/artifax plugins/clax/skills/clax
git mv plugins/claude-code/skills/artifax plugins/claude-code/skills/clax
git mv plugins/pi/skills/artifax plugins/pi/skills/clax
git mv scripts/ensure-artifax.sh scripts/ensure-clax.sh
git mv plugins/clax/scripts/ensure-artifax.sh plugins/clax/scripts/ensure-clax.sh
git mv plugins/claude-code/scripts/ensure-artifax.sh plugins/claude-code/scripts/ensure-clax.sh
git mv scripts/test-ensure-artifax.sh scripts/test-ensure-clax.sh
git mv plugins/pi/src/artifax.ts plugins/pi/src/clax.ts
git mv plugins/pi/test/artifax.test.ts plugins/pi/test/clax.test.ts
git ls-files | grep -i artifax | grep -v '^docs/superpowers/plans/'
```
Expected: the last command prints nothing.

- [ ] **Step 6: Run the sweep over every tracked file outside the exceptions**

```bash
git grep -lIiz artifax -- . \
  ':(exclude)docs/superpowers/plans/2026-09-28-*.md' \
  ':(exclude)docs/superpowers/plans/2026-09-29-clax-rename.md' \
  ':(exclude)docs/superpowers/plans/2026-09-29-svelte-port.md' \
  ':(exclude)docs/superpowers/specs/2026-09-28-clax-design.md' \
  | xargs -0 perl -pi -e 's/ARTIFAX/CLAX/g; s/Artifax/Clax/g; s/artifax/clax/g'
git grep -il artifax -- . \
  ':(exclude)docs/superpowers/plans/2026-09-28-*.md' \
  ':(exclude)docs/superpowers/plans/2026-09-29-clax-rename.md' \
  ':(exclude)docs/superpowers/plans/2026-09-29-svelte-port.md' \
  ':(exclude)docs/superpowers/specs/2026-09-28-clax-design.md'
```
Expected: the second command prints nothing. `git grep -I` skips binary files; there are no tracked binaries holding the name. (`git grep -z` writes NUL-separated names; on macOS `grep -Z` means `--decompress`, so it is not used here.)

Spot-check a few results:
```bash
grep -n '^name\|^\[\[bin\]\]' -A1 crates/clax-cli/Cargo.toml | head -6
grep -n 'COOKIE\|BRIDGE_PATH' crates/clax-server/src/viewer.rs crates/clax-core/src/wrap.rs | head -3
grep -n '"name"' plugins/pi/package.json .agents/plugins/marketplace.json | head -4
grep -n 'REPO=' scripts/ensure-clax.sh
```
Expected: `name = "clax-cli"`, `name = "clax"`; `COOKIE: &str = "clax_viewer"`, `BRIDGE_PATH: &str = "/_clax/bridge.js"`; `"@empathic/clax-pi"`, `"clax"`; `REPO="empathic/clax"`.

- [ ] **Step 7: Check articles**

Run: `git grep -nE '\b[Aa]n (Clax|clax|CLAX)\b'`
Expected: no output. For any line it prints, change "an" to "a" (or "An" to "A") in that line.

- [ ] **Step 8: Re-pad the installer's variable column, and keep the three copies identical**

The five one-line entries in the header comment lost three columns. Restore the description column:

```bash
perl -pi -e 's/^(#   CLAX_(?:BIN|SOURCE_DIR|HOME|INSTALL_DIR|CONFIG_DIR) +)(?=\S)/$1   /' scripts/ensure-clax.sh
sed -n '35,50p' scripts/ensure-clax.sh
cp scripts/ensure-clax.sh plugins/clax/scripts/ensure-clax.sh
cp scripts/ensure-clax.sh plugins/claude-code/scripts/ensure-clax.sh
cmp scripts/ensure-clax.sh plugins/clax/scripts/ensure-clax.sh && cmp scripts/ensure-clax.sh plugins/claude-code/scripts/ensure-clax.sh && echo same
```
Expected: the printed header shows each description starting in the same column as its continuation lines (column 27, as before the rename), and `same`.

- [ ] **Step 9: Regenerate the generated files**

```bash
cargo update --workspace
git diff --stat -- Cargo.lock
python3 - <<'PY'
import subprocess, re
def pkgs(text):
    return sorted(re.findall(r'^name = "([^"]+)"\nversion = "([^"]+)"', text, re.M))
old = subprocess.run(["git", "show", "HEAD:Cargo.lock"], capture_output=True, text=True, check=True).stdout
new = open("Cargo.lock").read()
mapped = sorted((n.replace("artifax", "clax"), v) for n, v in pkgs(old))
print("lock packages identical after the rename" if mapped == pkgs(new) else "LOCK DIFFERS")
PY
(cd web && npm install --package-lock-only --ignore-scripts --silent) && (cd plugins/pi && npm install --package-lock-only --ignore-scripts --silent)
git diff --stat -- web/package-lock.json plugins/pi/package-lock.json
grep -n '"name"' web/package-lock.json plugins/pi/package-lock.json | head -4
python3 scripts/sync-skill-tools.py && python3 scripts/sync-skill-tools.py --check && echo tools-ok
cargo fmt --all
rm -rf web/dist/_artifax web/dist/index.html
```
Expected: `lock packages identical after the rename`; the package-lock diffs show only the two `name` lines each (from the sweep; `npm install --package-lock-only` changed nothing more); `"name": "clax-web"` and `"name": "@empathic/clax-pi"`; `tools-ok`.

- [ ] **Step 10: Run the name gate again**

Run: `bash scripts/test-plugins.sh | grep -E 'previous name|name-history|FAIL'`
Expected: two `PASS` lines (the previous name, the name-history note) and no `FAIL`.

- [ ] **Step 11: Pin the clean break for the home, variables and daemon file**

Create `crates/clax-cli/tests/clean_break.rs`:

```rust
//! Clax never reads, migrates, or deletes its previous name's home, and never
//! reads its previous variables: a live-looking daemon.json there is ignored
//! and left exactly as it was.

use assert_cmd::Command;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The previous name, assembled so the repository's name gate finds no literal.
const OLD: &str = concat!("arti", "fax");

#[test]
fn the_previous_home_and_variables_are_ignored_and_left_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let old_home = dir.path().join(format!(".{OLD}"));
    std::fs::create_dir_all(&old_home).unwrap();

    // A daemon the old home names: this process's PID (alive) and a port that
    // answers /healthz and counts every connection.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let hits = Arc::new(AtomicUsize::new(0));
    let seen = hits.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            seen.fetch_add(1, Ordering::SeqCst);
            let mut buf = [0u8; 1024];
            let _ = s.read(&mut buf);
            let body = r#"{"version":"0.2.0","pid":1,"started_at":"2026-09-29T00:00:00Z"}"#;
            let _ = write!(
                s,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    let info = serde_json::json!({
        "port": port,
        "pid": std::process::id(),
        "token": "t",
        "started_at": "2026-09-29T00:00:00Z",
        "bind": "127.0.0.1",
        "version": "0.2.0",
    })
    .to_string();
    let daemon_json = old_home.join("daemon.json");
    std::fs::write(&daemon_json, &info).unwrap();

    let old_var = |suffix: &str| format!("{}_{suffix}", OLD.to_uppercase());
    let out = Command::cargo_bin("clax")
        .unwrap()
        .arg("stop")
        .env("HOME", dir.path())
        .env_remove("CLAX_HOME")
        .env("CLAX_CODEX_BIN", "")
        .env(old_var("HOME"), &old_home)
        .env(old_var("BIN"), "/nonexistent")
        .output()
        .unwrap();

    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "no clax daemon is running"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 0, "the old daemon.json was followed");
    assert_eq!(std::fs::read_to_string(&daemon_json).unwrap(), info);
    let entries: Vec<_> = std::fs::read_dir(&old_home)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(entries, vec![std::ffi::OsString::from("daemon.json")]);
}
```

Run: `cargo test -p clax-cli --test clean_break`
Expected: `test the_previous_home_and_variables_are_ignored_and_left_untouched ... ok`.

Check that it bites: in `crates/clax-core/src/home.rs`, temporarily change `h.join(".clax")` in `from_env_with` to `h.join(concat!(".arti", "fax"))`, run `cargo test -p clax-cli --test clean_break` and expect FAIL (the stop follows the old daemon.json: `hits` is nonzero and stdout is not `no clax daemon is running`). Undo that one-line edit by hand, rerun, and expect `ok` again.

- [ ] **Step 12: Pin the clean break for the installer**

In `scripts/test-ensure-clax.sh`, insert before the line `# --- Hook mode never downloads and never fails` :

```bash
# --- No alias for the previous name -------------------------------------------
# Its variables, its binary on PATH, its home's bin directory and a Codex
# marketplace under its name are all ignored. The name is assembled from two
# halves so the repository's name gate finds no literal.
OLD="arti""fax"
OLD_UPPER="ARTI""FAX"
new_env
fake_clax "$SANDBOX/elsewhere" "clax 0.2.0"
printf '#!/bin/sh\necho "%s 0.2.0"\n' "$OLD" > "$FAKEBIN/$OLD"
chmod +x "$FAKEBIN/$OLD"
mkdir -p "$HOME/.$OLD/bin"
cp "$FAKEBIN/$OLD" "$HOME/.$OLD/bin/$OLD"
fake_checkout "$SANDBOX/repo"
fake_clax "$SANDBOX/repo/target/debug" "clax 0.2.0"
plugin_copy "$SANDBOX/codex-home/plugins/cache/clax/clax/0.2.0"
printf '[marketplaces.%s]\nsource_type = "local"\nsource = "%s"\n' "$OLD" "$SANDBOX/repo" > "$SANDBOX/codex-home/config.toml"
export "${OLD_UPPER}_BIN=$SANDBOX/elsewhere/clax" "${OLD_UPPER}_HOME=$HOME/.$OLD" "${OLD_UPPER}_SOURCE_DIR=$SANDBOX/repo"
run_copy "$SANDBOX/codex-home/plugins/cache/clax/clax/0.2.0/scripts/ensure-clax.sh" exec hook --agent codex stop
unset "${OLD_UPPER}_BIN" "${OLD_UPPER}_HOME" "${OLD_UPPER}_SOURCE_DIR"
if [ "$RC" = 0 ] && [ -z "$OUT" ] && echo "$ERR" | grep -q "no binary found" \
    && [ ! -e "$HOME/.$OLD/logs" ] && [ -s "$HOME/.clax/logs/hooks.log" ] \
    && [ "$(PATH="$ORIG_PATH" ls -A "$HOME/.$OLD")" = bin ] && [ "$(PATH="$ORIG_PATH" ls -A "$HOME/.$OLD/bin")" = "$OLD" ]; then
    pass "the previous name's variables, binary, home and marketplace are ignored and left untouched"
else fail "the previous name's variables, binary, home and marketplace are ignored and left untouched (rc=$RC out=$OUT err=$ERR)"; fi
```

The sandbox PATH holds only the tools the installer needs, and `ls` is not one of them, so the two `ls` calls run with the test's own `ORIG_PATH`; the installer's PATH stays as it is.

Run: `bash scripts/test-ensure-clax.sh | grep -E 'previous name|FAIL|all installer'`
Expected: `PASS: the previous name's variables, binary, home and marketplace are ignored and left untouched` and `all installer tests passed`.

- [ ] **Step 13: Pin that old plugin cache copies are not this plugin**

In `crates/clax-cli/src/commands/doctor_agent.rs`, in the `tests` module after `a_current_codex_cache_copy_passes_plugin_and_skill`, add:

```rust
    #[test]
    fn a_cache_copy_under_the_previous_name_is_not_this_plugin() {
        const OLD: &str = concat!("arti", "fax");
        let f = Fixture::new();
        f.plugin(
            &format!(".codex/plugins/cache/{OLD}/{OLD}/0.2.0"),
            ".codex-plugin/plugin.json",
            V,
            DoctorAgent::Codex,
        );
        let (p, found) = plugin_check(DoctorAgent::Codex, &f.dirs(), V);
        assert_eq!(p["ok"], false, "{p}");
        assert_eq!(found, None);
    }
```

Run: `cargo test -p clax-cli --bin clax a_cache_copy_under_the_previous_name_is_not_this_plugin`
Expected: `... ok`.

- [ ] **Step 14: Pin that the old viewer cookie names no viewer**

At the end of `crates/clax-server/src/viewer.rs`, add:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_clax_viewer_cookie_names_a_viewer() {
        // Cookies on localhost ignore the port, so the previous daemon's
        // cookie reaches this one; it never names a viewer here.
        const OLD: &str = concat!("arti", "fax");
        let id = "01J9Z3K4M5N6P7Q8R9S0T1V2W3";
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, HeaderValue::from_str(&format!("{OLD}_viewer={id}")).unwrap());
        assert_eq!(read(&h), None);
        h.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("{OLD}_viewer={id}; clax_viewer={id}")).unwrap(),
        );
        assert_eq!(read(&h).as_deref(), Some(id));
    }
}
```

Run: `cargo test -p clax-server --lib only_the_clax_viewer_cookie_names_a_viewer`
Expected: `... ok`.

- [ ] **Step 15: Format, then run every gate**

```bash
cargo fmt --all
bash scripts/quality_gates.sh; echo "exit=$?"
```
Expected: every gate prints `ok`, then `all gates passed` and `exit=0`. (`web build` recreates `web/dist/_clax/`; `web e2e` runs both frame modes; `installer` and `plugins` include Steps 4 and 12; the gates start their daemons in temp homes on free ports.)

- [ ] **Step 16: Commit**

```bash
git add -u -- .
git add crates/clax-cli/tests/clean_break.rs
git status --porcelain | grep -v '^[RMAD] ' ; true
git commit --no-gpg-sign -m "Rename Artifax to Clax with a clean break

The binary, crates, packages, plugins, skill, home (~/.clax), CLAX_*
variables, clax: message prefix, /_clax/ routes, x-clax-* headers,
clax_viewer cookie, window.__clax and log paths all carry the new name.
Nothing reads, migrates or deletes the old home or variables; tests pin
that the old daemon.json, variables, binary, plugin caches and cookie
are ignored, and test-plugins.sh keeps the old name to the approved
exceptions."
```
`git add -u -- .` stages the moves, the sweep and the edits to tracked files; it adds nothing untracked. The `git status` filter must print nothing (no untracked or unstaged leftovers). Note: `git add -u` touches only files git already tracks in this checkout, never `.claude/worktrees` (untracked, ignored).

---

### Task 2: Verify the rename from the outside

**Files:**
- Modify: only what a failed check below points at (expected: nothing).

**Interfaces:**
- Consumes: Task 1's commit (`HEAD`) and `BASE` (`HEAD~1`).
- Produces: evidence; no new names.

- [ ] **Step 1: The old name is only in the approved exceptions**

```bash
git grep -il artifax | sort
git ls-files | grep -i artifax
```
Expected output of the first command, exactly:
```
docs/superpowers/plans/2026-09-28-phase-1-daemon-publish-viewer.md
docs/superpowers/plans/2026-09-28-phase-2-mcp-and-plugins.md
docs/superpowers/plans/2026-09-28-phase-2-scoped.md
docs/superpowers/plans/2026-09-28-phase-3-comments-and-feedback.md
docs/superpowers/plans/2026-09-28-phase-4-runtime-capabilities.md
docs/superpowers/plans/2026-09-28-phase-5-room-and-sample.md
docs/superpowers/plans/2026-09-29-clax-rename.md
docs/superpowers/plans/2026-09-29-svelte-port.md
docs/superpowers/specs/2026-09-28-clax-design.md
```
The second prints nothing. Then:
```bash
awk '/<!-- name-history:begin -->/{on=1;next} /<!-- name-history:end -->/{on=0;next} !on && tolower($0) ~ /artifax/' docs/superpowers/specs/2026-09-28-clax-design.md
git diff --stat HEAD~1 HEAD -- 'docs/superpowers/plans/2026-09-28-*.md'
```
Expected: both print nothing (the spec names the old name only in its note; the historical plans are byte-for-byte unchanged).

- [ ] **Step 2: Every change is the rename, except the listed hand edits**

Files are paired by name (`rename(old path) == new path`) from each commit's tree, not by git's rename detection: that detection pairs the three identical installer copies crosswise and misses files whose small size puts them under its similarity threshold. Rust files are run through `rustfmt` on both sides, because `cargo fmt` re-sorts imports once `clax_*` sorts after `anyhow`/`axum`, which whitespace normalization cannot hide. The plans are exceptions and are compared unswept. `NEW` is Task 1's commit (`f06b04a`).

```bash
NEW=f06b04a python3 - <<'PY'
import os, re, subprocess
NEW = os.environ["NEW"]; OLD = NEW + "~1"
def git(*a):
    return subprocess.run(["git", *a], capture_output=True, text=True, check=True).stdout
def rename(s):
    return s.replace("ARTIFAX", "CLAX").replace("Artifax", "Clax").replace("artifax", "clax")
def rustfmt(s):
    return subprocess.run(["rustfmt", "--edition", "2024", "--emit", "stdout"],
                          input=s, capture_output=True, text=True, check=True).stdout
norm = lambda s: re.sub(r"\s+", "", s)
before = set(git("ls-tree", "-r", "--name-only", OLD).splitlines())
after = set(git("ls-tree", "-r", "--name-only", NEW).splitlines())
other = []
for old in sorted(before):
    new = rename(old)
    if new not in after:
        other.append("deleted " + old); continue
    a, b = git("show", f"{OLD}:{old}"), git("show", f"{NEW}:{new}")
    if not old.startswith("docs/superpowers/plans/"):
        a = rename(a)
    if new.endswith(".rs"):
        a, b = rustfmt(a), rustfmt(b)
    if norm(a) != norm(b):
        other.append("content " + new)
for new in sorted(after - {rename(p) for p in before}):
    other.append("added " + new)
print("\n".join(other))
PY
```
Expected output, exactly (the hand edits and the regenerated lockfile, whose package blocks are re-sorted; whitespace-only changes, the re-padded installer header, READMEs and spec diagrams, and the rewrapped tool blocks compare equal):
```
content Cargo.lock
content crates/clax-cli/src/commands/doctor_agent.rs
content crates/clax-server/src/viewer.rs
content docs/superpowers/specs/2026-09-28-clax-design.md
content scripts/test-ensure-clax.sh
content scripts/test-plugins.sh
added crates/clax-cli/tests/clean_break.rs
```
Review each listed file's change by hand against the swept, formatted BASE text (for example `diff <(git show f06b04a~1:<old path> | perl -pe 's/ARTIFAX/CLAX/g; s/Artifax/Clax/g; s/artifax/clax/g') <(git show f06b04a:<path>)`): only the steps of Task 1 that name that file may appear.

- [ ] **Step 3: The workspace, binary and lockfiles carry only the new name**

```bash
cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(sorted(p["name"] for p in json.load(sys.stdin)["packages"]))'
cargo build -q -p clax-cli && target/debug/clax --version
cargo metadata --locked --format-version 1 >/dev/null && echo lock-ok
(cd web && npm ci --silent) && (cd plugins/pi && npm ci --silent) && echo npm-ok
```
Expected: `['clax-cli', 'clax-core', 'clax-hooks', 'clax-mcp', 'clax-server']`, `clax 0.2.0`, `lock-ok`, `npm-ok`.

- [ ] **Step 4: The built web UI and the release binary hold no old name**

```bash
rm -rf web/dist/_artifax web/dist/_clax web/dist/index.html
(cd web && npm run build >/dev/null) && ls web/dist
git ls-files -o --exclude-standard web/dist; grep -rli artifax web/dist; echo "web-dist-grep-exit=$?"
cargo build -q --release -p clax-cli
LC_ALL=C grep -a -o -i '[[:print:]]*artifax[[:print:]]*' target/release/clax | grep -v "$(pwd -P)" | sort -u
```
Expected: `ls` shows `_clax` and `index.html` (plus `.gitkeep`); the `grep -rli` prints nothing and `web-dist-grep-exit=1`; the last command prints nothing (any hit in the binary is a source path under this unrenamed checkout, which the filter removes; a hit outside it is a missed rename).

- [ ] **Step 5: A hermetic run lays out the new home**

```bash
T="$(mktemp -d)"
mkdir -p "$T/fakehome"
printf '<!doctype html><title>Smoke</title><h1>Smoke</h1>' > "$T/index.html"
env HOME="$T/fakehome" CLAX_HOME="$T/home" CLAX_NO_OPEN=1 CLAX_CODEX_BIN= \
  target/debug/clax --port 0 publish "$T/index.html"
ls "$T/home"
python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print(d["port"] != 7480, d["version"])' "$T/home/daemon.json"
env HOME="$T/fakehome" CLAX_HOME="$T/home" target/debug/clax stop
ls -A "$T/fakehome"
rm -rf "$T"
```
Expected: the publish prints the artifact's URL; `ls` lists `clax.db` and `daemon.json` (with `artifacts`, `logs`, `daemon.lock` and SQLite's `clax.db-shm` and `clax.db-wal`); `True 0.2.0`; `clax daemon stopped`; the fake HOME is empty (nothing was written outside `CLAX_HOME`).

- [ ] **Step 6: Run every gate**

Run: `bash scripts/quality_gates.sh; echo "exit=$?"`
Expected: `all gates passed` and `exit=0`. In its output, `web e2e` has run the suite in both frame modes (the `viewer renders content with the bridge` test logs `frame mode: subdomain` or `frame mode: sandboxed fallback`, and the parametrised specs run once per mode).

- [ ] **Step 7: Commit any fix**

If Steps 1–6 needed a fix, stage the fixed files by name and commit:
```bash
git add <each fixed path>
git commit --no-gpg-sign -m "Finish the Clax rename: <what the check found>"
```
If nothing needed fixing, make no commit.

---

## Steps for the person (not for agents)

Agents never run these; they touch `~/.claude`, `~/.codex`, the old home and the GitHub repository.

1. Rename the GitHub repository `empathic/artifax` to `empathic/clax` (the installer, `Cargo.toml` and the Pi package now point there), and update the remote if you want: `git remote set-url origin git@github.com:empathic/clax.git`.
2. Stop the old daemon if it runs: `artifax stop` (the old binary).
3. Codex: `codex plugin remove artifax@artifax`, delete the `[marketplaces.artifax]` and `[plugins."artifax@artifax"]` sections from `~/.codex/config.toml`, then `codex plugin marketplace add /Users/alex/Devel/empathic/artifax` (it registers the marketplace as `clax`) and `codex plugin add clax@clax`. The installer finds this checkout through `[marketplaces.clax]`'s `source` and the `clax-cli` line in its `Cargo.toml`, so the unrenamed directory works.
4. Claude Code: `/plugin uninstall artifax@artifax`, `/plugin marketplace remove artifax`, then `/plugin marketplace add /Users/alex/Devel/empathic/artifax` and `/plugin install clax@clax`.
5. Pi: replace the `@empathic/artifax-pi` entry in Pi's settings with the local path `…/plugins/pi` (now `@empathic/clax-pi`).
6. `just install` builds and installs `clax`; `cargo uninstall artifax-cli` and `rm ~/.local/bin/artifax` remove the old one.
7. `~/.artifax` is yours to keep or delete; Clax starts empty in `~/.clax`.
8. Unset any `ARTIFAX_*` variables in your shell profile and set the `CLAX_*` equivalents you need.
