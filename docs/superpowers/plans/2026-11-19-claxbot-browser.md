# ClaxBot browser testing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** ClaxBot scripts can call `clax.browser.test(target, opts)` to load the run's draft or a version of its artifact in Cloudflare Browser Run, act on it, assert on it, run its saved `tests/*.test.js`, and get console, errors, blocked requests and screenshots back; and, behind an admin switch that is off by default, `clax.browser.browse(url, opts)` on public pages. Charged by measured browser time under the Spend rule.

**Architecture:** All policy lives in `clax-bot`'s new `browser` module: the CDP session over a `CdpTransport` trait, the request policy that serves a clax from a synthetic origin through the CDP `Fetch` domain, steps, caps, result shaping and metering. A host provides only the transport (a WebSocket to Browser Run on the daemon), Clax's UI files, the site's headers and its read-only API (`BrowserSource`). The Spend path reuses `Store::spend_journaled`. The Worker answers not configured.

**Tech Stack:** Rust (clax-bot, clax-store over SQLite, clax-server on axum, tokio-tungstenite with rustls), TypeScript (`web/bridge`, vitest), Svelte 5 (`web/shell`, vitest), Bazel through `just`.

**Spec:** `docs/superpowers/specs/2026-11-19-claxbot-browser-design.md` (cited as "spec §n"). Read it whole, and its qualifier threads (`qualifier threads docs/superpowers/specs/2026-11-19-claxbot-browser-design.md`), before Task 1.

## Global Constraints

- No `unsafe`. No hard links. No Perl. No new npm runtime dependency.
- Build and test only through `just` (Bazel): `just test-affected` while working, `just check` once before merge. Never `cargo build`/`cargo test`/`pnpm install` in the worktree. After any `Cargo.toml`/`Cargo.lock` change run `just bazel-sync` (with `~/.local/homebrew/bin` first on `PATH` for Python's `tomllib`) and commit its result in the same task.
- Commit with `git -c commit.gpgsign=false commit`; conventional subjects (`type(scope): summary`); no attribution lines; never push.
- Before editing a file, `qualifier threads <path>`; account for what is open. Record what the next person needs (`qualifier record …`); more than two writes go through `record --stdin`, dry-run first, with the file outside the repository.
- Tests: no sleeps, no wall-clock budgets, injected clocks and deadlines (`ManualClock`, `FakeBrowserSource`'s manual deadline). A new `#[cfg(test)]` module carries `#[warn(clippy::disallowed_methods)]`.
- One migration, number **67** (the next free on main at b605b4893). If another branch lands 67 first, renumber; later tasks extend 67 in place.
- Exact values (spec §5, §8, §10): per-test time cap default 60 s, 10–120; tests per run 10; browses per run 10; concurrent browsers 2; steps per call 50; `wait` 10 s a step; `eval` value 16 KiB; console 100 entries of 1,000 characters; errors 50; each request list 100; `html` 100,000 characters; `text` 20,000; screenshots 6 a test, longer side at most `RunLimits::image_side`; saved test files 50, 5 s a test; `keep_alive=10000`; line `browser.time` at 90,000 micro-USD per browser hour, unit `browser_h`, base 3,600,000 (ms); measured time rounded up to the whole second.
- Viewports: `desktop` 1280×800 scale 1; `tablet` 820×1180 scale 2 mobile; `phone` 390×844 scale 3 mobile; custom 200–3,840 × 200–4,000, scale 1–3.
- The synthetic origin is `https://<artifact ID>.clax-test.invalid`.
- "ID" in prose, never "id".

## Review Focus

Inputs the spec implies and no requirement names. Each has a test in the task that owns the code.

1. **Two `browser.test` calls in one `Promise.all`** open one browser after the other, never two at once for one run, and each is charged its own time (Task 9, `two_tests_in_one_script_run_one_after_the_other`).
2. **A page that navigates itself away on load** (`location = "https://elsewhere.example/?d=" + data`) is blocked, listed in `requests.blocked` with `why: "origin"`, and the test still answers (Task 5, `a_navigation_to_another_origin_is_blocked`).
3. **A CDP event that arrives between a command and its answer** (a console message during `Runtime.evaluate`) is kept, in order (Task 4, `events_between_a_command_and_its_answer_are_kept`).
4. **The deadline firing during a step** returns the steps done, the rest `skipped`, `"time"` in `truncated`, `passed: false`, sends `Browser.close`, and is charged (Task 7, `the_deadline_mid_step_answers_what_was_gathered`).
5. **A draft edit not yet committed** (`files.write` earlier in the same script) is what the browser loads (Task 9, `the_test_loads_the_drafts_uncommitted_edit`).
6. **A step replayed after a crash** answers its test from the journal: the fake source's `opened` count stays 1 (Task 9, `a_replayed_step_opens_no_browser`).

## Threads

Spec threads each task addresses are named in the task. Open threads on code this plan touches, to account for and leave open:

- `1014d9d7` (`web/bridge/src/bridge.ts`, eager bridge over its gzip budget): Task 10 adds only a few lines to the eager bridge and measures with `node web/scripts/bundle-size.mjs`; reply to the thread with the measurement.
- `c386f7f1` (`crates/clax-bot/src/images.rs`, image side): screenshots are scaled to `RunLimits::image_side` (Task 7).
- `a602eba5`, `70a0f3e0`, `285218bc` (`crates/clax-bot/src/clax/web.rs`): Task 9 copies the check order, not these choices; reread them before writing `clax/browser.rs`.
- `8349de4e`, `1dc8415a` (`crates/clax-store/src/bot_content.rs`, upload cost inside the write transaction): screenshots go through `put_bot_content` as is.

## File Structure

| File | Responsibility |
|---|---|
| `crates/clax-store/src/migrations.rs` | Migration 69 and its Rust step |
| `crates/clax-store/src/credits.rs` | `Unit::BrowserHour`, `LINE_BROWSER_TIME`, seeding |
| `crates/clax-store/src/bot_settings.rs` | The `browser.*` settings |
| `crates/clax-store/src/bot_web.rs` | `bot_spend_counts`; `browser_choice`/`set_browser_choice` beside `web_choice` |
| `crates/clax-store/src/bot_runs.rs` | `Run::browser`, `NewRun::browser`, `Bindings::clax(artifact, web, browser)` |
| `crates/clax-core/src/audit/mod.rs` | `AuditKind::BotBrowser`, `AuditKind::MeBrowser` |
| `crates/clax-bot/src/browser/mod.rs` (new) | Module docs, `BrowserSource`, `CdpTransport`, `Served`, `BrowserError` and codes, op names |
| `crates/clax-bot/src/browser/access.rs` (new) | `BrowserShape`, `BrowserAccess`, `BrowserGrant`, `BrowserOff` |
| `crates/clax-bot/src/browser/cdp.rs` (new) | The CDP connection: IDs, answers, events, flattened sessions, deadline |
| `crates/clax-bot/src/browser/opts.rs` (new) | Parsing and checking `BrowserTarget`, `BrowserOpts`, steps, viewports |
| `crates/clax-bot/src/browser/serve.rs` (new) | The synthetic origin's page set and request policy |
| `crates/clax-bot/src/browser/session.rs` (new) | One test from open to close: sequence, steps, screenshots, saved tests, result |
| `crates/clax-bot/src/browser/browse.rs` (new) | `BrowseRule`, the sender scan, browse egress |
| `crates/clax-bot/src/browser/fake.rs` (new) | `FakeChrome`, `FakeBrowserSource` |
| `crates/clax-bot/src/clax/browser.rs` (new) | The Spend path of `browser.*` in Clax's session |
| `crates/clax-bot/src/clax.d.ts`, `api.rs`, `prompt.rs`, `clax.prompt.md`, `clax/session.rs`, `clax/provider.rs`, `events.rs` | Declaration, `OpShape`, `{rules.browser}`, `Part::Browser`, `with_browser`, `with_call_fields` |
| `crates/clax-bot/tests/browser.rs` (new) | Run-level tests |
| `crates/clax-server/src/cloudflare.rs` (new) | The Cloudflare credentials, shared |
| `crates/clax-server/src/browser_source.rs` (new) | `DaemonBrowser` |
| `crates/clax-server/src/routes/admin.rs`, `routes/bot.rs`, `router.rs`, `bot.rs`, `core.rs` | Admin and Me routes, the run's shape, wiring, `[bot.browser]`, the content route |
| `web/bridge/src/bridge.ts` | The `clax-test` hook |
| `web/shell/src/bot/AdminBrowser.svelte` (new), `AdminPage.svelte`, `MePage.svelte`, `RunPanel.svelte`, `model.ts`, `api.ts` | The UI |
| `docs/usage.md`, `docs/contract.md`, `crates/clax-worker/README.md` | The docs |

---

## Phase 1: Core (`clax-store`, `clax-bot`)

### Task 1: Migration 69: the browser line, the person's switch, the run's shape

Addresses spec threads `c80fad7d` (charging rule), `e4dc1bbb` (separate switch).

**Files:** modify `crates/clax-store/src/migrations.rs`, `credits.rs`, `bot_web.rs`, `bot_runs.rs`, `crates/clax-core/src/audit/mod.rs`; web `UNITS` map in `web/shell/src/bot/model.ts` (label "browser hour").

**Interfaces produced:**
- `Unit::BrowserHour` (`"browser_h"`, base `3_600_000`), in `ALL`, `as_str`, `base`.
- `pub const LINE_BROWSER_TIME: &str = "browser.time"`, seeded by migration 69's Rust step (`seed_browser_line`, `INSERT OR IGNORE`) at 90,000 micro-USD, the infrastructure margin, `applies` always, `source` "Cloudflare Browser Run list price, $0.09 per browser hour (check)".
- Migration 69: rebuild `cost_lines` with `'browser_h'` in the `unit` CHECK (as migration 63 did); `ALTER TABLE bot_person_prefs ADD COLUMN browser INTEGER NOT NULL DEFAULT 1`; `ALTER TABLE bot_runs ADD COLUMN browser TEXT NOT NULL DEFAULT ''`; its mark entry, as migration 66's.
- `Store::browser_choice(person) -> Result<bool>` (true without a row), `Store::set_browser_choice(ctx, person, on) -> Result<bool>`, audited `me.browser`.
- `Run::browser`, `NewRun::browser` (`""`, `"test"`, `"browse"`, `"test,browse"`); `Bindings::clax(artifact, web, browser)` adds `browser.test`/`browser.browse` to `granted` after the web operations.
- `AuditKind::BotBrowser = "bot.browser"`, `AuditKind::MeBrowser = "me.browser"`.

- [ ] **Tests first** (in-module): `migration_69_seeds_the_browser_line` (unit, price, applies), `browser_h_round_trips_with_base_3_600_000`, `a_person_turns_the_browser_off_and_back` (one `me.browser` record), `a_run_keeps_its_browser_shape` (`create_run_exclusive` with `browser: "test"`, read back, `bindings().bindings[0].granted() == ["browser.test"]`), and the migration suite's existing checks with the new version.
- [ ] Run `just test-affected`; expect compile failures.
- [ ] Implement; fix every `match` on `Unit` and fixtures the compiler and tests name (toolpath expected files for the audit kinds).
- [ ] `just test-affected` passes (`//crates/clax-store:clax_store_lib_test` among them).
- [ ] Commit: `feat(store): price browser time and keep a run's browser shape`

### Task 2: The browser settings and spend counts

**Files:** modify `crates/clax-store/src/bot_settings.rs`, `bot_web.rs`.

**Interfaces produced:** `Setting` variants (spec §10.1): `BrowserEnabled` (`browser.enabled`, Bool, false), `BrowserMaxTestsPerRun` (Positive, 10), `BrowserTestSeconds` (new kind `TenTo120`, 60), `BrowserMaxConcurrent` (new kind `OneTo50`, 2), `BrowserBrowseEnabled` (Bool, false), `BrowserBrowseUrlRule` (new kind `BrowseRule`: `sender`, `allowlist`, `any_public`; default `sender`), `BrowserBrowseAllowlist` (`Hosts`, empty), `BrowserMaxBrowsesPerRun` (Positive, 10). `Store::bot_spend_counts(run, namespace) -> Result<BTreeMap<String, u32>>` (`op LIKE '<namespace>.%'`); `bot_web_counts` becomes a call of it with `"web"`.

- [ ] **Tests first:** `the_browser_settings_default_off`, `a_bad_browse_rule_or_time_is_refused` (`"open"`, `9`, `121`), `spend_counts_are_per_namespace` (a `web.fetch` and a `browser.test` entry marked `counted`, counted apart).
- [ ] `just test-affected`; fail; implement; pass.
- [ ] Commit: `feat(store): browser settings and per-namespace spend counts`

### Task 3: The `browser` module's types and the grant

Addresses spec threads `78f39245`, `a4a92ac5`.

**Files:** create `crates/clax-bot/src/browser/mod.rs`, `browser/access.rs`; modify `crates/clax-bot/src/lib.rs`.

**Interfaces produced:**
- `CdpTransport`, `BrowserSource` exactly as spec §4.4 (with the crate's `MaybeSend`/`MaybeSendSync` and the `async_trait` `?Send` split on wasm32 that `WebSource` uses), `Served { status: u16, headers: Vec<(String, String)>, body: Vec<u8> }`.
- `BrowserError { code: &'static str, message, retryable, opened: bool }` and the codes of spec §5.4 as constants (`CODE_BROWSER_DISABLED`, …); `OP_TEST = "browser.test"`, `OP_BROWSE = "browser.browse"`.
- `BrowserShape { test, browse }` with `parse`/`as_str`/`any` like `WebShape`; `BrowserAccess::now(store, person, configured: bool) -> Result<BrowserAccess>` reading the Task 2 settings, `browser_choice`, and for browse `web_choice` too; `BrowserAccess::shape()`.

- [ ] **Tests first** (in-module): `shapes_round_trip_through_their_text`, `access_is_off_when_the_admin_or_the_person_says`, `browse_needs_the_web_choice_too`, `not_configured_without_a_source`.
- [ ] `just test-affected`; fail; implement; pass.
- [ ] Commit: `feat(bot): the browser module's source, transport and grant`

### Task 4: The CDP connection and `FakeChrome`

**Files:** create `crates/clax-bot/src/browser/cdp.rs`, `browser/fake.rs`.

**Interfaces produced:**
- `Cdp::new(transport, deadline: BoxFuture<'static, ()>, cancel)`; `async fn call(&mut self, session: Option<&str>, method, params: Value) -> Result<Value, CdpFail>` (assigns IDs, matches answers, queues every event that arrives meanwhile); `fn drain_events(&mut self) -> Vec<CdpEvent>`; `async fn next_event(&mut self) -> Result<CdpEvent, CdpFail>`; `CdpFail::{Deadline, Cancelled, Closed, Protocol(String), Transport(BrowserError)}`.
- `FakeChrome`: an in-memory `CdpTransport` driven by a handler `Fn(&Value) -> Vec<Value>` (the frames it answers a command with: events then the answer), recording every command; helpers that build a page model answering `Target.*`, `Page.navigate` (emitting `Fetch.requestPaused` for the URL, then `Page.loadEventFired` once fulfilled), `Runtime.evaluate` by a closure. `FakeBrowserSource { opened: AtomicU32, deadline: manual trigger, clax_file/site_headers/site_api tables }`.

- [ ] **Tests first:** `answers_are_matched_by_id_out_of_order`, `events_between_a_command_and_its_answer_are_kept`, `the_session_id_goes_on_page_commands`, `the_deadline_ends_a_waiting_call`, `cancel_ends_a_waiting_call`, `a_closed_socket_is_closed_not_a_hang`.
- [ ] `just test-affected`; fail; implement; pass.
- [ ] Commit: `feat(bot): a CDP connection over any transport, and a fake Chrome`

### Task 5: Serving a clax from the synthetic origin

Addresses spec threads `bdf65886`, `4d8ee9bb`, `f10e6a2c` (read-only API).

**Files:** create `crates/clax-bot/src/browser/serve.rs`.

**Interfaces produced:** `enum Pages { Draft(BTreeMap<String, DraftEntry>, base_version), Version(ArtifactId, u32) }`; `struct Origin { artifact, host: String }` (`<artifact>.clax-test.invalid`); `async fn answer(&self, req: &Paused, source: &dyn BrowserSource, store: &Store, person: &str) -> Decision` where `Decision::{Fulfill(Served), Fail { why: "origin" | … }}` per spec §6.2: `/` → `index.html`; HTML wrapped with `clax_core::wrap::wrap_page`; `/_clax/bridge.js`, `/_clax/bridge/*` from `clax_file`; `/_clax/blob/<ID>` the artifact's asset; `/_clax/api/` GET through `site_api`, other methods 403 `read_only_test`; else 404; every answer with `site_headers`; `Service-Worker: script` an empty script; any other origin `Fail`.

- [ ] **Tests first:** `the_index_is_served_wrapped_with_the_bridge`, `a_draft_file_is_served_with_its_content_type`, `a_missing_file_is_a_404_listed_as_failed`, `api_gets_go_to_the_host_and_writes_are_refused`, `every_answer_carries_the_site_headers`, `a_navigation_to_another_origin_is_blocked`, `a_cdn_script_is_blocked_by_origin`.
- [ ] `just test-affected`; fail; implement; pass.
- [ ] Commit: `feat(bot): serve a clax to a browser from a synthetic origin`

### Task 6: Options and steps

**Files:** create `crates/clax-bot/src/browser/opts.rs`.

**Interfaces produced:** `BrowserTarget`, `BrowserOpts`, `Step`, `Viewport` parsed from the call's JSON with every cap of the Global Constraints, refusing with `invalid_args` naming the field; `Step::is_assertion()`; the JavaScript each step evaluates (`click` returns the centre and the element on top, `wait` a promise over `MutationObserver` and animation frames bounded in the page, `expect` the checks of spec §5.2) as functions returning strings, unit-tested as text; the `press` key table.

- [ ] **Tests first:** `viewport_presets_have_their_sizes`, `a_custom_viewport_is_bounded`, `too_many_steps_is_invalid_args`, `an_unknown_step_names_itself`, `tests_are_refused_under_browse`, `timeout_s_is_capped_by_the_grant`, `press_knows_enter_tab_escape_and_arrows`.
- [ ] `just test-affected`; fail; implement; pass.
- [ ] Commit: `feat(bot): browser test options and steps`

### Task 7: One test, open to close

Addresses spec threads `d3100720` (Browser.close on every path), `2c32e212` (preamble: `RTCPeerConnection`, CSP listener).

**Files:** create `crates/clax-bot/src/browser/session.rs`.

**Interfaces produced:** `async fn run_test(cdp: &mut Cdp, origin_or_url, pages: Option<&Pages>, opts: &BrowserOpts, serve: …, clock: &dyn Clock) -> TestRun` where `TestRun { result: Value (spec §5.1 minus `screenshots`, `cost`, `left`), shots: Vec<Vec<u8>>, opened_at, closed_at }`. The sequence of spec §4.3 exactly; the preamble (`window.name = "clax-test"` for `test`, `securitypolicyviolation` recorder, WebRTC removal); `Fetch.requestPaused` answered from `serve` while waiting on anything; settle; steps (action failure skips the rest, assertion failure does not); screenshots scaled to `RunLimits::image_side`; collect; `Target.disposeBrowserContext`, `Browser.close`, `close()` on every path; the caps and `truncated`.

- [ ] **Tests first** (against `FakeChrome`): `the_command_sequence_is_the_specs`, `a_failed_click_skips_the_rest`, `a_failed_expect_does_not_stop_the_list`, `console_and_errors_are_gathered_and_capped`, `a_covered_button_fails_its_click_naming_what_covers_it`, `a_full_page_screenshot_is_scaled_to_the_image_side`, `the_deadline_mid_step_answers_what_was_gathered`, `cancel_closes_the_browser`, `browser_close_is_sent_on_every_path`.
- [ ] `just test-affected`; fail; implement; pass.
- [ ] Commit: `feat(bot): drive one browser test over CDP`

### Task 8: The declaration, `OpShape` and the prompt

**Files:** modify `crates/clax-bot/src/clax.d.ts` (the `browser` namespace, its doc comment as rules, the interfaces of spec §5.1), `api.rs` (`OpShape { web: WebShape, browser: BrowserShape }`, `From<WebShape>`, `declared`, `dts`, `dts_with`), `prompt.rs` (`shape_of` reads `browser.*` from the binding), `clax.prompt.md` (`- {rules.browser}` after `{rules.web}`), `clax/session.rs` (`PARTS` gains `browser.test`, `browser.browse` → `Part::Browser`, answering `browser_disabled` until Task 9), `clax/provider.rs` (`standing` with `OpShape::default()`, `undeclared` covers `browser.*`); goldens under `crates/clax-bot/src/*.golden` and `tests/golden`.

- [ ] **Tests first:** in `api.rs`: `every_operation_of_the_file_is_an_op` (26 operations; `Spend` exactly `web.*` and `browser.*`), `the_browser_operations_fail_with_the_browser_codes`, `dts_declares_browser_only_with_its_shape`, `a_web_shape_alone_declares_what_it_did` (byte-for-byte with today's goldens); in `prompt.rs`: new goldens `prompt.system.thread.browser-test.golden` and `…browser-test-browse.golden`; `the_browser_rule_appears_only_with_the_namespace`.
- [ ] `just test-affected`; fail; implement; regenerate goldens the way `golden.rs` documents; review each diff by eye.
- [ ] Commit: `feat(bot): declare clax.browser by the run's shape`

### Task 9: The Spend path of `browser.*`

Addresses spec threads `c80fad7d`, `66b42c3c`.

**Files:** create `crates/clax-bot/src/clax/browser.rs`, `crates/clax-bot/tests/browser.rs`; modify `clax/session.rs`, `clax/provider.rs` (`browser: Option<Arc<dyn BrowserSource>>`, `with_browser`), `crates/clax-bot/tests/integration.rs` (`mod browser;`), `tests/common/mod.rs` (a helper running a script with a `FakeBrowserSource`).

**Behaviour:** the checks of `clax/web.rs`'s module docs in the same order, with the browser's grant, codes and line: replay from the journal; declared and a source; granted now; arguments (Task 6); the target exists (`browser_target_not_found`); cap (`bot_spend_counts(run, "browser")`); the line exists and the person can pay `seconds × rate` (`credits_exhausted`); the host's semaphore is the source's (`browser_busy` from `open`); open, `run_test` with `Pages::Draft` from the session's `Draft` (uncommitted edits included) or `Pages::Version`; keep each screenshot (`put_bot_content`, surface `clax`, `note_bot_content` with op and key); `Store::spend_journaled` with `page: None`, usage `Metered::new(LINE_BROWSER_TIME, ms_rounded_up_to_second, ks)` when a browser opened, the `bot.browser` audit record, the entry's `browser` summary (spec §11) and `counted`; answer with `screenshots` handles, `cost`, `left`.

- [ ] **Tests first** (`tests/browser.rs`, as `tests/web.rs` drives runs): `a_test_answers_and_is_charged_its_measured_time` (the fake advances `ManualClock` by 4.2 s → charged 5 s), `a_replayed_step_opens_no_browser`, `two_tests_in_one_script_run_one_after_the_other`, `the_test_loads_the_drafts_uncommitted_edit`, `a_version_target_loads_that_version`, `the_cap_and_credits_refuse_before_opening`, `a_failure_before_opening_is_not_charged_but_counts`, `a_mid_run_opt_out_answers_browser_disabled`, `returned_screenshots_are_shown_to_the_model`, `an_undeclared_browser_call_is_refused`.
- [ ] `just test-affected` (`//crates/clax-bot:integration`, `//crates/clax-bot:clax_bot_lib_test`); fail; implement; pass.
- [ ] Commit: `feat(bot): clax.browser.test, charged by browser time`

### Task 10: Saved tests: the bridge hook and the driver

Addresses spec threads `1c3fa7d3`, `005307b2`.

**Files:** modify `web/bridge/src/bridge.ts` (a top-level page whose `window.name === "clax-test"` gets `window.claxTest = Object.freeze({run})`, `run({files, testMs, totalMs})` loading the `test` part through `parts.test()` and answering `runTests`), its tests (a new `web/bridge/test/bridge-clax-test.test.ts`, beside `bridge-inspect.test.ts`); `crates/clax-bot/src/browser/session.rs` (with `opts.tests`, list `tests/*.test.js` of the target, at most 50, and evaluate `window.claxTest.run(…)` with `awaitPromise`, `returnByValue`; a missing hook is a result `tests: {passed: 0, failed: [{name: "tests", error: "the page has no Clax bridge"}], skipped: 0}`).

- [ ] **Tests first:** vitest `the_clax_test_window_gets_the_hook_and_others_do_not`, `the_hook_answers_test_results`; Rust `saved_tests_run_through_the_bridge_hook` (`FakeChrome` asserting the evaluated expression and answering `TestResults`), `a_failed_saved_test_fails_the_result`, `tests_true_lists_the_targets_test_files`.
- [ ] `just web-test` and `just test-affected`; fail; implement; pass. Measure `node web/scripts/bundle-size.mjs` and reply on thread `1014d9d7` with the numbers.
- [ ] Commit: `feat(bridge): let a browser test run an artifact's saved tests`

## Phase 2: The daemon

### Task 11: Share the Cloudflare credentials

**Files:** create `crates/clax-server/src/cloudflare.rs`; modify `crates/clax-server/src/web_source.rs`, `lib.rs`. A move with no behaviour change: `credentials()`, `usable()` and `has_credentials()` move; `DaemonWeb` calls them.

- [ ] Existing `web_source.rs` tests pass unchanged; add `cloudflare::tests::credentials_need_a_kek_an_account_and_a_token`.
- [ ] `just test-affected`.
- [ ] Commit: `refactor(server): share the Cloudflare credentials between web and browser`

### Task 12: `DaemonBrowser::open` over a WebSocket

Addresses spec threads `7f2fb7b4`, `d3100720`.

**Files:** modify `crates/clax-server/Cargo.toml` (`tokio-tungstenite` under `[target.'cfg(not(target_arch = "wasm32"))'.dependencies]` with its rustls feature; keep the ring provider: check `Cargo.lock` gains no `aws-lc-rs`), `Cargo.lock`, Bazel files `just bazel-sync` writes; create `crates/clax-server/src/browser_source.rs`.

**Interfaces produced:** `pub const BROWSER_RUN_PATH: &str = "browser-run"`; `DaemonBrowser::new(store, kek)`, test-only `sending_to(ws_base)`; `open`: credentials at each call, `try_acquire` on a semaphore sized from `browser.max_concurrent` (`browser_busy`), `GET {base}/client/v4/accounts/{account}/browser-run/devtools/browser?keep_alive=10000` upgraded with `Authorization: Bearer`, TLS from the web source's `fetch_tls()` (moved to be shared), 401/403 → not retryable "the token lacks Browser Rendering – Edit", 429 → retryable with `Retry-After`, 5xx and socket errors retryable; a `CdpTransport` over the stream; `deadline` from `clax_rt::time`.

- [ ] **Tests first** (in-module, a `tokio-tungstenite` server on 127.0.0.1): `open_sends_the_path_keep_alive_and_token`, `a_401_is_not_retryable_and_names_the_permission`, `a_429_is_retryable_with_retry_after`, `the_semaphore_refuses_past_max_concurrent`, `frames_go_both_ways`.
- [ ] `just bazel-sync`; `just test-affected`; fail; implement; pass.
- [ ] Commit: `feat(server): open Browser Run sessions over CDP`

### Task 13: The daemon's page files, headers and read-only API

**Files:** modify `crates/clax-server/src/browser_source.rs`, and where the site's serving lives (`crates/clax-server/src/routes/content.rs`, `clax_http::site_access`): expose the bridge and part files, and the site headers for an artifact, as functions both the site route and `DaemonBrowser` call (no copy of the CSP string).

**Interfaces produced:** `clax_file(path)` from the daemon's embedded web tree; `site_headers(artifact)` as the site sends them; `site_api(person, artifact, path_and_query)`: an in-process `GET` through the router's site API handlers acting as `person` on the artifact's site.

- [ ] **Tests first:** `the_bridge_and_its_parts_are_served`, `site_headers_are_the_sites` (equal to what a site response carries in `tests/api_content.rs`'s setup), `site_api_reads_as_the_person_and_refuses_another_artifact`.
- [ ] `just test-affected`; fail; implement; pass.
- [ ] Commit: `feat(server): give browser tests the site's files, headers and API`

### Task 14: Wire it: the run's shape, routes, config

**Files:** modify `crates/clax-server/src/routes/bot.rs` (`start_run` writes `NewRun::browser` from `BrowserAccess::now(...).shape()`; `me_view` gains `browser {on, offered, why_off, price_milli_per_minute}`; the Me `PUT` takes `browser`), `crates/clax-server/src/bot.rs` (`browser_configured`; the provider built `with_browser(DaemonBrowser)` on native), `routes/admin.rs` + `router.rs` (`GET`/`PUT /api/admin/browser`: settings, `configured`, `unavailable`, sources), `core.rs` + `crates/clax-core/src/config.rs` (`[bot.browser]` laid over the settings as `[bot.web]` is).

- [ ] **Tests first** (`crates/clax-server/tests/api_claxbot.rs` and `api_claxbot_run.rs`, with `DaemonBrowser` `sending_to` a fake CDP server): `admin_browser_round_trips_and_needs_an_admin`, `me_bot_shows_the_browser_switch`, `a_run_started_with_browser_on_declares_it`, `a_whole_browser_test_through_the_daemon` (a script calling `clax.browser.test()` against the fake server: fulfilled index, one screenshot, charged), `config_browser_overrides_the_setting`.
- [ ] `just test-affected`; fail; implement; pass.
- [ ] Commit: `feat(server): wire browser tests into runs, admin and me`

## Phase 3: What people see

### Task 15: The run panel's data and the content route

Addresses spec thread `72813432`.

**Files:** modify `crates/clax-bot/src/events.rs` (`with_web_fields` → `with_call_fields(v, cost_milli, extra)`, passing `browser` as `web` is), `crates/clax-server/src/routes/bot.rs` + `router.rs` (`GET /api/bot/runs/<run>/content/<id>`: whoever may view the run; image items of that run only; `Content-Security-Policy: sandbox; default-src 'none'`, `X-Content-Type-Options: nosniff`; 404 otherwise).

- [ ] **Tests first:** `events.rs` `a_browser_call_carries_its_summary_and_cost`; server `run_content_is_served_to_the_runs_viewers_only`, `a_non_image_item_is_not_served`.
- [ ] `just test-affected`; fail; implement; pass.
- [ ] Commit: `feat(server): show a run's browser tests and screenshots to its viewers`

### Task 16: The Browser admin page and the Me switch

**Files:** create `web/shell/src/bot/AdminBrowser.svelte`; modify `AdminPage.svelte` (a "Browser" tab after "Web access"), `MePage.svelte` ("Let ClaxBot test pages in a browser", with the price per minute), `api.ts` (`getAdminBrowser`, `putAdminBrowser`, the Me field), tests in `admin.test.ts`, `pages.test.ts`. The page shows whether the Cloudflare account is set (linking to Web access) and the token permission; the browse switch and rule carry the risk line of spec §9.4.

- [ ] **Tests first** (vitest): `the_browser_page_shows_and_saves_its_settings`, `browse_shows_its_risk_and_any_public_warns`, `the_me_switch_turns_the_browser_off`.
- [ ] `just web-test`; fail; implement; pass.
- [ ] Commit: `feat(shell): browser settings for admins and a switch for people`

### Task 17: The run panel's browser line

**Files:** modify `web/shell/src/bot/RunPanel.svelte`, `model.ts` (`browserLine(c)`: target, viewport, passed or failed, errors, console, time, cost), `run-panel.test.ts`.

- [ ] **Tests first:** `a_browser_call_shows_its_line_and_thumbnails`, `a_failed_test_reads_failed`.
- [ ] `just web-test`; fail; implement; pass.
- [ ] Commit: `feat(shell): browser tests in the run panel`

## Phase 4: Docs and the live test

### Task 18: Docs

**Files:** `docs/usage.md` (a paragraph after the web tools': what the admin turns on, the token permission, the cost, `[bot.browser]`, what a test can reach), `docs/contract.md` (ClaxBot routes: `/api/admin/browser`, the Me field, `/api/bot/runs/<run>/content/<id>`; the `bot.browser` and `me.browser` audit kinds), `crates/clax-worker/README.md` (browser use not configured on the Worker). Run `scripts/test-plugins.sh` through `just check`'s lane if `docs/contract.md`'s page-contract block is touched (it should not be).

- [ ] `just test-affected` (doc tests that read `docs/`).
- [ ] Commit: `docs(claxbot): browser testing for operators`

### Task 19: The opt-in live test

Addresses spec thread `7f2fb7b4` (the path's spelling is only checkable here).

**Files:** create `crates/clax-server/tests/live_browser_run.rs` (in the `integration` target), `justfile` (`browser-live`: runs that test with `--test_env=CLAX_LIVE_BROWSER_RUN_ACCOUNT --test_env=CLAX_LIVE_BROWSER_RUN_TOKEN` and the Bazel tags it needs for network).

**Behaviour:** without both variables the test returns at once, passing (CI runs it as a no-op); with them it runs one `clax.browser.test()` of a two-file artifact against real Browser Run through a scratch home and asserts a title, one screenshot and `Browser.close` answered. No wall-clock assertions.

- [ ] Run `just browser-live` with real credentials if available; else record that it was not run, in the commit message.
- [ ] Commit: `test(server): an opt-in test against real Browser Run`

## Phase 5: Browsing public pages (off by default; can be cut)

### Task 20: `clax.browser.browse`

Addresses spec threads `5ee4dbf8`, `7b40db95`, `e688825f`.

**Files:** create `crates/clax-bot/src/browser/browse.rs`; modify `clax/browser.rs`, `session.rs` (browse egress: continue public requests with `Fetch.continueRequest`, fail private literals, `localhost` and `OwnHosts`), `opts.rs` (`goto` under browse is a URL checked by the rule), `clax.d.ts` doc of `browse`, `tests/browser.rs`.

**Interfaces produced:** `BrowseRule::{Sender, Allowlist(Vec<String>), AnyPublic}`; `fn sender_urls(cx: &RequestContext) -> Vec<PublicUrl>` (from `from_sender` comments and the brief, by the browser's own scan; not `web::access::sources`); `fn allowed(url, rule, own_hosts, cx) -> Result<PublicUrl, BrowserError>`.

- [ ] **Tests first:** `browse_needs_its_own_switch_and_the_web_choice`, `sender_rule_admits_only_the_senders_urls`, `allowlist_matches_subdomains_with_a_star`, `own_hosts_and_private_literals_are_refused_under_every_rule`, `a_browse_session_blocks_private_requests_and_continues_public_ones`, `tests_are_refused_under_browse`, `browse_counts_against_its_own_cap`.
- [ ] `just test-affected`; fail; implement; pass.
- [ ] Commit: `feat(bot): browse public pages behind the admin's switch`

### Task 21: Browse in the UI and the docs

**Files:** `web/shell/src/bot/AdminBrowser.svelte` (browse section, done in Task 16 as fields; here enabled and tested end to end), `docs/usage.md` (browse and its risk), `crates/clax-server/tests/api_claxbot_run.rs` (`a_browse_run_with_the_switch_on_declares_browse`).

- [ ] `just test-affected`, `just web-test`.
- [ ] Commit: `feat(shell): turn browsing on for ClaxBot`

## Before merge

- [ ] `just check` once (in the background, logged under `target/`), every lane green; a lane failing in untouched code is rerun alone and both results reported.
- [ ] `qualifier diff main`: every spec thread this plan addressed has a reply or a close by its own commit, with evidence and `--ref git:<sha>` in a separate commit; the two `status:needs-decision` threads (`78f39245`, `5ee4dbf8`) are left for the owner.
