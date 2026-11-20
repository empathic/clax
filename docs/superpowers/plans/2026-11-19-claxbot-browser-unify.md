# ClaxBot capabilities: unify web and browser, browser sessions — Migration Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the branch's built code (the first plan,
`docs/superpowers/plans/2026-11-19-claxbot-browser.md`, all 21 tasks done)
to the revised design: five capabilities on one mechanism, `clax.web`
retrieval with an HTML format, and `clax.browser` sessions that live
across script steps, on a URL, the run's clax or supplied content,
charged by browser time, driven by a Playwright-shaped `Page`,
`Locator` and `expect` (spec §6.1, §6.5, §6.9 to §6.12; the owner's
sixth direction, decision `e3168e55`).

**Spec:** `docs/superpowers/specs/2026-11-19-claxbot-browser-design.md`
("spec §n"). Read it whole, and its open threads (`qualifier threads
docs/superpowers/specs/2026-11-19-claxbot-browser-design.md`), before
Task 1. The web tools' design is `2026-11-02-claxbot-web-tools-design.md`
("web §n").

**Architecture:** `clax-bot` gains `capability.rs` (the five capabilities,
`Capabilities::now`, `CapSet`, `UrlRule`) and `clax/spend.rs` (one Spend
sequence, a `Performer` per operation); `web/` and `browser/` become
performers on it. Browser sessions live in a host-owned registry
(`browser/live.rs`) of tasks that own their sockets; locators resolve
and actions auto-wait in `browser/locate.rs` and `browser/act.rs`.
The sandbox's shared prelude (`crates/clax-bot-sandbox/src/prelude.js`)
gains the `Page` layer over six flat `browser.*` operations, and
`browser/layer.rs` is the one table the model's declaration is
generated from and the prelude's table is checked against. `clax-store` gains
`bot_cap_grants`, `bot_runs.caps`, `bot_browser_sessions` and the
per-second line, all in the branch's own migration 67, edited in place.

**Tech Stack:** Rust (clax-bot, clax-store over SQLite, clax-server on
axum, tokio-tungstenite), TypeScript (`web/bridge`), Svelte 5
(`web/shell`, vitest), Bazel.

## Global Constraints

- Everything in `AGENTS.md`. No `unsafe`, no hard links, no Perl, no new
  npm runtime dependency, no new Rust crate (the recording format needs
  none, spec §6.6).
- **Verification is narrow per task, full per phase.** `just
  test-affected` is broken at its bazel query step. Each task runs only
  the targets it names (the crates it touches), in the background,
  logged under `target/unify-task-N.log`, and reports the summary lines.
  The last task of each phase also runs the full suite, logged under
  `target/unify-phase-X.log`:

      bazel test //crates/clax-bot/... //crates/clax-store/... //crates/clax-server/... //crates/clax-cli/... //crates/clax-core/... //bazel/wasm:check //web:unit //web:lint //web:typecheck //web:bundle_size //apps/clax_app:prompt_sync

  Never `--nocache_test_results`: a wrong cache hit is a BUILD-input bug
  to fix.
- Commit each task alone with `git -c commit.gpgsign=false commit`, a
  conventional subject, no attribution lines; never push.
- **Every task compiles and passes on its own.** Where an old API has
  callers a later task moves, the task keeps it as a shim over the new
  one; Task 20 removes the shims.
- **Migration 67 is the branch's own** (unmerged; `AGENTS.md`): edit it
  in place, never add 68. A dev home an earlier build migrated is reset,
  not upgraded. Main's data (`bot_person_prefs.web`, `bot_runs.web`,
  `bot_web_pages`, `web.*` settings, `bot.web` and `me.web` audit rows)
  must read the same after 67; each task that touches it has a migration
  test from a main-shaped database (migration 66 state).
- Before editing a file, `qualifier threads <path>`. Record what the next
  person needs; more than two writes in one `record --stdin` batch,
  dry-run first, the file outside the repository.
- Tests: no sleeps, no wall-clock budgets; `ManualClock`,
  `FakeBrowserSource`'s manual deadline. A new `#[cfg(test)]` module
  carries `#[warn(clippy::disallowed_methods)]`.
- "ID" in prose, never "id".

## Exact values (spec)

Capabilities `web.search`, `web.fetch`, `browser.open.url`,
`browser.open.artifact`, `browser.open.content`. Settings and defaults:
spec §11.1 table. `browser.time`: unit `browser_s`, base 1,000 (ms), 25
micro-USD a second. Session: idle 300 s (60–1,200), lifetime 1,800 s
(60–7,200), call 60 s (10–120), open per run 1 (1–4), concurrent 2
(1–50). `keep_alive=60000`; heartbeat after 20 s without a command.
Content: `html` ≤ 2 MiB, `files` ≤ 100 and ≤ 16 MiB. Recording 1–10 s,
≤ 8 frames a second, 4 frames shown. Content origin
`https://<session ID>.clax-content.invalid`; artifact origin unchanged.

## Review Focus

Inputs the spec implies and no requirement names. Each has a test in the
task that owns it.

1. **A person who turned main's web switch off** holds no runs grant
   of either web capability after migration 67, though the instance-wide
   grant exists, and their runs declare neither (Task 1,
   `migration_67_writes_main_web_as_explicit_grants`).
2. **A journal from main with `format: "auto"`** answers its window from
   the page store as `markdown` (Task 8, `an_auto_fetch_from_main_answers_from_the_store`).
3. **A session opened in step 1 and clicked in step 3, step 2 a model
   turn longer than `keep_alive`**: the heartbeat kept it, the click is
   charged from the open's end to its own end (Task 14,
   `a_session_lives_across_steps_and_its_time_is_charged_once`).
4. **Step 2 replayed after a crash**: every journaled call answers from
   the journal; the first un-journaled call answers
   `browser_session_lost` (Task 14, `a_replay_after_a_restart_answers_lost`).
5. **The idle end fires while no step is open**: the tail is charged
   with no step, once (Task 9, `a_host_end_with_no_open_step_charges_once`).
6. **A `goto` out of an artifact session**: refused without
   `browser.open.url`; with it, the session converts once, the
   synthetic origin answers `why: "left"`, `runTests` answers
   `browser_disabled` (Task 11, `a_goto_out_converts_the_session_once`).
7. **The person revokes `browser.open.content` mid-session**: the
   session closes before `PUT /api/me/bot` answers, and its next call is
   `browser_session_closed` `revoked` (Task 17,
   `a_revoke_closes_the_session_before_it_answers`).
8. **`open({html})` with neither `web.fetch` nor `browser.open.url`**
   works, and its page's request to another origin is blocked (Task 14,
   `content_opens_without_any_web_capability`).
9. **A script that replaced `Array.prototype.push` and
   `JSON.stringify` before calling `page.getByRole(...).click()`** sends
   the same `browser.act` call as one that did not (Task 13,
   `a_page_ignores_replaced_builtins`).
10. **A replayed step whose `expect` waited 3 s live** answers from the
   journal at once with the same pass, and charges nothing (Task 14,
   `a_replayed_expect_answers_at_once`).
11. **A locator matching two buttons** fails `click` at once with a
   strict-mode message naming both, as an answer journaled and charged,
   thrown by the prelude as an `Error`; the session stays open (Tasks
   12 and 14, `two_matches_are_a_strict_mode_violation`).
12. **A run holding `browser.open.content` alone** is declared no
   `runTests`, and its `BrowserSource` lists only `{html, files?}`
   (Task 15, `the_declaration_holds_only_the_runs_methods_and_kinds`).

## Threads

Spec decisions are qualifier records on the spec; each task names those
it builds. Open threads on code the plan touches, to account for:

- `369f1a79` (crash not noticed), `11b7cb34` (no auto-attach),
  `214066ff` (iframes and workers bypass a browse's policy), `9cf3f36d`
  (popup closing rests on `waitForDebuggerOnStart`): Task 10.
- `ee9cc794` (no Browser Run session ID in `bot.browser`): unchanged by
  this plan; Task 17 keeps the record's question open.
- `a395ac41` (screenshots scaled to the default image side, not the
  run's): Task 14 passes `RunLimits::image_side` to the registry.
- `a602eba5`, `70a0f3e0`, `285218bc` (`clax/web.rs`): Task 6 keeps each
  behaviour as it is and re-anchors their records onto `clax/spend.rs`.
- `c56c644b` (a browse's own time cap): Task 3 replaces both caps with
  `browser.call_seconds`; reply on the thread.
- `445dba5f` (the Me page's browser switch speaks only of testing): Task
  7 replaces the switch; reply with the commit.
- `840f091e` (where the web namespace's rule lives): Task 5 keeps the
  namespace doc comment as the rule.
- `7166a9f8` (a call's `ms` is always 0 in run events): Task 18 takes a
  session call's `ms` from its journal entry, not the run loop's record.
- The Playwright-shaped API's spec records: decisions `e3168e55`
  (owner), `0a68bd34`, `a6a4be11`, `0ea8fdf2`, `aeb201b7`, `d0c458ee`,
  `b103bb09`, `64081c34`, `3e3c85fa`, `a978ffa2`; risks `f83e9b51`
  (prelude budget, prompt size: Task 13, Task 15), `c2b4608a`
  (getByRole: Task 12), `7bda2d4c` (actionability: Task 12),
  `a1237142` (`Function.prototype.toString` in QuickJS: Task 13);
  alternatives `3cb2a1f8`, `306b1499`, `dcb76129`, `8c891ad2`,
  `d8f7f6d3`, `da63457e`. Each task names those it builds.

## File Structure

| File | Change |
|---|---|
| `crates/clax-store/src/migrations.rs` | Migration 67 in place: `bot_cap_grants`, `bot_runs.caps`, `bot_browser_sessions`, `bot_web_pages` with `format`, `browser_s` |
| `crates/clax-store/src/bot_web.rs` | Person grants per capability; `Spend.session`; session rows; `end_browser_session`, `lose_open_browser_sessions` |
| `crates/clax-store/src/bot_runs.rs` | `Run::caps`, `NewRun::caps`, `Bindings::clax(artifact, caps)` |
| `crates/clax-store/src/bot_settings.rs` | Browser settings per capability; `UrlRule` kind gains `sender` |
| `crates/clax-store/src/credits.rs` | `Unit::BrowserSecond`, `browser.time` per second |
| `crates/clax-core/src/audit/mod.rs` | `bot.spend`, `bot.grant`, `browser.session.end` (an op of `bot.spend`) |
| `crates/clax-bot/src/capability.rs` (new) | `Capability`, `CapSet`, `Capabilities::now`, `Off`, `UrlRule`, shared codes |
| `crates/clax-bot/src/clax/spend.rs` (new) | The one Spend sequence; `Performer` |
| `crates/clax-bot/src/clax/web.rs`, `clax/browser.rs` | Performers |
| `crates/clax-bot/src/web/page.rs` | `clean_html`; formats |
| `crates/clax-bot/src/browser/live.rs` (new) | `BrowserSessions`, the session task |
| `crates/clax-bot/src/browser/session.rs` | `run_test` split into open, act, collect, close primitives |
| `crates/clax-bot/src/browser/locate.rs`, `act.rs` (new) | Locator steps resolved per call; actionability, strictness, input, `expect`'s retries |
| `crates/clax-bot/src/browser/layer.rs` (new) | The `Page`/`Locator`/`expect` method table: declaration, `args` checks, the prelude's table checked |
| `crates/clax-bot/src/browser.examples.ts` (new) | Playwright-style scripts the generated declaration must compile |
| `crates/clax-bot-sandbox/src/prelude.js`, `lib.rs`, `conformance` | The `Page` layer, its table, the global `expect`; the budget test |
| `crates/clax-bot/src/browser/serve.rs` | `ContentSite`; the left origin |
| `crates/clax-bot/src/browser/opts.rs` | `BrowserSource` parsing; viewports; `BrowserTestOpts` |
| `crates/clax-bot/src/browser/record.rs` (new) | Screencast frames and the clip manifest |
| `crates/clax-bot/src/clax.d.ts`, `api.rs`, `prompt.rs`, `clax/session.rs`, `clax/provider.rs`, `events.rs` | Declaration by `CapSet`, the new operations, the registry |
| `crates/clax-bot/tests/browser.rs`, `tests/web.rs` | Run-level tests |
| `crates/clax-server/src/browser_source.rs`, `bot.rs`, `core.rs`, `routes/admin.rs`, `routes/bot.rs` | keep-alive, the registry, grants, routes |
| `crates/clax-core/src/config.rs` | `[bot.browser]` keys |
| `crates/clax-worker/src/web_source.rs` (`BindingSource`) | `html` format |
| `web/shell/src/bot/` | `CapabilityCard.svelte` (new), `AdminWeb.svelte`, `AdminBrowser.svelte`, `MePage.svelte`, `RunPanel.svelte`, `model.ts`, `api.ts` |
| `docs/usage.md`, `docs/contract.md`, `crates/clax-worker/README.md` | Docs |

---

## Phase A: the shared mechanism

### Task 1: Explicit grants and a run's capabilities, in migration 67

Builds spec decisions `c8a7f9b5` (grants are explicit, none implied:
the owner's standing rule), `a4d4b502` (grant storage), `df067c3c`
(declared at the start).

**Files:** `crates/clax-store/src/migrations.rs`, create
`crates/clax-store/src/bot_grants.rs`, `lib.rs`, `bot_web.rs`,
`bot_runs.rs`; `crates/clax-core/src/audit/mod.rs`; toolpath expected
files the audit kind changes.

**Interfaces produced:**
- Migration 67 adds `bot_cap_grants (id TEXT PRIMARY KEY, capability
  TEXT NOT NULL, granter TEXT NOT NULL, grantee TEXT NOT NULL, parent
  TEXT REFERENCES bot_cap_grants(id), status TEXT NOT NULL CHECK (status
  IN ('issued', 'revoked')), issued_at, issued_by, revoked_at,
  revoked_by)` with indexes by grantee and by parent. Its Rust step, for
  each of `web.search` and `web.fetch` whose `web.<op>.enabled` setting
  is on: one grant to `everyone` (granter `migration`), and one child to
  `runs_of:<person>` for the owner and every person row, except those
  whose `bot_person_prefs.web` is 0. Nothing for an instance whose
  switch is off; nothing for any `browser.open` kind. It adds
  `bot_runs.caps TEXT NOT NULL DEFAULT ''`, filled from `bot_runs.web`
  (`search` → `web.search`, `fetch` → `web.fetch`) and the branch's
  `bot_runs.browser` (`test` → `browser.open.artifact`, `browse` →
  `browser.open.url`). The branch's `bot_person_prefs.browser` and
  `bot_runs.browser` stay until Task 20.
- `Store::issue_cap_grant(ctx, capability, granter, grantee, parent) ->
  Result<GrantRow>` (an instance-wide grant issues every person's child
  in its transaction; refused when the granter does not hold the
  capability), `Store::revoke_cap_grant(ctx, id) -> Result<Vec<String>>`
  (the grant and its live descendants, transitively; the IDs ended),
  `Store::cap_grants(filter)`, `Store::person_caps(person) ->
  BTreeMap<String, PersonCap { granted_by: Option<id>, runs_grant:
  Option<id> }>`; a person row created issues the children of live
  instance-wide grants. Each write records `bot.grant {grant,
  capability, granter, grantee, parent, op}` (`AuditKind::BotGrant`).
- Shims: `web_choice` is true when both web capabilities reach the
  person's runs; `set_web_choice` revokes or issues both runs grants;
  `browser_choice`/`set_browser_choice` likewise for the three
  `browser.open` kinds.
- `Run::caps: String`, `NewRun::caps`; `Run::web` and `Run::browser`
  derived from `caps` (shims).

- [ ] **Tests first** (in-module): `migration_67_writes_main_web_as_explicit_grants` (a migration-66 database with web on, persons `web = 0`, `web = 1` and no row: the first has no child, the others do), `migration_67_grants_nothing_where_web_was_off`, `migration_67_grants_no_browser_kind`, `an_instance_wide_grant_issues_a_child_to_each_person_and_to_a_new_one`, `revoking_a_grant_ends_its_children_transitively`, `a_person_revokes_and_reissues_only_their_own`, `a_person_cannot_issue_what_they_do_not_hold`, `every_issue_and_revoke_is_one_bot_grant`, `migration_67_fills_run_caps_from_web_and_browser`, `a_run_keeps_its_capabilities`.
- [ ] Run `bazel test //crates/clax-store:clax_store_lib_test`; expect failures.
- [ ] Implement.
- [ ] Full verification command passes.
- [ ] Commit: `feat(store): explicit capability grants and a run's capabilities`

### Task 2: The per-second line, the browser settings per capability, `bot.spend`

Builds `d848c013` (one browser line), `63bbbbab` (settings).

**Files:** `crates/clax-store/src/credits.rs`, `migrations.rs` (67's
`cost_lines` CHECK and seed), `bot_settings.rs`;
`crates/clax-core/src/audit/mod.rs`; `web/shell/src/bot/model.ts`
(`UNITS`: "browser second").

**Interfaces produced:**
- `Unit::BrowserSecond` (`browser_s`, base 1,000) replacing
  `Unit::BrowserHour`; `seed_browser_line` at 25 micro-USD. Credits for a
  given number of milliseconds are unchanged (the rate per millisecond
  is the same); assert it.
- New `Setting` variants for every browser key of spec §11.1
  (`browser.open.url.enabled`, `.url_rule` (kind `UrlRule`, default
  `sender`), `.allowlist`, `.max_per_run`; `browser.open.artifact.enabled`,
  `.max_per_run`; `browser.open.content.enabled`, `.max_per_run`;
  `browser.call_seconds`; `browser.session.idle_seconds` (new kind
  `SixtyTo1200`); `browser.session.max_seconds` (new kind
  `SixtyTo7200`); `browser.max_open_per_run` (new kind `OneTo4`)). The
  `UrlRule` kind accepts `sender` (`web.fetch.url_rule` too). The first
  version's `browser.*` variants stay until Task 3 moves their readers.
- `AuditKind::BotSpend = "bot.spend"`, unused until Task 6.

- [ ] **Tests first:** `browser_s_round_trips_with_base_1000`, `a_second_costs_what_a_thousandth_of_an_hour_cost` (same credits for 4,200 ms as before), `the_capability_settings_default_off`, `url_rule_accepts_sender_for_fetch_and_open`, `bad_session_times_are_refused` (59, 1,201, 7,201, 0, 5).
- [ ] `bazel test //crates/clax-store/...`; fail; implement; full verification.
- [ ] Commit: `feat(store): price browser time per second, settings per browser capability`

### Task 3: `capability.rs`: one way to compute a run's capabilities

Builds `d163aa82` (one function), `df067c3c`. Replies on `c56c644b`.

**Files:** create `crates/clax-bot/src/capability.rs`; modify
`crates/clax-bot/src/lib.rs`, `web/access.rs`, `browser/access.rs`,
`clax/web.rs`, `clax/browser.rs` (call sites only),
`crates/clax-store/src/bot_settings.rs` (remove the first version's
`browser.*` variants), `crates/clax-server/src/routes/bot.rs`
(`start_run` writes `NewRun::caps` from `Capabilities::now(...).live()`),
`crates/clax-server/src/routes/admin.rs` (`/api/admin/browser` reads and
writes the new keys, its JSON shape unchanged until Task 7),
`crates/clax-core/src/config.rs` (`[bot.browser]` maps to the new keys).

**Interfaces produced:**
- `enum Capability { WebSearch, WebFetch, OpenUrl, OpenArtifact,
  OpenContent }` with `id()` (`"web.search"`, …), `family()` (`Web`,
  `Browser`), `ALL`.
- `struct CapSet` (a set of `Capability`), `parse`/`as_str` (the
  `bot_runs.caps` text, sorted, unknown parts ignored).
- `enum Off { NotOffered, NotGranted, PersonOff, NotConfigured }` (spec §4.3); `enum Terms {
  Search(SearchGrant), Fetch(FetchGrant), Open(OpenGrant) }` with
  `OpenGrant { kind, max_per_run, url_rule: Option<UrlRule> }` and
  `BrowserTerms { call_seconds, idle_seconds, max_seconds,
  max_open_per_run }`.
- `Capabilities::now(store, person, configured: &dyn Fn(Capability) ->
  bool) -> Result<Capabilities>`; `get(cap) -> Result<&Terms, Off>`;
  `live() -> CapSet`; `browser() -> Option<BrowserTerms>`.
- `WebAccess::now` and `BrowserAccess::now` become thin views over it
  (shims, removed in Task 20).

- [ ] **Tests first** (in-module): `each_capability_is_on_and_off_on_its_own` (all five, admin and person), `the_admin_reason_comes_before_the_persons`, `not_configured_per_capability`, `an_offered_capability_with_no_grant_is_not_granted`, `a_revoked_parent_leaves_the_run_nothing`, `caps_round_trip_through_their_text`, `open_artifact_needs_no_web_capability`.
- [ ] `bazel test //crates/clax-bot:clax_bot_lib_test //crates/clax-server/...`; fail; implement; full verification.
- [ ] Reply on `c56c644b`: one per-call cap, `browser.call_seconds`, with the commit.
- [ ] Commit: `feat(bot): compute a run's capabilities one way, per capability`

### Task 4: One URL rule

Builds `2d74a750` (as changed), `e601beb4`.

**Files:** `crates/clax-bot/src/capability.rs` (move `UrlRule` here from
`web/access.rs`, add `Sender`), `web/access.rs` (`sources()` unchanged;
`admit(url, rule, cx, journal, pages, own) -> Result<PublicUrl,
(code, message)>` shared), `browser/browse.rs` (`BrowseRule` removed;
`BrowseScope` built from `UrlRule` and the shared `admit`; a URL with no
source answers `url_not_in_prior_context`), `clax/web.rs`,
`clax/browser.rs`.

- [ ] **Tests first:** `sender_admits_only_the_senders_urls` (for fetch and for open), `sourced_admits_search_and_fetched_links_for_the_browser_too`, `allowlist_needs_a_source_and_a_listed_host`, `any_public_admits_any_public_url`, `every_rule_refuses_own_hosts_private_literals_and_credentials`, `an_unsourced_browser_url_is_not_in_prior_context`.
- [ ] `bazel test //crates/clax-bot/...`; fail; implement; full verification.
- [ ] Re-anchor `a17bd8e0`, `4d17eb43`, `47639fbf`, `7f838801` (`browse.rs`) if their spans drift.
- [ ] Commit: `refactor(bot): one URL rule for fetch and the browser`

### Task 5: Declare by capabilities

Builds `df067c3c`. The browser namespace stays the first version's
(`test`, `browse`) until Task 15 replaces it with the `Page`
declaration; this task makes only its presence follow the run's
capabilities.

**Files:** `crates/clax-bot/src/api.rs` (`OpShape` replaced by
`CapSet`: `declared(name, &CapSet)`, `dts(&CapSet)`), `prompt.rs`
(`shape_of` reads the binding's capabilities; the browser rules name the
run's source kinds), `crates/clax-store/src/bot_runs.rs`
(`Bindings::clax(artifact, &str caps)` grants the operations of spec
§6.1's rule), `clax/provider.rs`, goldens under `crates/clax-bot/src/`
and `tests/golden`.

- [ ] **Tests first:** `a_web_capability_set_declares_what_it_did` (byte for byte with today's web goldens), `browser_namespace_needs_an_open_capability`, `test_needs_the_artifact_capability`, `the_browser_rules_name_the_runs_source_kinds`.
- [ ] Regenerate goldens as `golden.rs` documents; review every diff by eye.
- [ ] Verify: `bazel test //crates/clax-bot/... //crates/clax-store/... //apps/clax_app:prompt_sync`.
- [ ] Commit: `feat(bot): declare operations by the run's capabilities`

### Task 6: One Spend path, one error style, one audit kind

Builds `a95888d5`, `b4b539ce`, `bc335300`.

**Files:** create `crates/clax-bot/src/clax/spend.rs`; modify
`clax/web.rs` and `clax/browser.rs` (each becomes `Performer`s:
`Search`, `Fetch`, `Test`), `clax/session.rs` (`Part::Spend`),
`capability.rs` (shared codes `CODE_CREDITS_EXHAUSTED`,
`CODE_URL_NOT_ALLOWED`, `CODE_URL_NOT_IN_PRIOR_CONTEXT`; errors carry
`capability`), `crates/clax-store/src/audit_query.rs` (a `bot.spend`
filter also matches `bot.web`), `docs/contract.md` (the audit kinds).

**Interfaces produced:** `trait Performer { fn capability(&self, args)
-> Capability; fn check_args; async fn admit; fn free_answer; fn
count_key; fn price_cap; async fn perform; fn shown }`; `spend::run(session,
call, &dyn Performer) -> Answer` doing spec §4.6's ten steps in order.
Both families write `bot.spend` with `capability`; the `HostError` of
every family carries `capability`. A performer's `perform` may answer
`ok: false` (spec §4.7: a failure on the page is an answer), journaled
and charged as any answer: Task 14's session performers rely on it.

- [ ] **Tests first:** the existing `tests/web.rs` and `tests/browser.rs` suites pass unchanged except the audit kind; `the_check_order_is_one_for_every_performer` (a table test driving each performer to each refusal in order); `every_call_writes_one_bot_spend`; `a_bot_web_row_is_found_by_the_bot_spend_filter`; `an_ok_false_answer_is_journaled_and_charged`.
- [ ] Verify: `bazel test //crates/clax-bot/... //crates/clax-store/...`.
- [ ] Re-anchor `a602eba5`, `70a0f3e0`, `285218bc` onto `clax/spend.rs` where the behaviour moved.
- [ ] Commit: `refactor(bot): one Spend path for every capability`

### Task 7: Grants in the admin pages and on the Me page

Builds `c8a7f9b5`, `a4d4b502`, `63bbbbab`. Replies on `445dba5f`.

**Files:** `crates/clax-server/src/routes/admin.rs` (`/api/admin/web`
gains each capability's `{enabled, max_per_run, url_rule?, allowlist?,
price, configured, unavailable}` beside its existing fields;
`/api/admin/browser` answers the three open capabilities and the
browser settings; `GET/POST/DELETE /api/admin/bot/grants` lists,
issues and revokes the admin's grants, everyone or a person),
`routes/bot.rs` (`GET /api/me/bot` `capabilities`,
still `web`; `PUT` takes `capabilities` and main's `web`),
`web/shell/src/bot/CapabilityCard.svelte` (new), `AdminWeb.svelte`,
`AdminBrowser.svelte` (each card lists its grants, "Everyone on this
instance" first, each revocable, and grants to everyone or a person),
`MePage.svelte` (one switch per capability granted to the person, with
its price; offered but not granted shows "not granted to you"), `api.ts`, `admin.test.ts`, `pages.test.ts`,
`crates/clax-server/tests/api_claxbot.rs`.

- [ ] **Tests first:** server `admin_browser_round_trips_each_capability`, `admin_web_keeps_its_fields_and_adds_the_cards`, `admin_grants_to_everyone_and_revokes`, `a_browser_kind_is_granted_to_no_one_until_the_admin_grants_it`, `me_bot_lists_each_capability_and_whether_it_is_granted`, `me_put_web_sets_both_web_capabilities`; vitest `each_capability_has_its_own_card`, `a_card_lists_and_revokes_its_grants`, `the_me_page_switches_one_capability`, `url_rule_any_public_warns`.
- [ ] Verify: `bazel test //crates/clax-server/... //web:unit //web:lint //web:typecheck`.
- [ ] **Phase A gate:** the full suite (Global Constraints), logged to `target/unify-phase-A.log`.
- [ ] Commit: `feat(shell): grant each capability on its own`

## Phase B: retrieval

### Task 8: Fetch as Markdown, HTML or the raw body

Builds `c5971758`.

**Files:** `crates/clax-bot/src/web/mod.rs` (`Format::{Markdown, Html,
Raw}`, `"auto"` parsed as `Markdown`), `web/page.rs` (`clean_html`:
html5ever, removing `script`, `style`, `noscript`, `template`,
comments, `on*` attributes; links absolute; unit-tested as text),
`clax/web.rs` (the page store per format), `crates/clax-store/src/bot_web.rs`
(`WebPageRow::format`, `web_page(run, url, format)`), `migrations.rs`
(67 rebuilds `bot_web_pages` with `format TEXT NOT NULL` in the primary
key, each row's format read from its fetch's journal entry,
`json_extract(result_json, '$.format')`, `auto` and missing as
`markdown`), `crates/clax-server/src/web_source.rs` (HTML body to
`clean_html`), `crates/clax-worker/src/web_source.rs` (`BindingSource`: the same, without
`toMarkdown`), `clax.d.ts` (`fetch`'s doc and type).

- [ ] **Tests first:** `clean_html_removes_code_and_keeps_structure`, `relative_links_become_absolute`, `html_of_a_non_html_type_is_unsupported`, `an_auto_fetch_from_main_answers_from_the_store`, `two_formats_of_one_page_are_two_requests`, `migration_67_keeps_every_page_with_its_format`; Worker: the html format in its sandbox test.
- [ ] Verify: `bazel test //crates/clax-bot/... //crates/clax-store/... //crates/clax-server/... //bazel/wasm:check`.
- [ ] **Phase B gate:** the full suite, logged to `target/unify-phase-B.log`.
- [ ] Commit: `feat(bot): fetch a page's HTML`

## Phase C: browser sessions

The order inside this phase: the store (9), the live session (10), its
sources (11), the driver's locators and actions (12) and the prelude's
layer (13) are independent of one another after 10; Task 14 joins
them into flat operations on the Spend path; Task 15 declares them;
Task 16 adds recording.

### Task 9: Browser time in segments

Builds `83274b43`; waiver `26eff349` stays.

**Files:** `crates/clax-store/src/migrations.rs` (67 adds
`bot_browser_sessions (run_id, session_id, kind, opened_at,
charged_ms, closed_at, reason, PRIMARY KEY (run_id, session_id))`),
`bot_web.rs`.

**Interfaces produced:** `Spend.session: Option<SessionSpend>` with
`SessionSpend::{Open {session, kind, opened_at, through_ms}, Call
{session, through_ms, kind: Option<kind>}, Close {session, through_ms,
reason}}`, applied in the Spend transaction: the usage row is
`through_ms - charged_ms` milliseconds of `browser.time`, refused
(`session_mismatch`) for a closed or missing session or a point before
`charged_ms`. `Store::end_browser_session(run, session, at_ms, reason)
-> Result<i64>`: charges the tail on the run's open step or on none,
writes `bot.spend` `op: browser.session.end`, sets `closed_at`, only
while open (else 0). `Store::lose_open_browser_sessions(at) ->
Result<u32>`. `Store::browser_session(run, session) ->
Result<Option<SessionRow>>`.

- [ ] **Tests first:** `segments_of_a_session_sum_to_its_open_time`, `a_replayed_call_charges_nothing`, `a_host_end_with_no_open_step_charges_once`, `a_second_end_charges_nothing`, `a_point_before_the_charged_one_is_refused`, `a_start_marks_open_sessions_lost`.
- [ ] Verify: `bazel test //crates/clax-store/...`.
- [ ] Commit: `feat(store): charge browser time in segments, once`

### Task 10: The live session

Builds `b7c5fd90`, `95e6aeb2`. Addresses `369f1a79`, `11b7cb34`,
`214066ff`, `9cf3f36d`.

**Files:** create `crates/clax-bot/src/browser/live.rs`; modify
`browser/session.rs` (split `run_test` into `open_page`, `act`,
`collect`, `close_page`, which `run_test` composes unchanged in
behaviour), `browser/cdp.rs` (crash and detach events end waits as
`CdpFail::Lost`), `browser/fake.rs` (`FakeChrome` emits auto-attach,
`Target.targetCrashed`, `Target.detachedFromTarget`; counts
`Browser.getVersion`).

**Interfaces produced:** `BrowserSessions::new(source, limits)`;
`open(run, kind, start, policy, viewport, deadline) -> Result<Handle,
BrowserError>`; `call(run, handle, call: SessionCall, draft) ->
Result<Answered, BrowserError>` (`browser_session_lost`,
`browser_session_closed`), where `SessionCall` is `Page {action, args}
| Act {locator, action, args} | Expect {locator, matcher, args}` (spec
§6.9) and the task hands each to a `SessionDriver` trait that Task 12
implements for `Act` and `Expect`; this task implements `Page` for
`goto`, `reload`, `title`, `content` and `evaluate` only, enough to
test the session; `close(run, handle, reason)`; `end_run(run)`;
`reevaluate(&dyn Fn(&RunId, kind) -> bool)`; `ended() -> Vec<Ended>`
(host ends for the store to charge). The task per session: serves
`Fetch.requestPaused` between calls, heartbeats after 20 s idle, fires
idle and lifetime by the source's deadline, ends as lost on crash or
detach. `Target.setAutoAttach {autoAttach: true, waitForDebuggerOnStart:
true, flatten: true}`: each attached iframe or worker gets
`Fetch.enable` (and, for a worker, WebSocket and WebTransport removed by
`Runtime.evaluate`) before `Runtime.runIfWaitingForDebugger`.

- [ ] **Tests first** (against `FakeChrome`): `requests_are_served_between_calls`, `the_heartbeat_is_sent_after_twenty_idle_seconds`, `the_idle_end_fires_by_the_deadline`, `the_lifetime_end_cuts_a_running_call`, `a_crashed_page_ends_the_session_as_lost`, `an_iframe_and_a_worker_are_held_to_the_policy`, `a_popup_is_closed_after_auto_attach`, `run_test_answers_as_before` (the existing session tests unchanged).
- [ ] Verify: `bazel test //crates/clax-bot:clax_bot_lib_test`.
- [ ] Reply on `369f1a79`, `11b7cb34`, `214066ff`, `9cf3f36d` with the tests and the commit (closes are the owner's).
- [ ] Commit: `feat(bot): browser sessions that outlive a step`

### Task 11: Three sources and leaving an origin

Builds `b2f849b9`, `ef116de5`.

**Files:** `crates/clax-bot/src/browser/serve.rs` (`ContentSite`:
`<session ID>.clax-content.invalid`, `/` and `files`, the site's CSP, no
`/_clax/`; `Site::left()`: every request to the origin failed `why:
"left"`), `browser/opts.rs` (`parse_source`: `{url}`, `{artifact:
"draft" | Int, path?}`, `{html, files?}` with the caps of spec §6.2;
`Image` handles resolved to content items), `browser/browse.rs`
(`BrowsePolicy` is the URL session's policy), `browser/live.rs` (a
`goto` out swaps the policy once and records the new kind).

- [ ] **Tests first:** `content_is_served_at_its_own_origin`, `content_reaches_nothing_else`, `content_files_take_text_base64_and_images`, `content_over_its_caps_is_invalid_args`, `a_goto_within_the_origin_needs_nothing`, `a_goto_out_converts_the_session_once`, `an_artifact_session_serves_the_draft_as_the_last_call_saw_it`.
- [ ] Verify: `bazel test //crates/clax-bot:clax_bot_lib_test`.
- [ ] Commit: `feat(bot): open the browser on a URL, the clax or supplied content`

### Task 12: Locators, actions and `expect` in the driver

Builds `0a68bd34` (locators and actions), `64081c34` (`expect` retried
in the browser), `a6a4be11` (evaluate in the page); spec §6.5, §6.10.
Accounts for risks `c2b4608a` (getByRole over the accessibility tree)
and `7bda2d4c` (actionability over CDP): a test pins each approximation
the risk names.

**Files:** create `crates/clax-bot/src/browser/locate.rs` (the
`LocatorStep` enum and its parse from JSON, at most 16 steps, `{regex,
flags}`; resolution per call: `{css}`, `{text}`, `{label}`,
`{placeholder}`, `{testId}`, `{nth}`, `{hasText}`, `{hasNotText}` by
`Runtime.callFunctionOn` over the scope's elements, `{role}` by
`Accessibility.queryAXTree` and `DOM.resolveNode`, names matched by the
driver; a description of the chain in Playwright's words,
`getByRole('button', { name: 'Save' })`) and `browser/act.rs` (each
locator action of spec §6.5's table with its actionability checks,
strictness, scrolling, `Input.dispatchMouseEvent`,
`Input.dispatchKeyEvent`, `Input.insertText`, the key table of
Playwright's names; every `expect` matcher of §6.10, its `.not`, its
in-browser retry; the page actions `keyboard.*`, `mouse.*`,
`setViewportSize`, `waitForLoadState`, `waitForURL`, `goBack`,
`goForward`, `consoleMessages`, `pageErrors`, `failedRequests`,
`screenshot` of a locator or the page; timeouts and their caps);
`browser/live.rs` (the `SessionDriver` for `Act` and `Expect`, and the
remaining `Page` actions); `browser/fake.rs` (`FakeChrome` keeps a small
DOM with boxes, an accessibility tree and a hit-test answer, and
records input events; no real browser).

**Interfaces produced:** `act::perform(cdp, scope, LocatorAction,
args, deadline) -> Answered { ok, value, error, timeout, url }`,
`act::expect(...)`, `act::page(...)`; failures as `ok: false` with
Playwright's message, never a `BrowserError` unless the session is lost.

- [ ] **Tests first** (against `FakeChrome`, with its manual deadline, no sleeps): `each_step_kind_resolves_as_the_spec_says`, `a_chain_scopes_its_next_step`, `role_names_match_as_substrings_unless_exact`, `two_matches_are_a_strict_mode_violation`, `click_waits_until_the_cover_is_gone`, `click_times_out_naming_the_cover`, `an_unstable_box_is_waited_for`, `fill_selects_and_inserts`, `count_and_is_visible_do_not_wait`, `expect_retries_in_the_browser_and_answers_once`, `expect_not_waits_for_the_condition_to_stop`, `to_have_text_with_an_array_matches_in_order`, `timeouts_are_capped_by_call_seconds`, `evaluate_calls_the_source_with_its_arg`, `a_regex_step_matches_in_the_page`.
- [ ] Verify: `bazel test //crates/clax-bot:clax_bot_lib_test`.
- [ ] Reply on `c2b4608a` and `7bda2d4c` with the tests that pin each approximation (closes are the owner's).
- [ ] Commit: `feat(bot): locators, auto-waiting actions and expect in the browser driver`

### Task 13: The prelude's `Page`, `Locator` and `expect`

Builds `b103bb09` (the layer), `64081c34` (the value matchers),
`a6a4be11` (sending a function), `0ea8fdf2` (console objects). Accounts
for risks `f83e9b51` (the budget) and `a1237142`
(`Function.prototype.toString` in QuickJS: settle it first; if the
native sandbox does not keep source, stop and record a blocker with the
failing case before building on it).

**Files:** `crates/clax-bot-sandbox/src/prelude.js` (the layer: the
method table as a JSON literal between `// browser-layer-table:begin`
and `:end` markers; `Page`, `Locator`, `Keyboard`, `Mouse`, `Response`,
`ConsoleMessage` and page-error objects with `toJSON`; `clax.browser`
as `{open, test}`, the flat `page`, `act`, `expect`, `close` kept in
the closure; the global `expect` with the locator, page and value
matchers; `page.url()` from the last answer; `setDefaultTimeout`;
installed only when the ops list holds the five session operations;
every built-in it uses captured before the script runs),
`crates/clax-bot-sandbox/src/lib.rs` (`PRELUDE`'s doc names the layer
and its install rule; `pub const PRELUDE_BUDGET_BYTES: usize = 24 *
1024`; conformance cases), `crates/clax-bot-js/tests/sandbox.rs` and
the Worker sandbox's conformance run (`crates/clax-worker/src/sandbox/tests.rs`).

Conformance cases use a recording host that answers each flat call from
a script; no browser.

- [ ] **Tests first** (`clax_bot_sandbox::conformance`, run by both sandboxes): `each_method_makes_its_one_flat_call` (a table-driven case over every row: method, arguments, the flat call expected), `locator_builders_and_url_make_no_call`, `an_ok_false_answer_throws_timeout_error_or_error`, `a_failed_matcher_throws_expected`, `value_matchers_agree_with_the_harness` (the cases of `web/bridge/test`'s `expect` table, copied as data), `evaluate_sends_a_functions_source`, `evaluate_refuses_a_native_function`, `a_page_ignores_replaced_builtins`, `the_layer_is_absent_without_the_session_ops`, `the_flat_ops_are_not_on_clax`, `returned_console_messages_show_their_data`; `the_prelude_stays_within_its_budget` (`PRELUDE.len() <= PRELUDE_BUDGET_BYTES`).
- [ ] Verify: `bazel test //crates/clax-bot-sandbox/... //crates/clax-bot-js/... //bazel/wasm:check`, and the Worker sandbox's test target if its `BUILD.bazel` has one.
- [ ] Reply on `a1237142` with the case that shows QuickJS keeps (or does not keep) function source; on `f83e9b51` with the prelude's size after the layer.
- [ ] Commit: `feat(sandbox): a Playwright-shaped Page, Locator and expect in the prelude`

### Task 14: The flat session operations on the Spend path

Builds `b103bb09` (the six operations), `d0c458ee` (`browser.test`
without steps), `0a68bd34` (failures are answers), `633b7b5e`,
`87804b1a`, `0a6381ce`; spec §6.2 to §6.4, §6.8, §6.9.

**Files:** `crates/clax-bot/src/clax.d.ts` (the flat `browser`
operations of spec §6.9: `open`, `page`, `act`, `expect`, `close`,
`test`, with `BrowserPageAction`, `BrowserLocatorAction` and
`BrowserMatcher` as unions of string literals, `BrowserLocatorStep`,
`BrowserAnswer`, `BrowserOpened`, `BrowserTestOpts`; `browse` and
`test`'s `steps`, `html`, `text` and `screenshots` removed),
`clax/browser.rs` (performers `Open`, `Page`, `Act`, `Expect`,
`Close`, `Test` on `spend.rs`: per-call segments through
`Spend.session`; the run's open sessions; per-kind caps; `left`; a
`credits_exhausted` refusal closes the session; each action's `args`
checked by `browser/layer.rs`'s entry for it, created here with the
`args` checks only, Task 15 adding signatures and docs),
`browser/opts.rs` (`BrowserTestOpts`; the steps parser removed),
`browser/session.rs` (`run_test` without steps), `clax/session.rs`
(`PARTS` for every new operation), `clax/provider.rs`
(`with_browser_sessions`; screenshots at `RunLimits::image_side`,
`a395ac41`), `crates/clax-bot/tests/browser.rs`, `tests/common/mod.rs`.

Run-level tests drive scripts through the prelude's `Page` (Task 13)
against `FakeBrowserSource`.

- [ ] **Tests first** (`tests/browser.rs`, through the run loop): `a_session_lives_across_steps_and_its_time_is_charged_once`, `a_replayed_step_opens_no_browser`, `a_replay_after_a_restart_answers_lost` (a new registry), `a_replayed_expect_answers_at_once`, `an_idle_session_answers_closed_idle`, `a_failed_click_is_journaled_and_thrown`, `two_matches_are_a_strict_mode_violation` (through the run), `content_opens_without_any_web_capability`, `an_open_of_a_kind_the_run_lacks_is_disabled`, `a_second_open_past_the_runs_limit_is_capped`, `credits_running_out_close_the_session`, `the_run_end_closes_its_sessions`, `test_runs_the_saved_tests_in_one_call`, `an_unknown_action_is_invalid_args`, `two_calls_in_one_promise_all_run_in_order`.
- [ ] Verify: `bazel test //crates/clax-bot/... //crates/clax-store/...`.
- [ ] Reply on `a395ac41` with the commit.
- [ ] Commit: `feat(bot): browser session operations behind the Page layer`

### Task 15: The declaration: `Page`, `Locator` and `expect`

Builds `3e3c85fa` (generated and checked), `e3168e55` (the owner's
decision: Playwright's names and signatures, only what works),
`a978ffa2` (what is not declared). Accounts for `f83e9b51`'s prompt
size: report the declaration's line and byte count in the commit
message.

**Files:** `crates/clax-bot/src/browser/layer.rs` (every row of spec
§6.1 and §6.10: class, method, TypeScript signature, one-line doc in
Playwright's meaning, flat operation and action or none, parameter
names, `args` check, capability needed beyond `browser.open.*`),
`api.rs` (`dts(&CapSet)` writes the `browser` namespace, `Page`,
`Locator`, `Keyboard`, `Mouse`, `Response`, `ConsoleMessage`,
`LocatorAssertions`, `PageAssertions`, `ValueAssertions` and the
`expect` overloads from the table, with only the run's methods and
source kinds, in place of the surface's flat `browser` operations),
`prompt.rs` (the namespace's rules of spec §6.1), `clax.d.ts` (the
`browser` namespace's doc comment), create
`crates/clax-bot/src/browser.examples.ts` (scripts as Playwright's docs
write them, using every declared method), `crates/clax-bot/BUILD.bazel`
(a `tsc --noEmit` test over the generated declaration and the examples,
with the repository's pinned TypeScript; if wiring it outside `web/` is
heavy, the same check inside `//web:typecheck` reading the golden),
goldens.

- [ ] **Tests first:** `every_row_has_its_action_in_the_flat_union_and_back`, `the_prelude_table_equals_the_layer_table` (reads `clax_bot_sandbox::PRELUDE` between the markers), `the_declaration_holds_only_the_runs_methods_and_kinds`, `no_flat_browser_operation_is_declared_to_the_model`, `the_declaration_and_examples_compile` (the TypeScript test), goldens per shape (`browser-artifact`, `browser-content`, `browser-url-artifact-content`).
- [ ] Regenerate goldens as `golden.rs` documents; review every diff by eye.
- [ ] Verify: `bazel test //crates/clax-bot/... //apps/clax_app:prompt_sync` and the TypeScript test's target.
- [ ] Commit: `feat(bot): declare the browser as Playwright's Page, Locator and expect`

### Task 16: Recording

Builds `c448d956`, `aeb201b7` (`page.record`, not `recordVideo`).

**Files:** create `crates/clax-bot/src/browser/record.rs`
(`Page.startScreencast {format: "jpeg", quality: 70, maxWidth,
maxHeight}`, `Page.screencastFrameAck` for each frame, at most 8 a
second kept, `Page.stopScreencast`; frames to content items, the
manifest item `application/vnd.clax.frames+json`); `browser/live.rs`
(the `record` page action); `browser/layer.rs` (its row, if Task 15
left it out); `crates/clax-store/src/bot_content.rs` (accept the
manifest's media type as a non-image item); the prelude's table row
for `Page.record` if Task 13 did not add it.

- [ ] **Tests first:** `every_frame_is_acknowledged`, `frames_are_kept_at_most_eight_a_second`, `four_evenly_spaced_frames_are_returned`, `the_manifest_names_each_frame_and_its_time`, `ten_seconds_is_the_most`, `page_record_makes_one_call` (conformance, if the row is new).
- [ ] Verify: `bazel test //crates/clax-bot/... //crates/clax-store/...` (and `//crates/clax-bot-sandbox/...` if the prelude changed).
- [ ] **Phase C gate:** the full suite, plus `//crates/clax-bot-sandbox/... //crates/clax-bot-js/...`, logged to `target/unify-phase-C.log`.
- [ ] Commit: `feat(bot): record a browser session as a frame sequence`

## Phase D: the daemon and people

### Task 17: Wire sessions into the daemon

Builds `df067c3c` (revoke), `95e6aeb2` (keep-alive).

**Files:** `crates/clax-server/src/browser_source.rs` (`KEEP_ALIVE_MS =
60_000`), `bot.rs` (the `Bot` owns one `BrowserSessions`; providers get
it; a task drains `ended()` into `Store::end_browser_session`),
`core.rs` (at start, `Store::lose_open_browser_sessions`),
`routes/admin.rs` and `routes/bot.rs` (every grant change calls
`BrowserSessions::reevaluate` with `Capabilities::now` before
answering), the run loop's end of a run (`end_run`),
`crates/clax-server/tests/api_claxbot_run.rs`, `api_browser_source.rs`.

- [ ] **Tests first:** `open_sends_keep_alive_60000`, `a_session_across_two_steps_through_the_daemon` (fake CDP server, a script using `page.getByRole(...).click()` in its second step), `a_revoke_closes_the_session_before_it_answers`, `an_admin_switch_off_closes_every_session_of_its_kind`, `a_restart_marks_sessions_lost`, `the_run_end_closes_and_charges`.
- [ ] Verify: `bazel test //crates/clax-server/...`.
- [ ] Comment on `ee9cc794`: the session ID is still not known; sessions make it more useful (one ID across many calls).
- [ ] Commit: `feat(server): hold browser sessions across steps on the daemon`

### Task 18: The run panel's sessions and clips

**Files:** `crates/clax-bot/src/events.rs` (a session call carries
`browser: {session, kind, op, action, locator, ms, ok}` and
`cost_milli`; `ms` from the journal entry, `7166a9f8`),
`crates/clax-server/src/routes/bot.rs` (`run_content` also serves a
frame manifest as JSON under the same headers),
`web/shell/src/bot/RunPanel.svelte`, `model.ts` (`sessionLine`,
grouping a session's calls; a call named as the script wrote it,
`getByRole('button', { name: 'Book' }).click()`, from its action and
locator), a flipbook component, `run-panel.test.ts`.

- [ ] **Tests first:** server `a_frame_manifest_is_served_as_json_to_the_runs_viewers`; vitest `a_session_is_one_line_with_its_calls`, `a_call_is_named_as_the_script_wrote_it`, `a_host_end_shows_its_reason`, `a_clip_plays_its_frames_in_order`.
- [ ] Verify: `bazel test //crates/clax-bot/... //crates/clax-server/... //web:unit //web:lint //web:typecheck //web:bundle_size`.
- [ ] Commit: `feat(shell): browser sessions and clips in the run panel`

### Task 19: Docs

**Files:** `docs/usage.md` (the five capabilities, their settings,
prices and risks; sessions and what they cost while a model thinks; the
token permissions; the script API is Playwright's subset, and where to
find what is declared), `docs/contract.md` (the admin and Me routes'
fields; `bot.spend`, `bot.grant`; `bot.web` and `me.web` as
historical; the content route's manifest), `crates/clax-worker/README.md`
(browser capabilities not configured; `html` fetch works).

- [ ] Verify: the doc tests that read `docs/`: `bazel test //crates/clax-server/... //crates/clax-cli/...`.
- [ ] Commit: `docs(claxbot): capabilities and browser sessions for operators`

### Task 20: Remove the first version's shapes

**Files:** `migrations.rs` (67 no longer adds `bot_person_prefs.browser`
or `bot_runs.browser`; its mark becomes `bot_browser_sessions`;
`MARKS_AFTER_APP` and the migration tests follow), `bot_web.rs`
(`browser_choice`, `set_browser_choice` removed; `web_choice` stays for
the Me route's `web` field), `bot_runs.rs` (`Run::browser` removed;
`Run::web` derived for the run JSON's `web` field only),
`crates/clax-core/src/audit/mod.rs` (`bot.browser` and `me.browser`
removed: no main data holds them), `web/access.rs` and
`browser/access.rs` (`WebAccess`, `BrowserAccess`, `WebShape`,
`BrowserShape` removed where nothing reads them), `browser/opts.rs` (any
step type left from the first version), `crates/clax-server/tests/live_browser_run.rs`
(one session across two calls with a pause longer than `keep_alive`,
by the heartbeat, driven through a `Page`; still skipped without
credentials), `justfile` (`browser-live` unchanged in form).

- [ ] **Tests first:** `migration_67_from_main_has_no_branch_columns`.
- [ ] Verify: `bazel test //crates/clax-bot/... //crates/clax-store/... //crates/clax-server/... //crates/clax-core/...`.
- [ ] **Phase D gate:** the full suite, logged to `target/unify-phase-D.log`.
- [ ] Commit: `refactor(bot): drop the first version's browser grant and shapes`

## Before merge

- [ ] `just check` once (background, logged under `target/`); a lane failing in code this branch did not touch is rerun alone and both results reported.
- [ ] `qualifier diff main`: every spec decision a task built has a reply naming its commit; closes of records other sessions wrote, and every `status:needs-decision` thread, are left for the owner.
- [ ] Run `just browser-live` with real credentials if the owner provides them; reply on `88b76c21` (path spelling) and `d48cf9c0` (keep-alive) with what it showed.

## Phase F: run state and session capabilities

The owner's decisions `70eababf` (a run-scoped `state`; an unforgeable
session capability; a session lives as long as its handle) and
`c2e00840` (the layer only in browser runs), as spec §17 sets them out.
Closes, once built: `36272d22` (`clax.browser.session(id)`, superseded)
and `f83e9b51` (the prelude's budget). Each task verifies its own
targets, logged under `target/unify-task-F<n>.log`; the last runs `just
check`.

### Task F1: the layer only in browser runs

Spec §17.2 (a `Page`'s JSON, no session from a string), §17.5. First,
because the prelude has 8 bytes of its budget left (`f42ca857`) and
`state`'s codec joins it. **Files:** `crates/clax-bot-sandbox/src/browser.js`
(new: the layer, moved out of `prelude.js`; `toJSON` of `"<browser
page>"` and `"<browser locator>"`; no `clax.browser.session`),
`prelude.js` (the marker line), `lib.rs` (`Script::browser`,
`prelude_source`, `BROWSER_LAYER`, `BROWSER_LAYER_BUDGET_BYTES`,
`browser_layer_table` from the layer), `conformance.rs` (browser cases
ask for the layer), both sandboxes (`prelude_source(script.browser)`),
`crates/clax-bot/src/script.rs` (a script asks for the layer when a
binding has the five session operations among its own).

- [ ] Tests first: `a_returned_page_shows_no_capability`, `the_layer_is_absent_without_browser`, `there_is_no_session_from_a_string`, `the_prelude_and_the_layer_stay_within_their_budgets`; run level: `a_run_without_a_browser_capability_has_no_page_layer`.
- [ ] Verify: `bazel test //crates/clax-bot-sandbox/... //crates/clax-bot-js/... //crates/clax-bot/... //bazel/wasm:check`.
- [ ] Commit: `feat(sandbox): load the Page layer only in browser runs`

### Task F2: `state` in the sandbox, handles in it

Spec §17.1, §17.2. **Files:** `crates/clax-bot-sandbox/src/prelude.js`
(the global `state`, restored from `options.state`, encoded when the
script ends and passed as `done`'s third argument; a hook the layer
gives its handles through), `browser.js` (its classes' handle entries,
restored live, one `Page` per handle), `lib.rs` (`Script::state`,
`ScriptOutcome::state`, `StateOut`, the prelude's documented contract),
`conformance.rs` (`Case::state`, `Expect::state`), `crates/clax-bot-js`
(the option in, the third argument out), `crates/clax-worker/src/sandbox`
(`job.state`, `final.done.state`, `driver.js`).

- [ ] Tests first (conformance, both sandboxes): `state_starts_empty_and_comes_back_as_left`, `state_is_kept_after_a_throw`, `state_is_replaced_by_an_object_and_refuses_anything_else`, `what_state_cannot_keep_is_listed_with_its_path`, `a_clax_member_is_not_kept`, `a_stopped_script_writes_no_state`, `a_page_and_its_locator_in_state_restore_live_as_one_page`.
- [ ] Verify: `bazel test //crates/clax-bot-sandbox/... //crates/clax-bot-js/... //crates/clax-worker/... //bazel/wasm:check`.
- [ ] Commit: `feat(sandbox): a run's state across scripts`

### Task F3: a run's state kept with its step

Spec §17.1. **Files:** `crates/clax-bot/src/script.rs` (the last
committed state into `Script::state`; the outcome's state capped at 64
KiB, largest members left out and listed; the payload's `state`; the
result's `state` only when something was not kept),
`crates/clax-server/src/routes/bot.rs` (`step_view`'s `state`, handles
shown opaque, the handle entries left out), `crates/clax-bot/tests/state.rs`
(new).

- [ ] Tests first: `a_steps_state_is_the_next_scripts`, `a_step_executed_again_starts_from_the_committed_state`, `a_state_over_its_cap_leaves_out_its_largest_members`, `a_stopped_script_keeps_the_last_state`, `a_result_says_nothing_of_a_state_kept_whole`; server: `a_steps_state_is_shown_without_its_capabilities`.
- [ ] Verify: `bazel test //crates/clax-bot/... //crates/clax-server/...`.
- [ ] Commit: `feat(bot): keep a run's state with its step`

### Task F4: a session's handle is an unforgeable capability

Spec §17.3. **Files:** `crates/clax-bot/src/browser/live.rs` (a 256-bit
handle drawn at the open, mapped to its session for its run;
`resolve`), `clax/browser.rs` (the open answers `page`; session calls
name their session by the handle, resolved at admission; shown, audit
and errors by the session's ID), `browser.js` (the `Page` keeps the
handle), `clax.d.ts` (the flat operations' `page` parameter),
`tests/browser.rs`.

- [ ] Tests first: `an_open_answers_a_handle_no_record_names`, `another_runs_handle_is_lost`, `an_unknown_handle_is_lost_and_not_echoed`; existing session tests through `state`.
- [ ] Verify: `bazel test //crates/clax-bot/... //crates/clax-bot-sandbox/... //crates/clax-server/...`.
- [ ] Commit: `feat(bot): a browser session's handle is an unforgeable capability`

### Task F5: a session lives as long as its handle

Spec §17.4. **Files:** `crates/clax-store/src/bot_web.rs`
(`SessionEnd::Dropped`), `crates/clax-bot/src/surface.rs`
(`SurfaceProvider::state_committed`), `script.rs` (after a script
step's commit, each binding's provider told the committed state's
handles), `clax/provider.rs` (closes the run's sessions not held,
`dropped`, and charges them), `browser/live.rs` (`close_unheld`),
`web/shell/src/bot/model.ts` (the reason's words), `tests/browser.rs`.

- [ ] Tests first: `a_page_in_state_lives_into_the_next_script`, `a_page_only_returned_closes_dropped_when_its_step_commits`, `a_dropped_session_is_charged_once`, `a_replayed_step_drops_the_same_sessions`.
- [ ] Verify: `bazel test //crates/clax-bot/... //crates/clax-store/... //crates/clax-server/... //web:unit`.
- [ ] Commit: `feat(bot): a browser session lives as long as its handle`

### Task F6: what ClaxBot is told

Spec §17.6. **Files:** `crates/clax-bot/src/prompt/standard.rs` (the
code-mode paragraph), `clax.d.ts` (the browser namespace's rules),
`browser/layer.rs` (`close`'s doc), goldens, the Clax app's prompt copy
(`//apps/clax_app:prompt_sync`).

- [ ] Regenerate goldens as `golden.rs` documents; read every diff.
- [ ] Verify: `bazel test //crates/clax-bot/... //apps/clax_app:prompt_sync //web:unit`.
- [ ] Commit: `feat(bot): tell ClaxBot of state and of pages it lets go`

### Task F7: the run panel shows a step's state

Spec §17.1. **Files:** `web/shell/src/bot/RunPanel.svelte`, `model.ts`,
`run-panel.test.ts`.

- [ ] Tests first (vitest): `a_script_step_shows_its_state`.
- [ ] Verify: `bazel test //web:unit //web:lint //web:typecheck //web:bundle_size`.
- [ ] `just check`, in the background, logged to `target/unify-phase-F.log`.
- [ ] Commit: `feat(shell): show a script step's state in the run panel`
- [ ] Close `36272d22` and `f83e9b51` with the tests and `--ref git:<sha>`, in a commit of their own.

## Phase G: browser capabilities as macaroons

The owner's decision "delegatable capabilities, use macaroons", as spec
§18 sets it out; it replaces §17.3's random handle (decision record
`7d29f5b7`). Each task verifies its own targets, logged under
`target/unify-task-G<n>.log`; the last runs `just check`.

### Task G1: the macaroon format

Spec §18.1. **Files:** `crates/clax-core/src/macaroon.rs` (new:
`Macaroon::mint`, `attenuate`, `verify` with `ring::hmac::verify`,
`encode`, `decode`, `derive_key`), `crates/clax-core/src/lib.rs`.

- [ ] Tests first: `a_macaroon_verifies_with_its_key_alone`, `any_change_fails_verification`, `an_appended_caveat_verifies`, `what_does_not_decode_is_refused`.
- [ ] Verify: `bazel test //crates/clax-core/...`.
- [ ] Commit: `feat(core): macaroons over HMAC-SHA256`

### Task G2: capabilities in the session registry

Spec §18.2 to §18.4, §18.6, §18.7. **Files:**
`crates/clax-bot/src/browser/cap.rs` (new: the identifier, the
caveats, `Use`, `Class`, `check`), `browser/layer.rs` (`class` of a row;
the `share` and `revoke` rows), `browser/live.rs` (the instance key, a
root minted at the open, `authorize`, `share`, `revoke`, `holds` and
`close_unheld` by it), `browser/mod.rs` (`browser_forbidden`).

- [ ] Tests first: `caveats_only_narrow`, `an_unknown_caveat_fails_closed`, `expiry_is_on_the_injected_clock`, `a_read_only_capability_cannot_act`, `a_forged_or_tampered_capability_is_lost`, `revoking_a_parent_revokes_its_children`, `closing_a_session_ends_every_capability`, `a_session_held_by_a_revoked_capability_is_dropped`.
- [ ] Verify: `bazel test //crates/clax-bot/...`.
- [ ] Commit: `feat(bot): a browser session's handle is a macaroon`

### Task G3: `page.share` and `page.revoke`

Spec §18.4, §18.5, §18.9. **Files:** `crates/clax-bot/src/clax/browser.rs`
(the two actions on the Spend path, uncharged; `browser_forbidden`;
their audit), `crates/clax-bot-sandbox/src/browser.js` (`share`
answering a `Page`, its display, `shown` in its handle entry),
`lib.rs`, `conformance.rs`, `crates/clax-bot/src/clax.d.ts`,
`tests/browser.rs`.

- [ ] Tests first: `a_shared_read_only_page_reads_and_cannot_click`, `a_replayed_share_answers_the_same_capability`, `share_and_revoke_are_audited_without_the_macaroon`, conformance `a_shared_page_shows_its_classes`.
- [ ] Verify: `bazel test //crates/clax-bot/... //crates/clax-bot-sandbox/... //crates/clax-bot-js/... //bazel/wasm:check`.
- [ ] Commit: `feat(bot): share a narrowed page, and revoke it`

### Task G4: the daemon's key from the KEK

Spec §18.2. **Files:** `crates/clax-store/src/secrets.rs`
(`Kek::derive`), `crates/clax-server/src/bot.rs` (`browser_sessions`
with the instance key).

- [ ] Tests first: `a_derived_key_depends_on_its_label_and_kek`.
- [ ] Verify: `bazel test //crates/clax-store/... //crates/clax-server/...`.
- [ ] Commit: `feat(server): browser capabilities keyed from the KEK`

### Task G5: what ClaxBot is told

Spec §18.10. **Files:** `clax.d.ts` (the browser namespace's rules),
`browser/layer.rs` (`share`'s and `revoke`'s declarations), goldens, the
Clax app's prompt copy.

- [ ] Regenerate goldens as `golden.rs` documents; read every diff.
- [ ] Verify: `bazel test //crates/clax-bot/... //apps/clax_app:prompt_sync`.
- [ ] `just check`, in the background, logged to `target/unify-phase-G.log`.
- [ ] Commit: `feat(bot): tell ClaxBot how to share a page`
