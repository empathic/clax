# The Clax App's Core In Process Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The Clax app runs Clax's core as a library in its own process on Android, macOS and iOS (no daemon, no port, no HTTP), its WebViews load from a scheme the app answers, its queue of makes and changes runs in a Dart worker isolate over durable job steps in the core, and the seven JSON state files become core tables.

**Architecture:** A new crate, `crates/clax-embed`, starts the core that `clax serve` runs (store, `AppState`, background tasks, router) without a listener, behind four C functions; `call` feeds a binary request frame to the axum router through `tower::ServiceExt::oneshot` and posts the response frame to a Dart port, and `subscribe` pumps `GET /api/stream`'s body to a port. The shell in app mode replaces its document's `fetch` with one that relays `/api/` over the `ClaxHost` channel and streams pushed events; the host answers every other request from a scheme (`https://*.clax.invalid` on Android, `clax://` on Apple) through the core. The queue is an Elm-shaped model (`update(model, msg) → (model, effects)`) in a worker isolate; the core keeps jobs and steps, and a publish step commits with its version.

**Tech Stack:** Rust 1.94.0 (axum 0.8, tower, tokio, rusqlite via `clax-store`, `roxmltree` 0.20 new), rules_rust with the NDK and rules_apple, Flutter 3.47.7 / Dart 3 (`dart:ffi`, `dart:isolate`, `flutter_test`), Kotlin (androidx.webkit `WebViewCompat`, WorkManager), Swift (`WKURLSchemeHandler`), TypeScript (Svelte shell, vitest, Playwright), Bash.

**Spec:** `docs/superpowers/specs/2026-10-19-in-process-core-design.md` (the "spec"; §N below are its sections). Read the section a task names before starting it. Open owner decisions on the spec: `"$Q" exec threads docs/superpowers/specs/2026-10-19-in-process-core-design.md --status needs-decision`. Tasks that depend on one say so; every other task runs without it.

## Global Constraints

- **No build carrying an unmerged migration of this plan is installed on the owner's devices** except in Task 33, after the migration's number is settled (spec §10.2).
- **Commits are unsigned:** `git -c commit.gpgsign=false commit …`. `git add` only the paths your task lists (never `git add -A`, `.` or `commit -a`; `apps/clax_app/analysis_options.yaml` carries the owner's uncommitted change and must never be staged). Never push. Never file, close or comment on issues or pull requests. If a git operation fails on signing or 1Password, stop and report.
- **Git configuration:** never change shared git configuration, and never use bare `git stash`.
- **Homes and daemons:** never run anything against `~/.clax`, the daemon on port 7480, or the owner's app data (the phone's `dev.empathic.clax` data, the Mac's `~/Library/Containers/dev.empathic.clax`) except Task 33's read-only copy. Tests use temporary homes. Never wait on processes with `pgrep -f`/`pkill -f`; use the PID you started, or an exit file, with a timeout.
- **Bazel only.** Build and test with `bazelisk build|test|run //…` (`docs/bazel.md`). No `cargo`, `pnpm`, `npm`, `flutter` or `dart` command in the worktree except through a `-native` recipe. After a `Cargo.toml` or `Cargo.lock` change, `just bazel-sync` and commit what it writes with the change. Rust unit tests: `bazelisk test //crates/<crate>:<lib>_lib_test`; integration: `bazelisk test //crates/<crate>:all --test_arg=<module>::`. Flutter: `bazelisk test //apps/clax_app:<dir>_<file>` (a new test directory is added to `SUITES` in `apps/clax_app/BUILD.bazel`; a new test-support file to `TEST_SUPPORT`). Web unit: `bazelisk test //web:unit --test_arg=<path under web/>`. Web e2e: `bazelisk test //web:e2e_rest --test_arg=<spec>`. Anything longer than a couple of minutes runs in the background, logged to `target/plan-logs/core-<task>.log`, waited on in bounded stretches.
- **Deterministic tests:** no sleeps standing in for a condition, no wall-clock budgets. Timing rules run on an injected clock: `tokio::time::pause()` in Rust, `Clock` (`apps/clax_app/lib/daemon/clock.dart`, moved to `lib/platform/clock.dart` in Task 30) and `fakeAsync` in Dart, `vi.useFakeTimers()` in vitest, `page.clock` in Playwright. The whole check suite stays under two minutes on a warm cache.
- **`unsafe`:** the workspace forbids it. From Task 7 the one exception is `crates/clax-embed/src/ffi.rs` as spec §14 words it; until Task 31 removes it, clax-ios's existing exception stands beside it. No hard links, anywhere.
- **Layering:** `"clax-embed": SERVER | {"clax-embedded"}` in `scripts/check-crate-layers.py`; no crate turns on another's features for tests; test support lives in `clax-testkit` and `clax-fake-exe`.
- **Prose:** "ID" in prose, comments and commit messages; `id` only as a literal symbol. Doc comments and commit messages describe the contract or the change, never this plan, the conversation, or a rating of quality.
- **Copy** the person sees follows spec §15: "Waiting for Claude", "Can't reach Claude", "Paused — open Clax to keep building", "Open Clax to keep building <title>", "Clax's builder stopped. Restart Clax to go on."; no transport state for local work.
- **Qualifier** runs as `Q="/Users/alex/.claude/plugins/cache/qualifier/qual/0.1.0/scripts/ensure-qualifier.sh"; "$Q" exec <args>`. Read `"$Q" exec agents` and each topic it lists before your session's first record. Before editing a file, `"$Q" exec threads <path>` and read what is open on it. More than two writes go in one `"$Q" exec record --stdin` batch written to a file outside the repository, dry-run first. Records cite paths, line ranges and command output, never this plan. Close only a record your session wrote or one your own commit fixed, with evidence and `--ref git:<sha>`, in a separate commit; propose every other close in the task report.
- **UI is looked at:** a task that changes what a person sees is done only once it was seen running (the macOS debug app, or the phone) and its screenshots opened with the Read tool. "Tests pass" is not verification for UI.
- **Order:** phases run in order; main is never broken. Phase 2's Dart code is reached only by tests until Task 16 switches the app to the core.
- Before the branch merges, `just check` runs once (in the background) and `"$Q" exec diff main` is read; both results go in the final report.

## Review Focus

1. **Kill mid-publish.** The app dies after a publish step's content files are written and before its transaction commits, or after the commit and before the reply step. A person expects one version and one reply after the next launch. Test: Task 25 (`api_jobs::a_step_cut_before_commit_leaves_no_version_and_resumes_once`, `api_jobs::a_replayed_publish_step_publishes_once`), Task 28 (`integration_test/kill_resume_test.dart`).
2. **A double resume.** Two workers (a crash-restarted worker isolate, or a WorkManager wake beside a launch) resume the same job. A person expects it done once, with at most a repeated model turn. Test: Task 24 (`jobs::a_held_lease_refuses_a_second_taker`), Task 19 (`update_test.dart` "a job whose lease another worker holds is left alone").
3. **A WebView reload during a job.** The page or the whole UI engine goes away while the worker waits on a render. A person expects the job to wait, render again when it can, and not fail or spend an extra turn. Test: Task 21 (`render_bridge_test.dart` "a render lost to a reload is asked again once, then waits"), Task 19 (property: no `StartTurn` follows `RenderLost`).
4. **A migration on a large old home.** The owner's phone home at this branch's 43. A person expects a backup, every artifact kept, and a splash that moves. Test: Task 18 (`old_homes::the_installed_flutter_app_home_backs_up_and_migrates_to_44`, `old_homes::the_perf_home_opens_through_the_core`).
5. **Origin isolation.** A page tries to read the shell's document or the API, post on `ClaxHost`, or frame the shell. A person expects nothing to leak. Test: Task 11 (`app-transport.spec.ts` "a page reads nothing of the shell"), Task 5 (`embed::a_content_frame_reaches_no_api`), Task 14's spike checklist on both platforms.
6. **Two isolates writing.** The UI posts a comment while the worker publishes on the same app. A person expects the comment kept and the publish re-planned on a moved base. Test: Task 25 (`api_jobs::a_step_on_a_moved_base_records_nothing`), Task 19 (`update_test.dart` "version_moved re-plans, never republishes").

---

## Order and parallel groups

| Phase | Groups (tasks in a group touch disjoint files and may run in parallel) |
|---|---|
| 1 clax-embed | **1a** T1, T3 · **1b** T2, T4 · **1c** T5 · **1d** T6, T7 · **1e** T8 |
| 2 Shell transport and scheme | **2a** T9, T12 · **2b** T10, T13 · **2c** T11, T14 · **2d** T15 |
| 3 Off the daemon | **3a** T18 (baseline and fixtures first) · **3b** T16 · **3c** T17 |
| 4 Queue | **4a** T19 · **4b** T20 · **4c** T21, T23 · **4d** T22 |
| 5 Durable steps, checks | **5a** T24, T26 · **5b** T25, T27 · **5c** T28 |
| 6 Import | **6a** T29 |
| 7 Delete and finish | **7a** T30, T31 · **7b** T32 · **7c** T33 |

## Commands used throughout

```bash
Q="/Users/alex/.claude/plugins/cache/qualifier/qual/0.1.0/scripts/ensure-qualifier.sh"
mkdir -p target/plan-logs
# Every Rust task, before its commit:
bazelisk build --config=fmt --config=clippy //crates/<crate>/...
bazelisk test //scripts:no_unsafe //scripts:crate_layers
# Every Dart task, before its commit:
bazelisk test //apps/clax_app:dart_analyze
# Every web task, before its commit:
bazelisk test //web:lint //web:typecheck
# The macOS debug app, to look at a change:
just app-macos-dev
```

---

## Phase 1: `crates/clax-embed` with `call` and events (spec §2)

### Task 1: The core without a listener (`clax_server::core`)

**Files:**
- Create: `crates/clax-server/src/core.rs`
- Modify: `crates/clax-server/src/serve.rs:344-640` (`serve` over `Core`), `crates/clax-server/src/lib.rs:145-160` (`pub mod core;`)
- Test: `crates/clax-server/tests/core_start.rs` (declared in `crates/clax-server/tests/integration.rs` as `mod core_start;`)

**Interfaces:**
- Produces:
  ```rust
  // crates/clax-server/src/core.rs
  /// What `serve` runs, without a listener: the store, the state, the
  /// journal, the listeners, the router and the six background tasks but
  /// the stale watcher.
  pub struct CoreConfig {
      pub home: clax_core::Home,
      pub version: &'static str,
      pub reap_interval: std::time::Duration,
      pub codex: clax_realtime::push::CodexPush,
      pub sample: std::sync::Arc<clax_sample::Sampler>,
      pub web: std::sync::Arc<dyn clax_core::embedded::FileTree>,
      pub hosted: Option<std::sync::Arc<clax_http::hosted::Hosted>>,
      pub require_credential: bool,
      /// The address and port `self_base` and the host checks name; the
      /// embedded core passes 127.0.0.1 and 0.
      pub bind: std::net::IpAddr,
      pub port: u16,
      /// Set by an embedding app (Task 2); `None` for `clax serve`.
      pub embed: Option<clax_http::state::EmbedOrigins>,
  }
  pub struct Core { /* state, router, task handles, shutdown sender */ }
  impl Core {
      /// Opens the store (backup, adoption, migrations; `progress` hears
      /// each phase) and starts everything. Runs on the caller's Tokio runtime.
      pub async fn start(cfg: CoreConfig, progress: &mut dyn FnMut(clax_store::OpenProgress)) -> anyhow::Result<Core>;
      pub fn state(&self) -> &clax_http::state::AppState;
      /// The router with `/api/admin/shutdown` wired to `shutdown_requested`.
      pub fn router(&self) -> axum::Router;
      /// Resolves when the shutdown route or `stop` asked.
      pub fn shutdown_requested(&self) -> tokio::sync::watch::Receiver<bool>;
      /// Ends the tasks, stops the journal, shuts the store down: what
      /// `serve` does after its drain.
      pub async fn stop(self);
  }
  ```
  (`clax_store::OpenProgress` is whatever `Store::open_reporting`'s callback takes today; use that type by name.)
- Consumes: nothing.

- [ ] **Step 1: Read the threads** on `crates/clax-server/src/serve.rs` and `crates/clax-server/src/lib.rs`.
- [ ] **Step 2: Write the failing test** `crates/clax-server/tests/core_start.rs`:
  ```rust
  use clax_server::core::{Core, CoreConfig};
  use tower::ServiceExt;

  fn cfg(home: &std::path::Path) -> CoreConfig {
      CoreConfig {
          home: clax_core::Home::at(home.to_path_buf()),
          version: "test",
          reap_interval: std::time::Duration::from_secs(60),
          codex: clax_realtime::push::CodexPush::disabled(),
          sample: std::sync::Arc::new(clax_sample::Sampler::disabled()),
          web: clax_testkit::web_ui(),
          hosted: None,
          require_credential: true,
          bind: std::net::Ipv4Addr::LOCALHOST.into(),
          port: 0,
          embed: None,
      }
  }

  #[tokio::test]
  async fn a_core_answers_its_router_with_no_listener_and_writes_no_daemon_json() {
      let dir = tempfile::tempdir().unwrap();
      let core = Core::start(cfg(dir.path()), &mut |_| {}).await.unwrap();
      let req = axum::http::Request::get("/healthz")
          .header("host", "localhost")
          .body(axum::body::Body::empty())
          .unwrap();
      let res = core.router().oneshot(req).await.unwrap();
      assert_eq!(res.status(), 200);
      assert!(!dir.path().join("daemon.json").exists());
      core.stop().await;
  }

  #[tokio::test]
  async fn stopping_a_core_closes_its_store_so_another_opens_the_home() {
      let dir = tempfile::tempdir().unwrap();
      let core = Core::start(cfg(dir.path()), &mut |_| {}).await.unwrap();
      core.stop().await;
      let again = Core::start(cfg(dir.path()), &mut |_| {}).await.unwrap();
      again.stop().await;
  }
  ```
  Use the constructors `CodexPush`, `Sampler` and the test web UI already offer (`grep -n "pub fn" crates/clax-realtime/src/push.rs crates/clax-sample/src/lib.rs crates/clax-testkit/src/*.rs`); if `disabled()` is named otherwise, use the existing name and keep the test's meaning.
- [ ] **Step 3: Run it** — `bazelisk test //crates/clax-server:all --test_arg=core_start::` — FAIL: `unresolved import clax_server::core`.
- [ ] **Step 4: Move the code.** Cut from `serve()` into `Core::start` everything from `Store::open_reporting` to `build_router_with_shutdown` except `bind_first_free`, `write_daemon_info` and the `stale` task; `self_base` and `browser_base` come from `cfg.bind`/`cfg.port` as `serve` computes them. Cut the post-drain sequence (abort tasks, journal stop, `store.shutdown()`) into `Core::stop`. `serve()` becomes: `ensure_dirs`, `Starting::begin`, file limit, bind, `Core::start` (with `port` the bound one and `starting.progress` as `progress`), `write_daemon_info`, the stale watcher, `axum::serve(..., core.router()...)` with the same graceful shutdown (its select also on `core.shutdown_requested()`), drain, `core.stop().await`, remove `daemon.json`. `DaemonInfo.pid`/`exe` stay in `serve`.
- [ ] **Step 5: Run** the new test and every serve test: `bazelisk test //crates/clax-server:all //crates/clax-cli:all //crates/clax-ios:all` — PASS.
- [ ] **Step 6: Commit** `crates/clax-server/src/core.rs crates/clax-server/src/serve.rs crates/clax-server/src/lib.rs crates/clax-server/tests/core_start.rs crates/clax-server/tests/integration.rs`: "Start the core's store, state, tasks and router without a listener, and serve over it".

### Task 2: The embedded app's origins in the state, the shell document and the content policy

**Files:**
- Modify: `crates/clax-http/src/state.rs:62-120` (`EmbedOrigins`, `AppState.embed`, `StateParts.embed`), `crates/clax-http/src/content_policy.rs:30-80` (`frame-ancestors` from the embed), `crates/clax-server/src/shell/mod.rs` and `route.rs` (every shell document gains the `clax-embed` meta when `embed` is set), `crates/clax-worker/src/instance.rs:428` (passes `embed: None`)
- Test: `crates/clax-http/src/content_policy.rs` (unit tests module), `crates/clax-server/tests/shell_boot.rs` (new module in `integration.rs`)

**Interfaces:**
- Produces:
  ```rust
  /// The origins an app embedding the core gives its WebViews (spec §4.1).
  #[derive(Clone, Debug, PartialEq, Eq)]
  pub struct EmbedOrigins {
      /// The shell's origin, e.g. "https://app.clax.invalid" or "clax://app".
      pub shell: String,
      /// An artifact's content origin with `{id}` for its ID, e.g.
      /// "https://{id}.c.clax.invalid" or "clax://{id}.c".
      pub content: String,
  }
  impl EmbedOrigins { pub fn content_of(&self, id: &str) -> String; }
  // AppState: pub embed: Option<EmbedOrigins>
  ```
  Every shell document the core renders gains, only when `embed` is set, `<meta name="clax-embed" content='{"transport":"host","shell":"<shell>","content":"<content template>"}'>` in its `<head>` before any script (attribute-escaped JSON). An artifact view's boot `frame` is subdomain mode with `src` on the content origin.
- Consumes: Task 1 (`CoreConfig.embed`).

- [ ] **Step 1: Read the threads** on the four files.
- [ ] **Step 2: Write failing tests.** In `content_policy.rs`'s tests:
  ```rust
  #[test]
  fn an_embedded_artifact_origin_is_framed_only_by_the_shell() {
      let embed = EmbedOrigins { shell: "clax://app".into(), content: "clax://{id}.c".into() };
      let v = content_policy_embedded(&embed, None);
      let s = v.to_str().unwrap();
      assert!(s.contains("frame-ancestors clax://app"), "{s}");
      assert!(!s.contains("localhost"), "{s}");
  }
  ```
  In `tests/shell_boot.rs`: start a testkit daemon whose state has `embed: Some(…)` (`clax_testkit` builds `StateParts`; add an `embed` option to its builder) and assert `GET /a/<id>?app=1` carries the `clax-embed` meta with `transport == "host"` and the two origins before the first `<script`, and its boot `frame.src` starts with the content origin; a daemon without `embed` has no such meta.
- [ ] **Step 3: Run** — `bazelisk test //crates/clax-http:clax_http_lib_test //crates/clax-server:all --test_arg=shell_boot::` — FAIL.
- [ ] **Step 4: Implement.** `content_policy` takes `embed: Option<&EmbedOrigins>` (thread it from `rewrite_artifact_host`, which has the state): on an artifact origin with an embed, `frame-ancestors <shell>`; everything else unchanged. Add `content_policy_embedded` as the test seam if the existing signature makes the direct test awkward. The shell renderer writes the meta; the boot's frame uses `EmbedOrigins::content_of`. `StateParts.embed` defaults to `None` everywhere it is built (daemon, worker, testkit).
- [ ] **Step 5: Run** the tests above plus `bazelisk test //crates/clax-http:all //crates/clax-server:all` — PASS; `scripts/check-wasm.sh` via `bazelisk test //scripts:wasm32` — PASS.
- [ ] **Step 6: Commit** the listed files: "Give an embedding app's shell and content origins to the shell documents and the content policy".

### Task 3: The `clax-embed` crate and the shared-library build

**Files:**
- Create: `crates/clax-embed/Cargo.toml`, `crates/clax-embed/BUILD.bazel`, `crates/clax-embed/src/lib.rs`, `crates/clax-embed/src/web.rs` (copy of `crates/clax-ios/src/web.rs`), `crates/clax-embed/src/log.rs` (copy of `crates/clax-ios/src/log.rs`)
- Modify: `Cargo.toml` (members), `Cargo.lock`, `bazel/cargo.bzl:21-32,71,211` (`cdylib` option declaring `<lib>_shared`), `scripts/check-crate-layers.py` (`"clax-embed": SERVER | {"clax-embedded"}`), `scripts/check-no-unsafe.sh:46` (`%OWN_LINTS` gains `crates/clax-embed/Cargo.toml` and a rule that it denies `unsafe_code`, as clax-ios's), `scripts/test-check-no-unsafe.sh` (a clax-embed crate in the tree), `bazel/workspace.bzl`, `MODULE.bazel.lock` (by `just bazel-sync`)

**Interfaces:**
- Produces `Cargo.toml`:
  ```toml
  [package]
  name = "clax-embed"
  version.workspace = true
  edition.workspace = true
  license.workspace = true

  # The workspace forbids unsafe_code; this crate denies it and allows it
  # only in src/ffi.rs (AGENTS.md; scripts/check-no-unsafe.sh checks it).
  [lints.rust]
  unsafe_code = "deny"
  unexpected_cfgs = { level = "warn", check-cfg = ['cfg(clax_embedded)', 'cfg(clax_split)', 'cfg(clax_split, values(any()))'] }

  [lib]
  crate-type = ["rlib", "cdylib", "staticlib"]

  [dependencies]
  clax-core = { path = "../clax-core" }
  clax-server = { path = "../clax-server" }
  clax-sample = { path = "../clax-sample" }
  clax-realtime = { path = "../clax-realtime" }
  clax-http = { path = "../clax-http" }
  clax-embedded = { path = "../clax-embedded" }
  clax-store = { path = "../clax-store" }
  anyhow.workspace = true
  axum.workspace = true
  bytes.workspace = true
  futures.workspace = true
  http-body-util.workspace = true
  serde.workspace = true
  serde_json.workspace = true
  tokio = { workspace = true, features = ["rt-multi-thread", "sync", "time", "macros"] }
  tower.workspace = true
  tracing.workspace = true
  tracing-subscriber.workspace = true

  [dev-dependencies]
  tempfile.workspace = true
  ```
  (Each `workspace = true` must exist in the root `[workspace.dependencies]`; add any missing one at the version `Cargo.lock` already resolves.)
- Produces `bazel/cargo.bzl`: `cargo_package(..., cdylib = False)`; with `cdylib = True`, `<lib>_shared`, a `rust_shared_library` of the crate (same deps and flags as `<lib>_static`).
- Produces `BUILD.bazel`: `cargo_package(staticlib = True, cdylib = True)` only (Task 8 adds the platform targets).
- Produces `src/lib.rs`: `//!` doc naming spec §2, `mod log; mod web;` and nothing public yet.
- Consumes: nothing.

- [ ] **Step 1: Read the threads** on `bazel/cargo.bzl`, `scripts/check-crate-layers.py`, `scripts/check-no-unsafe.sh`.
- [ ] **Step 2: Extend `scripts/test-check-no-unsafe.sh`** with a `crates/clax-embed/` (its `Cargo.toml` as above, `src/lib.rs` with no `unsafe`) in `_copy`, and the cases: the tree passes; `unsafe_code = "allow"` in clax-embed's manifest fails with `crates/clax-embed/Cargo.toml: must deny unsafe_code`. Run `bazelisk test //scripts:no_unsafe_test` — FAIL.
- [ ] **Step 3: Update the check** (`%OWN_LINTS`, the deny rule) and the layers map; create the crate; `just bazel-sync`. Run `bazelisk test //scripts:no_unsafe //scripts:no_unsafe_test //scripts:crate_layers //crates/clax-embed:all` and `bazelisk build //crates/clax-embed:clax_embed_shared //crates/clax-embed:clax_embed_static` — PASS.
- [ ] **Step 4: Commit** the listed files: "Add the clax-embed crate, built as a shared and a static library, in clax-ios's layer".

### Task 4: Request and response frames

**Files:**
- Create: `crates/clax-embed/src/frame.rs`
- Modify: `crates/clax-embed/src/lib.rs` (`pub mod frame;`)

**Interfaces:**
- Produces:
  ```rust
  //! The bytes `clax_embed_call` takes and posts (spec §2.3): `CLX1`, a
  //! u32 little-endian header length, the header's JSON, the body.

  pub const MAGIC: &[u8; 4] = b"CLX1";

  #[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
  #[serde(rename_all = "kebab-case")]
  pub enum As { Owner, Content, ShellPage }

  #[derive(Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
  pub struct RequestHead {
      pub method: String,
      pub path: String,
      #[serde(default)]
      pub headers: Vec<(String, String)>,
      #[serde(rename = "as")]
      pub as_: As,
      #[serde(default)]
      pub host: Option<String>,
  }

  #[derive(Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
  pub struct ResponseHead { pub status: u16, pub headers: Vec<(String, String)> }

  #[derive(Debug, PartialEq, Eq)]
  pub enum FrameError { Short, Magic, Header(String) }

  /// Splits a frame into its head and body.
  pub fn decode_request(bytes: &[u8]) -> Result<(RequestHead, &[u8]), FrameError> {
      let rest = bytes.strip_prefix(MAGIC.as_slice()).ok_or(FrameError::Magic)?;
      if rest.len() < 4 { return Err(FrameError::Short); }
      let n = u32::from_le_bytes(rest[..4].try_into().unwrap()) as usize;
      let rest = &rest[4..];
      if rest.len() < n { return Err(FrameError::Short); }
      let head = serde_json::from_slice(&rest[..n]).map_err(|e| FrameError::Header(e.to_string()))?;
      Ok((head, &rest[n..]))
  }

  pub fn encode<H: serde::Serialize>(head: &H, body: &[u8]) -> Vec<u8> {
      let h = serde_json::to_vec(head).expect("a frame head serializes");
      let mut out = Vec::with_capacity(8 + h.len() + body.len());
      out.extend_from_slice(MAGIC);
      out.extend_from_slice(&(h.len() as u32).to_le_bytes());
      out.extend_from_slice(&h);
      out.extend_from_slice(body);
      out
  }

  /// A response frame preceded by the call's tag, as posted to Dart.
  pub fn tagged(tag: i64, frame: Vec<u8>) -> Vec<u8> {
      let mut out = Vec::with_capacity(8 + frame.len());
      out.extend_from_slice(&tag.to_le_bytes());
      out.extend(frame);
      out
  }
  ```
- Consumes: Task 3.

- [ ] **Step 1: Write the failing tests** in `frame.rs`'s `#[cfg(test)] mod tests`:
  ```rust
  #[test]
  fn a_request_round_trips_with_a_binary_body() {
      let head = RequestHead { method: "POST".into(), path: "/api/x?y=1".into(),
          headers: vec![("content-type".into(), "image/png".into())], as_: As::Owner, host: None };
      let body = [0u8, 255, 10, 13];
      let bytes = encode(&head, &body);
      assert_eq!(decode_request(&bytes).unwrap(), (head, &body[..]));
  }
  #[test]
  fn the_as_field_is_kebab_case() {
      let j = br#"{"method":"GET","path":"/v/1/","as":"shell-page"}"#;
      let bytes = [MAGIC.as_slice(), &(j.len() as u32).to_le_bytes(), j].concat();
      assert_eq!(decode_request(&bytes).unwrap().0.as_, As::ShellPage);
  }
  #[test]
  fn bad_frames_are_named() {
      assert_eq!(decode_request(b"CLX2xxxx").unwrap_err(), FrameError::Magic);
      assert_eq!(decode_request(b"CLX1\x09\0\0\0{}").unwrap_err(), FrameError::Short);
      assert!(matches!(decode_request(b"CLX1\x02\0\0\0{]").unwrap_err(), FrameError::Header(_)));
  }
  #[test]
  fn a_tag_precedes_the_frame_little_endian() {
      assert_eq!(&tagged(258, vec![9])[..], &[2, 1, 0, 0, 0, 0, 0, 0, 9]);
  }
  ```
- [ ] **Step 2: Run** `bazelisk test //crates/clax-embed:clax_embed_lib_test` — FAIL (no module).
- [ ] **Step 3: Write `frame.rs`** as above.
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** `crates/clax-embed/src/frame.rs crates/clax-embed/src/lib.rs`: "Encode and decode the embedded core's request and response frames".

### Task 5: `Embed`: open a home, answer calls by who they speak for

**Files:**
- Create: `crates/clax-embed/src/core.rs`, `crates/clax-embed/tests/embed.rs`
- Modify: `crates/clax-embed/src/lib.rs`, `crates/clax-embed/BUILD.bazel` (test data: none)

**Interfaces:**
- Produces (safe Rust; the FFI in Task 7 wraps it):
  ```rust
  /// A message to the port given at open (spec §2.2), as JSON.
  #[derive(Debug, Clone, PartialEq, serde::Serialize)]
  #[serde(tag = "type", rename_all = "kebab-case")]
  pub enum OpenEvent {
      Opening { phase: String },
      Ready,
      Failed { code: String, message: String, backup: Option<String> },
      Closed,
  }

  pub struct OpenOptions {
      pub home: std::path::PathBuf,
      pub origins: clax_http::state::EmbedOrigins,
      /// Where the rolling log goes (`<cache>/core.log`).
      pub log: Option<std::path::PathBuf>,
  }

  /// One open core. Cloning shares it.
  #[derive(Clone)]
  pub struct Embed { /* Arc<Inner>: runtime handle, OnceCell<Core>, lock file, closed flag */ }

  impl Embed {
      /// Starts opening on a runtime the Embed owns; `on_event` hears every
      /// OpenEvent, from the core's threads. Err(HomeInUse) when this
      /// process holds the home already.
      pub fn open(opts: OpenOptions, on_event: Box<dyn Fn(OpenEvent) + Send + Sync>) -> Result<Embed, OpenError>;
      /// Answers one request frame; waits for Ready first. Never panics on
      /// bad input: a frame that does not parse is FrameError.
      pub async fn call(&self, frame: &[u8]) -> Result<Vec<u8>, frame::FrameError>;
      /// Spawns `fut` on the core's runtime.
      pub fn spawn<F: std::future::Future<Output = ()> + Send + 'static>(&self, fut: F);
      /// Stops the core; `on_event` hears Closed when done.
      pub fn close(&self);
  }
  #[derive(Debug, PartialEq, Eq)]
  pub enum OpenError { HomeInUse, NotUtf8 }

  /// The longest body read back (spec §2.3).
  pub const RESPONSE_LIMIT: usize = 96 << 20;
  ```
  Routing rules (spec §2.3), in `fn admit(head: &RequestHead) -> Result<(), &'static str>` (returns the refusal code `not_in_app`):
  - `Owner`: anything but `GET /api/stream` and `/api/admin/shutdown`;
  - `ShellPage`: `GET`/`HEAD` and a path starting `/a/`, `/assets/`, `/_clax/`, `/c/`, `/_blob/`, or equal to `/favicon.ico`;
  - `Content`: `GET`/`HEAD`, `host` present and 12 lowercase base32 characters (`clax_http::host::artifact_host` decides), any path (the router's `rewrite_artifact_host` refuses the rest).
  Headers the core sets: `Owner` → `host: localhost`, `authorization: Bearer <token>` unless present, extension `ConnectInfo(Conn { peer: 127.0.0.1:1, local: 127.0.0.1:1 })`; `ShellPage` → `host: localhost`, no `authorization`, no `cookie` (dropped if sent); `Content` → `host: <id>.localhost`, the same drops.
- Consumes: Task 1 (`Core`), Task 2 (`EmbedOrigins`), Task 4 (frames).

- [ ] **Step 1: Write the failing tests** `crates/clax-embed/tests/embed.rs` (a helper `open(dir) -> (Embed, Receiver<OpenEvent>)` waiting for `Ready` on a `std::sync::mpsc`, and `call(embed, head, body) -> (ResponseHead, Vec<u8>)` over `frame::encode`/a response decoder written in the test):
  ```rust
  #[tokio::test] async fn an_owner_call_lists_artifacts() { /* GET /api/artifacts → 200, {"artifacts": []} */ }
  #[tokio::test] async fn an_owner_call_creates_and_a_content_frame_reads_its_page() {
      // POST /api/artifacts (as owner) with index.html "<title>t</title>hi"
      // then GET /v/1/ as Content with host = the new ID: 200, body contains "hi",
      // content-security-policy contains "frame-ancestors clax://app".
  }
  #[tokio::test] async fn a_content_frame_reaches_no_api() { /* GET /api/artifacts as Content → 404 */ }
  #[tokio::test] async fn a_shell_page_frame_never_reaches_the_api() { /* GET /api/artifacts as ShellPage → 404 not_in_app; POST /a/x as ShellPage → 404 */ }
  #[tokio::test] async fn the_event_stream_is_not_a_call() { /* GET /api/stream as Owner → 404 not_in_app */ }
  #[tokio::test] async fn a_page_frame_carrying_a_cookie_gets_no_credential() { /* GET /api/token as ShellPage with cookie → 404; as Owner → 200 */ }
  #[tokio::test(start_paused = true)] async fn a_long_poll_answers_when_feedback_arrives() {
      // register an app session; spawn GET /api/sessions/{sid}/feedback?wait=20s;
      // post an owner comment sent to that session; the poll answers with it.
  }
  #[tokio::test] async fn calls_made_before_ready_wait_for_it() { /* call immediately after open; it answers 200 */ }
  #[test] fn a_second_open_of_one_home_is_refused() { /* Err(OpenError::HomeInUse) */ }
  #[tokio::test] async fn close_posts_closed_and_frees_the_home() { /* close; Closed heard; open again succeeds */ }
  #[tokio::test] async fn an_old_daemon_json_is_retired() {
      // write <home>/daemon.json; open; it is renamed daemon.json.retired.
  }
  #[tokio::test] async fn a_body_over_the_limit_answers_502() {
      // publish a 17 MiB asset is refused earlier; instead test the limit by a
      // unit test of `read_limited(body, limit)` with limit 8 and 9 bytes.
  }
  ```
  Write each body fully; publish through `POST /api/artifacts` with the JSON shape `crates/clax-server/tests/common` uses (`grep -n "fn publish" crates/clax-server/tests/common/*.rs`).
- [ ] **Step 2: Run** `bazelisk test //crates/clax-embed:all` — FAIL.
- [ ] **Step 3: Implement `core.rs`.** `open`: refuse a home this process holds (a `static OPEN: Mutex<HashSet<PathBuf>>` of canonical paths); build a multi-thread runtime (2 workers, `thread_name("clax-rt")`); on it: `std::fs::File::create(home/core.lock)?.try_lock()` (Err → `Failed{code:"home_in_use"}`), rename `daemon.json` if present, `Core::start` with `CoreConfig { require_credential: true, embed: Some(origins), sample: Sampler::from_home_with_slot(...) as clax-ios's config() does, web: web::web_ui(), port: 0, bind: 127.0.0.1, … }`, mapping `CoreError::MigrationFailed` to `Failed{code:"migration_failed", backup: <newest file in backup::dir(home)>}`; store the `Core` in a `tokio::sync::OnceCell`; emit `Ready`. `call`: decode, `admit`, wait on the `OnceCell`, build the `http::Request` (method, path, headers per the rules, body), `core.router().oneshot(req)`, read the body with `http_body_util::Limited::new(body, RESPONSE_LIMIT)` (over → 502 `{"error":{"code":"response_too_large"}}`), encode `ResponseHead`. Refusals answer `404 {"error":{"code":"not_in_app","message":"<path> is not served in the app"}}`.
- [ ] **Step 4: Run** — PASS. Then `bazelisk build --config=fmt --config=clippy //crates/clax-embed/...`.
- [ ] **Step 5: Commit** `crates/clax-embed/src/core.rs crates/clax-embed/src/lib.rs crates/clax-embed/tests/embed.rs crates/clax-embed/BUILD.bazel`: "Open a home in the embedded core and answer request frames through the router by who they speak for".

### Task 6: Subscriptions, trimming and the key over `call`

**Files:**
- Create: `crates/clax-embed/src/events.rs`
- Modify: `crates/clax-embed/src/lib.rs`, `crates/clax-server/src/router.rs:90` (`POST /api/core/trim`), `crates/clax-server/src/routes/health.rs` (the trim handler; owner token required, 404 without `AppState.embed`)
- Test: `crates/clax-embed/tests/embed.rs` (new cases)

**Interfaces:**
- Produces:
  ```rust
  impl Embed {
      /// Sends GET /api/stream as the owner and calls `on_block` with each
      /// body chunk until it returns false or the core closes.
      pub fn subscribe(&self, on_block: Box<dyn FnMut(bytes::Bytes) -> bool + Send>);
  }
  ```
  `POST /api/core/trim` answers `{"trimmed": <TrimReport as JSON>}` (`Store::trim_memory`, `crates/clax-store/src/lib.rs:392`).
- Consumes: Task 5.

- [ ] **Step 1: Write failing tests:**
  ```rust
  #[tokio::test] async fn a_subscription_hears_ready_then_a_version() {
      // subscribe collecting blocks into an mpsc; first block contains "event: ready";
      // POST /api/stream/<id> {"subscribe":["gallery"]} as owner; publish an artifact;
      // a later block contains "event: " and the artifact's ID.
  }
  #[tokio::test] async fn a_subscriber_that_returns_false_is_dropped() {
      // on_block returns false at once; GET /api/streams (owner) reports open: 0 soon after the next keep-alive (paused clock advanced past sse_keep_alive).
  }
  #[tokio::test] async fn trimming_answers_on_an_embedded_core() { /* POST /api/core/trim as owner → 200 with "trimmed" */ }
  #[tokio::test] async fn a_key_put_reaches_the_sampler_without_a_restart() {
      // PUT /api/sample/key {"key":"sk-test"} → 204; GET /api/sample → provider "anthropic".
  }
  ```
  And in `crates/clax-server/tests/` a case that `POST /api/core/trim` on a plain daemon answers 404.
- [ ] **Step 2: Run** — FAIL.
- [ ] **Step 3: Implement** `subscribe` by building the owner request for `GET /api/stream` (bypassing `admit`, which refuses it for `call`), `oneshot`, and a task polling `body.into_data_stream()`; implement the route.
- [ ] **Step 4: Run** `bazelisk test //crates/clax-embed:all //crates/clax-server:all` — PASS.
- [ ] **Step 5: Commit** the listed files: "Post the core's event stream to a subscriber, and trim an embedded core's memory by a route".

### Task 7: The four C functions and the `unsafe` exception

**Files:**
- Create: `crates/clax-embed/src/dart.rs`, `crates/clax-embed/src/ffi.rs`, `crates/clax-embed/include/clax_embed.h`, `crates/clax-embed/tests/ffi.rs`
- Modify: `crates/clax-embed/src/lib.rs`, `scripts/check-no-unsafe.sh` (the clax-embed rules of spec §14), `scripts/test-check-no-unsafe.sh`, `AGENTS.md:47` (a second exception line, spec §14's text; Task 31 removes clax-ios's)

**Interfaces:**
- Produces `src/dart.rs` (no `unsafe`):
  ```rust
  //! Dart's `Dart_CObject` (dart_native_api.h of the Dart SDK Flutter
  //! 3.47.7 ships), only as far as the core posts typed data.
  pub const K_TYPED_DATA: i32 = 7;
  pub const K_UINT8: i32 = 2;

  #[repr(C)] #[derive(Clone, Copy)]
  pub struct TypedData { pub ty: i32, pub length: isize, pub values: *const u8 }
  #[repr(C)] #[derive(Clone, Copy)]
  pub struct ExternalTypedData { pub ty: i32, pub length: isize, pub data: *mut u8, pub peer: *mut core::ffi::c_void, pub callback: *const core::ffi::c_void }
  /// Sized as the C union: its largest member is `as_external_typed_data`.
  #[repr(C)] pub union Value { pub as_typed_data: TypedData, pub as_external_typed_data: ExternalTypedData }
  #[repr(C)] pub struct DartCObject { pub ty: i32, pub value: Value }

  /// Dart's `NativeApi.postCObject`.
  pub type ClaxPost = extern "C" fn(port: i64, message: *mut DartCObject) -> i8;

  /// Posts `bytes` to `port` as a Uint8List (Dart copies it); false when
  /// the port is closed.
  pub fn post_bytes(post: ClaxPost, port: i64, bytes: &[u8]) -> bool {
      let mut obj = DartCObject { ty: K_TYPED_DATA, value: Value { as_typed_data: TypedData {
          ty: K_UINT8, length: bytes.len() as isize, values: bytes.as_ptr() } } };
      post(port, &mut obj) != 0
  }
  pub fn post_json<T: serde::Serialize>(post: ClaxPost, port: i64, v: &T) -> bool {
      post_bytes(post, port, &serde_json::to_vec(v).expect("serializes"))
  }
  ```
- Produces `src/ffi.rs` (the only `unsafe`; exactly this shape):
  ```rust
  //! The four C functions of include/clax_embed.h (spec §2.2). Handles are
  //! numbers in a registry, never pointers.

  use crate::dart::{ClaxPost, post_bytes, post_json};
  use crate::{Embed, OpenOptions};
  use std::collections::HashMap;
  use std::sync::{Mutex, atomic::{AtomicU64, Ordering}};

  static CORES: Mutex<Option<HashMap<u64, (Embed, ClaxPost)>>> = Mutex::new(None);
  static NEXT: AtomicU64 = AtomicU64::new(1);

  fn get(core: u64) -> Option<(Embed, ClaxPost)> { /* lock, clone */ }

  #[allow(unsafe_code)]
  #[unsafe(no_mangle)]
  pub unsafe extern "C" fn clax_embed_open(home: *const u8, home_len: usize, post: ClaxPost, port: i64) -> u64 {
      // SAFETY: Dart passes a pointer to home_len readable bytes that outlive this call.
      let home = unsafe { std::slice::from_raw_parts(home, home_len) };
      crate::ffi_open(home, post, port)
  }

  #[allow(unsafe_code)]
  #[unsafe(no_mangle)]
  pub unsafe extern "C" fn clax_embed_call(core: u64, req: *const u8, req_len: usize, port: i64, tag: i64) -> i32 {
      // SAFETY: Dart passes a pointer to req_len readable bytes that outlive this call.
      let req = unsafe { std::slice::from_raw_parts(req, req_len) };
      crate::ffi_call(core, req, port, tag)
  }

  #[allow(unsafe_code)]
  #[unsafe(no_mangle)]
  pub extern "C" fn clax_embed_subscribe(core: u64, port: i64) -> i32 { crate::ffi_subscribe(core, port) }

  #[allow(unsafe_code)]
  #[unsafe(no_mangle)]
  pub extern "C" fn clax_embed_close(core: u64) -> i32 { crate::ffi_close(core) }
  ```
  with `ffi_open/ffi_call/ffi_subscribe/ffi_close` in `lib.rs` (safe): `ffi_open` parses the path as UTF-8 (`0` if it is not) and reads the origins from `<home>/embed.json` `{"shell": …, "content": …}`, which the app writes before `open` (spec §2.5; Task 12), posting `failed` `{code: "no_origins"}` when it is missing or malformed; then `Embed::open`; `ffi_call` copies `req` into a `Vec`, spawns `embed.call` on the core's runtime and posts `frame::tagged(tag, response)` (a decode error posts a 400 frame `bad_frame`), returning 0, or 1 (unknown core) / 2 (`Short`/`Magic` detected synchronously).
- Produces `include/clax_embed.h`: spec §2.2's block verbatim, with an include guard.
- Produces `scripts/check-no-unsafe.sh` rules for `crates/clax-embed/src/ffi.rs` exactly as spec §14 lists (export names `clax_embed_open clax_embed_call clax_embed_subscribe clax_embed_close`; `unsafe extern` only for open and call; one `let <a> = unsafe { std::slice::from_raw_parts(<p>, <n>) };` per those two, directly after a `// SAFETY:` line; parameter types `u64 i64 i32 usize *const u8 ClaxPost`); `unsafe` anywhere else in `crates/clax-embed/` fails with `only the four exported functions may use unsafe`.
- Consumes: Tasks 5, 6.

- [ ] **Step 1: Read the threads** on `AGENTS.md`, `scripts/check-no-unsafe.sh`, `scripts/test-check-no-unsafe.sh`.
- [ ] **Step 2: Write the check's failing cases** in `scripts/test-check-no-unsafe.sh`: the tree with the `ffi.rs` above passes; a fifth export fails; `pub unsafe extern "C" fn clax_embed_subscribe(` fails; a `*mut u8` parameter fails; a second `from_raw_parts` in `clax_embed_call` fails; a `from_raw_parts` without the `// SAFETY:` line fails; `Box::from_raw` anywhere fails; `unsafe` in `crates/clax-embed/src/lib.rs` fails; the clax-ios cases still pass. Run `bazelisk test //scripts:no_unsafe_test` — FAIL.
- [ ] **Step 3: Rewrite the Perl** in `check-no-unsafe.sh` for the new file's rules beside clax-ios's. Run — PASS.
- [ ] **Step 4: Write `tests/ffi.rs`** (safe: it passes slices it owns and a test `extern "C" fn` as `post` that pushes copies into a `static Mutex<Vec<(i64, Vec<u8>)>>`): open a temp home (with `embed.json`), wait for `{"type":"ready"}` on port 1, `clax_embed_call` a `GET /api/artifacts` with tag 7 to port 2 and find the tagged 200 response, subscribe on port 3 and find a `ready` block, close and find `{"type":"closed"}`; `clax_embed_open` of a non-UTF-8 path answers 0; `clax_embed_call(999, …)` answers 1. Calling an `unsafe extern "C" fn` needs `unsafe` in the test, which the check forbids — so the test calls `crate::ffi_open`/`ffi_call` (the safe halves) and a separate unit test in `ffi.rs` is not written; the two `unsafe` lines are covered by Task 12's Dart test through the real library.
- [ ] **Step 5: Write `dart.rs`, `ffi.rs`, the header and the `lib.rs` halves.** Run `bazelisk test //crates/clax-embed:all //scripts:no_unsafe //scripts:no_unsafe_test` — PASS.
- [ ] **Step 6: Edit `AGENTS.md`**: add spec §14's line after the clax-ios line (both stand until Task 31).
- [ ] **Step 7: Commit** the listed files: "Expose the embedded core through four C functions, and allow unsafe only on them and their two byte slices".

### Task 8: Bazel targets for Android, macOS and iOS

**Files:**
- Create: `bazel/android/embed.bzl` (a copy of `bazel/android/clax.bzl` for a `rust_shared_library`, output `<abi>/libclax_embed.so`, no symlink to an executable)
- Modify: `crates/clax-embed/BUILD.bazel` (targets below), `docs/bazel.md` (the table in "The Clax app": three new rows)

**Interfaces:**
- Produces targets:
  - `//crates/clax-embed:clax_embed_android_arm64`, `:clax_embed_android_x86_64` → `<abi>/libclax_embed.so`, `-c opt`, `-Cstrip=symbols`, the NDK platform as `clax.bzl` uses;
  - `//crates/clax-embed:clax_embed_macos` → `libclax_embed.dylib`, `-c opt` in a transition (as `ios_opt_library`), `target_compatible_with = ["@platforms//os:macos"]`;
  - `//crates/clax-embed:clax_embed_host` → the `_shared` library in the test configuration, for Dart tests (`CLAX_EMBED_LIB`);
  - `//crates/clax-embed:clax_embed_ios_c` → `cc_library` over an `ios_opt_library` of `:clax_embed_static` with clax-ios's linkopts and `linkopts += ["-Wl,-exported_symbol,_clax_embed_open", … four]`, visible to `//apps/clax_app:__pkg__`; `:clax_embed_ios_builds` and `:ios_build_test` as clax-ios's;
  - `:android_symbols_test`, an `sh_test` that runs the NDK's `llvm-nm -D --defined-only` on both `.so` and finds exactly the four `clax_embed_*` functions among `T` symbols.
- Consumes: Task 7.

- [ ] **Step 1: Write `:android_symbols_test`** (script `crates/clax-embed/tool/symbols-check.sh <nm> <lib>…`, failing on a missing or extra `clax_embed_` symbol). Run `bazelisk test //crates/clax-embed:android_symbols_test` — FAIL (no targets).
- [ ] **Step 2: Write `embed.bzl` and the targets.** Run `bazelisk build //crates/clax-embed:all` and `bazelisk test //crates/clax-embed:android_symbols_test //crates/clax-embed:ios_build_test` — PASS. Record each artifact's size: `ls -l bazel-bin/crates/clax-embed/**/libclax_embed.*` beside `ls -l bazel-bin/crates/clax-cli/clax_android_arm64/arm64-v8a/libclax.so` (after `bazelisk build //crates/clax-cli:clax_android_arm64`) into the commit message body.
- [ ] **Step 3: Document** the targets in `docs/bazel.md`'s table.
- [ ] **Step 4: Commit** the listed files: "Build the embedded core for Android, macOS and iOS".

## Phase 2: The shell's app-mode transport and the scheme (spec §4, §5)

### Task 9: `hostFetch`: the shell's calls over `ClaxHost`, and no connection notices in app mode

**Files:**
- Create: `web/shell/src/transport.ts`, `web/shell/src/transport.test.ts`
- Modify: `web/shell/src/conn-notice.ts` (`connNotices(enabled)`), `web/shell/src/lifecycle.ts:96-140` (`retrying` reports no trouble when notices are off), `web/shell/src/app/host.ts` (the `callResult`, `streamBlock`, `streamEnd` methods on `window.claxHost`; the header comment's protocol table), the shell's entry modules that run first (`web/shell/src/artifact-main.ts` and every other `*-main.ts`: call `installTransport()` before anything else)

**Interfaces:**
- Produces:
  ```ts
  // web/shell/src/transport.ts
  /** The app's embed settings, from the shell document's
   * <meta name="clax-embed" content='{"transport":"host","shell":…,"content":…}'>
   * (written by the core when it runs in the app; absent in a browser). */
  export type Embed = { transport: "host"; shell: string; content: string };
  export function readEmbed(doc?: Document): Embed | null;

  /** What the host side of the channel looks like to the transport. */
  export type HostLink = {
    post(msg: Record<string, unknown>): void;     // ClaxHost.postMessage with the nonce
    ready: Promise<void>;                          // resolves at init
  };

  /** fetch for app mode: /api/ on the shell's origin goes over the link;
   * anything else to `fallback`. GET /api/stream answers a streaming body. */
  export function hostFetch(link: HostLink, fallback: typeof fetch, origin: string): typeof fetch;

  /** The host's answers, which claxHost forwards here. */
  export function callResult(id: number, r: { status: number; headers: [string, string][]; body?: string | null; body64?: string | null }): void;
  export function streamBlock(id: number, text: string): void;
  export function streamEnd(id: number): void;

  /** At boot: in app mode replaces window.fetch with hostFetch and turns
   * connection notices off. Returns whether it did. */
  export function installTransport(win?: Window): boolean;

  export const CHUNK = 1 << 20;        // 1 MiB a callChunk
  export const CHUNKED_OVER = 8 << 20; // bodies over 8 MiB go in chunks
  ```
  Messages posted (spec §5.1, §5.2): `call {id, method, path, headers, body | body64}`, `callChunk {id, n, last, body64}`, `callAbort {id}`, `subscribe {id}`, `unsubscribe {id}`.
- Consumes: Task 2 (the meta; this task's tests build the meta themselves).

- [ ] **Step 1: Read the threads** on `conn-notice.ts`, `lifecycle.ts`, `app/host.ts`.
- [ ] **Step 2: Write `transport.test.ts`** (vitest, jsdom):
  ```ts
  import { describe, expect, it, vi } from "vitest";
  import { hostFetch, callResult, streamBlock, streamEnd, CHUNKED_OVER } from "./transport";

  function link() {
    const sent: Record<string, unknown>[] = [];
    return { sent, link: { post: (m: Record<string, unknown>) => sent.push(m), ready: Promise.resolve() } };
  }
  const ORIGIN = "https://app.clax.invalid";

  describe("hostFetch", () => {
    it("relays an /api call and resolves on its result", async () => {
      const { sent, link: l } = link();
      const f = hostFetch(l, vi.fn(), ORIGIN);
      const p = f("/api/artifacts", { method: "POST", headers: { "content-type": "application/json" }, body: "{}" });
      await Promise.resolve(); await Promise.resolve();
      expect(sent[0]).toMatchObject({ type: "call", method: "POST", path: "/api/artifacts", body: "{}" });
      callResult(sent[0].id as number, { status: 201, headers: [["content-type", "application/json"]], body: '{"ok":true}' });
      const r = await p;
      expect(r.status).toBe(201);
      expect(await r.json()).toEqual({ ok: true });
    });
    it("passes anything but /api on its origin to the fallback", async () => {
      const fb = vi.fn(async () => new Response("x"));
      const f = hostFetch(link().link, fb, ORIGIN);
      await f("/_clax/bridge.js"); await f("https://example.com/api/x");
      expect(fb).toHaveBeenCalledTimes(2);
    });
    it("sends a binary body as base64", async () => {
      const { sent, link: l } = link();
      void hostFetch(l, vi.fn(), ORIGIN)("/api/assets", { method: "POST", body: new Uint8Array([0, 255]) });
      await Promise.resolve(); await Promise.resolve();
      expect(sent[0]).toMatchObject({ body: null, body64: "AP8=" });
    });
    it("chunks a body over the limit", async () => {
      const { sent, link: l } = link();
      void hostFetch(l, vi.fn(), ORIGIN)("/api/assets", { method: "POST", body: new Uint8Array(CHUNKED_OVER + 1) });
      await vi.waitFor(() => expect(sent.filter(m => m.type === "callChunk").length).toBe(9));
      expect(sent.at(-1)).toMatchObject({ type: "callChunk", last: true });
    });
    it("aborts", async () => {
      const { sent, link: l } = link();
      const ac = new AbortController();
      const p = hostFetch(l, vi.fn(), ORIGIN)("/api/x", { signal: ac.signal });
      await Promise.resolve(); ac.abort();
      await expect(p).rejects.toThrow(/abort/i);
      expect(sent.at(-1)).toMatchObject({ type: "callAbort" });
    });
    it("streams /api/stream from pushed blocks and ends", async () => {
      const { sent, link: l } = link();
      const r = await hostFetch(l, vi.fn(), ORIGIN)("/api/stream");
      const id = sent[0].id as number;
      expect(sent[0]).toMatchObject({ type: "subscribe" });
      const reader = r.body!.getReader();
      streamBlock(id, "event: ready\ndata: {}\n\n");
      expect(new TextDecoder().decode((await reader.read()).value)).toContain("event: ready");
      streamEnd(id);
      expect((await reader.read()).done).toBe(true);
    });
  });
  ```
  and in `conn-notice.test.ts` (create if absent): with `connNotices(false)`, `connTrouble("stream", true, STREAM_DOWN)` leaves `connNoticeText()` null; `lifecycle` test: `retrying` with notices off never calls `trouble(true)`.
- [ ] **Step 3: Run** `bazelisk test //web:unit --test_arg=shell/src/transport.test.ts` — FAIL.
- [ ] **Step 4: Implement.** `hostFetch` builds a `Request` to normalise input, matches `new URL(req.url, origin)`'s origin and `/api/` prefix, waits `link.ready`, reads the body as `ArrayBuffer` (UTF-8 text when `TextDecoder("utf-8", {fatal: true})` accepts it and the content type is text or JSON, else base64), posts, and keeps a `Map<number, {resolve, reject}>`; `callResult` builds `new Response(body, {status, headers})`. `GET /api/stream` returns `new Response(new ReadableStream({start(c) {…}, cancel() { post unsubscribe }}), {headers: {"content-type": "text/event-stream"}})`. `installTransport` reads the meta; with it, sets `window.fetch`, calls `connNotices(false)`, and wires the link to `window.ClaxHost` and `host.ts`'s nonce promise (export `nonceReady()` from `host.ts`). Add the three methods to `claxHost` in `host.ts`, forwarding to this module; document them in its header table.
- [ ] **Step 5: Run** the tests, then `bazelisk test //web:unit //web:lint //web:typecheck` — PASS.
- [ ] **Step 6: Commit** the listed files: "Carry the shell's API calls and event stream over the app's channel in app mode, with no connection notices".

### Task 10: The stream hub, origins and `/api` images in app mode

**Files:**
- Modify: `web/shell/src/stream.ts` (in app mode: an in-tab `Hub` with `env.fetch = window.fetch` and `host: true`), `web/shell/src/stream-hub.ts:130-190,326-440` (`HubEnv.host?: boolean`: no connect timeout, no idle timeout, no backoff, no `status up=false`), `web/shell/src/origin.ts:3-7` (`artifactOrigin` from the embed's `content` template), `web/shell/src/view/artifact-controller.ts:926-953` (app mode is always subdomain mode with the embed's origin; no probe), every `src=`/`href=` built from `/api/` (`grep -rn '"/api/[^"]*\(clip\|asset\|file\)' web/shell/src`), which in app mode fetch and use `URL.createObjectURL`
- Test: `web/shell/src/stream-hub.test.ts`, `web/shell/src/origin.test.ts` (new cases)

**Interfaces:**
- Produces: `HubEnv.host?: boolean`; `artifactOrigin(id, loc?, embed = readEmbed())` returns `embed.content.replace("{id}", id)` when `embed` is set; `apiImageSrc(path): Promise<string>` in `web/shell/src/transport.ts` (an object URL in app mode, `path` itself in a browser).
- Consumes: Task 9.

- [ ] **Step 1: Write failing tests.** `stream-hub.test.ts`: a `Hub` with `host: true` and a fake `fetch` whose body never sends a byte for 10 minutes of fake time sends no `status` message and no reconnect; a `Hub` without `host` does (the existing behaviour). `origin.test.ts`: with an embed `{content: "clax://{id}.c"}`, `artifactOrigin("abcdefghijkl")` is `clax://abcdefghijkl.c` whatever the location.
- [ ] **Step 2: Run** `bazelisk test //web:unit --test_arg=shell/src/stream-hub.test.ts --test_arg=shell/src/origin.test.ts` — FAIL.
- [ ] **Step 3: Implement**, then list the `/api` URL uses the grep finds in the commit message and convert each.
- [ ] **Step 4: Run** `bazelisk test //web:unit //web:lint //web:typecheck` — PASS.
- [ ] **Step 5: Commit** the listed files: "Run the shell's stream hub on the host's pushed events and give artifacts the app's content origins in app mode".

### Task 11: The app transport end to end in `web/e2e`

**Files:**
- Create: `web/e2e/app-scheme.ts`, `web/e2e/app-transport.spec.ts`
- Modify: `web/e2e/app-host.ts` (a `hostChannel(page, {relay})` option that answers `call`, `callChunk`, `callAbort`, `subscribe`, `unsubscribe` messages and drops messages not from the main frame), `web/BUILD.bazel` only if the new spec must be listed (it is globbed today: check `grep -n "spec.ts" web/BUILD.bazel`)

**Interfaces:**
- Produces `web/e2e/app-scheme.ts`:
  ```ts
  /** Routes https://app.clax.invalid/** and https://*.c.clax.invalid/** of
   * `page` to the test daemon `d` with spec §2.3's rules: only GET/HEAD;
   * the shell host serves /a/, /assets/, /_clax/, /c/, /_blob/,
   * /favicon.ico and answers 404 not_in_app otherwise; a content host maps
   * to Host <id>.localhost; the shell document only to a main-frame
   * navigation (route.request().isNavigationRequest() && frame is main);
   * every Access-Control-* header stripped. The daemon is started with an
   * embed (--embed-shell/--embed-content, below) so its meta and CSP name
   * these origins. */
  export async function routeAppScheme(page: Page, d: Daemon): Promise<void>;
  export const SHELL = "https://app.clax.invalid";
  export const content = (id: string) => `https://${id}.c.clax.invalid`;
  ```
  Because Playwright's routing answers in Node, the daemon receives ordinary HTTP; the relay sets `Host` and strips headers exactly as the host will.
- Produces a test-only daemon flag: `clax serve --embed-origins <shell> <content-template>` (hidden, `#[arg(hide = true)]`) that sets `CoreConfig.embed`; add it in `crates/clax-commands`' serve arguments with a doc comment saying it is for tests of the app's transport.
- Consumes: Tasks 2, 9, 10.

- [ ] **Step 1: Write `app-transport.spec.ts`** with these tests, each driving `page.goto(\`${SHELL}/a/${id}?app=1\`)` after `routeAppScheme` and `hostChannel(page, {relay: d})`:
  1. "the artifact view loads and a comment posts through the channel": comment mode, pick, send; the daemon has the comment (`api(d).get(...)`); the fake host saw a `call` for `POST /api/artifacts/<id>/threads`.
  2. "a version published elsewhere is pushed and followed": publish v2 by `api(d)`; the frame shows v2's text.
  3. "no connection notice, also with the relay held for a minute": `page.clock.install()`; hold the relay's answers and pushes; `page.clock.fastForward(60_000)`; `.conn-notice` is not visible; release; the view works.
  4. "a page reads nothing of the shell": a published page whose script runs `fetch(SHELL + "/a/<id>?app=1")` and `fetch(SHELL + "/api/artifacts")` and posts what it read to `parent` by `postMessage`; the shell's listener (installed by the test with `page.evaluate`) records an opaque/failed result for both (`TypeError` or `type === "opaque"`), never a body.
  5. "a page cannot post on ClaxHost": the page calls `window.ClaxHost?.postMessage(...)`; the fake host records nothing from a non-main frame.
- [ ] **Step 2: Run** `bazelisk test //web:e2e_rest --test_arg=e2e/app-transport.spec.ts` — FAIL (no scheme module, no daemon flag).
- [ ] **Step 3: Implement** `app-scheme.ts`, the `hostChannel` relay (calls go to the daemon with its token as `as: owner` would; `subscribe` opens `GET /api/stream` with the token and forwards each chunk through `page.evaluate(() => claxHost.streamBlock(id, text))`), and the daemon flag.
- [ ] **Step 4: Run** — PASS. Run the whole app-mode e2e set: `bazelisk test //web:e2e_rest --test_arg=e2e/app-mode.spec.ts --test_arg=e2e/app-questions.spec.ts --test_arg=e2e/inspect.spec.ts` — PASS (the browser path is unchanged).
- [ ] **Step 5: Commit** the listed files: "Test the shell's app transport end to end over a routed scheme, origin isolation included".

### Task 12: Dart's core client: FFI bindings, `CoreClient`, `FakeCore`

**Files:**
- Create: `apps/clax_app/lib/core/ffi.dart`, `apps/clax_app/lib/core/frame.dart`, `apps/clax_app/lib/core/core_client.dart`, `apps/clax_app/lib/core/origins.dart`, `apps/clax_app/test/core/frame_test.dart`, `apps/clax_app/test/core/core_client_test.dart`, `apps/clax_app/test/core/core_real_test.dart`, `apps/clax_app/test/support/fake_core.dart`
- Modify: `apps/clax_app/BUILD.bazel` (`SUITES` gains `core`; `TEST_SUPPORT` gains `test/support/fake_core.dart`; the `real` suite's `data` gains `//crates/clax-embed:clax_embed_host` and `env` `CLAX_EMBED_LIB`)

**Interfaces:**
- Produces:
  ```dart
  // lib/core/frame.dart
  enum CallAs { owner, content, shellPage }
  class CoreRequest {
    const CoreRequest(this.method, this.path, {this.headers = const [], this.body, this.as = CallAs.owner, this.host});
    final String method; final String path; final List<(String, String)> headers;
    final Uint8List? body; final CallAs as; final String? host;
    Uint8List encode();
  }
  class CoreResponse {
    const CoreResponse(this.status, this.headers, this.body);
    final int status; final List<(String, String)> headers; final Uint8List body;
    static CoreResponse decode(Uint8List frame);
    String? header(String name);
  }

  // lib/core/origins.dart
  class AppOrigins {
    const AppOrigins(this.shell, this.contentTemplate);
    final String shell; final String contentTemplate;
    static AppOrigins forPlatform() => Platform.isAndroid
        ? const AppOrigins('https://app.clax.invalid', 'https://{id}.c.clax.invalid')
        : const AppOrigins('clax://app', 'clax://{id}.c');
    String content(String id) => contentTemplate.replaceAll('{id}', id);
    /// The artifact ID of a content host, or null.
    String? idOfHost(String host);
    bool isShellHost(String host);
    Map<String, String> toJson() => {'shell': shell, 'content': contentTemplate};
  }

  // lib/core/ffi.dart — the four functions, looked up by name.
  final class CoreLib {
    CoreLib(DynamicLibrary lib);
    static CoreLib load();   // Android: open('libclax_embed.so'); macOS: <bundle>/Contents/Frameworks/libclax_embed.dylib, or CLAX_EMBED_LIB when set; iOS: process()
    int open(String home, int port);               // writes nothing; posts with NativeApi.postCObject
    int call(int core, Uint8List req, int port, int tag);
    int subscribe(int core, int port);
    int close(int core);
  }

  // lib/core/core_client.dart
  sealed class CoreOpen { const CoreOpen(); }
  final class CoreOpening extends CoreOpen { const CoreOpening(this.phase); final String phase; }
  final class CoreReady extends CoreOpen { const CoreReady(); }
  final class CoreFailed extends CoreOpen { const CoreFailed(this.code, this.message, this.backup); final String code, message; final String? backup; }
  final class CoreClosed extends CoreOpen { const CoreClosed(); }

  /// One isolate's door to the core.
  abstract interface class CoreCalls {
    Future<CoreResponse> call(CoreRequest r);
    /// The core's SSE blocks (text) until [cancel]; one subscription each.
    CoreSubscription subscribe();
  }
  abstract interface class CoreSubscription { Stream<String> get blocks; void cancel(); }

  class CoreClient implements CoreCalls {
    /// Opens the core on [home] (writing `<home>/embed.json` with [origins]
    /// first); [states] hears CoreOpen messages.
    static CoreClient open(String home, AppOrigins origins, {CoreLib? lib});
    /// The same core from another isolate.
    static CoreClient attach(int handle, {CoreLib? lib});
    int get handle;
    Stream<CoreOpen> get states;
    void close();
  }
  ```
  `test/support/fake_core.dart`: `class FakeCore implements CoreCalls` whose `call` dispatches on `(method, path)` to handlers a test registers (`on('GET', '/api/artifacts', (req) => …)`), records every request, and whose `subscribe` returns blocks a test `push`es; plus `FakeCore.fromApi(FakeClaxApi api)`, which answers the paths `HttpClaxApi` calls from a `FakeClaxApi` (the same shapes `test/clax/http_api_test.dart` asserts).
- Consumes: Tasks 7, 8.

- [ ] **Step 1: Write `frame_test.dart`**: a request with a binary body encodes to the bytes `crates/clax-embed/src/frame.rs`'s test expects (`CLX1`, little-endian length, JSON with `"as":"shell-page"`), and a response frame decodes; `AppOrigins.forPlatform` on each platform; `idOfHost('abcdefghijkl.c.clax.invalid')` is `abcdefghijkl`, `idOfHost('app.clax.invalid')` null, `idOfHost('x.c.clax.invalid')` null (not 12 base32 characters).
- [ ] **Step 2: Write `core_client_test.dart`** over a fake `CoreLib` (a Dart class whose `call` answers by posting to the given port with `SendPort` lookups via `IsolateNameServer`-free plumbing: the fake keeps a `Map<int, SendPort>` the test fills) checking tags match concurrent calls answered out of order, and `close` completes pending calls with status 0 `core_closed`.
- [ ] **Step 3: Write `core_real_test.dart`** (real suite): `CoreClient.open` on a temp home with `CoreLib(DynamicLibrary.open(Platform.environment['CLAX_EMBED_LIB']!))`, wait for `CoreReady`, `GET /api/artifacts` answers 200, a subscription yields a block containing `event: ready`, `close` yields `CoreClosed`. This exercises the two `unsafe` lines of `ffi.rs`.
- [ ] **Step 4: Run** `bazelisk test //apps/clax_app:core_frame_test //apps/clax_app:core_core_client_test //apps/clax_app:real_core_real_test` — FAIL.
- [ ] **Step 5: Implement** (`NativeApi.postCObject` and `NativeApi.initializeApiDLData` are not needed: `postCObject` is passed as the function pointer). Use `malloc`/`calloc` from `package:ffi` (already in `pubspec.lock`? check `grep -n "^  ffi:" apps/clax_app/pubspec.lock`; if absent, `flutter pub add ffi` through `bazelisk run @rules_flutter//flutter:pub -- add ffi --directory apps/clax_app` and add it to `:clax_app`'s `deps`), copy the request into native memory, call, free in a `finally`.
- [ ] **Step 6: Run** — PASS; `bazelisk test //apps/clax_app:dart_analyze` — PASS.
- [ ] **Step 7: Commit** the listed files: "Call the embedded core from Dart: frames, the four functions, a client per isolate, and a fake core for tests".

### Task 13: `HttpClaxApi` and `SseClaxStream` over a transport

**Files:**
- Create: `apps/clax_app/lib/clax/transport.dart`, `apps/clax_app/lib/core/core_transport.dart`, `apps/clax_app/test/core/core_transport_test.dart`
- Modify: `apps/clax_app/lib/clax/http_api.dart:16-75,408-440,943-993` (`_send` and `_sse` over the transport; the endpoint constructor becomes `HttpClaxApi.loopback(endpoint, …)` building a `LoopbackTransport`, and `HttpClaxApi.over(ClaxTransport t, {sessionId, …})`), `apps/clax_app/test/clax/http_api_test.dart` (its `HttpClaxApi(...)` constructions use `.loopback`)

**Interfaces:**
- Produces:
  ```dart
  // lib/clax/transport.dart
  class TransportAnswer { const TransportAnswer(this.status, this.body); final int status; final Uint8List body; }
  abstract interface class ClaxTransport {
    /// Throws ClaxApiError(status: 0, code: 'timeout' | 'unreachable') when no answer came.
    Future<TransportAnswer> send(String method, String path, {Map<String, String>? query, Map<String, String> headers = const {}, Uint8List? body, required Duration timeout});
    /// The SSE messages of GET [path]; reconnects as reconnectingSse does for HTTP.
    Stream<SseMessage> sse(String path, {Map<String, String>? query, String? Function()? resumeId});
  }
  class LoopbackTransport implements ClaxTransport { LoopbackTransport(DaemonEndpoint Function() endpoint, {HttpClient? client, …the event timeouts}); }

  // lib/core/core_transport.dart
  class CoreTransport implements ClaxTransport {
    CoreTransport(CoreCalls core);
    // send: CoreRequest(as: owner); a status-0 never arises from transport, only
    //   'core_closed' when the core is closed.
    // sse: a CoreSubscription's blocks parsed with sse.dart's SseParser; a
    //   new subscription after `core_closed` reopens are not this class's job.
  }
  ```
  `HttpClaxApi` keeps every public method unchanged; its `agent` flag still adds `x-clax-session`; the `Authorization` header is added by `LoopbackTransport` only.
- Consumes: Task 12.

- [ ] **Step 1: Write `core_transport_test.dart`**: `HttpClaxApi.over(CoreTransport(FakeCore.fromApi(api)))` runs the same five representative calls the HTTP tests check (`listArtifacts`, `createArtifact`, `postOwnerComment`, `pollFeedback(wait: 20 s)` answered when the fake pushes a comment, `batchDocs`) and the requests the fake saw carry `x-clax-session` for agent calls and no `authorization`; `SseClaxStream(connect: (resume) => CoreTransport(fake).sse('/api/stream', resumeId: resume), update: …)` delivers a pushed `version` event to a subscriber of `artifact:<id>`.
- [ ] **Step 2: Run** `bazelisk test //apps/clax_app:core_core_transport_test` — FAIL.
- [ ] **Step 3: Refactor** `http_api.dart`: move the `HttpClient` code of `_send` and `_sse` into `LoopbackTransport` verbatim; `_send` becomes `transport.send(...)` with the JSON body encoded as today; `ClaxApiError` mapping unchanged. Implement `CoreTransport`.
- [ ] **Step 4: Run** the new test and every `test/clax/` test (`bazelisk test //apps/clax_app:all --test_tag_filters=-manual 2>&1 | tail` filtered to `clax_`) — PASS.
- [ ] **Step 5: Commit** the listed files: "Send the app's API calls and event streams through a transport, over loopback HTTP or the embedded core".

### Task 14: The scheme and the channel on Android and Apple (with the spike)

**Files:**
- Create: `apps/clax_app/android/app/src/main/kotlin/dev/empathic/clax_app/SchemeHost.kt`, `apps/clax_app/macos/Runner/SchemeHost.swift`, `apps/clax_app/ios/Runner/SchemeHost.swift` (the same file content as the macOS one; iOS globs `ios/Runner/*.swift`), `bazel/flutter/plugin_overlays/webview_flutter_wkwebview/` (the patch and its overlay BUILD, as `speech_to_text`'s overlay is laid out), `scripts/patch-pub.sh` (applies the same patch to a copy for the `-native` twins), `apps/clax_app/integration_test/scheme_spike_test.dart`
- Modify: `apps/clax_app/android/app/src/main/kotlin/dev/empathic/clax_app/MainActivity.kt` (register `SchemeHost` on `dev.empathic.clax/scheme`), `apps/clax_app/macos/Runner/MainFlutterWindow.swift` and `apps/clax_app/ios/Runner/AppDelegate.swift` (register the handler hook), `apps/clax_app/BUILD.bazel` (`runner_deps` gains `@rules_android_maven//:androidx_webkit_webkit`; pin it in `MODULE.bazel` `maven.install` and re-pin per `docs/bazel.md` "A pub package"), `MODULE.bazel`, `bazel/android/maven_install.json`

**Interfaces:**
- Produces the native side of channel `dev.empathic.clax/scheme`:
  - Native → Dart `request {webViewId, reqId, url, method, headers: {…}, mainFrame: bool}`; Dart → native `respond {reqId, status, headers: {…}, body: Uint8List}`. Android's `shouldInterceptRequest` (a delegating `WebViewClient` installed by `attach {webViewId}` over `WebViewFlutterAndroidExternalApi.getWebView(engine, id)` and `webView.webViewClient`) posts the request to the main thread and blocks its IO thread on a `CompletableFuture` with a 30 s cap (then answers 504); Apple's `WKURLSchemeHandler.webView(_:start:)` posts and completes the `WKURLSchemeTask` on `respond` (a `stop` drops the answer).
  - Native → Dart `channel {webViewId, message: String}` for `ClaxHost` messages that pass the origin rule: Android `WebViewCompat.addWebMessageListener(view, "ClaxHost", setOf(<shell origin>), listener)` dropping `!isMainFrame`; Apple a `WKScriptMessageHandler` named `ClaxHost` dropping `!frameInfo.isMainFrame || securityOrigin.protocol != "clax" || securityOrigin.host != "app"`.
  - The `webview_flutter_wkwebview` patch adds `public static var configurationHook: ((WKWebViewConfiguration) -> Void)?` to the plugin's `WKWebView` creation, called before the view is made; the runner sets it to `config.setURLSchemeHandler(SchemeHost.shared, forURLScheme: "clax")`.
- Consumes: Task 12 (for the spike page's answers).

- [ ] **Step 1: Write `integration_test/scheme_spike_test.dart`** (macOS, `just app-integration`; a debug build only): opens a `WebViewWidget` on `clax://app/spike.html` answered by a Dart table (no core): the shell page frames `clax://abcdefghijkl.c/page.html`, and both report by `ClaxHost` (the shell) and `postMessage` (the frame, relayed by the shell): `window.origin`, `localStorage` set and read back after a `reload()`, `crypto.subtle` present, the frame's `parent.document` access throwing, the frame's `fetch('clax://app/a/x')` and `fetch('clax://app/api/artifacts')` failing or opaque, `event.origin` of the frame's message equal to `clax://abcdefghijkl.c`, and no `ClaxHost` message arriving from the frame when it posts one. Assert each.
- [ ] **Step 2: Run** `bazelisk test //apps/clax_app:integration_scheme_spike` in the background — FAIL (no handler).
- [ ] **Step 3: Implement** the Apple handler, the plugin patch and the hook; run the spike — PASS on macOS. **If `window.origin` is `"null"` or `localStorage` throws, stop here and report to the owner (spec §4.3): nothing later works without real origins.**
- [ ] **Step 4: Implement** the Android side. The same spike runs on the phone or an emulator by hand: `flutter test integration_test/scheme_spike_test.dart -d <device>` through `just app-integration-native` is macOS-only, so instead build the debug APK with the spike as its entry point (`bazelisk build //apps/clax_app:debug_apk --define=CLAX_SPIKE=1` if that target exists; otherwise add a `spike` flavor to `integration_main`) and record each value from `adb logcat -s flutter`; write the results into the commit message.
- [ ] **Step 5: Commit** the listed files: "Answer the app's scheme in its WebViews and accept ClaxHost only from the shell's main frame, on Android and Apple".

### Task 15: The host relay in the app's WebViews

**Files:**
- Create: `apps/clax_app/lib/core/scheme.dart`, `apps/clax_app/lib/core/channel_relay.dart`, `apps/clax_app/test/core/scheme_test.dart`, `apps/clax_app/test/core/channel_relay_test.dart`
- Modify: `apps/clax_app/lib/ui/webview_surface.dart:60-260` (with a `CoreCalls` given: attach the scheme host, load `AppOrigins.shell + /a/<id>?app=1`, route `ClaxHost` messages of type `call`, `callChunk`, `callAbort`, `subscribe`, `unsubscribe` to the relay and every other one to `ShellSession` as today), `apps/clax_app/lib/ui/inspector_surface.dart:327` (the inspect URL from `AppOrigins`), `apps/clax_app/lib/ui/app_screen.dart:441` (`appUrl` from `AppOrigins` when the surface has a core)

**Interfaces:**
- Produces:
  ```dart
  // lib/core/scheme.dart
  /// Answers one scheme request (spec §4.2) through the core.
  class SchemeAnswerer {
    SchemeAnswerer(this.core, this.origins);
    Future<SchemeAnswer> answer(SchemeRequest r);
  }
  class SchemeRequest { const SchemeRequest({required this.url, required this.method, required this.headers, required this.mainFrame}); final Uri url; final String method; final Map<String, String> headers; final bool mainFrame; }
  class SchemeAnswer { const SchemeAnswer(this.status, this.headers, this.body); final int status; final Map<String, String> headers; final Uint8List body; }

  // lib/core/channel_relay.dart
  /// The host's half of hostFetch (web/shell/src/transport.ts).
  class ChannelRelay {
    ChannelRelay(this.core, {required this.runJs});
    final CoreCalls core;
    /// Runs `window.claxHost.<method>(...)` in the shell document.
    final Future<void> Function(String js) runJs;
    /// Handles one ClaxHost message of the relay's types; false for others.
    bool handle(Map<String, Object?> msg);
    void dispose(); // cancels subscriptions, drops pending answers
  }
  ```
  `SchemeAnswerer` rules: `GET`/`HEAD` only (else 405); shell host → `CallAs.shellPage`, refusing `/a/…` unless `mainFrame` (403); `<id>.c` host → `CallAs.content` with `host: id`; forward only `range`, `if-none-match`, `accept`; drop `access-control-*` from the answer.
  `ChannelRelay`: `call` → `CoreRequest(as: owner)` → `runJs('claxHost.callResult(<id>, <json>)')` with `body` as text when the answer's content type is text or JSON and its bytes are valid UTF-8, else `body64`; a body over 1 MiB is answered in `claxHost.callResultChunk(id, n, last, body64)` messages (add that method to `host.ts` and `transport.ts` in this task, with a vitest case); `callChunk` assembles; `callAbort` drops the answer; `subscribe` opens a core subscription and forwards each block by `claxHost.streamBlock(id, <json string>)`.
- Consumes: Tasks 9–14.

- [ ] **Step 1: Write `scheme_test.dart`** (with `FakeCore`): each rule above, one test each, asserting the `CoreRequest` the fake saw and the answer.
- [ ] **Step 2: Write `channel_relay_test.dart`**: a `call` is answered with the right `callResult` JS; a binary answer goes as `body64`; a 3 MiB answer goes in three `callResultChunk` calls; a `callAbort` before the answer runs no JS; `subscribe` forwards pushed blocks in order; `dispose` cancels the subscription.
- [ ] **Step 3: Run** `bazelisk test //apps/clax_app:core_scheme_test //apps/clax_app:core_channel_relay_test //web:unit --test_arg=shell/src/transport.test.ts` — FAIL.
- [ ] **Step 4: Implement**, wiring `webview_surface.dart` to the relay and the scheme when it is given a `CoreCalls` (nothing gives it one until Task 16; the loopback path is unchanged).
- [ ] **Step 5: Run** — PASS (the app is looked at in Task 16, when it runs on the core).
- [ ] **Step 6: Commit** the listed files: "Answer the app's scheme and relay the shell's calls and events through the core in the app's WebViews".

## Phase 3: All three platforms off the daemon (spec §2.5, §6, §10, §11, §12)

### Task 18: Old homes, the launch measure and its baseline

Runs before Task 16, so the baseline is the daemon's.

**Files:**
- Create: `crates/clax-store/tests/fixtures/old-migrations/flutter-app-853502f77.sql`, `crates/clax-embed/tests/old_homes.rs`, `scripts/perf-app-launch.sh`, `scripts/test-perf-app-launch.sh`
- Modify: `web/perf/budget.json` (`app_home_usable_ms`, `app_home_cover_ms`, each `{"budget": <the baseline's p90 rounded up to 50 ms>}`), `scripts/run.sh` (the `perf` recipe runs `scripts/perf-app-launch.sh --macos` on macOS after the others, never failing), `scripts/BUILD.bazel` (`sh_test` `perf_app_launch_test`), `docs/usage.md` ("Measuring the home screen's start": the script)

**Interfaces:**
- Produces `scripts/perf-app-launch.sh [--macos|--android] [--runs N] [--app <Clax.app>]`: dry run without `--i-know-what-i-am-doing` (prints each step), as `scripts/app-phone.sh` does. macOS: builds or takes `--app`, makes a scratch `CLAX_APP_DIR` inside `~/Library/Containers/dev.empathic.clax/Data/tmp/perf-<time>`, runs `Contents/MacOS/Clax` N times (each waits for a new `home-usable` line in `<dir>/support/timings.jsonl`, with a 60 s cap that counts the run as missing, then sends the app `SIGTERM` by its PID and waits for it to exit), and writes `{"app_home_usable_ms": {"median": …, "p90": …, "runs": N}, "app_home_cover_ms": {…}}` to `$CLAX_PERF_JSON` or stdout. Android: `adb shell am force-stop` and `am start -W` N times, then `adb exec-out run-as dev.empathic.clax cat files/timings.jsonl` (the support directory's path on Android: read it from `getApplicationSupportDirectory` once by `adb shell run-as dev.empathic.clax ls`), the same JSON. The first run after an install is dropped (no snapshot yet; `docs/usage.md`).
- Produces the fixture: the schema (`sqlite3 <home>/clax.db .schema`) and a handful of rows (two artifacts with three versions, two threads with comments, one app session, a `db_read_grants` row) of a home written by this branch's `clax` at commit 853502f77 (`user_version` 43), as an SQL script in the format the other fixtures in that directory use.
- Consumes: Task 5.

- [ ] **Step 1: Make the fixture.** Build `//crates/clax-cli:clax` at 853502f77 (`git worktree add ../fixture-853502f77 853502f77` is forbidden by the shared-worktree rule only if left behind: create it under `target/`, build with `bazelisk build //crates/clax-cli:clax` there, remove it with `git worktree remove` when done), run it on a scratch `CLAX_HOME` with a free port, publish and comment through its API with `curl`, stop it by its PID, dump. Commit nothing from the scratch home but the `.sql`.
- [ ] **Step 2: Write `old_homes.rs`**:
  ```rust
  #[tokio::test]
  async fn the_installed_flutter_app_home_backs_up_and_migrates() {
      let home = fixture_home("flutter-app-853502f77.sql"); // loads the script into <tmp>/clax.db
      let (embed, events) = open(&home);
      wait_ready(&events);
      assert!(std::fs::read_dir(home.join("backups")).unwrap().any(|e| {
          e.unwrap().file_name().to_string_lossy().starts_with("clax-43-")
      }));
      let list = call_json(&embed, "GET", "/api/artifacts").await;
      assert_eq!(list["artifacts"].as_array().unwrap().len(), 2);
      // every version and comment of the fixture is readable
  }
  #[tokio::test]
  async fn the_perf_home_opens_through_the_core() {
      // seeds 300 artifacts, 2,400 threads, 7,200 comments with the seeding
      // helper scripts/perf-daemon.sh uses (its Rust seeder, if it has one;
      // else the store's own insert functions in a loop), then opens it and
      // lists the gallery. No time is asserted.
  }
  ```
  (Task 24 extends the first test to `user_version` 44.)
- [ ] **Step 3: Run** `bazelisk test //crates/clax-embed:all --test_arg=old_homes::` — PASS (no migration is new yet; this pins today's adoption through the core).
- [ ] **Step 4: Write `test-perf-app-launch.sh`** (the dry run prints the steps for `--macos` and `--android`; a fake `timings.jsonl` of five runs gives the median and p90 the script prints) and the script; run `bazelisk test //scripts:perf_app_launch_test` — PASS.
- [ ] **Step 5: Measure the baseline** on the current build: `scripts/perf-app-launch.sh --macos --runs 9 --i-know-what-i-am-doing` with `CLAX_PERF_JSON=target/perf/app-launch-before.json`, and on the owner's phone only with the owner present (`--android`, after `just app-phone`). Set the budgets from these numbers. Append the run to the perf log: `scripts/perf-log.sh record`.
- [ ] **Step 6: Commit** the listed files (not the scratch homes), with the baseline numbers in the message: "Measure launch to a usable home screen on demand, and pin the installed app's home through the embedded core".

### Task 16: `CoreHost`: the app on the core

**Files:**
- Create: `apps/clax_app/lib/core/core_host.dart`, `apps/clax_app/test/core/core_host_test.dart`
- Modify: `apps/clax_app/lib/main.dart:108-262` (the core instead of the supervisor; `api: (core) => HttpClaxApi.over(CoreTransport(core), …)`; `key` changes call `CoreHost.keyChanged`; every surface is given the core's `CoreCalls`), `apps/clax_app/lib/app.dart:110-200,491-501,581,870,1462-1556,1621,2037,2054,2264,2332,2518,2733,2896,3164-3238` (`AppServices.core: CoreHost` instead of `supervisor`; `api: ClaxApi Function(CoreCalls)`; `surface: AppSurface Function()`; every endpoint gate becomes "the core is open"; the stream is `SseClaxStream` over `CoreTransport` with no `SharedClaxStream`), `apps/clax_app/lib/ui/error_screen.dart` (the migration-failure screen takes `CoreFailed`), `apps/clax_app/test/support/fake_supervisor.dart` users → `FakeCoreHost` (in `test/support/fake_core.dart`), `apps/clax_app/test/app_test.dart`, `test/app_background_test.dart`, `test/app_bundles_test.dart`, `test/home/app_home_test.dart`, `test/home/home_snapshot_shots_test.dart`, `test/agent/worker_restart_test.dart`, `integration_test/rig.dart`, `integration_test/ios_rig.dart`

**Interfaces:**
- Produces:
  ```dart
  class CoreHost {
    CoreHost({required this.home, required this.supportDir, required this.origins, required this.apiKey, CoreLib? lib, this.onTrim});
    final String home, supportDir;
    final AppOrigins origins;
    final Future<String?> Function() apiKey;
    /// Opens the core (once): deletes <support>/daemon-port, writes the
    /// [sample] section of config.toml as sampleConfig did, clears the
    /// WebViews' clax_key_* cookies once (a flag file <support>/.cookies-cleared),
    /// opens, then puts the key (PUT /api/sample/key) when there is one.
    void open();
    ValueListenable<CoreOpen> get state;
    CoreCalls get calls;          // valid once state is CoreReady
    int get handle;               // for the worker isolate (Task 20)
    Future<void> keyChanged();    // PUT /api/sample/key, or DELETE for none
    Future<void> trim();          // POST /api/core/trim
  }
  ```
  `test/support/fake_core.dart` gains `FakeCoreHost implements CoreHost`-shaped class (an interface `CoreHostLike` extracted if needed) with a settable state.
- Consumes: Tasks 12, 13, 15, 18.

- [ ] **Step 1: Read the threads** on `lib/main.dart`, `lib/app.dart`, `lib/daemon/supervisor.dart` (records 69557070, 9fbb9e03, c58da88f), `lib/ui/error_screen.dart`.
- [ ] **Step 2: Write `core_host_test.dart`** over a fake `CoreLib`: `open` deletes `daemon-port`, writes `config.toml`'s `[sample]`, opens once even when called twice, puts the key after `ready` and not before; `keyChanged` with no key sends `DELETE`; a `failed` open ends in `CoreFailed` with the backup path; `state` never goes back to opening after `ready`.
- [ ] **Step 3: Run** `bazelisk test //apps/clax_app:core_core_host_test` — FAIL.
- [ ] **Step 4: Implement** `CoreHost`, then rewire `main.dart` and `app.dart`. The app's first frame must not wait on the open: `open()` is called before `runApp` and returns at once; the home WebView's first request waits in the core. Update the tests listed above to `FakeCoreHost`/`FakeCore` (they keep their assertions; a test that asserted a restart or a health probe is deleted with a line in the commit message naming it).
- [ ] **Step 5: Run** every app test: `bazelisk test //apps/clax_app:all` in the background (`target/plan-logs/core-16.log`) — PASS, `dart_analyze` included.
- [ ] **Step 6: Look at it.** `just app-macos-dev`: the home screen shows, an app opens, a comment gets its reply, the inbox updates live, Settings saves a key without anything restarting. Screenshots `target/flutter-app/shots/core-home.png`, `core-app.png`; open each with Read. Run `scripts/perf-app-launch.sh --macos --runs 9 --i-know-what-i-am-doing` with `CLAX_PERF_JSON=target/perf/app-launch-after.json` and put both medians in the commit message.
- [ ] **Step 7: Commit** the listed files: "Run the app on the embedded core: open it at launch, call it in process, and load the WebViews from the scheme".
- [ ] **Step 8: Propose closes** of records 69557070, 9fbb9e03 and c58da88f in the task report (evidence: the key test of Step 2 and the absence of a health limit), not closing them.

### Task 17: Ship the library instead of `clax`

**Files:**
- Modify: `apps/clax_app/BUILD.bazel:516-535` (`native_files`: `//crates/clax-embed:clax_embed_android_arm64` → `arm64-v8a/libclax_embed.so`, x86_64 likewise), `:571-615` (`macos_clax` removed from `additional_contents`; `{":macos_core": "Frameworks"}` where `macos_core` is `macos_helper(binary = "//crates/clax-embed:clax_embed_macos", entitlements = None)` or, if `macos_helper` requires an executable, a new `macos_dylib` rule in `bazel/flutter/macos.bzl` that copies and signs ad hoc), the integration apps at `:890` likewise, `bazel/flutter/ios.bzl:87-93,195` (`ios_daemon` → the runner's `deps` gain `//crates/clax-embed:clax_embed_ios_c`; `ios_app_test` checks `:clax_embed_ios_builds`), `apps/clax_app/tool/apk-check.sh` (expects `lib/<abi>/libclax_embed.so` with the four symbols, no `libclax.so`; prints each `.so`'s size), `apps/clax_app/tool/macos-app-check.sh` (expects `Contents/Frameworks/libclax_embed.dylib` with the four symbols, no `Contents/Resources/clax`), `apps/clax_app/tool/ios-app-check.sh` (the runner exports the four `clax_embed_*` symbols instead of the four `clax_*`), `apps/clax_app/android/app/src/main/AndroidManifest.xml:35` (`extractNativeLibs` removed), `apps/clax_app/android/app/build.gradle.kts:37-41` (`useLegacyPackaging` removed), `scripts/build-app-clax.sh` (the native twin builds `clax-embed` into `jniLibs/<abi>/libclax_embed.so` and `build-native/macos/libclax_embed.dylib`), `apps/clax_app/macos/Runner.xcodeproj/project.pbxproj` ("Embed clax" phase copies the dylib into `Frameworks` instead), `scripts/test-build-app-clax.sh`

**Interfaces:**
- Produces: APK, macOS app and iOS app carrying the core and no `clax`.
- Consumes: Tasks 8, 16.

- [ ] **Step 1: Change the checks first** (`apk-check.sh`, `macos-app-check.sh`, `ios-app-check.sh`, `test-build-app-clax.sh`). Run `bazelisk test //apps/clax_app:apk_test //apps/clax_app:macos_app_test //apps/clax_app:ios_app_test //scripts:build_app_clax_test` — FAIL.
- [ ] **Step 2: Change the builds.** Run the same tests — PASS. Write each artifact's before and after size (the checks print them) into the commit message.
- [ ] **Step 3: Look at it** on the Mac: `just app-macos`, open the zip's app with a scratch `CLAX_APP_DIR`, see the home screen and open an app. On the phone, only with the owner present: `just app-phone` and the same (Task 33 is the full device check).
- [ ] **Step 4: Commit** the listed files: "Ship the embedded core in the APK, the Mac app and the iOS app instead of the clax executable".

## Phase 4: The queue in a worker isolate (spec §7)

### Task 19: The queue's model and its pure `update`

**Files:**
- Create: `apps/clax_app/lib/queue/model.dart`, `apps/clax_app/lib/queue/msg.dart`, `apps/clax_app/lib/queue/effect.dart`, `apps/clax_app/lib/queue/update.dart`, `apps/clax_app/test/queue/update_test.dart`, `apps/clax_app/test/queue/update_properties_test.dart`
- Modify: `apps/clax_app/BUILD.bazel` (`SUITES` gains `queue`)

**Interfaces:**
- Produces (all immutable, `==` by value, `toJson`/`fromJson` for the isolate boundary):
  ```dart
  // model.dart
  typedef JobId = String;
  enum JobKind { make, change }
  enum JobState { queued, running, waiting, paused, stopped, failed, cancelled, done }
  enum WaitingFor { claude, answer, render }
  class Job {
    const Job({required this.id, required this.kind, required this.state, this.step, this.artifactId,
      this.title, required this.request, this.threads = const {}, this.plan, this.spentUsd = 0,
      this.stops = 0, this.error, this.waitingFor, this.leaseOwner, this.rev = 0, this.steps = const {}});
    final JobId id; final JobKind kind; final JobState state; final String? step;
    final String? artifactId; final String? title; final String request;
    /// thread ID → the newest comment ID the job answers.
    final Map<String, String> threads;
    final PlanState? plan; final double spentUsd; final int stops;
    final JobError? error; final WaitingFor? waitingFor;
    /// Another worker's lease, when one holds the job.
    final String? leaseOwner; final int rev;
    /// Committed steps by key (publish:<base>, reply, turn:<n>, done).
    final Map<String, StepResult> steps;
    Job copyWith({...});
  }
  class QueueModel {
    const QueueModel({this.jobs = const {}, this.inFlight = const {}, this.keepAlive = false,
      this.coreOpen = false, this.claude = Reachability.unknown, this.canRender = true, this.workerId = '', this.nextEffect = 0});
    final Map<JobId, Job> jobs; final Map<int, JobId> inFlight; final bool keepAlive;
    final bool coreOpen; final Reachability claude; final bool canRender; final String workerId; final int nextEffect;
    static const maxRunning = 3;
  }
  enum Reachability { unknown, reachable, unreachable }
  /// What the UI sees (spec §7.2).
  class QueueState { const QueueState(this.jobs); final List<JobView> jobs; }
  class JobView { /* id, kind, state, step label, title, artifactId, waitingFor, error text, offers: {keepGoing, retry, cancel, discard} */ }
  QueueState viewOf(QueueModel m);

  // msg.dart — sealed class Msg with:
  //   Intents: Make(request, {title}), Change(artifactId, threads), Cancel(jobId),
  //     Retry(jobId), KeepGoing(jobId), Discard(jobId), Answer(jobId, questionId, answer)
  //   Loaded(List<Job> jobs), CoreOpened(), JobChanged(Job job)
  //   TurnDone(effect, jobId, TurnOutcome outcome), TurnFailed(effect, jobId, TurnFailure kind),
  //   CoreAnswered(effect, jobId, int status, Map<String, Object?> body),
  //   StepCommitted(effect, jobId, String key, StepResult result, {bool replayed}),
  //   StepRefused(effect, jobId, String key, String code)   // version_moved, doc_moved, not_rendered_clean, job_moved
  //   Rendered(effect, jobId, RenderOutcome outcome), RenderLost(effect, jobId), CanRender(bool)
  //   ClaudeReachable(bool), Tick(DateTime now), Woke({required bool foreground})

  // effect.dart — sealed class Effect with: CoreCall(id, jobId, CoreRequestSpec), StartTurn(id, jobId, TurnInput),
  //   Render(id, jobId, RenderSpec), CommitStep(id, jobId, String key, Map<String, Object?> body),
  //   PatchJob(id, jobId, int ifRev, Map<String, Object?> patch), Notice(NoticeKind kind, JobId jobId, String title),
  //   KeepAlive(bool on), PublishState(QueueState state), ProbeClaude()

  // update.dart
  (QueueModel, List<Effect>) update(QueueModel m, Msg msg);
  ```
  The rules `update` implements (each is a test):
  1. `Make` adds a `queued` job (ID from the message's `id` field, which the client derives: `sha256(request + clientNonce)` truncated), emits `PatchJob`-free `CoreCall(POST /api/jobs)` and, if fewer than 3 run, starts it (`state: running`, `StartTurn`).
  2. A job whose `leaseOwner` is another worker is never started.
  3. `TurnFailed(network)` → `waiting/claude`, `ProbeClaude`, no new turn until `ClaudeReachable(true)`.
  4. `TurnDone` with a finish → `CommitStep('publish:<base>', …)` exactly once per base; `StepRefused(version_moved)` → re-plan (a `StartTurn` with the moved base), never a second `CommitStep` for the same base.
  5. `StepCommitted('publish:<b>')` → `CommitStep('reply', …)`; `StepCommitted('reply')` → `done`, `Notice(ready)`.
  6. A `Render` effect's `RenderLost` → one more `Render`; a second `RenderLost` → `waiting/render` until `CanRender(true)`; no `StartTurn` in between.
  7. `Cancel` → `cancelled`, no effect for that job afterwards but `PatchJob`; in-flight results for it are dropped.
  8. Limits: a turn result over the job's limit → `stopped` (Keep going offered); `KeepGoing` → `running` with the carried state; a second stop → `KeepGoing` plans the rest (`kind` stays, `plan` set).
  9. `KeepAlive(true)` is emitted when the count of `running` or `waiting` jobs goes from 0 to more, `KeepAlive(false)` when it goes back to 0.
  10. Every change of `viewOf(m)` emits one `PublishState`.
  11. `Loaded` resumes each job by spec §7.4's table (its steps decide).
  12. `Tick` past a render's 60 s deadline is a `RenderLost`.
- Consumes: nothing (pure Dart).

- [ ] **Step 1: Write `update_test.dart`**: one `group` per rule above, each with at least one `test` constructing a model, sending the message, and asserting the whole returned model and effect list, e.g.
  ```dart
  test('a publish refused for a moved base re-plans and never commits that base again', () {
    final m0 = model(jobs: [job('j', state: JobState.running, steps: {})]);
    final (m1, e1) = update(m0, TurnDone(1, 'j', finishOutcome(base: 4)));
    expect(e1.whereType<CommitStep>().single.key, 'publish:4');
    final (m2, e2) = update(m1, StepRefused(2, 'j', 'publish:4', 'version_moved'));
    expect(e2.whereType<CommitStep>(), isEmpty);
    expect(e2.whereType<StartTurn>().single.input.base, 5);
    expect(m2.jobs['j']!.state, JobState.running);
  });
  test('a job whose lease another worker holds is left alone', () {
    final m0 = model(workerId: 'w1', jobs: [job('j', state: JobState.queued, leaseOwner: 'w2')]);
    final (m1, e1) = update(m0, Loaded([m0.jobs['j']!]));
    expect(e1.whereType<StartTurn>(), isEmpty);
    expect(m1.jobs['j']!.state, JobState.queued);
  });
  ```
  and a table test over every `JobState` × every intent asserting the documented next state (a `const` table in the test file, 8 × 7 entries).
- [ ] **Step 2: Write `update_properties_test.dart`**: a seeded `Random(42)` generating 2,000 sequences of up to 40 messages from a generator that only produces messages valid for the effects emitted so far (results answer issued effect IDs), checking after each step: no job has two `CommitStep`s with one key; no effect for a `cancelled` or `done` job but `PatchJob`; at most three `running`; `keepAlive` equals "some job running or waiting"; no `StartTurn` for a job between a `RenderLost` and its next `Rendered`.
- [ ] **Step 3: Run** `bazelisk test //apps/clax_app:queue_update_test //apps/clax_app:queue_update_properties_test` — FAIL.
- [ ] **Step 4: Implement** `model.dart`, `msg.dart`, `effect.dart`, `update.dart`. `update` imports nothing from `dart:io`, `dart:async`, `package:flutter`, or `lib/agent` beyond the plain data types it carries (`TurnInput`, `TurnOutcome` are defined in `lib/queue/` as data and mapped by the runner).
- [ ] **Step 5: Run** — PASS; `dart_analyze` — PASS.
- [ ] **Step 6: Commit** the listed files: "Model the app's queue of makes and changes as a pure update over jobs and their steps".

### Task 20: The effect runner and the worker isolate

**Files:**
- Create: `apps/clax_app/lib/queue/runner.dart`, `apps/clax_app/lib/queue/worker_main.dart`, `apps/clax_app/lib/queue/client.dart`, `apps/clax_app/lib/queue/codec.dart` (Msg/Effect/QueueState to and from JSON-able maps), `apps/clax_app/test/queue/runner_test.dart`, `apps/clax_app/test/queue/client_test.dart`, `apps/clax_app/test/queue/codec_test.dart`

**Interfaces:**
- Produces:
  ```dart
  // runner.dart — the only code in the worker that awaits.
  class EffectRunner {
    EffectRunner({required this.core, required this.turns, required this.render, required this.toUi,
      required this.connection, required this.clock, required this.send});
    final CoreCalls core;
    /// Runs one model turn with the agent's runner and tools (lib/agent).
    final Future<TurnOutcome> Function(TurnInput input, CancelToken cancel) turns;
    /// Asks the UI isolate to render; completes with its answer.
    final Future<RenderOutcome> Function(RenderSpec spec) render;
    final void Function(Object uiMessage) toUi;       // KeepAlive, Notice, PublishState
    final ApiConnection connection; final Clock clock;
    /// Feeds a result back into the loop.
    final void Function(Msg msg) send;
    void run(Effect e);
    void cancelJob(JobId id);
  }

  // worker_main.dart
  /// The worker isolate's entry: attaches to the core by handle, builds the
  /// agent's runners from [WorkerStart], loads jobs and loops:
  /// msg → update → effects → runner.
  void workerMain(WorkerStart start);
  class WorkerStart { /* SendPort toUi; int coreHandle; String supportDir; Map settings; String? apiKey; String workerId; */ }

  // client.dart — the UI isolate's only view of the queue.
  class QueueClient {
    static Future<QueueClient> spawn({required CoreHost core, required String supportDir, required AppSettings settings, required String? apiKey, RenderService? render, BackgroundWork? background});
    ValueListenable<QueueState> get state;
    void send(Msg intent);        // only intents; others are refused by an assert
    void settingsChanged(AppSettings s); void keyChanged(String? key);
    Future<void> dispose();
  }
  ```
  In this task the durable state stays in the JSON stores (`PendingMake`, `FileChangeLedger`, `PlanStore`, `FileAskedQuestions`, `FileStoppedThreads`, `FileUnfinishedChanges`, `FileConsentMemory`), now opened by the worker isolate only: `CommitStep` and `PatchJob` effects are run by a `JsonJobStore` adapter in `runner.dart` that writes them as today's code does (the ledger before a publish, `docs_done` after). Task 28 swaps the adapter for core calls.
- Consumes: Task 19, Task 16 (`CoreHost.handle`).

- [ ] **Step 1: Write `codec_test.dart`**: every `Msg` and `Effect` variant and a `QueueState` round-trip through `codec.dart` (generated list of one instance each).
- [ ] **Step 2: Write `runner_test.dart`** on `fakeAsync` with `FakeCore` and `ScriptedRunner` (`lib/agent/runner/scripted_runner.dart`): a `StartTurn` runs one scripted turn and sends `TurnDone`; a network failure sends `TurnFailed(network)`; `CommitStep('publish:4')` with the JSON adapter writes the ledger record before calling the fake's publish and sends `StepCommitted`; `cancelJob` aborts a turn in flight and sends nothing for it.
- [ ] **Step 3: Write `client_test.dart`**: `QueueClient.spawn` with a real isolate and `FakeCore`-backed handle replaced by an in-isolate fake (the worker takes a `CoreCalls Function()` in tests through `WorkerStart.testCore`, a `SendPort` to a fake served in the test isolate): a `Make` intent ends in a `QueueState` with one job `running`, then `done` after the scripted run; the UI side never awaits a job (the test's `send` returns `void`). A worker whose loop throws (a scripted turn that throws a `StateError` outside the runner's guards) is restarted once and its jobs resume from the fake core; a second death within a minute of the worker's clock leaves the jobs showing "Clax's builder stopped. Restart Clax to go on." in `QueueState`.
- [ ] **Step 4: Run** the three — FAIL. **Step 5: Implement.** **Step 6: Run** — PASS.
- [ ] **Step 7: Commit** the listed files: "Run the queue in a worker isolate: an effect runner around the pure update, and a client the UI reads and sends intents to".

### Task 21: Rendering asked of the UI isolate

**Files:**
- Create: `apps/clax_app/lib/queue/render_service.dart`, `apps/clax_app/test/queue/render_bridge_test.dart`
- Modify: `apps/clax_app/lib/ui/inspector_surface.dart` (`WebViewInspector` usable by `RenderService` with no `ClaxApi` of its own; it loads the draft by `AppOrigins`), `apps/clax_app/lib/queue/client.dart` (routes `RenderRequest` to the service and answers)

**Interfaces:**
- Produces:
  ```dart
  class RenderSpec { /* draftId, revision, kind: inspect|tests|migration, settleMs, maxBytes, files (tests), migration {file, from, to} */ }
  sealed class RenderOutcome {}  // Inspection(png, errors, …) | TestsRun(results) | MigrationRun(result) | RenderError(message)
  class RenderService {
    RenderService(this.inspector, {required this.clock});
    final Inspector inspector; final Clock clock;
    /// Whether a WebView can be made now (false while the app's UI engine is
    /// detached on Android, or the app is in the background on iOS).
    ValueListenable<bool> get canRender;
    Future<RenderOutcome> render(RenderSpec spec);
  }
  ```
  The worker's 60 s deadline and one retry are `update`'s (Task 19 rules 6, 12); the service only renders and reports `canRender`.
- Consumes: Tasks 19, 20.

- [ ] **Step 1: Write `render_bridge_test.dart`**: a render answered by a fake inspector reaches the worker as `Rendered`; "a render lost to a reload is asked again once, then waits": the fake inspector's surface is torn down mid-render (`RenderLost`), the worker asks again, the second is lost too, the job is `waiting/render` in `QueueState`, `canRender` false→true releases one more `Render`; no `StartTurn` was issued meanwhile (the scripted runner's call count is unchanged).
- [ ] **Step 2: Run** — FAIL. **Step 3: Implement.** **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** the listed files: "Render a job's draft on the UI isolate at the worker's request, and wait when no WebView can be made".

### Task 22: The UI on `QueueState`

**Files:**
- Modify: `apps/clax_app/lib/ui/make_service.dart` (`MakeService` and `MakeSession` become views over `QueueState` for `make` jobs; `keepGoing`, `cancel`, `dismiss`, `resume` send intents), `apps/clax_app/lib/agent/worker.dart` (`FeedbackWorker`'s scheduling moves into `update`; what stays is the per-comment polling that turns a comment into a `Change` intent, run in the worker), `apps/clax_app/lib/ui/change_tracker.dart`, `apps/clax_app/lib/ui/working_chip.dart:285-299`, `apps/clax_app/lib/ui/change_sheet.dart`, `apps/clax_app/lib/ui/thread_detail.dart:878-1156`, `apps/clax_app/lib/ui/launcher.dart:724-774`, `apps/clax_app/lib/ui/split/sidebar.dart:428`, `apps/clax_app/lib/ui/making_screen.dart`, `apps/clax_app/lib/home/home_host.dart:739`, `apps/clax_app/lib/app.dart` (builds `QueueClient` once the core is open and passes it down; `_MakeDetail` at 3313-3336 sends `Discard`/`Make`), and their tests (`test/ui/make_service_test.dart`, `test/ui/change_tracker_test.dart`, `test/agent/worker_test.dart`, `test/ui/launcher_test.dart`, `test/ui/making_screen_test.dart`, `test/ui/thread_detail_test.dart`, `test/ui/working_chip_test.dart`)

**Interfaces:**
- Consumes: `QueueClient.state`, `QueueClient.send` (Task 20), `JobView` (Task 19).
- Produces: no new public API; every widget that showed a make or change reads a `JobView`.

- [ ] **Step 1: Read the threads** on each modified file (records 3f5fb92f, 2713e7df, ea3fb7b3, 5920b083, 0fad2b12 are open on them: keep their behaviour unless a step here changes it, and say so in the report).
- [ ] **Step 2: Rewrite each widget test** to drive a `FakeQueueClient` (in `test/support/fake_queue.dart`, added to `TEST_SUPPORT`) by pushing `QueueState`s and asserting the intents the widget sends; the visible text each test asserts stays as today (copy changes are Task 32).
- [ ] **Step 3: Run** them — FAIL. **Step 4: Change the widgets and services.** **Step 5: Run** `bazelisk test //apps/clax_app:all` in the background — PASS.
- [ ] **Step 6: Look at it** (`just app-macos-dev`): make an app, leave its build view while it builds, see its tile's progress, tap back in, Stop and Resume, Keep going on a change that hits its limit (a scripted agent: `--dart-define=CLAX_SCRIPTED_AGENT=<file>`); screenshots to `target/flutter-app/shots/queue-*.png`, each opened with Read.
- [ ] **Step 7: Commit** the listed files: "Show makes and changes from the worker's queue state and send the person's choices to it as intents".

### Task 23: Keep-alive, the engine on Android, the wake and trimming

**Files:**
- Create: `apps/clax_app/android/app/src/main/kotlin/dev/empathic/clax_app/ClaxApplication.kt` (holds the `FlutterEngine` in `FlutterEngineCache` under `"main"`), `apps/clax_app/android/app/src/main/kotlin/dev/empathic/clax_app/WakeWorker.kt` (a `CoroutineWorker` that starts a headless engine on entry point `queueWakeMain` when no engine runs, and waits until Dart says `idle` or 9 minutes pass), `apps/clax_app/lib/queue/wake_main.dart` (`@pragma('vm:entry-point') void queueWakeMain()`: opens the core, runs the worker with `canRender` false, posts the paused notice at a render, and says `idle` when no job is `running`), `apps/clax_app/test/queue/keep_alive_test.dart`
- Modify: `apps/clax_app/android/app/src/main/AndroidManifest.xml` (`android:name=".ClaxApplication"`; WorkManager's initializer as androidx.work requires), `MainActivity.kt` (`provideFlutterEngine` returns the cached engine; `shouldDestroyEngineWithHost` false), `apps/clax_app/BUILD.bazel` (`runner_deps` gain `@rules_android_maven//:androidx_work_work_runtime_ktx`, pinned in `MODULE.bazel` and `maven_install.json`), `apps/clax_app/lib/platform/background_work.dart` (`enqueueWake()`/`cancelWake()` on Android over `dev.empathic.clax/background`; `trim` from Swift and from Android's `onTrimMemory` calls `CoreHost.trim`), `apps/clax_app/ios/Runner/Background.swift` (no `ClaxDaemon`: sends `trim` to Dart), `apps/clax_app/lib/queue/client.dart` (applies `KeepAlive`, `Notice`, wake enqueue/cancel)

**Depends on the owner's decision** recorded on spec lines 117-121 (S6, the WorkManager wake). Until it is answered, do Steps 1-4 for the keep-alive, the cached engine and trimming only, and leave `WakeWorker.kt`, `wake_main.dart` and the wake's enqueue out; finish them once it is answered (option (a)), or drop them (option (c)).

**Interfaces:**
- Consumes: Tasks 20, 21, 16.
- Produces: `KeepAlive(true)` → `BackgroundWork.start(title, step)` and `enqueueWake()`; `KeepAlive(false)` → `stop()` and `cancelWake()` when no job is open.

- [ ] **Step 1: Write `keep_alive_test.dart`** with a fake `BackgroundWork`: a make's run starts the foreground work and enqueues the wake; its `done` stops it and cancels the wake; a job `paused` by `IosBackgroundWork`'s expiry keeps the wake enqueued on Android only.
- [ ] **Step 2: Run** — FAIL. **Step 3: Implement** Dart, Kotlin and Swift. **Step 4: Run** — PASS; `bazelisk test //apps/clax_app:apk_test` — PASS.
- [ ] **Step 5: Device check, with the owner present** (record what you saw in the commit message): on the phone, start a make, press Back (the activity finishes), the progress notification stays and the make lands; start another, `adb shell am kill dev.empathic.clax` while it runs in the background, wait for the wake (or `adb shell cmd jobscheduler run -f dev.empathic.clax <job ID>` to force it), see it resume until its next render and post "Open Clax to keep building <title>".
- [ ] **Step 6: Commit** the listed files: "Hold the process and the engine while jobs run, wake to resume them on Android, and trim the core's memory on request".

## Phase 5: Durable steps and publish checks in the core (spec §8)

### Task 24: Migration 44 and the store's jobs

**Files:**
- Create: `crates/clax-store/src/jobs.rs`
- Modify: `crates/clax-store/src/migrations.rs` (migration 44: spec §8.1's SQL verbatim, under `// 44: the app's jobs, their steps, and the agent's read consents (spec 2026-10-19-in-process-core-design §8.1)`; `pub const JOBS_MIGRATION: u32 = 44;`; `APP_MIGRATION_MARKS` gains the 44 marks `jobs`, `job_steps`, `job_threads`, `job_questions`, `agent_read_consents`, `client_imports`, `draft_renders`; `adopt_earlier_flutter_app` (`:1307`) adopts a database that has those tables at another `user_version` by checking each table's columns, and refuses one whose tables disagree), `crates/clax-store/src/lib.rs` (`pub mod jobs;`), `crates/clax-embed/tests/old_homes.rs` (the installed home ends at 44)
- Test: `crates/clax-store/src/jobs.rs` (unit tests), `crates/clax-store/src/migrations.rs` (adoption tests beside the existing ones at `~:2901`)

**Interfaces:**
- Produces:
  ```rust
  pub struct JobRow { pub id: String, pub client: String, pub kind: String, pub artifact_id: Option<String>,
      pub state: String, pub step: Option<String>, pub title: Option<String>, pub input: serde_json::Value,
      pub plan: Option<serde_json::Value>, pub carry: Option<serde_json::Value>, pub error: Option<serde_json::Value>,
      pub lease_until: Option<String>, pub rev: i64, pub created_at: String, pub updated_at: String }
  pub struct NewJob { pub id: String, pub client: String, pub kind: String, pub artifact_id: Option<String>,
      pub title: Option<String>, pub input: serde_json::Value, pub plan: Option<serde_json::Value> }
  pub struct JobPatch { pub if_rev: i64, pub state: Option<String>, pub step: Option<Option<String>>,
      pub title: Option<String>, pub plan: Option<Option<serde_json::Value>>, pub carry: Option<Option<serde_json::Value>>,
      pub error: Option<Option<serde_json::Value>>, pub lease_until: Option<Option<String>> }
  pub struct StepRow { pub key: String, pub result: serde_json::Value, pub version_n: Option<i64>, pub created_at: String }
  pub enum JobError { Moved(JobRow), Conflict, NotFound, Leased { until: String } }

  impl Store {
      pub fn create_job(&self, j: NewJob) -> Result<JobRow, JobError>;            // same body → Ok(existing); other body → Conflict
      pub fn open_jobs(&self, client: &str) -> Result<Vec<(JobRow, Vec<StepRow>, Vec<(String, String, bool)>, Vec<(String, Option<String>)>)>>;
      pub fn job(&self, id: &str) -> Result<Option<JobRow>>;
      pub fn patch_job(&self, id: &str, p: JobPatch, now: &str) -> Result<JobRow, JobError>; // rev+1; a lease held by another until > now → Leased
      pub fn set_job_thread(&self, id: &str, thread: &str, comment: &str, stopped: bool) -> Result<()>;
      pub fn remove_job_thread(&self, id: &str, thread: &str) -> Result<()>;
      pub fn set_job_question(&self, id: &str, question: &str, answer: Option<&str>) -> Result<()>;
      /// Inserts a non-publishing step; an existing key returns it with `replayed = true`.
      pub fn commit_step(&self, id: &str, key: &str, result: serde_json::Value) -> Result<(StepRow, bool), JobError>;
      pub fn agent_read_consents(&self) -> Result<Vec<(String, String)>>;
      pub fn put_agent_read_consent(&self, reader: &str, source: &str) -> Result<()>;
      pub fn delete_agent_read_consent(&self, reader: &str, source: &str) -> Result<bool>;
  }
  ```
  Leases: `patch_job` with `lease_until: Some(Some(t))` succeeds when the row's `lease_until` is null, in the past (`<= now`), or the patch carries the same `if_rev` the caller read (renewal by the holder); the holder is whoever last moved `rev`.
- Consumes: nothing in this plan (store level).

- [ ] **Step 1: Read the threads** on `crates/clax-store/src/migrations.rs` and `lib.rs`.
- [ ] **Step 2: Write the failing tests:** `jobs::creating_a_job_twice_with_one_body_returns_it`, `jobs::creating_with_another_body_conflicts`, `jobs::a_stale_rev_is_refused_with_the_current_job`, `jobs::a_held_lease_refuses_a_second_taker`, `jobs::an_expired_lease_is_taken`, `jobs::a_step_committed_twice_is_replayed`, `jobs::open_jobs_lists_steps_threads_and_questions`, `jobs::consents_round_trip`; in `migrations.rs`: `a_database_with_the_jobs_tables_at_another_version_is_adopted`, `a_database_whose_jobs_tables_disagree_is_refused`, `a_fresh_database_reaches_44`; and `old_homes::the_installed_flutter_app_home_backs_up_and_migrates_to_44` asserting `PRAGMA user_version` is 44 and a `clax-43-*.db` backup exists.
- [ ] **Step 3: Run** `bazelisk test //crates/clax-store:clax_store_lib_test //crates/clax-embed:all --test_arg=old_homes::` — FAIL.
- [ ] **Step 4: Implement.** Check main's migration numbers first (`git show main:crates/clax-store/src/migrations.rs | grep -E '^\s*// [0-9]+:' | tail -1`) and note in the commit message which number this took and that the merge's numbering is the owner's open decision on the spec (§10.2).
- [ ] **Step 5: Run** — PASS; also `bazelisk test //crates/clax-store:all //crates/clax-server:all //scripts:wasm32` (the Worker opens the same migrations).
- [ ] **Step 6: Commit** the listed files: "Keep the app's jobs, their steps and the agent's read consents in the store (migration 44), adopting the tables by schema".

### Task 25: Job routes, and publish and reply steps that commit with their effects

**Files:**
- Create: `crates/clax-server/src/routes/jobs.rs`, `crates/clax-server/tests/api_jobs.rs` (declared in `integration.rs`)
- Modify: `crates/clax-store/src/jobs.rs` (`commit_publish_step`, `commit_reply_step`), `crates/clax-store/src/artifacts.rs:1004-1090` (the version insert factored so it runs inside a caller's transaction: `insert_version_tx`), `crates/clax-server/src/router.rs` (routes of spec §8.1, owner only: `RequireToken` as other owner routes take it), `crates/clax-core/src/events.rs:12` (`Event::Job { id, state, step, rev }`), `crates/clax-realtime/src/stream.rs` (topic `jobs`, owner only), `docs/contract.md` (a "Jobs" section under "The Clax app": the routes, the step bodies, the errors `version_moved`, `doc_moved`, `job_moved`, `not_rendered_clean`)

**Interfaces:**
- Produces (store):
  ```rust
  pub struct PublishStep { pub artifact_id: Option<String>, pub base: i64,
      pub request: clax_core::publish::ValidatedPublish, pub docs: Vec<clax_core::docs::DocWrite>,
      pub draft: Option<DraftRef> }
  pub struct DraftRef { pub id: String, pub digest: String }
  pub struct ReplyStep { pub session_id: String, pub threads: Vec<String>, pub body: String, pub resolve: bool, pub ack: Vec<String> }
  impl Store {
      /// One IMMEDIATE transaction: replay if the key exists; else check the
      /// base, insert the version (or the artifact), apply the pinned doc
      /// writes, insert the step with version_n, bump the job. Content files
      /// are written before, as publish_version writes them.
      pub fn commit_publish_step(&self, job: &str, key: &str, s: PublishStep) -> Result<(StepRow, bool), StepError>;
      pub fn commit_reply_step(&self, job: &str, key: &str, s: ReplyStep) -> Result<(StepRow, bool), StepError>;
  }
  pub enum StepError { VersionMoved { current: i64 }, DocMoved { path: String }, NotRenderedClean, Job(JobError), Store(clax_core::CoreError) }
  ```
  (Use the existing type names for document writes and validated publishes: `grep -n "pub struct" crates/clax-core/src/publish.rs crates/clax-core/src/docs*.rs`.)
  Routes answer: `POST /api/jobs/{id}/steps/{key}` → `200 {"step": {...}, "replayed": bool, "version"?: n}`; `409 {"error":{"code":"version_moved","current":n}}` etc.
  A **failure point for tests**: `Store::fail_after_version_insert_for_tests(bool)` is not allowed (no test-only features). Instead `commit_publish_step` takes its transaction body from a private function `publish_step_tx(tx, …) -> Result<…>` and the unit test calls it inside a transaction it then rolls back, asserting no version and no step remain.
- Consumes: Task 24.

- [ ] **Step 1: Write the failing tests** in `api_jobs.rs`:
  - `a_replayed_publish_step_publishes_once`: create a job, commit `publish:0` for a new app twice; one artifact, one version; the second answers `replayed: true` with the same `version`.
  - `a_step_on_a_moved_base_records_nothing`: publish v2 outside the job, then the step with base 1 → 409 `version_moved` `current: 2`; `GET /api/jobs/{id}` has no `publish:1` step.
  - `a_moved_document_refuses_the_whole_step`: a doc write pinned to a version the doc moved past → 409 `doc_moved`; no new version.
  - `a_reply_step_replies_resolves_and_acks_once`.
  - `jobs_are_owner_only`: an agent credential and a viewer get 403/404.
  - `the_jobs_topic_hears_each_change` over `/api/stream`.
  And in `jobs.rs` unit tests: `a_step_cut_before_commit_leaves_no_version_and_resumes_once` (the rolled-back transaction above, then the real commit succeeds once).
- [ ] **Step 2: Run** `bazelisk test //crates/clax-server:all --test_arg=api_jobs:: //crates/clax-store:clax_store_lib_test` — FAIL.
- [ ] **Step 3: Implement.** **Step 4: Run** — PASS, plus `bazelisk test //crates/clax-server:all //scripts:wasm32`.
- [ ] **Step 5: Commit** the listed files: "Commit a job's publish and reply steps in one transaction with what they publish, and serve the app's jobs".

### Task 26: The icon rules in the core, shared with Dart

**Files:**
- Create: `crates/clax-core/src/icon.rs`, `crates/clax-core/tests/fixtures/icons/` (`ok-*.svg`, `bad-*.svg`, `expect.json` mapping each file to `null` or the problem's code), `apps/clax_app/test/agent/icon_fixtures_test.dart`
- Modify: `crates/clax-core/Cargo.toml` (`roxmltree = "0.20"`), `Cargo.lock`, `crates/clax-core/src/lib.rs`, `crates/clax-core/src/publish.rs:260` (`validate` runs `icon::problem` on an `icon.svg` file and refuses with `CoreError::InvalidIcon { rule }` → 400 `invalid_icon`), `crates/clax-http/src/error.rs` (the mapping; find where `CoreError` maps to codes), `apps/clax_app/lib/agent/icon_svg.dart` (doc comment: the core is authoritative; the codes match), `apps/clax_app/BUILD.bazel` (the Dart test's `data` gains the fixtures), `docs/contract.md` ("Page contract": the icon rules), `plugins/*/skills/clax/SKILL.md` (by `scripts/sync-skill-tools.py` if it generates the section; else by hand, word for word as `scripts/test-plugins.sh` checks), `apps/clax_app/assets/prompt/*` (by `scripts/sync-app-prompt.sh`), `bazel/workspace.bzl`, `MODULE.bazel.lock`

**Interfaces:**
- Produces:
  ```rust
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub enum IconProblem { TooLarge, NotXml, Doctype, NotSvg, ViewBox, Element(String), EventAttribute(String),
      ExternalHref(String), Filter, ExternalUrl(String), UseTooMany, UseTooDeep }
  impl IconProblem { pub fn code(&self) -> &'static str; /* "too_large", "not_xml", … */ }
  pub const MAX_ICON_BYTES: usize = 32 * 1024;
  pub const ICON_ELEMENTS: &[&str] = &[/* exactly iconElements of icon_svg.dart */];
  pub fn problem(svg: &[u8]) -> Option<IconProblem>;
  ```
  Dart's `iconProblem` returns the same codes (add `code` to its result type if it returns text today).
- Consumes: nothing.

- [ ] **Step 1: Write the fixtures** (at least one `bad-` file per `IconProblem` variant and four `ok-` files, among them the home app's own `apps/clax_app/home/icon.svg` if it has one) and `expect.json`.
- [ ] **Step 2: Write the tests:** Rust `icon::tests::every_fixture_gets_its_expected_answer` (reads the directory with `include_dir`-free `std::fs` from `CARGO_MANIFEST_DIR`/runfiles, as other clax-core tests read fixtures), `publish::tests::a_bad_icon_refuses_the_publish`; an MCP integration test `crates/clax-server/tests/mcp_publish.rs` case `an_outside_agents_bad_icon_is_refused` (publish over MCP with `icon.svg` containing `<script>`: error `invalid_icon`); Dart `icon_fixtures_test.dart` (every fixture → `iconProblem(...)?.code` equals `expect.json`).
- [ ] **Step 3: Run** them — FAIL. **Step 4: Implement** the port, rule for rule from `icon_svg.dart` (read it whole first). **Step 5: Run** — PASS; `bazelisk test //scripts:plugins //apps/clax_app:prompt_sync //scripts:wasm32` — PASS.
- [ ] **Step 6: Commit** the listed files: "Check icon.svg in the core's publish for every client, with fixtures the app's early check shares".

### Task 27: The render gate in the core

**Files:**
- Create: `crates/clax-core/src/digest.rs`, `apps/clax_app/test/bundle/files_digest_test.dart`
- Modify: `crates/clax-store/src/jobs.rs` (`record_render`, `clean_render`; the `draft_renders` table is Task 24's), `crates/clax-server/src/routes/app_drafts.rs` (`POST /api/drafts/{id}/renders` `{digest, errors, failed_tests}`), `crates/clax-store/src/jobs.rs` (`commit_publish_step` refuses `NotRenderedClean` unless a clean row exists for `draft.digest` and the request's files digest to it), `apps/clax_app/lib/bundle/content_hash.dart` (doc comment: matches `clax_core::digest::files_digest`), `docs/contract.md` (Jobs: `not_rendered_clean`; Drafts: the renders route)

**Interfaces:**
- Produces:
  ```rust
  /// SHA-256 over the files but icon.svg, in path order, each as
  /// "<path>\0<len>\0<bytes>": the digest content_hash.dart computes.
  pub fn files_digest<'a>(files: impl IntoIterator<Item = (&'a str, &'a [u8])>) -> String;
  ```
- Consumes: Tasks 24, 25.

- [ ] **Step 1: Read `content_hash.dart`** and write `digest.rs`'s test with a vector computed by it (three files and an `icon.svg`): the same hex. Write `files_digest_test.dart` with the same vector.
- [ ] **Step 2: Write** `api_jobs::a_draft_publish_needs_a_clean_render_of_its_digest` (no render → 409; a render with errors → 409; a clean render of another digest → 409; a clean render of this digest → 200) and `api_jobs::a_publish_without_a_draft_has_no_render_gate`.
- [ ] **Step 3: Run** — FAIL. **Step 4: Implement.** **Step 5: Run** — PASS.
- [ ] **Step 6: Commit** the listed files: "Refuse a draft's publish step unless the app recorded a clean render of exactly its files".

### Task 28: The worker on the core's jobs, and kill-and-resume

**Files:**
- Create: `apps/clax_app/lib/queue/core_jobs.dart`, `apps/clax_app/test/queue/core_jobs_test.dart`, `apps/clax_app/integration_test/kill_resume_test.dart`
- Modify: `apps/clax_app/lib/queue/runner.dart` (`CommitStep`/`PatchJob` go to `core_jobs.dart`; `JsonJobStore` stays only for Task 29's import), `apps/clax_app/lib/queue/worker_main.dart` (loads `GET /api/jobs?open=1&client=app`; takes leases, renews each minute on the worker's clock), `apps/clax_app/lib/agent/tools/finish.dart:824-867` (the publish, doc batch and reply/resolve/ack become the `publish:<base>` and `reply` step bodies; the ledger record and `docs_done` go), `apps/clax_app/lib/agent/tools/render_tool.dart` (posts each render's result to `/api/drafts/{id}/renders` with the draft's digest), `apps/clax_app/lib/ui/read_consent_dialog.dart:51` (`FileConsentMemory` → `CoreConsentMemory` over `/api/agent-read-consents`), `apps/clax_app/integration_test/rig.dart` (a launcher that runs the debug app as a child process by its PID, for the kill)

**Interfaces:**
- Produces: `class CoreJobs { Future<StepOutcome> commit(JobId id, String key, Map<String, Object?> body); Future<Job> patch(JobId id, int ifRev, Map<String, Object?> patch); Future<List<Job>> open(); }` mapping 409 codes to `StepRefused`.
- Consumes: Tasks 20, 24, 25, 27.

- [ ] **Step 1: Read the threads** on `finish.dart` (records 76578be7, f487fab9 are open on it: the conflict rule of 76578be7's decision stays: a moved document fails the step and the change offers Retry).
- [ ] **Step 2: Write `core_jobs_test.dart`** over `FakeCore`: each status maps to the right message; a replayed step is `StepCommitted(replayed: true)`.
- [ ] **Step 3: Write `kill_resume_test.dart`** (macOS, `just app-integration`): with a scripted agent (`CLAX_SCRIPTED_AGENT`), launch the app as a child (`Process.start` of the debug app's executable with `CLAX_APP_DIR` scratch and `CLAX_TEST_KILL_AT=after-publish-commit`, a debug-only hook in `core_jobs.dart` that writes `<support>/kill-point` and blocks after that step's answer), wait for the file (with a 120 s cap), `Process.killPid(pid, ProcessSignal.sigkill)`, relaunch without the hook, wait for the job `done` in `GET /api/jobs/{id}` read by a `CoreClient` the test opens on the same home after the app quits; assert one version and one agent reply. A second case with `before-publish-commit` asserts one version after resume.
- [ ] **Step 4: Run** the unit test — FAIL; implement; PASS. Run `bazelisk test //apps/clax_app:integration_kill_resume` in the background — PASS.
- [ ] **Step 5: Commit** the listed files: "Run the queue's steps on the core's jobs, so a job killed mid-publish resumes once".

## Phase 6: The JSON state, imported once (spec §9)

### Task 29: The one-time import

**Files:**
- Create: `apps/clax_app/lib/core/json_import.dart`, `apps/clax_app/test/core/json_import_test.dart`, `apps/clax_app/test/core/fixtures/json-state/` (one real-shaped file of each of the seven, from today's writers' formats in `pending_make.dart`, `ledger.dart`, `plan.dart`, `asked.dart`, `stops.dart`, `unfinished.dart`, `read_consent_dialog.dart`), `crates/clax-server/tests/api_client_imports.rs`
- Modify: `crates/clax-server/src/routes/jobs.rs` (`POST /api/client-imports/{key}` `{jobs: [NewJob + steps + threads + questions], consents}`: one transaction with the `client_imports` row; an existing key answers 200 `{"already": true}` and changes nothing), `crates/clax-store/src/jobs.rs` (`import(key, …)`), `apps/clax_app/lib/queue/worker_main.dart` (runs the import before loading jobs)

**Interfaces:**
- Produces: `Future<ImportSummary> importJsonState({required String supportDir, required CoreCalls core, required Clock clock})`; `ImportSummary {int jobs; int consents; List<String> skipped; String? copiedTo; bool already;}`. Job IDs `json-<sha256 of the entry's canonical JSON, first 24 hex>`.
- Consumes: Tasks 24, 25, 28.

- [ ] **Step 1: Write `api_client_imports.rs`**: an import applies every job, step and consent at once; a second import with the same key changes nothing; an import with one bad job (unknown artifact for a publish step) applies nothing.
- [ ] **Step 2: Write `json_import_test.dart`** with the fixtures and `FakeCore` recording the import body: each file maps per spec §9's table (one test per file: e.g. a ledger record whose version `base + 1` exists and names its `addresses` becomes a job with a `publish:<base>` step; one whose version is missing becomes a job with no step); the files are copied to `imported-json/<time>/` then deleted; a malformed `plans.json` is skipped and named; a second run after the key exists does nothing; a crash between the core's commit and the deletes (the fake throws on delete) leaves a second run harmless (the core answers `already`, the files are deleted then).
- [ ] **Step 3: Run** — FAIL. **Step 4: Implement.** **Step 5: Run** — PASS.
- [ ] **Step 6: Try it on a copy**: copy the Mac app's real support directory's seven files (read-only, from `~/Library/Containers/dev.empathic.clax/Data/Library/Application Support/dev.empathic.clax/`) into a scratch `CLAX_APP_DIR` with a copy of its `clax-home`, run the debug app on it, and check the summary in the log; never write to the original.
- [ ] **Step 7: Commit** the listed files: "Import the app's seven JSON state files into the core's tables once, keeping a copy".

## Phase 7: Delete the old code, words, and the devices

### Task 30: Delete the Dart daemon code

**Files:**
- Delete: `apps/clax_app/lib/daemon/{supervisor,health,ports,process,in_process,exe_locator,sample_key,daemon_key}.dart`, `apps/clax_app/lib/ui/daemon_cookie.dart`, `apps/clax_app/lib/clax/shared_stream.dart`, `apps/clax_app/test/daemon/`, `apps/clax_app/test/ui/daemon_cookie_test.dart`, `apps/clax_app/test/support/fake_supervisor.dart`, `apps/clax_app/test/support/real_daemon.dart`, `apps/clax_app/integration_test/locked_daemon_test.dart`, `apps/clax_app/lib/platform/pending_make.dart`, `apps/clax_app/lib/agent/ledger.dart`, `apps/clax_app/lib/agent/stops.dart` (their readers' code needed by the import moves into private functions of `lib/core/json_import.dart` first)
- Move: `apps/clax_app/lib/daemon/clock.dart` → `apps/clax_app/lib/platform/clock.dart` (27 importers updated)
- Modify: `apps/clax_app/lib/clax/http_api.dart` (`LoopbackTransport` and `HttpClaxApi.loopback` removed), `apps/clax_app/lib/clax/sse.dart` (the HTTP `reconnectingSse` removed; the parser stays), `apps/clax_app/lib/agent/unfinished.dart`, `apps/clax_app/lib/agent/asked.dart`, `apps/clax_app/lib/agent/plan.dart` (file stores removed; the data types stay), `apps/clax_app/BUILD.bazel` (`SUITES` without `daemon`; `TEST_SUPPORT` without the deleted files; the real suite without `//crates/clax-cli:clax`), every test still importing a deleted file (`test/clax/http_api_real_test.dart` runs against the core through `CoreTransport` instead)

- [ ] **Step 1: Delete and fix imports** until `bazelisk test //apps/clax_app:dart_analyze` passes.
- [ ] **Step 2: Run** `bazelisk test //apps/clax_app:all` in the background — PASS.
- [ ] **Step 3: Commit** the listed paths: "Remove the daemon supervisor, its HTTP transport and the JSON state stores from the app".

### Task 31: Delete clax-ios and the native and build leftovers; update the docs

**Files:**
- Delete: `crates/clax-ios/`, `apps/clax_app/ios/Daemon/ClaxDaemon.swift`, `apps/clax_app/android/app/src/main/res/xml/network_security_config.xml`, `apps/clax_app/macos/Runner/ClaxHelper.entitlements`, `bazel/android/clax.bzl` (if nothing but the removed targets loads it), `//crates/clax-cli:clax_android_arm64` and `:clax_android_x86_64` in `crates/clax-cli/BUILD.bazel:51-66`
- Modify: `Cargo.toml` (members), `Cargo.lock`, `bazel/workspace.bzl`, `MODULE.bazel.lock` (`just bazel-sync`), `scripts/check-crate-layers.py` (`clax-ios` out of `ALLOWED`), `scripts/check-no-unsafe.sh` and `scripts/test-check-no-unsafe.sh` (clax-ios's rule and cases out), `AGENTS.md:47` (the clax-ios line removed; spec §14's line stays), `apps/clax_app/ios/Runner/NativeChannel.swift` (`daemon*` methods out), `apps/clax_app/ios/Runner/Info.plist:46` (ATS loopback exceptions out), `apps/clax_app/android/app/src/main/AndroidManifest.xml` (`networkSecurityConfig` out), `MainActivity.kt:80` (`nativeLibraryDir` out), `bazel/flutter/ios.bzl` (`ios_daemon` out), `bazel/flutter/macos.bzl` (helper signing out if unused), `scripts/ios_spawns` check's list (`//scripts:ios_spawns`: the core's spawn sites, unchanged in meaning), `docs/bazel.md` ("The Clax app": rows and prose for clax-ios, `clax_android_*`, the helper; the iOS paragraph), `docs/usage.md` ("The Clax app" first paragraph, "On an Android phone"'s port paragraph, "Clax on iPhone and iPad" first paragraph, `app-clax` recipe, the app's records list: `clax-home/` is the core's home, no `daemon-port`), `docs/contract.md` ("The Clax app" first paragraph, "Security model"'s key-cookie part: the scheme and channel rules of spec §4.3), the superseded documents' first lines (spec §13.2: `docs/superpowers/specs/2026-10-12-flutter-app-design.md`, `2026-10-12-flutter-app-round2-design.md`, `2026-10-18-ios-release-design.md`, one line each: "Parts superseded by 2026-10-19-in-process-core-design.md: §…")

- [ ] **Step 1: Read the threads** on each modified file; records open on deleted files are listed in the report for the owner (propose `obsolete` closes, close none).
- [ ] **Step 2: Delete and edit.** Run `just bazel-sync`, then `bazelisk test //scripts:all //crates/... //apps/clax_app:apk_test //apps/clax_app:macos_app_test //apps/clax_app:ios_app_test` in the background — PASS.
- [ ] **Step 3: Commit** the listed paths: "Remove the in-process iOS daemon, the Android and Mac executables' leftovers and their unsafe exception, and document the app's core".

### Task 32: Words: Claude, not connection

**Files:**
- Modify: `apps/clax_app/home/home.js:267-279` (`MAKE_STATUS.offline` → "Can't reach Claude"; the spoken text "Waiting for Claude"), `apps/clax_app/lib/ui/launcher.dart:730`, `apps/clax_app/lib/ui/split/sidebar.dart:428`, `apps/clax_app/lib/ui/making_screen.dart:105`, `apps/clax_app/lib/ui/change_tracker.dart:256`, `apps/clax_app/lib/ui/thread_detail.dart:917`, `apps/clax_app/lib/ui/working_chip.dart` (the waiting label), the queue's notices (`lib/queue/client.dart`: "Paused — open Clax to keep building", "Open Clax to keep building <title>", "Clax's builder stopped. Restart Clax to go on."), `docs/usage.md` ("No connection" paragraph), and the tests asserting the old words (`grep -rn "Waiting for connection\|'Offline'" apps/clax_app/test apps/clax_app/home`)

- [ ] **Step 1: Read the threads** (3f5fb92f and 2713e7df are on these lines).
- [ ] **Step 2: Change the tests' expected words**, run them — FAIL; change the words; run — PASS (`bazelisk test //apps/clax_app:all` and the home app's tests, `bazelisk test //apps/clax_app:home_tests` or whatever `grep -n "home" apps/clax_app/BUILD.bazel` names).
- [ ] **Step 3: Look at it**: a make with the network off (macOS: turn Wi-Fi off) shows "Can't reach Claude" on its tile and "Waiting for Claude" in its build view; screenshots opened with Read.
- [ ] **Step 4: Commit** the listed files: "Name Claude as what is unreachable, and show no connection state for Clax's own work".
- [ ] **Step 5: Propose closes** of 3f5fb92f and 2713e7df in the report.

### Task 33: The owner's homes, the gates, the record

**Files:** none in the repository but `.qual` files and `docs/superpowers/verification/2026-10-19-in-process-core.md` (the device checklist's results).

- [ ] **Step 1: Copy the owner's homes, read-only**, only with the owner's go-ahead in the session: the phone (`adb exec-out run-as dev.empathic.clax tar c files/ > target/owner-homes/phone.tar` — the support directory's real path as Task 18 found it) and the Mac (`cp -R` of the container's support directory into `target/owner-homes/mac/`). Never write to either original.
- [ ] **Step 2: Open each copy with the new core** on the Mac (`crates/clax-embed/tests/old_homes.rs` gains an `#[ignore]` test `an_owner_home_copy_opens` that reads `CLAX_OWNER_HOME_COPY`; run it with `--test_arg=--ignored --test_env=CLAX_OWNER_HOME_COPY=…`): backup made, migration to 44, the artifact and thread counts before (from `sqlite3` on the copy) equal after, and the import of the copy's JSON files (Task 29's function run by a Dart test pointed at the copy) reports every job.
- [ ] **Step 3: Install**, with the owner present: `just app-phone`; on the Mac, `just app-macos` and replace the app. Walk the device checklist and write the results to the verification file: launch to home (and the time, `scripts/perf-app-launch.sh --android`), open three apps, a comment and its change, a make left with Back, a kill during a make (`adb shell am kill`) and its resume, Settings › key change, the inbox live, a `.clax` import, the home screen's Undo.
- [ ] **Step 4: Gates.** `just check` in the background (`target/plan-logs/core-33-check.log`); a lane that fails in code this plan did not touch is rerun alone and both results reported. Read `"$Q" exec diff main`.
- [ ] **Step 5: Records.** Re-record any of this plan's records that drifted; propose (never make) the closes listed in Tasks 16, 31 and 32; list the spec's open decisions (`"$Q" exec threads docs/superpowers/specs/2026-10-19-in-process-core-design.md --status needs-decision`).
- [ ] **Step 6: Commit** the verification file and the `.qual` changes separately: "Record the in-process core's checks on the owner's devices".
