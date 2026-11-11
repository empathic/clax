# Working marks across background subagents: implementation plan

**Goal:** A thread whose work the agent hands to subagents that run on after its turn stays marked working until the agent replies, in every harness, without marking the other threads of the session.

**Architecture:** The agent says which threads it handed off (`working` with `background: true`). The daemon's working registry flags them; a turn end and a publish end the rest of the record's work and keep them; the 600 s lapse covers harnesses whose subagents cannot renew the parent. No hook logic per harness.

**Spec:** `docs/superpowers/specs/2026-11-06-working-across-background-subagents-design.md`

## Global Constraints

- Build and test through Bazel (`just`, or `bazelisk test //…` targets); never `cargo build`/`cargo test`/`pnpm install` in a worktree.
- No `unsafe`. Tests use injected clocks (`ManualClock`), never sleeps.
- The four skills' `## Comment loop` sections stay word for word the same (`scripts/test-plugins.sh`), and every tool description is the same in `crates/clax-mcp/src/tools.rs`, `plugins/pi/src/clax.ts` and `plugins/pi/test/fixtures/contract.json`.

## Review Focus

1. A thread answered in the same turn as a hand-off must end at the turn end; only the flagged thread stays. Task 1 (`a_turn_end_keeps_the_background_threads_and_ends_the_others`) and Task 2 (`a_background_thread_outlives_a_publish_and_the_turn_and_ends_with_the_reply`).
2. A publish must not count a background thread as addressed unless `addresses` names it. Task 2.
3. Re-naming a record's threads without `background` drops the flag from threads no longer named, and the lapse returns to 120 s. Task 1.
4. `background` without `thread_ids`, or with `done`, is `invalid_args` at every layer. Tasks 2 and 3.

---

### Task 1: The registry flags background threads (`crates/clax-core/src/working.rs`)

- [x] `BACKGROUND_TTL_SECS = 600`; `SetWorking::background`; `Record::background`; `WorkingView::background_thread_ids` (skipped when empty).
- [x] `set` with `background` adds and flags the threads; without it, replacing the threads keeps flags only on threads still named.
- [x] `End::TurnEnd` and `End::Publish` end a live record's foreground work (its finished item and `working.stop`) and carry its background threads on in a fresh record (`Working::end_record`, `Record::split_background`); a plain `set` replaces only foreground threads; a call past 20 threads on the record fails; removing a thread drops its flag, and the last background thread's removal renews the record; `Working::foreground_threads_of`.
- [x] The lapse, `expires_at` and `next_lapse` use 600 s while a record names background threads.
- [x] Tests: the eight background tests at the end of the module's tests.

### Task 2: The daemon routes (`crates/clax-server`)

- [x] `PUT /api/sessions/<sid>/working/<aid>` takes `background`; without non-empty `thread_ids` it is `invalid_args`.
- [x] A publish links `foreground_threads_of` to its version.
- [x] Tests in `tests/api_working.rs`: `a_background_thread_outlives_a_publish_and_the_turn_and_ends_with_the_reply`, `background_names_the_threads_it_applies_to`.

### Task 3: The `working` tool, in Rust and in Pi

- [x] `WorkingArgs::background` (`crates/clax-mcp/src/tools.rs`) and Pi's `WorkingArgs` (`plugins/pi/src/clax.ts`): refused without `thread_ids` or with `done`; the result carries `background_thread_ids` and `expires_in_s` 600 when the record has background threads.
- [x] The tool description, in all three copies, says what `background` does; the publish description says background threads are not added to `addresses` unasked.
- [x] Tests: `working_in_the_background_keeps_threads_past_the_turn_end` (`crates/clax-mcp/tests/comments.rs`), `clax_working keeps background threads past agent_end` (`plugins/pi/test/clax.test.ts`).

### Task 4: Skills and contract

- [x] The `## Comment loop` paragraph in all four skills: hand-offs take `background: true`; subagents report to the owner, who replies; a Codex owner waits with `wait_agent`.
- [x] `docs/contract.md` "Working": `background`, `background_thread_ids`, the turn end and publish exception, the 600 s lapse.
- [x] `scripts/test-plugins.sh` and `scripts/check-version.sh` pass.

### Task 5: Close the loop

- [ ] CI's `just check` (it cannot run on a machine whose Bazel fails resolving the Apple and Android toolchains).
- [ ] A manual run in a dev home: hand a thread to a background subagent with `background: true`, answer another thread, end the turn; the page keeps only the handed-off thread until the reply.
