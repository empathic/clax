# The Rust workspace's crate and module layout

Date: 2026-10-16
Status: decided (§9, 2026-10-16); carried out on branch `layout`, with the rulings of §10

Goal, in order: a layout a newcomer can read (the name of a crate or module
says what is in it, and the dependency graph says what may call what), then
compile speed under Bazel. Three changes come from the Bazel speed diagnosis
(`docs/bazel.md`, "Speed"): the web UI's embed moves out of `clax-server`
into a crate only the binary links; `clax-server` is split; and the
`clax-server` test-feature variant, which compiles the crate a second time
on every edit, goes away. This document places them in a layout for the
whole workspace.

Figures are from `main` at 34a2b9a9: line counts by `wc -l` (inline unit
tests included unless a column says otherwise), module references by a scan
of `crate::…` paths outside `#[cfg(test)]` modules, and the compile times
quoted from `docs/bazel.md`.

## 1. The layout today

### 1.1 Crates

| Crate | `src/` files | `src/` lines | `tests/` lines | Responsibility, as its root says |
|---|---|---|---|---|
| `clax-core` | 65 | 79,511 | 190 | "Core types and storage": IDs, model, events, config, home, signing, remote mode, git context, publish checks, page wrapping, and the whole SQLite store (`store/`, 49,061 lines) with visibility (2,659) and the audit export (`toolpath/`, 8,335) |
| `clax-server` | 69 | 36,885 | 43,959 | "HTTP server for Clax: REST API, content serving, SSE, embedded UI", plus the daemon's process lifecycle, the `sample()` providers, the realtime hubs, the discovery files every client reads, and the test daemon (`testing.rs`) |
| `clax-cli` | 41 | 17,914 | 10,647 | The `clax` binary: every subcommand, the blocking daemon client, and the embedded extension, plugin trees and marketplaces |
| `clax-mcp` | 12 | 6,923 | 5,888 | The MCP tool set, the async daemon client, the stdio shim |
| `clax-hooks` | 5 | 1,656 | 1,259 | Harness hook input and output |
| `clax-fake-exe` | 2 | 648 | 0 | Test helper: fake executables, the `clax` binary's path, the repository root |

### 1.2 Dependency graph

```
clax-cli ──┬── clax-server ──┬── clax-mcp ── clax-core
           │                 └── clax-core
           ├── clax-mcp
           ├── clax-hooks ── clax-core
           └── clax-core

dev only:  clax-cli tests, clax-mcp tests, clax-server tests
             ── clax-server with features test-support (+ test-routes)
```

The design spec's graph (`docs/superpowers/specs/2026-09-28-clax-design.md`
§4) shows `clax-cli → clax-server → clax-core` and no `clax-server →
clax-mcp` edge; the code has it (`crates/clax-server/src/routes/mcp.rs:17`,
`routes/sessions.rs:568`), and `clax-mcp`'s tests reach back to
`clax-server` through a dev-dependency (`crates/clax-mcp/Cargo.toml`), a
cycle Cargo allows for integration tests.

### 1.3 Pain points

**A. Two crates hold most of the code and many responsibilities.**
`clax-core` is the vocabulary every client needs (IDs, model, signing,
`remote`) and the SQLite store only the daemon and a few CLI commands need.
`clax-mcp` and `clax-hooks` use the store for nothing but `store::stamp_now`
(`crates/clax-mcp/src/calls.rs:114`) and hashing helpers in
`toolpath::args` (`crates/clax-hooks/src/events.rs:297`), yet every store
edit recompiles them. `clax-server` holds seven responsibilities (state and
request identity, realtime hubs, the `sample()` providers, the routes, the
daemon process, the discovery files clients read, the embedded web UI) and a
test harness.

**B. Lower modules reach up into routes.** Inside `clax-server`, the modules
every route builds on depend on particular routes:

| Module | Reaches into | For |
|---|---|---|
| `agent.rs` | `routes::{artifacts, assets, docs, questions, threads}` | body limits and headers (`PUBLISH_BODY_LIMIT`, `VIA_HEADER`, `parse_id`, `UPLOAD_BODY_LIMIT`, `DOCS_BATCH_LIMIT`, `ASK_BODY_LIMIT`, `THREAD_BODY_LIMIT`) |
| `identity.rs`, `db_caller.rs` | `routes::artifacts` | `SESSION_HEADER`, `VIA_HEADER` |
| `boot.rs` | `routes::artifacts` | `with_owner`, `strip_sessions`, `live_part` |
| `inbox.rs` | `routes::grants` | `grant_view` |
| `state.rs` (`AppState`) | `routes::{perf, sessions, toolpath}` | `Slot`, `VerifyTimes`, `CallIdWait`, `Exports` |
| `daemon.rs` | `routes::{grants, people, sessions, toolpath}` | `sweep`, `listen`, `CALL_ID_*`, `Exports` |
| `auth.rs` | `daemon` | `tune_connection` |

`agent`, `auth` and `identity` reference each other, and `AppState`
(`crates/clax-server/src/state.rs:59`) names a type from nearly every
module. Nothing in the build stops the next upward reference.

**C. Client-side code lives in the server.** `clax_server::daemon`
(`crates/clax-server/src/daemon.rs`) mixes the daemon's process (`serve`,
idle reaping, open-file limit) with the discovery files and start lock every
client reads: `DaemonInfo`, `StartingInfo`, `DaemonLock`, `pid_alive`,
`probe_host`, `DEFAULT_PORT`. The CLI's client imports them
(`crates/clax-cli/src/client.rs:14`), so the CLI depends on the whole server
to find the daemon, and every server edit recompiles all 17,914 lines of the
CLI.

**D. Embedded files are compile inputs of large crates.** Under Bazel the
debug `clax` embeds the web UI (`--cfg clax_embedded`, rust-embed's
`debug-embed`). The embed sits in `crates/clax-server/src/routes/shell.rs:13`,
so a web edit recompiles `clax-server` twice (with and without test
features) and everything after it. `clax-cli` embeds the extension build
(`src/extension_files.rs:9`), the plugin trees and marketplaces
(`src/plugins.rs:18-38`) and the plugin wrapper (`src/plugin_bin.rs:17`), so
a plugin or extension edit recompiles the whole CLI. Under Cargo the CLI's
`build.rs` also stamps `CLAX_BUILD_COMMIT`, so every commit recompiles it.

**E. Test support is a feature of the crate it tests.** `clax-server`'s
`[features]` (`crates/clax-server/Cargo.toml`):

- `test-support` gates one module, `testing` (`src/lib.rs:29`,
  `src/testing.rs`, 1,912 lines: `TestServer`, `TestClient`, `OwnerBrowser`,
  `EventReader`, `ManualSleeper`, `ManualTimer`, `load_fixture`, a LAN
  rewrite middleware), and two optional dependencies (`tempfile`,
  `rusqlite`). It uses only public items of the crate.
- `test-routes` adds three routes (`/api/_test/sleep/{ms}`,
  `/api/_test/slow_publish/{ms}`, `/api/_test/audit/ctx`) and a middleware
  that sleeps for an `x-clax-test-delay-ms` header
  (`src/routes/mod.rs:311-523`). Three test files use them
  (`tests/api_timeout.rs`, `api_feedback.rs`, `api_toolpath.rs`). About 25
  other `/api/_test/` routes are already compiled under `debug_assertions`
  instead (`src/routes/mod.rs:315-353`).

Because `clax-cli`, `clax-mcp` and `clax-server` itself turn these on in
dev-dependencies, Bazel builds `clax_server_test` beside `clax_server`
(`bazel/cargo.bzl:21-38`): 31.7 s of compiling on every edit at or below the
server (`docs/bazel.md`, "Speed"). Cargo does the same split between
`cargo build` and `cargo test`, and the gates run clippy twice to cover both
(`scripts/quality_gates.sh:125,130`).

Test support is also scattered: `clax-server/tests/common/people.rs` is
compiled into `clax-cli`'s tests by `#[path]`
(`crates/clax-cli/tests/common/mod.rs:76`), and reads the router's source
(`people.rs:948`).

**F. A process-wide override forces a second test binary.**
`routes::shell::set_web_dist` (`src/routes/shell.rs:29`) points the whole
process at another web UI directory, so the tests that use it
(`bridge_dev`, `shell_boot`, `shell_entries`) run in their own binary under
a lock (`tests/web_dist.rs`).

## 2. The proposed layout

### 2.1 Crates and the rule that orders them

```
crates/
  clax-core       Shared vocabulary and pure rules. No SQLite queries, no HTTP server.
  clax-store      The SQLite store: schema, migrations, queries, visibility, the audit export.
  clax-sample     The sample() capability's providers (Anthropic, stub), quota and cache.
  clax-realtime   The daemon's in-memory delivery: the event-stream hub, rooms, presence,
                  feedback and question waiters and fan-out, Codex push, live-page IDs.
  clax-http       What every route shares: AppState, ApiError, who a request speaks for,
                  middleware, the JSON views of threads, questions, inbox items and grants.
  clax-server     The daemon: every route, the router, the shell and content serving, serve().
  clax-mcp        The MCP tool set, its async daemon client, the stdio shim.        (unchanged)
  clax-hooks      Harness hook input and output.                                   (unchanged)
  clax-commands   Every `clax` subcommand and the blocking daemon client.
  clax-embedded   Files the binary carries: the web UI, the extension, the plugin trees,
                  the marketplaces, the contract's type definitions.
  clax-cli        The `clax` binary: wires the daemon and the embedded files into the commands.
  clax-testkit    The in-process test daemon, its clients and fixtures.
  clax-fake-exe   Test helpers with no workspace dependency.                       (unchanged)
```

The layering rule: a crate depends only on crates above it in this picture,
never sideways or up.

```
                         clax-core
                 ┌───────────┼──────────────────┬──────────────┐
            clax-store   clax-sample        clax-mcp      clax-hooks
                 │           │                 │              │
           clax-realtime     │                 │              │
                 │           │                 │              │
             clax-http ──────┘                 │              │
                 │                             │              │
            clax-server ───────────────────────┘              │
                 │                                            │
                 │        clax-commands ── clax-store, clax-mcp, clax-hooks
                 │              │
                 └──── clax-cli ┴── clax-embedded (no workspace dependency)

  tests only:  clax-testkit ── clax-server (and everything above it), clax-fake-exe
               clax-fake-exe ── nothing in the workspace
```

Two test-only rules keep test support in its own place:

- No crate turns on features of a workspace crate in a dev-dependency. The
  Bazel derivation fails when one does (§5.4), so a test-feature variant
  cannot come back.
- `clax-testkit` is a dev-dependency of integration tests (`tests/`) only,
  never of a crate's unit tests: it sits below every crate it tests, and
  `clax-mcp`'s and `clax-server`'s integration tests reach it through
  dev-dependency cycles that Cargo and Bazel both build once.

A newcomer's questions and where the names send them:

| Looking for | Crate, module |
|---|---|
| The `/api/artifacts/{aid}/threads` handler | `clax-server`, `routes/threads.rs` |
| How a request is authenticated | `clax-http`, `auth.rs`, `identity.rs`, `agent.rs` |
| An SQL query, a migration | `clax-store`, `threads.rs`, `migrations.rs` |
| How feedback wakes a long-poll | `clax-realtime`, `feedback.rs` |
| How the CLI finds the daemon | `clax-core`, `daemon.rs` (files) and `clax-commands`, `client.rs` |
| What the binary embeds | `clax-embedded` |
| The test daemon | `clax-testkit` |

### 2.2 Each crate: responsibility, public interface, sizes

Sizes are estimates from the module mapping in §3, `src/` lines with inline
tests.

**`clax-core`** (about 19,500 lines; today 79,511). The vocabulary and the
rules that need no database: `ids`, `model`, `anchor`, `events`
(`EventBus`), `config`, `home`, `secret`, `sig`, `remote`, `gitctx`,
`publish`, `wrap`, `names`, `mentions`, `capabilities`, `changelog`,
`clips`, `db`, `rights`, `room`, `live`, `feedback`, `questions`,
`working`, `presence`, `extension`, `perf`, `error` (`CoreError`), and two
new modules: `daemon` (the discovery files and start lock, from
`clax-server`) and `time` (`stamp`, `stamp_now`, from `store`). The pure
half of `visibility` (`Author`, `Reader`, `normalise`, `authors_of_*`, the
`placeholder` texts) stays here; `audit::args` holds the tool-call argument
hashing from `toolpath/args.rs`. `CoreError` keeps its `Db(rusqlite::Error)`
variant, so `clax-core` keeps `rusqlite` as a dependency for that type only
(§8, alternative 6).

**`clax-store`** (about 60,000 lines). `Store` and every `store::*` module
under its own name (`clax_store::threads`, `clax_store::people`, …), the
store-backed half of `visibility` (`Visibility`, `Connected`,
`for_reader`, `for_session`), and `toolpath` (the audit history's
projection and export, which reads `store::audit`). The store stays one
crate: its modules reference each other in a dense web (approvals,
artifacts, inbox, feedback and threads each reach the others), under one
schema and one migration sequence (§8, alternative 7).

**`clax-sample`** (about 3,000 lines). `Sampler`, `Provider`,
`SampleSettings`, the Anthropic and stub providers, `flight::drive`, the
SSE parser, quota and cache, and the request checks, which return a
`SampleError` the route maps to `ApiError` (today `request.rs` builds
`ApiError` itself, 11 uses). Depends on `clax-core` (`config::SampleConfig`)
and `reqwest`; no axum.

**`clax-realtime`** (about 4,400 lines). The daemon's in-memory delivery,
with no HTTP types and no `AppState`: `stream` (`Hub`, `Topic`, `Conn`),
`room` (`Rooms`), `presence`, `push` (`CodexPush`, `dispatch`), `working`
(sweeper, `settle`, `renew_for_tier`), `feedback` (`FeedbackWaiters`,
`Followers`, `FeedbackCtx`, `publish_states`, `wake`, `apply`),
`questions` (`QuestionWaiters`, `Sleeper`, `TokioSleeper`, `count_until`),
`live` (`LiveIds`, `sees_live_pages`) and `call_ids` (`CallIdWait`, from
`routes/sessions.rs`). Today these modules use no axum
(`stream.rs` mentions it once) and `AppState` only in functions that stay in
`clax-http` (`questions::announce`, `start_grace`, `inbox::listen`,
`approvals::listen`).

**`clax-http`** (about 10,000 lines). What every route shares: `AppState`
and `blocking` (`store_call`), `error` (`ApiError`), request identity
(`auth`, `identity`, `agent`, `grant_auth`, `db_caller`, `viewer`,
`extension`), middleware (`host`, `http_cache`, `live::hide_live_pages`,
`extension::gateway`, the `audit` context extractor and journal),
`wrap_cache`, and the response views (`views::thread`, from the second half
of today's `feedback.rs` with `Vis` and `VisFor`; `views::question`,
`views::inbox`, `views::approval`, `views::grant`, `views::artifact` for
`with_owner`, `strip_sessions` and `live_part`). Two new modules hold what
lower modules took from routes: `limits` (every request body limit) and
`headers` (`VIA_HEADER`, `SESSION_HEADER`, `parse_id`, the artifact path
extractor). `perf` (`Slot`, `VerifyTimes`) and `exports` (`Exports`) move
here from their routes because `AppState` holds them. The extractor impls
(`impl FromRequestParts<AppState> for Caller`) stay legal under the orphan
rule because `AppState` and the extracted types share this crate.

**`clax-server`** (about 19,000 lines). The daemon: `routes/` (unchanged
file names and paths, so `docs/contract.md`'s pointers into
`crates/clax-server/src/routes/` stay right), `router` (today's
`routes::router`, `build_router`, `build_router_with_shutdown`, `wrap`),
`shell` (`routes/shell.rs` without the embed, `boot.rs`, `shell_route.rs`),
and `serve` (the process half of `daemon.rs`: `ServeConfig`, `serve`,
`reap_idle`, `raise_open_file_limit`, `tune_connection`, ports). Public
interface: `serve(ServeConfig, Option<…>)`, `build_router(AppState)`, and
`routes::*` handlers for the testkit's use. Depends on `clax-mcp` for `/mcp`.

**`clax-mcp`** and **`clax-hooks`**: unchanged in content. They now depend
on a 19,500-line `clax-core` and no store.

**`clax-commands`** (about 17,000 lines). Every subcommand and the clients
they share: today's `main.rs` `Cli` and dispatch (as `cli.rs`),
`commands/*`, `client.rs`, `channel.rs`, `codex_approvals.rs`,
`hooklog.rs`, `host.rs`, `plugin_bin.rs`, `plugins.rs`,
`extension_files.rs`, `remote.rs`, `term.rs`, and `clippy.toml` (the rule
that only `client.rs` and `commands/tools.rs` make HTTP clients). Public
interface: `run(Env) -> ExitCode`, where `Env` carries what the binary
provides:

```rust
pub struct Env {
    /// The files the binary carries.
    pub embedded: &'static dyn clax_core::embedded::Bundle,
    /// Runs the daemon in this process (`clax serve --foreground`).
    pub serve: fn(&Home, ServeArgs) -> anyhow::Result<()>,
    /// The commit the binary was built from.
    pub build_commit: &'static str,
}
```

It depends on `clax-core`, `clax-store` (`doctor` and `init` open the store
directly: `commands/doctor.rs:4`, `commands/init.rs:1142`, and qualifier
concern `30b40937` on `commands/doctor.rs:730`), `clax-mcp` and
`clax-hooks`; not on any server crate and not on `clax-embedded`.

**`clax-embedded`** (a few hundred lines; its compile cost is the bytes it
embeds). The rust-embed folders and `include_bytes!` of: `web/dist` (the
web UI, from `clax-server`), `web/contract/**/*.d.ts` (from
`routes/shell.rs:253-300`), `web/dist-extension` (from `clax-cli`), the
plugin trees, `plugins/pi/package.json` and `README.md`, the three
marketplaces, and `scripts/ensure-clax.sh` (all from `clax-cli`). Its
`build.rs` is today's `clax-cli/build.rs` minus the commit stamp, plus
today's `clax-server/build.rs`. It depends on no workspace crate, only
`rust-embed`, so nothing but an asset change rebuilds it. It exposes plain
functions (`web_ui(path)`, `extension_files()`, `plugin_files()`,
`launcher()`), and the binary adapts them to the traits in
`clax_core::embedded`.

**`clax_core::embedded`** (new, tens of lines): the trait the daemon and
the commands read files through, so neither depends on `clax-embedded`:

```rust
pub trait FileTree: Send + Sync {
    fn get(&self, path: &str) -> Option<EmbeddedFile>;
    fn paths(&self) -> Vec<String>;
}
pub struct EmbeddedFile { pub data: Cow<'static, [u8]>, pub sha256: [u8; 32] }
pub trait Bundle: Send + Sync {
    fn web_ui(&self) -> &dyn FileTree;
    fn extension(&self) -> &dyn FileTree;
    fn plugins(&self) -> &dyn FileTree;
    fn launcher(&self) -> &'static str;
}
/// A tree read from a directory on every call (debug builds' `--web-dist`, tests).
pub struct DiskTree { … }
```

`AppState` gains `web: Arc<dyn FileTree>`; `ServeConfig` takes it.
`bridge_version` becomes a function of the tree (with today's restat
behaviour for a `DiskTree`), and `set_web_dist` and its process-wide
`RwLock` go away.

**`clax-cli`** (a few hundred lines). The package keeps its name, its
binary `clax` and its Bazel labels (`//crates/clax-cli:clax`,
`clax_warm`, `clax_opt`), so `cargo install --path crates/clax-cli`,
`cargo uninstall clax-cli` and `release.yml`'s `-p clax-cli --bin clax`
are unchanged. `main.rs` builds `Env` (the `Bundle` over `clax-embedded`,
`serve` over `clax_server::serve` with `CodexPush::from_env` and
`Sampler::from_home`, which today sit in `commands/serve.rs:48-55`) and
calls `clax_commands::run`. `build.rs` stamps `CLAX_BUILD_COMMIT`, so a new
commit recompiles this crate only. Its `tests/` stay: they test the binary.

**`clax-testkit`** (about 3,600 lines). `src/testing.rs` as the crate root's
modules (`server` for `TestServer`, `client` for `TestClient` and
`OwnerBrowser`, `events` for `EventReader`, `clock` for `ManualSleeper` and
`ManualTimer`, `fixture` for `load_fixture`), `tests/common/people.rs` as
`people`, `tests/common/clients.rs` as `clients` (the real-client tests'
harness), and `tests/fixtures/` as `fixtures/`. Ordinary dependencies
(`tempfile`, `rusqlite`, `reqwest`), no features.

**`clax-fake-exe`**: unchanged. It stays separate from `clax-testkit`
because `clax-core`'s and `clax-hooks`' tests use it and cannot depend on a
crate below the server.

## 3. What moves where

Whole files move with `git mv`, so `git log --follow` keeps their history.
Files that split (marked *split*) keep their history in the part that keeps
the path.

### 3.1 From `clax-core`

| Old | New |
|---|---|
| `src/store/mod.rs` | `clax-store/src/lib.rs` (`Store`, `schema_version`); `stamp`, `stamp_now` to `clax-core/src/time.rs` |
| `src/store/<name>.rs` (32 files) | `clax-store/src/<name>.rs` |
| `src/visibility.rs` *split* | pure half stays; `Visibility`, `Connected` and their store reads to `clax-store/src/visibility.rs` |
| `src/toolpath/{mod,project,segment,redact,…}.rs`, `tests.rs`, `segment_tests.rs` | `clax-store/src/toolpath/` |
| `src/toolpath/args.rs` | `clax-core/src/audit/args.rs` (`audit.rs` becomes `audit/mod.rs`) |
| constants the pure modules borrow from the store: `store::threads::{MAX_CLIP_BYTES, AUTHOR_AGENT}`, `store::artifacts::TEXT_FIELDS`, `store::approvals::{OWNER, UNCONNECTED}`, `store::people::CONTINUE_WAIT_SECS`, `store::inbox::is_item_id` | `clax-core` (`clips`, `model`, `visibility`), the store re-exporting them |
| `tests/toolpath/` (vectors read by the web and Pi tests through `//crates/clax-core:toolpath_vectors`) | stays; `clax-store`'s tests read it as test data |

Every `clax_core::store::X` becomes `clax_store::X`, and `clax_core::Store`
becomes `clax_store::Store`; `src/lib.rs`'s re-exports of store types move
to `clax-store`'s root. The source scan in `clax-core/src/feedback.rs:926`,
which walks `src` and `../clax-server/src`, walks every `crates/*/src`
instead.

### 3.2 From `clax-server`

| Old | New |
|---|---|
| `src/sample/*.rs` | `clax-sample/src/*.rs` (`mod.rs` → `lib.rs`) |
| `src/stream.rs`, `room.rs`, `presence.rs`, `push.rs`, `working.rs` | `clax-realtime/src/` |
| `src/feedback.rs` *split* | waiters, followers, `FeedbackCtx` and fan-out (lines 1-650) to `clax-realtime/src/feedback.rs`; thread views, `Vis`, `VisFor`, `delivery_viewer` to `clax-http/src/views/thread.rs` |
| `src/questions.rs` *split* | waiters and `Sleeper` to `clax-realtime/src/questions.rs`; `view`, `announce`, graces to `clax-http/src/views/question.rs` |
| `src/live.rs` *split* | `LiveIds`, `sees_live_pages` to `clax-realtime/src/live.rs`; `hide_live_pages` to `clax-http/src/live.rs` |
| `src/agent.rs`, `auth.rs`, `identity.rs`, `grant_auth.rs`, `db_caller.rs`, `viewer.rs`, `extension.rs`, `host.rs`, `http_cache.rs`, `error.rs`, `blocking.rs`, `state.rs`, `audit.rs`, `wrap_cache.rs` | `clax-http/src/` |
| `src/inbox.rs`, `approvals.rs` | `clax-http/src/views/inbox.rs`, `views/approval.rs` (with their `listen`) |
| body limits and headers in `routes/{artifacts,assets,docs,questions,threads}.rs` | `clax-http/src/limits.rs`, `headers.rs` |
| `routes/artifacts.rs` `with_owner`, `strip_sessions`, `live_part`; `routes/grants.rs` `grant_view` | `clax-http/src/views/artifact.rs`, `views/grant.rs` |
| `routes/perf.rs` `Slot`, `VerifyTimes`; `routes/toolpath.rs` `Exports`; `routes/sessions.rs` `CallIdWait` | `clax-http/src/perf.rs`, `exports.rs`; `clax-realtime/src/call_ids.rs` |
| `src/daemon.rs` *split* | discovery files, lock, `pid_alive`, hosts, ports, `retry_interrupted`, `DEFAULT_PORT` to `clax-core/src/daemon.rs`; `serve` and the rest to `clax-server/src/serve.rs` |
| `src/lib.rs` router assembly, `routes/mod.rs` router | `clax-server/src/router.rs`; `routes/mod.rs` keeps its tests (the L10 guard and route-table tests) |
| `src/boot.rs`, `shell_route.rs`, `routes/shell.rs` | `clax-server/src/shell/{boot,route,mod}.rs`; the `RustEmbed` and `include_str!`s to `clax-embedded` |
| `routes/*.rs` (other handlers) | stay at `clax-server/src/routes/` |
| `build.rs` | `clax-embedded/build.rs` |
| `src/testing.rs`, `tests/common/{people,clients}.rs`, `tests/fixtures/` | `clax-testkit/src/`, `clax-testkit/fixtures/` |
| `tests/web_dist.rs` and its modules `bridge_dev`, `shell_boot`, `shell_entries` | the modules join `tests/integration.rs`; `web_dist.rs` and its lock are deleted |
| `tests/*.rs` (the API tests) | stay; `use clax_server::testing::…` becomes `use clax_testkit::…` |

### 3.3 From `clax-cli`

| Old | New |
|---|---|
| `src/main.rs` *split* | `Cli`, dispatch to `clax-commands/src/cli.rs`; `fn main` and `Env` to `clax-cli/src/main.rs` |
| `src/commands/`, `client.rs`, `channel.rs`, `codex_approvals.rs`, `hooklog.rs`, `host.rs`, `remote.rs`, `term.rs` | `clax-commands/src/` |
| `src/plugins.rs`, `extension_files.rs`, `plugin_bin.rs` *split* | the embeds to `clax-embedded`; the logic (writing the marketplace, the launcher checks) to `clax-commands`, reading through `Bundle` |
| `src/commands/serve.rs` *split* | argument parsing and the background path stay in `clax-commands`; building `ServeConfig` and `close_inherited_fds` to `clax-cli/src/serve.rs` |
| `build.rs` *split* | the commit stamp stays; the embed directories to `clax-embedded/build.rs` |
| `clippy.toml` | `clax-commands/clippy.toml` (paths `clax::client` → `clax_commands::client`) |
| `tests/` | stay; `tests/common/mod.rs`'s `#[path]` include becomes `use clax_testkit::people` |

## 4. Compile speed

Bazel compiles every crate that depends on an edited one, whole
(`docs/bazel.md`, "Speed"). The table names what recompiles for three
edits; the times are the measured ones from that section for the targets
that exist today.

| Edit | Today | After |
|---|---|---|
| A store query (today part of `clax-core`) | `clax-core` lib (9.4 s + 6.2 s metadata) and unit tests (21.7 s); `clax-mcp` lib (7.1 s + 5.2 s) and tests; `clax-hooks`; `clax-server` lib (31.6 s) **and its test variant (31.7 s)**; server unit (33.4 s) and integration tests (31.0 s); `clax-cli` binary (12.1 s) and unit tests (11.1 s); every integration test | `clax-store` lib and unit tests; `clax-realtime`, `clax-http`, `clax-server` once each; `clax-commands`; `clax-cli` (thin); `clax-testkit`; the integration tests. Not rebuilt: `clax-core`, `clax-mcp`'s and `clax-hooks`' libraries and unit tests, `clax-sample`, `clax-embedded`, the variant |
| A route handler | `clax-server` lib and its variant (31.6 s + 31.7 s), server unit and integration tests, `clax-cli` binary and unit tests (12.1 s + 11.1 s), `clax-cli` and `clax-mcp` integration tests | `clax-server` lib and unit tests, `clax-testkit`, `clax-cli` (thin), the integration tests that use the testkit. Not rebuilt: `clax-commands` (today's 12.1 s + 11.1 s), the variant (31.7 s), `clax-http`, `clax-realtime`, `clax-sample`, `clax-store`, `clax-core` |
| The web UI (`//web:dist`) | `clax-server` lib and variant, server unit and integration tests, `clax-cli` binary, unit and integration tests, `clax-mcp` integration tests | `clax-embedded`, `clax-cli` (thin), one link. The Rust tests that serve the shell take `//web:dist` as run-time data through a `DiskTree`, so they rerun without recompiling |

Further effects: a plugin, skill or extension edit rebuilds `clax-embedded`
and the thin binary instead of the whole CLI; under Cargo a new commit
recompiles the thin `clax-cli` instead of 17,914 lines; a `clax-core` edit
still rebuilds every crate but `clax-embedded` and `clax-fake-exe`, without
the variant, with a 19,500-line root instead of 79,511.

One cost: an edit at the root now crosses a chain (`clax-core` →
`clax-store` → `clax-realtime` → `clax-http` → `clax-server` →
`clax-testkit` → integration tests) where today it crosses three crates.
rules_rust pipelines on metadata (the `+ metadata` figures above), so each
step starts once the step before has written its metadata, not its code.
The last commit of the migration measures the three edits and a
`clax-core` edit, under the conditions of `docs/bazel.md`'s table, and
replaces that table's rows.

## 5. How the test-feature variant goes away

### 5.1 `test-support`

`testing.rs` uses only `pub` items of `clax-server` (its `crate::` paths
resolve to public modules and types: `AppState`, `build_router_with_shutdown`,
`agent::Gate`, `feedback::FeedbackCtx`, `questions::Sleeper`, …), so it
moves unchanged into `clax-testkit` with `crate::` rewritten. Its LAN
rewrite middleware (`as_lan`) already wraps the router from outside. The
feature, its gate (`src/lib.rs:29`) and the optional dependencies go.

### 5.2 `test-routes`

The three routes and the delay middleware compile under `debug_assertions`,
as the other 25 `/api/_test/` routes do. The delay middleware is added to
each route group only when `AppState::test_delays` is set: a runtime field,
`false` in `serve`, set by the testkit's `TestServer`. A `just watch` or
`just dev` daemon then serves the test routes (as it already serves the
others, behind their extractors' checks) but never honours
`x-clax-test-delay-ms`. Release builds have none of them, as today.

### 5.3 What each build compiles

`clax-server` has no `[features]`. Its unit tests, the testkit and every
integration test link the same `clax_server` library the binary links, so
Bazel's `<lib>_test` twins are never declared and Cargo's `cargo build`
and `cargo test` share the library's build. The gate "clippy (no test
features)" (`scripts/quality_gates.sh:130`) checks nothing the first clippy
pass does not, and goes.

### 5.4 Keeping it gone

`scripts/bazel-derive.py` stops deriving `test_features` and fails when a
crate's dev-dependency turns on a feature of a workspace crate, naming the
crate and `clax-testkit`. `bazel/cargo.bzl` drops `_test_variants` and
`_test_label`. `scripts/bazel-sync.test.sh:32`, whose drift fixture edits
`test-routes`, uses another manifest edit.

## 6. Migration order

Each step is one commit (a few for the large ones) that builds and passes
`just test`, with `just bazel-sync` run and committed whenever a
`Cargo.toml` changes, so `scripts/bazel-sync.sh --check` passes at every
commit. Pure moves (`git mv` plus import rewrites) are committed apart from
edits, so a branch that conflicts can redo a move by rerunning it. A new
crate's commit adds, with it: the workspace member, a `BUILD.bazel` calling
`cargo_package()` (the sync check fails without one), its `rust_sources` in
`RUST_SOURCES` (`scripts/BUILD.bazel:63`), its name in
`scripts/check-version.sh:37` and `scripts/bump-version.sh:26`, a crate root
`//!` stating its responsibility and what it may not depend on, and the
qualifier record moves of §6.2 in the commit after it.

1. **Untangle inside `clax-server`.** Move the upward references of §1.3 B
   into new modules of the same crate (`limits`, `headers`, `views/*`,
   `perf`, `exports`, `call_ids`, `conn`); split `feedback.rs`,
   `questions.rs` and `live.rs` along the lines of §3.2. No crate changes;
   this makes every later extraction a file move.
2. **Discovery files to `clax_core::daemon`; `stamp` to `clax_core::time`.**
   `clax-cli`, `clax-mcp` tests and `testing.rs` import from core.
   `clax-core` gains `nix`'s `signal` feature.
3. **`clax-testkit`, and no test features.** Create the crate (§5.1); move
   the people fixture and clients harness; `test-routes` to
   `debug_assertions` with `test_delays` (§5.2); drop `[features]`; the
   derivation check and `cargo.bzl` cleanup (§5.4); drop the second clippy
   gate. Update `docs/bazel.md` "Targets" and "Gate for gate". This step
   alone removes the 31.7 s variant.
4. **The web UI as a per-daemon `FileTree`.** `clax_core::embedded`;
   `AppState::web`, `ServeConfig::web`; `bridge_version` of a tree;
   `--web-dist` builds a `DiskTree`; `set_web_dist` and `tests/web_dist.rs`
   go and their modules join `integration` (`split_tests` loses its `not`
   list). The embed still sits in `clax-server`, behind the trait.
5. **`clax-embedded`.** Move the web UI, contract `.d.ts`, extension,
   plugin and marketplace embeds and both build scripts' embed halves.
   `clax-server`'s `BUILD.bazel` loses `build_script_data`, `//web:dist`
   and the `*_bin` copies from `compile_data` (they were needed only
   because a generated directory was a compile input); its tests keep
   `//web:dist` as `test_data`. The parity entry
   (`scripts/bazel-parity.py:27`) follows
   `an_embedded_debug_build_versions_the_embedded_bridge` to
   `clax-embedded`. At this step `clax-cli` still holds the commands, so a
   web edit rebuilds it; the next step ends that.
6. **`clax-commands` and the thin `clax-cli`.** Move per §3.3; the
   commands read files through `Bundle` and run the daemon through
   `Env::serve`. `scripts/run.sh:148-150` names
   `//crates/clax-commands/...` for its own `clippy.toml`.
7. **`clax-sample`.** `SampleError` replaces `ApiError` in `request.rs`.
8. **`clax-realtime`.**
9. **`clax-http`;** `clax-server` keeps routes, router, shell, serve.
10. **`clax-store`.** Move the borrowed constants and the pure half of
    `visibility` first (one commit, inside `clax-core`), then the store,
    the store half of `visibility` and `toolpath`. `AGENTS.md`'s migration
    rule names `crates/clax-store/src/migrations.rs`.
11. **Docs and measurements** (§7), and a layering check:
    `scripts/check-crate-layers.py` (a `//scripts:crate_layers` test and a
    `quality_gates.sh` gate) reads `cargo metadata` and fails on any
    workspace dependency edge not in §2.1's graph.

Steps 1-6 deliver the three requested changes; 7-10 complete the layout and
can land later without blocking anything.

### 6.2 Qualifier records move with their code

Records name their subject by path, and supersession must keep the subject
(`qualifier agents concepts`), so a record cannot follow a `git mv` by
itself; its old subject goes MISSING in `qualifier review`. After each move
commit, in a commit of its own:

1. `qualifier threads <old path> --format json` for every moved file, and
   for every file that split, before the move (from the move commit's
   parent).
2. A `qualifier record --stdin` batch, written outside the repository and
   dry-run first, that records each open root at its new location (same
   kind, summary, detail, tags, suggested fix; the span at the moved lines;
   the detail opening with `Moved from <old path>:<lines>, record <old ID>`;
   `--ref git:<move sha>`), and each live reply as a reply to the new root.
3. A second batch that resolves each old root with `reason:obsolete` and
   `Moved to <new path>:<lines> as <new ID> in git:<move sha>`.
4. Evidence: `qualifier review <new paths>` reports every moved record
   FRESH, `qualifier threads <old paths>` lists nothing open, and
   `qualifier diff main` shows each old record resolved by its move.

Records on lines that move within one file are re-anchored with
`--supersedes` (same subject). Records on routes stay put: the routes keep
their paths.

What moves, by `qualifier threads crates` at 34a2b9a9 (65 open roots, 16
replies): 18 roots from `clax-core/src/store/` and `visibility.rs` to
`clax-store`; 14 from `clax-cli/src` to `clax-commands`; 7 from
`clax-server/src` to `clax-http` (on `agent.rs`, `audit.rs`,
`extension.rs`, `identity.rs`, `viewer.rs`); 2 on `feedback.rs`
(`1cdecf55` on lines 79-86 to `clax-realtime`, `e7b6d312` on lines
1569-1613 to `clax-http/src/views/thread.rs`); 1 on `stream.rs` to
`clax-realtime`; 2 on `clax-server/tests/common` and `tests/fixtures` to
`clax-testkit`. The 10 on `clax-server/src/routes/`, 4 on modules that stay
in `clax-core`, and those on `clax-mcp`, `clax-hooks` and `clax-fake-exe`
stay. `e3918e09` names `crates/clax-server/tests/fixtures/pi-agent.mjs`,
which commit `baa15c24` deleted; it is the owner's to close or re-anchor.

Resolving the old roots is a close that `AGENTS.md` reserves to the owner
unless the closing session wrote the record (§9, question 4).

## 7. Documentation that changes

- `docs/superpowers/specs/2026-09-28-clax-design.md` §4: the crate list and
  dependency graph become §2.1's.
- `docs/bazel.md`: "Targets" (no `<lib>_test` twins; the server's
  integration groups without a `not` list; `clax-embedded` as the one crate
  with generated compile inputs), "Keeping Bazel in step with Cargo"
  (test features are refused, not derived), "Gate for gate" (one clippy
  row), "Speed" (the new measurements, and the paragraph "What remains is
  structural" rewritten), "Differences from Cargo" (embedded files live in
  `clax-embedded`), and the dev-loop timings.
- `docs/usage.md` "Developing Clax": the paragraph on test binaries (no
  `clax-server/tests/web_dist.rs`), and one paragraph naming the crates and
  the layering rule, pointing at the design spec.
- `AGENTS.md`: the migration path (`crates/clax-store/src/migrations.rs`);
  "Test support lives in `clax-testkit` and `clax-fake-exe`; no crate turns
  on another's features for tests."
- `docs/contract.md`: line 2901's generated comment names
  `agent::ROUTE_NEEDS`, which becomes `clax_http::agent::ROUTE_NEEDS`; its
  pointers into `crates/clax-server/src/routes/` and the install lines
  stay right.
- `docs/verification.md` is evidence about commit `f680986` and keeps that
  commit's paths.
- Comments that name moved files: `web/BUILD.bazel:155,166`,
  `scripts/BUILD.bazel:5,31`, `plugins/pi/BUILD.bazel:7`,
  `docs/BUILD.bazel:2`, `web/e2e/fixtures.ts:27`.

## 8. Risks and alternatives

### Risks

- **Conflicts with branches in flight.** About five worktrees run at once.
  Each step lands as soon as it passes, pure moves apart from edits, and a
  branch rebases by rerunning a move (`git mv` and a scripted import
  rewrite) rather than resolving it by hand. Steps 9 and 10 touch the most
  files; land them when few branches are open.
- **Wider public surfaces.** Items that are `pub(crate)` today and used
  across a new crate boundary become `pub`. Each crate root documents its
  interface; the layering check (step 11) keeps callers to the allowed
  direction.
- **Source-scanning tests.** Several tests read source text: the router's
  route table and L10 guards (`routes/mod.rs`), the people fixture
  (`tests/common/people.rs:948`), `clax-core/src/feedback.rs:926`,
  `every_command_reaches_the_daemon_through_the_marked_clients`
  (`commands/tools.rs`), `store/people.rs:3750`. Each fails loudly when a
  path it reads moves; each step updates them, and the Bazel
  `test_compile_data` that names them.
- **A longer chain at the root** (§4). Measured in step 11; if the
  critical path for a `clax-core` edit grows, `clax-realtime` can fold into
  `clax-http` without touching the rest of the layout.
- **Records left MISSING** if a step's record move is skipped; §6.2's
  evidence step catches it, and `qualifier diff main` before merge lists
  any.
- **Migration file renamed.** A branch adding a migration rebases across
  the move of `migrations.rs`; git's rename detection carries content
  edits, and `AGENTS.md`'s numbering rule is unchanged.

### Alternatives considered

1. **Only drop the variant and move the embed, keep `clax-server` whole.**
   The least churn and most of the speed, but it leaves the upward
   references of §1.3 B, the client code in the server, and a 37,000-line
   crate. Not taken: readability comes first.
2. **Split `clax-server` by resource into vertical crates** (pages,
   conversation, people, live). Every group needs `AppState` and request
   identity, so a shared base crate exists anyway, and the routes reference
   each other across any such cut (`live` ↔ `sites`, `threads` → `assets`,
   `watches` → `live` and `sites`, `approvals` → `questions`). Revisit when
   `clax-server`'s own compile after a route edit is the largest step of
   `just test`'s critical path.
3. **axum `FromRef` sub-states** so extractors need a slice of the state,
   not `AppState`. Idiomatic, and it would let identity live below the
   state, but it rewrites every extractor and handler signature. Revisit if
   a crate needs the extractors without the whole state.
4. **Test routes injected by the testkit** through a router hook. Keeps
   them out of debug daemons, but adds a public extension point to the
   router used only by tests, and the delay middleware has to sit inside
   each group's timeout layer. The runtime flag of §5.2 keeps debug
   daemons free of the delay at much less cost.
5. **Keep `set_web_dist`.** It needs its own test binary and lock (§1.3 F);
   a per-daemon tree makes both unnecessary.
6. **A store-specific error type in `clax-store`.** It would drop
   `rusqlite` from `clax-core`, but every `CoreError` conversion in the
   server and the CLI would change. `rusqlite` is already built for the
   store; holding it as a dependency of the error type costs no build time.
7. **Several store crates by domain.** The store modules reference one
   another throughout (approvals, artifacts, inbox, feedback, threads,
   sessions, people), over one schema and one migration sequence.
8. **The `FileTree` trait in `clax-embedded`.** The daemon would then
   depend on `clax-embedded`, and a web edit would rebuild the server
   again.
9. **Rename the binary's package to `clax`.** User-visible:
   `cargo install --path crates/clax-cli`, `cargo uninstall clax-cli`
   (`docs/usage.md:50,83`, `docs/contract.md:4320,4328`) and `release.yml`
   would change.
10. **Fold `clax-fake-exe` into `clax-testkit`.** `clax-core`'s tests use
    it and cannot depend on a crate below the server.
11. **Rustc's incremental compilation under Bazel.** Already decided
    against: its incremental directory is full of hard links
    (`docs/bazel.md`, "Speed"). The owner's ruling of 2026-10-16 on hard
    links ("Hard links that we create are banned, not those created by well
    tested tools") no longer forbids it, but it stays off: "Disk filling up
    is a bigger issue right now than slightly slower incremental builds."

## 9. Open questions for the owner

1. **Crate names.** Recommended: `clax-store`, `clax-sample`,
   `clax-realtime`, `clax-http`, `clax-commands`, `clax-embedded`,
   `clax-testkit`. Others considered: `clax-delivery` for `clax-realtime`;
   `clax-api` or `clax-server-core` for `clax-http`; `clax-assets` for
   `clax-embedded`; `clax-test-daemon` for `clax-testkit`.
   **Decided:** the proposed names.
2. **The binary's package stays `clax-cli`,** with the commands in
   `clax-commands`. Recommended: yes; nothing a user types changes.
   **Decided:** not asked; the recommendation stands.
3. **The `clax-core`/`clax-store` split (step 10).** The largest step and
   the one most in the way of open branches. Recommended: do it, after
   steps 1-9 have landed and been measured; it is what makes clients unable
   to reach SQL and stops store edits from rebuilding `clax-mcp` and
   `clax-hooks`. **Decided:** "In this pass, last": every other step first,
   measured, then the store, on the same branch.
4. **Moving qualifier records.** Approve once, for this migration, the
   convention of §6.2 (re-record at the new path citing the old ID, resolve
   the old with `reason:obsolete` and the new ID), so the migrating session
   may close records it did not write. Recommended: approve. Otherwise each
   step leaves its old records open for the owner to close.
   **Decided:** "Approve the move": each record pinned to moved code is
   recorded again at its new path, and the old one closed citing the new
   record's ID.
5. **Test routes in debug daemons.** §5.2 compiles the three `test-routes`
   routes into every debug build, as the other 25 `/api/_test/` routes
   are, with the delay header honoured only in the testkit. Recommended:
   accept; the alternative is a router hook (§8, alternative 4).
   **Decided:** "Debug builds serve them": the three routes are under
   `debug_assertions`, the delay header is honoured only where the testkit
   turns it on (`AppState::test_delays`), no test features remain, and the
   Bazel derivation refuses one.
6. **`clax-commands` opens the store directly** (`doctor`, `init`'s
   credential revoke), so it depends on `clax-store` and is rebuilt by
   store edits. Recommended: keep for now; concern `30b40937` already
   tracks the risk that brings. **Decided:** not asked; the recommendation
   stands.
7. **Record `e3918e09`** names a file deleted in `baa15c24`: close it, or
   re-anchor it on `crates/clax-server/tests/common/clients.rs`.
   **Decided:** not asked; re-anchored as recommended, on that file's new
   home, `crates/clax-testkit/src/clients.rs` (record `62a2827d`).

The owner also set the order of the goals: "We should be thinking about the
total layout for readability and maintainability first and speed of
compilation second", and, for build settings, "Disk filling up is a bigger
issue right now than slightly slower incremental builds."

## 10. Rulings made while carrying it out

Where the plan above left a choice open or did not fit the code, the
migration decided as follows.

- **`live_access` stays an HTTP module** (`clax-http/src/live_access.rs`):
  `hide_live_pages` is middleware and `sees_live_pages` reads a request's
  connection; only `LiveIds` is in-memory delivery (`clax-realtime`).
- **`FeedbackCtx`'s builders sit in `state.rs`** (`feedback_ctx`,
  `sessions_ended`): they read `AppState`, which `clax-realtime` may not
  name.
- **`conn` (the accepted connections' socket options) is `clax-http`'s,
  not `clax-server`'s:** the connect-info impl for the tuned listener
  (`auth::TunedListener`) must sit in the crate that defines `Conn`, by the
  orphan rule, and `TunedListener` calls `tune_connection`.
- **The test fixtures move to `crates/clax-testkit/fixtures/`,** except
  `harness-ids`, which only `clax-server`'s `tests/harness_ids.rs` reads:
  it stays in `crates/clax-server/tests/fixtures/`.
- **`clax-testkit` is never a unit-test dependency:** a crate's `src/`
  tests use its own helpers; only `tests/` reach the testkit.
- **`clax-testkit`'s root is split into modules** (`server`, `client`,
  `events`, `clock`, `fixture`, `people`, `clients`), re-exported at the
  root with globs, so tests keep writing `clax_testkit::TestServer`.
- **The contract's `.d.ts` files stay in `clax-server`'s
  `routes/shell.rs`:** they are small, change with the routes that serve
  them, and moving them would make a contract edit rebuild the binary's
  embed for no gain.
- **`Bundle` has no `web_ui`:** the commands never read the web UI; the
  daemon gets it as `ServeConfig::web`, built by the binary's
  `Env::serve` (`clax-cli/src/serve.rs`, `bundle.rs`).
- **`clax-embedded` exports `LAUNCHER`, a `&'static str` constant,** in
  place of a `launcher()` function.
- **The test daemon serves `web/dist` from disk** (`clax_testkit::web_ui`,
  a `DiskTree`), so the testkit does not link the embed.
- **No library with rmeta consumers reads `env!` of a sandbox path.**
  Bazel's pipelined build compiles a crate's metadata and its rlib in two
  sandboxes; a path in the crate (an `env!` set from `$(execpath)`) differs
  between them, so does the crate hash, and a dependent built against the
  metadata fails to link (E0463). `clax-embedded` passes its directories
  only to rust-embed, whose embedded bytes are the same in both.
- **`cargo_package` drops repeated labels** in `compile_data` (`_uniq` in
  `bazel/cargo.bzl`): a test's compile data is the library's plus its own,
  and Bazel refuses a label listed twice.
- **The commands reach the binary's `Env` through a process-wide
  `OnceLock`** (`clax_commands::run` sets it once), not through an argument
  threaded into every command; unit tests, which run no binary, read a
  bundle of the checkout's launcher and empty trees.
- **`doctor --agent` still compiles in the plugins' `SKILL.md` files**
  (`clax-commands/src/commands/doctor_agent.rs`, `include_str!`), so a
  skill edit rebuilds the commands; they belong behind `Bundle`, a follow-up
  recorded on those lines.
- **`SampleError` becomes `ApiError` by `From`,** in `clax-http`'s
  `error.rs`: a 400 with the check's code and message, as before.
- **The delivery guard** (`clax-core/src/feedback.rs`,
  `feedback_items_are_built_only_under_visibility`) names the crates it
  scans, the daemon's (`clax-core`, `clax-store`, `clax-realtime`,
  `clax-http`, `clax-server`), rather than every crate: the commands' hook
  builds a `Notice` type of its own.
- **The route-table guards skip a `clax_http::` prefix** as they skip
  `crate::`, and the router imports `agent` and `limits` so each route
  stays one line the guards can read.

- **`store::people::CONTINUE_WAIT_SECS` and `store::inbox::is_item_id` stay
  in `clax-store`** (§3.1 moved them to `clax-core`): no pure module reads
  either, only the store and the daemon's routes, which depend on the store.
- **`RegisterSession` moves to `clax_core::model`** (re-exported at the
  core root and as `clax_store::RegisterSession`): `clax-mcp` builds one
  for the daemon, and holding it in the store would have made the tool set
  link SQLite.
- **The toolpath vectors stay in `clax-core/tests/toolpath/`:**
  `clax-store`'s tests read them across the crate boundary (`#[path]` and
  `include_str!` of `../../../clax-core/tests/toolpath/…`, and
  `//crates/clax-core:toolpath_vectors` as test data), as the web and Pi
  tests already do, so the shared vectors keep one home.
- **`Rights::parse_internal` is `pub`:** the store reads the internal form
  of a grant's rights, and now from another crate.
- **`cargo_package` declares a `manifest` target** (each crate's
  `Cargo.toml`), which `//scripts:crate_layers` reads to check the layering
  rule (`scripts/check-crate-layers.py`). Dev-dependencies are not checked
  by it: tests reach the support crates and the crates they drive, and the
  derivation already refuses features turned on there.
