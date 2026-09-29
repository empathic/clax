# Artifax Phase 2: Debts, Sessions, MCP, and Harness Plugins — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Any agent in Claude Code, Codex, or Pi can publish, read, list, and update artifacts through MCP with a stable session identity, and the phase 1 debts that would compound under many callers are cleared first.

**Architecture:** Phase 1's daemon gains a session table, a request-timeout layer, and off-runtime store calls. A new `artifax-mcp` crate holds one `ArtifaxTools` implementation (rmcp `tool_router`) that calls the daemon over HTTP; it is served both as the stdio shim (`artifax mcp --agent <x>`, one per harness session) and as the daemon's `/mcp` streamable-HTTP endpoint. A new `artifax-hooks` crate parses Claude-format hook JSON (Codex uses the same format) for `artifax hook`. Three plugin directories package it: `plugins/claude-code` (marketplace at `.claude-plugin/marketplace.json`), `plugins/artifax` for Codex (marketplace at `.agents/plugins/marketplace.json`, whose entries must point at `./plugins/<plugin-name>`), and `plugins/pi` (a TypeScript extension that calls the daemon directly, since Pi's extension API has no MCP registration).

**Tech Stack:** phase 1 stack plus `rmcp` 3.5 (`server`, `transport-io`, `transport-streamable-http-server`, dev: `client`, `transport-child-process`), `schemars`, `reqwest` async, `tower-http` `timeout`, `oxlint` 1.x for the web lint gate, `@mariozechner/pi-coding-agent` 0.73 types for the Pi extension.

**Spec:** `docs/superpowers/specs/2026-09-28-artifax-design.md` §3, §4, §6 (Sessions), §7, §11, §12, §13, §14, §15, §16, §17 "Phase 2", §18.

## Pre-flight results (verified on this machine, 2026-09-28)

- **Codex plugin manifest** (`~/.codex/skills/.system/plugin-creator/references/plugin-json-spec.md` and the 0.158 binary): `.codex-plugin/plugin.json` accepts `name`, `version` (strict semver), `description`, `author.name`, `skills` (path), `mcpServers` (path to `./.mcp.json` or inline object), `interface` (`displayName`, `shortDescription`, `longDescription`, `developerName`, `category`, `capabilities`, `defaultPrompt` required). Hooks: the binary discovers `hooks/hooks.json` in plugins and `.codex/hooks.json`, in Claude Code's format (events `SessionStart`, `SessionEnd`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `Stop`, `SubagentStart`, `SubagentStop`, `PreCompact`, `PostCompact`, `PermissionRequest`, `Interrupt`; output wire with `decision`, `reason`, `hookSpecificOutput`, `stop_hook_active`, `additionalContext`, `continue`, `stopReason`), gated by `features.hooks` and per-hook trust. The manifest validator rejects a top-level `hooks` key, so hooks ship as `hooks/hooks.json` discovered by convention, not declared.
- **Codex marketplace**: `<repo>/.agents/plugins/marketplace.json` with `{name, interface.displayName, plugins: [{name, source: {source: "local", path: "./plugins/<name>"}, policy: {installation: "AVAILABLE", authentication: "ON_INSTALL"}, category}]}`. Install: `codex plugin marketplace add <repo-root>` then `codex plugin add artifax@<marketplace-name>`. `CODEX_HOME` selects the Codex home directory, so tests use a scratch home.
- **Codex MCP server environment**: only `PATH` and `PWD`; no session or thread ID. The parent-PID join with the `SessionStart` hook is the primary mechanism for Codex.
- **Claude Code MCP server environment**: `CLAUDE_CODE_SESSION_ID`, `CLAUDE_PID`, `CLAUDE_PROJECT_DIR`, `CLAUDE_CODE_ENTRYPOINT` are set. The shim registers with the session ID directly; the parent-PID join is the fallback. `CLAUDE_CODE_MESSAGING_SOCKET` and `CLAUDE_CODE_MESSAGING_TOKEN` also exist (undocumented); a phase 3 pre-flight item.
- **Pi 0.73.1** (installed in the scratchpad, runs): `ExtensionAPI.on("session_start" | "tool_call" | "tool_result" | ...)`, `registerTool({name, label, description, parameters: TypeBox schema, execute(toolCallId, params, signal, onUpdate, ctx)})`, `registerCommand`, `sendUserMessage(content, {deliverAs})` which always triggers a turn when the agent is idle, `ctx.sessionManager.getSessionId()`, `ctx.cwd`, `ctx.hasUI`. No MCP registration in the extension API. `pi install /absolute/path` installs a local package; `pi -e <path>` loads one for a single run. Settings at `~/.pi/agent/settings.json`; `PI_CODING_AGENT_DIR` is not confirmed, so tests pass `--session-dir`/`-e` and never touch the real home.

## Global Constraints

- All phase 1 constraints hold (edition 2024, clippy `-D warnings`, `cargo fmt`, `ARTIFAX_HOME` in every test, JSON error shape, token gate, "ID" spelling, doc comments describe contracts).
- Tool names are exactly `publish`, `read`, `list`, `delete`, `open`, `pin`, `unpin`, `asset_upload`, `status`. Every tool result is a JSON object rendered as text; URLs are browser URLs (`http://localhost:<port>/a/<id>`).
- Every tool result ends with a feedback block slot: the JSON object carries `"feedback": []` in phase 2 so phase 3 can fill it without changing shapes.
- Hooks finish within 5 s and exit 0 with empty stdout when the daemon is unreachable.
- Store calls from async handlers run on the blocking pool; API routes have a 30 s timeout, publish routes 120 s; `/api/events`, `/mcp`, and streaming blob/file responses are exempt.
- No test starts Claude Code, Codex, or Pi against the person's real configuration: Codex tests set `CODEX_HOME`, Claude tests use `--mcp-config` with `--strict-mcp-config` and a scratch cwd, Pi tests use a scratch settings directory.
- Commits: `git commit --no-gpg-sign` (signing agent unavailable); commit messages describe the change.

## Review Focus

1. A shim whose daemon dies mid-session must return a structured tool error naming the log path, not hang; pinned in Task 7.
2. A hook invoked with malformed or empty stdin must exit 0 with empty stdout within 5 s; pinned in Task 8.
3. Two shims for the same Claude Code session (a `--resume` after a crash) must not create two live session rows; pinned in Task 5 (register is idempotent on `(harness, harness_session_id)`).
4. A publish through MCP with a stale `if_version` must surface the current version in the tool error text so the agent can merge; pinned in Task 6.
5. The `/mcp` endpoint must refuse a request without the bearer token and must refuse a non-local `Host`; pinned in Task 6.
6. A `read` of a multi-megabyte page must truncate with a flag rather than flood the agent; pinned in Task 6.

---

## File structure

```
crates/artifax-core/src/store/sessions.rs        sessions table ops (new)
crates/artifax-core/src/store/migrations.rs      migration 2
crates/artifax-core/src/model.rs                 Session struct
crates/artifax-server/src/blocking.rs            AppState::store_call via spawn_blocking (new)
crates/artifax-server/src/routes/sessions.rs     session routes (new)
crates/artifax-server/src/routes/mcp.rs          /mcp mount (new)
crates/artifax-server/src/routes/mod.rs          timeout layers, new routes
crates/artifax-server/src/wrap_cache.rs          per-version wrap cache (new)
crates/artifax-mcp/                              new crate: ArtifaxTools, DaemonClient (async), stdio shim runner
crates/artifax-hooks/                            new crate: hook input/output types, per-event handlers
crates/artifax-cli/src/commands/{mcp,hook}.rs    new subcommands
plugins/claude-code/                             Claude Code plugin
plugins/artifax/                                 Codex plugin (name-pinned directory)
plugins/pi/                                      Pi extension package
.claude-plugin/marketplace.json, .agents/plugins/marketplace.json
scripts/ensure-artifax.sh, scripts/test-ensure-artifax.sh
.github/workflows/release.yml
docs/contract.md
web/.oxlintrc.json, web/package.json (lint script)
```

---

### Task 1: Store calls off the runtime, request timeouts, and a wrap cache

**Files:**
- Create: `crates/artifax-server/src/blocking.rs`, `crates/artifax-server/src/wrap_cache.rs`, `crates/artifax-server/tests/api_timeout.rs`
- Modify: `crates/artifax-server/src/state.rs`, `src/routes/mod.rs`, every route module that calls `s.store.*` (`artifacts.rs`, `assets.rs`, `content.rs`), `src/lib.rs`, `src/daemon.rs`, `tests/common/mod.rs`, `Cargo.toml` (server)

**Interfaces:**
- Produces: `AppState::store_call<T: Send + 'static>(&self, f: impl FnOnce(&Store) -> artifax_core::Result<T> + Send + 'static) -> Result<T, ApiError>` in `blocking.rs`: runs `f` on `tokio::task::spawn_blocking` with a clone of the `Arc<Store>`, maps `JoinError` to 500 `internal`. Every handler replaces `s.store.x(...)?` with `s.store_call(move |st| st.x(...)).await?`.
- `AppState.request_timeout: Duration` (default 30 s) and `publish_timeout: Duration` (120 s); `routes::router` applies `tower_http::timeout::TimeoutLayer::new(request_timeout)` to the `/api` group except `/api/events` and `/api/sessions/{id}/feedback` (phase 3), and `publish_timeout` to the two publish routes and the asset upload. A timed-out request returns 408 with the JSON error `{"error":{"code":"timeout","message":"request exceeded 30s"}}` (map `tower::timeout::error::Elapsed` via `HandleErrorLayer`).
- `wrap_cache::WrapCache`: `get_or_wrap(artifact_id, n, || -> io::Result<String>) -> io::Result<Arc<String>>` bounded to 64 entries (LRU by insertion order, `VecDeque` + `HashMap`), invalidated by `remove_artifact(id)` on delete. `content::index` uses it.
- Test route, only with feature `test-routes`: `GET /api/_test/sleep/{ms}` sleeps `ms` then returns 200. The server crate's `[dev-dependencies]` includes itself with that feature: `artifax-server = { path = ".", features = ["test-routes"] }`.

- [ ] **Step 1: Write the failing tests**

`tests/api_timeout.rs`:
```rust
mod common;
use common::TestServer;

#[tokio::test]
async fn slow_api_requests_time_out_with_json_408() {
    let ts = TestServer::spawn_with(|state| { state.request_timeout = std::time::Duration::from_millis(200); }).await;
    let res = ts.get("/api/_test/sleep/1000").await;
    assert_eq!(res.status(), 408);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["error"]["code"], "timeout");
}

#[tokio::test]
async fn events_stream_is_exempt_from_the_timeout() {
    let ts = TestServer::spawn_with(|state| { state.request_timeout = std::time::Duration::from_millis(200); }).await;
    let res = ts.get("/api/events").await;
    assert_eq!(res.status(), 200);
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    let mut stream = res.bytes_stream();
    use futures::StreamExt;
    assert!(stream.next().await.is_some(), "stream still open after the API timeout would have fired");
}

#[tokio::test]
async fn store_calls_run_off_the_runtime_worker() {
    let ts = TestServer::spawn().await;
    let futs: Vec<_> = (0..16).map(|i| ts.publish(&format!("A{i}"), &[("index.html", "<p>")])).collect();
    let all = futures::future::join_all(futs).await;
    assert_eq!(all.len(), 16);
    let list: serde_json::Value = ts.get("/api/artifacts").await.json().await.unwrap();
    assert_eq!(list["artifacts"].as_array().unwrap().len(), 16);
}
```

`src/wrap_cache.rs` unit tests:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn caches_per_version_and_invalidates_on_remove() {
        let c = WrapCache::new(2);
        let mut calls = 0;
        let a = c.get_or_wrap("x", 1, || { calls += 1; Ok("A".into()) }).unwrap();
        let b = c.get_or_wrap("x", 1, || { calls += 1; Ok("B".into()) }).unwrap();
        assert_eq!(*a, "A"); assert_eq!(*b, "A"); assert_eq!(calls, 1);
        c.get_or_wrap("y", 1, || Ok("Y".into())).unwrap();
        c.get_or_wrap("z", 1, || Ok("Z".into())).unwrap();
        let again = c.get_or_wrap("x", 1, || { calls += 1; Ok("A2".into()) }).unwrap();
        assert_eq!(*again, "A2", "evicted after capacity");
        c.remove_artifact("y");
        let y = c.get_or_wrap("y", 1, || Ok("Y2".into())).unwrap();
        assert_eq!(*y, "Y2");
    }
}
```

`tests/common/mod.rs` gains `TestServer::spawn_with(f: impl FnOnce(&mut AppState))` that builds the state, applies `f`, then serves; `spawn()` calls it with a no-op.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p artifax-server --test api_timeout`
Expected: compile errors (`spawn_with`, `request_timeout`, sleep route missing).

- [ ] **Step 3: Implement**

`src/blocking.rs`:
```rust
//! Runs synchronous store work on the blocking pool so SQLite and file I/O never stall a runtime worker.

use crate::error::ApiError;
use crate::state::AppState;
use artifax_core::Store;
use axum::http::StatusCode;

impl AppState {
    pub async fn store_call<T, F>(&self, f: F) -> Result<T, ApiError>
    where
        T: Send + 'static,
        F: FnOnce(&Store) -> artifax_core::Result<T> + Send + 'static,
    {
        let store = self.store.clone();
        match tokio::task::spawn_blocking(move || f(&store)).await {
            Ok(r) => r.map_err(ApiError::from),
            Err(e) => {
                tracing::error!(error = %e, "store task failed");
                Err(ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", "storage task failed"))
            }
        }
    }
}
```

`src/wrap_cache.rs`:
```rust
//! Bounded cache of wrapped index documents keyed by (artifact, version). Versions are immutable,
//! so an entry only becomes stale when its artifact is deleted.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

pub struct WrapCache {
    inner: Mutex<Inner>,
    capacity: usize,
}

struct Inner {
    map: HashMap<(String, u32), Arc<String>>,
    order: VecDeque<(String, u32)>,
}

impl WrapCache {
    pub fn new(capacity: usize) -> Self {
        WrapCache { inner: Mutex::new(Inner { map: HashMap::new(), order: VecDeque::new() }), capacity }
    }

    pub fn get_or_wrap(&self, artifact_id: &str, n: u32, wrap: impl FnOnce() -> std::io::Result<String>) -> std::io::Result<Arc<String>> {
        let key = (artifact_id.to_string(), n);
        if let Some(v) = self.inner.lock().unwrap().map.get(&key) {
            return Ok(v.clone());
        }
        let value = Arc::new(wrap()?);
        let mut g = self.inner.lock().unwrap();
        if g.map.len() >= self.capacity {
            if let Some(old) = g.order.pop_front() { g.map.remove(&old); }
        }
        g.order.push_back(key.clone());
        g.map.insert(key, value.clone());
        Ok(value)
    }

    pub fn remove_artifact(&self, artifact_id: &str) {
        let mut g = self.inner.lock().unwrap();
        g.map.retain(|(id, _), _| id != artifact_id);
        g.order.retain(|(id, _)| id != artifact_id);
    }
}
```

`AppState` gains `pub wrap_cache: Arc<WrapCache>` (capacity 64), `pub request_timeout: Duration`, `pub publish_timeout: Duration`. `daemon::serve` and the test harness construct them. `routes/mod.rs`:
```rust
use tower::ServiceBuilder;
use tower_http::timeout::TimeoutLayer;
use axum::error_handling::HandleErrorLayer;

async fn timeout_error(err: tower::BoxError) -> ApiError {
    if err.is::<tower::timeout::error::Elapsed>() {
        ApiError::new(StatusCode::REQUEST_TIMEOUT, "timeout", "request exceeded the time limit")
    } else {
        ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", err.to_string())
    }
}

fn with_timeout(router: Router<AppState>, d: Duration) -> Router<AppState> {
    router.layer(ServiceBuilder::new().layer(HandleErrorLayer::new(timeout_error)).layer(TimeoutLayer::new(d)))
}
```
Group the routes: `api_fast` (artifact GETs, PATCH, DELETE, versions GET, files, assets list/delete, token, sessions) under `request_timeout`; `api_slow` (the two publish routes, asset upload) under `publish_timeout`; `events`, content, blob, shell, and `/mcp` (Task 6) with no timeout. Add `#[cfg(feature = "test-routes")]` route `/api/_test/sleep/{ms}` to `api_fast`. Add `timeout` to tower-http's features in the workspace `Cargo.toml` and `[features] test-routes = []` in the server crate.

Replace every `s.store.method(args)?` in handlers with `s.store_call(move |st| st.method(args)).await?` (clone owned args before the closure). `content::index` becomes: `let html = s.wrap_cache.get_or_wrap(id.as_str(), n, || std::fs::read_to_string(&path).map(|p| wrap_document(&p, id.as_str(), n, CONTRACT_VERSION)))?` inside a `store_call` or `spawn_blocking`. `artifacts::delete` calls `s.wrap_cache.remove_artifact(id.as_str())`.

- [ ] **Step 4: Run to verify pass, then commit**

Run: `cargo test -p artifax-server && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all`

```bash
git add -A
git commit --no-gpg-sign -m "Run store work on the blocking pool, add request timeouts and a wrap cache"
```

---

### Task 2: Asset store and asset route hardening

**Files:**
- Modify: `crates/artifax-core/src/store/assets.rs`, `crates/artifax-server/src/routes/assets.rs`, `crates/artifax-server/src/routes/artifacts.rs` (export `path()` as `pub(crate)` and delete the duplicate), `crates/artifax-server/tests/api_assets.rs`

**Interfaces:**
- `add_asset` writes to `<id>.<ext>.tmp`, inserts the row, then renames; on insert failure it removes the temp file. `ext_for` overrides: `text/plain` → `txt`, `image/jpeg` → `jpg`, `text/csv` → `csv`, `application/json` → `json`, `image/svg+xml` → `svg`, `text/css` → `css`, `font/woff2` → `woff2`, `application/pdf` → `pdf`; otherwise mime_guess, else `bin`. `delete_asset` removes the file first (tolerating `NotFound`), then the row, so a retry after a file error still deletes the row. `list_assets` orders by `created_at, id`. Doc comments on every public item state the returned errors and that the cap is inclusive.
- Route: `Multipart` is taken as `Result<Multipart, MultipartRejection>` and mapped to 400 `invalid_multipart` (or 413 `body_too_large` when the rejection status is 413); `blob` sets `Content-Length` from `asset.size`.

- [ ] **Step 1: Write the failing tests** (append to `store/assets.rs` tests and `tests/api_assets.rs`)

```rust
// store/assets.rs
#[test]
fn extensions_are_sane_for_common_types() {
    for (ct, ext) in [("text/plain", "txt"), ("image/jpeg", "jpg"), ("text/csv", "csv"), ("application/json", "json"),
                      ("image/png", "png"), ("image/svg+xml", "svg"), ("application/pdf", "pdf"), ("text/css", "css"), ("font/woff2", "woff2")] {
        assert_eq!(super::ext_for(ct), ext, "{ct}");
    }
}
#[test]
fn exact_cap_is_accepted_and_no_temp_file_remains() {
    let (_d, store) = store();
    let id = store.insert_artifact_for_test("A", "2026-01-01T00:00:00.000Z");
    let bytes = vec![0u8; super::MAX_ASSET_BYTES as usize];
    let a = store.add_asset(&id, "image/png", &bytes).unwrap();
    let dir = store.home().assets_dir(&id);
    assert!(std::fs::read_dir(&dir).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().ends_with(".tmp")));
    let _ = a;
}
#[test]
fn delete_removes_row_even_if_file_is_already_gone() {
    let (_d, store) = store();
    let id = store.insert_artifact_for_test("A", "2026-01-01T00:00:00.000Z");
    let a = store.add_asset(&id, "image/png", &[1]).unwrap();
    let (_, path) = store.get_asset(&a.id).unwrap().unwrap();
    std::fs::remove_file(&path).unwrap();
    store.delete_asset(&a.id).unwrap();
    assert!(store.get_asset(&a.id).unwrap().is_none());
}
```
```rust
// tests/api_assets.rs
#[tokio::test]
async fn blob_404s_after_artifact_delete_and_cross_artifact_delete_is_404() {
    let ts = TestServer::spawn().await;
    let a = ts.publish("A", &[("index.html", "<p>")]).await; let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let b = ts.publish("B", &[("index.html", "<p>")]).await; let bid = b["artifact"]["id"].as_str().unwrap().to_string();
    let part = reqwest::multipart::Part::bytes(vec![1]).file_name("x.png").mime_str("image/png; charset=binary").unwrap();
    let res = ts.authed(ts.client.post(format!("{}/api/artifacts/{aid}/assets", ts.base)).multipart(reqwest::multipart::Form::new().part("file", part))).send().await.unwrap();
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["asset"]["ext"], "png");
    let url = body["url"].as_str().unwrap().to_string();
    let asset_id = body["asset"]["id"].as_str().unwrap().to_string();
    let res = ts.get(&url).await;
    assert_eq!(res.headers()["content-length"], "1");
    let res = ts.authed(ts.client.delete(format!("{}/api/artifacts/{bid}/assets/{asset_id}", ts.base))).send().await.unwrap();
    assert_eq!(res.status(), 404);
    assert_eq!(ts.get(&url).await.status(), 200, "cross-artifact delete did nothing");
    ts.authed(ts.client.delete(format!("{}/api/artifacts/{aid}", ts.base))).send().await.unwrap();
    let res = ts.get(&url).await;
    assert_eq!(res.status(), 404);
    assert_eq!(res.json::<serde_json::Value>().await.unwrap()["error"]["code"], "not_found");
}

#[tokio::test]
async fn non_multipart_upload_is_json_400() {
    let ts = TestServer::spawn().await;
    let a = ts.publish("A", &[("index.html", "<p>")]).await; let aid = a["artifact"]["id"].as_str().unwrap();
    let res = ts.authed(ts.client.post(format!("{}/api/artifacts/{aid}/assets", ts.base)).header("content-type", "application/json").body("{}")).send().await.unwrap();
    assert_eq!(res.status(), 400);
    assert_eq!(res.json::<serde_json::Value>().await.unwrap()["error"]["code"], "invalid_multipart");
}
```

- [ ] **Step 2: Run to verify failure**, **Step 3: implement per the interfaces**, **Step 4: run, clippy, fmt, commit**

```bash
git commit --no-gpg-sign -m "Harden the asset store: atomic writes, sane extensions, JSON multipart errors"
```

---

### Task 3: Events, store, wrap, auth, and daemon minors

**Files:**
- Modify: `crates/artifax-server/src/routes/events.rs`, `tests/api_events.rs`, `src/auth.rs`, `tests/api_auth.rs`, `src/daemon.rs`, `crates/artifax-core/src/store/{mod,artifacts}.rs`, `crates/artifax-core/src/publish.rs`, `crates/artifax-core/src/wrap.rs`, `crates/artifax-core/src/home.rs`, `crates/artifax-core/src/error.rs`

**Interfaces and behaviours (each with a test):**
- Events: a lagged receiver emits `event: resync` with `data: {"dropped": <n>}` instead of dropping silently; doc comments on `EventsQuery` and `events`; tests for the unfiltered stream, `artifact_deleted`, keep-alive comment within 20 s (use a 1 s keep-alive when `cfg!(test)`? No: make the keep-alive interval a field on `AppState`, `sse_keep_alive: Duration`, default 15 s, test sets 200 ms), host-404 on `<aid>.localhost`, and every stream read wrapped in `tokio::time::timeout(5 s)`.
- Store: `list_artifacts` and `list_versions` order with an `id`/`n` tiebreaker; corrupt `capabilities_json` or `files_json` surfaces `CoreError::Db(rusqlite::Error::FromSqlConversionFailure)` instead of an empty default; `delete_artifact` tolerates `remove_dir_all` `NotFound` and logs other errors without failing (row deletion already committed); `update_meta` doc comment states it does not bump `updated_at` and cannot clear fields; `create_artifact` performs the collision check before inserting the artifact row so a rejected publish leaves no zero-version row; carry-forward uses `fs::copy`; `content_type_for` lowercases the extension taken from `Path::extension`; base64 decoding strips ASCII whitespace first; add a `body_too_large` validation test; `Home::from_env` returns an error when neither `ARTIFAX_HOME` nor `HOME` is set (signature becomes `from_env() -> Result<Home>`; callers `?` it).
- Wrap: unterminated `<script>`/`<style>` sets the scan position to the end (no body match in the tail); the doc comment's `-->` sentence corrected; for a full document with no `<body>`, still insert after the doctype (documented as the fallback).
- Auth: `Bearer` scheme matched case-insensitively; `healthz` test asserts `started_at` is an RFC 3339 string; `is_loopback` unit tests already exist, add a `not_loopback` integration test using `spawn_with` binding to `0.0.0.0` and connecting via a non-loopback local IP if one exists (skip with a message when none).
- Daemon: `DaemonLock::acquire` retries on `EINTR`; doc comments on `DaemonInfo`, `read/write/remove_daemon_info`, `pid_alive` (EPERM semantics), `ServeConfig`.

- [ ] **Steps:** write the failing tests per bullet, run to see them fail, implement, run `cargo test --workspace`, clippy, fmt, commit as `"Clear phase 1 store, events, wrap, auth, and daemon debts"`.

---

### Task 4: CLI, web, lint gate, and spec wording debts

**Files:**
- Modify: `crates/artifax-cli/src/commands/{publish,doctor,open}.rs`, `src/main.rs`, `src/client.rs`, `tests/cli.rs`; `web/shell/src/{gallery.tsx,artifact.tsx,api.ts}`, `web/bridge/src/bridge.ts`, `web/shell/src/*.test.tsx`, `web/e2e/viewer.spec.ts`, `web/package.json`; `scripts/quality_gates.sh`; `docs/superpowers/specs/2026-09-28-artifax-design.md` (§7.4, §8, §14, §16); `README.md`
- Create: `web/.oxlintrc.json`

**Interfaces and behaviours:**
- CLI: `--file <src>=<dest>` splits on the last `=` only when the part after it contains no path separator that starts with `/`; simpler rule adopted: split on the first `=` only if the source path before it exists as a file, else treat the whole spec as a source path (test: a source named `a=b.js` publishes as `a=b.js`). Clap usage errors exit 1 (use `Cli::try_parse()`, print the clap error, exit 1; `--help` and `--version` exit 0). Discovery probes the daemon's recorded bind: unspecified (`0.0.0.0`, `::`) maps to the same-family loopback, a specific address is used as-is, IPv6 bracketed; `browser_url` uses `localhost` only for loopback binds. `doctor` gains a `stale_files` check that lists `.tmp-*` staging directories, `versions/<n>` directories above `current_version`, and zero-version artifact rows, and `doctor --fix` removes the first two and deletes the third; tests inject each condition.
- Web: `aria-label` on the search input and the pin button; the bridge wraps `Object.defineProperty` in try/catch and logs once; `artifact.tsx` detects 404 via a `status` field on the error thrown by `api.ts`'s `json()`; gallery tests poll for the DOM (`waitFor` helper with a 2 s cap) instead of fixed sleeps; `events.test.ts` restores globals in `afterEach`; `origin.test.ts` covers a non-OK probe response and storage throwing; e2e uses `page.frame({ url })` everywhere.
- Lint: `oxlint` added as a dev dependency with `web/.oxlintrc.json` (`{"$schema": "./node_modules/oxlint/configuration_schema.json", "plugins": ["typescript"], "categories": {"correctness": "error", "suspicious": "warn"}}`), `"lint": "oxlint shell bridge e2e"` in `package.json`, a `web lint` gate line in `scripts/quality_gates.sh` before typecheck, and spec §16 restored to "web lint and typecheck" with a sentence naming oxlint. Fix whatever oxlint reports.
- Spec wording: §7.4 gains "and exits when `daemon.json` is missing on two consecutive checks"; §8 and §14 state the exact header values: `/c/...` carries `Content-Security-Policy: sandbox allow-scripts allow-forms allow-modals allow-popups allow-downloads` and `/_blob/...` carries `Content-Security-Policy: sandbox`, with the note that a bare sandbox stops Chrome rendering PDFs top-level (assets are meant for `<img>`, `<video>`, `<a download>`, and fonts).
- README: mention `doctor --fix` and the lint gate.

- [ ] **Steps:** write the failing tests per bullet (CLI tests in `tests/cli.rs`, vitest cases, an oxlint run that must pass), run to see them fail, implement, run the full gate script, commit as `"Clear CLI and web debts, add the oxlint gate, and fix spec wording"`.

---

### Task 5: Sessions: storage, routes, publish attribution, reaper

**Files:**
- Create: `crates/artifax-core/src/store/sessions.rs`, `crates/artifax-server/src/routes/sessions.rs`, `crates/artifax-server/tests/api_sessions.rs`
- Modify: `crates/artifax-core/src/store/migrations.rs`, `src/model.rs`, `src/store/mod.rs`, `src/store/artifacts.rs` (session on create/publish), `crates/artifax-server/src/routes/{mod,artifacts}.rs`, `src/daemon.rs` (reaper task), `web/shell/src/gallery.tsx` (publisher label)

**Interfaces:**
- Migration 2: `sessions(id TEXT PRIMARY KEY, harness TEXT NOT NULL, harness_session_id TEXT, cwd TEXT NOT NULL, pid INTEGER, parent_pid INTEGER, started_at TEXT NOT NULL, last_seen_at TEXT NOT NULL, ended_at TEXT)` plus `CREATE UNIQUE INDEX sessions_harness_id ON sessions(harness, harness_session_id) WHERE harness_session_id IS NOT NULL AND ended_at IS NULL`.
- `model::Session { id, harness, harness_session_id: Option<String>, cwd, pid: Option<u32>, parent_pid: Option<u32>, started_at, last_seen_at, ended_at: Option<String> }`.
- `Store::register_session(RegisterSession { harness, harness_session_id: Option<String>, cwd, pid: Option<u32>, parent_pid: Option<u32> }) -> Result<Session>`: if `harness_session_id` is given and a live row with the same `(harness, harness_session_id)` exists, update its pid/parent_pid/last_seen and return it (idempotent, Review Focus 3); else if `harness_session_id` is `None` and a live row has the same `(harness, parent_pid)` with a known ID, adopt it; else insert with a new ULID.
- `Store::join_session(harness, parent_pid, harness_session_id) -> Result<Option<Session>>`: from a hook; finds a live row by `(harness, parent_pid)` with null `harness_session_id` and sets it; if none, inserts a hook-only row (pid None) so the shim can adopt it later.
- `Store::heartbeat(id)`, `Store::end_session(id)`, `Store::get_session(id)`, `Store::list_sessions(live_only: bool)`, `Store::reap_sessions(idle: Duration, pid_alive: &dyn Fn(u32) -> bool) -> Result<usize>` (ends rows with `last_seen_at` older than `idle` whose pid is None or dead).
- `create_artifact` and `publish_version` take `session_id: Option<&str>`; `create` stores it as `owner_session_id`, both store it on the version. The publish routes read `X-Artifax-Session` and pass it through after checking the session exists and is live (unknown → 400 `unknown_session`).
- Routes: `POST /api/sessions` (W) body `RegisterSession` → 201 `{"session"}`; `PATCH /api/sessions/{id}` (W) `{"heartbeat": true}` or `{"ended": true}` → `{"session"}`; `GET /api/sessions` (`?live=true`) → `{"sessions"}`; `GET /api/sessions/{id}`; `POST /api/sessions/join` (W) `{harness, parent_pid, harness_session_id}` → `{"session"}`; `GET /api/artifacts/{aid}` also returns `"owner_session": Session | null`.
- Daemon: a reaper task every 60 s calls `reap_sessions(5 min, pid_alive)`.
- Gallery: "published by <harness> session" when `owner_session_id` is set, with a green dot when the session is live (the list route embeds `owner_live: bool`).

- [ ] **Step 1: Write the failing tests** (store unit tests: idempotent register, join-then-adopt in both orders, reaper ends dead-pid rows only; route tests: register/heartbeat/end, publish with header sets owner and version session, unknown header → 400).
- [ ] **Step 2–4:** run red, implement, run green, clippy, fmt, commit `"Add sessions: registration, join by parent PID, publish attribution, reaper"`.

---

### Task 6: `artifax-mcp` crate: tools over the daemon, served on `/mcp`

**Files:**
- Create: `crates/artifax-mcp/Cargo.toml`, `src/lib.rs`, `src/client.rs` (async `DaemonClient`), `src/tools.rs` (`ArtifaxTools` with `#[tool_router]`), `src/render.rs` (result JSON + feedback slot), `tests/tools.rs`
- Create: `crates/artifax-server/src/routes/mcp.rs`; Modify: `src/routes/mod.rs`, `src/lib.rs`, `Cargo.toml` (workspace members, deps)

**Interfaces:**
- `DaemonClient::new(base: String, token: String, session_id: Option<String>) -> DaemonClient` (async reqwest, `.no_proxy()`, 30 s timeout, 120 s for publish); methods mirror the REST API: `list`, `get`, `create`, `publish_version`, `patch`, `delete`, `files`, `file_bytes(id, n, path)`, `upload_asset(id, filename, content_type, bytes)`, `healthz`. Sends `X-Artifax-Session` when set.
- `ArtifaxTools { client: DaemonClient, browser_base: String, session: Option<Session>, tool_router: ToolRouter<Self> }` with `#[tool_router] impl ArtifaxTools` defining the nine tools, each `async fn name(&self, Parameters<Args>) -> Result<CallToolResult, McpError>` where `Args` derives `schemars::JsonSchema` + `Deserialize`:
  - `publish`: `{ file_path?: String, html?: String, files?: BTreeMap<String, FileArg>, url?: String, id?: String, if_version?: u32, title?, description?, icon?, label?, capabilities?: Value }` where `FileArg` is `{ path?: String, content?: String, encoding?: "utf8"|"base64", content_type?: String }` or JSON `null` to remove. Exactly one of `file_path`/`html`; `url`/`id` selects update. Text vs binary by extension as the CLI does. Result: `{artifact_id, url, version, title, files: [..]}`.
  - `read`: `{ url_or_id: String, path?: String, version?: u32, max_bytes?: u64 (default 200_000) }` → `{artifact_id, version, path, content, truncated: bool, size}`; binary files return `{content_base64}` when under the cap.
  - `list`: `{ limit?: u32 }` → `{artifacts: [{id, url, title, version, pinned, updated_at, owner_session_id}]}`.
  - `delete`, `pin`, `unpin`: `{ url_or_id }`.
  - `open`: `{ url_or_id }` runs `open`/`xdg-open` on the daemon host (the shim runs on the same host) → `{url, opened: bool}`.
  - `asset_upload`: `{ url_or_id, file_path?: String, file_paths?: Vec<String> }` → `{assets: [{id, url, content_type, size}]}`.
  - `status`: `{}` → `{daemon_url, version, session: Session|null, harness, watches: []}`.
- Every result is `CallToolResult::success(vec![ContentBlock::text(json)])` where `json` is the pretty object with `"feedback": []` appended (Task 3 of phase 3 fills it). Errors: `CallToolResult::error(vec![ContentBlock::text(json)])` with `{error: {code, message, current?}}` so a 409 carries `current` (Review Focus 4); never a JSON-RPC error for a daemon-side failure. Daemon unreachable → `{error: {code: "daemon_unreachable", message, log: "<home>/logs/daemon.log"}}`.
- `impl ServerHandler for ArtifaxTools` via `#[tool_handler]`, `get_info` with instructions text (one paragraph: what Artifax is, the page contract pointer, that URLs are for the person).
- Server: `routes/mcp.rs` mounts `StreamableHttpService::new(move || Ok(ArtifaxTools::new(...)), Arc::new(LocalSessionManager::default()), StreamableHttpServerConfig::default().with_allowed_hosts(["localhost", "127.0.0.1", "::1"]))` at `/mcp`, wrapped in a `RequireToken` middleware (`axum::middleware::from_fn_with_state` checking the bearer) so Review Focus 5 holds; `ArtifaxTools` on the daemon side is constructed with a `DaemonClient` pointing at itself (loopback) using its own token, `session: None`.
- Tests (`crates/artifax-mcp/tests/tools.rs`): start a `TestServer` (re-export the phase 1 harness as a `pub mod testing` behind a `test-support` feature of `artifax-server`, or copy the 40 lines), construct `ArtifaxTools` against it, and call the tool methods directly: publish → read round trip; stale `if_version` → error result containing `"current": 1`; `read` of a 1 MB page → `truncated: true` with `content.len() == 200_000`; `list` order; unreachable daemon (bogus port) → `daemon_unreachable` naming the log path. Server test (`tests/api_mcp.rs`): `POST /mcp` `initialize` without token → 401; with token and `Host: evil.com` → 4xx from rmcp's host check; with token and `tools/list` → nine tool names.

- [ ] **Steps:** write the failing tests, run red, implement (`cargo add` in the new crate: rmcp with `server`, `transport-io`, `transport-streamable-http-server`, `macros`; `schemars`; `reqwest`; `serde`; `serde_json`; `tokio`; `base64`; `anyhow`), run green, clippy, fmt, commit `"Add the artifax-mcp tool set and serve it on /mcp"`.

---

### Task 7: The stdio shim: `artifax mcp --agent <harness>`

**Files:**
- Create: `crates/artifax-mcp/src/shim.rs`, `crates/artifax-cli/src/commands/mcp.rs`, `crates/artifax-mcp/tests/shim.rs`
- Modify: `crates/artifax-cli/src/main.rs`, `Cargo.toml` (cli deps on artifax-mcp; mcp dev-deps rmcp `client` + `transport-child-process`)

**Interfaces:**
- `shim::run(harness: Harness, home: &Home) -> anyhow::Result<()>`: (1) `Client::connect` (blocking, in `spawn_blocking`) to ensure the daemon; (2) collect identity: `harness_session_id` from `CLAUDE_CODE_SESSION_ID` when harness is claude, else `ARTIFAX_SESSION_ID` env if set, else `None`; `cwd`, `pid`, `parent_pid` (`libc::getppid`); (3) `POST /api/sessions` to register; (4) build `ArtifaxTools` with the session; (5) serve over `rmcp::transport::stdio()` and await; (6) heartbeat every 60 s on a background task; (7) on transport close (stdin EOF), `PATCH /api/sessions/{id}` `{"ended": true}` with a 3 s timeout, then exit 0. Logging to stderr only (stdout is the protocol channel); `RUST_LOG` respected.
- CLI: `artifax mcp --agent <claude|codex|pi>` (default `claude`).
- Tests: spawn the built binary with `TokioChildProcess` (env `ARTIFAX_HOME` temp, `CLAUDE_CODE_SESSION_ID=test-sess`), use `rmcp`'s client `serve` to `list_tools` (nine names), `call_tool("publish", ...)` then `call_tool("read", ...)`, check `GET /api/sessions?live=true` shows one session with `harness_session_id == "test-sess"`; drop the client (closes stdin) and poll `/api/sessions/{id}` until `ended_at` is set (5 s cap); Review Focus 1: start a second shim, stop the daemon with `artifax stop`, call a tool, assert an error result with code `daemon_unreachable` within 10 s.

- [ ] **Steps:** failing tests, red, implement, green, clippy, fmt, commit `"Add the stdio MCP shim with session registration and heartbeat"`.

---

### Task 8: `artifax-hooks` crate and `artifax hook`

**Files:**
- Create: `crates/artifax-hooks/Cargo.toml`, `src/lib.rs`, `src/input.rs`, `src/output.rs`, `src/events.rs`, `tests/fixtures/{claude,codex}-{session-start,session-end,malformed}.json`, `tests/golden.rs`; `crates/artifax-cli/src/commands/hook.rs`
- Modify: `crates/artifax-cli/src/main.rs`, `Cargo.toml`

**Interfaces:**
- `input::HookInput { session_id: Option<String>, cwd: Option<String>, hook_event_name: Option<String>, transcript_path: Option<String>, stop_hook_active: Option<bool>, #[serde(flatten)] rest: Map }` parsed leniently from stdin (`serde_json::from_str`; on parse error → `HookInput::default()`).
- `output::HookOutput` builders: `none()` (print nothing), `additional_context(event, text)` → `{"hookSpecificOutput": {"hookEventName": <event>, "additionalContext": text}}`, `block(reason)` → `{"decision": "block", "reason": reason}` (phase 3).
- `events::session_start(harness, input, client) -> HookOutput`: joins the session by `(harness, getppid(), session_id)` via `POST /api/sessions/join`; returns `additional_context("SessionStart", "Artifax daemon at <url>; artifacts publish with the `publish` tool.")` for claude and codex.
- `events::session_end(harness, input, client)`: looks up the live session by `(harness, session_id)` and ends it.
- CLI: `artifax hook --agent <claude|codex> <session-start|session-end>`; the whole command is wrapped in a 4 s deadline (`std::thread::spawn` + `recv_timeout`); any failure or timeout → exit 0 with empty stdout (Review Focus 2); daemon discovery only, never auto-start (a hook must not spawn a daemon).
- Fixtures: captured shapes for Claude Code (`{"session_id": "...", "transcript_path": "...", "cwd": "...", "hook_event_name": "SessionStart", "source": "startup"}`) and Codex (same keys; the pre-flight showed the same wire); malformed (`{nope`) and empty.
- Tests: golden stdout per fixture against a test daemon (session-start prints the context JSON with the daemon URL and creates/joins a session; session-end ends it); malformed and empty stdin → empty stdout, exit 0, under 5 s; no daemon → empty stdout, exit 0, under 5 s.

- [ ] **Steps:** failing tests, red, implement, green, clippy, fmt, commit `"Add hook handling for session start and end"`.

---

### Task 9: `ensure-artifax.sh` and the release workflow

**Files:**
- Create: `scripts/ensure-artifax.sh` (copied into `plugins/claude-code/scripts/` by Task 10 and referenced by the Codex plugin), `scripts/test-ensure-artifax.sh`, `.github/workflows/release.yml`

**Interfaces:**
- `ensure-artifax.sh [exec <args...>]` with resolution order: `ARTIFAX_BIN` (warn if unusable), `artifax` on PATH that answers `--version` with "artifax", `ARTIFAX_INSTALL_DIR/artifax` (default `~/.local/bin`), `ARTIFAX_CONFIG_DIR/bin/artifax` (default `~/.artifax/bin`), then download from GitHub releases (`empathic/artifax`, targets `aarch64-apple-darwin`, `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-gnu`, tarball `artifax-<target>.tar.gz` + `.sha256`, verified). `MIN_VERSION=0.2.0`.
- `test-ensure-artifax.sh`: bash tests with a fake `artifax` script and a `PATH` sandbox: `ARTIFAX_BIN` wins; a foreign `artifax` on PATH (no "artifax" in `--version`) is skipped; fallback dir chosen when shadowed; `exec` passes arguments through; the download path is exercised against a local `python3 -m http.server` serving a fake tarball and checksum (no network).
- `release.yml`: on tag `v*`, build the three targets (macOS runner for darwin, ubuntu with `cross` or musl toolchain), run `just web` first so `web/dist` is embedded, package, checksum, upload with `softprops/action-gh-release`.

- [ ] **Steps:** write the tests script first (it must fail because the script does not exist), implement, run `bash scripts/test-ensure-artifax.sh`, add it to `quality_gates.sh`, commit `"Add the artifax installer script and release workflow"`.

---

### Task 10: Claude Code plugin

**Files:**
- Create: `plugins/claude-code/.claude-plugin/plugin.json`, `plugins/claude-code/.mcp.json`, `plugins/claude-code/hooks/hooks.json`, `plugins/claude-code/skills/artifax/SKILL.md`, `plugins/claude-code/commands/{open,serve,doctor,list}.md`, `plugins/claude-code/scripts/ensure-artifax.sh` (copy of `scripts/ensure-artifax.sh`, kept in sync by a test), `plugins/claude-code/README.md`, `.claude-plugin/marketplace.json`, `scripts/smoke-claude.sh`, `scripts/test-plugins.sh`

**Interfaces:**
- `plugin.json`: `{"name": "artifax", "version": "0.2.0", "description": "Local artifacts with comment-driven development: publish HTML pages, view them, and get feedback back", "author": {"name": "Empathic"}, "keywords": ["artifacts", "html", "preview", "comments"]}`.
- `.mcp.json`: `{"artifax": {"command": "${CLAUDE_PLUGIN_ROOT}/scripts/ensure-artifax.sh", "args": ["exec", "mcp", "--agent", "claude"]}}`.
- `hooks/hooks.json`: `SessionStart` → `${CLAUDE_PLUGIN_ROOT}/scripts/ensure-artifax.sh exec hook --agent claude session-start` (timeout 5), `SessionEnd` → `... session-end` (timeout 5).
- `SKILL.md` (frontmatter `name: artifax`, `description: Use when the user wants a web page, app, dashboard, or visual they can open in a browser and comment on; publishes HTML through the artifax tools`): when to publish; the page contract (a `<title>`, CSS tokens on `:root`, dark mode under `prefers-color-scheme` guarded by `:root:not([data-theme="light"])` and again under `:root[data-theme="dark"]`, explicit body background, phone width with 16 px gutters, browser storage wrapped in try/catch, `window.claude.use(name)` resolves `null` until phase 4); tool reference with argument shapes; the update workflow (`if_version` from the last `publish` or `read`); that URLs are for the person and `open` opens the browser; what is not yet available (comments arrive in phase 3, capabilities in phase 4).
- Commands: `/artifax:open [id]` (context block: `!\`"${CLAUDE_PLUGIN_ROOT}/scripts/ensure-artifax.sh" exec list --json\`` then instruct to call `open`), `/artifax:list`, `/artifax:serve [--bind 0.0.0.0|stop|status]`, `/artifax:doctor`. Each has `allowed-tools: Bash(${CLAUDE_PLUGIN_ROOT}/scripts/ensure-artifax.sh:*)`.
- `.claude-plugin/marketplace.json` at the repo root listing `artifax` with `source: "./plugins/claude-code"`.
- `scripts/test-plugins.sh`: validates every JSON file under `plugins/` and the two marketplace files with `python3 -m json.tool`, checks that `plugins/claude-code/scripts/ensure-artifax.sh` is byte-identical to `scripts/ensure-artifax.sh`, that every `commands/*.md` has frontmatter with `description`, and that `SKILL.md` frontmatter has `name` and `description`. Added to `quality_gates.sh`.
- `scripts/smoke-claude.sh` (manual, documented, not a gate because it calls a model): starts a daemon in a scratch home, runs `claude -p "Publish a one-line HTML page titled Smoke via the artifax publish tool, then call artifax status. Reply with only the artifact URL." --max-turns 4 --mcp-config <generated json pointing at target/debug/artifax mcp --agent claude> --strict-mcp-config`, greps a `/a/<id>` URL from the output, verifies it with `curl`, stops the daemon.

- [ ] **Steps:** write `test-plugins.sh` first and watch it fail on the missing files, create the plugin files, run `bash scripts/test-plugins.sh`, run `scripts/smoke-claude.sh` once and paste its output into the report (if `claude` is unavailable or has no credentials, say so; the test script is the gate), commit `"Add the Claude Code plugin and marketplace"`.

---

### Task 11: Codex plugin and marketplace

**Files:**
- Create: `plugins/artifax/.codex-plugin/plugin.json`, `plugins/artifax/.mcp.json`, `plugins/artifax/hooks/hooks.json`, `plugins/artifax/skills/artifax/SKILL.md`, `plugins/artifax/scripts/ensure-artifax.sh` (copy), `plugins/artifax/README.md`, `.agents/plugins/marketplace.json`, `scripts/smoke-codex.sh`
- Modify: `scripts/test-plugins.sh`

**Interfaces:**
- Directory is `plugins/artifax` because Codex marketplace entries must point at `./plugins/<plugin-name>` and the plugin name is `artifax`; the README says so.
- `plugin.json`: `name: "artifax"`, `version: "0.2.0"`, `description`, `author: {name: "Empathic"}`, `skills: "./skills/"`, `mcpServers: "./.mcp.json"`, `interface: {displayName: "Artifax", shortDescription: "Publish and preview HTML artifacts locally", longDescription: "...", developerName: "Empathic", category: "Developer Tools", capabilities: ["Interactive", "Write"], defaultPrompt: ["Publish this page as an artifact and open it", "List my artifacts", "Update the artifact with the latest changes"]}`.
- `.mcp.json`: `{"mcpServers": {"artifax": {"command": "bash", "args": ["${CODEX_PLUGIN_ROOT}/scripts/ensure-artifax.sh", "exec", "mcp", "--agent", "codex"]}}}` — verify the variable name Codex substitutes (the binary strings show `PLUGIN_ROOT` and `CLAUDE_PLUGIN_ROOT`); if neither expands in `codex mcp list` output, fall back to an absolute path written by an `install.sh` that the README documents.
- `hooks/hooks.json` in Claude format with `SessionStart` and `SessionEnd` running `ensure-artifax.sh exec hook --agent codex ...`; README notes `features.hooks = true` and the hook-trust prompt.
- `.agents/plugins/marketplace.json`: `{"name": "artifax", "interface": {"displayName": "Artifax"}, "plugins": [{"name": "artifax", "source": {"source": "local", "path": "./plugins/artifax"}, "policy": {"installation": "AVAILABLE", "authentication": "ON_INSTALL"}, "category": "Developer Tools"}]}`.
- `scripts/smoke-codex.sh` (manual, documented): with `CODEX_HOME` set to a scratch directory containing a minimal `config.toml`, runs `codex plugin marketplace add "$PWD"`, `codex plugin add artifax@artifax`, `codex mcp list` (assert `artifax` present), then `codex exec --skip-git-repo-check -s read-only "Publish a one-line HTML page titled Smoke via the artifax publish tool and reply with only its URL"`, verifies the URL with `curl`. The scratch `CODEX_HOME` needs auth: copy `~/.codex/auth.json` into it only for the run and delete it afterwards; document that.
- `test-plugins.sh` additionally runs `python3 ~/.codex/skills/.system/plugin-creator/scripts/validate_plugin.py plugins/artifax` when that script exists (skip with a note otherwise).

- [ ] **Steps:** extend `test-plugins.sh` (fails on missing files), create files, run it, run `smoke-codex.sh` once and record the result including whether the plugin root variable expanded, commit `"Add the Codex plugin and marketplace"`.

---

### Task 12: Pi extension

**Files:**
- Create: `plugins/pi/package.json`, `plugins/pi/tsconfig.json`, `plugins/pi/src/artifax.ts`, `plugins/pi/src/client.ts`, `plugins/pi/src/daemon.ts`, `plugins/pi/test/artifax.test.ts`, `plugins/pi/test/fake-api.ts`, `plugins/pi/README.md`, `scripts/smoke-pi.sh`
- Modify: `scripts/quality_gates.sh`, `justfile`

**Interfaces:**
- `package.json`: `name: "@empathic/artifax-pi"`, `type: "module"`, `pi: {extensions: ["src/artifax.ts"]}`, `peerDependencies: {"@mariozechner/pi-coding-agent": "^0.73"}`, dev deps `typescript`, `vitest`, `@sinclair/typebox`, `@mariozechner/pi-coding-agent@0.73.1` (for types), scripts `test`, `typecheck`.
- `daemon.ts`: `discover(home): DaemonInfo | null` (reads `daemon.json`, probes `/healthz` with a 1 s timeout), `ensure(home): Promise<DaemonInfo>` (runs `artifax serve --json` via `execFile` when discovery fails; locates the binary via `ARTIFAX_BIN`, then PATH; throws a clear error naming the install command otherwise).
- `client.ts`: `DaemonClient` with the same methods as the Rust one, using `fetch`, sending `X-Artifax-Session`.
- `artifax.ts` default export `(pi: ExtensionAPI) => void`: on `session_start`, ensure the daemon and `POST /api/sessions` with `{harness: "pi", harness_session_id: ctx.sessionManager.getSessionId(), cwd: ctx.cwd, pid: process.pid}`; register the nine tools with TypeBox schemas mirroring Task 6 (`promptSnippet` one line each); `registerCommand("artifax", ...)` with subcommands `open`, `list`, `status`; on `session_shutdown`, end the session. Tool results return `{content: [{type: "text", text: JSON}]}` with the same JSON shapes as the Rust tools, including `feedback: []`.
- Tests: vitest with `fake-api.ts` implementing the subset of `ExtensionAPI` used (`on`, `registerTool`, `registerCommand`), and a real daemon started by the test (spawn `cargo run -q -p artifax-cli -- serve --foreground --port 0` with a temp `ARTIFAX_HOME`, same pattern as the web e2e fixture): `session_start` registers a session (assert via `GET /api/sessions`); `publish` then `read` round trip; `if_version` conflict returns an error result naming `current`; unreachable daemon → error result with `daemon_unreachable`.
- `scripts/smoke-pi.sh` (manual): `PI=<scratchpad>/node_modules/.bin/pi` if present, else `npx --yes @mariozechner/pi-coding-agent@0.73.1`; runs `pi -e "$PWD/plugins/pi" --print "Publish a one-line HTML page titled Smoke via the artifax_publish tool and reply with only its URL"` with a scratch settings dir if Pi supports one (`--help` check), and records whether a provider key was available; not a gate.
- Gates: `just pi-test` (`cd plugins/pi && npm ci && npm run typecheck && npm test`) added to `quality_gates.sh`.

- [ ] **Steps:** write the failing vitest cases, implement, run, add the gate, run `smoke-pi.sh` once and record the outcome, commit `"Add the Pi extension"`.

---

### Task 13: Contract docs, skills sync, and gates

**Files:**
- Create: `docs/contract.md`
- Modify: `plugins/*/skills/artifax/SKILL.md` (shared content from one source: `docs/skill-body.md` included by a build step? No: keep three copies and have `test-plugins.sh` assert the section under `## Page contract` is identical across them), `scripts/quality_gates.sh`, `README.md`, `docs/superpowers/specs/2026-09-28-artifax-design.md` §13 (Pi is verified; describe the direct-HTTP extension; §11 note that Claude Code passes its session ID)

**Interfaces:**
- `docs/contract.md` sections: Tools (each tool, arguments, result shape, error codes), Sessions (how identity is established per harness), Page contract (as in the skills), Security model (from the README, expanded with the artifact-origin rules), What is not yet available.
- `quality_gates.sh` order: fmt, clippy, cargo test, web lint, web typecheck+unit, web build, pi typecheck+test, plugin structure test, installer test, e2e.

- [ ] **Steps:** write `test-plugins.sh`'s identical-section check first, write the docs, run the full gate script, commit `"Document the tool contract and wire the new gates"`.

---

## Self-review notes

- Spec §17 Phase 2 coverage: shim (T7), sessions (T5), hooks for start/end (T8), artifact tools and status (T6), HTTP MCP (T6), installer (T9), Claude plugin (T10), Codex plugin (T11), Pi extension (T12), release workflow (T9). Debts assigned by the coordinator: blocking store + timeouts (T1), asset store (T2), events and store minors (T3), CLI/web/lint/spec wording (T4).
- Review Focus mapping: 1 → T7; 2 → T8; 3 → T5; 4 → T6; 5 → T6; 6 → T6.
- Names used across tasks: `AppState::store_call`, `WrapCache`, `Session`, `RegisterSession`, `Store::{register_session, join_session, heartbeat, end_session, get_session, list_sessions, reap_sessions}`, `DaemonClient`, `ArtifaxTools`, `shim::run`, `HookInput`, `HookOutput`, `events::{session_start, session_end}`.
