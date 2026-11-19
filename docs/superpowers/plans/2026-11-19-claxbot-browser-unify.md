# ClaxBot capabilities: unify web and browser, browser sessions — Migration Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the branch's built code (the first plan,
`docs/superpowers/plans/2026-11-19-claxbot-browser.md`, all 21 tasks done)
to the revised design: five capabilities on one mechanism, `clax.web`
retrieval with an HTML format, and `clax.browser` sessions that live
across script steps, on a URL, the run's clax or supplied content,
charged by browser time.

**Spec:** `docs/superpowers/specs/2026-11-19-claxbot-browser-design.md`
("spec §n"). Read it whole, and its open threads (`qualifier threads
docs/superpowers/specs/2026-11-19-claxbot-browser-design.md`), before
Task 1. The web tools' design is `2026-11-02-claxbot-web-tools-design.md`
("web §n").

**Architecture:** `clax-bot` gains `capability.rs` (the five capabilities,
`Capabilities::now`, `CapSet`, `UrlRule`) and `clax/spend.rs` (one Spend
sequence, a `Performer` per operation); `web/` and `browser/` become
performers on it. Browser sessions live in a host-owned registry
(`browser/live.rs`) of tasks that own their sockets. `clax-store` gains
`bot_cap_grants`, `bot_runs.caps`, `bot_browser_sessions` and the
per-second line, all in the branch's own migration 67, edited in place.

**Tech Stack:** Rust (clax-bot, clax-store over SQLite, clax-server on
axum, tokio-tungstenite), TypeScript (`web/bridge`), Svelte 5
(`web/shell`, vitest), Bazel.

## Global Constraints

- Everything in `AGENTS.md`. No `unsafe`, no hard links, no Perl, no new
  npm runtime dependency, no new Rust crate (the recording format needs
  none, spec §6.6).
- **Verification.** `just test-affected` is broken at its bazel query
  step. Each task runs, in the background, logged under `target/`:

      bazel test //crates/clax-bot/... //crates/clax-store/... //crates/clax-server/... //crates/clax-cli/... //bazel/wasm:check //web:unit //web:lint //web:typecheck //web:bundle_size //apps/clax_app:prompt_sync > target/unify-task-N.log 2>&1

  and reports the summary lines. While iterating, the narrower target
  the task names first. Never `--nocache_test_results`: a wrong cache hit
  is a BUILD-input bug to fix.
- Commit each task alone with `git -c commit.gpgsign=false commit`, a
  conventional subject, no attribution lines; never push.
- **Every task compiles and passes on its own.** Where an old API has
  callers a later task moves, the task keeps it as a shim over the new
  one; Task 17 removes the shims.
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
   charged from the open's end to its own end (Task 12,
   `a_session_lives_across_steps_and_its_time_is_charged_once`).
4. **Step 2 replayed after a crash**: every journaled call answers from
   the journal; the first un-journaled call answers
   `browser_session_lost` (Task 12, `a_replay_after_a_restart_answers_lost`).
5. **The idle end fires while no step is open**: the tail is charged
   with no step, once (Task 9, `a_host_end_with_no_open_step_charges_once`).
6. **A `goto` out of an artifact session**: refused without
   `browser.open.url`; with it, the session converts once, the
   synthetic origin answers `why: "left"`, `runTests` answers
   `browser_disabled` (Task 11, `a_goto_out_converts_the_session_once`).
7. **The person revokes `browser.open.content` mid-session**: the
   session closes before `PUT /api/me/bot` answers, and its next call is
   `browser_session_closed` `revoked` (Task 14,
   `a_revoke_closes_the_session_before_it_answers`).
8. **`open({html})` with neither `web.fetch` nor `browser.open.url`**
   works, and its page's request to another origin is blocked (Task 12,
   `content_opens_without_any_web_capability`).

## Threads

Spec decisions are qualifier records on the spec; each task names those
it builds. Open threads on code the plan touches, to account for:

- `369f1a79` (crash not noticed), `11b7cb34` (no auto-attach),
  `214066ff` (iframes and workers bypass a browse's policy), `9cf3f36d`
  (popup closing rests on `waitForDebuggerOnStart`): Task 10.
- `ee9cc794` (no Browser Run session ID in `bot.browser`): unchanged by
  this plan; Task 14 keeps the record's question open.
- `a395ac41` (screenshots scaled to the default image side, not the
  run's): Task 12 passes `RunLimits::image_side` to the registry.
- `a602eba5`, `70a0f3e0`, `285218bc` (`clax/web.rs`): Task 6 keeps each
  behaviour as it is and re-anchors their records onto `clax/spend.rs`.
- `c56c644b` (a browse's own time cap): Task 3 replaces both caps with
  `browser.call_seconds`; reply on the thread.
- `445dba5f` (the Me page's browser switch speaks only of testing): Task
  7 replaces the switch; reply with the commit.
- `840f091e` (where the web namespace's rule lives): Task 5 keeps the
  namespace doc comment as the rule.
- `7166a9f8` (a call's `ms` is always 0 in run events): Task 15 takes a
  session call's `ms` from its journal entry, not the run loop's record.

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
| `crates/clax-bot/src/browser/serve.rs` | `ContentSite`; the left origin |
| `crates/clax-bot/src/browser/opts.rs` | `BrowserSource` parsing; session operation arguments |
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
  `bot_runs.browser` stay until Task 17.
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
  (shims, removed in Task 17).

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

Builds `df067c3c`.

**Files:** `crates/clax-bot/src/api.rs` (`OpShape` replaced by
`CapSet`: `declared(name, &CapSet)`, `dts(&CapSet)`), `prompt.rs`
(`shape_of` reads the binding's capabilities; the browser rules name the
run's source kinds), `crates/clax-store/src/bot_runs.rs`
(`Bindings::clax(artifact, &str caps)` grants the operations of spec
§6.1's rule), `clax/provider.rs`, goldens under `crates/clax-bot/src/`
and `tests/golden`.

- [ ] **Tests first:** `a_web_capability_set_declares_what_it_did` (byte for byte with today's web goldens), `browser_namespace_needs_an_open_capability`, `test_and_run_tests_need_the_artifact_capability`, `the_browser_rules_name_the_runs_source_kinds`.
- [ ] Regenerate goldens as `golden.rs` documents; review every diff by eye; full verification (`//apps/clax_app:prompt_sync` among them).
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
every family carries `capability`.

- [ ] **Tests first:** the existing `tests/web.rs` and `tests/browser.rs` suites pass unchanged except the audit kind; `the_check_order_is_one_for_every_performer` (a table test driving each performer to each refusal in order); `every_call_writes_one_bot_spend`; `a_bot_web_row_is_found_by_the_bot_spend_filter`.
- [ ] `bazel test //crates/clax-bot/... //crates/clax-store/...`; fail; implement; full verification.
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
- [ ] `bazel test //crates/clax-server/... //web:unit`; fail; implement; full verification.
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
- [ ] `bazel test //crates/clax-bot/... //crates/clax-store/... //crates/clax-server/... //bazel/wasm:check`; fail; implement; full verification.
- [ ] Commit: `feat(bot): fetch a page's HTML`

## Phase C: browser sessions

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
- [ ] `bazel test //crates/clax-store/...`; fail; implement; full verification.
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
BrowserError>`; `call(run, handle, op, draft) -> Result<Acted,
BrowserError>` (`browser_session_lost`, `browser_session_closed`);
`close(run, handle, reason)`; `end_run(run)`; `reevaluate(&dyn
Fn(&RunId, kind) -> bool)`; `ended() -> Vec<Ended>` (host ends for the
store to charge). The task per session: serves `Fetch.requestPaused`
between calls, heartbeats after 20 s idle, fires idle and lifetime by
the source's deadline, ends as lost on crash or detach.
`Target.setAutoAttach {autoAttach: true, waitForDebuggerOnStart: true,
flatten: true}`: each attached iframe or worker gets `Fetch.enable` (and,
for a worker, WebSocket and WebTransport removed by `Runtime.evaluate`)
before `Runtime.runIfWaitingForDebugger`.

- [ ] **Tests first** (against `FakeChrome`): `requests_are_served_between_calls`, `the_heartbeat_is_sent_after_twenty_idle_seconds`, `the_idle_end_fires_by_the_deadline`, `the_lifetime_end_cuts_a_running_call`, `a_crashed_page_ends_the_session_as_lost`, `an_iframe_and_a_worker_are_held_to_the_policy`, `a_popup_is_closed_after_auto_attach`, `run_test_answers_as_before` (the existing session tests unchanged).
- [ ] `bazel test //crates/clax-bot:clax_bot_lib_test`; fail; implement; full verification.
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
- [ ] `bazel test //crates/clax-bot/...`; fail; implement; full verification.
- [ ] Commit: `feat(bot): open the browser on a URL, the clax or supplied content`

### Task 12: The session operations

Builds `633b7b5e`, `87804b1a`, `0a6381ce`; spec §6.1–§6.5, §6.8.

**Files:** `crates/clax-bot/src/clax.d.ts` (the `browser` namespace of
spec §6.1; `browse` removed; `test` takes `{artifact, path}`),
`clax/browser.rs` (performers `Open`, `SessionCall`, `Close`, `Test` on
`spend.rs`: per-call segments through `Spend.session`; the run's open
sessions; per-kind caps; `left`; `new` counts; a `credits_exhausted`
refusal closes the session), `clax/session.rs` (`PARTS` for every new
operation), `clax/provider.rs` (`with_browser_sessions`; screenshots at
`RunLimits::image_side`, `a395ac41`), `prompt.rs` and goldens,
`crates/clax-bot/tests/browser.rs`, `tests/common/mod.rs`.

- [ ] **Tests first** (`tests/browser.rs`, through the run loop): `a_session_lives_across_steps_and_its_time_is_charged_once`, `a_replayed_step_opens_no_browser`, `a_replay_after_a_restart_answers_lost` (a new registry), `an_idle_session_answers_closed_idle`, `a_failed_click_is_an_answer_not_an_error`, `content_opens_without_any_web_capability`, `an_open_of_a_kind_the_run_lacks_is_disabled`, `a_second_open_past_the_runs_limit_is_capped`, `credits_running_out_close_the_session`, `the_run_end_closes_its_sessions`, `test_still_answers_its_result`, `two_calls_in_one_promise_all_run_in_order`.
- [ ] `bazel test //crates/clax-bot/...`; fail; implement; regenerate goldens; full verification.
- [ ] Reply on `a395ac41` with the commit.
- [ ] Commit: `feat(bot): browser sessions for a computer-use loop`

### Task 13: Recording

Builds `c448d956`.

**Files:** create `crates/clax-bot/src/browser/record.rs`
(`Page.startScreencast {format: "jpeg", quality: 70, maxWidth,
maxHeight}`, `Page.screencastFrameAck` for each frame, at most 8 a
second kept, `Page.stopScreencast`; frames to content items, the
manifest item `application/vnd.clax.frames+json`); `clax/browser.rs`
(`record` performer); `crates/clax-store/src/bot_content.rs` (accept the
manifest's media type as a non-image item); `clax.d.ts`.

- [ ] **Tests first:** `every_frame_is_acknowledged`, `frames_are_kept_at_most_eight_a_second`, `four_evenly_spaced_frames_are_returned`, `the_manifest_names_each_frame_and_its_time`, `ten_seconds_is_the_most`.
- [ ] `bazel test //crates/clax-bot/... //crates/clax-store/...`; fail; implement; full verification.
- [ ] Commit: `feat(bot): record a browser session as a frame sequence`

## Phase D: the daemon and people

### Task 14: Wire sessions into the daemon

Builds `df067c3c` (revoke), `95e6aeb2` (keep-alive).

**Files:** `crates/clax-server/src/browser_source.rs` (`KEEP_ALIVE_MS =
60_000`), `bot.rs` (the `Bot` owns one `BrowserSessions`; providers get
it; a task drains `ended()` into `Store::end_browser_session`),
`core.rs` (at start, `Store::lose_open_browser_sessions`),
`routes/admin.rs` and `routes/bot.rs` (every grant change calls
`BrowserSessions::reevaluate` with `Capabilities::now` before
answering), the run loop's end of a run (`end_run`),
`crates/clax-server/tests/api_claxbot_run.rs`, `api_browser_source.rs`.

- [ ] **Tests first:** `open_sends_keep_alive_60000`, `a_session_across_two_steps_through_the_daemon` (fake CDP server), `a_revoke_closes_the_session_before_it_answers`, `an_admin_switch_off_closes_every_session_of_its_kind`, `a_restart_marks_sessions_lost`, `the_run_end_closes_and_charges`.
- [ ] `bazel test //crates/clax-server/...`; fail; implement; full verification.
- [ ] Comment on `ee9cc794`: the session ID is still not known; sessions make it more useful (one ID across many calls).
- [ ] Commit: `feat(server): hold browser sessions across steps on the daemon`

### Task 15: The run panel's sessions and clips

**Files:** `crates/clax-bot/src/events.rs` (a session call carries
`browser: {session, kind, op, ms, ok}` and `cost_milli`; `ms` from the
journal entry, `7166a9f8`), `crates/clax-server/src/routes/bot.rs`
(`run_content` also serves a frame manifest as JSON under the same
headers), `web/shell/src/bot/RunPanel.svelte`, `model.ts`
(`sessionLine`, grouping a session's calls), a flipbook component,
`run-panel.test.ts`.

- [ ] **Tests first:** server `a_frame_manifest_is_served_as_json_to_the_runs_viewers`; vitest `a_session_is_one_line_with_its_calls`, `a_host_end_shows_its_reason`, `a_clip_plays_its_frames_in_order`.
- [ ] `bazel test //crates/clax-server/... //web:unit //web:lint //web:typecheck //web:bundle_size`; fail; implement; full verification.
- [ ] Commit: `feat(shell): browser sessions and clips in the run panel`

### Task 16: Docs

**Files:** `docs/usage.md` (the five capabilities, their settings,
prices and risks; sessions and what they cost while a model thinks; the
token permissions), `docs/contract.md` (the admin and Me routes'
fields; `bot.spend`, `bot.grant`; `bot.web` and `me.web` as
historical; the content route's manifest), `crates/clax-worker/README.md`
(browser capabilities not configured; `html` fetch works).

- [ ] Full verification (doc tests reading `docs/`).
- [ ] Commit: `docs(claxbot): capabilities and browser sessions for operators`

### Task 17: Remove the first version's shapes

**Files:** `migrations.rs` (67 no longer adds `bot_person_prefs.browser`
or `bot_runs.browser`; its mark becomes `bot_browser_sessions`;
`MARKS_AFTER_APP` and the migration tests follow), `bot_web.rs`
(`browser_choice`, `set_browser_choice` removed; `web_choice` stays for
the Me route's `web` field), `bot_runs.rs` (`Run::browser` removed;
`Run::web` derived for the run JSON's `web` field only),
`crates/clax-core/src/audit/mod.rs` (`bot.browser` and `me.browser`
removed: no main data holds them), `web/access.rs` and
`browser/access.rs` (`WebAccess`, `BrowserAccess`, `WebShape`,
`BrowserShape` removed where nothing reads them), `crates/clax-server/tests/live_browser_run.rs`
(one session across two calls with a pause longer than `keep_alive`,
by the heartbeat; still skipped without credentials), `justfile`
(`browser-live` unchanged in form).

- [ ] **Tests first:** `migration_67_from_main_has_no_branch_columns`; the whole suite.
- [ ] Full verification.
- [ ] Commit: `refactor(bot): drop the first version's browser grant and shapes`

## Before merge

- [ ] `just check` once (background, logged under `target/`); a lane failing in code this branch did not touch is rerun alone and both results reported.
- [ ] `qualifier diff main`: every spec decision a task built has a reply naming its commit; closes of records other sessions wrote, and every `status:needs-decision` thread, are left for the owner.
- [ ] Run `just browser-live` with real credentials if the owner provides them; reply on `88b76c21` (path spelling) and `d48cf9c0` (keep-alive) with what it showed.
