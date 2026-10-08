# Toolpath Audit Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Clax records every provenance-relevant change, and every agent tool call, as Toolpath:

- an append-only `audit_events` table;
- a rotated Toolpath JSONL journal under `~/.clax/toolpath/journal/`, projected continuously from that table;
- deterministic `clax toolpath export` graphs, with one path per artifact plus the install's audit trail.

Agent steps carry complete cross-link references: harness, harness session ID, transcript path, tool name with canonical argument hash and time, exact call IDs where they come free, and git context. The scope is recording only: no importer and no correlation (spec O6). Clax paths follow artifacts, not agent sessions (spec O7).

**Architecture:**

- The daemon writes an audit row inside each mutation's own transaction and nudges an appender thread.
- The appender renders new rows into `.path.jsonl` segments, off the writer path.
- One pure renderer in `clax-core::toolpath` serves both the journal and export.
- The MCP shim, `clax hook` and the Pi extension capture git state and hash tool arguments. They send both in headers, and post a `tool.call` record after each call returns.

**Tech Stack:** Rust (rusqlite, axum, serde_json, sha2), clap, TypeScript (Pi extension). Ajv, in the web unit gate, validates the Rust-written golden documents against Toolpath's published schema (no Rust schema validator: Task 9 re-review ruling).

**Spec:** `docs/superpowers/specs/2026-10-06-toolpath-audit-design.md` ("the spec"; §N refers to it).

**Before Task 1:**

- Confirm the migration numbers on main. Questions take 19 and the inbox 20; this plan takes 21 (owner, 2026-10-06). If the numbers have shifted, take the next free one.
- Read spec §2, §5–§8, §10 and §12 in full.

## Global Constraints

- No `unsafe` anywhere (the workspace `unsafe_code = "forbid"` lint and the `no unsafe code` gate).
- No hard links (the `no hard links` gate). Copy, rename or write.
- Keep the whole check suite under about 2 minutes:
  - use `ManualClock` and injected seams;
  - never sleep for a fixed time;
  - wait on exit files or channels with timeouts, never `pgrep -f`.
- Never edit an earlier migration. Every new migration gets a migration test.
- Leave the perf budgets unchanged (`scripts/perf-daemon-budget.json`, `scripts/perf-clients-budget.json`, `web/perf/budget.json`). Task 16 runs `just perf` and records the numbers.
- Doc comments and commit messages describe the contract. Write "ID" in prose, never "id".
- Commit with `git -c commit.gpgsign=false commit`. Never push or merge.
- Comment text, notes and titles are recorded (spec O3).
- Never record diff contents, page bytes, tool argument values (only their hash), tokens, cookies or credentials.
- **Recording only:**
  - No task imports or correlates.
  - No task depends on Toolpath reading a harness: every harness, Grok included, gets the same references.
  - Toolpath's repo is read-only reference, and only its schema is vendored, for tests.
- **Paths follow artifacts.** No session-shaped path, and no session-only path fields. Sessions are actors and references (spec O7).
- **Redaction classes (Task 9 ruling T9-I1).** A task that adds a kind or a body field gives it a class in `crates/clax-core/src/toolpath/redact.rs` (safe, text, name or path); `every_recorded_kind_is_classified_and_renders_conformant` fails until it does. Regenerate the goldens with `CLAX_UPDATE_GOLDEN=1 cargo test -p clax-core toolpath` and review the diff (Tasks 12, 14, 15).

## Review Focus

1. Each mutation writes its audit row in the same transaction as the mutation. Pinned in Tasks 5–7 by `every_mutation_records_one_event`, a table test listing every mutating route.
2. The journal never holds a partial line, and never duplicates or skips a seq across crashes. Pinned in Task 11 by `recovery_truncates_partial_line` and `crash_mid_batch_resumes_without_duplicates`.
3. Rendering is deterministic. Pinned in Task 9 by `render_is_byte_stable` and in Task 10 by `export_twice_is_identical`.
4. Output conforms to Toolpath's published schema. Pinned in Task 9 by `golden_export_validates_against_schema` and `golden_segments_seal_and_validate`.
5. The argument hash matches the spec vectors on both sides. Pinned in Task 12 by `args_hash_vectors`, and in Task 13 by the Pi `args hash vectors` test.
6. The writer path stays cheap. Pinned in Task 4 by `nudge_never_blocks`, and in Task 16 by the perf gates.
7. Git capture never blocks or fails an action, and never sends contents. Pinned in Task 12 by `capture_times_out_at_deadline`, `header_has_no_paths_or_contents` and `userinfo_is_stripped`.
8. Paths are artifact and install paths only. Pinned in Task 10 by `export_has_no_session_paths`.
9. Export is owner-only. Pinned in Task 10 by `lan_viewer_cannot_export`.

## File structure

```
crates/clax-core/src/audit.rs            records, Actor, AuditCtx, CallHeader, Store::record_audit, nudge seam
crates/clax-core/src/gitctx.rs           GitContext, validation, header codec, capture runner
crates/clax-core/src/toolpath/mod.rs     render_step, ActorDef rendering, URIs, provider mapping
crates/clax-core/src/toolpath/args.rs    canonical argument hash (JCS, RFC 8785) + SHA-256
crates/clax-core/src/toolpath/redact.rs  --no-text / --no-names / --no-paths
crates/clax-core/src/toolpath/project.rs artifact / install / journal projections
crates/clax-core/src/toolpath/segment.rs segment writer, rotation, recovery (std::fs only)
crates/clax-core/src/store/audit.rs      queries, backfill
crates/clax-core/src/store/migrations.rs migration 22
crates/clax-cli/build.rs                 CLAX_BUILD_COMMIT (handed to clax-core at startup)
crates/clax-core/tests/toolpath/         golden histories, segments, exports, args-hash-vectors.json,
                                         schema/toolpath.schema.json + SOURCE (vendored, read-only)
crates/clax-server/src/audit.rs          AuditCtx extractor, appender thread, export/status/tool-call routes
crates/clax-server/tests/api_toolpath.rs integration tests
crates/clax-mcp/src/git.rs               git capture before mutating tool calls
crates/clax-mcp/src/calls.rs             call identity, x-clax-call header, background tool.call POST
crates/clax-hooks/src/events.rs          transcript_path + git on join; PostToolUse call IDs
crates/clax-cli/src/commands/toolpath.rs `clax toolpath export|status`
plugins/pi/src/{git,calls}.ts            Pi-side capture, hashing, exact call IDs
docs/contract.md                         journal and export contract
```

---

### Task 1: Audit records, git context and call identity types

**Files:**
- Create: `crates/clax-core/src/audit.rs`, `crates/clax-core/src/gitctx.rs`
- Modify: `crates/clax-core/src/lib.rs`

**Interfaces:**
- Produces:
  - `enum AuditKind`: every kind in spec §6, including `tool.call` and `tool.call_id`.
  - `enum Actor`: spec §5.2. The agent variant carries `transcript_path`.
  - `enum Via`.
  - `struct AuditCtx { actor, via, git: GitField, call: Option<CallHeader> }`.
  - `enum GitField { Ok(GitContext), Capture(&'static str), Absent }`.
  - `struct CallHeader { call_id, tool, harness_tool, args_sha256, started_at, harness_call_id }`.
  - `struct AuditRecord { kind, at, ids: AuditIds{artifact, artifact2, thread, session, question, call, origin}, body }`.
  - `GitContext` with the spec §9.1 fields, and `validate()`.
  - `gitctx::encode_header` / `decode_header`, and the same for `CallHeader`: base64url JSON, at most 2 KiB, otherwise `invalid`.
  - `gitctx::sanitize_remote`.

- [ ] **Step 1: Failing tests.**
  - `kind_names_match_spec`
  - `actor_json_shapes`
  - `git_header_roundtrip`
  - `call_header_roundtrip`
  - `oversized_header_is_invalid`
  - `userinfo_is_stripped`: `https://u:tok@github.com/o/r.git?x=1#f` becomes `https://github.com/o/r.git`; the scp form is kept; `ssh://git@host/x` is kept; `ssh://u:p@host/x` is stripped.
  - `validate_rejects_bad_head`
- [ ] **Step 2: Run** `cargo test -p clax-core audit gitctx`. Expected: FAIL.
- [ ] **Step 3: Implement.** Use `BTreeMap` only. Base64url comes from an existing dependency if one is in the tree; otherwise write a small hand-rolled codec.
- [ ] **Step 4: Run.** Expected: PASS.
- [ ] **Step 5: Commit** `"Define audit records, actors, and the git and call headers"`.

---

### Task 2: Migration 22, `Store::record_audit`, install ID, version content hash

**Files:**
- Modify: `crates/clax-core/src/store/migrations.rs`. Append migration 22 as in spec §5.1, with its test.
- Create: `crates/clax-core/src/store/audit.rs`
- Modify: `crates/clax-core/src/store/artifacts.rs` (`write_version_then` hashes the files and sets `content_sha256`), `crates/clax-core/src/model.rs`

**Interfaces:**
- Produces:
  - `Store::record_audit(tx, &AuditCtx, AuditRecord) -> Result<i64>`
  - `Store::install_id()`
  - `Store::events_after(seq, limit)`
  - `Store::events_for_call(call_id)`
  - `Store::newest_seq()`
  - `audit::content_manifest_sha256` (spec §5.3)
  - `Store::set_audit_nudge(..)`, which fires after `COMMIT`

- [ ] **Step 1: Failing tests.**
  - `migration_21_creates_audit_tables`
  - `install_id_is_stable_across_reopen`
  - `record_audit_rolls_back_with_tx`
  - `seq_is_commit_ordered`
  - `content_sha256_matches_manifest_rule` (a fixed fixture with a known hex)
  - `carried_files_keep_their_hash`
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4:** After Task 8, measure the mean row size on the perf seed. Replace the estimate in spec §5.1 with the measurement, in Task 8's commit.
- [ ] **Step 5: Run** `cargo test -p clax-core`. Expected: PASS.
- [ ] **Step 6: Commit** `"Add the audit_events table, install ID and version content hashes"`.

---

### Task 3: Embed the build commit

**Files:**
- Modify: `crates/clax-cli/build.rs` (`CLAX_BUILD_COMMIT`), `crates/clax-core/src/lib.rs` (`build_commit()`, set at startup), `crates/clax-cli/src/main.rs` and `commands/{version,status,doctor}.rs`, `crates/clax-server/src/daemon.rs` (`DaemonInfo.commit`), and the release workflow, which sets `CLAX_BUILD_COMMIT`

- [ ] **Step 1: Failing tests.** `crates/clax-cli/tests/version.rs`: `version_is_exactly_clax_and_the_version` (`clax --version` stays exactly `clax <version>`, which the wrappers and installers compare) and `version_verbose_names_commit`; `doctor_runs_all_checks` checks the `build` check and `clax status`'s `commit`.
- [ ] **Step 2: Implement** as in spec §5.5. Embedded by the leaf crate, so a commit or branch switch relinks `clax-cli` only. `rerun-if-changed` covers the resolved `--git-path HEAD`, the nearest existing directory on the path of the ref file it names, and `packed-refs`.
- [ ] **Step 3: Run.** Expected: PASS.
- [ ] **Step 4: Commit** `"Embed the build commit; show it in clax version --verbose, status and doctor"`.

---

### Task 4: `AuditCtx` extraction and the nudge

**Files:**
- Create: `crates/clax-server/src/audit.rs` (the extractor)
- Modify: `crates/clax-server/src/state.rs`, `identity.rs` (`Identity::audit_actor`)

**Interfaces:**
- `AuditCtx: FromRequestParts<AppState>` resolves:
  - the actor, from `Identity` plus `x-clax-session`;
  - `via`, from the `x-clax-via` header, or inferred;
  - `x-clax-git` and `x-clax-call`.
- The agent actor's `transcript_path` comes from the session row.

- [ ] **Step 1: Failing tests** in `crates/clax-server/tests/api_toolpath.rs`, against a test-only route:
  - `ctx_owner_cookie_is_owner_shell`
  - `ctx_lan_viewer_has_public_id_and_name`
  - `ctx_session_header_is_agent_with_harness_ids`
  - `ctx_bad_headers_are_invalid_not_errors`
  - `nudge_never_blocks`
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** `just test`. Expected: PASS.
- [ ] **Step 4: Commit** `"Resolve each request's audit actor, channel, git context and tool call"`.

---

### Task 5: Record artifact, version, asset, doc and live-page events

**Files:**
- Modify:
  - `crates/clax-core/src/store/{artifacts,assets,live,site}.rs`. Each mutating function takes `&AuditCtx` and records inside its transaction.
  - The docs store.
  - Call sites in `crates/clax-server/src/routes/{artifacts,docs,live,mcp}.rs`.
- Kinds: `artifact.create`, `version.publish`, `live.snapshot`, `artifact.update`, `artifact.delete`, `asset.upload`, `doc.write` (hash only), `live.page`.
- Every event made under a request with `x-clax-call` stores `call` and `call_id`.

- [ ] **Step 1: Failing tests.**
  - `every_mutation_records_one_event`: a table of (request, kind, actor type, call present). Tasks 6 and 7 extend it.
  - `publish_records_file_hashes_and_addresses`
  - `doc_write_records_hash_not_content`
  - `live_snapshot_records_origin_and_path`
  - `failed_publish_records_nothing`
  - `event_under_call_carries_call_id`
- [ ] **Step 2: Implement.** Internal callers pass `AuditCtx::system(..)`. Leave the `EventBus` publishes untouched.
- [ ] **Step 3: Run** `just test`. Expected: PASS.
- [ ] **Step 4: Commit** `"Record artifact, version, asset, doc and live-page changes"`.

---

### Task 6: Record thread, comment, resolve, send, delivery and addressed events

**Files:**
- Modify: `crates/clax-core/src/store/{threads,feedback,batches,changelog}.rs`, `crates/clax-server/src/routes/threads.rs`, `crates/clax-server/src/feedback.rs`
- Kinds: spec §6.2.

- [ ] **Step 1: Failing tests.**
  - extend `every_mutation_records_one_event`
  - `lan_reply_records_viewer_identity`
  - `agent_reply_records_session_git_and_call`
  - `batch_send_is_one_event_listing_threads`
  - `delivery_retry_is_not_recorded`
  - `resolve_addressed_links_version`
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run.** Expected: PASS.
- [ ] **Step 4: Commit** `"Record threads, comments, sends, deliveries and addressed links"`.

---

### Task 7: Record moves, rules, watches, working records and sessions

**Files:**
- Modify:
  - `crates/clax-core/src/store/{site,sessions,watches}.rs`. A `thread.move` sets `artifact2_id`.
  - `crates/clax-server/src/working.rs`. Start and stop are recorded, and the TTL sweep records `system:ttl` with `for_actor`.
  - `crates/clax-server/src/routes/{watches,live,mod}.rs`.
  - `crates/clax-hooks/src/events.rs`. `join` sends `transcript_path`, falling back to `transcriptPath`.
- Kinds: spec §6.3–§6.5 and §6.8. Session events go to the install path; a session is never a path.

- [ ] **Step 1: Failing tests.**
  - extend the table
  - `ttl_expiry_records_system_stop_with_for_actor` (uses `ManualClock`)
  - `heartbeat_records_nothing`
  - `join_stores_transcript_path_for_claude_codex_and_grok`
  - `merge_records_each_moved_thread_with_both_artifacts`
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run.** Expected: PASS.
- [ ] **Step 4: Commit** `"Record moves, rules, watches, working records and sessions"`.

---

### Task 8: Backfill existing history

**Files:**
- Modify: `crates/clax-core/src/store/audit.rs` (`backfill`), `migrations.rs` (migration 22's Rust step)

The backfill sorts all events by `(at, kind rank, natural ID)`, then inserts them, so `seq` follows history. The sources are:

- sessions;
- artifacts and versions, hashing the stored files (a missing file is recorded as `missing: true`);
- threads, comments and resolves;
- sends and feedback;
- `version_threads`;
- `thread_moves`;
- watches;
- `questions`, when that table exists.

Every backfilled event gets `backfilled = 1` and `via: daemon`, and has no git context and no tool calls.

- [ ] **Step 1: Failing tests.**
  - `backfill_orders_by_time`
  - `backfill_hashes_version_files`
  - `backfill_marks_rows`
  - `backfill_on_perf_seed_under_3s`. Record the measured time in the commit message.
- [ ] **Step 2: Implement.** Log progress every 10,000 rows.
- [ ] **Step 3: Run.** Expected: PASS.
- [ ] **Step 4: Commit** `"Backfill the audit journal from existing history"`.

---

### Task 9: The renderer, redaction, and schema conformance

**Files:**
- Create: `crates/clax-core/src/toolpath/{mod,redact}.rs` and `crates/clax-core/tests/toolpath/`:
  - golden histories and expected outputs;
  - `schema/toolpath.schema.json`, copied read-only from the Toolpath repo's `schema/` at commit `77dc16a5`, with a `SOURCE` file naming the commit;
  - `seal.rs`, a test-only reader that applies the JSONL RFC's "Reading JSONL" algorithm.
- Create: `web/scripts/toolpath-schema.test.ts`, validating the golden documents with Ajv (`ajv`, `ajv-formats` web dev-dependencies). Clax takes no dependency on any Toolpath crate, and no Rust schema validator (Task 9 re-review ruling).

**Interfaces:**
- `render_step(row, prev, env, opts)` renders a step as in spec §10:
  - `clax://` change keys with a `clax.<kind>` structural perspective;
  - `meta.clax`;
  - the §10.4 refs (`agent-session`, `transcript`, `tool-call`, `tool-use`, `at-revision`, ...);
  - a one-sentence `meta.description`;
  - step ID `e<seq:012>`;
  - no `meta.source`.
- `actor_string`, `actor_def` (spec §10.1), `provider_for`, `clax_uri`.
- `normalize_remote`, which follows `toolpath_git::normalize_git_url`'s documented cases for `at-revision`. The cases are copied into the test, and their source is cited in `SOURCE`.

- [ ] **Step 1: Failing tests.**
  - `render_each_kind_matches_golden`
  - `render_is_byte_stable`
  - `actor_strings_match_schema_pattern`, which reads the pattern from the vendored schema
  - `no_text_hashes_bodies`
  - `no_names_keeps_public_ids`
  - `no_paths_redacts_and_drops_file_refs`
  - `args_hash_never_redacted`
  - `no_step_has_meta_source`
  - `golden_export_validates_against_schema`
  - `golden_segments_seal_and_validate`
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** `cargo test -p clax-core toolpath`. Expected: PASS.
- [ ] **Step 4: Commit** `"Render audit events as Toolpath steps, validated against Toolpath's schema"`.

---

### Task 10: Projections, the export route and `clax toolpath`

**Files:**
- Create: `crates/clax-core/src/toolpath/project.rs`, `crates/clax-cli/src/commands/toolpath.rs`
- Modify: `crates/clax-core/src/store/audit.rs` (`select(&Selection)`, in one read transaction), `crates/clax-server/src/audit.rs` (`GET /api/toolpath/export`, `GET /api/toolpath/status`), `crates/clax-cli/src/main.rs`, `commands/mod.rs`

**Interfaces:**
- `Selection{artifacts, live_pages, by_sessions, since, until}`.
- `Shape::{Artifacts, Journal}`.
- `project_artifacts`, which returns one path per artifact plus the install path, with `same-change` on moves (spec §8.2).
- `project_journal`.
- Render with `toolpath::render` (step and actor definitions in one parse) under `RenderEnv::export(install, browser base)`. A path's `meta.actors` merges each actor's definitions with `merge_actor_def` (spec §7.2).
- CLI flags as in spec §8.1.

- [ ] **Step 1: Failing tests.**
  - `export_twice_is_identical`
  - `export_has_no_session_paths`
  - `by_session_filters_steps_not_shape`
  - `move_appears_in_both_artifacts_with_same_change`
  - `selectors_union_within_and_intersect_across`
  - `jsonl_requires_single_path`
  - `live_selector_matches_origin_and_path`
  - `lan_viewer_cannot_export`
  - `cli_export_writes_atomically`
- [ ] **Step 2: Implement.** Stream from a blocking reader task (`Store::call`), querying one path at a time.
- [ ] **Step 3: Run** `just test`. Expected: PASS.
- [ ] **Step 4: Commit** `"Export Clax history as Toolpath graphs with clax toolpath export"`.

---

### Task 11: The journal appender

**Files:**
- Create: `crates/clax-core/src/toolpath/segment.rs`
- Modify:
  - `crates/clax-server/src/audit.rs`: the appender thread. Start it in `boot.rs`. On shutdown, sync, then write `Head` and `PathClose`.
  - `crates/clax-core/src/config.rs`: the `[toolpath]` keys.
  - `crates/clax-cli/src/commands/doctor.rs`.

**Interfaces:**
- `SegmentWriter::open_or_recover(dir, install, clock, fs)`, `append_batch`, `maybe_rotate`, `close`.
- `JournalFs`: create, append, sync_data, set_len, rename, remove. There is no link operation.
- `Appender::drain_now()`, for tests.
- Behaviour follows spec §7. Files are 0600 and directories 0700, and a directory is fsynced after it is created.
- Render under `RenderEnv::journal(install)` (no browser URLs, spec §7.6). Write an actor's `ActorDef` before its first step and again, merged with `merge_actor_def`, whenever its definition grows (spec §7.2). `segments_seal_and_validate_against_schema` seals with `tests/toolpath/seal.rs` and checks conformance in Rust; a segment the writer produces deterministically is also written as a golden `expected/*.path.json`, which the web unit gate validates with Ajv.

- [ ] **Step 1: Failing tests.**
  - `first_start_writes_path_open_and_backfill`
  - `rotates_at_utc_day`
  - `rotates_at_size_cap`
  - `recovery_truncates_partial_line`
  - `crash_mid_batch_resumes_without_duplicates`
  - `damaged_segment_is_renamed_not_deleted`
  - `eio_backs_off_and_retries_same_batch`
  - `fsync_is_coalesced`
  - `journal_off_still_records_table_and_catches_up`
  - `retain_days_removes_whole_old_segments_only`
  - `segments_seal_and_validate_against_schema`
- [ ] **Step 2: Implement.** Loop on `recv_timeout(1 s)`, drain, then check whether to sync.
- [ ] **Step 3: Run** `just test`. Expected: PASS.
- [ ] **Step 4: Commit** `"Append the audit journal to rotated Toolpath JSONL segments"`.

---

### Task 12: The MCP shim: argument hashes, tool-call records, git capture

**Files:**
- Create:
  - `crates/clax-core/src/toolpath/args.rs`: `args_sha256(&serde_json::Value) -> String`. It canonicalizes by JCS (RFC 8785): keys in UTF-16 code-unit order, ECMAScript number formatting, minimal escapes.
  - `crates/clax-core/tests/toolpath/args-hash-vectors.json`: the seven vectors from spec §12.2.
  - `crates/clax-mcp/src/git.rs` and `crates/clax-mcp/src/calls.rs`.
- Modify:
  - `crates/clax-core/src/gitctx.rs`: `capture(cwd, deadline, git)`.
  - `crates/clax-mcp/src/client.rs`: send `x-clax-via: mcp` on every request (the stdio shim, and the daemon's own `/mcp` `DaemonClient`, which has no session and so records the sessionless agent of spec §5.2); send `x-clax-call` on every request for a call, and `x-clax-git` on mutating ones.
  - `crates/clax-mcp/src/tools.rs`: mint a `call_id` at each tool call and hash the arguments as received. After the result is returned, post `POST /api/sessions/<sid>/tool-calls` in the background.
  - `crates/clax-mcp/src/shim.rs`: capture at registration.
  - `crates/clax-server/src/audit.rs`: the tool-calls route records `tool.call` with `produced`.
  - `crates/clax-core/src/toolpath/redact.rs`: `redaction_hash` hashes a structured value's JCS form through `args.rs` (spec §11), with a test that the redaction hash of a vector's arguments is its `args_sha256`.

**Number formatting:** use a JCS crate (for example `serde_json_canonicalizer`), or a hand implementation over `ryu-js`. The vectors decide either way. Plain `serde_json` output fails vector 6.

- [ ] **Step 1: Failing tests.**
  - `args_hash_vectors`: every vector in the JSON file.
  - `read_only_tool_records_tool_call`
  - `publish_tool_call_lists_produced_seq`
  - `tool_call_post_does_not_delay_result`: the result channel resolves before the POST is sent, which a gated fake daemon proves.
  - Git capture, against fixture repositories created with real `git`:
    - `capture_clean_repo`
    - `capture_dirty_tracked_hashes_diff`
    - `capture_staged_only`
    - `capture_untracked_only_counts_without_hash`
    - `capture_detached_has_no_branch`
    - `capture_unborn_branch`
    - `capture_linked_worktree_root`
    - `capture_not_a_repo`
    - `capture_no_git_is_unavailable`
    - `capture_times_out_at_deadline`: a blocking fake `git` from `clax-fake-exe`, with no sleeping.
  - `header_has_no_paths_or_contents`
  - `read_only_tools_send_no_git_header`
  - `shim_and_daemon_mcp_send_via_mcp`: the `AuditCtx` of a shim request is the session's agent on `mcp`, and of a daemon `/mcp` request the sessionless agent on `mcp`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Measure** `capture` 50 times on this repo and record p50 and p95 in the commit message. If p95 exceeds 100 ms, report it to the owner; do not raise the deadline.
- [ ] **Step 4: Run** `just test`. Expected: PASS.
- [ ] **Step 5: Commit** `"Record each MCP tool call with its argument hash, and the agent's git state"`.

---

### Task 13: Hooks and the Pi extension; harness ID checks

**Files:**
- Modify: `crates/clax-hooks/src/events.rs` (git on join), `crates/clax-cli/src/commands/hook.rs`
- Create: `plugins/pi/src/git.ts`, `plugins/pi/src/calls.ts`, `plugins/pi/test/{git,calls}.test.ts`
- Modify: `plugins/pi/src/client.ts`. Send `x-clax-via: pi` and the call header on every request. `clax hook` sends `x-clax-via: hook` on every request it makes. The Pi extension passes its `toolCallId` as `harness_call_id` and its registered tool name as `harness_tool`.
- Create: `crates/clax-server/tests/fixtures/harness-ids/`:
  - a Claude hook stdin and the transcript's first line;
  - a Codex hook stdin and a rollout `session_meta` line;
  - a Grok hook stdin;
  - a Pi session header.

- [ ] **Step 1: Failing tests.**
  - `hook_join_carries_git_and_transcript`
  - `codex_hook_session_id_is_rollout_session_meta_id`. If they differ, stop and report, because spec §9.4 is wrong in that case.
  - `claude_hook_session_id_matches_transcript_session_id`
  - `grok_hook_records_same_refs_as_others`
  - Pi: `args hash vectors` (reading the same vectors JSON file), `capture matches the Rust fixture cases`, `tool call carries toolCallId`, and `requests send via pi`.
  - `hook_requests_send_via_hook`: the `AuditCtx` of a `clax hook` request is the session's agent on `hook`.
- [ ] **Step 2: Implement.** In TypeScript, hash with a JCS canonicalizer (for example the `canonicalize` package, or about 40 lines inline) and `node:crypto`.
- [ ] **Step 3: Run** `just test` and `cd plugins/pi && npm test`. Expected: PASS.
- [ ] **Step 4: Commit** `"Capture git state, tool calls and exact call IDs from hooks and the Pi extension"`.

---

### Task 14: Claude Code exact call IDs (PostToolUse)

**Files:**
- Modify:
  - `plugins/claude-code/hooks/hooks.json`: add a PostToolUse matcher for Clax's MCP tools.
  - `crates/clax-hooks/src/events.rs`: `tool_call_id` reads `tool_use_id`, `tool_name` and `tool_input`, and hashes `tool_input` with `args_sha256`.
  - `crates/clax-server/src/routes/sessions.rs`: `POST /api/sessions/<sid>/tool-call-ids` matches as in spec §6.7, then records `tool.call_id`.
  - the renderer: emit the `tool-use` ref.

- [ ] **Step 1: Failing tests.**
  - `posttooluse_matches_recent_call_by_tool_and_hash`
  - `unmatched_records_null_call_id`
  - `hook_hash_equals_shim_hash_for_same_input`
  - `hook_failure_exits_zero_and_logs`
- [ ] **Step 2: Implement.** Keep the hook inside its existing deadline.
- [ ] **Step 3: Run** `just test` and the plugins gate. Expected: PASS.
- [ ] **Step 4: Commit** `"Record Claude Code's tool_use IDs for Clax tool calls"`.

---

### Task 15: Question events (after agent-questions lands)

**Files:**
- Modify: the questions store and routes from the agent-questions branch; extend Task 8's backfill.
- Kinds: spec §6.6. Record one per transition, in the same transaction.
- The question routes (`questions::create`, `terminal`, `withdraw`, `release`,
  `answer`, `decline`, `release_owner`) are in the route guard's `UNAUDITED`
  list naming this task: each takes `AuditCtx` (or `DeferredAudit`) and
  leaves the list here. The transitions made outside routes record here too:
  a session's end and the reaper withdrawing its questions, a hook question's
  grace withdrawing it, and the daemon's start withdrawing mirrored questions
  left open. The backfill already records `question.ask` and the closing
  transition from the `questions` table (Task 8); this task makes it share
  the live builders.

- [ ] **Step 1: Failing tests.** `each_question_transition_records_one_event`, `question_text_redacted_under_no_text`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run.** Expected: PASS.
- [ ] **Step 4: Commit** `"Record agent questions and answers in the audit journal"`.

---

### Task 16: Contract, docs, perf verification

**Files:**
- Modify:
  - `docs/contract.md`: add a "Toolpath journal and export" section covering the files, kinds, actors, URIs, refs, the argument hash with its vectors, and redaction.
  - `docs/superpowers/specs/2026-09-28-clax-design.md`: add a D-row pointing at the spec.
  - `docs/verification.md`: add the manual checks.

- [ ] **Step 1:** Run `just perf` on main and on this branch. Both must pass with unchanged budgets; put both sets of numbers in the commit message.
- [ ] **Step 2:** Run `just check`. Every gate must pass.
- [ ] **Step 3: Manual check.**
  1. Publish from Claude Code in a dirty repo.
  2. Reply as a LAN viewer.
  3. Run `clax toolpath export --artifact <ID> -o /tmp/a.path.json`.
  4. Validate `/tmp/a.path.json` against the vendored schema with the test helper, and confirm the publish step has `at-revision`, `agent-session`, `transcript`, `tool-call` and `tool-use` refs.
- [ ] **Step 4: Commit** `"Document the Toolpath journal and export contract"`.
