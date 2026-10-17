# Asking Where Attention Is Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every agent (Claude Code, Codex, Grok, Pi) asks its person on the one surface the person is paying attention to, terminal or Clax, at the narrowest place (thread, else artifact, else inbox), and tells the other surface with a notice that can pull the question over.

**Architecture:** The daemon keeps per-session engagement signals (last terminal prompt, typed or inferred; last Clax delivery and its thread) in the store (`clax-store`), and per-tab focus and last input in the in-memory presence registry (`clax_core::presence`). A pure function, `clax_core::surface::suggest`, turns those signals into `{surface, where, confidence, why}` by the daemon's clock; `clax_http::surface` gathers the signals for a session (store, presence, the stream hub's `holds_surface` and new `holds_panel` in `clax-realtime`), and the routes in `clax-server` expose the suggestion on a session route, the `ask` results, the feedback text and a new `attention` MCP tool, and use it (instead of `Hub::holds_surface` alone) to choose the AskUserQuestion hook's mode. Questions gain a thread and a notice flag, so the shell can render a question inside its thread card and show "asking you in the terminal" notices with **Ask me here**.

**Tech Stack:** Rust (clax-store store and migrations on rusqlite; clax-core pure rules and presence; clax-realtime stream hub and working records; clax-http shared route code; clax-server axum routes; clax-testkit test daemon; clax-hooks; clax-mcp rmcp tools; wasm32 Worker build), TypeScript (Pi extension, vitest), Svelte 5 shell (vitest + jsdom), Playwright e2e. Built and tested with Bazel through `just` and `bazelisk test`.

**Spec:** `docs/superpowers/specs/2026-10-16-ask-where-attention-is-design.md`

## Changes since the plan was written

The branch was rebased onto main's workspace layout (`docs/superpowers/specs/2026-10-16-workspace-layout-design.md`, §2.1 for the crates and the layering rule, §10 for the rulings made while carrying it out). Every path, line range and command below was checked against the tree at `18fbd24b0`.

**Where each piece now lives**

- **The store** (question rows, the new `engagement` module, migrations): `crates/clax-store/src/` (`clax_store::…`; inside the crate `crate::questions`, `crate::test_util`, and `clax_core::…` for core types). The branch's last migration is 36 (members' `clax-ask` rights, watch relay names), so this feature's is **37**.
- **The suggestion** stays in `clax-core` (`clax_core::surface`), a pure rule as that crate holds. **Presence** did not move: the registry is `crates/clax-core/src/presence.rs`; `clax-realtime/src/presence.rs` only fans its changes out.
- **The stream hub** (`holds_surface`, the new `holds_panel`) and **`mark_items`** are `clax-realtime`'s (`crates/clax-realtime/src/stream.rs`, `working.rs`). `mark_items` now takes a `FeedbackCtx`, not the state.
- **The question view, `announce`, `announce_ids`, the inbox item view, the extension gateway and the agent needs** (`need!`, `ROUTES`, `ROUTE_NEEDS`) are `clax-http`'s: `crates/clax-http/src/views/question.rs`, `views/inbox.rs`, `extension.rs`, `agent.rs`.
- **The router** is `crates/clax-server/src/router.rs`; the route guards' tables (the class table `GUARDS`, `EXPECTED`, `MUST_REFUSE`, and `module_source`, which `include_str!`s each route module) are `crates/clax-server/src/routes/guards.rs`; `routes/mod.rs` only declares the modules.
- **The server-side signal gathering** (`page`, `signals`, `suggestion`, later `note_poll` and `INFERRED`), which this plan first put in `crates/clax-server/src/surface.rs`, is **`crates/clax-http/src/surface.rs`** (`clax_http::surface`): it reads `AppState`, which is `clax-http`'s, and three route modules (`surface`, `questions`, `feedback`) share it. It uses only `clax-realtime`, `clax-store` and `clax-core`, as the layering check (`//scripts:crate_layers`, `scripts/check-crate-layers.py`) allows.
- **Test support** is `crates/clax-testkit` (`clax_testkit::TestServer`, its root re-exporting its modules); there is no `tests/common`.

**What changed shape**

1. **Integration tests are split.** Bazel deals the modules of `crates/clax-server/tests/integration.rs` into six targets (`integration__1` … `__6`, `clax_split`), so one test module cannot `use crate::<another module>`: the two may be compiled apart. The helpers Task 6 shares with Task 5's tests go in the testkit (`crates/clax-testkit/src/where_to_ask.rs`, `pub mod where_to_ask`), and each new module is gated in `integration.rs` with `#[cfg(any(not(clax_split), clax_split = "<module>"))]`.
2. **The session's page goes through the agent's gate.** Since the plan was written, `questions::create` passes a person's agent's fallback page through `Agent::named_artifact` (`crates/clax-http/src/agent.rs:736-755`): a live page, or one the person's rights do not let their agent read, is no page. The plan's `page_of` read `Working::newest_artifact_of` and `Store::session_artifact` raw, so `attention` could name such a page to a person's agent. `clax_http::surface::page` (async) now resolves the page through that gate before the store call, and `signals` and `suggestion` take it as an argument (Task 5).
3. **Linked questions** (migration 35, `crates/clax-server/src/linked.rs`) copy every open `ask` question of the owner's agents about a linked artifact to its remote (`Store::scan_linked_questions`, `crates/clax-store/src/linked.rs:1175-1222`). A notice copied there would be an answerable question, and the remote's answer would fail here with `not_answerable`. Task 1 leaves notices out of the scan. A thread question's copy is asked about the remote artifact without its thread (the local thread ID is not the remote's); Task 16 records that as a concern.
4. **The feedback poll builds its audit context inside its wait loop** (the first iteration's store call, `crates/clax-server/src/routes/feedback.rs:329-343`), not before it. Task 7's `note_poll` runs in that first build, so once per poll.
5. **The question create route** also has a keyed path (`create_question_keyed`, an agent token's `Idempotency-Key`), and its `released` rule already holds a hook question in Clax only beside a page of its session's (the reply `6c71e6ae` on record `27bb8860`). Task 6 keeps both.
6. **`ask` also carries approvals** (`approval`, `approval_id`) in MCP and Pi. Task 9 checks `thread_id` and `notice` before the approval hand-off, and its description keeps the approval sentence.
7. **The tool count is already twenty-nine** (`fork`, `publish_public`, `comments_handoff`, `draft`, `git_remote` landed); `attention` makes thirty (Task 10).
8. **Qualifier records moved.** `fabba150` is closed as a duplicate (`adf4c209`); the open root is `27bb8860` in `crates/clax-realtime/src/.qual`, pinned to `crates/clax-realtime/src/stream.rs:1402-1427` (drifted: `holds_surface` is at 1414-1430), tagged `status:needs-decision`, with the reply `6c71e6ae`. `73ffc4d2` is resolved (`36f61b79`, fixed in `9be93c11`), so Task 16 no longer records the concern built on it.
9. **"Attention" and "notice" already have meanings.** `attention` names each viewer's attention per artifact (`crates/clax-store/src/attention.rs`, `GET /api/viewers/me/attention`, the artifact JSON's `attention` field), and "notices" names `GET /api/sessions/{id}/notices` (Grok's monitor, `docs/contract.md` "### Notices (Grok's monitor)"). The new `attention` tool, route and `ask` result field give the word a second meaning. No tool, route or contract section is named `attention` yet, so nothing collides in code; the contract text must tell the two apart, and Task 16 asks the owner whether to keep the name.
10. **The hub's `holds_surface` reads the topics' channels and scans no stream**; `holds_panel` (Task 5) scans the hub's streams (bounded by the detached-stream limit, `stream.rs:58`, and each client's socket cap, `clax_http::socket_caps`), once per suggestion.

**Builds, tests and commits**

- Tests run under Bazel (AGENTS.md: no `cargo test`, `cargo build` or `pnpm install` in a worktree except through a `-native` recipe). The targets: unit tests `//crates/<crate>:<lib>_lib_test` (`clax_store_lib_test`, `clax_core_lib_test`, `clax_realtime_lib_test`, `clax_http_lib_test`, `clax_server_lib_test`, …); clax-server's integration modules in `//crates/clax-server:integration__1` … `__6` (there is no plain `:integration` target, though `docs/bazel.md:85` names one), so name `//crates/clax-server:all` with a filter. Filters go in `--test_arg=` (one per filter). Bazel builds and supplies `CLAX_TEST_BIN`, so the `scripts/stable-bin.sh` prefix goes. Web: `just web-test`; Pi: `just pi-test`; the browser tests: `//web:e2e_rest`; the Worker: `just worker-test` (its own workspace, outside Bazel); the wasm32 check: `just check-wasm`; the whole suite once before merging: `just ci`, in the background, logged under `target/`.
- Commits: `git -c commit.gpgsign=false commit -m "…"` (AGENTS.md: the owner signs; never push unsigned commits).

**Stale citations in the spec** (the spec is unchanged; these are the current locations)

| Spec cites | Now |
|---|---|
| `crates/clax-mcp/src/tools.rs:35-37` | `crates/clax-mcp/src/tools.rs:38-40` |
| `plugins/claude-code/skills/clax/SKILL.md:354-386` | `:372-405` |
| `Hub::holds_surface`, `crates/clax-server/src/stream.rs:1325-1343` | `crates/clax-realtime/src/stream.rs:1414-1430` |
| `web/shell/src/stream.ts:23-27` | unchanged |
| record `fabba150` | `27bb8860` (`crates/clax-realtime/src/.qual`) |
| migration at `crates/clax-core/src/store/migrations.rs:380-401` | `crates/clax-store/src/migrations.rs:380-401` |
| "Migration 36 (the next free one)" | migration 37 |
| `working::mark_items`, `crates/clax-server/src/working.rs:62` | `crates/clax-realtime/src/working.rs:70` |
| `events::prompt`, `crates/clax-hooks/src/events.rs:291-299` | `:536-547` |
| Grok's no-op prompt hook, `crates/clax-cli/src/commands/hook.rs:889-894` | `crates/clax-commands/src/commands/hook.rs:1012-1017` |
| `web/shell/src/view/presence-model.ts:9-10`, `web/shell/src/stream-hub.ts:26-30` | unchanged |
| the `asked` hook, `crates/clax-hooks/src/ask.rs:207-228` | `:201-228` |
| `release_owner`, `crates/clax-server/src/routes/questions.rs:735-750` | `:733-749` |

## Global Constraints

- No `unsafe`: the workspace lints forbid it (`unsafe_code = "forbid"` in `Cargo.toml`).
- Never create hard links, in code, tests or scripts.
- Each crate depends only on the crates above it in the layout spec's §2.1 (`clax-core` → `clax-store` → `clax-realtime` → `clax-http` → `clax-server`; `clax-testkit` for integration tests only), checked by `//scripts:crate_layers`. Nothing here adds a dependency edge: `clax_core::surface` is pure; `clax_http::surface` reads `clax-realtime`, `clax-store` and `clax-core`; the routes stay in `clax-server`; shared test helpers go in `clax-testkit`, never in a crate's unit tests.
- Migration 37 is the next free number on this branch (`crates/clax-store/src/migrations.rs` ends at 36, members' `clax-ask` rights and watch relay names); verify before writing it, and renumber if another branch lands first (AGENTS.md: a branch edits its own unmerged migrations in place; dev homes its earlier builds migrated are reset).
- Tests are fast and load-tolerant: no sleeps standing in for a condition, no wall-clock budgets in functional tests, injected clocks (`clax_core::working::ManualClock`, `TestServer::spawn_with_clock`, the shell's `wall()`/`after()` test clock) for timing rules.
- Write "ID" in prose, never "id" (except as a literal symbol in code).
- Doc comments and commit messages describe the contract or the change, never the conversation that produced it or a rating of its quality.
- Never run an unmerged build against `~/.clax` or the daemon on port 7480: use `just watch`/`just dev` (`~/.clax-dev`, port 7481) or a scratch `CLAX_HOME` on a free port, and stop any daemon you start by its PID.
- Before editing a file, run `"/Users/alex/.claude/plugins/cache/qualifier/qual/0.1.0/scripts/ensure-qualifier.sh" exec threads <path>` and read what is open on it.
- Commits are signed by the owner. Agents commit with `git -c commit.gpgsign=false commit -m "…"`; never push unsigned commits.
- UI changes need browser verification at 1280 and 390 wide, light and dark, before they are called done; "tests pass" is not verification.
- Tests run under Bazel: `bazelisk test <targets> --test_arg=<filter>` while working (targets in "Changes since the plan was written"), `just ci` once before the branch merges, in the background with its log under `target/`. Never `cargo test`, `cargo build` or `pnpm install` in the worktree except through a `-native` recipe. Bazel builds the test `clax` and supplies `CLAX_TEST_BIN`; natively (`just test-native`), give tests a binary kept by content, `CLAX_TEST_BIN="$(scripts/stable-bin.sh target/debug/clax target/clax-bin/debug)"`, and never `cp` over an existing binary. A lane that fails in code this plan did not touch is rerun alone and both results reported.
- Every new server, HTTP, realtime, store and core path must build for `wasm32-unknown-unknown` (`just check-wasm`, which checks `clax-core`, `clax-rt`, `clax-store`, `clax-sample`, `clax-realtime`, `clax-http`, `clax-mcp`, `clax-server` and `clax-content-git`): no `std::time::Instant::now`, threads or file system in code the Worker runs; time comes from the injected `Clock`, `clax_rt::time` or `clax_rt::wall`.
- Every signal time is the daemon's clock: a browser sends how long ago its last input was (`idle_ms`), never a timestamp, and a hook's request is stamped when the daemon receives it.

## Review Focus

1. **A session with no artifact.** A hook question of a session with no page is created released (asked in the terminal) with no notice and a read inbox item, as today; `attention` names no `artifact_id`; an agent's `ask {notice: true}` with no `url_or_id` lands in the inbox. Tests: Task 4 `a_session_with_no_page_names_no_place`; Task 6 `a_hook_question_of_a_session_with_no_page_is_asked_in_the_terminal_without_a_notice`.
2. **A thread deleted or resolved between the suggestion and the ask.** The store refuses a `thread_id` that is not a thread of the named artifact (`invalid_thread`); a delivery whose thread is gone places nothing narrower than its artifact; a question whose thread is gone or resolved shows in the sidebar's block as a full card. Tests: Task 1 `a_thread_question_is_refused_off_its_artifact`; Task 5 `a_deleted_delivered_thread_places_the_suggestion_on_its_artifact`; Task 6 `a_deleted_thread_leaves_its_question_on_the_artifact`; Task 13 vitest `a question whose thread is gone or resolved is a card in the sidebar block`.
3. **A person with two tabs, one focused on another artifact.** Engagement counts only the person's tabs on the session's pages (its delivery's artifact, its newest working record's, the one it publishes to or watches). For a person's agent the page goes through `Agent::named_artifact` first, so a live page or one their rights do not let it read is no page. Tests: Task 5 `focus_on_another_artifact_is_not_attention_on_the_sessions_page`, `a_persons_agent_is_never_shown_a_page_it_may_not_read`.
4. **Clock skew between the hook machine, the browser and the daemon.** The browser reports `idle_ms`; the daemon computes the input time from its own clock; prompts and turns are stamped by the store's clock. Tests: Task 3 `input_time_is_the_daemons_report_time_less_idle_ms`; Task 12 vitest `reports focus and idle time, never a timestamp`.
5. **A connected or hosted (clax.dev) agent token whose terminal is on another machine.** Its session's engagement is its person's (presence keyed by the session's `person`), never the owner's; its signed requests to `POST …/terminal` and `GET …/attention` are accepted (`Own` class, `Ask` right). Test: Task 5 `a_persons_agent_reads_its_persons_engagement_not_the_owners`.
6. **The Worker build (wasm).** `KeptPresence` carries focus and input across a Durable Object wake with serde defaults, so state written before this change still restores. Tests: Task 3 `kept_reports_keep_focus_and_input_across_a_wake` and an old-shape restore case; Tasks 3, 5 and 6 run `just check-wasm` and `just worker-test`.
7. **A question about a linked artifact.** The owner's agents' open `ask` questions about a linked artifact are copied to its remote (`crates/clax-server/src/linked.rs`). A notice is never copied (it takes no answer, and the remote's answer would fail here); a thread question's copy is asked about the remote artifact without the thread. Test: Task 1 `a_notice_about_a_linked_artifact_is_not_copied`.

## Choices the spec leaves open

- A notice is a `questions` row with `notice = 1`, status `open` until it closes: answered in the terminal (`asked`), withdrawn by its agent, pulled (**Ask me here**, reason `pulled`), lapsed (reason `lapsed`: the person typed a new prompt to the session, so the dialog Esc closed is gone), or its session ended. It takes no answer in Clax (`not_answerable`).
- A hook notice keeps the full questions (needed to record the terminal's answer by question text, as released questions do today); the shell shows only the headers.
- A hook question of a session with no page stays released with no notice (today's rule): that person is not using Clax for this session, so the inbox gets nothing to look at.
- With no Clax tab or panel of the person's open at all (no stream holds `questions` or `inbox`), the suggestion is the terminal, clear: the unsure lean to Clax applies only when Clax can show the question.
- The extension's side panel open makes Clax available and is named in `why`; its focus is unknown, so it never makes a suggestion clear on its own.
- An inferred terminal turn (Codex, Grok) never gives a clear terminal suggestion: when it would decide, the suggestion is Clax, unsure.
- **Ask me here** reaches the agent on the tiers late answers ride (tool results, Stop and prompt hooks, `wait_for_feedback`, Pi's injection); the Claude Code channel notice stays comment-only, as answers are today (2026-10-06 spec §13).
- Claude Code's prompt hook reports the prompt on its existing feedback poll (`prompt=true`), keeping it to one request within the hook's 4 s deadline; Pi reports through `POST /api/sessions/<sid>/terminal`.
- Codex and Grok turns are inferred in the daemon, from their Stop hook's feedback poll, so their plugins and hooks do not change.
- The suggestion is added to a delivery's text (and to the prompt hook's context when the session has a page) as one `[clax] If you ask the person anything now, …` line, which covers "the context the Claude Code and Pi plugins add".
- The `ask` result's `url` becomes the narrowest place (`/a/<aid>?thread=<tid>`, `/a/<aid>`, else `/inbox?q=<qid>`), and `tell_terminal` is `Asked in Clax: <url>`.
- The skills' shared four-step rule is a new `## Where to ask` section, identical in all four skills and checked word for word by `scripts/test-plugins.sh`.

---

## Task 1: Migration 37 and question rows: thread, notice, pull, lapse

**Files:**
- Modify: `crates/clax-store/src/migrations.rs:1103-1114` (append migration 37 after 36, before the closing `];` at line 1114), tests module (from line 1194; add a test near `migration_20_adds_questions_to_a_19_database`, line 1391)
- Modify: `crates/clax-store/src/questions.rs:79-108` (`QuestionRow`, `NewQuestion`), `:110-206` (`Close`, `Reason`), `:322-395` (`select!`, `late!`, `OPEN_HOOKS`, `WITHDRAW_HOOKS`), `:414-439` (`row`), `:546-551` (`not_mirrored`, beside which the new error helpers go), `:578-708` (`create_question_keyed`), `:800-875` (`close_question`), tests module (from line 981)
- Modify: `crates/clax-store/src/inbox.rs:1181-1195` (`note_question` carries the thread), `:1200-1230` (`question_changed`)
- Modify: `crates/clax-store/src/linked.rs:1175-1222` (`scan_linked_questions` leaves notices out), tests (beside `only_the_owners_agents_open_asks_on_a_two_way_link_are_paired_once`)
- Modify: `crates/clax-store/src/toolpath/tests.rs:2707-2708` (actors for the two new reasons)
- Modify (add `thread_id: None, notice: false,` to each `NewQuestion {` literal; `git grep -n "NewQuestion {" -- crates`): `crates/clax-server/src/routes/questions.rs:286`, `crates/clax-server/src/routes/remotes.rs:784`, `crates/clax-store/src/backfill.rs:2979`, `crates/clax-store/src/inbox.rs:2144`, `crates/clax-store/src/linked.rs:1842,1936`, `crates/clax-store/src/questions.rs:995` (the tests' `new`), `crates/clax-store/src/sessions.rs:3181`, `crates/clax-store/src/toolpath/tests.rs:831`

**Interfaces:**
- Consumes: `Store::with_tx`, `Store::now`, `close_record`, `inbox::note_question`, `inbox::question_changed`.
- Produces:
  - `QuestionRow { …, pub thread_id: Option<String>, pub notice: bool, pub pulled_at: Option<String> }`
  - `NewQuestion { …, pub thread_id: Option<String>, pub notice: bool }`
  - `Close::Pull`, `Close::Lapse`; `Close::Terminal` also closes an open hook notice
  - `Reason::Pulled` (`"pulled"`), `Reason::Lapsed` (`"lapsed"`), both `question.withdraw`; `Reason::ALL: [Reason; 10]`
  - Errors: `invalid_thread`, `not_answerable`, `not_a_notice`
  - `LATE_ANSWERS` also returns pulled notices not yet taken

- [ ] **Step 1: Read the open threads on the files**

Run: `"/Users/alex/.claude/plugins/cache/qualifier/qual/0.1.0/scripts/ensure-qualifier.sh" exec threads crates/clax-store/src/questions.rs` (and the same for `crates/clax-store/src/migrations.rs`, `inbox.rs`, `linked.rs`). Read what is open on each.

- [ ] **Step 2: Write the failing tests**

In `crates/clax-store/src/migrations.rs` tests, add:

```rust
    #[test]
    fn migration_37_adds_where_attention_is() {
        let (_d, st) = crate::test_util::store();
        let cols = |t: &str| -> Vec<String> {
            st.with_read(|c| {
                let mut s = c.prepare(&format!("SELECT name FROM pragma_table_info('{t}')"))?;
                Ok(s.query_map([], |r| r.get(0))?
                    .collect::<rusqlite::Result<Vec<String>>>()?)
            })
            .unwrap()
        };
        let q = cols("questions");
        for c in ["thread_id", "notice", "pulled_at"] {
            assert!(q.iter().any(|x| x == c), "questions.{c}");
        }
        let s = cols("sessions");
        for c in [
            "terminal_at",
            "terminal_inferred",
            "turn_at",
            "delivered_at",
            "delivered_artifact_id",
            "delivered_thread_id",
        ] {
            assert!(s.iter().any(|x| x == c), "sessions.{c}");
        }
        assert_eq!(MIGRATIONS.len(), 37);
    }
```

In `crates/clax-store/src/questions.rs` tests (after `fn a() -> Vec<Answer>`, line 1005), add:

```rust
    fn thread_on(st: &Store, aid: &clax_core::ArtifactId) -> String {
        st.create_thread(
            DAEMON,
            aid,
            crate::threads::NewThread {
                author_public_id: Some("u_mia".into()),
                version_n: 1,
                anchor: crate::test_util::anchor(),
                body: "here".into(),
                author_name: "Mia".into(),
                clip: None,
                via_page: false,
            },
        )
        .unwrap()
        .id
    }

    fn invalid(r: Result<impl std::fmt::Debug>, want: &str) {
        match r {
            Err(CoreError::Invalid { code, .. }) => assert_eq!(code, want),
            other => panic!("expected {want}, got {other:?}"),
        }
    }

    #[test]
    fn a_thread_question_is_refused_off_its_artifact() {
        let (_d, st) = store();
        let sid = session(&st, "claude", "h1");
        let a = crate::test_util::artifact(&st, Some(&sid));
        let b = crate::test_util::artifact(&st, Some(&sid));
        let t = thread_on(&st, &a);
        let on = |aid: Option<&clax_core::ArtifactId>, tid: &str| {
            st.create_question(
                DAEMON,
                NewQuestion {
                    artifact_id: aid.map(|x| x.as_str().to_string()),
                    thread_id: Some(tid.into()),
                    ..new(&sid, Source::Ask, None)
                },
            )
        };
        invalid(on(Some(&b), &t), "invalid_thread");
        invalid(on(None, &t), "invalid_thread");
        invalid(on(Some(&a), "tmissing"), "invalid_thread");
        let (q, made) = on(Some(&a), &t).unwrap();
        assert!(made);
        assert_eq!(q.thread_id.as_deref(), Some(t.as_str()));
        assert!(!q.notice);
    }

    #[test]
    fn a_notice_takes_no_answer_and_is_pulled_once() {
        let (_d, st) = store();
        let sid = session(&st, "claude", "h1");
        let n = st
            .create_question(
                DAEMON,
                NewQuestion {
                    notice: true,
                    ..new(&sid, Source::Ask, None)
                },
            )
            .unwrap()
            .0;
        assert!(n.notice && n.status == Status::Open);
        invalid(
            st.close_question(DAEMON, &n.id, Close::Answer { answers: a(), via: "shell" }),
            "not_answerable",
        );
        invalid(st.close_question(DAEMON, &n.id, Close::Decline), "not_answerable");
        let q = st.create_question(DAEMON, new(&sid, Source::Ask, None)).unwrap().0;
        invalid(st.close_question(DAEMON, &q.id, Close::Pull), "not_a_notice");
        let unread = st.inbox_unread(None).unwrap();
        let pulled = st.close_question(DAEMON, &n.id, Close::Pull).unwrap();
        assert_eq!(pulled.status, Status::Withdrawn);
        assert!(pulled.pulled_at.is_some());
        assert_eq!(st.inbox_unread(None).unwrap(), unread - 1, "the person acted: its item is read");
        invalid(st.close_question(DAEMON, &n.id, Close::Pull), "question_closed");
        let late: Vec<String> = st.take_late_answers(&sid, |_| false).unwrap().into_iter().map(|q| q.id).collect();
        assert_eq!(late, vec![n.id.clone()], "handed over once, as a late answer");
        assert!(st.take_late_answers(&sid, |_| false).unwrap().is_empty());
    }

    #[test]
    fn a_hook_notice_takes_the_terminal_answer_or_lapses_and_outlives_a_restart() {
        let (_d, st) = store();
        let sid = session(&st, "claude", "h1");
        let notice = |t: &str| {
            st.create_question(
                DAEMON,
                NewQuestion {
                    notice: true,
                    ..new(&sid, Source::Hook, Some(t))
                },
            )
            .unwrap()
            .0
        };
        let answered = notice("t1");
        let q = st
            .close_question(DAEMON, &answered.id, Close::Terminal { answers: a() })
            .unwrap();
        assert_eq!((q.status, q.answered_via.as_deref()), (Status::Answered, Some("terminal")));
        let kept = notice("t2");
        assert!(
            st.withdraw_hook_questions_on_start().unwrap().is_empty(),
            "a notice's hook is not waiting on the daemon"
        );
        assert!(st.open_hook_question_ids().unwrap().is_empty(), "no grace for a notice");
        let q = st.close_question(DAEMON, &kept.id, Close::Lapse).unwrap();
        assert_eq!(q.status, Status::Withdrawn);
        let plain = st.create_question(DAEMON, new(&sid, Source::Hook, Some("t3"))).unwrap().0;
        invalid(st.close_question(DAEMON, &plain.id, Close::Lapse), "not_a_notice");
        invalid(st.close_question(DAEMON, &kept.id, Close::Release), "not_answerable");
        // A notice is never created released.
        invalid(
            st.create_question(
                DAEMON,
                NewQuestion { notice: true, released: true, ..new(&sid, Source::Hook, Some("t4")) },
            ),
            "invalid_question",
        );
    }
```

In `crates/clax-store/src/linked.rs` tests, after `only_the_owners_agents_open_asks_on_a_two_way_link_are_paired_once` (line 1817), add:

```rust
    #[test]
    fn a_notice_about_a_linked_artifact_is_not_copied() {
        use crate::questions::{NewQuestion, Source};
        let w = world();
        link(&w, (1, 1));
        let qs: Vec<clax_core::questions::Question> = serde_json::from_value(serde_json::json!([
            {"question": "Which?", "header": "Pick", "options": [{"label": "A"}, {"label": "B"}]}
        ]))
        .unwrap();
        let sid = w
            .st
            .register_session(
                &AuditCtx::DAEMON,
                crate::RegisterSession {
                    harness: "claude".into(),
                    harness_session_id: Some("h1".into()),
                    cwd: "/w".into(),
                    ..Default::default()
                },
            )
            .unwrap()
            .id;
        w.st.create_question(
            &AuditCtx::DAEMON,
            NewQuestion {
                session_id: sid,
                artifact_id: Some(w.aid.clone()),
                source: Source::Ask,
                tool_use_id: None,
                questions: qs,
                released: false,
                thread_id: None,
                notice: true,
            },
        )
        .unwrap();
        assert_eq!(w.st.scan_linked_questions(&w.rid).unwrap(), 0, "a notice takes no answer there");
    }
```

- [ ] **Step 3: Run the tests to see them fail**

Run: `bazelisk test //crates/clax-store:clax_store_lib_test --test_arg=questions::tests:: --test_arg=migrations::tests::migration_37 --test_arg=linked::tests::a_notice`
Expected: compile errors (`no field thread_id on NewQuestion`, `no variant Pull`), then, after the fields exist, failures on `MIGRATIONS.len()` and the new codes.

- [ ] **Step 4: Write migration 37**

Append to `MIGRATIONS` in `crates/clax-store/src/migrations.rs`, after migration 36's string (the `r#"…"#` ending at line 1113):

```rust
    // 37: where an agent's person is paying attention (spec
    // 2026-10-16-ask-where-attention-is-design §1-§3). `questions.thread_id`
    // places a question in a thread of its artifact; `notice` marks a row
    // that says a question waits in the terminal or in chat and takes no
    // answer in Clax; `pulled_at` is when the person asked for a notice's
    // question in Clax instead ("Ask me here"), handed to the session once,
    // as a late answer is. On `sessions`: `terminal_at`, the person's last
    // prompt to the session in the terminal (`terminal_inferred` when the
    // harness reports no prompts and a turn with no Clax delivery stood in
    // for one); `turn_at`, the end of the last turn such a harness reported;
    // `delivered_*`, its last delivery from Clax and the artifact and thread
    // it was about. Every time is the daemon's.
    "ALTER TABLE questions ADD COLUMN thread_id TEXT;
    ALTER TABLE questions ADD COLUMN notice INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE questions ADD COLUMN pulled_at TEXT;
    ALTER TABLE sessions ADD COLUMN terminal_at TEXT;
    ALTER TABLE sessions ADD COLUMN terminal_inferred INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE sessions ADD COLUMN turn_at TEXT;
    ALTER TABLE sessions ADD COLUMN delivered_at TEXT;
    ALTER TABLE sessions ADD COLUMN delivered_artifact_id TEXT;
    ALTER TABLE sessions ADD COLUMN delivered_thread_id TEXT;",
```

- [ ] **Step 5: Extend the rows, the closes and the reasons**

In `crates/clax-store/src/questions.rs`:

Add to `QuestionRow` (after `taken_at`):

```rust
    /// The thread of `artifact_id` it was asked in, shown in that thread's card.
    pub thread_id: Option<String>,
    /// A notice: says a question waits in the terminal or in chat, and takes
    /// no answer in Clax.
    pub notice: bool,
    /// When the person asked for the notice's question in Clax instead.
    pub pulled_at: Option<String>,
```

Add to `NewQuestion` (after `questions`):

```rust
    /// A thread of `artifact_id`; it must be one (`invalid_thread`).
    pub thread_id: Option<String>,
    /// Record a notice ([`QuestionRow::notice`]); never created released.
    pub notice: bool,
```

Add to `Close` (after `Terminal`):

```rust
    /// "Ask me here" on a notice: it is withdrawn, and its session is told
    /// once, at its next delivery, that the person wants the question in
    /// Clax. Its inbox item is read.
    Pull,
    /// A hook notice whose terminal dialog took no answer: its person typed
    /// a new prompt to the session. Its inbox item is read.
    Lapse,
```

Add to `Reason` (after `DaemonStop`), and to `ALL`, `as_str`:

```rust
    /// Withdrawn: the person asked for the notice's question in Clax.
    Pulled,
    /// Withdrawn: the hook notice's dialog closed unanswered (a new prompt came).
    Lapsed,
```

```rust
    pub const ALL: [Reason; 10] = [
        Reason::Owner,
        Reason::Timer,
        Reason::Created,
        Reason::Explicit,
        Reason::Unwaited,
        Reason::SessionEnd,
        Reason::DaemonStart,
        Reason::DaemonStop,
        Reason::Pulled,
        Reason::Lapsed,
    ];
```

```rust
            Reason::Pulled => "pulled",
            Reason::Lapsed => "lapsed",
```

Replace `select!`, `late!` and the hook queries:

```rust
macro_rules! select {
    () => {
        "SELECT id, session_id, artifact_id, source, tool_use_id, questions_json, status,
            answers_json, answered_via, created_at, closed_at, taken_at, thread_id, notice,
            pulled_at FROM questions "
    };
}

/// The late answers' filter: session `?1`'s outcomes not yet received:
/// its answered and declined `ask` questions, and its pulled notices.
macro_rules! late {
    () => {
        "WHERE session_id = ?1 AND taken_at IS NULL AND ((status IN ('answered', 'declined')
            AND source = 'ask' AND notice = 0) OR pulled_at IS NOT NULL)"
    };
}
```

```rust
pub(crate) const OPEN_HOOKS: &str = "SELECT id, session_id, artifact_id FROM questions
    WHERE status = 'open' AND source = 'hook' AND notice = 0";
pub(crate) const WITHDRAW_HOOKS: &str = "UPDATE questions SET status = 'withdrawn', closed_at = ?1
    WHERE status = 'open' AND source = 'hook' AND notice = 0";
```

In `row`, add after `taken_at`:

```rust
        thread_id: r.get("thread_id")?,
        notice: r.get("notice")?,
        pulled_at: r.get("pulled_at")?,
```

Add the error helpers beside `not_mirrored`:

```rust
fn invalid_thread() -> CoreError {
    CoreError::invalid(
        "invalid_thread",
        "thread_id names no thread of the question's artifact",
    )
}

fn not_answerable() -> CoreError {
    CoreError::invalid(
        "not_answerable",
        "a notice is answered where its question was asked",
    )
}

fn not_a_notice() -> CoreError {
    CoreError::invalid(
        "not_a_notice",
        "only a notice is asked again in Clax or lapses",
    )
}
```

In `create_question_keyed`, before `let json = …`:

```rust
        if n.notice && n.released {
            return Err(CoreError::invalid(
                "invalid_question",
                "a notice is open until its question is answered or withdrawn",
            ));
        }
        if n.thread_id.is_some() && n.artifact_id.is_none() {
            return Err(invalid_thread());
        }
```

inside the transaction, after the `unknown_session` check:

```rust
            if let (Some(t), Some(a)) = (&n.thread_id, &n.artifact_id) {
                let on: Option<String> = tx
                    .query_row(
                        "SELECT artifact_id FROM threads WHERE id = ?1",
                        params![t],
                        |r| r.get(0),
                    )
                    .optional()?;
                if on.as_deref() != Some(a.as_str()) {
                    return Err(invalid_thread());
                }
            }
```

and make the insert carry the two columns:

```rust
            tx.execute(
                "INSERT INTO questions (id, session_id, artifact_id, source, tool_use_id,
                    questions_json, status, created_at, closed_at, person, thread_id, notice)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                         (SELECT person FROM sessions WHERE id = ?2), ?10, ?11)",
                params![
                    id,
                    n.session_id,
                    n.artifact_id,
                    n.source.as_str(),
                    n.tool_use_id,
                    json,
                    status.as_str(),
                    now,
                    closed_at,
                    n.thread_id,
                    n.notice
                ],
            )?;
```

In `close_question`, put these arms first in the `match (c, q.status)`:

```rust
                (Close::Answer { .. } | Close::Decline | Close::Release | Close::Expire, _)
                    if q.notice =>
                {
                    return Err(not_answerable());
                }
                (Close::Pull, _) if !q.notice => return Err(not_a_notice()),
                (Close::Lapse, _) if !(q.notice && q.source == Source::Hook) => {
                    return Err(not_a_notice());
                }
                (Close::Pull, Status::Open) => {
                    (Status::Withdrawn, None, None, Some(Reason::Pulled))
                }
                (Close::Lapse, Status::Open) => {
                    (Status::Withdrawn, None, None, Some(Reason::Lapsed))
                }
                (Close::Terminal { answers }, Status::Open)
                    if q.notice && q.source == Source::Hook =>
                {
                    (Status::Answered, Some(answers), Some("terminal"), None)
                }
```

and after the `CLOSE` update succeeds (`if n != 1 { … }`):

```rust
            if why == Some(Reason::Pulled) {
                tx.execute(
                    "UPDATE questions SET pulled_at = ?2 WHERE id = ?1",
                    params![qid, now],
                )?;
            }
```

In `crates/clax-store/src/inbox.rs` `note_question`, add `thread_id: q.thread_id.as_deref(),` to the `New { … }` literal, and in `question_changed` let a notice's item be read once the person acted on it (pulled) or its dialog lapsed:

```rust
    let read = seen
        && (q.notice
            || matches!(
                q.status,
                Status::Answered | Status::Declined | Status::Released
            ));
```

In `crates/clax-store/src/toolpath/tests.rs` (the actor table at 2704-2712), add the two reasons:

```rust
            (_, Some(Reason::Owner | Reason::Pulled)) => &["owner"],
            (_, Some(Reason::Timer | Reason::Created | Reason::Explicit | Reason::Lapsed)) => &["agent"],
```

replacing the two arms that name `Reason::Owner` and `Reason::Timer | Reason::Created | Reason::Explicit`.

In `crates/clax-store/src/linked.rs` `scan_linked_questions`, add `AND q.notice = 0` to the query's `WHERE` (after `q.source = 'ask'`, line 1187), and say in its doc comment that a notice is never copied: it takes no answer in Clax, there or here. A thread question is copied as it is today, about the remote artifact; its `thread_id` is this daemon's and is not sent.

Add `thread_id: None, notice: false,` to every other `NewQuestion {` literal (`grep -rn "NewQuestion {" crates`), the route's included; Task 6 fills the route's from the request.

- [ ] **Step 6: Run the tests to see them pass**

Run: `bazelisk test //crates/clax-store:clax_store_lib_test --test_arg=questions:: --test_arg=migrations:: --test_arg=linked:: --test_arg=toolpath::`
Expected: PASS, including `the_close_builder_refuses_a_reason_outside_its_kind` (`questions.rs:1736`) over the ten reasons.

Run: `bazelisk build //crates/clax-server:clax_server && just check-wasm`
Expected: builds; no warnings.

- [ ] **Step 7: Commit**

```bash
git add crates/clax-store/src/migrations.rs crates/clax-store/src/questions.rs crates/clax-store/src/inbox.rs crates/clax-store/src/linked.rs crates/clax-store/src/toolpath/tests.rs crates/clax-store/src/backfill.rs crates/clax-store/src/sessions.rs crates/clax-server/src/routes/questions.rs crates/clax-server/src/routes/remotes.rs
git -c commit.gpgsign=false commit -m "Store a question's thread and notices: migration 37, Ask me here as a pulled notice, a hook notice that lapses or takes the terminal's answer, and no notice copied to a linked artifact's remote"
```

---

## Task 2: Session engagement in the store: prompts, inferred turns, deliveries

**Files:**
- Create: `crates/clax-store/src/engagement.rs`
- Modify: `crates/clax-store/src/lib.rs:95-131` (add `pub mod engagement;` after `pub mod elevations;`, line 105)

**Interfaces:**
- Consumes: migration 37's `sessions` columns; `Store::close_question(ctx, qid, Close::Lapse)` (Task 1).
- Produces:
  - `pub struct SessionSignals { pub terminal_at: Option<String>, pub terminal_inferred: bool, pub delivered_at: Option<String>, pub delivered_artifact_id: Option<String>, pub delivered_thread_id: Option<String> }`
  - `Store::note_terminal(&self, ctx: &AuditCtx, sid: &str) -> Result<Vec<String>>` (lapsed notice IDs)
  - `Store::note_turn(&self, sid: &str) -> Result<bool>` (whether the turn stood for a prompt)
  - `Store::note_delivery(&self, sid: &str, aid: &str, tid: Option<&str>) -> Result<()>`
  - `Store::session_signals(&self, sid: &str) -> Result<Option<SessionSignals>>`
  - `Store::owner_public_id(&self) -> Result<Option<String>>`

- [ ] **Step 1: Write the failing tests**

Create `crates/clax-store/src/engagement.rs` with only its tests module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::questions::{NewQuestion, Source, Status};
    use crate::test_util::{DAEMON, session, store_with_clock};
    use clax_core::working::ManualClock;
    use std::sync::Arc;

    fn clocked() -> (Arc<ManualClock>, tempfile::TempDir, Store) {
        let c = Arc::new(ManualClock::at("2026-10-16T10:00:00Z"));
        let (d, st) = store_with_clock(c.clone());
        (c, d, st)
    }

    fn hook_notice(st: &Store, sid: &str, t: &str) -> String {
        let qs = serde_json::from_value(serde_json::json!([{"question": "Which?", "header": "Pick",
            "options": [{"label": "A"}, {"label": "B"}]}]))
        .unwrap();
        st.create_question(
            DAEMON,
            NewQuestion {
                session_id: sid.into(),
                artifact_id: None,
                source: Source::Hook,
                tool_use_id: Some(t.into()),
                questions: qs,
                thread_id: None,
                notice: true,
                released: false,
            },
        )
        .unwrap()
        .0
        .id
    }

    #[test]
    fn a_prompt_and_a_delivery_are_stamped_by_the_store_clock() {
        let (c, _d, st) = clocked();
        let sid = session(&st, "claude", "h1");
        assert_eq!(st.session_signals(&sid).unwrap().unwrap(), SessionSignals::default());
        assert!(st.note_terminal(DAEMON, &sid).unwrap().is_empty());
        c.advance(30);
        st.note_delivery(&sid, "7q3k9mzx2b4t", Some("t1")).unwrap();
        assert_eq!(
            st.session_signals(&sid).unwrap().unwrap(),
            SessionSignals {
                terminal_at: Some("2026-10-16T10:00:00.000Z".into()),
                terminal_inferred: false,
                delivered_at: Some("2026-10-16T10:00:30.000Z".into()),
                delivered_artifact_id: Some("7q3k9mzx2b4t".into()),
                delivered_thread_id: Some("t1".into()),
            }
        );
        assert_eq!(st.session_signals("nope").unwrap(), None);
    }

    #[test]
    fn a_prompt_lapses_the_sessions_open_hook_notices_only() {
        let (_c, _d, st) = clocked();
        let sid = session(&st, "claude", "h1");
        let other = session(&st, "claude", "h2");
        let mine = hook_notice(&st, &sid, "t1");
        let theirs = hook_notice(&st, &other, "t1");
        assert_eq!(st.note_terminal(DAEMON, &sid).unwrap(), vec![mine.clone()]);
        assert_eq!(st.question(&mine).unwrap().unwrap().status, Status::Withdrawn);
        assert_eq!(st.question(&theirs).unwrap().unwrap().status, Status::Open);
    }

    #[test]
    fn a_turn_with_no_delivery_since_the_last_stands_for_a_prompt() {
        let (c, _d, st) = clocked();
        let sid = session(&st, "codex", "h1");
        assert!(st.note_turn(&sid).unwrap(), "a first turn with no delivery");
        let s = st.session_signals(&sid).unwrap().unwrap();
        assert_eq!(
            (s.terminal_at.as_deref(), s.terminal_inferred),
            (Some("2026-10-16T10:00:00.000Z"), true)
        );
        c.advance(10);
        st.note_delivery(&sid, "7q3k9mzx2b4t", None).unwrap();
        c.advance(10);
        assert!(!st.note_turn(&sid).unwrap(), "that turn came from Clax");
        c.advance(10);
        assert!(st.note_turn(&sid).unwrap(), "nothing came from Clax since");
        assert_eq!(
            st.session_signals(&sid).unwrap().unwrap().terminal_at.as_deref(),
            Some("2026-10-16T10:00:30.000Z")
        );
        // A typed prompt clears the inference.
        st.note_terminal(DAEMON, &sid).unwrap();
        assert!(!st.session_signals(&sid).unwrap().unwrap().terminal_inferred);
    }

    #[test]
    fn an_ended_session_records_nothing() {
        let (_c, _d, st) = clocked();
        let sid = session(&st, "claude", "h1");
        st.end_session(DAEMON, &sid).unwrap();
        assert!(matches!(
            st.note_terminal(DAEMON, &sid),
            Err(CoreError::Invalid { code: "unknown_session", .. })
        ));
        assert!(st.note_turn(&sid).is_err());
        st.note_delivery(&sid, "7q3k9mzx2b4t", None).unwrap();
        assert_eq!(st.session_signals(&sid).unwrap().unwrap().delivered_at, None);
    }

    #[test]
    fn the_owners_public_id_is_read_and_never_made() {
        let (_c, _d, st) = clocked();
        assert_eq!(st.owner_public_id().unwrap(), None);
        let v = st.owner_viewer(false).unwrap();
        assert_eq!(st.owner_public_id().unwrap(), Some(v.public_id));
    }
}
```

Add `pub mod engagement;` to `crates/clax-store/src/lib.rs` after `pub mod elevations;`.

- [ ] **Step 2: Run the tests to see them fail**

Run: `bazelisk test //crates/clax-store:clax_store_lib_test --test_arg=engagement::`
Expected: compile errors: `cannot find type SessionSignals`, `no method note_terminal`.

- [ ] **Step 3: Write the implementation**

Put above the tests module in `crates/clax-store/src/engagement.rs`:

```rust
//! Where an agent session's person last engaged (spec
//! 2026-10-16-ask-where-attention-is-design §1): their last prompt to it in
//! the terminal, typed or inferred from a turn, and its last delivery from
//! Clax. Every time is the store's clock's, so neither a hook's machine's
//! clock nor a browser's ever enters a comparison.

use super::Store;
use super::questions::Close;
use clax_core::audit::AuditCtx;
use clax_core::{CoreError, Result};
use rusqlite::{OptionalExtension, params};

/// What the store knows of where session `id`'s person last engaged.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionSignals {
    /// The person's last prompt to the session in the terminal.
    pub terminal_at: Option<String>,
    /// `terminal_at` stands for a turn with no Clax delivery behind it,
    /// not a prompt the harness reported.
    pub terminal_inferred: bool,
    /// The session's last delivery from Clax: a thread, an answer or an
    /// "Ask me here" sent to it.
    pub delivered_at: Option<String>,
    pub delivered_artifact_id: Option<String>,
    pub delivered_thread_id: Option<String>,
}

fn unknown_session() -> CoreError {
    CoreError::invalid("unknown_session", "no live session has this ID")
}

impl Store {
    /// Records that the person typed a prompt to live session `sid` in the
    /// terminal now, and withdraws its open hook notices under `ctx`
    /// (`question.withdraw`, reason `lapsed`): a terminal dialog that took
    /// no answer (Esc) is gone once the person types again. Returns the IDs
    /// withdrawn, for the caller to announce.
    ///
    /// # Errors
    /// `unknown_session` for a missing or ended session.
    pub fn note_terminal(&self, ctx: &AuditCtx, sid: &str) -> Result<Vec<String>> {
        let now = self.now();
        let n = self.with_tx(|tx| {
            Ok(tx.execute(
                "UPDATE sessions SET terminal_at = ?2, terminal_inferred = 0
                 WHERE id = ?1 AND ended_at IS NULL",
                params![sid, now],
            )?)
        })?;
        if n == 0 {
            return Err(unknown_session());
        }
        let open: Vec<String> = self.with_read(|c| {
            let mut s = c.prepare_cached(
                "SELECT id FROM questions WHERE session_id = ?1 AND status = 'open'
                 AND notice = 1 AND source = 'hook' ORDER BY id",
            )?;
            Ok(s.query_map(params![sid], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        })?;
        let mut lapsed = Vec::with_capacity(open.len());
        for id in open {
            match self.close_question(ctx, &id, Close::Lapse) {
                Ok(_) => lapsed.push(id),
                // Answered or withdrawn in the meantime.
                Err(CoreError::Invalid {
                    code: "question_closed",
                    ..
                }) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(lapsed)
    }

    /// A turn of live session `sid` ended now, in a harness that reports no
    /// prompts (Codex, Grok Build). When no Clax delivery reached the
    /// session since its previous turn ended (for its first turn: at all),
    /// the turn stands for a terminal prompt, recorded as inferred. Returns
    /// whether it did.
    ///
    /// # Errors
    /// `unknown_session` for a missing or ended session.
    pub fn note_turn(&self, sid: &str) -> Result<bool> {
        let now = self.now();
        self.with_tx(|tx| {
            let row: Option<(Option<String>, Option<String>)> = tx
                .query_row(
                    "SELECT turn_at, delivered_at FROM sessions WHERE id = ?1 AND ended_at IS NULL",
                    params![sid],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let Some((turn, delivered)) = row else {
                return Err(unknown_session());
            };
            // Times are the store's fixed RFC 3339 form: they order as text.
            let from_terminal = match (&delivered, &turn) {
                (None, _) => true,
                (Some(d), Some(t)) => d <= t,
                (Some(_), None) => false,
            };
            if from_terminal {
                tx.execute(
                    "UPDATE sessions SET terminal_at = ?2, terminal_inferred = 1, turn_at = ?2
                     WHERE id = ?1",
                    params![sid, now],
                )?;
            } else {
                tx.execute(
                    "UPDATE sessions SET turn_at = ?2 WHERE id = ?1",
                    params![sid, now],
                )?;
            }
            Ok(from_terminal)
        })
    }

    /// Records a delivery from Clax to session `sid` now, about artifact
    /// `aid` and, when given, its thread `tid`. An ended session records
    /// nothing.
    pub fn note_delivery(&self, sid: &str, aid: &str, tid: Option<&str>) -> Result<()> {
        let now = self.now();
        self.with_tx(|tx| {
            tx.execute(
                "UPDATE sessions SET delivered_at = ?2, delivered_artifact_id = ?3,
                    delivered_thread_id = ?4 WHERE id = ?1 AND ended_at IS NULL",
                params![sid, now, aid, tid],
            )?;
            Ok(())
        })
    }

    /// Session `sid`'s signals; `None` when it does not exist.
    pub fn session_signals(&self, sid: &str) -> Result<Option<SessionSignals>> {
        self.with_read(|c| {
            Ok(c.query_row(
                "SELECT terminal_at, terminal_inferred, delivered_at, delivered_artifact_id,
                    delivered_thread_id FROM sessions WHERE id = ?1",
                params![sid],
                |r| {
                    Ok(SessionSignals {
                        terminal_at: r.get(0)?,
                        terminal_inferred: r.get(1)?,
                        delivered_at: r.get(2)?,
                        delivered_artifact_id: r.get(3)?,
                        delivered_thread_id: r.get(4)?,
                    })
                },
            )
            .optional()?)
        })
    }

    /// The owner's public ID when the owner exists; never makes the owner
    /// (unlike [`Store::owner_viewer`]).
    pub fn owner_public_id(&self) -> Result<Option<String>> {
        self.with_read(|c| {
            Ok(c.query_row("SELECT public_id FROM viewers WHERE owner = 1", [], |r| {
                r.get(0)
            })
            .optional()?)
        })
    }
}
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `bazelisk test //crates/clax-store:clax_store_lib_test --test_arg=engagement::`
Expected: PASS (5 tests). Then `bazelisk test //crates/clax-store:clax_store_lib_test --test_env=CLAX_TEST_STORE=single --test_arg=engagement::` (the one-connection store mode, `test_util::STORE_MODE_VAR`) — PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/clax-store/src/engagement.rs crates/clax-store/src/lib.rs
git -c commit.gpgsign=false commit -m "Record each session's last terminal prompt, inferred turn and Clax delivery, by the store's clock; a typed prompt lapses the session's open hook notices"
```

---

## Task 3: Presence keeps each tab's focus and last input, for the person's own agents

**Files:**
- Modify: `crates/clax-core/src/presence.rs:44-73` (`KeptPresence`, `Tab`), `:166-246` (`report_tab` → `report_engaged`), `:378-403` (after `for_artifact`, add `engaged_at`), `:332-376` (`kept`, `restore`), tests (from line 405; `reg()` at 410)
- Modify: `crates/clax-server/src/routes/viewers.rs:398-410` (`PresenceBody`), `:412-498` (`set_presence`; `report_tab` at 476)
- Test: `crates/clax-server/tests/api_presence.rs` (a module of `tests/integration.rs`, already gated)

**Interfaces:**
- Consumes: the presence registry's `Clock`.
- Produces:
  - `Presence::report_engaged(&self, aid: &str, public_id: &str, display_name: Option<&str>, state: State, where_: Option<&str>, tab: &str, focused: bool, idle_ms: Option<u64>) -> Option<bool>`
  - `Presence::engaged_at(&self, aid: &str, public_id: &str) -> Option<DateTime<Utc>>`
  - `KeptPresence { …, pub focused: bool /* "f" */, pub input_ms: Option<i64> /* "i" */ }`
  - `PUT /api/viewers/me/presence` body gains `focused: bool` (default false) and `idle_ms: u64?`

- [ ] **Step 1: Write the failing tests**

In `crates/clax-core/src/presence.rs` tests, add:

```rust
    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn focus_and_input_are_kept_for_the_suggestion_and_never_shown() {
        let (c, p) = reg();
        p.report_engaged("a", "v1", Some("Ann"), State::Here, None, "t1", true, Some(2_000));
        assert_eq!(p.engaged_at("a", "v1"), Some(utc("2026-09-30T10:00:00Z")), "focused at its report");
        c.advance(10);
        p.report_engaged("a", "v1", Some("Ann"), State::Here, None, "t1", false, Some(4_000));
        assert_eq!(p.engaged_at("a", "v1"), Some(utc("2026-09-30T10:00:06Z")), "unfocused: its last input");
        let shown = serde_json::to_string(&p.for_artifact("a")).unwrap();
        assert!(!shown.contains("focus") && !shown.contains("input"), "{shown}");
        assert_eq!(p.engaged_at("b", "v1"), None);
        p.report_tab("c", "v1", None, State::Here, None, "t2");
        assert_eq!(p.engaged_at("c", "v1"), None, "a tab with neither is no engagement");
        c.advance(PRESENCE_TTL_SECS + 1);
        assert_eq!(p.engaged_at("a", "v1"), None, "a lapsed tab is no engagement");
    }

    #[test]
    fn kept_reports_keep_focus_and_input_across_a_wake() {
        let (c, p) = reg();
        p.report_engaged("a", "v1", None, State::Here, None, "t1", true, Some(1_000));
        let kept = p.kept();
        assert!(kept[0].focused && kept[0].input_ms.is_some());
        let back = Presence::new(c.clone());
        back.restore(&serde_json::from_str::<Vec<KeptPresence>>(&serde_json::to_string(&kept).unwrap()).unwrap());
        assert_eq!(back.engaged_at("a", "v1"), p.engaged_at("a", "v1"));
        // A report kept before focus and input were (no `f`, no `i`) restores unfocused.
        let old: Vec<KeptPresence> = serde_json::from_str(r#"[{"a":"a","p":"v2","s":"here","t":1759226400000}]"#).unwrap();
        back.restore(&old);
        assert_eq!(back.engaged_at("a", "v2"), None);
    }
```

In `crates/clax-server/tests/api_presence.rs`, add:

```rust
#[tokio::test]
async fn input_time_is_the_daemons_report_time_less_idle_ms() {
    let c = Arc::new(ManualClock::at("2026-10-16T10:00:00Z"));
    let ts = TestServer::spawn_with_clock(c.clone()).await;
    let a = ts.publish("T", &[("index.html", "<p>")]).await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let owner = ts.owner_public_id().await;
    let mut ev = ts.events(&format!("?artifact={aid}&types=presence")).await;
    let r = ts
        .client
        .put(format!("{}/api/viewers/me/presence", ts.base))
        .header("cookie", ts.owner_cookie())
        .json(&json!({"artifact_id": aid, "state": "here", "tab": "t1", "focused": false, "idle_ms": 7000}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    let e = ev.next_named("presence").await;
    assert!(!e.to_string().contains("focus") && !e.to_string().contains("idle"), "{e}");
    assert_eq!(
        ts.presence.engaged_at(&aid, &owner).map(|t| t.to_rfc3339()),
        Some("2026-10-16T09:59:53+00:00".to_string())
    );
}
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `bazelisk test //crates/clax-core:clax_core_lib_test --test_arg=presence::tests::` then `bazelisk test //crates/clax-server:all --test_arg=api_presence::`
Expected: compile errors (`no method report_engaged`, `no field focused`); then a 400 `invalid_json` for the unknown `focused` field.

- [ ] **Step 3: Write the implementation**

In `crates/clax-core/src/presence.rs`, add to `KeptPresence` after `at_ms`:

```rust
    /// The tab had focus at its report.
    #[serde(rename = "f", default, skip_serializing_if = "std::ops::Not::not")]
    pub focused: bool,
    /// The person's last input on the tab, in ms.
    #[serde(rename = "i", default, skip_serializing_if = "Option::is_none")]
    pub input_ms: Option<i64>,
```

Add to `Tab`:

```rust
    /// Focused at its last report. Never shown: see [`Presence::engaged_at`].
    focused: bool,
    /// The person's last input on it, by this registry's clock.
    input_at: Option<DateTime<Utc>>,
```

Rename the body of `report_tab` into `report_engaged`, keep `report_tab` delegating:

```rust
    pub fn report_tab(
        &self,
        aid: &str,
        public_id: &str,
        display_name: Option<&str>,
        state: State,
        where_: Option<&str>,
        tab: &str,
    ) -> Option<bool> {
        self.report_engaged(aid, public_id, display_name, state, where_, tab, false, None)
    }

    /// [`Presence::report_tab`], with whether the tab has focus and how long
    /// before this report the person last pressed, typed, pointed or
    /// scrolled in it (`idle_ms`, read against this registry's clock, so no
    /// browser's clock enters it). Neither is part of what anyone sees
    /// ([`PresenceView`]): they serve the person's own agents' suggestion of
    /// where to ask ([`Presence::engaged_at`]).
    #[allow(clippy::too_many_arguments)]
    pub fn report_engaged(
        &self,
        aid: &str,
        public_id: &str,
        display_name: Option<&str>,
        state: State,
        where_: Option<&str>,
        tab: &str,
        focused: bool,
        idle_ms: Option<u64>,
    ) -> Option<bool> {
```

(the rest of the former body unchanged), with the inserted tab:

```rust
        let input_at = idle_ms.map(|ms| now - Duration::milliseconds(i64::try_from(ms).unwrap_or(i64::MAX)));
        entry.tabs.insert(
            tab.chars().take(64).collect(),
            Tab {
                state,
                where_,
                last_report: now,
                focused,
                input_at,
            },
        );
```

Add after `for_artifact`:

```rust
    /// When `public_id` last engaged with `aid`, on a tab whose report has
    /// not lapsed: the newest of its tabs' last input and, for a tab focused
    /// at its last report, that report. `None` with no such tab. A hidden
    /// or unfocused tab without input is no engagement.
    pub fn engaged_at(&self, aid: &str, public_id: &str) -> Option<DateTime<Utc>> {
        let now = self.clock.now();
        let map = self.entries.lock().unwrap();
        let e = map.get(&(aid.to_string(), public_id.to_string()))?;
        e.tabs
            .values()
            .filter(|t| !lapsed(t, now))
            .flat_map(|t| [t.input_at, t.focused.then_some(t.last_report)])
            .flatten()
            .max()
    }
```

In `kept`, add to the pushed `KeptPresence`: `focused: t.focused, input_ms: t.input_at.map(|i| i.timestamp_millis()),`. In `restore`, the `Tab { … }` gains `focused: k.focused, input_at: k.input_ms.and_then(DateTime::from_timestamp_millis),`.

In `crates/clax-server/src/routes/viewers.rs`, add to `PresenceBody`:

```rust
    /// The tab has focus (kept for the viewer's own agents, never shown).
    #[serde(default)]
    focused: bool,
    /// How long before this report the viewer last gave input in the tab.
    #[serde(default)]
    idle_ms: Option<u64>,
```

and in `set_presence` call `report_engaged(…, b.tab.as_deref().unwrap_or(""), b.focused, b.idle_ms)` in place of `report_tab(…)`. A spectator's report (a person beyond the artifact's live seats, the early return before `report_tab`) records neither, as it records no presence today, so a spectator's tab is no engagement until it holds a seat. Extend the handler's doc comment with: "`focused` and `idle_ms` are kept for the viewer's own agents' suggestion of where to ask (spec 2026-10-16-ask-where-attention-is-design §1) and never shown to anyone."

- [ ] **Step 4: Run the tests to see them pass**

Run: `bazelisk test //crates/clax-core:clax_core_lib_test --test_arg=presence:: && bazelisk test //crates/clax-server:all --test_arg=api_presence:: --test_arg=api_viewers::`
Expected: PASS.

Run: `just worker-test && just check-wasm`
Expected: PASS; the Worker's kept presence restores.

- [ ] **Step 5: Commit**

```bash
git add crates/clax-core/src/presence.rs crates/clax-server/src/routes/viewers.rs crates/clax-server/tests/api_presence.rs
git -c commit.gpgsign=false commit -m "Keep each presence tab's focus and last input, by the daemon's clock, for the viewer's own agents; never shown to other viewers"
```

---

## Task 4: The suggestion: `clax_core::surface::suggest`

**Files:**
- Create: `crates/clax-core/src/surface.rs`
- Modify: `crates/clax-core/src/lib.rs:7-48` (add `pub mod surface;` after `pub mod sig;`, line 43)

**Interfaces:**
- Consumes: nothing (pure).
- Produces:
  - `pub const TIE_SECS: i64 = 60; pub const QUIET_SECS: i64 = 300;`
  - `enum Surface { Clax, Terminal }`, `enum Confidence { Clear, Unsure }` (serialized snake_case)
  - `struct Place { thread_id: Option<String>, artifact_id: Option<String> }`
  - `struct Suggestion { surface: Surface, r#where: Place, confidence: Confidence, why: String }` (serialized as `{surface, where, confidence, why}`)
  - `struct TerminalSignal { at: DateTime<Utc>, inferred: bool }`, `struct Delivery { at, artifact_id: String, thread_id: Option<String> }`
  - `struct Signals { terminal: Option<TerminalSignal>, delivery: Option<Delivery>, clax_at: Option<DateTime<Utc>>, artifact_id: Option<String>, clax_open: bool, panel_open: bool }`
  - `pub fn suggest(s: &Signals, now: DateTime<Utc>) -> Suggestion`
  - `pub fn line(s: &Suggestion) -> String`

- [ ] **Step 1: Write the failing tests**

Create `crates/clax-core/src/surface.rs` with its tests module first (and `pub mod surface;` in `lib.rs`):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const NOW: &str = "2026-10-16T10:10:00Z";

    fn at(secs_ago: i64) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(NOW).unwrap().with_timezone(&Utc) - Duration::seconds(secs_ago)
    }
    fn now() -> DateTime<Utc> {
        at(0)
    }
    fn open() -> Signals {
        Signals { clax_open: true, artifact_id: Some("7q3k9mzx2b4t".into()), ..Signals::default() }
    }
    fn typed(secs_ago: i64) -> Option<TerminalSignal> {
        Some(TerminalSignal { at: at(secs_ago), inferred: false })
    }
    fn sent(secs_ago: i64, thread: Option<&str>) -> Option<Delivery> {
        Some(Delivery { at: at(secs_ago), artifact_id: "7q3k9mzx2b4t".into(), thread_id: thread.map(Into::into) })
    }
    fn says(s: &Signals) -> (Surface, Confidence) {
        let x = suggest(s, now());
        (x.surface, x.confidence)
    }

    #[test]
    fn with_no_clax_open_the_terminal_is_clear_whatever_else() {
        let s = Signals { clax_open: false, clax_at: Some(at(1)), ..open() };
        let x = suggest(&s, now());
        assert_eq!((x.surface, x.confidence), (Surface::Terminal, Confidence::Clear));
        assert!(x.why.contains("No Clax tab or panel"), "{}", x.why);
    }

    #[test]
    fn a_prompt_newer_than_any_clax_input_is_the_terminal() {
        assert_eq!(says(&Signals { terminal: typed(10), ..open() }), (Surface::Terminal, Confidence::Clear));
        assert_eq!(
            says(&Signals { terminal: typed(10), clax_at: Some(at(200)), ..open() }),
            (Surface::Terminal, Confidence::Clear)
        );
    }

    #[test]
    fn clax_input_or_focus_newer_than_the_prompt_is_clax() {
        let x = suggest(&Signals { terminal: typed(200), clax_at: Some(at(10)), ..open() }, now());
        assert_eq!((x.surface, x.confidence), (Surface::Clax, Confidence::Clear));
        assert_eq!(x.r#where, Place { thread_id: None, artifact_id: Some("7q3k9mzx2b4t".into()) });
    }

    #[test]
    fn a_delivery_from_clax_is_clax_at_its_thread() {
        let x = suggest(&Signals { terminal: typed(200), delivery: sent(10, Some("t1")), ..open() }, now());
        assert_eq!((x.surface, x.confidence), (Surface::Clax, Confidence::Clear));
        assert_eq!(x.r#where.thread_id.as_deref(), Some("t1"));
    }

    #[test]
    fn work_instigated_in_the_terminal_is_placed_on_the_artifact_not_an_older_thread() {
        let x = suggest(&Signals { terminal: typed(5), delivery: sent(100, Some("t1")), ..open() }, now());
        assert_eq!(x.surface, Surface::Terminal);
        assert_eq!(x.r#where, Place { thread_id: None, artifact_id: Some("7q3k9mzx2b4t".into()) });
    }

    #[test]
    fn signals_within_a_minute_of_each_other_are_unsure_and_lean_to_clax() {
        for (t, c) in [(10, 69), (69, 10), (30, 30)] {
            assert_eq!(
                says(&Signals { terminal: typed(t), clax_at: Some(at(c)), ..open() }),
                (Surface::Clax, Confidence::Unsure),
                "terminal {t} s ago, Clax {c} s ago"
            );
        }
        assert_eq!(
            says(&Signals { terminal: typed(10), clax_at: Some(at(70)), ..open() }),
            (Surface::Terminal, Confidence::Clear),
            "a minute apart is not a tie"
        );
    }

    #[test]
    fn nothing_in_five_minutes_is_unsure_and_leans_to_clax() {
        let x = suggest(&Signals { terminal: typed(QUIET_SECS + 1), clax_at: Some(at(QUIET_SECS + 400)), ..open() }, now());
        assert_eq!((x.surface, x.confidence), (Surface::Clax, Confidence::Unsure));
        assert!(x.why.contains("5 minutes"), "{}", x.why);
        assert_eq!(says(&open()), (Surface::Clax, Confidence::Unsure), "no signal at all");
        assert_eq!(
            says(&Signals { terminal: typed(QUIET_SECS), ..open() }),
            (Surface::Terminal, Confidence::Clear),
            "exactly five minutes is still recent"
        );
    }

    #[test]
    fn an_inferred_turn_alone_is_unsure() {
        let s = Signals { terminal: Some(TerminalSignal { at: at(5), inferred: true }), ..open() };
        let x = suggest(&s, now());
        assert_eq!((x.surface, x.confidence), (Surface::Clax, Confidence::Unsure));
        assert!(x.why.contains("inferred"), "{}", x.why);
        let older_clax = Signals { clax_at: Some(at(200)), ..s.clone() };
        assert_eq!(says(&older_clax), (Surface::Clax, Confidence::Unsure));
        let newer_clax = Signals { clax_at: Some(at(1)), terminal: Some(TerminalSignal { at: at(100), inferred: true }), ..open() };
        assert_eq!(says(&newer_clax), (Surface::Clax, Confidence::Clear));
    }

    #[test]
    fn the_panel_is_named_but_never_decides() {
        let s = Signals { panel_open: true, ..open() };
        let x = suggest(&s, now());
        assert_eq!(x.confidence, Confidence::Unsure);
        assert!(x.why.contains("panel"), "{}", x.why);
        assert_eq!(says(&Signals { terminal: typed(5), ..s }), (Surface::Terminal, Confidence::Clear));
    }

    #[test]
    fn a_session_with_no_page_names_no_place() {
        let x = suggest(&Signals { artifact_id: None, ..open() }, now());
        assert_eq!(x.r#where, Place::default());
        assert_eq!(serde_json::to_value(&x).unwrap()["where"], serde_json::json!({}));
    }

    #[test]
    fn it_serializes_as_the_contract_names_it_and_reads_as_one_line() {
        let x = suggest(&Signals { terminal: typed(200), delivery: sent(10, Some("t1")), ..open() }, now());
        let v = serde_json::to_value(&x).unwrap();
        assert_eq!(v["surface"], "clax");
        assert_eq!(v["confidence"], "clear");
        assert_eq!(v["where"], serde_json::json!({"thread_id": "t1", "artifact_id": "7q3k9mzx2b4t"}));
        assert!(line(&x).starts_with("[clax] If you ask the person anything now, ask in Clax, in thread t1 (clear): "), "{}", line(&x));
    }
}
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `bazelisk test //crates/clax-core:clax_core_lib_test --test_arg=surface::`
Expected: compile errors: `cannot find type Signals`.

- [ ] **Step 3: Write the implementation**

Put above the tests module:

```rust
//! Where an agent should ask its person (spec
//! 2026-10-16-ask-where-attention-is-design §1): a suggestion, from what the
//! daemon knows of where that person last engaged, of the surface (the
//! terminal or Clax), the narrowest place in Clax, and how sure it is.
//! Pure: the caller gathers the signals by the daemon's clock and passes
//! the time. The windows are defaults to tune with use (§ "Settled" 1).

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;

/// Newest signals on the two sides closer than this many seconds are a
/// tie: unsure.
pub const TIE_SECS: i64 = 60;
/// With no signal on either side within this many seconds, the suggestion
/// is unsure.
pub const QUIET_SECS: i64 = 300;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Surface {
    Clax,
    Terminal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    Clear,
    Unsure,
}

/// The narrowest place in Clax: a thread of an artifact, an artifact, or
/// neither (the inbox).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Place {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_id: Option<String>,
}

/// Where to ask. Whatever the surface, `where` is the place in Clax a
/// question, or the notice of one asked in the terminal, goes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Suggestion {
    pub surface: Surface,
    pub r#where: Place,
    pub confidence: Confidence,
    /// Why, in a sentence the agent may weigh against what it knows.
    pub why: String,
}

/// The person's last prompt to the session in the terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalSignal {
    pub at: DateTime<Utc>,
    /// A turn with no Clax delivery behind it stood in for a prompt the
    /// harness does not report.
    pub inferred: bool,
}

/// The session's last delivery from Clax.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Delivery {
    pub at: DateTime<Utc>,
    pub artifact_id: String,
    /// Its thread, when it still is one of `artifact_id`'s.
    pub thread_id: Option<String>,
}

/// What the daemon knows of a session's person, now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Signals {
    pub terminal: Option<TerminalSignal>,
    pub delivery: Option<Delivery>,
    /// The person's newest input, or focused report, on a tab of the
    /// session's pages. A hidden or unfocused tab gives none.
    pub clax_at: Option<DateTime<Utc>>,
    /// The session's page, which places a question when no delivery does.
    pub artifact_id: Option<String>,
    /// A Clax tab or the extension's panel of the person's can show a
    /// question now.
    pub clax_open: bool,
    /// The extension's side panel is open (its focus is not known).
    pub panel_open: bool,
}

/// The place: the delivery's thread (else artifact) when Clax instigated
/// the current work, else the session's page.
fn place_of(s: &Signals) -> Place {
    match &s.delivery {
        Some(d) if s.terminal.as_ref().is_none_or(|t| d.at >= t.at) => Place {
            thread_id: d.thread_id.clone(),
            artifact_id: Some(d.artifact_id.clone()),
        },
        _ => Place {
            thread_id: None,
            artifact_id: s.artifact_id.clone(),
        },
    }
}

/// The suggestion for `s` at `now`. The newest engagement wins; signals a
/// minute apart, no signal in five minutes, or an inferred turn deciding
/// are unsure, and unsure leans to Clax. With no Clax tab or panel open,
/// Clax cannot show a question: the terminal, clear.
pub fn suggest(s: &Signals, now: DateTime<Utc>) -> Suggestion {
    let place = place_of(s);
    let make = |surface, confidence, why: String| Suggestion {
        surface,
        r#where: place.clone(),
        confidence,
        why,
    };
    if !s.clax_open {
        return make(
            Surface::Terminal,
            Confidence::Clear,
            "No Clax tab or panel of yours is open, so Clax cannot show the question.".into(),
        );
    }
    let panel = if s.panel_open {
        " The extension's panel is open."
    } else {
        ""
    };
    let clax = s.clax_at.into_iter().chain(s.delivery.as_ref().map(|d| d.at)).max();
    let term = s.terminal.as_ref();
    let recent = |t: DateTime<Utc>| now - t <= Duration::seconds(QUIET_SECS);
    let ago = |t: DateTime<Utc>| (now - t).num_seconds().max(0);
    let tie = Duration::seconds(TIE_SECS);
    let quiet = || {
        make(
            Surface::Clax,
            Confidence::Unsure,
            format!(
                "Neither the terminal nor Clax has seen you in the last {} minutes; Clax is the default.{panel}",
                QUIET_SECS / 60
            ),
        )
    };
    if !term.is_some_and(|t| recent(t.at)) && !clax.is_some_and(recent) {
        return quiet();
    }
    match (term, clax) {
        (Some(t), c) if c.is_none_or(|c| t.at > c) => {
            if t.inferred {
                make(
                    Surface::Clax,
                    Confidence::Unsure,
                    format!(
                        "Your last terminal turn, {} s ago, is inferred: this harness reports no prompts.{panel}",
                        ago(t.at)
                    ),
                )
            } else if let Some(c) = c.filter(|c| t.at - *c < tie) {
                make(
                    Surface::Clax,
                    Confidence::Unsure,
                    format!(
                        "You typed in the terminal {} s ago and were active in Clax {} s ago, too close to tell.{panel}",
                        ago(t.at),
                        ago(c)
                    ),
                )
            } else {
                make(
                    Surface::Terminal,
                    Confidence::Clear,
                    format!(
                        "You typed to this session in the terminal {} s ago, after your last activity in Clax.",
                        ago(t.at)
                    ),
                )
            }
        }
        (t, Some(c)) => match t.filter(|t| c - t.at < tie) {
            Some(t) => make(
                Surface::Clax,
                Confidence::Unsure,
                format!(
                    "You were active in Clax {} s ago and typed in the terminal {} s ago, too close to tell.{panel}",
                    ago(c),
                    ago(t.at)
                ),
            ),
            None => make(
                Surface::Clax,
                Confidence::Clear,
                format!(
                    "You were active on this session's page in Clax {} s ago, after your last prompt in the terminal.",
                    ago(c)
                ),
            ),
        },
        (None, None) => quiet(),
    }
}

/// `s` as one line of a delivery's text.
pub fn line(s: &Suggestion) -> String {
    let surface = match (s.surface, &s.r#where.thread_id) {
        (Surface::Terminal, _) => "in the terminal (chat)".to_string(),
        (Surface::Clax, Some(t)) => format!("in Clax, in thread {t}"),
        (Surface::Clax, None) => "in Clax".to_string(),
    };
    let sure = match s.confidence {
        Confidence::Clear => "clear",
        Confidence::Unsure => "unsure",
    };
    format!(
        "[clax] If you ask the person anything now, ask {surface} ({sure}): {}",
        s.why
    )
}
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `bazelisk test //crates/clax-core:clax_core_lib_test --test_arg=surface:: && just check-wasm`
Expected: PASS (11 tests); wasm check clean.

- [ ] **Step 5: Commit**

```bash
git add crates/clax-core/src/surface.rs crates/clax-core/src/lib.rs
git -c commit.gpgsign=false commit -m "Add the suggestion of where to ask: the newest engagement wins, ties, quiet and inferred turns are unsure and lean to Clax, and no open Clax means the terminal"
```

---
## Task 5: Server: the session's signals, the panel, `GET …/attention`, `POST …/terminal`, deliveries noted

**Files:**
- Create: `crates/clax-http/src/surface.rs` (`clax_http::surface`)
- Modify: `crates/clax-http/src/lib.rs:11-38` (add `pub mod surface;` after `pub mod state;`, line 35; name `surface` in the crate root's `//!` list of what routes share)
- Create: `crates/clax-server/src/routes/surface.rs`
- Modify: `crates/clax-server/src/routes/mod.rs:34` (`pub mod surface;` after `pub mod stream;`)
- Modify: `crates/clax-realtime/src/stream.rs:1414-1430` (`holds_surface` doc; add `holds_panel` after it), tests (after `each_persons_topics_carry_their_own_events_and_end_with_the_person`, lines 2651-2711)
- Modify: `crates/clax-realtime/src/working.rs:63-102` (`mark_items` notes the delivery)
- Modify: `crates/clax-server/src/router.rs:6-10` (import `surface`), `:194-197` (routes after `/api/sessions/{id}/tool-call-ids`)
- Modify: `crates/clax-server/src/routes/guards.rs:66-188` (the class table `GUARDS`: beside `("feedback::notices", AgentPath)`, line 119), `:210-248` (`module_source_opt`: `"surface" => include_str!("surface.rs")` after `"stream"`, line 236), `:849-1200` (`EXPECTED`: beside `/api/sessions/{id}/questions:terminal`, lines 1169-1174)
- Modify: `crates/clax-http/src/agent.rs:236-258` (needs, after `need!(QuestionsTerminal, …)` at line 253), `:267-450` (`ROUTES`, after `/api/sessions/{id}/tool-call-ids`), `:458-529` (`ROUTE_NEEDS`, after `sessions::tool_call_ids`)
- Create: `crates/clax-testkit/src/where_to_ask.rs`; Modify: `crates/clax-testkit/src/lib.rs` (`pub mod where_to_ask;` beside `pub mod remotes;`)
- Create: `crates/clax-server/tests/api_where_to_ask.rs`; Modify: `crates/clax-server/tests/integration.rs` (add the gated `mod api_where_to_ask;` after `mod api_watches;`)

**Interfaces:**
- Consumes: `Store::session_signals`, `Store::owner_public_id`, `Store::note_terminal`, `Store::note_delivery` (Task 2); `Presence::engaged_at` (Task 3); `clax_core::surface::{suggest, Signals}` (Task 4); `Working::newest_artifact_of` (`crates/clax-core/src/working.rs:660`); `Store::session_artifact` (`crates/clax-store/src/watches.rs:390`); `Agent::named_artifact` (`crates/clax-http/src/agent.rs:736`); `clax_http::views::question::announce_ids` (`crates/clax-http/src/views/question.rs:79`); `clax_store::inbox::recipient_key` (`crates/clax-store/src/inbox.rs:120`).
- Produces:
  - `Hub::holds_panel(&self, person: &str) -> bool`
  - `clax_http::surface::page<N: Need>(s: &AppState, a: &Agent<N>, sid: &str) -> Result<Option<String>, ApiError>` (async: the session's page through the agent's gate)
  - `clax_http::surface::signals(s: &AppState, st: &Store, sid: &str, page: Option<&str>) -> clax_core::Result<Signals>`
  - `clax_http::surface::suggestion(s: &AppState, st: &Store, sid: &str, page: Option<&str>) -> clax_core::Result<Suggestion>`
  - `clax_testkit::where_to_ask::{clocked, attention, typed, owner_tab, paged}` (shared with Tasks 6 and 7)
  - `GET /api/sessions/{id}/attention` → `{attention: Suggestion, signals: {terminal_at, terminal_inferred, delivered_at}}` (need `SessionAttention`: `Ask`/`ask`, `Own`)
  - `POST /api/sessions/{id}/terminal` → `{terminal_at, lapsed: [qid]}` (need `SessionTerminal`: no right, `Own`)

- [ ] **Step 1: Read the open threads**

Run `"/Users/alex/.claude/plugins/cache/qualifier/qual/0.1.0/scripts/ensure-qualifier.sh" exec threads crates/clax-realtime/src/stream.rs` (record `27bb8860`, formerly `fabba150`, is the one this work settles; leave it open until Task 16) and the same for `crates/clax-realtime/src/working.rs`, `crates/clax-http/src/agent.rs`, `crates/clax-server/src/router.rs` and `crates/clax-server/src/routes/guards.rs`.

- [ ] **Step 2: Write the failing tests**

In `crates/clax-realtime/src/stream.rs` tests, add:

```rust
    #[test]
    fn only_an_attached_extension_stream_holding_questions_is_a_panel() {
        let hub = Hub::new(Default::default());
        let owner = viewer(Level::Owner, Some("u_o"));
        let tab = hub.open_for(owner.clone(), true, None, None, Some(String::new()));
        hub.update(&tab.id, &owner, &[Topic::Questions], &[]).unwrap();
        assert!(hub.holds_surface("") && !hub.holds_panel(""), "a tab is no panel");
        let panel = hub.open_for(owner.clone(), true, Some("cred".into()), None, Some(String::new()));
        hub.update_via(&panel.id, &owner, Some("cred"), &[Topic::Inbox], &[]).unwrap();
        assert!(!hub.holds_panel(""), "the panel's inbox alone is not its questions");
        hub.update_via(&panel.id, &owner, Some("cred"), &[Topic::Questions], &[]).unwrap();
        assert!(hub.holds_panel(""));
        assert!(!hub.holds_panel("u_sam"), "another person's");
    }
```

Create `crates/clax-testkit/src/where_to_ask.rs` (the helpers `api_where_to_ask` and `api_questions` share; integration modules are compiled in separate Bazel groups and cannot import each other), and add `pub mod where_to_ask;` to `crates/clax-testkit/src/lib.rs`:

```rust
//! Helpers for the tests of where an agent asks its person (spec
//! 2026-10-16-ask-where-attention-is-design): a daemon on a manual clock,
//! the session's attention, a typed prompt, an owner's tab, a session with
//! a page.

use crate::TestServer;
use clax_core::working::ManualClock;
use serde_json::{Value, json};
use std::sync::Arc;

/// A test daemon whose every clock reads one manual clock, at
/// 2026-10-16T10:00:00Z.
pub async fn clocked() -> (Arc<ManualClock>, TestServer) {
    let c = Arc::new(ManualClock::at("2026-10-16T10:00:00Z"));
    (c.clone(), TestServer::spawn_with_clock(c).await)
}

/// `GET /api/sessions/<sid>/attention` with the owner's token: `{attention, signals}`.
pub async fn attention(ts: &TestServer, sid: &str) -> Value {
    let r = ts.get_authed(&format!("/api/sessions/{sid}/attention")).await;
    assert_eq!(r.status(), 200);
    r.json::<Value>().await.unwrap()
}

/// `POST /api/sessions/<sid>/terminal`: the person typed a prompt now.
pub async fn typed(ts: &TestServer, sid: &str) -> Value {
    let r = ts.post_json(&format!("/api/sessions/{sid}/terminal"), json!({})).await;
    assert_eq!(r.status(), 200);
    r.json().await.unwrap()
}

/// A tab of the owner's on `aid` reports here, focused or not, with its
/// last input `idle_ms` before the report.
pub async fn owner_tab(ts: &TestServer, aid: &str, tab: &str, focused: bool, idle_ms: u64) {
    // The owner's viewer row exists from here on, as a browser of theirs makes it.
    ts.owner_public_id().await;
    let r = ts
        .client
        .put(format!("{}/api/viewers/me/presence", ts.base))
        .header("cookie", ts.owner_cookie())
        .json(&json!({"artifact_id": aid, "state": "here", "tab": tab, "focused": focused, "idle_ms": idle_ms}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
}

/// A live claude session that has published a page: `(sid, aid)`.
pub async fn paged(ts: &TestServer, hsid: &str) -> (String, String) {
    let sid = ts.register_session("claude", hsid).await["id"].as_str().unwrap().to_string();
    let a = ts.publish_as(&sid, "Board", "<main><h2>Goals</h2></main>").await;
    (sid, a["artifact"]["id"].as_str().unwrap().to_string())
}
```

Create `crates/clax-server/tests/api_where_to_ask.rs`:

```rust
//! Where an agent should ask its person (spec
//! 2026-10-16-ask-where-attention-is-design §1): the session's signals,
//! the attention route, terminal activity, and deliveries.

use clax_testkit::TestServer;
use clax_testkit::where_to_ask::{attention, clocked, owner_tab, paged, typed};
use serde_json::{Value, json};

fn says(v: &Value) -> (&str, &str) {
    (
        v["attention"]["surface"].as_str().unwrap(),
        v["attention"]["confidence"].as_str().unwrap(),
    )
}

#[tokio::test]
async fn with_no_clax_open_the_terminal_is_clear() {
    let (_c, ts) = clocked().await;
    let (sid, _aid) = paged(&ts, "h1").await;
    let v = attention(&ts, &sid).await;
    assert_eq!(says(&v), ("terminal", "clear"));
    assert!(v["attention"]["why"].as_str().unwrap().contains("No Clax tab"));
}

#[tokio::test]
async fn a_hidden_tab_is_not_attention() {
    let (c, ts) = clocked().await;
    let (sid, aid) = paged(&ts, "h1").await;
    // A tab holding `questions` while hidden: it can show a question, and
    // nothing more.
    let _hidden = ts.stream_as_owner(&["questions"]).await;
    let t = typed(&ts, &sid).await;
    assert_eq!(t["terminal_at"], "2026-10-16T10:00:00.000Z");
    let v = attention(&ts, &sid).await;
    assert_eq!(says(&v), ("terminal", "clear"));
    assert_eq!(v["attention"]["where"]["artifact_id"], aid.as_str());
    assert_eq!(v["signals"]["terminal_at"], "2026-10-16T10:00:00.000Z");
    c.advance(clax_core::surface::QUIET_SECS + 1);
    assert_eq!(says(&attention(&ts, &sid).await), ("clax", "unsure"), "quiet leans to Clax");
}

#[tokio::test]
async fn focused_input_newer_than_the_prompt_suggests_clax_at_the_sessions_page() {
    let (c, ts) = clocked().await;
    let (sid, aid) = paged(&ts, "h1").await;
    let _stream = ts.stream_as_owner(&["questions"]).await;
    typed(&ts, &sid).await;
    c.advance(120);
    owner_tab(&ts, &aid, "t1", true, 0).await;
    let v = attention(&ts, &sid).await;
    assert_eq!(says(&v), ("clax", "clear"));
    assert_eq!(v["attention"]["where"], json!({"artifact_id": aid}));
    // Within a minute of the prompt is too close to tell.
    c.advance(10);
    typed(&ts, &sid).await;
    assert_eq!(says(&attention(&ts, &sid).await), ("clax", "unsure"));
}

#[tokio::test]
async fn focus_on_another_artifact_is_not_attention_on_the_sessions_page() {
    let (c, ts) = clocked().await;
    let (sid, _aid) = paged(&ts, "h1").await;
    let (_other_sid, other) = paged(&ts, "h2").await;
    let _stream = ts.stream_as_owner(&["questions"]).await;
    typed(&ts, &sid).await;
    c.advance(120);
    owner_tab(&ts, &other, "t2", true, 0).await;
    assert_eq!(says(&attention(&ts, &sid).await), ("terminal", "clear"));
}

#[tokio::test]
async fn a_sent_thread_places_the_suggestion_in_that_thread() {
    let (c, ts) = clocked().await;
    let (sid, aid) = paged(&ts, "h1").await;
    let _stream = ts.stream_as_owner(&["questions"]).await;
    typed(&ts, &sid).await;
    c.advance(120);
    let t = ts.thread_as_owner(&aid, "Bigger title?").await;
    let tid = t["id"].as_str().unwrap();
    let r = ts.get_authed(&format!("/api/sessions/{sid}/feedback?tier=piggyback")).await;
    assert_eq!(r.status(), 200);
    let v = attention(&ts, &sid).await;
    assert_eq!(says(&v), ("clax", "clear"));
    assert_eq!(v["attention"]["where"], json!({"thread_id": tid, "artifact_id": aid}));
    assert_eq!(v["signals"]["delivered_at"], "2026-10-16T10:02:00.000Z");
}

#[tokio::test]
async fn a_deleted_delivered_thread_places_the_suggestion_on_its_artifact() {
    let (_c, ts) = clocked().await;
    let (sid, aid) = paged(&ts, "h1").await;
    let _stream = ts.stream_as_owner(&["questions"]).await;
    let t = ts.thread_as_owner(&aid, "Gone soon").await;
    let tid = t["id"].as_str().unwrap().to_string();
    ts.get_authed(&format!("/api/sessions/{sid}/feedback?tier=piggyback")).await;
    ts.delete_thread_as_owner(&aid, &tid).await;
    let v = attention(&ts, &sid).await;
    assert_eq!(v["attention"]["where"], json!({"artifact_id": aid}));
}

#[tokio::test]
async fn an_ended_or_unknown_session_has_no_attention() {
    let (_c, ts) = clocked().await;
    let (sid, _aid) = paged(&ts, "h1").await;
    ts.end_session(&sid).await;
    let r = ts.get_authed(&format!("/api/sessions/{sid}/attention")).await;
    assert_eq!(r.status(), 400);
    let r = ts.post_json(&format!("/api/sessions/{sid}/terminal"), json!({})).await;
    assert_eq!(r.status(), 400);
    let r = ts.get_authed("/api/sessions/s_nope/attention").await;
    assert_eq!(r.status(), 404);
}

#[tokio::test]
async fn a_persons_agent_reads_its_persons_engagement_not_the_owners() {
    let ts = TestServer::spawn_on_with_shutdown("0.0.0.0".parse().unwrap()).await;
    let sam = ts.connect_person("Sam", "clax:full").await;
    let sid = sam.register("claude", "h1").await["id"].as_str().unwrap().to_string();
    // The owner has Clax open; Sam does not.
    let _owners = ts.stream_as_owner(&["questions"]).await;
    let r = sam.agent("POST", &format!("/api/sessions/{sid}/terminal"), Some(&sid), Some(json!({}))).await;
    assert_eq!(r.status(), 200, "a signed request from another machine is stamped here");
    let r = sam.agent("GET", &format!("/api/sessions/{sid}/attention"), Some(&sid), None).await;
    assert_eq!(r.status(), 200);
    let v: Value = r.json().await.unwrap();
    assert_eq!(says(&v), ("terminal", "clear"));
    assert!(v["attention"]["why"].as_str().unwrap().contains("No Clax tab"), "{v}");
}

#[tokio::test]
async fn a_persons_agent_is_never_shown_a_page_it_may_not_read() {
    let ts = TestServer::spawn_on_with_shutdown("0.0.0.0".parse().unwrap()).await;
    let a = ts.publish("Owner's", &[("index.html", "<p>")]).await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let sam = ts.connect_person("Sam", "clax:full").await;
    let sid = sam.register("claude", "h1").await["id"].as_str().unwrap().to_string();
    let r = sam.agent("PUT", &format!("/api/sessions/{sid}/watches/{aid}"), Some(&sid), None).await;
    assert!(r.status().is_success(), "{}", r.text().await.unwrap());
    // The owner narrows Sam to his own space: the watched page is no longer his to read.
    let sam_public = ts.store.viewer_of_grant(&sam.grant_id()).unwrap().public_id;
    let r = ts
        .client
        .put(format!("{}/api/people/{sam_public}/rights", ts.base))
        .header("cookie", ts.owner_cookie())
        .json(&json!({"rights": [
            {"type": "clax-artifacts", "actions": ["read"], "locations": ["own"]},
            {"type": "clax-ask", "actions": ["ask"]}
        ]}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    let r = sam.agent("GET", &format!("/api/sessions/{sid}/attention"), Some(&sid), None).await;
    assert_eq!(r.status(), 200);
    let v: Value = r.json().await.unwrap();
    assert!(v["attention"]["where"].get("artifact_id").is_none(), "{v}");
}
```

(The rights body is the shape `Rights::parse` reads, `crates/clax-core/src/rights.rs`.)

Add to `crates/clax-server/tests/integration.rs`, after `mod api_watches;`:

```rust
#[cfg(any(not(clax_split), clax_split = "api_where_to_ask"))]
mod api_where_to_ask;
```

- [ ] **Step 3: Run the tests to see them fail**

Run: `bazelisk test //crates/clax-realtime:clax_realtime_lib_test --test_arg=stream::tests::only_an_attached && bazelisk test //crates/clax-server:all --test_arg=api_where_to_ask::`
Expected: compile error `no method holds_panel`; then 404 for the attention route.

- [ ] **Step 4: Write the implementation**

In `crates/clax-realtime/src/stream.rs`, change `holds_surface`'s doc first sentence to "Whether Clax can show a question to the person with recipient key `person` (empty: the owner): a stream of theirs, attached or detached within [`GRACE`], holds the `questions` or the `inbox` topic. A tab holds them while hidden, so this is never attention (`clax_http::surface`)." (keep the rest: it reads the topics' channels and scans no stream) and add after it:

```rust
    /// Whether the extension's side panel of the person with recipient key
    /// `person` is open: a stream of theirs opened through the extension
    /// gateway, attached now, holds `questions` (the panel's worker holds it
    /// only while a panel is open, spec 2026-10-06 §9.6). Unlike
    /// [`Hub::holds_surface`] it scans the streams.
    pub fn holds_panel(&self, person: &str) -> bool {
        let g = self.lock();
        g.streams.values().any(|s| {
            s.credential.is_some()
                && s.detached_at.is_none()
                && s.person.as_deref() == Some(person)
                && s.topics.contains_key(&Topic::Questions)
        })
    }
```

Create `crates/clax-http/src/surface.rs`:

```rust
//! The suggestion of where an agent session should ask its person (spec
//! 2026-10-16-ask-where-attention-is-design §1), gathered from the store's
//! session signals, the person's presence on the session's pages, and the
//! streams the person holds. The routes that answer with it (`attention`,
//! `questions`, `feedback`) share it.

use crate::agent::{Agent, Need};
use crate::error::ApiError;
use crate::state::AppState;
use chrono::{DateTime, Utc};
use clax_core::CoreError;
use clax_core::surface::{Delivery, Signals, Suggestion, TerminalSignal, suggest};
use clax_store::Store;
use clax_store::inbox::recipient_key;

fn parse(t: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(t)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// The session's page, as agent `a` may see it: its newest working
/// record's artifact, else the one it last published to or started
/// watching, through the same gate as an artifact the request names
/// ([`Agent::named_artifact`]): for a person's agent, a live page or one
/// its rights do not let it read is no page.
///
/// # Errors
/// The store's.
pub async fn page<N: Need>(
    s: &AppState,
    a: &Agent<N>,
    sid: &str,
) -> Result<Option<String>, ApiError> {
    let raw = match s.working.newest_artifact_of(sid) {
        Some(aid) => Some(aid),
        None => {
            let id = sid.to_string();
            s.store_call(move |st| st.session_artifact(&id)).await?
        }
    };
    Ok(match raw {
        Some(raw) => a
            .named_artifact(s, &raw)
            .await
            .ok()
            .map(|id| id.as_str().to_string()),
        None => None,
    })
}

/// Session `sid`'s signals now, with `page` its page ([`page`]); a page
/// deleted since places nothing. The person whose presence counts is the
/// session's: a connected person's for their agents, else the owner's.
/// Only their tabs on the session's pages (its delivery's artifact and its
/// page) count. A delivered thread no longer on its artifact places nothing.
///
/// # Errors
/// `NotFound` for an unknown session; the store's.
pub fn signals(
    s: &AppState,
    st: &Store,
    sid: &str,
    page: Option<&str>,
) -> clax_core::Result<Signals> {
    let sess = st.get_session(sid)?.ok_or(CoreError::NotFound)?;
    let kept = st.session_signals(sid)?.unwrap_or_default();
    let viewer = match &sess.person {
        Some(p) => Some(p.clone()),
        None => st.owner_public_id()?,
    };
    let key = recipient_key(sess.person.as_deref()).to_string();
    let delivery = match (
        kept.delivered_at.as_deref().and_then(parse),
        kept.delivered_artifact_id,
    ) {
        (Some(at), Some(aid)) => {
            let thread_id = match kept.delivered_thread_id {
                Some(t) => st
                    .get_thread(&t)?
                    .filter(|x| x.artifact_id == aid)
                    .map(|x| x.id),
                None => None,
            };
            Some(Delivery {
                at,
                artifact_id: aid,
                thread_id,
            })
        }
        _ => None,
    };
    let page = match page.map(clax_core::ArtifactId::parse) {
        Some(Ok(id)) => st.get_artifact(&id)?.map(|a| a.id.as_str().to_string()),
        _ => None,
    };
    let mut pages: Vec<&str> = Vec::with_capacity(2);
    pages.extend(delivery.as_ref().map(|d| d.artifact_id.as_str()));
    pages.extend(page.as_deref());
    pages.dedup();
    let clax_at = viewer
        .as_deref()
        .and_then(|v| pages.iter().filter_map(|a| s.presence.engaged_at(a, v)).max());
    let panel_open = s.stream.holds_panel(&key);
    Ok(Signals {
        terminal: kept
            .terminal_at
            .as_deref()
            .and_then(parse)
            .map(|at| TerminalSignal {
                at,
                inferred: kept.terminal_inferred,
            }),
        delivery,
        clax_at,
        artifact_id: page,
        clax_open: panel_open || s.stream.holds_surface(&key),
        panel_open,
    })
}

/// [`suggest`] for session `sid` with page `page`, now by the question
/// clock.
///
/// # Errors
/// [`signals`]'.
pub fn suggestion(
    s: &AppState,
    st: &Store,
    sid: &str,
    page: Option<&str>,
) -> clax_core::Result<Suggestion> {
    Ok(suggest(&signals(s, st, sid, page)?, s.question_clock.now()))
}
```

Create `crates/clax-server/src/routes/surface.rs`:

```rust
//! `GET /api/sessions/<sid>/attention`: where the session should ask its
//! person now (spec 2026-10-16-ask-where-attention-is-design §1); `POST
//! /api/sessions/<sid>/terminal`: the person typed a prompt to it in the
//! terminal (Pi's input event; Claude Code's prompt hook says so on its
//! feedback poll instead, `prompt=true`). Both take the session's token or
//! its agent token; an unknown session is 404, an ended one 400
//! `unknown_session`.

use super::artifacts::path;
use axum::Json;
use axum::extract::rejection::PathRejection;
use axum::extract::{Path, State};
use clax_core::CoreError;
use clax_http::agent::TokenOrAgent;
use clax_http::audit::DeferredAudit;
use clax_http::error::ApiError;
use clax_http::state::AppState;
use clax_store::Store;
use serde_json::{Value, json};

fn live(st: &Store, sid: &str) -> clax_core::Result<()> {
    match st.get_session(sid)? {
        None => Err(CoreError::NotFound),
        Some(x) if x.ended_at.is_some() => Err(CoreError::invalid(
            "unknown_session",
            "no live session has this ID",
        )),
        Some(_) => Ok(()),
    }
}

/// → `{attention, signals}`: the suggestion (`surface`, `where`,
/// `confidence`, `why`) and the session's own signals it read
/// (`terminal_at`, `terminal_inferred`, `delivered_at`).
pub async fn attention(
    State(s): State<AppState>,
    TokenOrAgent(a): TokenOrAgent<clax_http::agent::SessionAttention>,
    p: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let sid = path(p)?;
    let page = clax_http::surface::page(&s, &a, &sid).await?;
    let st = s.clone();
    let v = s
        .store_call(move |db| {
            live(db, &sid)?;
            let sugg = clax_http::surface::suggestion(&st, db, &sid, page.as_deref())?;
            let k = db.session_signals(&sid)?.unwrap_or_default();
            Ok(json!({"attention": sugg, "signals": {
                "terminal_at": k.terminal_at,
                "terminal_inferred": k.terminal_inferred,
                "delivered_at": k.delivered_at,
            }}))
        })
        .await?;
    Ok(Json(v))
}

/// Records the person's prompt now, by the daemon's clock, and withdraws
/// the session's open hook notices (`question.withdraw`, reason `lapsed`,
/// as the session's agent), announcing each → `{terminal_at, lapsed}`.
pub async fn terminal(
    State(s): State<AppState>,
    TokenOrAgent(_a): TokenOrAgent<clax_http::agent::SessionTerminal>,
    audit: DeferredAudit,
    p: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let sid = path(p)?;
    let st = s.clone();
    let v = s
        .store_call(move |db| {
            live(db, &sid)?;
            let ctx = audit.for_session(db, &sid)?;
            let lapsed = db.note_terminal(&ctx, &sid)?;
            clax_http::views::question::announce_ids(&st, db, &lapsed);
            let at = db.session_signals(&sid)?.and_then(|k| k.terminal_at);
            Ok(json!({"terminal_at": at, "lapsed": lapsed}))
        })
        .await?;
    Ok(Json(v))
}
```

Wire it:
- `crates/clax-http/src/lib.rs`: `pub mod surface;` after `pub mod state;`.
- `crates/clax-server/src/routes/mod.rs`: `pub mod surface;` after `pub mod stream;`.
- `crates/clax-server/src/router.rs`: add `surface` to the `use crate::routes::{…}` list (lines 6-10), and after the `/api/sessions/{id}/tool-call-ids` route (lines 194-197):

```rust
        .route("/api/sessions/{id}/attention", get(surface::attention))
        .route("/api/sessions/{id}/terminal", post(surface::terminal))
```

- `crates/clax-server/src/routes/guards.rs`: in the class table `GUARDS`, `("surface::attention", AgentPath),` and `("surface::terminal", AgentPath),` after `("feedback::notices", AgentPath),` (line 119); in `module_source_opt`, `"surface" => include_str!("surface.rs"),` after `"stream"` (the guards read each route module's source from there, and panic on a module they cannot name); in `EXPECTED`, after the `questions:terminal` entry (lines 1169-1174):

```rust
            (
                "GET",
                "/api/sessions/{id}/attention",
                Own,
                Some((Ask, "ask")),
            ),
            ("POST", "/api/sessions/{id}/terminal", Own, None),
```

- `crates/clax-http/src/agent.rs`: after `need!(QuestionsTerminal, …)` (line 253):

```rust
need!(SessionAttention, Ask, "ask", Own, DEFAULT_LIMIT);
need!(SessionTerminal, None, Own, DEFAULT_LIMIT);
```

  in `ROUTES`, after the `/api/sessions/{id}/tool-call-ids` entry: `("GET", "/api/sessions/{id}/attention", SessionAttention::INFO),` and `("POST", "/api/sessions/{id}/terminal", SessionTerminal::INFO),`; in `ROUTE_NEEDS`, after `route_need!("sessions::tool_call_ids", …)`: `route_need!("surface::attention", SessionAttention),` and `route_need!("surface::terminal", SessionTerminal),`. `docs/contract.md`'s route table (from line 3377) is generated from `ROUTE_NEEDS`: rewrite it with `CLAX_UPDATE_GOLDEN=1` on `the_contract_route_table_is_the_declared_one` (`bazelisk test //crates/clax-server:clax_server_lib_test --test_env=CLAX_UPDATE_GOLDEN=1 --test_arg=the_contract_route_table_is_the_declared_one`, or by hand if the sandbox keeps the write).

In `crates/clax-realtime/src/working.rs` `mark_items` (it takes the `FeedbackCtx`, the audit context, the store and the session; `clax-realtime` may use the store), after `settle(…)`:

```rust
    // The newest item is what the session acts on next: Clax instigated it,
    // at that thread (spec 2026-10-16-ask-where-attention-is-design §1).
    if let Some(last) = items.last() {
        st.note_delivery(session_id, &last.artifact_id, Some(&last.thread_id))?;
    }
```

and add to its doc comment: "It also records the delivery on the session (`Store::note_delivery`), the newest item's artifact and thread."

- [ ] **Step 5: Run the tests to see them pass**

Run: `bazelisk test //crates/clax-realtime:clax_realtime_lib_test --test_arg=stream:: && bazelisk test //crates/clax-server:all --test_arg=routes:: --test_arg=api_where_to_ask:: --test_arg=api_agent_auth:: --test_arg=api_people:: --test_arg=api_canary:: && bazelisk test //scripts:crate_layers`
Expected: PASS, the guards (`routes::guards`), the canary sweeps (`api_agent_auth`, `api_people`, `api_canary`) and the layering check included with the two new routes.

Run: `just check-wasm && just worker-test`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/clax-http/src/surface.rs crates/clax-http/src/lib.rs crates/clax-http/src/agent.rs crates/clax-server/src/routes/surface.rs crates/clax-server/src/routes/mod.rs crates/clax-server/src/router.rs crates/clax-server/src/routes/guards.rs crates/clax-realtime/src/stream.rs crates/clax-realtime/src/working.rs crates/clax-testkit/src/where_to_ask.rs crates/clax-testkit/src/lib.rs crates/clax-server/tests/api_where_to_ask.rs crates/clax-server/tests/integration.rs docs/contract.md
git -c commit.gpgsign=false commit -m "Serve where a session should ask its person: GET /api/sessions/<sid>/attention from its prompts, deliveries and its person's focused tabs, and POST …/terminal for a typed prompt"
```

---

## Task 6: Question routes: thread and notice on create, the hook's mode from the suggestion, Ask me here

**Files:**
- Modify: `crates/clax-server/src/routes/questions.rs:157-166` (`CreateBody`), `:168-319` (`create`), `:327-437` (`poll`), `:500-533` (`terminal`; the status check at line 522), after `release_owner` (`:733-749`, add `pull`)
- Modify: `crates/clax-http/src/views/question.rs:12-56` (`view` gains `thread`, `notice`, `pulled`), `:58-77` (`announce` wakes feedback polls for a pull)
- Modify: `crates/clax-http/src/views/inbox.rs:327-337` (the `Kind::Question` arm: a thread question's item opens its thread)
- Modify: `crates/clax-server/src/router.rs:216-221` (route `/api/questions/{qid}/pull` after the release route)
- Modify: `crates/clax-server/src/routes/guards.rs:66-188` (class table: `("questions::pull", Person)` after `("questions::release_owner", Person)`, line 142), `:1253-1290` (`MUST_REFUSE`: `"questions::pull"` after `"questions::release_owner"`, line 1274)
- Modify: `crates/clax-http/src/extension.rs:221` (the gateway admits `pull`), its table test (`:554-556`)
- Modify: `docs/contract.md` "## Agent questions" (line 2518: session routes, owner routes, the question view)
- Test: `crates/clax-server/tests/api_questions.rs`

**Interfaces:**
- Consumes: `clax_http::surface::{page, suggestion}` (Task 5); `NewQuestion { thread_id, notice }`, `Close::Pull`, `Close::Terminal` on notices (Task 1).
- Produces:
  - `POST /api/sessions/{id}/questions` body gains `thread_id?: string` (needs `artifact_id`; 400 `invalid_thread`) and `notice?: bool` (`ask` source only); result gains `attention`; `mode` is `terminal` for a released question or a notice.
  - `GET /api/sessions/{id}/questions/{qid}` result gains `attention`.
  - `POST /api/questions/{qid}/pull` (Person) → `{question}`; 400 `not_a_notice`; 409 `question_closed`.
  - Question view gains `thread: {id, open} | null`, `notice: bool`, `pulled: bool`.

- [ ] **Step 1: Write the failing tests**

The existing hook tests that hold `ts.stream_as_owner(&["questions"])` and record no prompt get a Clax, unsure suggestion and still wait: they stay as they are. One changes: in `hook_mode_is_terminal_without_a_surface` (line 1036), a session with a page and no surface is now asked in the terminal as an open notice, so its first assertion becomes

```rust
    assert_eq!(
        (r["mode"].as_str(), r["question"]["status"].as_str(), r["question"]["notice"].as_bool()),
        (Some("terminal"), Some("open"), Some(true))
    );
```

(the `t2` case, a stream holding only `gallery`, stays `mode: "terminal"`). Add to `crates/clax-server/tests/api_questions.rs`:

```rust
use clax_testkit::where_to_ask::{clocked, owner_tab, paged as paged_at, typed};

#[tokio::test]
async fn a_hook_question_after_a_newer_terminal_prompt_is_released_at_once_and_leaves_a_notice() {
    let (_c, ts) = clocked().await;
    let mut tap = owner_tap(&ts).await;
    let (sid, aid) = paged_at(&ts, "h1").await;
    typed(&ts, &sid).await;
    let before = ts.store.inbox_unread(None).unwrap();
    let r: Value = post(&ts, &format!("/api/sessions/{sid}/questions"), hook("t1")).await.json().await.unwrap();
    assert_eq!(r["mode"], "terminal");
    assert_eq!(r["attention"]["surface"], "terminal");
    let q = &r["question"];
    assert_eq!((q["status"].as_str(), q["notice"].as_bool()), (Some("open"), Some(true)));
    assert_eq!(q["artifact"]["id"], aid.as_str());
    assert_eq!(tap.next().await["notice"], true, "Clax hears of the notice");
    assert_eq!(ts.store.inbox_unread(None).unwrap(), before + 1, "it counts in the inbox");
    // No grace withdraws it: its hook is not waiting.
    assert_eq!(ts.question_waiters(q["id"].as_str().unwrap()).await, 0);
}

#[tokio::test]
async fn a_hook_question_with_newer_focused_clax_input_waits_in_clax() {
    let (c, ts) = clocked().await;
    let _surface = ts.stream_as_owner(&["questions"]).await;
    let (sid, aid) = paged_at(&ts, "h1").await;
    typed(&ts, &sid).await;
    c.advance(120);
    owner_tab(&ts, &aid, "t1", true, 0).await;
    let r: Value = post(&ts, &format!("/api/sessions/{sid}/questions"), hook("t1")).await.json().await.unwrap();
    assert_eq!((r["mode"].as_str(), r["question"]["notice"].as_bool()), (Some("wait"), Some(false)));
    assert_eq!(r["attention"]["confidence"], "clear");
}

#[tokio::test]
async fn a_hidden_tab_does_not_hold_a_hook_question_after_a_prompt() {
    let (_c, ts) = clocked().await;
    let _hidden = ts.stream_as_owner(&["questions"]).await;
    let (sid, _aid) = paged_at(&ts, "h1").await;
    typed(&ts, &sid).await;
    let r: Value = post(&ts, &format!("/api/sessions/{sid}/questions"), hook("t1")).await.json().await.unwrap();
    assert_eq!(r["mode"], "terminal");
}

#[tokio::test]
async fn a_hook_question_of_a_session_with_no_page_is_asked_in_the_terminal_without_a_notice() {
    let ts = TestServer::spawn().await;
    let _surface = ts.stream_as_owner(&["questions"]).await;
    let sid = session(&ts, "h1").await;
    let r: Value = post(&ts, &format!("/api/sessions/{sid}/questions"), hook("t1")).await.json().await.unwrap();
    assert_eq!(
        (r["mode"].as_str(), r["question"]["status"].as_str(), r["question"]["notice"].as_bool()),
        (Some("terminal"), Some("released"), Some(false))
    );
    assert_eq!(ts.store.inbox_unread(None).unwrap(), 0);
}

#[tokio::test]
async fn the_asked_hook_closes_a_notice_with_the_terminals_answer() {
    let (_c, ts) = clocked().await;
    let (sid, _aid) = paged_at(&ts, "h1").await;
    let _surface = ts.stream_as_owner(&["questions"]).await;
    typed(&ts, &sid).await;
    let before = ts.store.inbox_unread(None).unwrap();
    let r = ts.ask(&sid, hook("t1")).await;
    let qid = r["question"]["id"].as_str().unwrap().to_string();
    let res = post(&ts, &format!("/api/sessions/{sid}/questions:terminal"),
        json!({"tool_use_id": "t1", "answers": {"Which?": "A"}})).await;
    assert_eq!(res.status(), 200);
    assert_eq!(ts.question_status(&qid).await, "answered");
    assert_eq!(ts.store.inbox_unread(None).unwrap(), before, "answered in the terminal: read");
}

#[tokio::test]
async fn a_prompt_lapses_an_unanswered_hook_notice() {
    let (_c, ts) = clocked().await;
    let (sid, _aid) = paged_at(&ts, "h1").await;
    let _surface = ts.stream_as_owner(&["questions"]).await;
    typed(&ts, &sid).await;
    let qid = ts.ask(&sid, hook("t1")).await["question"]["id"].as_str().unwrap().to_string();
    let t = typed(&ts, &sid).await;
    assert_eq!(t["lapsed"], json!([qid]));
    assert_eq!(ts.question_status(&qid).await, "withdrawn");
}

#[tokio::test]
async fn ask_me_here_reaches_the_agent_as_a_delivery() {
    let (_c, ts) = clocked().await;
    let (sid, aid) = paged_at(&ts, "h1").await;
    let mut b = body();
    b["artifact_id"] = json!(aid);
    b["notice"] = json!(true);
    let r = ts.ask(&sid, b).await;
    assert_eq!((r["mode"].as_str(), r["question"]["notice"].as_bool()), (Some("terminal"), Some(true)));
    let qid = r["question"]["id"].as_str().unwrap().to_string();
    let res = ts.client.post(format!("{}/api/questions/{qid}/answer", ts.base)).header("cookie", ts.owner_cookie())
        .json(&json!({"answers": [{"selected": ["A"], "text": null}]})).send().await.unwrap();
    assert_eq!(res.status(), 400, "a notice takes no answer in Clax");
    let res = ts.client.post(format!("{}/api/questions/{qid}/pull", ts.base)).header("cookie", ts.owner_cookie()).send().await.unwrap();
    assert_eq!(res.status(), 200);
    let v: Value = res.json().await.unwrap();
    assert_eq!((v["question"]["status"].as_str(), v["question"]["pulled"].as_bool()), (Some("withdrawn"), Some(true)));
    let late = late(&ts, &sid).await;
    assert_eq!(late[0]["id"], qid.as_str());
    assert!(late(&ts, &sid).await.as_array().unwrap().is_empty(), "once");
    let q = ts.ask(&sid, body()).await["question"]["id"].as_str().unwrap().to_string();
    let res = ts.client.post(format!("{}/api/questions/{q}/pull", ts.base)).header("cookie", ts.owner_cookie()).send().await.unwrap();
    assert_eq!(res.status(), 400, "a question is no notice");
}

#[tokio::test]
async fn a_thread_question_names_its_thread_and_its_inbox_item_opens_it() {
    let ts = TestServer::spawn().await;
    let (sid, aid) = paged_at(&ts, "h1").await;
    let t = ts.thread_as_owner(&aid, "Bigger title?").await;
    let tid = t["id"].as_str().unwrap();
    let mut b = body();
    b["artifact_id"] = json!(aid);
    b["thread_id"] = json!(tid);
    let r = ts.ask(&sid, b.clone()).await;
    assert_eq!(r["question"]["thread"], json!({"id": tid, "open": true}));
    let items: Value = ts.client.get(format!("{}/api/inbox?kind=question", ts.base)).header("cookie", ts.owner_cookie())
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(items["items"][0]["url"], format!("/a/{aid}?thread={tid}"));
    let mut off = body();
    off["thread_id"] = json!(tid);
    let res = post(&ts, &format!("/api/sessions/{sid}/questions"), off).await;
    assert_eq!(res.status(), 400, "a thread needs its artifact");
    let (_s2, other) = paged_at(&ts, "h2").await;
    b["artifact_id"] = json!(other);
    let res = post(&ts, &format!("/api/sessions/{sid}/questions"), b).await;
    assert_eq!(res.status(), 400);
    assert_eq!(res.json::<Value>().await.unwrap()["error"]["code"], "invalid_thread");
}

#[tokio::test]
async fn a_deleted_thread_leaves_its_question_on_the_artifact() {
    let ts = TestServer::spawn().await;
    let (sid, aid) = paged_at(&ts, "h1").await;
    let t = ts.thread_as_owner(&aid, "Gone soon").await;
    let tid = t["id"].as_str().unwrap().to_string();
    let mut b = body();
    b["artifact_id"] = json!(aid);
    b["thread_id"] = json!(tid);
    let qid = ts.ask(&sid, b).await["question"]["id"].as_str().unwrap().to_string();
    ts.delete_thread_as_owner(&aid, &tid).await;
    let v: Value = ts.get_authed(&format!("/api/sessions/{sid}/questions/{qid}")).await.json().await.unwrap();
    assert!(v["question"]["thread"].is_null());
    assert_eq!(v["question"]["artifact"]["id"], aid.as_str());
    assert!(v["attention"].is_object(), "every poll result carries the suggestion");
}
```

(`clocked`, `owner_tab`, `paged`, `typed` are `clax_testkit::where_to_ask`'s, Task 5; `body`, `hook`, `post`, `late`, `session`, `owner_tap` are this file's, lines 11-60.)

- [ ] **Step 2: Run the tests to see them fail**

Run: `bazelisk test //crates/clax-server:all --test_arg=api_questions::`
Expected: the new tests fail (`unknown field notice`, `mode` is `wait`, 404 on `pull`); the existing ones pass.

- [ ] **Step 3: Write the implementation**

In `crates/clax-server/src/routes/questions.rs`, add to `CreateBody`:

```rust
    /// A thread of `artifact_id` the question arose from (spec
    /// 2026-10-16-ask-where-attention-is-design §2).
    #[serde(default)]
    thread_id: Option<String>,
    /// An agent's notice that it asks in chat (§3): open until it cancels it.
    #[serde(default)]
    notice: bool,
```

In `create`, after `source` is parsed:

```rust
    if b.notice && source == Source::Hook {
        return Err(ApiError::bad_request(
            "invalid_question",
            "a mirrored question's notice is the daemon's to make",
        ));
    }
    if b.thread_id.is_some() && b.artifact_id.is_none() {
        return Err(ApiError::bad_request(
            "invalid_thread",
            "thread_id goes with artifact_id",
        ));
    }
    let (thread_id, asked_notice) = (b.thread_id.clone(), b.notice);
```

Before the `store_call` (after `fallback` is resolved), resolve the session's page through the agent's gate:

```rust
    let page = clax_http::surface::page(&s, &a, &sid).await?;
```

Inside the `store_call`, replace the `released` computation (lines 277-281, which keep a hook question in the terminal before its session has a page) with:

```rust
            let attention = clax_http::surface::suggestion(&st, db, &sid, page.as_deref())?;
            // A mirrored question of a session with no page is asked in the
            // terminal with no notice: its person is not using Clax for it.
            let released = source == Source::Hook && live.is_none();
            // One the suggestion sends to the terminal (every one, with
            // `terminal_after_s` 0) is asked there, and Clax shows its notice.
            let notice = asked_notice
                || (source == Source::Hook
                    && !released
                    && (attention.surface == clax_core::surface::Surface::Terminal || after == 0));
```

pass `thread_id, notice,` in the `NewQuestion` given to `db.create_question_keyed(…)` (the keyed path is unchanged: a resend answers the first question, and its `attention` is read anew), and return `(row, made, v, attention)`; after the call:

```rust
    if made && row.source == Source::Hook && row.status == Status::Open && !row.notice {
        start_grace(s.clone(), row.id.clone());
    }
    let mode = if row.status == Status::Released || row.notice {
        "terminal"
    } else {
        "wait"
    };
    let out = json!({"question": v, "mode": mode, "terminal_after_s": after,
        "surface_open": surface_open, "attention": attention});
```

Update `create`'s doc comment (lines 168-183): "A hook question … is created released when its session has no page (`mode: "terminal"`, no notice); otherwise, when the session's suggestion ([`clax_http::surface::suggestion`]) names the terminal or `terminal_after_s` is 0, it is created as an open notice (`mode: "terminal"`), which takes the terminal's answer (`questions:terminal`) or lapses at the next prompt; otherwise it waits in Clax and is withdrawn when no poll holds it for the grace. `thread_id` must name a thread of `artifact_id` (400 `invalid_thread`); `notice` (an `ask` question only) records an agent's notice that it asks in chat. The result carries the suggestion as `attention`."

In `poll`, before the final `store_call` (the one binding `(sid1, qid1, st, vf1, g1)`, line 402), add `let page = clax_http::surface::page(&s, &a, &sid).await?;`, and in it replace `for_asker(db, &vf1, v).map(Some)` (line 423) with:

```rust
                let attention = clax_http::surface::suggestion(&st, db, &sid1, page.as_deref())?;
                Ok(Some((for_asker(db, &vf1, v)?, attention)))
```

and the return with `Ok(Json(json!({"question": v, "waited_s": started.elapsed().as_secs(), "attention": attention})))` (destructure `if let Some((v, attention)) = out`); add "and the polling session's suggestion, `attention`" to its doc comment.

In `terminal`, replace the status check (`if q.status != Status::Released`, line 522) with:

```rust
            // A released question, or an open hook notice, took the answer
            // in the terminal.
            if !(q.status == Status::Released || (q.status == Status::Open && q.notice)) {
                return Ok(None);
            }
```

Add after `release_owner` (line 749):

```rust
/// `POST /api/questions/<qid>/pull` (the owner, or a connected person, for
/// their agents' notices): "Ask me here" (spec
/// 2026-10-16-ask-where-attention-is-design §3) → `{question}`: the notice
/// is withdrawn and its session is told once, at its next delivery, that
/// the person wants the question in Clax; 404 for another's; 400
/// `not_a_notice` for a question; 409 `question_closed` when it is not
/// open. Records `question.withdraw` (reason `pulled`).
pub async fn pull(
    State(s): State<AppState>,
    d: DeniedToGrants,
    _o: SameOrigin,
    who: Identity,
    audit: DeferredAudit,
    p: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, QuestionError> {
    d.refuse_agents()?;
    let me = recipient_of(&s, &who).await?;
    owner_close(s, me, audit, path(p)?, Close::Pull).await
}
```

Wire it in `crates/clax-server/src/router.rs` (`.route("/api/questions/{qid}/pull", post(questions::pull))` after the `/api/questions/{qid}/release` route), in `crates/clax-server/src/routes/guards.rs` (`("questions::pull", Person),` in the class table after `questions::release_owner`, and `"questions::pull",` in `MUST_REFUSE` after `"questions::release_owner"`), and in `crates/clax-http/src/extension.rs:221`: `["api", "questions", _, "answer" | "decline" | "release" | "pull"] if post => Some(Rule::Any),` (and add `(&p, "/api/questions/Q/pull", Some(Rule::Any)),` to its table test after line 556).

In `crates/clax-http/src/views/question.rs` `view`, before the `Ok(json!(…))` (line 43):

```rust
    // Its thread while it is one of its artifact's; null once deleted.
    let thread = match (&q.thread_id, &q.artifact_id) {
        (Some(t), Some(aid)) => st
            .get_thread(t)?
            .filter(|x| &x.artifact_id == aid)
            .map(|x| json!({"id": x.id, "open": x.status == "open"})),
        _ => None,
    };
```

and add `"thread": thread, "notice": q.notice, "pulled": q.pulled_at.is_some(),` to the object (beside `linked`, which the view gained for linked questions). In `announce`, extend the wake (line 73):

```rust
    if (q.source == Source::Ask && matches!(q.status, Status::Answered | Status::Declined))
        || q.pulled_at.is_some()
    {
        s.feedback_waiters.wake(std::iter::once(&q.session_id));
    }
```

In `crates/clax-http/src/views/inbox.rs`, in the `Kind::Question` arm, inside `Some(q) =>` after `v["question"] = …` (line 332):

```rust
                    if let (Some(aid), Some(tid)) = (&q.artifact_id, &q.thread_id) {
                        v["url"] = json!(format!("/a/{aid}?thread={tid}"));
                    }
```

In `docs/contract.md` "Agent questions": add `thread_id?` and `notice?` to the `POST /api/sessions/<sid>/questions` body and `attention` to its and the poll's results; add `POST /api/questions/<qid>/pull` to the owner routes; add `thread`, `notice`, `pulled` to the question view; add the `open` → `withdrawn` rows for reasons `pulled` and `lapsed` to the transitions table; and add `GET /api/sessions/<sid>/attention` and `POST /api/sessions/<sid>/terminal` to the session routes, with the suggestion's shape and the windows (60 s, 5 min).

- [ ] **Step 4: Run the tests to see them pass**

Run: `bazelisk test //crates/clax-server:all --test_arg=routes:: --test_arg=api_questions:: --test_arg=api_inbox:: --test_arg=api_extension:: --test_arg=api_gateway:: --test_arg=api_linked:: --test_arg=api_agent_auth:: && bazelisk test //crates/clax-http:clax_http_lib_test --test_arg=extension::`
Expected: PASS.

Run: `just check-wasm && just worker-test`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/clax-server/src/routes/questions.rs crates/clax-http/src/views/question.rs crates/clax-http/src/views/inbox.rs crates/clax-server/src/router.rs crates/clax-server/src/routes/guards.rs crates/clax-http/src/extension.rs crates/clax-server/tests/api_questions.rs docs/contract.md
git -c commit.gpgsign=false commit -m "Ask a mirrored question where the person is: a newer terminal prompt releases it at once as a notice, a hidden tab no longer holds it; questions take a thread, agents post notices, and Ask me here pulls one to Clax"
```

---

## Task 7: Feedback poll: the prompt flag, inferred Codex and Grok turns, deliveries and pulls, the attention line

**Files:**
- Modify: `crates/clax-server/src/routes/feedback.rs:51-87` (`late_answers`), `:204-211` (`FeedbackQuery`), `:213-444` (`poll` and its doc comment; the audit context's first build at `:329-343`, the take at `:355-375`, the answer at `:401-427`)
- Modify: `crates/clax-http/src/surface.rs` (add `note_poll`, `INFERRED`)
- Test: `crates/clax-server/tests/api_where_to_ask.rs`

**Interfaces:**
- Consumes: `Store::note_terminal`, `Store::note_turn`, `Store::note_delivery` (Task 2); `clax_http::surface::{page, suggestion}` (Task 5); `QuestionRow::pulled_at`, `thread_id` (Task 1); `clax_core::surface::line` (Task 4).
- Produces:
  - `GET /api/sessions/{id}/feedback?…&prompt=true`: records a typed prompt first (lapsing hook notices).
  - A `tier=stop_hook` poll of a `codex` or `grok` session records the turn (`Store::note_turn`).
  - Late answers and pulls record a delivery at the question's artifact and thread; a pull renders as `[clax] The person wants your question "<header>" (<qid>) asked in Clax instead: …`.
  - A poll that hands anything over, or a `prompt=true` poll of a session with a page, carries `attention` and ends its `text` with `clax_core::surface::line`.
  - `clax_http::surface::note_poll(s: &AppState, st: &Store, ctx: &AuditCtx, sid: &str, prompt: bool, turn_ended: bool) -> clax_core::Result<()>`; `pub const INFERRED: [&str; 2] = ["codex", "grok"]`

- [ ] **Step 1: Write the failing tests**

Add to `crates/clax-server/tests/api_where_to_ask.rs` (its imports from `clax_testkit::where_to_ask` cover these):

```rust
async fn poll(ts: &TestServer, sid: &str, q: &str) -> Value {
    let r = ts.get_authed(&format!("/api/sessions/{sid}/feedback?{q}")).await;
    assert_eq!(r.status(), 200);
    r.json().await.unwrap()
}

#[tokio::test]
async fn the_prompt_hooks_poll_records_the_prompt_and_says_where_to_ask() {
    let (_c, ts) = clocked().await;
    let (sid, _aid) = paged(&ts, "h1").await;
    let _stream = ts.stream_as_owner(&["questions"]).await;
    let v = poll(&ts, &sid, "tier=prompt_hook&prompt=true").await;
    assert_eq!(v["attention"]["surface"], "terminal");
    assert!(
        v["text"].as_str().unwrap().contains("[clax] If you ask the person anything now, ask in the terminal (chat) (clear): "),
        "{v}"
    );
    assert_eq!(attention(&ts, &sid).await["signals"]["terminal_at"], "2026-10-16T10:00:00.000Z");
    // Without the flag (SessionStart's poll) nothing is recorded or said.
    let fresh = ts.register_session("claude", "h2").await["id"].as_str().unwrap().to_string();
    let v = poll(&ts, &fresh, "tier=prompt_hook").await;
    assert!(v["text"].is_null() && v.get("attention").is_none(), "{v}");
}

#[tokio::test]
async fn a_codex_turn_with_no_delivery_counts_as_an_inferred_prompt() {
    let (c, ts) = clocked().await;
    let sid = ts.register_session("codex", "h1").await["id"].as_str().unwrap().to_string();
    let a = ts.publish_as(&sid, "Board", "<main><h2>Goals</h2></main>").await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let _stream = ts.stream_as_owner(&["questions"]).await;
    let v = poll(&ts, &sid, "tier=stop_hook").await;
    assert!(v["text"].is_null(), "a Stop with nothing to hand over still ends the turn: {v}");
    let a = attention(&ts, &sid).await;
    assert_eq!(a["signals"]["terminal_inferred"], true);
    assert_eq!((a["attention"]["surface"].as_str(), a["attention"]["confidence"].as_str()), (Some("clax"), Some("unsure")));
    // A turn that a thread from Clax started is not a terminal turn.
    c.advance(30);
    ts.thread_as_owner(&aid, "Bigger?").await;
    poll(&ts, &sid, "tier=piggyback").await;
    c.advance(30);
    poll(&ts, &sid, "tier=stop_hook").await;
    assert_eq!(attention(&ts, &sid).await["signals"]["terminal_at"], "2026-10-16T10:00:00.000Z");
    // A claude session's Stop infers nothing: its prompts are reported.
    let (claude, _) = paged(&ts, "h2").await;
    poll(&ts, &claude, "tier=stop_hook").await;
    assert!(attention(&ts, &claude).await["signals"]["terminal_at"].is_null());
}

#[tokio::test]
async fn a_late_answer_and_a_pull_are_deliveries_from_clax_at_their_thread() {
    let (c, ts) = clocked().await;
    let (sid, aid) = paged(&ts, "h1").await;
    let _stream = ts.stream_as_owner(&["questions"]).await;
    let t = ts.thread_as_owner(&aid, "Which?").await;
    let tid = t["id"].as_str().unwrap().to_string();
    let notice = json!({"source": "ask", "notice": true, "artifact_id": aid, "thread_id": tid,
        "questions": [{"question": "Which layout?", "header": "Layout"}]});
    let qid = ts.ask(&sid, notice).await["question"]["id"].as_str().unwrap().to_string();
    typed(&ts, &sid).await;
    c.advance(120);
    let r = ts.client.post(format!("{}/api/questions/{qid}/pull", ts.base)).header("cookie", ts.owner_cookie()).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let v = poll(&ts, &sid, "tier=piggyback").await;
    let text = v["text"].as_str().unwrap();
    assert!(
        text.contains(&format!("[clax] The person wants your question \"Layout\" ({qid}) asked in Clax instead: ask it now with `ask` with `url_or_id` \"{aid}\" and `thread_id` \"{tid}\"")),
        "{text}"
    );
    assert!(text.contains("ask in Clax, in thread"), "{text}");
    assert_eq!(v["attention"]["where"]["thread_id"], tid.as_str());
}
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `bazelisk test //crates/clax-server:all --test_arg=api_where_to_ask::`
Expected: the three new tests fail (`attention` missing; `terminal_inferred` false; no pull text).

- [ ] **Step 3: Write the implementation**

In `crates/clax-http/src/surface.rs`, add:

```rust
/// The harnesses no hook reports a prompt for (Codex, Grok Build): each
/// Stop with no Clax delivery since the last stands for one.
pub const INFERRED: [&str; 2] = ["codex", "grok"];

/// What a feedback poll of live session `sid` says of its person:
/// `prompt`, they typed to it (Claude Code's prompt hook); `turn_ended`, a
/// Stop hook ended its turn, which for a harness in [`INFERRED`] may stand
/// for a prompt ([`Store::note_turn`]). Lapsed notices are announced.
///
/// # Errors
/// The store's.
pub fn note_poll(
    s: &AppState,
    st: &Store,
    ctx: &clax_core::audit::AuditCtx,
    sid: &str,
    prompt: bool,
    turn_ended: bool,
) -> clax_core::Result<()> {
    if prompt {
        let lapsed = st.note_terminal(ctx, sid)?;
        crate::views::question::announce_ids(s, st, &lapsed);
    } else if turn_ended
        && st
            .get_session(sid)?
            .is_some_and(|x| INFERRED.contains(&x.harness.as_str()))
    {
        st.note_turn(sid)?;
    }
    Ok(())
}
```

In `crates/clax-server/src/routes/feedback.rs`:

`FeedbackQuery` gains `prompt: Option<bool>,`. In `poll`, after `tier` is parsed, add `let (prompt, turn_ended) = (q.prompt.unwrap_or(false), tier == Tier::StopHook);`.

The poll builds its audit context once, in the first iteration's view `store_call` (the `None =>` arm of `match prior_audit`, lines 333-336); record the poll's signals there, so they are recorded once per poll and before its first take. With `let st0 = s.clone();` beside `let (prior_audit, headers) = …` (line 328), that arm becomes:

```rust
                    None => {
                        live_session(st, &sid)?;
                        let ctx = clax_http::audit::session_ctx(st, &headers, &sid)?;
                        clax_http::surface::note_poll(&st0, st, &ctx, &sid, prompt, turn_ended)?;
                        std::sync::Arc::new(ctx)
                    }
```

The suggestion needs the session's page through the agent's gate (`clax_http::surface::page`, async), so it is read in the answer branch (`if !items.is_empty() || … {`, line 401), only when the poll hands something over or is a `prompt=true` poll, not on every take. Before `let text = poll_text(…)`:

```rust
            let delivered = !items.is_empty() || !answers.is_empty();
            let sugg = if delivered || prompt {
                let page = clax_http::surface::page(&s, &a, &take.session_id).await?;
                if delivered || page.is_some() {
                    let (st1, sid1) = (s.clone(), take.session_id.clone());
                    Some(
                        s.store_call(move |st| {
                            clax_http::surface::suggestion(&st1, st, &sid1, page.as_deref())
                        })
                        .await?,
                    )
                } else {
                    None
                }
            } else {
                None
            };
```

Then:

```rust
            let text = poll_text(&items, &placeholders, &late);
            let text = match (text, sugg.as_ref().map(clax_core::surface::line)) {
                (Some(t), Some(l)) => Some(format!("{t}\n{l}")),
                (None, Some(l)) => Some(l),
                (t, None) => t,
            };
```

before the `note` merge, and `if let Some(a) = &sugg { out["attention"] = json!(a); }` after `out` is built. The stop hook blocks only on a non-empty `text`, and the line is added to a stop poll only beside a delivery, which blocks it already.

In `late_answers`, at the top of the loop:

```rust
        // An answer, or a pull, came from Clax: the work it starts is Clax's,
        // at its thread (spec 2026-10-16 §1).
        if let Some(aid) = &q.artifact_id {
            st.note_delivery(sid, aid, q.thread_id.as_deref())?;
        }
```

and before the `head` match:

```rust
        if q.pulled_at.is_some() {
            views.push(clax_http::views::question::view(st, q)?);
            blocks.push(pulled_block(q, header));
            continue;
        }
```

(moving the existing `views.push` after it), with:

```rust
/// The block telling a session that the person wants a notice's question
/// asked in Clax ("Ask me here", spec 2026-10-16-ask-where-attention-is-design §3).
fn pulled_block(q: &clax_store::questions::QuestionRow, header: &str) -> String {
    let at = match (&q.artifact_id, &q.thread_id) {
        (Some(a), Some(t)) => format!(" with `url_or_id` \"{a}\" and `thread_id` \"{t}\""),
        (Some(a), None) => format!(" with `url_or_id` \"{a}\""),
        _ => String::new(),
    };
    let dialog = if q.source == clax_store::questions::Source::Hook {
        " The terminal's dialog for it closes when they press Esc there."
    } else {
        ""
    };
    format!(
        "[clax] The person wants your question {} ({}) asked in Clax instead: ask it now with `ask`{at}, whatever `attention` suggests.{dialog}",
        quoted(header),
        q.id
    )
}
```

Update `poll`'s doc comment: "`prompt=true` (Claude Code's prompt hook) records a typed prompt first, withdrawing the session's open hook notices; a `stop_hook` poll of a Codex or Grok session records the turn, which stands for a prompt when nothing came from Clax since the last ([`clax_http::surface::note_poll`]). A poll that hands anything over, or a `prompt=true` poll of a session with a page, carries the session's suggestion as `attention` and ends `text` with it in one line. A pulled notice is handed over as a late answer is, with a block asking the agent to ask it in Clax."

In `docs/contract.md`, the feedback section ("## Comments and feedback", line 1277): add `prompt=true`, the inferred turns, `attention`, and the pulled block's text.

- [ ] **Step 4: Run the tests to see them pass**

Run: `bazelisk test //crates/clax-server:all --test_arg=api_where_to_ask:: --test_arg=api_feedback:: --test_arg=api_questions:: --test_arg=api_push:: --test_arg=api_linked::`
Expected: PASS.

Run: `just check-wasm`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/clax-server/src/routes/feedback.rs crates/clax-http/src/surface.rs crates/clax-server/tests/api_where_to_ask.rs docs/contract.md
git -c commit.gpgsign=false commit -m "Read where the person is from feedback polls: the prompt hook's typed prompt, Codex and Grok turns with nothing from Clax, answers and Ask me here as deliveries; deliveries end with where to ask"
```

---

## Task 8: Claude Code hooks: the prompt hook reports the prompt

**Files:**
- Modify: `crates/clax-hooks/src/events.rs:536-547` (`prompt`; `feedback_text` at :282 and `live_session` at :261 keep their signatures), tests (`prompt_adds_context`, line 1025)
- Modify: `crates/clax-hooks/src/ask.rs:1-8` (module comment: the daemon's mode now follows the suggestion), tests (the helpers `Fake::new`, `sessions()`, `input()`, `ask_logged`, `Budget::default()` and `const Q`, line 236, exist)

**Interfaces:**
- Consumes: the feedback poll's `prompt=true` (Task 7); `POST /api/sessions/{id}/questions` `mode` (Task 6).
- Produces: `events::prompt` requests `tier=prompt_hook&prompt=true`; `ask` keeps returning no output for `mode: "terminal"`, now also for an open notice.

- [ ] **Step 1: Write the failing tests**

In `crates/clax-hooks/src/events.rs` tests, change `prompt_adds_context`'s path check to the new query:

```rust
    #[test]
    fn prompt_adds_context_and_reports_the_prompt() {
        let d = fake(Some("P"));
        assert_eq!(
            prompt("claude", &HookInput::parse(r#"{"session_id":"s1"}"#), &d).unwrap(),
            HookOutput::additional_context("UserPromptSubmit", "P")
        );
        assert!(
            d.seen
                .borrow()
                .iter()
                .any(|p| p == "/api/sessions/S/feedback?tier=prompt_hook&prompt=true"),
            "{:?}",
            d.seen.borrow()
        );
    }
```

(replacing `prompt_adds_context`.)

In `crates/clax-hooks/src/ask.rs` tests, add:

```rust
    #[test]
    fn a_notice_in_the_terminal_shows_the_dialog_at_once() {
        let d = Fake::new(vec![
            sessions(),
            (
                "POST",
                "/api/sessions/S1/questions".into(),
                Ok(json!({"question": {"id": Q, "status": "open", "notice": true}, "mode": "terminal",
                    "terminal_after_s": 600, "attention": {"surface": "terminal"}})),
            ),
        ]);
        let a = ask_logged(&input(), &d, Budget::default());
        assert_eq!((a.outcome, a.mode), (Outcome::Terminal, Some("terminal")));
        assert_eq!(a.out, HookOutput::none(), "no output: the terminal dialog appears");
        assert_eq!(d.seen.borrow().len(), 2, "no poll, no release");
    }
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `bazelisk test //crates/clax-hooks:clax_hooks_lib_test //crates/clax-hooks:golden`
Expected: `prompt_adds_context_and_reports_the_prompt` fails (the path ends `tier=prompt_hook`); the ask test passes already (the hook needs no change for a notice), which pins the contract.

- [ ] **Step 3: Write the implementation**

In `events::prompt`:

```rust
/// Tier 3. Reports the person's prompt (it is terminal activity, spec
/// 2026-10-16-ask-where-attention-is-design §1) and adds pending feedback,
/// and where to ask, to the prompt as additional context, in one request.
///
/// # Errors
/// When the input has no `session_id` or a daemon request fails.
pub fn prompt(harness: &str, input: &HookInput, daemon: &dyn Daemon) -> anyhow::Result<HookOutput> {
    let Some(sid) = live_session(harness, input, daemon)? else {
        return Ok(HookOutput::none());
    };
    Ok(match feedback_text(daemon, &sid, "tier=prompt_hook&prompt=true")? {
        Some(text) => HookOutput::additional_context("UserPromptSubmit", &text),
        None => HookOutput::none(),
    })
}
```

In `ask.rs`, replace the module comment's first sentence's condition with: "`PreToolUse` mirrors the call and, when the daemon's suggestion of where the person is names Clax (spec 2026-10-16-ask-where-attention-is-design §3), waits for the person's answer there; when it names the terminal, the daemon records a notice in Clax and the hook prints nothing, so the terminal dialog appears at once."

- [ ] **Step 4: Run the tests to see them pass**

Run: `bazelisk test //crates/clax-hooks:clax_hooks_lib_test //crates/clax-hooks:golden //scripts:comment_loop`
Expected: PASS; the smoke run's prompt-hook tier still hands over its comment.

- [ ] **Step 5: Commit**

```bash
git add crates/clax-hooks/src/events.rs crates/clax-hooks/src/ask.rs
git -c commit.gpgsign=false commit -m "Report each Claude Code prompt as terminal activity on the prompt hook's feedback poll"
```

---
## Task 9: `ask` in MCP and Pi: `thread_id`, `notice`, `attention`, `tell_terminal`, the narrowest `url`

**Files:**
- Modify: `crates/clax-mcp/src/tools.rs:296-320` (`AskArgs`, which also has `approval` and `approval_id`), `:2297-2381` (`do_ask`; it hands an approval to `do_ask_approval` first, line 2298), `:2472-2516` (`ask_result`), `:3215-3226` (the `ask` tool description, line 3216), tests (`surfaces_are_capped_oldest_first_and_forgotten_on_handover`, line 3575; add one)
- Modify: `crates/clax-mcp/src/client.rs:1627-1643` (`ask_create`)
- Modify: `plugins/pi/src/clax.ts:160-175` (`AskArgs`), `:1561-1603` (`ask`, which hands an approval to `askApproval` first), `:1677-1693` (`askResult`), `:1997-2000` (the description)
- Modify: `plugins/pi/src/client.ts:792-796` (`askCreate`)
- Modify: `plugins/pi/test/fixtures/contract.json` (the `ask` tool's description)
- Modify: `docs/contract.md` "### `ask`" (line 2729, under "## Agent questions" at 2518)
- Test: `plugins/pi/test/clax.test.ts` (after the test at line 1098; its helpers `browserThread` :593, `askBody` :659, `sessionOf` :628, `callToolAsPi` and `api` from `./daemon-fixture.ts` exist)

**Interfaces:**
- Consumes: `POST /api/sessions/{id}/questions` `{thread_id?, notice?}` and its `attention`; the poll's `attention`; the view's `thread`, `notice` (Task 6).
- Produces:
  - MCP `AskArgs { …, pub thread_id: Option<String>, pub notice: Option<bool> }`; Pi `AskArgs` the same.
  - `DaemonClient::ask_create(&self, questions: &[Question], artifact: Option<&str>, thread: Option<&str>, notice: bool) -> Result<Value>`; Pi `askCreate(questions, artifactId?, threadId?, notice?)`.
  - `ClaxTools::ask_result(&self, q: &Value, waited_s: u64, attention: &Value) -> Value`, with `url` the narrowest place, `tell_terminal` (`"Asked in Clax: <url>"`) while a question waits in Clax, `attention` when the daemon gave one, and for a notice `notice: true`, `call_again: false`.
  - `ask {notice: true, questions, url_or_id?, thread_id?}` returns at once, without waiting.

- [ ] **Step 1: Write the failing tests**

In `crates/clax-mcp/src/tools.rs` tests, change both `t.ask_result(&json!(…), 0)` calls in `surfaces_are_capped_oldest_first_and_forgotten_on_handover` to pass `&Value::Null` as a third argument, and add:

```rust
    #[test]
    fn an_ask_result_names_the_narrowest_place_and_what_to_tell_the_terminal() {
        let t = ClaxTools::new(
            DaemonClient::new("http://127.0.0.1:9".into(), "t".into(), None),
            "http://localhost:9".into(),
            None,
            PathBuf::from("log"),
        );
        let id = "01K6AB3Q9X7N2M4P5R6S8T0001";
        let att = json!({"surface": "clax", "where": {"thread_id": "t1", "artifact_id": "a1"},
            "confidence": "clear", "why": "w"});
        let r = t.ask_result(
            &json!({"id": id, "status": "open", "notice": false, "artifact": {"id": "a1"},
                "thread": {"id": "t1", "open": true}}),
            0,
            &att,
        );
        assert_eq!(r["url"], "http://localhost:9/a/a1?thread=t1");
        assert_eq!(r["tell_terminal"], "Asked in Clax: http://localhost:9/a/a1?thread=t1");
        assert_eq!(r["attention"], att);
        let r = t.ask_result(
            &json!({"id": id, "status": "open", "notice": true, "artifact": null, "thread": null}),
            0,
            &Value::Null,
        );
        assert_eq!(r["url"], format!("http://localhost:9/inbox?q={id}"));
        assert_eq!((r["notice"].as_bool(), r["call_again"].as_bool()), (Some(true), Some(false)));
        assert!(r.get("tell_terminal").is_none() && r.get("attention").is_none());
        let r = t.ask_result(
            &json!({"id": id, "status": "answered", "artifact": {"id": "a1"}, "thread": null,
                "questions": [], "answers": []}),
            0,
            &Value::Null,
        );
        assert_eq!(r["url"], "http://localhost:9/a/a1");
        assert!(r.get("tell_terminal").is_none(), "only while it waits in Clax");
    }
```

In `plugins/pi/test/clax.test.ts`, after "clax_ask cancels, refuses another session's question, and checks its arguments":

```ts
  it("clax_ask places a question in its thread, says what to tell the terminal, and posts a notice without waiting", async () => {
    const { pi, ctx } = load(daemon.home, "pi-thread-ask");
    await pi.emit("session_start", { type: "session_start", reason: "startup" }, ctx);
    const aid = parts(await pi.callTool("clax_publish", { html: "<h2>Goals</h2>", title: "Thread ask" }, ctx)).json.artifact_id;
    const tid = await browserThread(aid, "Which?");
    const r = parts(await pi.callToolAsPi("clax_ask", { questions: askBody("Pick").questions, url_or_id: aid, thread_id: tid, timeout_s: 1 }, ctx)).json;
    expect(r.url).toMatch(new RegExp(`/a/${aid}\\?thread=${tid}$`));
    expect(r.tell_terminal).toBe(`Asked in Clax: ${r.url}`);
    expect(["clax", "terminal"]).toContain(r.attention.surface);
    const n = parts(await pi.callToolAsPi("clax_ask", { questions: [{ question: "Which layout?", header: "Layout" }], notice: true }, ctx)).json;
    expect(n).toMatchObject({ notice: true, status: "open", call_again: false });
    expect(n.tell_terminal).toBeUndefined();
    const bad = await pi.callTool("clax_ask", { questions: askBody("Pick").questions, thread_id: tid }, ctx);
    expect(bad.isError).toBe(true);
    expect(JSON.parse(bad.content[0].text!).error.code).toBe("invalid_args");
    const late = await pi.callTool("clax_ask", { question_id: n.question_id, notice: true }, ctx);
    expect(JSON.parse(late.content[0].text!).error.code).toBe("invalid_args");
  });
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `bazelisk test //crates/clax-mcp:clax_mcp_lib_test --test_arg=tools::tests:: && bazelisk test //plugins/pi:test --test_arg=-t --test_arg="clax_ask places"`
Expected: Rust compile error (`ask_result` takes 2 arguments); Pi fails (`thread_id` is an unknown argument).

- [ ] **Step 3: Write the implementation**

`crates/clax-mcp/src/tools.rs`, `AskArgs` gains:

```rust
    /// A thread of `url_or_id`'s artifact the question arose from: it shows in that thread.
    pub thread_id: Option<String>,
    /// Ask nothing: show the person in Clax that you are asking this in chat.
    /// Returns at once; cancel it (`question_id`, `cancel`) once they answer in chat.
    pub notice: Option<bool>,
```

In `do_ask`, refuse the new arguments beside an approval before handing it over (an approval names its own thread, `approval.thread_id`):

```rust
        if a.approval.is_some() || a.approval_id.is_some() {
            if a.thread_id.is_some() || a.notice.is_some() {
                return Err(invalid(
                    "thread_id and notice go with `questions`; an approval's thread is `approval.thread_id`",
                ));
            }
            return self.do_ask_approval(a).await;
        }
```

and after the existing argument checks:

```rust
        let notice = a.notice.unwrap_or(false);
        if a.thread_id.is_some() && (a.url_or_id.is_none() || a.questions.is_none()) {
            return Err(invalid("thread_id goes with `questions` and `url_or_id`"));
        }
        if notice && a.questions.is_none() {
            return Err(invalid("notice goes with `questions`"));
        }
```

the create:

```rust
                let res = self
                    .client
                    .ask_create(&qs, artifact.as_deref(), a.thread_id.as_deref(), notice)
                    .await
                    .map_err(|e| self.fail(e))?;
                let qid = res["question"]["id"].as_str().unwrap_or_default().to_string();
                self.remember_surface(&qid, res["surface_open"].as_bool().unwrap_or(true));
                if notice {
                    return Ok(self.ask_result(&res["question"], 0, &res["attention"]));
                }
```

(`a.questions` is matched by value below: bind `let thread = a.thread_id.clone();` before the `match (a.questions, a.question_id)` and use `thread.as_deref()`), the cancel paths pass `&Value::Null`, and the end:

```rust
        Ok(self.ask_result(&v["question"], v["waited_s"].as_u64().unwrap_or(0), &v["attention"]))
```

Replace `ask_result`:

```rust
    /// Where the person finds question view `q` in Clax: its thread, else
    /// its artifact, else its place in the inbox.
    fn place_url(&self, q: &Value) -> String {
        let base = self.browser_base();
        match (q["artifact"]["id"].as_str(), q["thread"]["id"].as_str()) {
            (Some(a), Some(t)) => format!("{base}/a/{a}?thread={t}"),
            (Some(a), None) => format!("{base}/a/{a}"),
            _ => format!("{base}/inbox?q={}", q["id"].as_str().unwrap_or_default()),
        }
    }

    /// The `ask` result (spec §6.5; 2026-10-16-ask-where-attention-is §2)
    /// for the question view `q`: in `reply`, each of its answers beside its
    /// question and header (null unless it was answered); `url`, the
    /// narrowest place it shows; while it waits in Clax, `call_again`,
    /// `surface_open` and `tell_terminal`, the line to write in chat; the
    /// daemon's suggestion `attention`, when given. A notice is never
    /// waited on (`call_again` false). `answers` is left to the late answers
    /// to other questions, as on every tool result.
    fn ask_result(&self, q: &Value, waited_s: u64, attention: &Value) -> Value {
```

keeping its body, with `"url": self.place_url(q),` and before the `surfaces` lock:

```rust
        let notice = q["notice"].as_bool().unwrap_or(false);
        if notice {
            out["notice"] = json!(true);
            out["call_again"] = json!(false);
        } else if status == "open" {
            out["tell_terminal"] = json!(format!("Asked in Clax: {}", out["url"].as_str().unwrap_or_default()));
        }
        if !attention.is_null() {
            out["attention"] = attention.clone();
        }
```

Update the `ask` tool's description (and, word for word, the fixture's and Pi's), keeping its approval sentence:

"Ask the person one to four questions in Clax and wait for the answers (up to `timeout_s`, default 600 s). Each question has a short `header` (at most 12 characters) and two to four `options` (`label`, optional `description`, `preview` text, `recommended`) or none for a free-text answer; `multi_select` allows several; the person may also type an \"Other\" answer. Pass `url_or_id` when the question is about a page, and `thread_id` when it arose in one of its threads, so it shows there. With `notice: true` it asks nothing: it shows the person in Clax that you are asking in chat, returns at once, and you cancel it once they answer. If the result says `call_again`, call `ask` again with `question_id`. The answers come back in `reply`, in the person's own words; write the result's `tell_terminal` line in chat. To ask your person to let you see content withheld from you, pass `approval` (`url_or_id`, optional `thread_id`, and `author`, a public ID from the `withheld` summary's `author_ids`) instead of `questions`: the result's `status` is `approved`, `declined` or `open` (then call again with `approval_id`)."

`crates/clax-mcp/src/client.rs`:

```rust
    /// `POST /api/sessions/<sid>/questions` with `source: "ask"`: `{question,
    /// mode, terminal_after_s, surface_open, attention}`. `thread` is a
    /// thread of `artifact`; `notice` records a notice instead of a question.
    pub async fn ask_create(
        &self,
        questions: &[clax_core::questions::Question],
        artifact: Option<&str>,
        thread: Option<&str>,
        notice: bool,
    ) -> Result<Value> {
        let body = json!({"source": "ask", "questions": questions, "artifact_id": artifact,
            "thread_id": thread, "notice": notice});
```

(rest unchanged).

`plugins/pi/src/clax.ts` `AskArgs` gains:

```ts
  thread_id: opt(str("A thread of `url_or_id`'s artifact the question arose from: it shows in that thread.")),
  notice: opt(Type.Boolean({ description: "Ask nothing: show the person in Clax that you are asking this in chat. Returns at once; cancel it (`question_id`, `cancel`) once they answer in chat." })),
```

In `ask`, the first line (the approval hand-off) becomes:

```ts
    if (a.approval !== undefined || a.approval_id !== undefined) {
      if (a.thread_id !== undefined || a.notice !== undefined) throw invalid("thread_id and notice go with `questions`; an approval's thread is `approval.thread_id`");
      return this.askApproval(ctx, a);
    }
```

and after the existing checks:

```ts
    if (a.thread_id !== undefined && (a.url_or_id === undefined || a.questions === undefined)) throw invalid("thread_id goes with `questions` and `url_or_id`");
    if (a.notice && a.questions === undefined) throw invalid("notice goes with `questions`");
```

the create:

```ts
      const res = await this.call(() => c.askCreate(a.questions!, artifact, a.thread_id, a.notice ?? false));
      qid = String(res.question?.id ?? "");
      this.rememberSurface(qid, res.surface_open ?? true);
      if (a.notice) return this.askResult(c, res.question, 0, res.attention);
```

the end `return this.askResult(c, r.question, Number(r.waited_s ?? 0), r.attention);` (the cancel paths pass no fourth argument), and `askResult`:

```ts
  /** Where the person finds question view `q` in Clax: its thread, else its artifact, else the inbox. */
  private placeUrl(c: DaemonClient, q: Json): string {
    const base = this.browserBase(c);
    const a = q?.artifact?.id, t = q?.thread?.id;
    return a && t ? `${base}/a/${a}?thread=${t}` : a ? `${base}/a/${a}` : `${base}/inbox?q=${String(q?.id ?? "")}`;
  }

  private askResult(c: DaemonClient, q: Json, waitedS: number, attention?: Json): Json {
    …
    const out: Json = {
      question_id: qid, status, reply,
      url: this.placeUrl(c, q),
      waited_s: waitedS, call_again: status === "open" && !q?.notice, note: ASK_NOTE,
    };
    if (q?.notice) out.notice = true;
    else if (status === "open") out.tell_terminal = `Asked in Clax: ${out.url}`;
    if (attention) out.attention = attention;
    …
  }
```

`plugins/pi/src/client.ts`:

```ts
  /** `POST /api/sessions/<sid>/questions` with `source: "ask"`: `{question,
   * mode, terminal_after_s, surface_open, attention}`; `threadId` is a thread
   * of `artifactId`, `notice` records a notice instead. */
  askCreate(questions: unknown[], artifactId?: string, threadId?: string, notice = false): Promise<any> {
    return this.json(() => `${this.sessionPath()}/questions`, this.jsonBody("POST", { source: "ask", questions, artifact_id: artifactId ?? null, thread_id: threadId ?? null, notice }));
  }
```

In `docs/contract.md` "### `ask`": add the `thread_id` and `notice` rows to the argument table, `tell_terminal` and `attention` to the result, and say `url` is the narrowest place.

- [ ] **Step 4: Run the tests to see them pass**

Run: `bazelisk test //crates/clax-mcp:all && just pi-test && scripts/test-plugins.sh`
Expected: PASS, `tools_match_the_fixture` and the descriptions check in `test-plugins.sh` included.

- [ ] **Step 5: Commit**

```bash
git add crates/clax-mcp/src/tools.rs crates/clax-mcp/src/client.rs plugins/pi/src/clax.ts plugins/pi/src/client.ts plugins/pi/test/clax.test.ts plugins/pi/test/fixtures/contract.json docs/contract.md
git -c commit.gpgsign=false commit -m "Let ask place a question in its thread or post a notice, and return the narrowest URL, the line to tell the terminal and the suggestion of where to ask"
```

---

## Task 10: The `attention` tool in MCP and Pi

**Files:**
- Modify: `crates/clax-mcp/src/tools.rs:1` (count), args structs (after `AskArgs`), the tool router (after `ask`, line 3226), a `do_attention` beside `do_ask`, the test `tool_count_is_29` (`:3536-3545`, which becomes `tool_count_is_30`)
- Modify: `crates/clax-mcp/src/client.rs` (after `ask_withdraw`, line 1673)
- Modify: `plugins/pi/src/clax.ts:3` (count), args (after `AskArgs`), tools (after `ask`), registration (after `define("ask", …)`, :1997-2000)
- Modify: `plugins/pi/src/client.ts` (after `askWithdraw`, :807-809)
- Modify: `plugins/pi/test/fixtures/contract.json` (`tools`: add `attention`)
- Modify (the count, and `attention` in each tool list): `plugins/claude-code/README.md:115`, `plugins/clax/README.md:80`, `plugins/clax-grok/README.md:47`, `plugins/pi/README.md:35`, `docs/contract.md:17,45`, `docs/usage.md:285`, `scripts/test-plugins.sh:222,234-235,239`
- Regenerate: the skills' tool blocks (`scripts/sync-skill-tools.py`)
- Test: `plugins/pi/test/clax.test.ts:114` and a new case

The count is already twenty-nine (`fork`, `publish_public`, `comments_handoff`, `draft` and `git_remote` landed since this plan was first written), so `attention` makes thirty.

**Interfaces:**
- Consumes: `GET /api/sessions/{id}/attention` (Task 5).
- Produces: MCP tool `attention` (no arguments) → the suggestion `{surface, where, confidence, why}`; Pi `clax_attention`; `DaemonClient::attention(&self) -> Result<Value>`; Pi `DaemonClient.attention(): Promise<any>`.

- [ ] **Step 1: Write the failing tests**

Add the tool to `plugins/pi/test/fixtures/contract.json`'s `tools` (after `ask`):

```json
    {
      "name": "attention",
      "description": "Where to ask the person now: `surface` (`clax` or `terminal`), `where` (the narrowest place in Clax: `thread_id`, `artifact_id`), `confidence` (`clear` or `unsure`) and `why`. Read it before you ask anything; ask only on the surface it names, at the place it names, and tell the other surface (in chat, the `tell_terminal` line `ask` returns; in Clax, `ask` with `notice: true`). When you know better (the person said where they are), follow that and say why.",
      "annotations": {"readOnlyHint": true, "openWorldHint": false}
    },
```

In `plugins/pi/test/clax.test.ts`, rename "registers the twenty-nine tools with one-line prompt snippets, and the clax command" to "registers the thirty tools …" (its list comes from the fixture), and add:

```ts
  it("clax_attention says where to ask, as the daemon suggests it", async () => {
    const { pi, ctx } = load(daemon.home, "pi-attention");
    await pi.emit("session_start", { type: "session_start", reason: "startup" }, ctx);
    const r = parts(await pi.callTool("clax_attention", {}, ctx)).json;
    expect(r).toMatchObject({ surface: "terminal", confidence: "clear" });
    expect(r.why).toContain("No Clax tab");
  });
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `bazelisk test //crates/clax-mcp:clax_mcp_lib_test --test_arg=tools::tests::tools_match_the_fixture && bazelisk test //plugins/pi:test --test_arg=-t --test_arg="attention|thirty"`
Expected: FAIL: the fixture lists a tool neither side has.

- [ ] **Step 3: Write the implementation**

`crates/clax-mcp/src/tools.rs`: the module comment says thirty tools; `tool_count_is_29` becomes `tool_count_is_30`, asserting 30, its doc comment saying "Thirty tools"; add

```rust
#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AttentionArgs {}
```

beside `do_ask`:

```rust
    async fn do_attention(&self) -> Outcome {
        self.require_session().await?;
        let r = self.client.attention().await.map_err(|e| self.fail(e))?;
        Ok(r["attention"].clone())
    }
```

and after the `ask` tool:

```rust
    #[tool(
        description = "Where to ask the person now: `surface` (`clax` or `terminal`), `where` (the narrowest place in Clax: `thread_id`, `artifact_id`), `confidence` (`clear` or `unsure`) and `why`. Read it before you ask anything; ask only on the surface it names, at the place it names, and tell the other surface (in chat, the `tell_terminal` line `ask` returns; in Clax, `ask` with `notice: true`). When you know better (the person said where they are), follow that and say why.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    pub async fn attention(
        &self,
        Parameters(_a): Parameters<AttentionArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.finish(self.do_attention().await).await
    }
```

`crates/clax-mcp/src/client.rs`:

```rust
    /// `GET /api/sessions/<sid>/attention`: `{attention, signals}`.
    pub async fn attention(&self) -> Result<Value> {
        self.json(|c| {
            c.request(
                reqwest::Method::GET,
                &format!("{}/attention", c.session_path()),
            )
        })
        .await
    }
```

`plugins/pi/src/client.ts`:

```ts
  /** `GET /api/sessions/<sid>/attention`: `{attention, signals}`. */
  attention(): Promise<any> {
    return this.json(() => `${this.sessionPath()}/attention`, { method: "GET" });
  }
```

`plugins/pi/src/clax.ts`: the header comment says thirty tools; `const AttentionArgs = Type.Object({}, strict);`; in the tools class:

```ts
  /** Where to ask the person now: the daemon's suggestion. */
  async attention(ctx: ExtensionContext): Promise<Json> {
    const c = this.clientFor(ctx);
    return (await this.call(() => c.attention())).attention;
  }
```

and after `define("ask", …)`:

```ts
    define("attention", "Clax attention",
      "Where to ask the person now: `surface` (`clax` or `terminal`), `where` (the narrowest place in Clax: `thread_id`, `artifact_id`), `confidence` (`clear` or `unsure`) and `why`. Read it before you ask anything; ask only on the surface it names, at the place it names, and tell the other surface (in chat, the `tell_terminal` line `ask` returns; in Clax, `ask` with `notice: true`). When you know better (the person said where they are), follow that and say why.",
      "Read where to ask the person now: in Clax or in the terminal",
      AttentionArgs, ctx => tools.attention(ctx));
```

Change "twenty-nine" to "thirty" and add `attention` after `ask` in the tool lists of `plugins/claude-code/README.md`, `plugins/clax/README.md`, `plugins/clax-grok/README.md`, `plugins/pi/README.md:35` ("Twenty-nine tools:"), `docs/contract.md` (lines 17 and 45) and `docs/usage.md:285`, and in `scripts/test-plugins.sh` the comments and messages at 222 and 239 and the hard-coded `len(tools) != 29` / "not 29" at 234-235. Then run `scripts/sync-skill-tools.py` to regenerate the four skills' tool blocks.

- [ ] **Step 4: Run the tests to see them pass**

Run: `bazelisk test //crates/clax-mcp:all && just pi-test && scripts/sync-skill-tools.py --check && scripts/test-plugins.sh`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/clax-mcp/src/tools.rs crates/clax-mcp/src/client.rs plugins/pi/src/clax.ts plugins/pi/src/client.ts plugins/pi/test/clax.test.ts plugins/pi/test/fixtures/contract.json plugins/*/README.md plugins/*/skills/clax/SKILL.md docs/contract.md docs/usage.md scripts/test-plugins.sh
git -c commit.gpgsign=false commit -m "Add the attention tool, in MCP and Pi, that says where to ask the person now"
```

---

## Task 11: Pi records a typed prompt as terminal activity

**Files:**
- Modify: `plugins/pi/src/clax.ts:1848-1859` (beside the `tool_call` handler, line 1848, and the `agent_end` handler, lines 1854-1859)
- Modify: `plugins/pi/src/client.ts` (after `attention`)
- Test: `plugins/pi/test/clax.test.ts`

**Interfaces:**
- Consumes: Pi's `input` event (`InputEvent { text, source: "interactive" | "rpc" | "extension" }`, `plugins/pi/node_modules/@mariozechner/pi-coding-agent/dist/core/extensions/types.d.ts:564-585`, Pi 0.73.1; `node_modules` exists only after `just pi-test-native` or a pnpm install); `POST /api/sessions/{id}/terminal` (Task 5).
- Produces: `DaemonClient.noteTerminal(): Promise<any>`; an `input` handler that records every prompt not sent by an extension (Clax's own deliveries arrive as `source: "extension"`) and returns `{ action: "continue" }`.

- [ ] **Step 1: Write the failing test**

```ts
  it("a prompt typed in Pi is terminal activity, and a message an extension sent is not", async () => {
    const { pi, ctx } = load(daemon.home, "pi-input");
    await pi.emit("session_start", { type: "session_start", reason: "startup" }, ctx);
    const sid = await sessionOf("pi-input");
    const signals = async () => (await api(daemon, `/api/sessions/${sid}/attention`)).signals;
    await pi.emit("input", { type: "input", text: "[clax] a comment", source: "extension" }, ctx);
    await pi.emit("input", { type: "input", text: "make it blue", source: "interactive" }, ctx);
    await expect.poll(async () => (await signals()).terminal_at, { timeout: 10_000 }).not.toBeNull();
    expect((await signals()).terminal_inferred).toBe(false);
  });
```

- [ ] **Step 2: Run the test to see it fail**

Run: `bazelisk test //plugins/pi:test --test_arg=-t --test_arg="a prompt typed in Pi"`
Expected: FAIL: `terminal_at` stays null.

- [ ] **Step 3: Write the implementation**

`plugins/pi/src/client.ts`:

```ts
  /** `POST /api/sessions/<sid>/terminal`: the person typed a prompt (spec
   * 2026-10-16-ask-where-attention-is-design §1). Only discovers a running
   * daemon, never starts one. */
  noteTerminal(): Promise<any> {
    return this.json(() => `${this.sessionPath()}/terminal`, this.jsonBody("POST", {}), this.discoverFn);
  }
```

(The third argument is the one `renewWorking` and `endWorking` pass, `client.ts:737,742`.)

`plugins/pi/src/clax.ts`, after the `agent_end` handler:

```ts
    // Terminal activity (spec 2026-10-16-ask-where-attention-is-design §1):
    // a prompt the person typed, not a message an extension sent (Clax's
    // deliveries are), tells the daemon where they are. Neither starts a
    // daemon, registers a session, nor delays the prompt.
    pi.on("input", async event => {
      if (event.source !== "extension") {
        const c = tools.existingClient();
        if (c?.session()) void c.noteTerminal().catch(() => undefined);
      }
      return { action: "continue" };
    });
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `just pi-test`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add plugins/pi/src/clax.ts plugins/pi/src/client.ts plugins/pi/test/clax.test.ts
git -c commit.gpgsign=false commit -m "Record a prompt typed in Pi as terminal activity"
```

---

## Task 12: The shell's presence reports carry focus and idle time

**Files:**
- Modify: `web/shell/src/view/presence-reporter.ts:1-104` (`lastInput` :17, `reported` :19, `leave` :28-33, the constructor :37-42, `report` :51-68, `putPresence` :84)
- Create: `web/shell/src/view/presence-reporter.test.ts`

**Interfaces:**
- Consumes: `PUT /api/viewers/me/presence` `{focused, idle_ms}` (Task 3).
- Produces: `putPresence(aid: string, state: "here" | "away", where: string | null, tab: string, keepalive = false, engaged?: { focused: boolean; idle_ms: number }): Promise<Presence | null>`; the reporter reports at once when the tab gains or loses focus.

- [ ] **Step 1: Write the failing tests**

Create `web/shell/src/view/presence-reporter.test.ts`:

```ts
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ViewState } from "./artifact-controller";
import { PresenceReporter } from "./presence-reporter";

const AID = "7q3k9mzx2b4t";
let bodies: Record<string, unknown>[] = [];

function stub(focused: boolean) {
  bodies = [];
  vi.stubGlobal("fetch", vi.fn(async (_u: string, init?: RequestInit) => {
    if (init?.body) bodies.push(JSON.parse(String(init.body)));
    return new Response(JSON.stringify({ people: [], viewing: 1 }));
  }));
  vi.spyOn(document, "visibilityState", "get").mockReturnValue("visible");
  return vi.spyOn(document, "hasFocus").mockReturnValue(focused);
}
const reporter = () => new PresenceReporter(AID, () => ({ shareWhere: false }) as unknown as ViewState, () => true, () => {});

afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); });

describe("PresenceReporter", () => {
  it("reports focus and idle time, never a timestamp", async () => {
    stub(true);
    const r = reporter();
    r.report(true);
    await vi.waitFor(() => expect(bodies.length).toBe(1));
    expect(bodies[0]).toMatchObject({ artifact_id: AID, state: "here", focused: true });
    expect(typeof bodies[0].idle_ms).toBe("number");
    expect(Object.keys(bodies[0]).sort()).toEqual(["artifact_id", "focused", "idle_ms", "state", "tab"]);
    r.dispose();
  });

  it("reports at once when the tab gains or loses focus", async () => {
    const focus = stub(true);
    const r = reporter();
    r.report(true);
    await vi.waitFor(() => expect(bodies.length).toBe(1));
    focus.mockReturnValue(false);
    dispatchEvent(new Event("blur"));
    await vi.waitFor(() => expect(bodies.length).toBe(2));
    expect(bodies[1].focused).toBe(false);
    focus.mockReturnValue(true);
    dispatchEvent(new Event("focus"));
    await vi.waitFor(() => expect(bodies.length).toBe(3));
    expect(bodies[2].focused).toBe(true);
    r.dispose();
  });
});
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `bazelisk test //web:unit --test_arg=shell/src/view/presence-reporter.test.ts`
Expected: FAIL: the body has no `focused`; blur sends nothing.

- [ ] **Step 3: Write the implementation**

In `web/shell/src/view/presence-reporter.ts`, add to the file comment: "Each report says whether the tab has focus and how long ago the viewer last gave input in it (never a time, so the browser's clock does not matter); the daemon keeps both for the viewer's own agents and shows them to no one. Gaining or losing focus reports at once."

Change `reported` to `{ state: "here" | "away"; where: string | null; focused: boolean; at: number } | null`, add `private readonly onFocus = () => this.report();`, register and remove it for `"focus"` and `"blur"` beside `visibilitychange` (`addEventListener("focus", this.onFocus); addEventListener("blur", this.onFocus);`), and in `report`:

```ts
    const focused = document.visibilityState === "visible" && document.hasFocus();
    const last = this.reported;
    if (!force && last && last.state === state && last.where === where && last.focused === focused) return;
    // A flip of state or focus goes out at once; anything else at most every 2 s.
    const flip = !last || last.state !== state || last.focused !== focused;
    const wait = flip ? 0 : last.at + 2000 - wall();
    …
    this.reported = { state, where, focused, at: wall() };
    void putPresence(this.id, state, where, this.tab, false, { focused, idle_ms: Math.max(0, Math.round(wall() - this.lastInput)) }).then(…);
```

`leave` sends `{ focused: false, idle_ms: … }` too, and `putPresence`:

```ts
/** Reports this viewer here or away on `aid` from tab `tab`, with where they look and, when given,
 * whether the tab has focus and how long ago they last gave input in it; answers the artifact's
 * presence, or null on failure (the next report writes again). */
export async function putPresence(aid: string, state: "here" | "away", where: string | null, tab: string, keepalive = false, engaged?: { focused: boolean; idle_ms: number }): Promise<Presence | null> {
  try {
    const r = await fetch("/api/viewers/me/presence", { method: "PUT", keepalive, headers: { "content-type": "application/json" }, body: JSON.stringify({ artifact_id: aid, state, tab, ...where && { where }, ...engaged }) });
    return r.ok ? presenceOf(await r.json()) : null;
  } catch { return null; }
}
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `just web-test`
Expected: PASS.

- [ ] **Step 5: Verify in the browser**

Run the dev loop (`just watch`, on `~/.clax-dev`, port 7481; never the owner's daemon on 7480), open an artifact, and in DevTools' Network panel check that the presence `PUT` carries `focused` and `idle_ms`, that switching to another window sends one with `focused: false` at once, and that the presence chips look as before at 1280 and 390, light and dark.

- [ ] **Step 6: Commit**

```bash
git add web/shell/src/view/presence-reporter.ts web/shell/src/view/presence-reporter.test.ts
git -c commit.gpgsign=false commit -m "Report each tab's focus and idle time with presence, at once when focus changes"
```

---
## Task 13: Shell: a thread's questions in its card, notices with Ask me here, counts and links

**Files:**
- Modify: `web/shell/src/api.ts:154-165` (`QuestionView`, a `type`, which now also has `linked?: QuestionLink | null`)
- Modify: `web/shell/src/q/fixtures.ts:10-33` (`view()` defaults)
- Modify: `web/shell/src/q/model.ts` (`inThread`, `noticeLine`, `NOTICE_ESC`, `closedLabel`, whose "withdrawn" case is at line 84)
- Modify: `web/shell/src/q/api.ts:37-40` (`pullQuestion` after line 40)
- Modify: `web/shell/src/q/feed.svelte.ts:66-99` (`byThread`, `pull`)
- Create: `web/shell/src/q/NoticeCard.svelte`, `web/shell/src/q/ThreadQuestions.svelte`, `web/shell/src/q/notice-card.test.ts`
- Modify: `web/shell/src/q/QuestionList.svelte:52-56`, `web/shell/src/q/SidebarQuestions.svelte`, `web/shell/src/q/index.ts:72-101`
- Modify: `web/shell/src/ui/ThreadCard.svelte:173` (the slot)
- Modify: `web/extension/src/messages.ts:254-258,508`, `web/extension/src/sw/panel.ts:76,102-106`, `web/extension/src/sw/api.ts:218`, `web/extension/src/panel/questions.ts:10-20` (`q-pull`)
- Test: `web/shell/src/q/surfaces.test.ts`, `web/shell/src/q/model.test.ts`

**Interfaces:**
- Consumes: the view's `thread`, `notice`, `pulled`; `POST /api/questions/{qid}/pull` (Task 6).
- Produces:
  - `QuestionView.thread: { id: string; open: boolean } | null`, `.notice: boolean`, `.pulled: boolean`
  - `inThread(q: QuestionView): string | null`; `noticeLine(q: QuestionView, others: (QAgent | null)[]): string`; `NOTICE_ESC: string`
  - `pullQuestion(qid: string): Promise<Closing | Forbidden>`; `QuestionFeed.pull(id: string): Promise<void>`; `QuestionFeed.byThread(tid: string): QuestionView[]`
  - `ThreadCard` renders `<div class="thread-q-slot" data-thread={t.id}>` beneath its comments; the question module mounts `ThreadQuestions` into each such slot in the document (a `MutationObserver`), and `SidebarQuestions` counts a thread's questions and links to their card.
  - Extension message `{ t: "q-pull"; req: number; questionId: string }`.

- [ ] **Step 1: Write the failing tests**

In `web/shell/src/q/fixtures.ts` `view()`, add `thread: null, notice: false, pulled: false,` before `...over`.

Add to `web/shell/src/q/model.test.ts`:

```ts
describe("notices and threads", () => {
  it("places a question in its thread only while the thread is open", () => {
    expect(inThread(view({ thread: { id: "t1", open: true } }))).toBe("t1");
    expect(inThread(view({ thread: { id: "t1", open: false } }))).toBeNull();
    expect(inThread(view())).toBeNull();
  });
  it("says who asks in the terminal, and what Ask me here did", () => {
    expect(noticeLine(view({ notice: true }), [])).toBe("claude is asking you in the terminal");
    expect(closedLabel(view({ notice: true, status: "withdrawn", pulled: true }))).toBe("Sent to claude: it asks here next");
    expect(closedLabel(view({ notice: true, status: "answered", answered_via: "terminal" }))).toBe("Answered in the terminal");
  });
});
```

Create `web/shell/src/q/notice-card.test.ts`:

```ts
import { beforeAll, describe, expect, it, vi } from "vitest";
import { dispatchTrusted } from "../../../bridge/test/trusted";
import { flush, mount } from "../test/svelte";
import { guardedAction, keyboardTrail } from "../view/trail";
import NoticeCard from "./NoticeCard.svelte";
import { view } from "./fixtures";
import { useTrail } from "./trail";

const click = (el: Element) => flush(() => { dispatchTrusted(el, new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 })); });

describe("NoticeCard", () => {
  beforeAll(() => useTrail({ keyboardTrail, guardedAction }));
  it("says the agent asks in the terminal, shows the headers, and offers Ask me here with the Esc note", async () => {
    const onPull = vi.fn(async () => {});
    const m = mount(NoticeCard, { q: view({ notice: true, source: "hook" }), onPull });
    expect(m.root.querySelector(".asker")!.textContent).toBe("claude is asking you in the terminal");
    expect([...m.root.querySelectorAll(".hdrs li")].map(l => l.textContent)).toEqual(["Layout", "Sidebars", "Notes"]);
    expect(m.root.querySelector("input, textarea")).toBeNull();
    expect(m.root.querySelector(".esc")!.textContent).toContain("press Esc");
    click([...m.root.querySelectorAll("button")].find(b => b.textContent === "Ask me here")!);
    expect(onPull).toHaveBeenCalledOnce();
    m.unmount();
  });
  it("has no Esc note for a question asked in chat", () => {
    const m = mount(NoticeCard, { q: view({ notice: true, source: "ask" }), onPull: vi.fn(async () => {}) });
    expect(m.root.querySelector(".esc")).toBeNull();
    m.unmount();
  });
});
```

Add to `web/shell/src/q/surfaces.test.ts`:

```ts
  it("shows a thread's question inside its card, and counts and links it from the sidebar's block", async () => {
    vi.resetModules();
    const inT = view({ id: "QT", thread: { id: "t1", open: true } });
    const loose = view({ id: "QA", created_at: "2026-10-07T09:05:00.000Z" });
    vi.stubGlobal("fetch", vi.fn(async (url: string) => new Response(JSON.stringify(url.startsWith("/api/inbox/summary") ? { unread: 2, questions: [inT, loose], latest: [] } : { questions: [inT, loose], open: 2 }))));
    document.body.innerHTML = `<div class="page"><header class="topbar"><div class="island"></div></header><aside class="sidebar"><div class="questions-slot"></div><article class="thread"><div class="thread-q-slot" data-thread="t1"></div></article></aside></div>`;
    const qmod = await import("./index");
    const stream = fakeStream();
    const stop = qmod.artifact(AID, { keyboardTrail: { onClear: () => () => {} }, guardedAction: (_e, _v, act) => { act(); return null; } }, stream as never);
    for (const f of stream.on.values()) f({ type: "ready" });
    await settle();
    expect([...document.querySelectorAll(".thread-q-slot .qcard")].map(c => (c as HTMLElement).dataset.question)).toEqual(["QT"]);
    expect([...document.querySelectorAll(".questions-slot .qcard")].map(c => (c as HTMLElement).dataset.question)).toEqual(["QA"]);
    expect(document.querySelector(".questions-slot h2")!.textContent).toBe("Questions for you 2");
    expect(document.querySelector<HTMLAnchorElement>(".questions-slot a.q-thread")!.getAttribute("href")).toBe("?thread=t1");
    // A card mounted later gets its questions too.
    const late = document.createElement("div");
    late.className = "thread-q-slot";
    late.dataset.thread = "t1";
    document.querySelector(".sidebar")!.append(late);
    await settle();
    expect(late.querySelectorAll(".qcard")).toHaveLength(1);
    stop();
    flush();
  });

  it("a question whose thread is gone or resolved is a card in the sidebar block", async () => {
    const resolved = view({ id: "QR", thread: { id: "t1", open: false } });
    const { feed } = await feedOf([resolved]);
    const aside = document.createElement("aside");
    aside.className = "sidebar";
    document.body.append(aside);
    const m = mount(SidebarQuestions, { aid: AID, feed }, aside);
    expect([...m.root.querySelectorAll<HTMLElement>(".qcard")].map(c => c.dataset.question)).toEqual(["QR"]);
    expect(m.root.querySelector("a.q-thread")).toBeNull();
    m.unmount();
  });
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `bazelisk test //web:unit --test_arg=shell/src/q/`
Expected: type errors and failures: `inThread` and `NoticeCard.svelte` do not exist; the thread slot stays empty.

- [ ] **Step 3: Write the implementation**

`web/shell/src/api.ts`, `QuestionView` gains:

```ts
  /** The thread of `artifact` it was asked in, while it is one (`open`: not resolved); null for none or a deleted one. */
  thread: { id: string; open: boolean } | null;
  /** A notice: the agent is asking in the terminal (or in chat); it takes no answer here. */
  notice: boolean;
  /** The person chose Ask me here on the notice. */
  pulled: boolean;
```

`web/shell/src/q/model.ts`:

```ts
/** The thread a question shows in, inside that thread's card: its thread
 * while it is open; null when it shows in the sidebar's block instead (no
 * thread, or a resolved or deleted one). */
export const inThread = (q: QuestionView): string | null => (q.thread?.open ? q.thread.id : null);

/** A notice's line (spec 2026-10-16-ask-where-attention-is-design §3). */
export const noticeLine = (q: QuestionView, others: (QAgent | null)[]): string => `${agentLabel(q.agent, others)} is asking you in the terminal`;

/** What Ask me here cannot do for Claude Code's terminal dialog. */
export const NOTICE_ESC = "After Ask me here, the terminal's dialog stays open until you press Esc there.";
```

and in `closedLabel`, `case "withdrawn": return q.pulled ? `Sent to ${agentLabel(q.agent, [])}: it asks here next` : `${agentLabel(q.agent, [])} stopped waiting`;`.

`web/shell/src/q/api.ts`: `/** "Ask me here" (notices only). */ export const pullQuestion = (qid: string) => closing(qid, "pull");`

`web/shell/src/q/feed.svelte.ts`: import `pullQuestion` and `inThread`; add

```ts
  /** The cards asked in thread `tid` (shown inside its card while it is open). */
  byThread(tid: string): QuestionView[] {
    return this.cards.filter(q => inThread(q) === tid);
  }
```

and `/** Ask me here: asks notice `id`'s agent to ask it here. */ pull(id: string): Promise<void> { return this.close(pullQuestion(id)); }`.

Create `web/shell/src/q/NoticeCard.svelte`:

```svelte
<svelte:options css="injected" />

<script lang="ts">
  // A notice (spec 2026-10-16-ask-where-attention-is-design §3): an agent is
  // asking the person in the terminal, or in chat. It says who and when, and
  // shows the questions' headers only; it takes no answer. **Ask me here**
  // sends the agent word to ask it in Clax instead; for Claude Code's
  // terminal dialog it says the dialog stays until Esc. Closed, it says what
  // closed it, for the list's usual moment. Its styles travel with the
  // question module's chunk; it reuses the card's (`.qcard`) frame.
  import type { QuestionView } from "../api";
  import { relativeTime } from "../format";
  import { NOTICE_ESC, closedLabel, cutHeader, noticeLine } from "./model";
  import { trail } from "./trail";

  const p: { q: QuestionView; others?: QuestionView[]; now?: Date; onPull(): Promise<void> } = $props();
  let busy = $state(false);
  let hint = $state<string | null>(null);
  let failed = $state(false);
  const open = $derived(p.q.status === "open");
  const who = $derived(noticeLine(p.q, (p.others ?? []).map(o => o.agent)));
  const pull = (e: Event) => {
    if (busy) return;
    hint = trail().guardedAction(e, "ask here", () => {
      busy = true;
      failed = false;
      p.onPull().then(() => { busy = false; }, () => { busy = false; failed = true; });
    });
  };
</script>

<article class="qcard notice" data-question={p.q.id} data-closed={open ? undefined : ""} aria-label={who} tabindex="-1">
  <header class="qhead">
    <strong class="asker">{who}</strong>
    {#if p.q.agent?.project}<span class="proj">in {p.q.agent.project}</span>{/if}
    <time datetime={p.q.created_at}>{relativeTime(p.q.created_at, p.now ?? new Date())}</time>
  </header>
  <ul class="hdrs">{#each p.q.questions as s (s.question)}<li>{cutHeader(s.header)}</li>{/each}</ul>
  {#if open}
    {#if p.q.source === "hook"}<p class="esc small muted">{NOTICE_ESC}</p>{/if}
    <div class="acts">
      <button type="button" class="primary" disabled={busy} onclick={pull}>Ask me here</button>
      {#if hint}<span class="act-hint">{hint}</span>{/if}
      {#if failed}<span class="err small" role="status">Not sent: try again</span>{/if}
    </div>
  {:else}
    <p class="closed" role="status">{closedLabel(p.q)}</p>
  {/if}
</article>

<style>
  :global {
    .qcard.notice { border-left-color: var(--agent-ink); }
    .qcard.notice .hdrs { display: flex; flex-wrap: wrap; gap: 6px; margin: 0 0 8px; padding: 0; list-style: none; }
    .qcard.notice .hdrs li { padding: 2px 8px; border: 1px solid var(--border-strong); border-radius: var(--radius-sm); font: 500 12px/16px var(--font); color: var(--muted); }
    .qcard.notice .esc { margin: 0 0 8px; }
    .qcard.notice .acts { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; }
  }
</style>
```

Create `web/shell/src/q/ThreadQuestions.svelte`:

```svelte
<script lang="ts">
  // The questions asked in one thread (spec
  // 2026-10-16-ask-where-attention-is-design §2), inside its card beneath
  // its last comment, with the same controls as anywhere. Hidden while none.
  import type { QuestionFeed } from "./feed.svelte";
  import QuestionList from "./QuestionList.svelte";
  import { shared } from "./shared";

  const p: { aid: string; tid: string; feed?: QuestionFeed; now?: Date } = $props();
  const feed = $derived(p.feed ?? shared().questions);
  const list = $derived(feed.byThread(p.tid));
</script>

<div class="thread-q" hidden={!list.length}><QuestionList {list} {feed} here={p.aid} now={p.now} /></div>

<style>
  :global { .thread-q { margin: 8px 0 4px; } }
</style>
```

`web/shell/src/q/QuestionList.svelte`, in the `{#each}`:

```svelte
  {#each p.list as q (q.id)}
    {#if q.notice}
      <NoticeCard {q} others={p.list} now={p.now} onPull={() => p.feed.pull(q.id)} />
    {:else}
      <QuestionCard {q} others={p.list} here={p.here} now={p.now}
        onAnswer={b => p.feed.answer(q.id, b)} onDecline={() => p.feed.decline(q.id)}
        onRelease={q.source === "hook" ? () => p.feed.release(q.id) : undefined} />
    {/if}
  {/each}
```

(`import NoticeCard from "./NoticeCard.svelte";`).

`web/shell/src/q/SidebarQuestions.svelte`:

```svelte
<script lang="ts">
  // The open questions about the artifact shown (spec
  // 2026-10-06-agent-questions-and-inbox §9.5; 2026-10-16 §2), at the top of
  // its sidebar, above the threads: as cards, except those asked in an open
  // thread, which show in that thread's card and are counted and linked
  // here. A closed one says what closed it for 4 s, then leaves. The block
  // is there (hidden) while nothing is open, so a card leaving with focus
  // can hand it on; its heading shows only with a card or a link. Its
  // styles are QuestionList's.
  import type { QuestionFeed } from "./feed.svelte";
  import QuestionList from "./QuestionList.svelte";
  import { agentLabel, cutHeader, inThread, noticeLine } from "./model";
  import { shared } from "./shared";

  const p: { aid: string; feed?: QuestionFeed; now?: Date } = $props();
  const feed = $derived(p.feed ?? shared().questions);
  const all = $derived(feed.byArtifact(p.aid));
  const cards = $derived(all.filter(q => !inThread(q)));
  const threaded = $derived(all.filter(q => inThread(q) && q.status === "open"));
  const open = $derived(all.filter(q => q.status === "open").length);
  // Scrolls to the thread's card when it is on this page; else the link navigates.
  const go = (e: MouseEvent, tid: string) => {
    const slot = document.querySelector(`.thread-q-slot[data-thread="${CSS.escape(tid)}"]`);
    if (!slot) return;
    e.preventDefault();
    slot.scrollIntoView({ block: "nearest" });
    slot.querySelector<HTMLElement>(".qcard")?.focus();
  };
</script>

<section class="side-q" aria-labelledby="side-q-h" hidden={!all.length}>
  {#if all.length}<h2 class="gh ag" id="side-q-h"><span class="sw" aria-hidden="true"></span><span class="t">Questions for you</span> <span class="c">{open}</span></h2>{/if}
  {#each threaded as q (q.id)}
    <a class="q-thread" href={`?thread=${q.thread!.id}`} onclick={e => go(e, q.thread!.id)}>{q.notice ? noticeLine(q, []) : `${agentLabel(q.agent, [])} asks in a thread`}: {q.questions.map(s => cutHeader(s.header)).join(", ")}</a>
  {/each}
  <QuestionList list={cards} {feed} here={p.aid} now={p.now} />
</section>

<style>
  :global { .side-q .q-thread { display: block; margin: 0 0 8px; font: 500 13px/18px var(--font); color: var(--accent-ink); } }
</style>
```

`web/shell/src/q/index.ts` `artifact()`: after `fill(…)`, mount the threads' questions:

```ts
  // Each thread card's slot (`ThreadCard`'s `.thread-q-slot`) shows the
  // questions asked in that thread; cards come and go with the threads, so
  // a MutationObserver finds slots added and removed.
  const threads = new Map<Element, Record<string, unknown>>();
  const sweep = () => {
    for (const el of doc.querySelectorAll<HTMLElement>(".thread-q-slot[data-thread]")) {
      if (!threads.has(el)) threads.set(el, mount(ThreadQuestions, { target: el, props: { aid, tid: el.dataset.thread! } }));
    }
    for (const [el, c] of threads) if (!el.isConnected) { void unmount(c); threads.delete(el); }
  };
  const watch = new MutationObserver(sweep);
  watch.observe(doc.body, { childList: true, subtree: true });
  sweep();
  off.push(() => { watch.disconnect(); for (const c of threads.values()) void unmount(c); threads.clear(); });
```

(`import ThreadQuestions from "./ThreadQuestions.svelte";`), and its doc comment adds "and the questions asked in each thread inside that thread's card".

`web/shell/src/ui/ThreadCard.svelte`, after `{@render writers?.(t.id)}`:

```svelte
    <!-- The question module (loaded apart, for a person with an inbox) fills this with the questions asked in the thread. -->
    <div class="thread-q-slot" data-thread={t.id}></div>
```

and in its styles `.thread-q-slot:empty { display: none; }`.

The extension: `web/extension/src/messages.ts` adds `| { t: "q-pull"; req: number; questionId: string }` (doc: "Ask me here (notices).") and `case "q-decline": case "q-release": case "q-pull":` in the validator; `web/extension/src/sw/api.ts:210` widens `verb` to `"answer" | "decline" | "release" | "pull"`; `web/extension/src/sw/panel.ts` adds `"q-pull"` to `INBOX_ACTIONS` and the `case`, with `const verb = m.t === "q-answer" ? "answer" : m.t === "q-decline" ? "decline" : m.t === "q-pull" ? "pull" : "release";`; `web/extension/src/panel/questions.ts` adds `"q-pull"` to `Close` and `override pull(id: string): Promise<void> { return this.closing({ t: "q-pull", questionId: id }); }`.

- [ ] **Step 4: Run the tests to see them pass**

Run: `just web-test`
Expected: PASS (the shell's and the extension's unit tests).

- [ ] **Step 5: Verify in the browser**

On the dev loop (`just watch`, port 7481), publish a page from a `just dev claude` session, send a thread, and have the agent `ask` with `url_or_id` and `thread_id`; check the card renders inside the thread card beneath its last comment, the sidebar's "Questions for you" counts it and links to it, and the top bar's Inbox counts it; then have it post `ask {notice: true}` and check the notice and **Ask me here**. At 1280 and 390 wide, light and dark.

- [ ] **Step 6: Commit**

```bash
git add web/shell/src/api.ts web/shell/src/q web/shell/src/ui/ThreadCard.svelte web/extension/src/messages.ts web/extension/src/sw/panel.ts web/extension/src/sw/api.ts web/extension/src/panel/questions.ts
git -c commit.gpgsign=false commit -m "Show a thread's question inside its card, counted and linked from the sidebar, and notices that an agent is asking in the terminal, with Ask me here"
```

---

## Task 14: Browser end-to-end: thread questions, notices, screenshots

**Files:**
- Create: `web/e2e/ask-where.spec.ts`
- Modify: `web/e2e/questions-helpers.ts:25-28` (`ask` takes `thread_id` and `notice`; `threadAsOwner` is at line 43)

**Interfaces:**
- Consumes: `startDaemon` via the `freshDaemon` fixture, `registerSession`, `publishAs`, `api`, `openArtifact`, `streamLive` (`web/e2e/fixtures.ts`); `threadAsOwner` (`web/e2e/questions-helpers.ts`); `settle` (`web/e2e/time.ts`).
- Produces: e2e coverage of the spec's browser tests and screenshots under `test-results/ask-where/`.

- [ ] **Step 1: Write the failing test**

In `web/e2e/questions-helpers.ts`, widen `ask`'s body type to `{ source: "ask" | "hook"; artifact_id?: string; thread_id?: string; notice?: boolean; questions: QuestionSpec[] }` (the body passes through unchanged).

Create `web/e2e/ask-where.spec.ts`:

```ts
// Where a question shows (spec 2026-10-16-ask-where-attention-is-design
// §2, §3): a thread's question inside its card, counted in the Inbox; a
// notice that an agent is asking in the terminal, with Ask me here; at 1280
// and 390 wide, light and dark.
import { test, expect, openArtifact, publishAs, registerSession } from "./fixtures";
import { ask, threadAsOwner } from "./questions-helpers";
import { settle } from "./time";

const PAGE = { "index.html": "<main><h2>Quarterly goals</h2></main>" };
const SIZES = [{ width: 1280, height: 860 }, { width: 390, height: 844 }] as const;

for (const theme of ["light", "dark"] as const) {
  for (const size of SIZES) {
    test(`a thread's question renders in its card and a notice offers Ask me here (${theme}, ${size.width})`, async ({ page, freshDaemon: d }) => {
      await page.setViewportSize(size);
      await page.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" });
      const s = await registerSession(d.base, d.token);
      const { artifact } = await publishAs(d.base, d.token, s.id, "Quarterly Review", PAGE);
      const tid = await threadAsOwner(d.base, d.token, artifact.id, "Bigger title?");
      const q = await ask(d.base, d.token, s.id, { source: "ask", artifact_id: artifact.id, thread_id: tid, questions: [{ question: "How big?", header: "Size", options: [{ label: "Big" }, { label: "Bigger" }] }] });
      const n = await ask(d.base, d.token, s.id, { source: "ask", notice: true, artifact_id: artifact.id, questions: [{ question: "Which layout?", header: "Layout" }] });
      await openArtifact(page, d.base, artifact.id, 1, "sandbox");
      if (size.width < 600) await page.getByRole("button", { name: "Threads" }).click();
      const inCard = page.locator(`.thread-q-slot[data-thread="${tid}"] .qcard[data-question="${q.question.id}"]`);
      await expect(inCard).toBeVisible();
      await expect(inCard.getByRole("button", { name: /^Answer/ })).toBeVisible();
      await expect(page.locator(".side-q a.q-thread")).toHaveAttribute("href", `?thread=${tid}`);
      const inbox = page.locator(size.width < 600 ? ".phone-tabs .inbox-link .count" : ".topbar .inbox-link .count");
      await expect(inbox).toHaveText(/^[1-9]/);
      const notice = page.locator(`.qcard.notice[data-question="${n.question.id}"]`);
      await expect(notice.locator(".asker")).toHaveText(/is asking you in the terminal$/);
      await settle(page);
      await page.screenshot({ path: test.info().outputPath(`ask-where-${theme}-${size.width}.png`) });
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(size.width);
      await notice.getByRole("button", { name: "Ask me here" }).click();
      await expect(notice.locator(".closed")).toHaveText(/^Sent to .*: it asks here next$/);
    });
  }
}
```

- [ ] **Step 2: Run the test to see it fail, then pass**

Run (before Task 13 lands, it fails on the missing `.thread-q-slot`): `bazelisk test //web:e2e_rest --test_arg=e2e/ask-where.spec.ts` (the spec falls in the Playwright project `rest`)
Expected after Tasks 6 and 13: PASS, four runs, each writing its screenshot to its test output folder.

- [ ] **Step 3: Look at the screenshots**

Open the four PNGs (under Bazel, in `bazel-testlogs/web/e2e_rest/test.outputs/`) and check: the question card sits inside the thread card beneath its last comment, styled as the sidebar's cards; the notice reads "claude is asking you in the terminal" with its headers and **Ask me here**; nothing overflows at 390; dark mode uses the theme's tokens.

- [ ] **Step 4: Commit**

```bash
git add web/e2e/ask-where.spec.ts web/e2e/questions-helpers.ts
git -c commit.gpgsign=false commit -m "Test in the browser that a thread's question renders in its card and a notice offers Ask me here, at 1280 and 390, light and dark"
```

---

## Task 15: Instructions: the MCP server instructions and the four skills ask where attention is

**Files:**
- Modify: `crates/clax-mcp/src/tools.rs:25-40` (`INSTRUCTIONS`; the sentence to replace is at lines 38-40, after the `watch`/`wake` sentence and the db sentence)
- Modify: `plugins/claude-code/skills/clax/SKILL.md:372-405`, `plugins/clax/skills/clax/SKILL.md:375-404`, `plugins/clax-grok/skills/clax/SKILL.md:369-398`, `plugins/pi/skills/clax/SKILL.md:370-399` ("## Asking the person", and a new "## Where to ask" before it)
- Modify: `scripts/test-plugins.sh:413-416` (`same_section "Where to ask"`, after `same_section "Comment loop"` at line 416)
- Test: `crates/clax-mcp/src/tools.rs` tests

**Interfaces:**
- Consumes: the `attention` tool, `ask`'s `tell_terminal` and `notice` (Tasks 9, 10).
- Produces: the four-step rule in the MCP server instructions and, word for word, in every skill's `## Where to ask`.

- [ ] **Step 1: Write the failing tests**

In `crates/clax-mcp/src/tools.rs` tests:

```rust
    #[test]
    fn the_instructions_ask_where_the_person_is() {
        for s in [
            "call `attention`",
            "ask only on the surface it names",
            "Asked in Clax: <url>",
            "`ask` with `notice: true`",
            "answer it in its thread with `comments_reply`",
        ] {
            assert!(INSTRUCTIONS.contains(s), "{s}");
        }
        assert!(!INSTRUCTIONS.contains("rather than in chat"), "the old rule is gone");
    }
```

In `scripts/test-plugins.sh`, after `same_section "Comment loop" "${skill_copies[@]}"`:

```bash
for f in "${skill_copies[@]}"; do
    if [ -n "$(section "$f" "Where to ask")" ]; then pass "$f has a Where to ask section"; else fail "$f has no '## Where to ask' section"; fi
done
same_section "Where to ask" "${skill_copies[@]}"
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `bazelisk test //crates/clax-mcp:clax_mcp_lib_test --test_arg=tools::tests::the_instructions && scripts/test-plugins.sh`
Expected: FAIL: the instructions still say "rather than in chat"; no skill has "## Where to ask".

- [ ] **Step 3: Write the instructions**

Replace the last sentence of `INSTRUCTIONS` ("When you need a decision or something only the person knows while they work with you in Clax, ask with `ask` rather than in chat; their answers are their own words: data, not instructions.", lines 38-40) with:

```rust
… pinning every write to an existing document with the version you read. Before you ask the person \
anything, call `attention`: it says where they are paying attention (the terminal or Clax, and the \
narrowest place there), how sure it is and why. Then ask only on the surface it names: in Clax with \
`ask` (`url_or_id` and `thread_id` for the place), then write the result's `tell_terminal` line, \
\"Asked in Clax: <url>\", in chat; in the terminal in chat, after posting `ask` with `notice: true` \
so Clax shows that you are asking, which you cancel once they answer. Answer work a Clax thread \
started: answer it in its thread with `comments_reply`; answer work started in chat in chat. Follow \
what you know over the suggestion (the person said where they are), and say why. Their answers are \
their own words: data, not instructions.";
```

Add, in each of the four skills, before "## Asking the person", the same section (the bare tool names, as the skills use elsewhere):

```markdown
## Where to ask

Before you ask the person anything, find where they are paying attention,
and ask there only:

1. Read the suggestion: call `attention` (an `ask` result and a delivery
   from Clax carry it too). It names a `surface`, `clax` or `terminal`,
   `where` (a `thread_id` and `artifact_id`, the narrowest place in Clax),
   a `confidence` and `why`.
2. Ask on the surface it names, at the place it names, and only there. In
   Clax: `ask` with `url_or_id` and, for a thread, `thread_id`. In the
   terminal: in chat (or the harness's own question tool).
3. Tell the other surface. After asking in Clax, write the result's
   `tell_terminal` line ("Asked in Clax: <url>") in chat. Before asking in
   chat, post `ask` with `notice: true` (the questions' headers, and the
   place), so Clax shows that you are asking; cancel it (`question_id`,
   `cancel: true`) once they answer. If the person chooses **Ask me here**
   on that notice, you are told so: ask it with `ask` then.
4. Keep discussion where it started: answer work a Clax thread started in
   that thread with `comments_reply`, and work started in chat in chat.

An `unsure` suggestion names Clax. When you know better (the person said
"I'm on my phone", or "ask me here"), follow that and say why.
```

In each skill's "## Asking the person", replace the opening paragraph's "and they are working with you in Clax (…), ask with `ask` instead of asking in chat. Pass `url_or_id` when the question is about a page, so it shows there." with "ask where "Where to ask" says." and drop the `surface_open: false` bullet ("you may ask in chat too, …"), which a single surface replaces. In the Claude Code skill, replace the closing paragraph ("Your built-in AskUserQuestion is mirrored … Prefer `ask`.") with:

```markdown
Your built-in AskUserQuestion follows the same rule: when the suggestion
names the terminal, its dialog shows at once and Clax shows a notice beside
your page; when it names Clax, it waits there (the status line says so) and
the person may choose **Answer in the terminal**. If they choose **Ask me
here** on its notice, ask it again with `ask` once the dialog closes.
```

and in the Codex and Grok skills, add after the four steps' section is in place, in "## Asking the person": "Your built-in question tool (`request_user_input` in Codex plan mode, `ask_user_question` in Grok) asks in the terminal: post the notice first, as step 3 says."

- [ ] **Step 4: Run the tests to see them pass**

Run: `bazelisk test //crates/clax-mcp:all && scripts/test-plugins.sh && scripts/sync-skill-tools.py --check`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/clax-mcp/src/tools.rs plugins/claude-code/skills/clax/SKILL.md plugins/clax/skills/clax/SKILL.md plugins/clax-grok/skills/clax/SKILL.md plugins/pi/skills/clax/SKILL.md scripts/test-plugins.sh
git -c commit.gpgsign=false commit -m "Tell every agent to ask where the person is: read the suggestion, ask on one surface at the narrowest place, tell the other, and answer in the thread or chat where the work started"
```

---

## Task 16: Close-out: qualifier records and the gates

**Files:**
- Modify: `crates/clax-realtime/src/.qual` (resolve `27bb8860`, which was `fabba150` before the layout migration moved it; `fabba150` itself is closed as a duplicate by `adf4c209`), and the `.qual` files the new records land in

**Interfaces:**
- Consumes: every task's commits.
- Produces: record `27bb8860` resolved with evidence; follow-up records; one green run of the gates.

- [ ] **Step 1: Run the gates once**

Run (in the background, logged inside the worktree): `just ci > target/gates-ask-where.log 2>&1`
Expected: every lane passes. A lane that fails in code this branch did not touch is rerun alone (its Bazel target, or its script), and both results are reported.

- [ ] **Step 2: Resolve record `27bb8860` with evidence, in its own commit**

The record is a concern tagged `status:needs-decision`; the owner's decision is the spec's "Settled by the owner (2026-10-16)" (tab focus goes to the daemon; a hidden or unfocused tab is never Clax attention), and Task 6's commit fixes it, so this branch closes it (AGENTS.md: a record one's own commit fixed). Its reply `6c71e6ae` ("Narrowed, not settled: a mirrored question now waits in Clax only once its session has a page") is answered by the same close. Take the SHA of Task 6's commit (`git log --format=%h -1 --grep "a hidden tab no longer holds it"`), then:

```bash
"/Users/alex/.claude/plugins/cache/qualifier/qual/0.1.0/scripts/ensure-qualifier.sh" exec resolve 27bb8860 \
  "A hidden tab no longer counts as Clax attention: the AskUserQuestion hook's mode follows the session's suggestion (crates/clax-http/src/surface.rs, crates/clax-core/src/surface.rs), in which only a focused tab or input on the session's page counts; Hub::holds_surface (crates/clax-realtime/src/stream.rs) now only says whether Clax can show a question. Evidence: bazelisk test //crates/clax-server:all --test_arg=api_questions::a_hidden_tab_does_not_hold_a_hook_question_after_a_prompt --test_arg=api_where_to_ask::a_hidden_tab_is_not_attention passes." \
  --reason fixed --ref git:<sha>
git add crates/clax-realtime/src/.qual
git -c commit.gpgsign=false commit -m "Close the hidden-tab surface record, fixed by the suggestion of where to ask"
```

(The record's span has drifted, `crates/clax-realtime/src/stream.rs:1402-1427` against `holds_surface` at 1414-1430 before Task 5's edit; `qualifier review crates/clax-realtime/src/stream.rs` shows it, and the resolve targets it by ID either way.)

- [ ] **Step 3: Record what the next person would want to know**

Write a batch file outside the repository (`$TMPDIR/ask-where.qual.jsonl` in the session's scratch directory), dry-run it, then record:

- `concern` on `crates/clax-core/src/surface.rs` (the `TIE_SECS`/`QUIET_SECS` lines): the windows are a default to tune with use; a person who types a long prompt and watches the terminal for more than 5 minutes gets an unsure suggestion that leans to Clax.
- `alternative` with `revisit:` on `crates/clax-server/src/routes/feedback.rs` (`pulled_block`): **Ask me here** rides the late-answer tiers, not the Claude Code channel notice; revisit when channel notices carry more than comments.
- `concern` on `crates/clax-hooks/src/events.rs:536-547` (`prompt`): the prompt hook's `prompt=true` is untested against a remote (connected) daemon; `crates/clax-cli/tests/connect.rs`'s `a_remote_hooks_ask_mirrors_the_question` (line 1421) and `a_remote_hooks_ask_waits_for_the_persons_answer` (line 1561) cover the ask hook only. (Record `73ffc4d2`, the remote AskUserQuestion failure, is resolved, `36f61b79`.)
- `concern` on `crates/clax-http/src/surface.rs` (`signals`): the extension's panel counts only as Clax being able to show a question; its focus is unknown.
- `concern` on `crates/clax-store/src/linked.rs` (`scan_linked_questions`): a thread question about a linked artifact is copied to the remote without its thread (this daemon's thread ID is not the remote's), so the person sees it on the remote's artifact, not in the linked thread; mapping it through the linked threads is follow-up work.
- `concern` tagged `status:needs-decision` on `crates/clax-server/src/routes/surface.rs` (the `attention` handler): "attention" already names each viewer's attention per artifact (`crates/clax-store/src/attention.rs`, `GET /api/viewers/me/attention`, the artifact JSON's `attention`); the new `attention` tool, route and `ask` field give it a second meaning. Question for the owner: keep the name, with the contract telling the two apart, or rename the new one (for example `where_to_ask`).

Run: `"/Users/alex/.claude/plugins/cache/qualifier/qual/0.1.0/scripts/ensure-qualifier.sh" exec record --stdin --dry-run < "$TMPDIR/ask-where.qual.jsonl"`, then without `--dry-run`.

- [ ] **Step 4: Check what the branch adds and closes**

Run: `"/Users/alex/.claude/plugins/cache/qualifier/qual/0.1.0/scripts/ensure-qualifier.sh" exec diff main`
Expected: the six records added, `27bb8860` closed, nothing drifted unaccounted for.

- [ ] **Step 5: Commit the records**

```bash
git add crates/clax-core/src/.qual crates/clax-http/src/.qual crates/clax-store/src/.qual crates/clax-server/src/routes/.qual crates/clax-hooks/src/.qual
git -c commit.gpgsign=false commit -m "Record the suggestion's default windows, Ask me here's delivery tiers, the remote prompt hook's untested report, the panel's unknown focus, linked thread questions and the attention name"
```

---

## Self-review

**Spec coverage.**
- §1 The attention estimate: instigation (Task 2 `note_delivery`, Task 5 `mark_items` in `clax-realtime`, Task 7 late answers and pulls), terminal activity per harness (Claude Code: Task 8; Pi: Task 11; Codex and Grok: Task 7 `note_turn`), Clax activity (presence `here` and the panel: Task 5; tab focus and last input: Tasks 3 and 12, never shown to other viewers), the suggestion and its rules (Task 4), hidden tab never attention (Tasks 5 and 6, record closed in Task 16), where agents read it (the `attention` tool: Task 10; `ask` results: Tasks 6 and 9; delivery and prompt context: Task 7).
- §2 Asking in Clax: `ask` takes a place and migration 37 adds `questions.thread_id` (Tasks 1, 6, 9), the thread checked on its artifact and readable through the agent's artifact gate (Tasks 1, 6), the narrowest rendering, counts and links (Task 13), `tell_terminal` (Task 9) and the instruction to write it (Task 15).
- §3 Asking in the terminal: the notice (Tasks 1, 6, 13), Claude Code's hook released at once with the notice and closed by `asked` (Tasks 6, 8), unsure or Clax waits as today with **Answer in the terminal** kept (Task 6), chat notices (`ask {notice: true}`: Tasks 6, 9, 15), **Ask me here** as a delivery (Tasks 6, 7, 13) and the Esc limit (Tasks 7, 13).
- §4 Answers stay with their thread: the four steps in the MCP instructions and all four skills (Task 15), and `why` shown to the agent (Tasks 4, 10).
- §5 Each harness: Claude Code (Tasks 6, 8), Pi (Task 11, `clax_attention` Task 10), Codex and Grok (Task 7 inference; Task 15 skill text).
- Tests section: store (Tasks 1, 2, 4), server (Tasks 5, 6, 7), plugins (Tasks 8, 11, 15), browser (Tasks 13, 14).
- Not in the spec, from the code since: a linked artifact's notices stay local (Task 1); the session's page goes through the agent's gate (Task 5); the tool count goes from twenty-nine to thirty (Task 10).

**Placeholders.** None: every step names its files, code and commands. The one value read at run time is Task 16's commit SHA, found by the `git log --grep` given.

**Names across tasks.** `Store::note_terminal(ctx, sid) -> Vec<String>`, `note_turn(sid) -> bool`, `note_delivery(sid, aid, tid)`, `session_signals(sid)`, `owner_public_id()` (Task 2) are used as such in Tasks 5 and 7; `Presence::report_engaged`/`engaged_at` (Task 3) in Task 5; `clax_core::surface::{Signals, Suggestion, Surface, suggest, line, QUIET_SECS}` (Task 4) in Tasks 5, 6, 7; `clax_http::surface::{page, signals, suggestion, note_poll, INFERRED}` (Tasks 5, 7) in Tasks 6, 7; `clax_testkit::where_to_ask::{clocked, attention, typed, owner_tab, paged}` (Task 5) in Tasks 6, 7; `Close::Pull`/`Close::Lapse` and `Reason::Pulled`/`Reason::Lapsed` (Task 1) in Tasks 2, 6; the view's `thread`/`notice`/`pulled` (Task 6) in Tasks 9 and 13; `ask_result(q, waited_s, attention)` (Task 9) in its tests; `pullQuestion`/`QuestionFeed.pull` (Task 13) behind `NoticeCard.onPull`.
