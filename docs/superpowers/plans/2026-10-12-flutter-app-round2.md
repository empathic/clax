# Clax App Round 2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the app's single structured agent call with a tool loop that edits a draft, renders and inspects it, and publishes once; stream its work as a live preview and narration; and add the round-2 UX (floating comment button, hold-then-drag, thread detail, whole-app comment, Retry, voice cues, native inbox, questions, version diff).

**Architecture:** One `Toolbox` per run holds the draft and every rule (tools, caps, publish gate, change ledger, pinned document writes). Runners only move tool calls: `MessagesApiRunner` (Android and macOS, a Dart loop over streamed Messages API turns) and `ScriptedRunner` (tests); on macOS without an API key, `ClaudeCliBackend` stays today's single structured call. Questions are Clax's own `ask` questions, answered in the shell's question card. Renders load a hidden draft artifact of the daemon in an in-app `WebViewInspector`.

**Tech Stack:** Rust 2024 (axum 0.8, rusqlite), Svelte 5 + TypeScript (Vitest, Playwright), Flutter 3.47.7 / Dart 3.13.5 (`webview_flutter`, `flutter_secure_storage`, `speech_to_text`, `path_provider`), Kotlin and Swift for one method channel each.

**Spec:** `docs/superpowers/specs/2026-10-12-flutter-app-round2-design.md` ("R§n"), on top of `docs/superpowers/specs/2026-10-12-flutter-app-design.md` ("base §n"). Read both before your task. The owner's decisions (R§1, and base O1–O10) are binding: a task that finds one unworkable stops and reports.

## Global Constraints

- Work only in `/Users/alex/Devel/empathic/clax/.claude/worktrees/flutter-app` (branch `flutter-app`). No worktrees. Read `AGENTS.md` and `.superpowers/sdd/flutter-app/briefs.md`; both are binding.
- **In-flight work.** P1 owns `apps/clax_app/lib/agent/{request,progress,json_stream,message_stream,messages_api_backend,claude_cli_backend,loop,worker}.dart` and their tests until the coordinator records "P1 S-TIER" in `.superpowers/sdd/flutter-app/progress.md`. P4 owns `apps/clax_app/android/**`, `apps/clax_app/lib/platform/**`, `apps/clax_app/lib/app.dart` until "P4 S-TIER". A task that names one of those files waits for that line.
- No test and no gate calls a live model; only Task L2 makes live calls, at most 3.
- **P1 S-TIER includes the fixes of its review records 4faff43f** (a cancelled or timed-out stream must close the response of an injected `HttpClient`) **and 3abb59de** (`MessageStream` drops `partial_json` from every finished block and parses `server_tool_use`/`mcp_tool_use` input). A1 and A4 build on both; A4's tests re-check them (`cancel_closes_the_response`, `no_partial_json_left_on_any_block`).
- Default model `claude-haiku-5-5`, effort `low`; offered `claude-sonnet-5-5`, `claude-opus-5-5`. Messages API: `anthropic-version: 2023-06-01`, `tool_choice` `{"type": "auto"}` only (forced tool choice is a 400 on the 5.5 models), `thinking: {"type": "adaptive"}`, beta `task-budgets-2026-03-13`; for the Sonnet and Opus 5.5 models also beta `server-side-fallback-2026-07-01` with `"fallbacks": "default"`.
- Caps (R§4.4, new app / change): turns 8 / 6; renders 3 / 2; output tokens 32,000 / 20,000 (also the task budget; 20,000 is its minimum); wall clock 90 s / 45 s, not counting `ask` waits; cost $0.50 / $0.50 (safety net). Soft note at 2 turns left or 60% of time, tokens or cost. At a hard cap: publish the last clean rendered draft with "I stopped early", else publish nothing and offer Retry. Prices per MTok: haiku-5-5 $0.10 in / $0.50 out; sonnet-5-5 $2 / $10; opus-5-5 $4 / $20; cache read 0.1× input, cache write 1.25× input.
- Migration 25 is the drafts migration (Task S1); if main takes 25 first, renumber to the next free number.
- No new Rust, npm or Flutter dependencies. `pubspec.yaml`/`pubspec.lock`, `Cargo.lock`, `web/package-lock.json` stay unchanged.
- Shared files have one owner each this round: `docs/contract.md` and `docs/usage.md` — D2; `web/perf/bundle-budget.json` — W7; `apps/clax_app/lib/ui/host_protocol.dart` — U3; `web/shell/src/app/host.ts` — W6 then W8; `apps/clax_app/lib/app.dart` — U6 then N2 (in that order); `apps/clax_app/lib/ui/app_screen.dart` — U3, then U4, then U2, then N2 (in that order); `apps/clax_app/lib/agent/progress.dart` — A1; `apps/clax_app/lib/clax/{api,http_api,fake_api,models}.dart` — C1; `apps/clax_app/lib/platform/settings_store.dart` and `lib/ui/settings_screen.dart` — U5.
- Commit only your own paths, unsigned: `git add <paths> && git -c commit.gpgsign=false commit -m "…" -- <paths>`. Never `git add -A`, `git stash`, `git commit -a`.
- Tests: deterministic, fakes and injected clocks, no sleeps standing in for a condition, no wall-clock budgets. Write the tests first.
- Long commands in the background, logged to `target/flutter-app/<task>.log`.
- Flutter: `/Users/alex/.local/homebrew/bin/flutter` (below: `flutter`), run from `apps/clax_app`. Rust tests: `cargo nextest run -p clax-server --test integration <filter>` with `CLAX_TEST_BIN` per AGENTS.md where a binary is needed. Web: `cd web && npx vitest run <file>`, Playwright `cd web && npx playwright test <spec>`.
- Risk tiers: **high** tasks get a fresh reviewer before any dependent task starts; **low** tasks are reviewed together at the end of their group.
- Qualifier: `"/Users/alex/.claude/plugins/cache/qualifier/qual/0.1.0/scripts/ensure-qualifier.sh" exec <args>`; `threads <path>` before editing; record what the next person needs, pinned to lines; batches over two writes via `record --stdin`, dry-run first. Read the open records on the round-2 spec (`threads docs/superpowers/specs/2026-10-12-flutter-app-round2-design.md`) before your task: four are owner questions whose answers may change your task.
- Prose: "ID", never "id", except as a literal symbol. Doc comments and commit messages describe the contract or the change.

## Review Focus

1. **A change runs while the person keeps using the app and writes into it** (adds a run on the page while the agent works): the draft's document copy diverges, the agent's staged `update` was pinned to the version it read, the live document moved on — `finish` must re-read and retry once, then restore the version and fail, never overwrite the person's entry. Pinned in Task A3 (`finish_never_overwrites_a_document_the_person_changed_mid_run`).
2. **The model streams a `write_file` and the stream is cut** (max_tokens, network cut, cancel) mid-content: the partial file must never reach the draft or a render; the preview may show it, nothing else. Pinned in Task A4 (`truncated_write_never_reaches_the_draft`).
3. **A page that writes to its db on load, or loops forever, is rendered**: writes land in the draft's copy only; a hung page returns an error result within the injected deadlines and the run continues. Pinned in Task S1 (`draft_writes_never_touch_the_app`) and Task A5 (`render_of_a_hung_page_returns_an_error_result`).
4. **The app is killed between `finish`'s publish and its replies**: recovery replies once with the landed version; the draft artifact is swept; no second version. Pinned in Task A7 (`kill_after_publish_recovers_with_one_version`).
6. **A cap hits right after the agent broke a page that rendered clean a turn earlier**: the published version is the clean rendered snapshot, never the broken later draft, and the reply says it stopped early. Pinned in Task A3 (`capStop` tests) and A4 (`hard_cap_publishes_the_last_clean_draft`).
5. **A hold-drag that starts on text and ends over an image or off the page**: the range ends at the last caret found; a drag that ends in the area under `AREA_MIN` is a tap; the page never scrolls while dragging and always scrolls before arming. Pinned in Task W5 (`text_drag_ending_off_text_keeps_the_last_caret`, `page_scrolls_before_arming_not_after`).

---

## Tasks, dependencies and parallel groups

| Task | One line | Risk | Depends on |
|---|---|---|---|
| S1 | Draft artifacts in the daemon (migration 25, routes, hidden, doc copy, sweep) | high | — |
| W5 | Bridge: hold, then drag (text range or area) on touch | high | — |
| W6 | Bridge `inspect`, shell `?inspect=1`, host `inspect`/`inspection`/`composeArtifact` | high | — |
| W8 | Shell: questions docked in app mode, `openQuestions`, `questions {open}` | high | W6 |
| W7 | Playwright for W5, W6 and W8; bundle budgets | low | W5, W6, W8 |
| C1 | ClaxApi: drafts, query, owner thread calls, questions, inbox, `ClaxStream`, late answers | high | — (S1's routes from the spec) |
| A1 | Toolbox core: specs, draft, file tools, `set_meta`, budget, progress events, runner interface | high | P1 |
| A2 | Data tools: `db_collections/list/get/query`, staged `db_write` with pins | high | A1, C1 |
| A3 | `finish` and `give_up`: the apply path moved from `loop.dart` | high | A1, A2 |
| A4 | `MessagesApiRunner` and streamed turns in `MessagesApiBackend` | high | A1 |
| A5 | `render` tool and the `Inspector` interface | high | A1, C1 |
| A6 | `ask` (Clax's elicitation), asked-questions map, late answers | high | A3, C1 |
| A7 | `AgentLoop`/`FeedbackWorker` on the runner (CLI single call kept); progress, retry, recovery | high | A3, A4, A5, A6, A9, A10 |
| A9 | Prompt for the tool loop (`system.md`, `PromptBuilder` first messages) | low | A1 |
| A10 | `ResumingTurns` (per-turn network-cut resume) | low | A4, P4 |
| U3 | Floating comment button, comment bar, whole-app comment; host protocol Dart side | high | W6 (protocol from the spec) |
| U4 | Thread detail with reply, resolve, Send to Clax, Retry hook | low | C1, U3 |
| U2 | Working chip narration and Retry | low | A7, U4 |
| U1 | Making screen: streamed preview, inspection image, inline question | low | A7 |
| U5 | Voice auto-start and cues (settings, cue channel on Android and macOS) | low | P4 |
| U6 | `WebViewInspector`, inspector host in the root stack, runtime wiring | high | A5, A7, W6, U3, P4 |
| N1 | Inbox screen and launcher button | low | C1 |
| N2 | Question sheets (docked and `/inbox?q=`), presenter, badge | low | C1, A6, U6, W8 |
| N3 | Version page and line diff | low | — |
| I1 | macOS integration test on the tool loop | high | U6, S1, W6, A7 |
| D2 | `docs/contract.md`, `docs/usage.md` | low | S1, W6, I1 |
| L2 | Live check, ≤ 3 live calls | low | D2 |
| V2 | APK, macOS app, gates, verification addendum | low | L2 |

Parallel groups (each group's tasks touch disjoint files and run at once):

- **G0 (now, beside P1 and P4):** S1 ∥ W5 ∥ W6 ∥ C1 ∥ N3, then W8 once W6 is in
- **G1 (P1 S-TIER):** A1, then A2 ∥ A4 ∥ A5 ∥ A9 ∥ U3 ∥ N1 ∥ W7
- **G2:** A3 ∥ U4 ∥ U5 (U5 once P4 is S-TIER) ∥ A10 (once P4 is S-TIER and A4 is in)
- **G3:** A6
- **G4:** A7
- **G5:** U6 ∥ U1 ∥ U2
- **G6:** N2 ∥ I1
- **G7:** D2 → L2 → V2 (serial)

## File structure

```
crates/clax-core/src/store/migrations.rs        migration 25 (S1)
crates/clax-core/src/store/drafts.rs            draft rows, copy, sweep (S1)
crates/clax-core/src/store/artifacts.rs         hidden filter on listings (S1)
crates/clax-core/src/store/grants.rs            draft → of on reads (S1)
crates/clax-server/src/routes/drafts.rs         /api/drafts (S1)
crates/clax-server/src/routes/{mod,artifacts,threads,events,stream,inbox}.rs   hidden + refusal (S1)
crates/clax-server/tests/api_drafts.rs          (S1)
crates/clax-mcp/src/tools.rs                    `list` skips drafts (S1)
web/bridge/src/long-press.ts, comment-mode.ts   (W5)
web/bridge/src/inspect.ts (new), bridge.ts, protocol.ts      (W6)
web/shell/src/app/host.ts, app.css, artifact-main.ts, caps/cross-read.ts   (W6)
web/shell/src/app/host.ts, app.css, web/shell/src/q/artifact-questions.ts, q/index.ts   (W8)
web/e2e/hold-drag.spec.ts, web/e2e/inspect.spec.ts, web/e2e/app-questions.spec.ts, web/perf/bundle-budget.json   (W7)
apps/clax_app/lib/clax/{api,http_api,fake_api,models}.dart, lib/clax/stream.dart (new)   (C1)
apps/clax_app/lib/agent/tools/{tool_spec,draft,toolbox,budget,file_tools}.dart, runner.dart, pricing.dart   (A1)
apps/clax_app/lib/agent/tools/data_tools.dart   (A2)
apps/clax_app/lib/agent/tools/finish.dart       (A3)
apps/clax_app/lib/agent/runner/messages_runner.dart, turn.dart; messages_api_backend.dart   (A4)
apps/clax_app/lib/agent/tools/render_tool.dart, lib/agent/inspector.dart   (A5)
apps/clax_app/lib/agent/tools/ask_tool.dart, lib/agent/asked.dart   (A6)
apps/clax_app/lib/agent/{loop,worker,timings}.dart, runner/scripted_runner.dart   (A7)
apps/clax_app/assets/prompt/system.md, lib/agent/prompt.dart   (A9)
apps/clax_app/lib/platform/network_pause.dart   (A10)
apps/clax_app/lib/ui/{making_screen}.dart        (U1)
apps/clax_app/lib/ui/{change_tracker,working_chip}.dart   (U2)
apps/clax_app/lib/ui/{comment_button,host_protocol}.dart   (U3)
apps/clax_app/lib/ui/{thread_detail,threads_panel}.dart   (U4)
apps/clax_app/lib/platform/{voice_cues,settings_store}.dart, lib/ui/{settings_screen,new_sheet}.dart, android/…/CuesChannel.kt, macos/Runner/CuesChannel.swift   (U5)
apps/clax_app/lib/ui/inspector_surface.dart, lib/app.dart   (U6)
apps/clax_app/lib/ui/{inbox_screen,launcher}.dart   (N1)
apps/clax_app/lib/ui/question_sheet.dart, lib/ui/app_screen.dart (badge), lib/app.dart   (N2)
apps/clax_app/lib/ui/{version_page,line_diff,versions_tab}.dart   (N3)
apps/clax_app/integration_test/{full_loop_test,script}.dart   (I1)
docs/contract.md, docs/usage.md                 (D2)
docs/superpowers/verification/2026-10-12-flutter-app.md   (V2)
```

---

## Group 0

### Task S1: Draft artifacts

Spec R§5.1. Risk: high.

**Files:**
- Modify: `crates/clax-core/src/store/migrations.rs` (migration 25: `ALTER TABLE artifacts ADD COLUMN draft_of TEXT`; `ALTER TABLE artifacts ADD COLUMN hidden INTEGER NOT NULL DEFAULT 0`; index `artifacts_hidden`)
- Create: `crates/clax-core/src/store/drafts.rs` (`create_draft`, `copy_docs`, `delete_draft`, `list_drafts`, `sweep_drafts(now, max_age)`)
- Modify: `crates/clax-core/src/store/mod.rs`, `store/artifacts.rs` (every listing and gallery query adds `AND hidden = 0`), `store/inbox.rs` (no item for a hidden artifact), `store/feedback.rs` and `store/watches.rs` (no watch, no feedback on hidden), `store/grants.rs` (`may_read(reader)` maps a draft to its `draft_of`)
- Create: `crates/clax-server/src/routes/drafts.rs`; Modify: `routes/mod.rs` (routes, token-only classification), `routes/threads.rs` (403 `draft` on create), `routes/events.rs` and `routes/stream.rs` (hidden artifacts' events only to `artifact=<id>` / `artifact:<id>` subscribers), `crates/clax-server/src/main.rs` or the daemon start path (sweep at start and hourly)
- Modify: `crates/clax-mcp/src/tools.rs` (`list` never returns a draft)
- Create: `crates/clax-server/tests/api_drafts.rs`; Modify: `crates/clax-server/tests/integration.rs` (`mod api_drafts;`)

**Interfaces:**
- Produces: `POST /api/drafts` body `{"of": "<aid>"|null, "title": "…", "capabilities": {…}|null, "files": {"index.html": "…"}}` → 201 `{"draft": {"id", "of", "version": 1, "docs_copied": n, "docs_truncated": bool}}`; `GET /api/drafts` → `{"drafts": [{"id", "of", "created_at"}]}`; `DELETE /api/drafts/<id>` → `{"deleted": true}` (404 for a non-draft); `POST /api/artifacts/<draft>/versions` works with the token and no session; `POST /api/artifacts/<draft>/threads` → 403 `{"error": {"code": "draft"}}`. All token-only.

- [ ] **Step 1: Write the failing tests** in `api_drafts.rs`:
  - `draft_copies_docs_and_stays_hidden`: publish app A with `db`, write 3 docs (one under `data/users/u1/`), `POST /api/drafts {of: A}` → `docs_copied == 2`; `GET /api/artifacts` has A only; `/api/events` without filter gets no event for the draft; `GET /api/inbox` gains no item.
  - `draft_writes_never_touch_the_app`: `PUT` a doc on the draft, `GET` it on A unchanged.
  - `draft_copy_stops_at_cap`: 2,001 docs → `docs_copied == 2000`, `docs_truncated == true`.
  - `threads_on_a_draft_are_refused`: 403 `draft`.
  - `draft_reads_another_app_with_ofs_grant`: A declares `db.read: [B]`, owner granted A→B; the draft of A reads B with `X-Clax-Reader: <draft>` → 200; without the grant 403 `not_granted`.
  - `delete_and_sweep_remove_everything`: delete removes rows, docs, files dir; sweep with an injected `now` 61 min later removes an old draft, keeps a new one.
  - `hidden_artifacts_never_listed`: one audit test calling every listing route (`/api/artifacts`, `?artifact=`, the gallery stream topic snapshot, the MCP `list` over `/mcp`) and asserting no draft ID.
- [ ] **Step 2:** `cargo nextest run -p clax-server --test integration api_drafts` → FAIL (routes missing).
- [ ] **Step 3:** Implement per R§5.1. The copy runs in one transaction with the draft row; documents keep their data and get version 1 in the draft.
- [ ] **Step 4:** Run the filter → PASS; then `cargo nextest run -p clax-core` and `-p clax-server` (migrations tests count `MIGRATIONS.len()`); `cargo clippy --all-targets -- -D warnings`; `cargo fmt --check`.
- [ ] **Step 5:** Commit `Add hidden draft artifacts with a document copy, for rendering an agent's work in progress`.

### Task W5: Hold, then drag

Spec R§9.2. Risk: high.

**Files:** Modify `web/bridge/src/long-press.ts`, `web/bridge/src/comment-mode.ts`; Test `web/bridge/test/long-press.test.ts`, `web/bridge/test/comment-mode.test.ts`.

**Interfaces:**
- Produces in `long-press.ts`: `LongPressHooks` gains `dragStart(x, y): void`, `dragMove(x, y): void`, `dragEnd(x, y): void`; after `armed`, a move beyond `SLOP_PX` calls `dragStart` instead of `abandoned`; `touchmove` is listened `{passive: false}` and `preventDefault`ed only between `dragStart` and the end.
- `comment-mode.ts` implements them: text under the finger at arming → `Selection.setBaseAndExtent(start caret, current caret)`; else the `.a` box via `dragRect`; `dragEnd` picks the range (the mouse-selection pick path) or the area (`pickArea`), or the target when under `AREA_MIN`.

- [ ] **Step 1: Failing tests** (timers injected, `trustedOnly: false`):

```ts
it("drag after arming selects a text range and picks it on release", () => {
  const { doc, hooks, timers } = setup('<p id="t">alpha beta gamma delta</p>');
  touch(doc, "touchstart", 10, 10); timers.run(HOLD_MS);
  touch(doc, "touchmove", 120, 12);
  expect(doc.getSelection()!.toString()).toMatch(/^alpha beta/);
  touch(doc, "touchend", 120, 12);
  expect(hooks.picks).toEqual([{ kind: "range", text: expect.stringMatching(/^alpha beta/) }]);
});
it("drag from empty space draws an area and picks it", …);       // pickArea called with the dragRect
it("small drag is a tap pick", …);                                 // under AREA_MIN → target pick
it("text_drag_ending_off_text_keeps_the_last_caret", …);           // end over an <img>: range ends at last caret
it("page_scrolls_before_arming_not_after", …);                     // touchmove before HOLD_MS: defaultPrevented false; after dragStart: true
it("second finger or touchcancel abandons and clears the selection", …);
```

- [ ] **Step 2:** `cd web && npx vitest run bridge/test/long-press.test.ts bridge/test/comment-mode.test.ts` → FAIL.
- [ ] **Step 3:** Implement. Keep the existing tap and hold-release paths unchanged.
- [ ] **Step 4:** Tests pass; `npm run lint`; `npx vitest run bridge` all pass.
- [ ] **Step 5:** Commit `Pick a text range or an area by holding, then dragging, in touch comment mode`.

### Task W6: Inspect and whole-app compose

Spec R§5.2, R§9.3, R§11. Risk: high.

**Files:** Create `web/bridge/src/inspect.ts`; Modify `web/bridge/src/bridge.ts` (install the error ring buffer first), `web/bridge/src/protocol.ts` (`clax:inspect`, `clax:inspection` in BRIDGE_TYPES), `web/shell/src/app/host.ts` (header protocol, `inspect`, `inspection`, `composeArtifact`), `web/shell/src/artifact-main.ts` (`inspect=1`), `web/shell/src/caps/cross-read.ts` (auto-decline in inspect mode), `web/shell/src/app/app.css`; Tests `web/bridge/test/inspect.test.ts`, `web/shell/src/app/host.test.ts`.

**Interfaces:**
- Produces: `claxHost.inspect({settleMs?: number, maxBytes?: number})` → host message `{type: "inspection", png: "<base64>", errors: [{kind: "error"|"unhandledrejection"|"console", text}], failed: ["<url>"], text: "…", title: "…", height: n}` (or `{type: "inspection", error: "<reason>"}`); `claxHost.composeArtifact()` opens the composer with the `artifact` anchor; `?inspect=1` (only with `app=1`) hides comment UI, subscribes to no events, declines consent silently.

- [ ] **Step 1: Failing tests:** ring buffer keeps the newest 20 in order with kinds; settle resolves after 300 ms quiet using injected timers and at `settleMs` at most; outline ≤ 3 KiB lists headings, buttons, inputs; PNG over `maxBytes` is re-rendered at half scale (stub renderer); host `inspect` forwards to the frame and posts `inspection` with the nonce; `composeArtifact` opens a composer whose anchor kind is `artifact`; consent request in inspect mode answers `not_granted` with no prompt.
- [ ] **Step 2:** `npx vitest run bridge/test/inspect.test.ts shell/src/app/host.test.ts` → FAIL.
- [ ] **Step 3:** Implement. The ring buffer wraps `console.error` and listens to `error` (capture, for element load failures too) and `unhandledrejection` at bridge start; no `requestAnimationFrame` anywhere in `inspect.ts`.
- [ ] **Step 4:** Pass; `npm run lint`; `npm run check` (svelte-check); full `npx vitest run`.
- [ ] **Step 5:** Commit `Add the inspect mode the app renders drafts in, and whole-app comments from the host`.

### Task C1: ClaxApi round-2 calls

Spec R§5.1, R§8, R§9.4, R§10. Risk: high.

**Files:** Modify `apps/clax_app/lib/clax/{api,http_api,fake_api,models}.dart`; Create `apps/clax_app/lib/clax/stream.dart`; Tests `test/clax/http_api_test.dart` (scripted server), `test/clax/stream_test.dart`, `test/clax/fake_api_test.dart`, `test/clax/http_api_real_test.dart` (drafts against the real daemon once S1 is in; skip with a reason before).

**Interfaces (Produces):**

```dart
// api.dart additions
Future<DraftInfo> createDraft({String? of, required String title,
    Map<String, Object?>? capabilities, required Map<String, String> files});
Future<PublishResult> publishDraft(String draftId, PublishRequest r); // token only, no session header
Future<void> deleteDraft(String draftId);
Future<List<DraftInfo>> listDrafts();
Future<List<Doc>> queryDocs(String aid, String collection,
    {List<List<Object?>>? where, String? orderBy, bool descending = false, int limit = 50, String? reader});
Future<List<Doc>> listDocsAs(String aid, String collection, {required String reader, int limit = 50});
Future<ThreadView> postOwnerComment(String aid, String tid, String body);
Future<ThreadView> sendThread(String aid, String tid);           // POST …/threads/<tid>/send
Future<ThreadView> resolveAsOwner(String aid, String tid);
Future<ThreadView> reopenThread(String aid, String tid);
Future<QuestionView> askQuestion(String sid, List<Map<String, Object?>> questions, {String? artifactId});
Future<QuestionView> waitQuestion(String sid, String qid, {required Duration wait});
Future<void> withdrawQuestion(String sid, String qid);
Future<List<QuestionView>> questions({String status = 'open'});
Future<QuestionView> answerQuestion(String qid, List<Map<String, Object?>> answers);
Future<QuestionView> declineQuestion(String qid);
Future<InboxPage> inbox({String? q, List<String>? kinds, String read = 'all', String? before, int limit = 50});
Future<InboxSummary> inboxSummary();
Future<int> markInboxRead(String itemId);                         // returns unread
Future<int> markAllInboxRead({int? upto, Map<String, Object?>? filter});
ClaxStream stream();                                              // /api/stream, owner token
// models.dart: DraftInfo{id, of, version, docsCopied, docsTruncated}, QuestionView (contract shape),
// InboxItem, InboxPage{items, nextCursor, unread, total}, InboxSummary{unread, questions, latest},
// FeedbackBatch.answers: List<QuestionView> (default const []).
// stream.dart:
abstract interface class ClaxStream {
  Future<void> subscribe(List<String> topics);
  Stream<StreamEvent> get events;   // StreamEvent{topic, type, data}; reconnects with Last-Event-ID; emits a `resync` event to refetch on
  Future<void> close();
}
```

The send route's body and the query parameter names of `docs::list` for `where`/`order_by` are read from `web/shell/src/threads.ts` (`sendToAgent`) and `crates/clax-server/src/routes/docs.rs` (`list`).

- [ ] **Step 1: Failing tests** per method against `test/support/scripted_server.dart` (method, path, headers: bearer on every call, `X-Clax-Session` only on session routes and never on `publishDraft`; bodies; error mapping); `ClaxStream`: opens, posts subscribe, parses `inbox_item`/`question`, reconnects with `Last-Event-ID` after a drop, surfaces `resync`; `FakeClaxApi` drafts/questions/inbox behave as the routes.
- [ ] **Step 2:** `flutter test test/clax` → FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** `flutter test test/clax`; `flutter analyze`.
- [ ] **Step 5:** Commit `Add the app client's drafts, queries, owner thread calls, questions, inbox and stream`.

### Task N3: Version page and line diff

Spec R§10.3. Risk: low.

**Files:** Create `apps/clax_app/lib/ui/line_diff.dart`, `lib/ui/version_page.dart`; Modify `lib/ui/versions_tab.dart` (row tap opens the page); Tests `test/ui/line_diff_test.dart`, `test/ui/version_page_test.dart`.

**Interfaces:** `List<DiffHunk> lineDiff(List<String> a, List<String> b, {int context = 3})` with `DiffHunk{aStart, bStart, lines: [DiffLine{kind: same|add|del, text}]}`; `VersionPage({required ClaxApi api, required String aid, required int n, required void Function(int n) onPreview, required Future<void> Function(int n) onRestore})`.

- [ ] **Step 1: Failing tests:** `lineDiff` on equal inputs → no hunks; one changed line → one hunk with 3 lines of context each side; insertion at start and end; two distant changes → two hunks; 4,001-line input → caller shows sizes only (`VersionPage` test). Widget: changed files list (added/removed/changed with sizes) from `FakeClaxApi.readFiles(n-1)` vs `(n)`; tapping a file shows coloured add/del lines; Preview and Restore call back.
- [ ] **Step 2–4:** `flutter test test/ui/line_diff_test.dart test/ui/version_page_test.dart` FAIL → implement Myers O(ND) → PASS; `flutter analyze`.
- [ ] **Step 5:** Commit `Show each version's changed files and a line diff`.

---

## Group 1 (P1 S-TIER)

### Task A1: Toolbox core

Spec R§2, R§3 (file tools, `set_meta`), R§4.4, R§6.4. Risk: high.

**Files:** Create `apps/clax_app/lib/agent/tools/{tool_spec,draft,toolbox,budget,file_tools}.dart`, `lib/agent/runner.dart`, `lib/agent/pricing.dart`; Modify `lib/agent/progress.dart` (add `Narration`, `ToolStarted`, `Inspected`); Tests `test/agent/tools/file_tools_test.dart`, `test/agent/tools/budget_test.dart`, `test/agent/pricing_test.dart`.

**Interfaces (Produces):**

```dart
// tool_spec.dart
class ToolSpec {
  const ToolSpec(this.name, this.description, this.inputSchema, {this.strict = true, this.eager = false});
  final String name; final String description; final Map<String, Object?> inputSchema;
  final bool strict; final bool eager;
  Map<String, Object?> toMessagesApi();   // {name, description, input_schema, strict?, eager_input_streaming?}
  Map<String, Object?> toMcp();           // {name, description, inputSchema}
}
const toolOrder = ['list_files','read_file','write_file','str_replace','delete_file','set_meta',
  'db_collections','db_list','db_get','db_query','db_write','render','ask','finish','give_up'];

// toolbox.dart
sealed class ToolOutput { const ToolOutput(); }
final class ToolOk extends ToolOutput { const ToolOk(this.json, {this.png}); final Map<String, Object?> json; final Uint8List? png; }
final class ToolRefused extends ToolOutput { const ToolRefused(this.message); final String message; }
abstract interface class Tool { ToolSpec get spec; Future<ToolOutput> call(Map<String, Object?> input, RunState s); }
class RunState { // shared by tools
  RunState({required this.ctx, required this.draft, required this.budget, this.progress});
  final RunContext ctx; final Draft draft; final RunBudget budget; final ProgressSink? progress;
  RunOutcome? outcome;            // set by finish/give_up/caps/cancel
  bool get ended => outcome != null;
}
class Toolbox {
  Toolbox(this.state, List<Tool> tools);      // orders by toolOrder; missing tools are left out
  List<ToolSpec> get specs;
  Future<ToolOutput> call(String name, Map<String, Object?> input); // unknown → ToolRefused; ended → ToolRefused('The run has ended.'); over caps → ToolRefused(cap text)
  RunState get state;
  /// Ends the run at a hard cap: the tool registered as the cap handler
  /// (FinishTool, A3) publishes `draft.lastClean` or gives up; without a
  /// handler, RunGaveUp(cap).
  Future<RunOutcome> stopAtCap(String cap, {String? narration});
  set capHandler(Future<RunOutcome> Function(String cap, String? narration) h);
}
class RunContext {
  RunContext({required this.kind, this.artifactId, this.baseVersion = 0, this.title,
      this.capabilities, required this.files, this.items = const [], this.threadIds = const [],
      this.shownDocs = const {}, this.sessionId, this.cancelled, required this.clock});
  final RequestKind kind; final String? artifactId; final int baseVersion; final String? title;
  final Map<String, Object?>? capabilities; final Map<String, String> files;
  final List<FeedbackItem> items; final List<String> threadIds;
  final Map<String, int?> shownDocs;   // doc path → version the prompt showed
  final String? sessionId; final Future<void>? cancelled; final Clock clock;
}
sealed class RunOutcome { const RunOutcome(); }
final class RunFinished extends RunOutcome { const RunFinished({this.artifactId, this.version, required this.answered}); final String? artifactId; final int? version; final bool answered; }
final class RunGaveUp extends RunOutcome { const RunGaveUp(this.reason, {required this.answered}); final String reason; final bool answered; }
final class RunCancelled extends RunOutcome { const RunCancelled(); }
final class RunInterrupted extends RunOutcome { const RunInterrupted(); }
final class RunDropped extends RunOutcome { const RunDropped(); }

// draft.dart
class Draft {
  Draft({required Map<String, String> files, this.title, this.capabilities, required this.baseVersion});
  Map<String, String> get files; String? title; Map<String, Object?>? capabilities; int baseVersion;
  int get revision;                 // +1 on every write, delete, set_meta, staged db op
  int? renderedRevision; bool lastRenderHadErrors;
  final List<DbOp> stagedDb; final Map<String, int?> pins; // doc path → version read
  String? draftArtifactId; int? draftVersion;
  DraftSnapshot? lastClean;   // set by render: files, title, capabilities, stagedDb, pins as rendered, when the render had no uncaught errors
  bool changedFrom(Map<String, String> base, {String? title, Map<String, Object?>? caps});
}

// budget.dart
class RunCaps { const RunCaps({required this.turns, required this.renders, required this.outputTokens, this.costUsd = 0.50, required this.wallClock});
  static const newApp = RunCaps(turns: 8, renders: 3, outputTokens: 32000, wallClock: Duration(seconds: 90));
  static const change = RunCaps(turns: 6, renders: 2, outputTokens: 20000, wallClock: Duration(seconds: 45)); ... }
class Usage { const Usage({this.input = 0, this.cacheWrite = 0, this.cacheRead = 0, this.output = 0}); Usage operator +(Usage o); }
class RunBudget {
  RunBudget(this.caps, this.model, this.clock);
  void addTurn(Usage u); void addRender();
  void pauseClock(); void resumeClock();   // `ask` waits do not count
  String? get hardStop;   // null, or "6 turns" / "45 seconds" / "20000 output tokens" / "$0.50"; renders over cap are refused per call, not a stop
  String? get softNote;   // "Time is nearly up: call finish now with what works." when 2 turns left or ≥ 60% of time, tokens or cost
  int get turns; Usage get usage; double get costUsd;
}

// pricing.dart
double costOf(String model, Usage u);   // table from Global Constraints; unknown model → opus prices

// runner.dart
class RunInput { const RunInput({required this.model, required this.system, required this.first, required this.caps});
  final String model; final String system; final List<Map<String, Object?>> first; final RunCaps caps; }
abstract interface class AgentRunner {
  String get name;          // messages-api | claude-cli | scripted
  bool get sendsImages;
  Future<RunOutcome> run(RunInput input, Toolbox toolbox, {ProgressSink? progress, Future<void>? cancelled});
}

// progress.dart additions
final class Narration extends AgentProgress { const Narration(this.text); final String text; }
final class ToolStarted extends AgentProgress { const ToolStarted(this.name, this.label); final String name; final String label; }
final class Inspected extends AgentProgress { const Inspected(this.png, this.errors); final Uint8List png; final int errors; }
String toolLabel(String name, Map<String, Object?> partialInput); // "Writing index.html", "Checking how it looks", …
```

File tools (`file_tools.dart`): `ListFilesTool`, `ReadFileTool`, `WriteFileTool` (eager, non-strict, validates `path`/`content` strings itself), `StrReplaceTool` (eager, non-strict; exactly-once via `apply.dart`), `DeleteFileTool`, `SetMetaTool` — limits per R§3, path validation reusing `validateResult`'s path rules from `edit_result.dart` (extract the path check into a function there if needed; `edit_result.dart` is not P1's).

- [ ] **Step 1: Failing tests**, e.g.:

```dart
test('str_replace needs exactly one match', () async {
  final tb = toolboxWith(files: {'index.html': '<p>a</p><p>a</p>'});
  final r = await tb.call('str_replace', {'path': 'index.html', 'old_str': '<p>a</p>', 'new_str': 'x'});
  expect(r, isA<ToolRefused>().having((e) => e.message, 'message', contains('2 times')));
  expect(tb.state.draft.files['index.html'], '<p>a</p><p>a</p>');
});
```

  plus: `write_file` refuses `../x`, absolute paths, over 2 MiB, a 65th file; `delete_file` refuses `index.html`; `read_file` windows and truncation; `set_meta` rejects invalid JSON; revision increments; non-string `content` refused (eager input); calls after `outcome` set refused; caps: `hardStop` per cap with an injected clock, the clock paused across an `ask` wait; `softNote` at 2 turns left and at 60% time; `costOf` sums with cache prices; spec JSON for `write_file` has `eager_input_streaming: true` and no `strict`, others `strict: true` and `additionalProperties: false`.
- [ ] **Step 2–4:** `flutter test test/agent/tools test/agent/pricing_test.dart` FAIL → implement → PASS; `flutter analyze`.
- [ ] **Step 5:** Commit `Add the agent's toolbox: draft, file tools, caps and the runner interface`.

### Task A2: Data tools

Spec R§3 (db rows). Risk: high.

**Files:** Create `apps/clax_app/lib/agent/tools/data_tools.dart`; Test `test/agent/tools/data_tools_test.dart`.

**Interfaces:** `DbCollectionsTool`, `DbListTool`, `DbGetTool`, `DbQueryTool`, `DbWriteTool` implementing `Tool` (A1), constructed with `ClaxApi` (C1: `collections`, `listDocs`, `getDoc`, `queryDocs`). Reading records `draft.pins[path] = version` for this app's documents. `db_write` stages `DbOp`s into `draft.stagedDb`; an op on an existing document without a pin (and not in `ctx.shownDocs`) is refused "Read <path> with db_get first"; an `app` other than null on `db_write` is refused; ≤ 200 ops.

- [ ] **Step 1: Failing tests:** list/get/query map to the API with `app` → other app ID; results capped at 50 docs and 32 KiB with `truncated`; `data/users/` paths never returned; `db_write` set on a new doc stages with pin null; update on an existing unread doc refused; after `db_get` staged with that version; `data_json` not an object refused; op count cap.
- [ ] **Step 2–4:** FAIL → implement → PASS; `flutter analyze`.
- [ ] **Step 5:** Commit `Add the agent's document tools, with writes staged and pinned to what it read`.

### Task A4: MessagesApiRunner

Spec R§4.2, R§6.1–R§6.4. Risk: high.

**Files:** Create `apps/clax_app/lib/agent/runner/turn.dart`, `lib/agent/runner/messages_runner.dart`; Modify `lib/agent/messages_api_backend.dart` (add `Future<ModelTurn> turn(TurnRequest r)`; keep `run` until A7 removes its callers); Test `test/agent/runner/messages_runner_test.dart`; Modify `test/support/fake_messages_api.dart` (scripted SSE turns).

**Interfaces:**

```dart
// turn.dart
class TurnRequest { const TurnRequest({required this.model, required this.system, required this.tools,
    required this.messages, this.maxTokens = 32000, required this.outputCap, this.cancelled, this.onProgress});
  final String model; final String system; final List<Map<String, Object?>> tools;
  final List<Map<String, Object?>> messages; final int maxTokens; final int outputCap;
  final Future<void>? cancelled; final ProgressSink? onProgress; }
class ModelTurn { const ModelTurn({required this.content, required this.stopReason, this.stopDetails, required this.usage});
  final List<Map<String, Object?>> content; final String stopReason;
  final Map<String, Object?>? stopDetails; final Usage usage; }
abstract interface class TurnBackend { String get name; Future<ModelTurn> turn(TurnRequest r); } // throws AgentError
// messages_runner.dart
class MessagesApiRunner implements AgentRunner { MessagesApiRunner(this.backend); final TurnBackend backend; … }
```

Request body exactly as R§6.1. Streaming: P1's `MessageStream` rebuilds the turn; `onDelta` for a `tool_use` block whose name is `write_file`/`str_replace` feeds a `JsonStreamScanner` that emits `PartialFile(path, content so far)` / `PartialEdit`; `content_block_start` of `tool_use` emits `ToolStarted`; text blocks emit `Narration` (first sentence, ≤ 90 chars).

- [ ] **Step 1: Failing tests** (fake Messages API serving scripted SSE per request; a Toolbox of A1 file tools):
  - `runs_tools_until_finish` — turn 1 `write_file`, turn 2 a fake `finish` tool sets the outcome → 2 requests, outcome returned.
  - `history_is_append_only` — request k's `messages` is a byte-equal prefix of request k+1's; assistant content (including a `thinking` block with a signature) echoed verbatim.
  - `request_shape` — tools in `toolOrder`, `tool_choice` auto, `thinking` adaptive, `output_config.task_budget.total == caps.outputTokens`, top-level `cache_control`, system block with `cache_control`, beta header; fallbacks only for sonnet/opus 5.5.
  - `truncated_write_never_reaches_the_draft` — a turn with `stop_reason: max_tokens` mid-`write_file`: tool not run, the turn is resent once with `max_tokens: 64000`; a second truncation → `RunGaveUp`.
  - `refusal_gives_up_with_its_category`.
  - `no_tool_call_turn_gets_the_finish_nudge` — text-only turn → next request ends with user text "Call finish or give_up.".
  - `soft_note_is_appended_after_tool_results` and `hard_cap_stops_without_a_request`.
  - `parallel_tool_uses_answered_in_one_message` — two `tool_use` blocks → one user message with two `tool_result`s in order.
  - `cancel_mid_turn_aborts_and_ends_cancelled`; `cancel_closes_the_response` (an injected `HttpClient`'s response is closed: 4faff43f).
  - `no_partial_json_left_on_any_block` (a `server_tool_use` block's input parsed, no `partial_json` echoed back in history: 3abb59de).
  - `hard_cap_publishes_the_last_clean_draft` — the cap hits after a clean render and a later edit: the runner calls `toolbox.stopAtCap(cap, narration: latest)`, which (with A3's handler; a fake handler here) publishes `draft.lastClean` with the stopped-early reply (A3); `hard_cap_without_clean_draft_gives_up`.
  - `progress_events` — PartialFile prefixes grow, ToolStarted per block, Narration per text block; usage summed and cost computed.
- [ ] **Step 2–4:** `flutter test test/agent/runner/messages_runner_test.dart` FAIL → implement → PASS; also `flutter test test/agent/messages_api_backend_test.dart` stays green.
- [ ] **Step 5:** Commit `Run the agent as a streamed tool loop over the Messages API`.

### Task A5: `render` and the Inspector interface

Spec R§5.2. Risk: high.

**Files:** Create `apps/clax_app/lib/agent/inspector.dart`, `lib/agent/tools/render_tool.dart`; Create `test/support/fake_inspector.dart`; Test `test/agent/tools/render_tool_test.dart`.

**Interfaces:**

```dart
abstract interface class Inspector {
  /// Loads [url] and returns what it shows; throws InspectorError when it does not load or answer.
  Future<Inspection> inspect(Uri url, {Duration settle = const Duration(seconds: 3), int maxBytes = 1 << 20, Future<void>? cancelled});
}
class Inspection { const Inspection({required this.png, required this.errors, required this.failed,
    required this.text, required this.title, required this.height, required this.settledMs});
  final Uint8List png; final List<({String kind, String text})> errors; final List<String> failed;
  final String text; final String title; final int height; final int settledMs; }
class InspectorError implements Exception { const InspectorError(this.message); final String message; }
class RenderTool implements Tool { RenderTool(this.api, this.inspector, {required this.baseUrl}); … }
```

Behaviour: first call `createDraft(of: ctx.artifactId, …files)`, later `publishDraft(id, ifVersion: draftVersion)`; staged db ops written to the draft with `batchDocs` (unpinned; refusals ignored and listed); `inspect(<base>/a/<draft>?app=1&inspect=1[&file])`; sets `renderedRevision`, `lastRenderHadErrors` (errors of kind `error`/`unhandledrejection`) and, on a clean render, `draft.lastClean`; emits `Inspected`; result JSON per R§3 with `png`. Cap from `RunCaps.renders`. A `cleanup()` deletes the draft (called by finish/give_up/cancel; idempotent).

- [ ] **Step 1: Failing tests:** first render creates, second publishes with `if_version`; staged ops reach the draft only; `render_of_a_hung_page_returns_an_error_result` (fake inspector throws `InspectorError('did not load')` → `ToolRefused` naming it, run state not ended); errors flag; render past `RunCaps.renders` refused; `lastClean` set only by a clean render and keeps the rendered state after later writes; cleanup deletes once; a cancelled run never creates a draft.
- [ ] **Step 2–4:** FAIL → implement → PASS.
- [ ] **Step 5:** Commit `Add the render tool: the draft is served as a hidden artifact and inspected`.

### Task A9: Prompt for the tool loop

Spec R§4.1. Risk: low.

**Files:** Modify `apps/clax_app/assets/prompt/system.md`, `lib/agent/prompt.dart`; Test `test/agent/prompt_test.dart`.

**Interfaces:** `PromptBuilder.firstNew(String request, List<OtherApp> others) → List<Map<String, Object?>>` (content blocks); `PromptBuilder.firstChange({required String title, required Map<String, String> files, required List<CommentContext> comments, required List<OtherApp> others, required List<AppCollection> documents, String? answers}) → List<Map<String, Object?>>` (image block first when a clip, files inline under 24 KiB total else `index.html` up to 24 KiB plus the file list). `system.md` keeps the synced contract copies (`scripts/sync-app-prompt.sh --check` passes) and replaces the `EditResult` instructions with R§4.1's working rules and the tool list.

- [ ] **Step 1: Failing tests:** inline-files threshold (23 KiB inline, 25 KiB list + index only); comments and answers quoted as JSON strings; clip block first; system prompt mentions every tool name in `toolOrder` and no `EditResult` field name.
- [ ] **Step 2–4:** FAIL → implement → PASS; `scripts/sync-app-prompt.sh --check`.
- [ ] **Step 5:** Commit `Teach the agent to work with tools: narrate, write once, render, finish`.

### Task U3: Floating comment button, comment bar, whole-app comment

Spec R§9.1, R§9.3. Risk: high.

**Files:** Create `apps/clax_app/lib/ui/comment_button.dart`; Modify `lib/ui/host_protocol.dart` (calls `ComposeArtifact`, `Inspect{settleMs, maxBytes}`, `OpenQuestions`; messages `Inspection` per W6 and `QuestionsMsg{open}` per W8), `lib/ui/app_screen.dart` (mount the button and bar; their callbacks), `test/support/fake_surface.dart`; Tests `test/ui/comment_button_test.dart`, `test/ui/host_protocol_test.dart`, `test/ui/app_screen_test.dart`.

**Interfaces:** `CommentButton({required bool commenting, required int threadCount, required VoidCallback onTap, required VoidCallback onLongPress, required double dy, required ValueChanged<double> onMoved})`; `CommentBar({required VoidCallback onWholeApp, required VoidCallback onThreads, required VoidCallback onClose})`. Host protocol: `ComposeArtifact()` → `claxHost.composeArtifact()`; `Inspect` → `claxHost.inspect({...})`; `Inspection.fromJson` (png base64 decoded, or `error`).

- [ ] **Step 1: Failing tests:** tap → `EnterCommentMode` sent and bar shown; tap again/✕ → `ExitCommentMode`; long-press → panel open; Whole app → `ComposeArtifact` sent and dictation starts when the setting is on (FakeVoice); button hidden while a draft is open and while the narrow panel is open; drag snaps to an edge and reports `dy`; swipe still opens the panel; `Inspection` parse with and without `error`.
- [ ] **Step 2–4:** `flutter test test/ui/comment_button_test.dart test/ui/host_protocol_test.dart test/ui/app_screen_test.dart` FAIL → implement → PASS.
- [ ] **Step 5:** Commit `Add the floating comment button and whole-app comments`.

### Task N1: Inbox

Spec R§10.1. Risk: low.

**Files:** Create `apps/clax_app/lib/ui/inbox_screen.dart`; Modify `lib/ui/launcher.dart` (inbox button with unread badge; navigation); Tests `test/ui/inbox_screen_test.dart`, `test/ui/launcher_test.dart`.

**Interfaces:** `InboxScreen({required ClaxApi api, required void Function(InboxItem item) onOpen})`; launcher gains `onOpenInbox`-free navigation (pushes `InboxScreen` itself) and `onOpenTarget(String aid, {String? thread, int? version})` passed in by its caller (default no-op until U6 wires it).

- [ ] **Step 1: Failing tests:** subscribe-then-fetch order on `FakeClaxApi.stream()`; rows per kind; Unread/All and kind chips change the query; search debounced (injected timer); paging with `before`; tap marks read and calls `onOpen`; Mark all read sends `upto` = newest `seq`; `inbox_item` event upserts by ID; `resync` refetches; badge count from `inboxSummary` and events.
- [ ] **Step 2–4:** FAIL → implement → PASS.
- [ ] **Step 5:** Commit `Add the inbox across apps`.

### Task W8: Questions in app mode

Spec R§10.2. Risk: high. Depends W6 (both edit `web/shell/src/app/host.ts`).

**Files:** Modify `web/shell/src/app/host.ts` (header protocol; `openQuestions()`; message `{type: "questions", open: n}`; `setDraftText` fills the question card's focused text field when the question sheet has focus), `web/shell/src/app/app.css` (dock the question list as a bottom sheet under `html.app`), `web/shell/src/q/artifact-questions.ts` and `web/shell/src/q/index.ts` (mount `SidebarQuestions` into the app-mode sheet instead of the hidden sidebar); Tests `web/shell/src/app/host.test.ts`, `web/shell/src/q/surfaces.test.ts`.

- [ ] **Step 1: Failing tests:** in app mode the question list mounts into the sheet; `openQuestions` shows it; `questions {open}` posted with the nonce on every count change (from the shared question feed); `setDraftText` reaches a focused Other field; outside app mode nothing changes (existing surface tests pass).
- [ ] **Step 2–4:** `npx vitest run shell/src/app/host.test.ts shell/src/q` FAIL → implement → PASS; `npm run lint`; `npm run check`.
- [ ] **Step 5:** Commit `Dock the shell's question card in app mode and tell the app how many questions are open`.

### Task W7: Playwright and budgets

Risk: low. Depends W5, W6, W8.

**Files:** Create `web/e2e/hold-drag.spec.ts`, `web/e2e/inspect.spec.ts`; Modify `web/perf/bundle-budget.json`.

- [ ] **Step 1:** Specs: touch emulation on `?app=1` with the stubbed `window.ClaxHost` (`web/e2e/app-host.ts`): hold 500 ms then drag over text → the composer shows the selected words; drag from empty space → an area composer; `?app=1&inspect=1` on a page that throws at load → `inspection` has the error and a PNG; `composeArtifact` → composer labelled the whole app; an `ask` question created over the session route shows in the app-mode sheet, `questions {open: 1}` is posted, answering it posts `{open: 0}` (`web/e2e/app-questions.spec.ts`, reusing `web/e2e/questions-helpers.ts`).
- [ ] **Step 2:** Run `npx playwright test hold-drag inspect` → PASS (the implementation exists).
- [ ] **Step 3:** Measure the bundles (`npm run build` then the budget script) and set budgets with the same headroom rule W4 used.
- [ ] **Step 4:** Commit `Cover hold-then-drag and inspect mode end to end; set bundle budgets`.

---

## Group 2

### Task A3: `finish` and `give_up`

Spec R§4.5, R§4.6. Risk: high.

**Files:** Create `apps/clax_app/lib/agent/tools/finish.dart`; Test `test/agent/tools/finish_test.dart`.

**Interfaces:** `FinishTool(ClaxApi api, ChangeLedger ledger, {required RenderTool render, void Function(String)? log})` with `Future<RunOutcome> capStop(String cap, {String? narration})` (R§4.4: publishes `draft.lastClean` with the reply "<narration>. I stopped early (<cap>), so this may not be everything you asked." and no resolve, or, with no clean draft, replies "Couldn't make that change: it ran out of time (<cap>)." and publishes nothing), `GiveUpTool(ClaxApi api, {required RenderTool render})`. Behaviour moved from `AgentLoop._change`/`newApp` (`lib/agent/loop.dart:270-500` and the create path): `_newerComments`, ledger record before publish, `publish` with `if_version` and `addresses`, conflict carry-over or reset (R§4.5 step 2), `_writeDocs` with pins and one retry, `restoreVersion` on refusal, replies, resolves, acknowledge, working off, draft cleanup. Sets `state.outcome` (`RunFinished`, `RunGaveUp`, `RunInterrupted` on status 0, `RunDropped` on 404). Copy the helper functions it needs (`_newerComments`, `_writeDocs`, `_cleanLine`, `_madeBy`) into `finish.dart` as top-level functions; A7 deletes the originals.

- [ ] **Step 1: Failing tests** (`FakeClaxApi`, `MemoryChangeLedger`), port every `loop_test.dart` change case to `finish`, plus:
  - gate: refuses with no render after the last write; refuses after a render with errors; allows after a clean render; new app refuses without title.
  - `ledger_written_before_publish` (the fake records call order).
  - `finish_never_overwrites_a_document_the_person_changed_mid_run`: pinned update; the fake bumps the doc's version before the batch → one re-read and retry, the second conflict restores v(base) and gives `RunGaveUp("its data couldn't be updated: …")`.
  - conflict carry-over (latest equals base on touched paths → published on top), conflict reset (touched path changed → `ToolRefused` with "re-apply" and draft reset to latest, run continues).
  - newer comment left out of addresses and reply.
  - `give_up` replies "Couldn't make that change: <reason>" and publishes nothing.
  - cancel before publish → `RunCancelled`, nothing replied; status 0 → `RunInterrupted` with the ledger kept.
  - draft deleted on every end.
  - `capStop` with a clean snapshot publishes the snapshot (not the later draft), threads stay open; without one, nothing published and the failure reply names the cap.
- [ ] **Step 2–4:** FAIL → implement → PASS.
- [ ] **Step 5:** Commit `Move publishing, pinned document writes and replies into the finish tool`.

### Task U4: Thread detail

Spec R§9.4. Risk: low.

**Files:** Create `apps/clax_app/lib/ui/thread_detail.dart`; Modify `lib/ui/threads_panel.dart` (tap → detail), `lib/ui/app_screen.dart` (pass api, voice, retry callback); Tests `test/ui/thread_detail_test.dart`.

**Interfaces:** `ThreadDetail({required ClaxApi api, required String aid, required String tid, required VoiceInput voice, required bool autoDictate, required void Function(int n) onPreview, required void Function(String tid) onRetry})`.

- [ ] **Step 1: Failing tests:** comments listed with agent replies marked; clip shown when present; Send posts `postOwnerComment`; on an unsent thread **Send to Clax** calls `sendThread`; Resolve/Reopen; "Changed in v5" calls `onPreview(5)`; Retry shown only when the last comment is an agent comment opening "Couldn't make that change:" and calls `onRetry`.
- [ ] **Step 2–4:** FAIL → implement → PASS.
- [ ] **Step 5:** Commit `Add the thread detail page with reply`.

### Task U5: Voice auto-start and cues

Spec R§9.7. Risk: low. Waits for P4 S-TIER (android/, lib/platform).

**Files:** Create `apps/clax_app/lib/platform/voice_cues.dart`, `android/app/src/main/kotlin/dev/empathic/clax_app/CuesChannel.kt`, `macos/Runner/CuesChannel.swift`; Modify `android/app/src/main/kotlin/dev/empathic/clax_app/MainActivity.kt` (register), `macos/Runner/MainFlutterWindow.swift` (register), `lib/platform/settings_store.dart` (`dictationCues`, default true), `lib/ui/settings_screen.dart` (labels "Start dictation automatically", "Dictation sounds and vibration"; last runs' turns and cost from the agent timings), `lib/ui/new_sheet.dart` (auto-start honours the setting); Tests `test/platform/voice_cues_test.dart`, `test/ui/settings_test.dart`, `test/ui/new_sheet_test.dart`.

**Interfaces:** `abstract interface class VoiceCues { Future<void> started(); Future<void> stopped(); }`; `ChannelVoiceCues(MethodChannel('dev.empathic.clax/cues'), {required bool Function() enabled})` calls `HapticFeedback` then `play {cue}`; `NoVoiceCues`.

- [ ] **Step 1: Failing tests:** fake channel receives `play {cue: start}` then `stop`; disabled → nothing; settings round trip of `dictation_cues`; New sheet starts dictation only when auto-start is on.
- [ ] **Step 2–4:** FAIL → implement → PASS; `flutter build apk --debug` compiles the Kotlin (background, logged).
- [ ] **Step 5:** Commit `Add dictation cues and make auto-start a setting everywhere`.

### Task A10: ResumingTurns

Spec R§4.7. Risk: low. Waits for P4 S-TIER and A4.

**Files:** Modify `apps/clax_app/lib/platform/network_pause.dart` (add `ResumingTurns implements TurnBackend`, same rule as `ResumingBackend` per turn); Test `test/platform/network_pause_test.dart`.

- [ ] **Step 1: Failing tests:** a turn failing with a network cut while backgrounded waits for foreground and is sent again with the same request; foreground failure rethrown; at most 3 resumes; cancel while paused.
- [ ] **Step 2–4:** FAIL → implement → PASS.
- [ ] **Step 5:** Commit `Resume an agent turn cut by the background network block`.

---

## Group 3

### Task A6: `ask`, Clax's elicitation

Spec R§8. Risk: high.

**Files:** Create `apps/clax_app/lib/agent/tools/ask_tool.dart`, `lib/agent/asked.dart`; Tests `test/agent/tools/ask_tool_test.dart`, `test/agent/asked_test.dart`. Read first: `docs/contract.md` "Agent questions" (question shape, session routes, "Late answers", "`ask`") and `crates/clax-mcp/src/tools.rs` (the `ask` tool: argument checks, result shape, `call_again`, `surface_open`) — the app's tool mirrors it.

**Interfaces:**

```dart
/// Clax's `ask` for the app session: the same arguments and result as the
/// `ask` MCP tool (docs/contract.md, "`ask`"), over the session routes.
class AskTool implements Tool {
  AskTool(ClaxApi api, AskedQuestions asked, {required Duration maxWaitNew, required Duration maxWaitChange, void Function(QuestionView q)? onAsked});
  // input: {questions: [...]|null, question_id: string|null, timeout_s: int|null, cancel: bool|null}
  // result: {question_id, status, reply, call_again, surface_open, note}
}
class AskedEntry { const AskedEntry({required this.questionId, required this.artifactId, required this.threadIds, required this.commentIds}); … }
abstract interface class AskedQuestions {
  Future<void> put(AskedEntry e); Future<AskedEntry?> take(String questionId); Future<List<AskedEntry>> all(); Future<void> drop(String questionId);
}
class FileAskedQuestions implements AskedQuestions { FileAskedQuestions(File file); } // temp file + rename, as FileChangeLedger
class MemoryAskedQuestions implements AskedQuestions { … }
```

Behaviour: `questions` → `askQuestion(sid, questions, artifactId: ctx.artifactId)` then `waitQuestion(…, wait: min(timeout_s, cap))` with the budget clock paused; `question_id` → wait again; `cancel` → `withdrawQuestion`; caps: 300 s new app, 45 s change; one question set per run (a second `questions` call is refused; `question_id` calls are allowed); daemon errors (`invalid_question`, `limit_reached`) come back as `ToolRefused` with the daemon's message. For a change, every question asked is put in `AskedQuestions` with the run's threads and comment IDs (dropped again when the answer comes back within the run).

- [ ] **Step 1: Failing tests** (`FakeClaxApi` questions):
  - answered within the wait → `{status: "answered", reply: [...], call_again: false}`;
  - declined → `{status: "declined"}`;
  - still open → `{status: "open", call_again: true}` and the entry kept in `AskedQuestions`;
  - `question_id` waits again; `cancel` withdraws;
  - `timeout_s` 600 on a change is waited as 45 (injected clock), and the run's wall clock does not advance during it;
  - `surface_open: false` passed through;
  - second `questions` refused; `invalid_question` surfaced;
  - `FileAskedQuestions` survives a reload.
- [ ] **Step 2–4:** FAIL → implement → PASS.
- [ ] **Step 5:** Commit `Let the app's agent ask the owner with Clax's ask, and remember what each open question was for`.

---

## Group 4

### Task A7: AgentLoop and FeedbackWorker on runners

Spec R§2, R§4, R§8, R§9.5, R§9.6. Risk: high.

**Files:** Modify `apps/clax_app/lib/agent/loop.dart` (bodies of `newApp` and `change` build a `RunContext`, a `Toolbox` with every tool, a `RunInput` from A9's first messages, and call the runner; map `RunOutcome` → `ChangeOutcome`; delete the code A3 moved), `lib/agent/worker.dart` (`WorkerProgress(artifactId, threads, AgentProgress event)`, `retry(aid, tid)`, `_failed` map rebuilt from threads on start, a feedback poll's late `answers` (`FeedbackBatch.answers`) looked up in `AskedQuestions` and re-run with the batch's late-answer text; an entry whose threads got a newer viewer comment is dropped), `lib/agent/timings.dart` (turns, tokens, cost on agent records); Create `lib/agent/runner/scripted_runner.dart`; Keep the single-call path for `ClaudeCliBackend` (macOS without an API key, R§4.3): `AgentLoop` takes either a runner factory or, for `claude-cli`, today's `AgentBackend` and runs the existing structured call; `MessagesApiBackend.run`, `edit_schema.dart` and `scripted_backend.dart` stay for that path and its tests; Tests `test/agent/loop_test.dart`, `test/agent/worker_test.dart`, `test/agent/worker_restart_test.dart` (rewritten on `ScriptedRunner`), `test/agent/loop_support.dart`.

**Interfaces:** `AgentLoop({required ClaxApi api, required AgentRunner Function() runner, required PromptBuilder prompts, required String Function() model, required Timings timings, required Inspector inspector, required Uri baseUrl, ChangeLedger? ledger, AskedQuestions? asked, AgentBackend Function()? singleCall, Clock clock}) — `singleCall` non-null means the CLI fallback: no tools, today's behaviour`; `Future<String> newApp(String request, {Future<void>? cancelled, ProgressSink? progress})`; `Future<ChangeOutcome> change(String aid, List<FeedbackItem> items, {String? session, Future<void>? cancelled, ProgressSink? progress, String? answers})`. `ScriptedRunner(List<ScriptedCall> Function(RunInput) script)` calls tools on the real Toolbox (`ScriptedCall{tool, input, narration?}`). `FeedbackWorker.retry(String aid, String tid)`. Also used by `main.dart`'s `CLAX_SCRIPTED_AGENT` debug path (keep it working with `ScriptedRunner`).

- [ ] **Step 1: Failing tests:** every existing loop/worker behaviour re-expressed with `ScriptedRunner` (new app create, change landed, answered without version, failure reply, cancel, interrupted, dropped, queue merging, `second_batch_runs_on_the_landed_version`, recovery rule of record 74211b60); plus `kill_after_publish_recovers_with_one_version` (ledger written, publish lands, process "dies" before replies → new worker's `settle` replies once naming the version, no second publish, the stale draft deleted by the start sweep call `listDrafts`/`deleteDraft`); `WorkerProgress` carries the run's events with its threads; `retry` re-runs the failed comments without posting; a late answer re-runs the asked change with the daemon's late-answer text, a skip likewise, a stale entry (newer comment) is dropped; the CLI single-call path still passes the old loop tests; `newApp` forwards progress.
- [ ] **Step 2–4:** `flutter test test/agent` FAIL → implement → PASS; `flutter analyze`.
- [ ] **Step 5:** Commit `Run new apps and changes as tool loops; report progress; retry failed changes`.

---

## Group 5

### Task U6: WebViewInspector and runtime wiring

Spec R§5.2, R§4.7. Risk: high. Waits for P4 S-TIER.

**Files:** Create `apps/clax_app/lib/ui/inspector_surface.dart`; Modify `lib/app.dart`; Tests `test/ui/inspector_surface_test.dart`, `test/app_test.dart`.

**Interfaces:** `class WebViewInspector implements Inspector` driving an `InspectorHost` widget (`InspectorHost({required WebViewInspector inspector, required Size size})`) mounted at the bottom of the root `Stack` under an opaque cover, never `Offstage`; per load: fresh nonce, `init`, `setVisible(true)`, wait `ready` (10 s, injected clock), `Inspect`, wait `inspection` (10 s); one render at a time. `app.dart`: with an API key, `MessagesApiRunner(ResumingTurns(MessagesApiBackend(...)))` on both platforms; on macOS with the "Claude Code on this Mac" setting and no key, `singleCall: () => StagedBackend(ClaudeCliBackend(...))` as today; pass the inspector, base URL, `FileAskedQuestions('<support>/asked.json')`, `FileChangeLedger`; hold one `ClaxStream` subscribed to `questions` (and `inbox`) for the app's life so it is an owner surface; feed `WorkerProgress`/making progress to `BackgroundRuns.step` (step text = latest `Narration`/`ToolStarted` label); at start delete leftover drafts (`listDrafts` → `deleteDraft`); launcher `onOpenTarget` → app screen with `openThread`/`loadVersion`.

- [ ] **Step 1: Failing tests:** with `FakeAppSurface` for the inspector's WebView: nonce per load, `setVisible(true)` sent, `inspection` turned into `Inspection`, timeouts → `InspectorError`, concurrent calls serialized; `app_test`: runner vs CLI single call per platform, key and settings, drafts cleaned at start, progress reaches a fake `BackgroundWork.update`.
- [ ] **Step 2–4:** FAIL → implement → PASS; `flutter build macos --debug` (background) to check the platform view builds.
- [ ] **Step 5:** Commit `Render drafts in an in-app WebView and wire the tool loop into the app`.

### Task U1: Making screen

Spec R§7. Risk: low.

**Files:** Modify `apps/clax_app/lib/ui/making_screen.dart`; Test `test/ui/making_screen_test.dart`.

**Interfaces:** `MakeApp` becomes `Future<String> Function(String request, Future<void> cancelled, ProgressSink progress)`; `MakeStage` keeps `paused`/`resuming` (P4) and drops `retrying`; a `PreviewView` interface (`loadHtml(String)`) with the webview implementation and a fake for tests.

- [ ] **Step 1: Failing tests:** `PartialFile('index.html', …)` events throttled to ≤ 4 loads/s (injected clock); `Inspected` replaces the preview with the PNG and shows an error count; narration line updates; a question event shows the inline sheet; Cancel still cancels; say→first-output recorded at the first preview load.
- [ ] **Step 2–4:** FAIL → implement → PASS; render PNGs with `matchesGoldenFile` off-CI as the old P2 asked (`target/flutter-app/u1/`).
- [ ] **Step 5:** Commit `Show the new app as it is written, then as it renders`.

### Task U2: Working chip narration and Retry

Spec R§9.5, R§9.6. Risk: low.

**Files:** Create `apps/clax_app/lib/ui/working_chip.dart`; Modify `lib/ui/change_tracker.dart` (latest step per change from `WorkerProgress`, failure keeps its threads for Retry), `lib/ui/app_screen.dart`; Tests `test/ui/change_tracker_test.dart`, `test/ui/working_chip_test.dart`, `test/ui/app_screen_test.dart`.

- [ ] **Step 1: Failing tests:** chip shows the latest step and elapsed; expand shows three steps and the inspection thumbnail; steps of another thread's change ignored; failure shows the reason and Retry → `worker.retry(aid, tid)` and the chip goes back to running; send→first-step timing recorded.
- [ ] **Step 2–4:** FAIL → implement → PASS.
- [ ] **Step 5:** Commit `Narrate a change in the Working chip and retry a failed one`.

---

## Group 6

### Task N2: Question sheets

Spec R§10.2, R§8. Risk: low. Depends C1, A6, U6, W8.

**Files:** Create `apps/clax_app/lib/ui/question_sheet.dart`; Modify `lib/ui/app_screen.dart` (question badge on the comment button from the `questions {open}` host message; `OpenQuestions` when a new question about this app arrives in front), `lib/app.dart` (presenter on the `questions` stream topic: in front and the question's artifact on screen → `OpenQuestions`; otherwise a `QuestionWebSheet` on `<base>/inbox?q=<qid>`; in the background → the P4 notification, whose tap opens the sheet); Tests `test/ui/question_sheet_test.dart`, `test/ui/app_screen_test.dart`, `test/app_test.dart`.

**Interfaces:** `QuestionWebSheet({required Uri url, required Stream<QuestionView> updates, required String questionId})` — a WebView (via the `AppSurface` interface, `FakeAppSurface` in tests) that closes when an update shows the question no longer `open`. Host protocol additions come from U3 (`OpenQuestions` call, `QuestionsMsg{open}` message).

- [ ] **Step 1: Failing tests:** badge count follows `QuestionsMsg`; a `question` event about the shown app while in front sends `OpenQuestions` and a light haptic; a question about no artifact or another app opens the web sheet at `/inbox?q=<qid>`; the sheet closes on `answered`/`declined`/`withdrawn`; in the background a notification is posted instead.
- [ ] **Step 2–4:** FAIL → implement → PASS.
- [ ] **Step 5:** Commit `Answer the agent's questions in the shell's question card, from the app`.

### Task I1: macOS integration test

Spec R§12. Risk: high.

**Files:** Modify `apps/clax_app/integration_test/full_loop_test.dart`, `integration_test/script.dart`.

- [ ] **Step 1:** Script with `ScriptedRunner`: new app = `write_file` index.html (a counter with `db`), `render`, `finish`; change = `str_replace` the label, `render`, `finish`; run against the release daemon from `build-native/macos` and the real `WebViewInspector`. Assert: the new app opens; the render returned a non-empty PNG and no errors; the change lands in place with the document kept; no draft remains (`listDrafts` empty).
- [ ] **Step 2:** `just app-integration` in the background, logged to `target/flutter-app/i1.log` → PASS.
- [ ] **Step 3:** Commit `Exercise the tool loop end to end on macOS with the real daemon and inspector`.

---

## Group 7

### Task D2: Contract and usage

Risk: low. **Files:** `docs/contract.md` ("The Clax app": drafts routes, inspect mode, host `inspect`/`inspection`/`composeArtifact`, hold-then-drag on touch, the app agent's tools and caps; drafts hidden from listings), `docs/usage.md` (the app's comment button, hold-drag, inbox, questions, versions diff). Commit `Document drafts, inspect mode and the app's round-2 features`.

### Task L2: Live check

Risk: low. At most 3 live calls, macOS debug build, scratch `CLAX_HOME` under `target/`. With an API key available (ask the coordinator; none is on this machine): the tool loop — (1) a new app ("a tracker for my runs with a weekly total": say→first-output, say→usable, turns, cost), (2) a change through the shell's Send (send→first-step, long-press→landed, turns, cost), (3) spare, only to repeat a failed run after its fix. Without a key: the same two runs through the desktop fallback (`claude -p`, single call), and the tool loop's live numbers are left to the owner's phone run, said so in the results. Results in `target/flutter-app/l2/RESULTS.md`; summary in the ledger.

### Task V2: Release builds, gates, verification

Risk: low. `just app-apk`, `just app-macos`, `scripts/quality_gates.sh` once (rerun a failing lane alone and report both), append a round-2 section to `docs/superpowers/verification/2026-10-12-flutter-app.md` (what ran, the L2 numbers against R§6.5's targets, what is unverified on a device: Android inspector rendering under the cover, hold-drag on a real touch screen, cue tones).
