# Agent Working Signal and Version Changelog Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let an agent tell the person, through Clax, that it is working on an artifact and on named comment threads, set automatically when comment feedback reaches it and explicitly through a new `working` tool. Show that state in the artifact header, on gallery cards, on sidebar thread cards and to the page (a Clax extension of the `comments` capability). Then use the same data to give every version a changelog: the threads it addressed and a short note from the agent. The changelog appears as a once-per-viewer banner, an "Addressed in vN" group in the sidebar with one-click Resolve, and a version menu that reads as a changelog across versions. Finally, let the person select several threads (checkboxes, Shift ranges, a bulk bar with an optional note, and "Send N unsent to agent") and send them as one batch. The agent receives them as one grouped delivery, led by the note, on every tier.

**Architecture:** The working state is an in-memory registry in the daemon (`clax_core::working::Working`), keyed by (session, artifact), with an injected clock and a 2 minute heartbeat expiry. The daemon marks work itself whenever it hands feedback to a session: every delivery tier already runs through `take_feedback`, so one hook point serves the Stop hook, the prompt hook, `SessionStart`, tier 1 piggyback, `wait_for_feedback`, Pi injection and Codex `codex queue`. Hooks and the Pi extension only renew (every tool call) and end the turn. Changes go out as one SSE event, `working`, carrying the artifact's whole list, and ride along on `GET /api/artifacts` and `GET /api/artifacts/<id>`, which the shell already loads. The changelog is persisted: a nullable `versions.note` column, a `version_threads` link table and a bounded `viewer_seen` table. Notes and links ride on the version and thread views the shell already loads. Only the viewer's seen mark is fetched after load. Batch send is one route and one store transaction (`send_batches`, `batch_threads`, `feedback.batch_id`). The grouping lives in the one payload renderer every tier already uses, `render_items`. The shell is the Svelte 5 shell that `docs/superpowers/plans/2026-09-29-svelte-port.md` produced. View state lives in `ArtifactController` (`web/shell/src/view/artifact-controller.ts`), the logic in framework-free `view/*-model.ts` modules, and the components in `web/shell/src/ui/*.svelte` islands, which read the controller's store with `fromStore` and `$derived`.

**Tech Stack:** Rust 2024 (axum 0.8, rusqlite, chrono, serde, rmcp), Svelte 5 (runes) + TypeScript + Vite 6, Vitest 3 + jsdom + `@testing-library/svelte` (through `web/shell/src/test/svelte.ts`), `svelte-check`, Playwright (Chromium), Python 3 (scripts), TypeBox (Pi extension, Pi 0.73.1).

**Spec:** `docs/superpowers/specs/2026-09-28-clax-design.md`. Task 1 amends §5 Storage, §6 HTTP API, §8 Shell UI, §9 Runtime bridge and capabilities, §10 Comments and the feedback loop, §11 Sessions, §12 MCP tool surface, §13 Plugins, §14 Security model and §16 Testing, and `docs/contract.md`.

**Decisions (binding):** `.superpowers/sdd/2026-09-30-agent-working/decisions.md`, all three sections ("Agent working signal", "Version changelog" and "Plan follow-ups").

**Precondition:** the rename plan (`docs/superpowers/plans/2026-09-29-clax-rename.md`) and the Svelte port (`docs/superpowers/plans/2026-09-29-svelte-port.md`) are both merged. The port runs before this plan. Check before Task 1:

```bash
test -f web/shell/src/view/artifact-controller.ts && test -f web/shell/src/ui/TopbarIsland.svelte \
  && test -f web/shell/src/view/boot.ts && test -f web/scripts/bundle-size.mjs \
  && test ! -e web/shell/src/artifact.tsx && ! grep -q '"preact"' web/package.json && echo ok
```

Expected: `ok`. Anything else means the port is not finished: stop.

## Global Constraints

- This plan starts from the post-port main: the Svelte shell, its two entries (`gallery-main.ts`, `artifact-main.ts`), the daemon's bootstrap block (`crates/clax-server/src/boot.rs`, `web/shell/src/view/boot.ts`), the lazy bridge parts, and the time-to-usable and bundle-size gates. File names below are the port's.
- Rust edition 2024. `cargo clippy --workspace --all-targets -- -D warnings` and `RUSTFLAGS=-Dwarnings cargo check --workspace` pass.
- `oxlint --deny-warnings` passes over `shell bridge e2e perf` and the config files (`npm run lint` in `web/`). `npm run typecheck` (`tsc --noEmit && svelte-check --fail-on-warnings`, a11y warnings included) passes. Svelte runs in runes mode only: no `export let`, no `svelte/legacy`. `web/shell/src/caps/**`, `web/shell/src/view/**` and `web/bridge/**` import no `svelte`: `grep -rlE "from \"svelte" web/bridge web/shell/src/caps web/shell/src/view` prints nothing.
- Every e2e spec that opens an artifact runs in both frame modes, `subdomain` and `sandbox` (`for (const mode of ["subdomain", "sandbox"] as const)`).
- Commits are signed. Commit with plain `git commit` (the repository's signing configuration applies). Never pass `--no-gpg-sign`. Stage with `git add` and explicit paths only. After each commit, `git cat-file commit HEAD | grep -q '^gpgsig '` must succeed (this machine has no `allowedSignersFile`, so `--show-signature` cannot verify).
- Never bind or connect to port 7480. Tests and smokes start daemons with `--port 0` and a temporary `CLAX_HOME`. Never read, write or delete the real `~/.clax`, `~/.claude` or `~/.codex`, nor the home directory Clax used before its rename (spec D15). `scripts/smoke-codex.sh` reads `~/.codex/auth.json`, so no task runs it; Task 7 only edits it, for the person to run.
- In prose, comments, doc comments and commit messages, write "ID", never "id", except as a literal symbol in code.
- Doc comments and commit messages describe the contract or the change. They never mention this plan, the conversation, or the history of names.
- Every task ends with `bash scripts/quality_gates.sh; echo "exit=$?"` printing `exit=0`. Check the status itself, not only the last line of output.
- The three skill copies stay word for word identical in their shared sections (`scripts/test-plugins.sh`). Tool blocks are regenerated with `python3 scripts/sync-skill-tools.py`, never edited by hand. A tool description is one string that appears verbatim in `plugins/pi/test/fixtures/contract.json`, `crates/clax-mcp/src/tools.rs` and `plugins/pi/src/clax.ts`.
- A working view never carries a session ID, a working directory or a PID, except the token-only `GET /api/sessions/<id>/working`. No thread, comment, event or page ever carries a session ID (spec §5, §14).
- The page never learns a thread's store ID. The capability hands it opaque handles only, and only for threads the page created in its current document.
- Time to usable does not regress. Nothing new is fetched before the shell's first paint. Working state and changelog notes ride on the artifact response, which the bootstrap block already embeds. The seen mark and the gallery's event stream start after load. The port's gates hold: `npm run perf` (budgets in `web/perf/budget.json`) and `node scripts/bundle-size.mjs` (`web/perf/bundle-budget.json`) pass unchanged. A budget is never raised. The changelog's banner and Changes components load by dynamic `import()`, off the entry's critical closure. If a gate fails, stop and report the numbers.
- UI changes are verified in a real browser (Task 11 and Task 18 have explicit steps). A passing test run is not verification for frontend work.
- UI logic lives in `web/shell/src/view/working-model.ts`, `web/shell/src/view/changelog-model.ts` and `ArtifactController`. The `.svelte` components only render and forward events. The one reactive module, `web/shell/src/ui/working-feed.svelte.ts`, holds the gallery's live working lists.
- Rust tests never sleep on the wall clock to observe expiry. They use `ManualClock` and call `sweep` directly.

## Review Focus

1. **A stale "working" after the agent stopped.** Every path that ends work must clear it: the reply to the last named thread, a publish of the artifact by that session, the Stop hook allowing the stop, Pi's `agent_end`, session end (PATCH, the reaper, the `SessionEnd` hook, the shim exiting) and artifact deletion. A path that is missed shows "working" for up to 2 minutes. Tests: `api_working_auto.rs` (one test per path) and the working steps Task 8 adds to the comment-loop smoke.
2. **Renewal that never lapses.** The shim's 60 s session heartbeat, the Pi injection long-poll, `wait_for_feedback` polls and `codex queue` must not renew a record. Otherwise a session that is merely alive shows "working" forever. Test: "a heartbeat and a wait poll do not renew".
3. **Session IDs leaking.** `GET /api/artifacts/<id>/working`, the `working` SSE event, `GET /api/artifacts` and the capability must never carry `session_id`. The record's `key` is a fresh ULID per record, not derived from the session. Test: "working views carry no session ID" (Rust) and the capability test.
4. **The capability leaking store IDs.** `working()` and `onWorking` name threads by the handles the page already holds, and count the rest. Test: "working() names only this document's own threads, by handle".
5. **The automatic changelog link.** A publish links the threads in the publishing session's record *before* the publish clears the record. Linking never resolves a thread. An agent resolve links to the current version only when the thread has no link yet. Tests: "a publish links the threads the session was working on, then clears", and "linking leaves the thread open".
6. **The banner showing twice, or never.** The seen mark moves forward only, only on unpinned views of the latest version, and is written as soon as the banner is decided (shown or not). A first visit sets the mark and shows nothing. Tests: e2e "seen state across reloads and viewers".
7. **The version menu replacing the native `<select>` in `TopbarIsland.svelte`.** Keyboard (Escape returns focus, rows are links in tab order), phone width (sheet under the top bar, no horizontal scroll) and version switching through `ctl.chooseVersion` must match what the select did (`viewer.spec.ts` is updated in Task 17, not weakened).
8. **The PostToolUse throttle.** `scripts/tool-hook.sh` must exit 0 and print nothing on every path, keep its stamp under the Clax home, and start `clax` at most once a minute per session. Test: `scripts/test-tool-hook.sh`.
9. **Time to usable.** The new eager code (the working status line, the version menu, the controller fields) stays within the port's bundle budget, and the changelog components load lazily. Tests: the `web bundle size` and `time to usable` gates.
10. **Batch atomicity and grouping.** A batch with any unknown, foreign or resolved thread writes nothing. A batch that is written wakes each target once, so no tier splits it. Tests: `api_batch.rs`, `api_push.rs` "a batch reaches codex as one queued message", and the hook, MCP and Pi goldens in Task 21.
11. **Selection state.** It lives only in the controller, is pruned whenever threads change, and never outlives a deleted artifact. Tests: `batch-model.test.ts`, the controller test "drops a thread from the selection when it disappears", and e2e "leaves the selection".

---

## File Structure

Rust:

| Path | Responsibility |
|---|---|
| `crates/clax-core/src/working.rs` | `Working` registry, `Clock`, `SystemClock`, `ManualClock`, `WorkingView`, `SessionWorking`, `clean_message`, TTL and bounds (Task 2) |
| `crates/clax-core/src/events.rs` | `Event::Working` (Task 2) |
| `crates/clax-core/src/changelog.rs` | `clean_note`, `MAX_NOTE_CHARS`, `MAX_ADDRESSES`, `LinkSource` (Task 12) |
| `crates/clax-core/src/store/changelog.rs` | Links, notes, seen marks (Task 12) |
| `crates/clax-core/src/store/migrations.rs` | Migration 10 (Task 12) |
| `crates/clax-server/src/working.rs` | `announce`, `sweep_and_announce`, `mark_items`, `renew_for_tier` (Tasks 3–4) |
| `crates/clax-server/src/routes/working.rs` | Working routes, and the debug-build clock skew route (Task 3) |
| `crates/clax-server/src/routes/viewers.rs` | Seen routes (Task 13) |
| `crates/clax-server/tests/api_working.rs`, `api_working_auto.rs`, `api_changelog.rs` | Route tests |
| `crates/clax-mcp/src/tools.rs`, `client.rs` | `working` tool; `publish` `addresses`/`note` (Tasks 5, 14) |
| `crates/clax-hooks/src/events.rs`, `crates/clax-cli/src/commands/hook.rs` | `tool` hook event; the Stop hook ends the turn (Task 7) |
| `crates/clax-cli/src/commands/publish.rs` | `--note`, `--addresses` (Task 14) |
| `crates/clax-core/src/store/batches.rs`, `crates/clax-core/src/feedback.rs` | Batch send, migration 11, the grouped payload (Task 19) |
| `crates/clax-server/tests/api_batch.rs` | The batch route (Task 20) |

Web (names follow the Svelte port's layout):

| Path | Responsibility |
|---|---|
| `web/shell/src/view/working-model.ts` | Pure: types, harness labels, header, badge and marker text (Task 9) |
| `web/shell/src/ui/WorkingStatus.svelte`, `WorkingBadge.svelte` | The header status line and the gallery badge (Task 9) |
| `web/shell/src/ui/working-feed.svelte.ts` | `WorkingFeed`: the gallery's live working lists (`$state`) (Task 9) |
| `web/shell/src/view/artifact-controller.ts` | `ViewState.working`, `changelog`, `changesOpen`, `changesFocus`; `dismissChangelog`, `showChanges` (Tasks 9, 17) |
| `web/shell/src/ui/TopbarIsland.svelte`, `SidebarIsland.svelte`, `StageIsland.svelte`, `Sidebar.svelte`, `ThreadCard.svelte`, `Gallery.svelte` | Wiring (Tasks 9, 17) |
| `web/shell/src/view/changelog-model.ts` | Pure: banner decision and text, sidebar groups, version rows (Task 16) |
| `web/shell/src/ui/ChangelogBanner.svelte`, `AddressedGroups.svelte` (lazy), `VersionMenu.svelte` | Changelog components (Task 17) |
| `web/shell/src/caps/comments.ts`, `caps/host.ts` | `working`, `watchWorking`, `unwatchWorking` calls; `CapEnv.working` (Task 10) |
| `web/bridge/src/caps/comments.ts` | `working()`, `onWorking(fn)` (Task 10) |
| `web/contract/0.2.61/comments.d.ts` | The typed Clax extension (Task 10) |
| `web/shell/src/view/batch-model.ts`, `web/shell/src/ui/BulkBar.svelte` | Batch selection model and the bulk bar (Task 22) |
| `web/e2e/working.spec.ts`, `web/e2e/changelog.spec.ts`, `web/e2e/batch.spec.ts` | Browser tests (Tasks 11, 18, 23) |

Plugins and scripts: `scripts/tool-hook.sh` and its copies `plugins/claude-code/scripts/tool-hook.sh`, `plugins/clax/scripts/tool-hook.sh`, `scripts/test-tool-hook.sh`, `plugins/pi/src/clax.ts`, `plugins/pi/src/client.ts`, `plugins/pi/test/clax.test.ts`, `plugins/pi/test/fixtures/contract.json`, `plugins/*/hooks/hooks.json`, `plugins/*/skills/clax/SKILL.md`, `plugins/*/README.md`, `scripts/test-plugins.sh`, `scripts/smoke-comment-loop.sh`, `scripts/smoke-codex.sh`.

---

## Design: the working record

### Why in memory

The record is kept in memory, in the daemon, and never written to SQLite:

- It is a claim about the present, true only while its session keeps renewing it, with a 2 minute lifetime. A restart kills every heartbeat that would renew it. A persisted row would be stale by definition after a restart and would need an explicit purge on start. An in-memory map starts empty, so "a daemon restart should not show stale work" holds by construction.
- It changes on every hook run and tool call (renewal). Writing that to SQLite would put a write transaction on the hot path of every tool call, behind the store's single connection mutex.
- Nothing needs its history. The changelog (Tasks 12–18) persists what matters, the threads a version addressed. It copies them out of the record at publish time.
- The session identity it needs (`session_id`, `harness`) is read from the persisted `sessions` table when the record is made.

### The record

| Field | Meaning |
|---|---|
| key (`session_id`, `artifact_id`) | One record per session per artifact |
| `key` (JSON) | A fresh ULID minted when the record is created; kept while it lives; never derived from the session |
| `harness` | `claude`, `codex` or `pi`, from the session row |
| `message` | Optional, at most 140 characters after whitespace is collapsed and control characters dropped; longer text is cut to 139 characters plus `…` |
| `thread_ids` | Optional, at most 20 open threads of the artifact, in the order first named |
| `started_at` | When the record was created (RFC 3339, milliseconds, UTC) |
| `last_heartbeat` | The latest renewal; the record lapses at `last_heartbeat + 120 s` |

### What sets, renews and clears it

| Event | Effect |
|---|---|
| Feedback handed to session S on artifact A by tier `piggyback`, `stop_hook`, `prompt_hook`, `wait`, `inject` (daemon, inside the feedback route) | mark: create or renew (S, A), add the items' thread IDs |
| `codex queue` exits 0 for S's claimed rows (daemon, `push::dispatch`) | mark, as above |
| `working` tool / `PUT /api/sessions/<S>/working/<A>` | set: create or renew; `message` and `thread_ids` replace the stored ones when given |
| A feedback take by tier `piggyback`, `stop_hook` or `prompt_hook`, even an empty one | renew every record of S |
| `POST /api/sessions/<S>/working/renew` (Claude Code and Codex `PostToolUse` hook, Pi `tool_call`) | renew every record of S |
| Agent reply or agent resolve by S on thread T of A | remove T from (S, A), renew S; a record left with no threads, after having had some, is cleared |
| Viewer resolve, any thread delete | remove T from every record on A (same rule) |
| Publish of A with `X-Clax-Session: S` | clear (S, A), after linking its threads to the new version (Task 13) |
| `working` tool with `done: true` / `DELETE /api/sessions/<S>/working/<A>` | clear (S, A), or only the named threads |
| Stop hook allows the stop (nothing blocks) / Pi `agent_end` → `POST /api/sessions/<S>/working/end` | clear every record of S |
| Session S ends (PATCH `ended`, reaper, `SessionEnd`, shim exit) | clear every record of S |
| Artifact A deleted | clear every record on A |
| `last_heartbeat + 120 s` passes | the record is invisible to reads at once, and the sweeper (every 5 s) removes it and announces the change |

Not renewals: the shim's 60 s `PATCH /api/sessions/<S>` heartbeat, `wait` and `inject` polls that return nothing, and `codex queue` runs. These mean the session is alive, not that it is working.

### What cannot be automatic, per harness

| Harness | Automatic | Not automatic, stated plainly |
|---|---|---|
| Claude Code | Marked when comments arrive by any tier. Renewed by tool calls (the `PostToolUse` hook, throttled to once a minute per session by `scripts/tool-hook.sh`) and every hook. Cleared by reply, publish, the Stop hook at the real end of a turn, and `SessionEnd`. | Work the person asks for in the terminal, not through a comment, is never marked: no hook knows which artifact a prompt is about. The agent must call `working`. Automatic marks carry no message. When the person interrupts a turn (Esc), Claude Code runs no Stop hook, so the mark stays until it lapses, up to 2 minutes later. Without the plugin's hooks (a plain `.mcp.json` install), only clax tool calls renew, so long work with other tools lapses after 2 minutes. |
| Codex | Marked when comments arrive (Stop hook, `SessionStart`, tier 1, `wait_for_feedback`, and `codex queue` on exit 0). Cleared by reply, publish, the Stop hook at turn end, and `SessionEnd`. | `codex queue` exiting 0 means "queued", not "seen". For a session with no TUI attached the mark is false and lapses after 2 minutes. Renewal by tool calls depends on Codex running the plugin's `PostToolUse` hook. Codex 0.159.0 names the event, but this is not measured yet. Task 7 adds the check to `scripts/smoke-codex.sh --hooks`, and the person runs it. The contract says "not yet measured" until their result is recorded. Until then, only clax tool calls and the Stop hook are known to renew. Hooks run only with `features.hooks = true` and after the person trusts them. `codex exec` skips untrusted hooks, so there is no turn-end clear: the mark lapses. No prompt hook is wired, so terminal requests are never marked. |
| Pi | Marked when comments arrive (tier 1, tier 5 injection, `wait_for_feedback`). Renewed by every tool call (`tool_call`, at most every 15 s). Cleared by reply, publish, `agent_end` and `session_shutdown`. | Terminal requests are never marked unless the agent calls `clax_working`. A Pi process that dies without `session_shutdown` leaves the mark to lapse (2 minutes). |

### What the person sees when several sessions work at once

The header shows the newest record (latest `started_at`) as `<Harness> is working: <message>`. With no message it reads `<Harness> is working on N comments` when the record names threads, else `<Harness> is working`. Then ` (+N more)` follows, and the element's `title` lists every record's line. The gallery badge reads `<Harness> working` for one record and `N agents working` for more. A thread card shows the marker of the newest record that names it.

## Design: the version changelog

### Storage

- `versions.note TEXT` (nullable): the agent's note for that version, at most 280 characters after whitespace is collapsed and control characters dropped. Longer notes are cut to 279 characters plus `…`, and the publish result says `note_truncated: true`.
- `version_threads(artifact_id, version_n, thread_id, source, created_at)`, primary key `(artifact_id, version_n, thread_id)`: a thread is addressed in a version. `source` is `working` (automatic at publish), `explicit` (`addresses` on publish) or `resolve` (an agent resolve with no earlier link). A thread may be linked to several versions. Deleting a thread deletes its links.
- `viewer_seen(viewer_id, artifact_id, seen_n, updated_at)`, primary key `(viewer_id, artifact_id)`: the highest version this viewer (the `clax_viewer` cookie's viewer row) has had the changelog decided for. It is monotonic. It is bounded to the 200 most recently updated artifacts per viewer, and older rows are pruned on write. Deleting an artifact deletes its rows.

### Linking rules

1. A publish of A by session S links every thread in S's working record on A (source `working`), then clears the record.
2. `addresses: [thread IDs]` on a publish links those threads (source `explicit`). Each must be a thread of A (else 400 `unknown_thread`, and nothing is published). There are at most 50 (else 400 `invalid_args`). Resolved threads may be named.
3. An agent resolve of thread T links T to A's current version (source `resolve`), but only when T has no link at all yet.
4. Linking never changes a thread's status. The viewer resolves it, one click from the changelog.

### Views

- Version views (`GET /api/artifacts/<id>`, `GET .../versions`, publish results) gain `note` (string or null) and `addresses` (thread IDs, in link order).
- Thread views gain `addressed_in` (version numbers, ascending).
- `GET /api/viewers/me/seen?artifact=<id>` answers `{seen: n | null}`. `PUT /api/viewers/me/seen` with `{artifact_id, version}` answers `{seen: n}`. These are viewer routes: no token, cookie required for PUT, foreign `Origin` refused.

### Banner decision (`view/changelog-model.ts` `decideBanner`)

Inputs: the versions, the viewer's `seen`, the latest version, and whether the view is pinned.

- Pinned view: no banner, and the mark is not written.
- `seen` is null (first visit): no banner. Write `seen = latest`.
- `seen >= latest`: no banner.
- One new version `vN`: with k addressed threads, `vN addressed k comment(s)`, followed by `: <note>` when there is a note. With no addressed threads and a note: `vN: <note>`. With neither: no banner.
- Several new versions (m): `m new versions, k comments addressed`. With k = 0 and at least one note: `m new versions: <latest note>`. With neither: no banner.
- Whatever the outcome, write `seen = latest` once decided. So the banner shows once per viewer per version.

### Time to usable

Notes and addresses ride on `GET /api/artifacts/<id>`, which the shell already awaits before it renders. The cost is one indexed query of `version_threads` per artifact and one more column. The seen mark is fetched after the viewer lookup that already runs after load, and the banner appears when it arrives. It is never on the path to first paint or to comment mode. The port's bootstrap block (`boot.rs` `assemble`) embeds that same artifact response and the thread views, so `working`, notes, `addresses` and `addressed_in` arrive in the HTML with no request at all. The seen mark stays an after-load fetch.

## Design: batch send to agent

Decisions: "Batch send to agent" in the decisions file.

### Route and access

`POST /api/artifacts/<aid>/threads:send` with `{thread_ids, note?}`. Access is exactly the single send's (`POST .../threads/<tid>/send`, `routes/threads.rs::send`), which the sidebar's "Send to agent" calls: no token, a foreign `Origin` refused (403 `forbidden_origin`), usable by LAN viewers. The viewer cookie names the sender (`viewer::author_name`: the display name, else `Viewer`).

### All or nothing

One SQLite transaction validates every thread, then writes every feedback row (each carrying the batch's ID), the `send_batches` row and its `batch_threads`. One `feedback::apply` then fans out for the whole batch, so every target's long-poll wakes once and `codex queue` runs once per target with every row. Errors, checked in this order, write nothing:

| Case | Answer |
|---|---|
| No threads, more than 20 (one working record's bound), or an ID that is not a ULID | 400 `invalid_args` |
| Note over 280 characters after whitespace is collapsed (the shell caps the field at 280) | 400 `note_too_long` |
| A thread that does not exist, was deleted, or is on another artifact | 400 `unknown_thread`, naming every such ID |
| A resolved thread | 400 `thread_resolved`, naming every such ID (as the single send refuses one) |
| Every thread already sent with nothing new to send | 409 `nothing_to_send` |
| Unknown or deleted artifact | 404 `not_found` |

Duplicate IDs collapse. An already-sent thread is accepted, as the single send is idempotent. It sends only its viewer comments that have no feedback row yet, and is reported in `unchanged` when that is none. The batch holds the threads in `sent`.

### Delivery

Every tier renders through `render_items`. A run of items from one batch is led by one line: `[clax] N comments on "<title>", sent together by <name>.`, followed by ` Note: "<note>"` when there is a note (JSON-quoted, like comment bodies). So the piggyback, the Stop hook, the prompt hook, `wait_for_feedback`, `codex queue` and Pi's `sendUserMessage` each hand the agent one grouped delivery, note first. Each thread keeps its own feedback rows, so sent state, acknowledgement, resends, the working marker and the changelog link stay per thread. A delivered batch marks every thread working through the usual `mark_items`. A publish then links every thread in the record.

### What the person sees

Checkboxes on open thread cards, Shift-click ranges, a sticky bulk bar (`N selected · Send to agent · Clear`, with an optional note, sent with Cmd+Enter or Ctrl+Enter), and a `Send N unsent to agent` button at the sidebar top. Each sent thread's history shows the send (`Sent to agent by Alex with 2 others · "note"`), from the thread view's `sends`.

### The page capability: no batch `sendToClaude`

Recommendation: **no**. The `comments` capability keeps one-thread `sendToClaude`. Reasons:
- claude.ai's contract has no batch verb, and adding one moves Clax's copy further from pages written for claude.ai.
- A page may only send threads it created in this document, so a batch adds little over calling `sendToClaude` per thread.
- Every call sits in the strict gesture tier (`frameGestureStrict`, `caps/gesture.ts`) and the 10-writes-a-minute budget. A batch verb would let one gesture, possibly a forged one within the activation window, send up to 20 threads.

The viewer's batch is the sidebar's. The spec and the contract say so in Task 1.

---

---

### Task 1: Spec and contract amendments

Docs only. The tool-count lists (`Twenty-two tools:` in `docs/contract.md` and the READMEs) are not touched here. `scripts/sync-skill-tools.py --check` compares them with the fixture, which gains `working` only in Task 5.

**Files:**
- Modify: `docs/superpowers/specs/2026-09-28-clax-design.md` (§5, §6, §8, §9, §10, §11, §12, §13, §14, §15, §16)
- Modify: `docs/contract.md` (`### publish`, `## Sessions`, `### The comments capability`, `### Tools` under "Comments and feedback", `### What the person sees`)

**Interfaces:** none (documentation of Tasks 2–23).

- [ ] **Step 1: §5 Storage and data model**

In the `versions(...)` bullet, replace `session_id, files_json)` with `session_id, files_json, note)`, and append to that bullet: ``; `note` is the agent's short change note for the version (at most 280 characters, null when none).``

After the `session_env(...)` bullet, add:

```markdown
- `version_threads(artifact_id, version_n, thread_id, source, created_at)`:
  the threads a version addressed (spec §10, "Version changelog"); `source`
  is `working`, `explicit` or `resolve`. Deleting a thread deletes its links.
  Linking never changes a thread's status.
- `viewer_seen(viewer_id, artifact_id, seen_n, updated_at)`: the highest
  version whose changelog was decided for this viewer (the `clax_viewer`
  cookie's row). Monotonic; at most 200 rows per viewer (the least recently
  updated are pruned on write); deleting an artifact deletes its rows.

Working records (§10, "Working") are not stored: the daemon keeps them in
memory, so a restart starts with none.
```

- [ ] **Step 2: §6 HTTP API**

After the `GET /api/events?artifact=<aid>` bullet, add:

```markdown
- The event stream also carries `working` (`{artifact_id, working: [view]}`,
  the artifact's whole list after any change, §10 "Working"). An optional
  `types=<name>,<name>` narrows the stream to those event names (`ready` and
  `resync` are always sent); the gallery opens `?types=working` with no
  `artifact` filter.
```

After the `Feedback:` bullet, add:

```markdown
- Working (§10, "Working"): `GET /api/artifacts/<aid>/working` (no token;
  `{working: [view]}`), `GET /api/sessions/<sid>/working` (token; views plus
  `session_id`, `artifact_id`, `expires_at`), `PUT
  /api/sessions/<sid>/working/<aid>` (W; `{thread_ids?, message?}`; `{working,
  message_truncated}`), `DELETE /api/sessions/<sid>/working/<aid>` (W;
  optional `?thread_ids=<id>,<id>`; `{cleared, working}`), `POST
  /api/sessions/<sid>/working/renew` (W; `{renewed}`), `POST
  /api/sessions/<sid>/working/end` (W, turn end; `{cleared}`). A view is
  `{key, harness, message, thread_ids, started_at, last_heartbeat}`; `key` is
  a ULID minted for the record, never a session ID. `GET /api/artifacts` and
  `GET /api/artifacts/<aid>` carry each artifact's `working` list.
- Changelog (§10, "Version changelog"): version views carry `note` and
  `addresses` (thread IDs); thread views carry `addressed_in` (version
  numbers). `GET /api/viewers/me/seen?artifact=<aid>` (`{seen}`) and `PUT
  /api/viewers/me/seen` (`{artifact_id, version}`, monotonic; `{seen}`) are
  viewer routes (no token, foreign `Origin` refused; PUT without a viewer
  cookie is 400 `no_viewer`).
```

In "### Publish body", add these two lines to the JSON example after the `"label"` line:

```json
  "note": "Two columns; third bullet dropped",  // optional, ≤ 280 chars, longer is cut
  "addresses": ["01J9..."],                    // optional, ≤ 50 threads of this artifact
```

and after the example add: ``A thread in `addresses` that is not a thread of the artifact is 400 `unknown_thread`, and nothing is published.``

- [ ] **Step 3: §8 Shell UI and viewer**

Append to the Gallery paragraph: ``A card whose artifact has working records shows a badge with a pulsing dot: `<Harness> working`, or `N agents working`.``

Replace the Header bullet with:

```markdown
- Header: title, the working status line (below), version menu (`v3 of 3`;
  a button opening a panel that lists every version newest first with its
  time, label, note and "addressed N" count, each a link to that version;
  older versions read-only), copy link, open raw content in a new tab, comment
  mode toggle, thread sidebar toggle, viewer display name.
- Working status line: a pulsing dot and `<Harness> is working: <message>`
  for the newest working record (`on N comments` when it has no message but
  names threads; nothing more when it has neither), then `(+N more)` with every
  record's line in the element's `title`. It is a polite live region whose
  text changes only when the set of records changes, so renewals are silent.
  At phone width only the dot and `<Harness> is working` show. The dot does
  not pulse under `prefers-reduced-motion: reduce`.
- Changelog banner: on loading the latest version unpinned, once per viewer
  per version (`viewer_seen`), `v5 addressed 3 comments: <note>` for one new
  version, or `3 new versions, 7 comments addressed` for several, with Show
  (opens the sidebar at its Changes section) and Dismiss. A first visit shows
  none.
```

In the Thread sidebar bullet, append: ``An open thread named by a working record shows `<Harness> is working…` in place of its waiting indicator until the record drops it (the agent's reply arrives at the same moment). Above the threads, a Changes section lists "Addressed in vN" groups, newest first, each thread as a row that jumps to its anchor with the existing flash (a static outline under reduced motion) and carries a Resolve button while open; the groups for versions new since the viewer's last visit start expanded.``

- [ ] **Step 4: §9 comments capability**

Append to the **comments** bullet: ``Clax extension, not part of claude.ai's contract: `working()` resolves `{working, agents: [{harness, label, message, since, threads, otherThreads}]}`, and `onWorking(fn)` calls `fn` with that state now and on every change and resolves an unsubscribe function. `threads` holds the handles of the threads this document created that the agent names, and `otherThreads` counts the rest. Both are available under either declaration form, need no consent or gesture, and never carry a store ID, session ID or record key.``

- [ ] **Step 5: §10 Comments and the feedback loop**

After "### Watch semantics" and its paragraphs, add:

```markdown
### Working

A working record says that a harness session is acting on an artifact, and
optionally on some of its threads, now. The daemon keeps records in memory
(a restart starts with none) keyed by session and artifact; each holds a
`key` (a ULID), the session's `harness`, an optional `message` (at most 140
characters; longer is cut), up to 20 open thread IDs, `started_at` and
`last_heartbeat`. A record lapses 120 s after its last heartbeat: reads stop
showing it at once, and a sweep every 5 s removes it and sends `working`.

- Marked (created, or renewed with the threads added) whenever the daemon
  hands feedback to the session by tier `piggyback`, `stop_hook`,
  `prompt_hook`, `wait` or `inject`, and when `codex queue` exits 0.
- Set explicitly by the `working` tool.
- Renewed by every feedback take of tier `piggyback`, `stop_hook` or
  `prompt_hook`, by `POST .../working/renew` (the `PostToolUse` hooks and
  Pi's `tool_call`), by the `working` tool, and by the session's agent replies
  and resolves. The shim's session heartbeat, `wait` and `inject` polls and
  `codex queue` never renew.
- Cleared by: the session's agent reply or resolve on the last thread a
  record names; a publish of the artifact by the session; `done: true`; the
  turn ending (the Stop hook allowing the stop, Pi's `agent_end`); the
  session ending; the artifact's deletion. A viewer resolve or a delete takes
  the thread out of every record, with the same last-thread rule.

What cannot be automatic, per harness, is listed in `docs/contract.md`
("Working").

### Version changelog

Each version may carry a `note` and a set of threads it addressed. A
publish by a session links the threads of that session's working record on
the artifact (then clears the record); `addresses` names more; an agent
resolve links the thread to the current version when it has no link yet.
Linking never resolves: the viewer does, from the Changes section.
```

- [ ] **Step 6: §11 Sessions and identity**

Append to the "Heartbeats:" paragraph: ``The session heartbeat keeps the session row alive only; it never renews working records (§10, "Working").``

- [ ] **Step 7: §12 MCP tool surface**

In the Artifacts paragraph, after `label;` in the `publish (...)` list, insert ``note (the version's change note), addresses (thread IDs this version addresses);``. In the Comments paragraph, replace `` `comments_resolve`
(url_or_id, thread_id), `` with `` `comments_resolve`
(url_or_id, thread_id; an agent resolve links the thread to the current version when it has no link yet), `working` (url_or_id, thread_ids, message, done), ``.

- [ ] **Step 8: §13 Plugins**

In the Claude Code `hooks/hooks.json` bullet, replace `` `Stop` →
  `hook --agent claude stop` (tier 2, timeout 10 s); `` with `` `Stop` →
  `hook --agent claude stop` (tier 2, timeout 10 s; when it allows the stop it
  ends the turn's working records); `PostToolUse` → `scripts/tool-hook.sh claude`
  (renews working records at most once a minute per session: a shell check
  of a stamp file under `~/.clax/run/tool-hook/` skips starting `clax` when
  the last renewal is under 60 s old; always exits 0; timeout 5 s); ``. In the Codex `hooks/hooks.json` bullet, after `` `Stop` → the same with `stop` `` add `` (which also ends the turn's working records when it allows the stop), `PostToolUse` → `bash "${PLUGIN_ROOT}/scripts/tool-hook.sh" codex` (the same once-a-minute renewal; not yet measured on Codex, see `docs/contract.md`) ``. In the Pi bullets, replace `for the fourteen tools` with `for the twenty-three tools`, and add a bullet: ``- Working: `tool_call` renews the session's working records (at most once every 15 s), and `agent_end` ends them.``

- [ ] **Step 9: §14, §15, §16**

§14, add a bullet: ``- Working views and events carry the record's `key`, `harness`, `message`, thread IDs and times, and are readable without the token, like threads: LAN viewers see which harness is working and its message. They never carry a session ID, working directory or PID. Messages and version notes are agent text, rendered by the shell as text only.``

§15, in "Hook timeouts", after `prompt submit 4 s` add `, tool 2 s (1 s per daemon request)`.

§16, append to the **clax-server** bullet: ``, working records (set, mark, renew, clear, expiry by an injected clock), the changelog links and seen marks``. Append to **Browser**: ``, the working status line, gallery badge, thread marker and capability, and the changelog banner, Changes section and version menu``.

- [ ] **Step 10: `docs/contract.md`**

In `### publish`, add two rows to the arguments table after `label`:

```markdown
| `note` | string | no | This version's change note for the person, at most 280 characters (longer is cut, with `note_truncated: true`). |
| `addresses` | array of thread IDs | no | Threads of this artifact this version addresses (at most 50). Linking does not resolve them. |
```

and after the table's paragraph about omitted fields, add: ``The result also carries `note` and `addressed` (every thread linked to the new version: those named, plus those this session was marked working on for the artifact).``

At the end of `## Sessions` (before `### Claude Code`), add:

```markdown
The hooks also keep working records current: the `stop` hook, when it allows
the stop, ends the turn (`POST /api/sessions/<id>/working/end`), and the
`PostToolUse` hook renews them (`POST /api/sessions/<id>/working/renew`) at
most once a minute per session. The hook command is `scripts/tool-hook.sh`,
a POSIX shell gate. It reads the hook input, takes the `session_id`, and
exits 0 without starting `clax` when the stamp file
`$CLAX_HOME/run/tool-hook/<harness>-<session ID>` (default home `~/.clax`)
was modified less than 60 s ago. Otherwise it touches the stamp and runs
`clax hook --agent <harness> tool` (2 s, 1 s per request). It always exits 0
and prints nothing. The `session-end` hook removes the session's stamp.
```

In "Comments and feedback" → `### Tools`, add a table row:

```markdown
| `working` | `url_or_id`; optional `thread_ids` (at most 20 open threads), `message` (at most 140 characters), `done` | `{artifact_id, url, working: true, message, thread_ids, started_at, expires_in_s, message_truncated}` or `{artifact_id, url, working: false, cleared}` |
```

and after that table's paragraphs add this subsection:

```markdown
### Working

`working` tells the person you are acting on an artifact. The page's header
shows `<Harness> is working: <message>`, its gallery card a badge, and each
thread named in `thread_ids` `<Harness> is working…`. Comments sent to you
mark you working automatically; call `working` for work that did not start
from a comment, or to add a message. `thread_ids` and `message` replace the
stored ones when given; `done: true` clears the record, or with `thread_ids`
only those threads. Errors: `invalid_id`, `invalid_args` (a thread ID that is
not a ULID, more than 20), `not_found` (no such artifact), `unknown_thread`
(not a thread of the artifact), `thread_not_open`, `no_session` (the
daemon's `/mcp`), `unknown_session`, `daemon_unreachable`.

It clears when you reply to or resolve the last thread it names, publish the
artifact, end your turn, or go 120 s without renewing it, and when your
session ends. Renewal is automatic:

| Harness | Renewed by | Not automatic |
|---|---|---|
| Claude Code | tool calls, at most once a minute (`PostToolUse` hook), every hook, clax tool calls | work asked for in the terminal (call `working`); a turn you interrupt with Esc runs no Stop hook, so its mark lapses within 2 minutes |
| Codex | clax tool calls, the Stop hook; tool calls through the `PostToolUse` hook: not yet measured (`scripts/smoke-codex.sh --hooks`) | a `codex queue` delivery to a session with no TUI marks it for up to 2 minutes; `codex exec` without trusted hooks never ends the turn; terminal requests |
| Pi | tool calls (at most every 15 s) | terminal requests; a Pi process killed without `session_shutdown` |
```

In `### The comments capability`, add at its end:

```markdown
Clax extension (not in claude.ai's contract): `working()` and
`onWorking(fn)` report which agents are working on this artifact:
`{working: boolean, agents: [{harness, label, message, since, threads,
otherThreads}]}`. `threads` are handles of threads this document created;
`otherThreads` counts the rest. Available under either declaration form,
without consent or gesture.
```

In `### What the person sees`, add at its end:

```markdown
An open thread a working record names shows "<Harness> is working…" instead
of its waiting indicator. On loading a new version, the viewer sees the
changelog banner once ("v5 addressed 3 comments: <note>", or a summary of
several versions); the sidebar's Changes section lists each version's
addressed threads with Resolve; the version menu lists every version's note.
```

- [ ] **Step 11: Batch send to agent (spec and contract)**

Spec §5: in the `feedback(...)` bullet, add `batch_id` to the column list, and append: ``; `batch_id` names the batch send that created the row, if any.`` After the `viewer_seen` bullet from Step 1, add:

```markdown
- `send_batches(id, artifact_id, note, sent_by, size, created_at)` and
  `batch_threads(batch_id, thread_id)`: batch sends to the agent (§10,
  "Batch send"); `sent_by` is the sender's display name as a comment author
  gets it, never a cookie. Deleting a thread or its artifact deletes its rows.
```

Spec §6: after the `.../threads/<tid>/send` sentence in the Comments bullet, add: ``POST .../threads:send`` (no token; foreign `Origin` refused, as the single send) takes `{thread_ids, note?}` and sends 1 to 20 threads as one batch, all or nothing. It answers `{batch, sent, unchanged, threads}`, or 400 `invalid_args` / `note_too_long` / `unknown_thread` / `thread_resolved`, or 409 `nothing_to_send`, and writes nothing on any error. Thread views carry `sends`, the batches that sent them.``

Spec §8: in the Thread sidebar bullet, append: ``Open thread cards carry a checkbox (Shift-click selects a range). While any is checked, a sticky bar at the sidebar top reads `N selected · Send to agent · Clear` and holds an optional one-line note (Cmd+Enter or Ctrl+Enter sends). A `Send N unsent to agent` button shows whenever open threads have not been sent. A sent thread's history shows its batch send and note. A thread that disappears leaves the selection.``

Spec §9: append to the **comments** bullet: ``There is no batch form of `sendToClaude`: a page sends threads it created one call at a time, each in the strict gesture tier; batches are the viewer's, from the sidebar.`` In the Clax extension sentence from Step 4, after `under either declaration form`, insert ``(for `composer_only`, a Clax extension to that form, which otherwise grants only `openComposer` and `anchorFor`)``.

Spec §10: after "### Version changelog", add:

```markdown
### Batch send

The viewer can send several threads at once (`POST .../threads:send`). One
transaction checks every thread and writes every feedback row, marked with
the batch; one fan-out follows, so every tier hands the batch over together.
The payload leads the batch's comments with `[clax] N comments on "<title>",
sent together by <name>.` and ` Note: "<note>"` when there is one. Each
thread keeps its own rows, sent state, working marker and changelog link.
```

`docs/contract.md`, "Comments and feedback" → `### Payload`: append:

```markdown
Comments the person sent together arrive together. After the counted
header, a line `[clax] N comments on "<title>", sent together by <name>.`
(and ` Note: "<note>"`, the note JSON-quoted like a comment body) leads the
batch's items. The note is the person's words for the whole batch, and like
comment text it is a request to weigh.
```

and in `### The comments capability`, add: ``Clax adds no batch `sendToClaude`; the batch send is the viewer's, from the sidebar.``

- [ ] **Step 12: Check and commit**

Run: `grep -n "fourteen tools" docs/superpowers/specs/2026-09-28-clax-design.md; bash scripts/test-plugins.sh | tail -1`
Expected: no `fourteen tools` line; `plugin checks passed`.

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add docs/superpowers/specs/2026-09-28-clax-design.md docs/contract.md
git commit -m "Specify the agent working signal, the version changelog and batch send to agent"
git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed
```

---

### Task 2: The working registry in clax-core

**Files:**
- Create: `crates/clax-core/src/working.rs`
- Modify: `crates/clax-core/src/lib.rs`, `crates/clax-core/src/events.rs`

**Interfaces:**
- Produces: `clax_core::working::{WORKING_TTL_SECS: i64 = 120, MAX_MESSAGE_CHARS: usize = 140, MAX_WORKING_THREADS: usize = 20, Clock, SystemClock, ManualClock, Actor, WorkingView, SessionWorking, SetWorking, Changed, Working, clean_message}`.
- `Working` methods: `new(clock: Arc<dyn Clock>) -> Working`, `now() -> DateTime<Utc>`, `skew(secs: i64)`, `set(&Actor, aid, SetWorking) -> (SessionWorking, Changed)`, `mark(&Actor, aid, &[String]) -> Changed`, `renew(sid) -> usize`, `clear(sid, aid, Option<&[String]>) -> Changed`, `thread_done(sid, aid, tid) -> Changed`, `thread_gone(aid, tid) -> Changed`, `end_session(sid) -> Changed`, `artifact_gone(aid) -> Changed`, `sweep() -> Changed`, `for_artifact(aid) -> Vec<WorkingView>`, `for_session(sid) -> Vec<SessionWorking>`, `threads_of(sid, aid) -> Vec<String>`, `all() -> BTreeMap<String, Vec<WorkingView>>`.
- Produces: `Event::Working { artifact_id: String, working: Vec<WorkingView> }`, SSE name `working`.

- [ ] **Step 1: Write the failing tests**

Create `crates/clax-core/src/working.rs` with only this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (Arc<ManualClock>, Working) {
        let clock = Arc::new(ManualClock::at("2026-09-30T10:00:00Z"));
        (clock.clone(), Working::new(clock))
    }
    fn claude(sid: &str) -> Actor {
        Actor { session_id: sid.into(), harness: "claude".into() }
    }
    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn mark_creates_then_adds_threads_and_keeps_the_key() {
        let (_c, w) = fixture();
        assert_eq!(w.mark(&claude("s1"), "a1", &ids(&["t1"])).0, ["a1".to_string()].into());
        let key = w.for_artifact("a1")[0].key.clone();
        w.mark(&claude("s1"), "a1", &ids(&["t2", "t1"]));
        let v = w.for_artifact("a1");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].thread_ids, ids(&["t1", "t2"]));
        assert_eq!(v[0].key, key);
        assert_eq!(v[0].harness, "claude");
        assert_eq!(v[0].message, None);
    }

    #[test]
    fn set_replaces_given_fields_and_keeps_started_at() {
        let (c, w) = fixture();
        w.mark(&claude("s1"), "a1", &ids(&["t1"]));
        let started = w.for_artifact("a1")[0].started_at.clone();
        c.advance(30);
        let (s, changed) = w.set(&claude("s1"), "a1", SetWorking { thread_ids: None, message: Some("Tightening spacing".into()) });
        assert!(!changed.is_empty());
        assert_eq!(s.view.message.as_deref(), Some("Tightening spacing"));
        assert_eq!(s.view.thread_ids, ids(&["t1"]));
        assert_eq!(s.view.started_at, started);
        assert_eq!(s.view.last_heartbeat, "2026-09-30T10:00:30.000Z");
        assert_eq!(s.expires_at, "2026-09-30T10:02:30.000Z");
        let (s, _) = w.set(&claude("s1"), "a1", SetWorking { thread_ids: Some(vec![]), message: None });
        assert!(s.view.thread_ids.is_empty());
        assert_eq!(s.view.message.as_deref(), Some("Tightening spacing"));
    }

    #[test]
    fn records_lapse_120_s_after_the_last_renewal() {
        let (c, w) = fixture();
        w.mark(&claude("s1"), "a1", &[]);
        c.advance(119);
        assert_eq!(w.for_artifact("a1").len(), 1);
        assert_eq!(w.renew("s1"), 1);
        c.advance(119);
        assert_eq!(w.for_artifact("a1").len(), 1, "renewed at 119 s");
        c.advance(1);
        assert!(w.for_artifact("a1").is_empty(), "hidden at 120 s, before any sweep");
        assert_eq!(w.sweep().0, ["a1".to_string()].into());
        assert!(w.sweep().is_empty(), "swept once");
        assert_eq!(w.renew("s1"), 0, "a lapsed record is not revived");
    }

    #[test]
    fn replying_to_the_last_named_thread_clears_the_record() {
        let (_c, w) = fixture();
        w.mark(&claude("s1"), "a1", &ids(&["t1", "t2"]));
        w.thread_done("s1", "a1", "t1");
        assert_eq!(w.for_artifact("a1")[0].thread_ids, ids(&["t2"]));
        assert!(!w.thread_done("s1", "a1", "t2").is_empty());
        assert!(w.for_artifact("a1").is_empty());
    }

    #[test]
    fn a_record_that_never_named_threads_survives_a_reply() {
        let (_c, w) = fixture();
        w.set(&claude("s1"), "a1", SetWorking { thread_ids: None, message: Some("Refactoring".into()) });
        assert!(w.thread_done("s1", "a1", "t9").is_empty());
        assert_eq!(w.for_artifact("a1").len(), 1);
    }

    #[test]
    fn thread_gone_touches_every_session_on_the_artifact() {
        let (_c, w) = fixture();
        w.mark(&claude("s1"), "a1", &ids(&["t1"]));
        w.mark(&Actor { session_id: "s2".into(), harness: "codex".into() }, "a1", &ids(&["t1", "t2"]));
        w.thread_gone("a1", "t1");
        let v = w.for_artifact("a1");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].harness, "codex");
        assert_eq!(v[0].thread_ids, ids(&["t2"]));
    }

    #[test]
    fn end_session_clear_and_artifact_gone() {
        let (_c, w) = fixture();
        w.mark(&claude("s1"), "a1", &[]);
        w.mark(&claude("s1"), "a2", &[]);
        w.mark(&claude("s2"), "a2", &[]);
        assert_eq!(w.end_session("s1").0, ["a1".to_string(), "a2".to_string()].into());
        assert_eq!(w.for_artifact("a2").len(), 1);
        assert!(!w.clear("s2", "a2", None).is_empty());
        w.mark(&claude("s3"), "a3", &[]);
        assert!(!w.artifact_gone("a3").is_empty());
        assert!(w.all().is_empty());
    }

    #[test]
    fn clear_with_threads_removes_only_those() {
        let (_c, w) = fixture();
        w.mark(&claude("s1"), "a1", &ids(&["t1", "t2"]));
        w.clear("s1", "a1", Some(&ids(&["t1"])));
        assert_eq!(w.threads_of("s1", "a1"), ids(&["t2"]));
    }

    #[test]
    fn views_are_newest_first_and_carry_no_session_id() {
        let (c, w) = fixture();
        w.mark(&claude("s1"), "a1", &[]);
        c.advance(1);
        w.mark(&Actor { session_id: "s2".into(), harness: "pi".into() }, "a1", &[]);
        let v = w.for_artifact("a1");
        assert_eq!(v[0].harness, "pi");
        let json = serde_json::to_string(&v).unwrap();
        assert!(!json.contains("s1") && !json.contains("s2") && !json.contains("session"), "{json}");
        assert_ne!(v[0].key, v[1].key);
        assert_eq!(w.for_session("s2")[0].session_id, "s2");
        assert_eq!(w.for_session("s2")[0].artifact_id, "a1");
    }

    #[test]
    fn skew_moves_the_registry_clock() {
        let (_c, w) = fixture();
        w.mark(&claude("s1"), "a1", &[]);
        w.skew(121);
        assert!(w.for_artifact("a1").is_empty());
    }

    #[test]
    fn messages_are_one_line_and_bounded() {
        assert_eq!(clean_message("  a\n\tb\u{2028}c\u{7}d  "), (Some("a b cd".into()), false));
        assert_eq!(clean_message(" \n "), (None, false));
        let long = "x".repeat(141);
        let (m, cut) = clean_message(&long);
        assert!(cut);
        assert_eq!(m.as_deref().unwrap().chars().count(), 140);
        assert!(m.unwrap().ends_with('…'));
        assert_eq!(clean_message(&"é".repeat(140)), (Some("é".repeat(140)), false));
    }
}
```

Add `pub mod working;` to `crates/clax-core/src/lib.rs`.

Run: `cargo test -p clax-core working`
Expected: FAIL to compile (`Working`, `ManualClock`, ... not found).

- [ ] **Step 2: Implement the registry**

Put this above the test module in `working.rs`:

```rust
//! The working signal (spec §10 "Working"): which harness session is acting
//! on which artifact, and on which of its threads, now. Records live in
//! memory only. Each lapses [`WORKING_TTL_SECS`] after its last renewal, so a
//! daemon restart starts with none and never shows stale work.

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

/// Seconds after its last renewal that a record lapses.
pub const WORKING_TTL_SECS: i64 = 120;
/// Longest message, in characters, after [`clean_message`].
pub const MAX_MESSAGE_CHARS: usize = 140;
/// Most threads one record names.
pub const MAX_WORKING_THREADS: usize = 20;

/// Where the registry reads the time.
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

/// The system clock.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// A clock that moves only when told to.
pub struct ManualClock(Mutex<DateTime<Utc>>);

impl ManualClock {
    /// A clock stopped at `rfc3339`.
    ///
    /// # Panics
    /// When `rfc3339` does not parse.
    pub fn at(rfc3339: &str) -> ManualClock {
        ManualClock(Mutex::new(
            DateTime::parse_from_rfc3339(rfc3339).expect("an RFC 3339 time").with_timezone(&Utc),
        ))
    }
    pub fn advance(&self, secs: i64) {
        *self.0.lock().unwrap() += Duration::seconds(secs);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> DateTime<Utc> {
        *self.0.lock().unwrap()
    }
}

/// The session a record belongs to, as the daemon knows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Actor {
    pub session_id: String,
    pub harness: String,
}

/// A record as anyone may read it: never names the session.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WorkingView {
    /// A ULID minted when the record was created.
    pub key: String,
    pub harness: String,
    pub message: Option<String>,
    pub thread_ids: Vec<String>,
    pub started_at: String,
    pub last_heartbeat: String,
}

/// A record as its session's token holder reads it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SessionWorking {
    #[serde(flatten)]
    pub view: WorkingView,
    pub session_id: String,
    pub artifact_id: String,
    pub expires_at: String,
}

/// An explicit update: each `Some` field replaces the stored one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SetWorking {
    pub thread_ids: Option<Vec<String>>,
    pub message: Option<String>,
}

/// The artifacts whose working list changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changed(pub BTreeSet<String>);

impl Changed {
    pub fn merge(&mut self, other: Changed) {
        self.0.extend(other.0);
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    fn one(aid: &str) -> Changed {
        Changed([aid.to_string()].into())
    }
}

struct Record {
    key: String,
    harness: String,
    message: Option<String>,
    threads: Vec<String>,
    /// The record has named a thread at some point: losing its last one clears it.
    had_threads: bool,
    started_at: DateTime<Utc>,
    heartbeat: DateTime<Utc>,
}

fn stamp(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Millis, true)
}

impl Record {
    fn view(&self) -> WorkingView {
        WorkingView {
            key: self.key.clone(),
            harness: self.harness.clone(),
            message: self.message.clone(),
            thread_ids: self.threads.clone(),
            started_at: stamp(self.started_at),
            last_heartbeat: stamp(self.heartbeat),
        }
    }
    fn add(&mut self, threads: &[String]) {
        for t in threads {
            if !self.threads.contains(t) && self.threads.len() < MAX_WORKING_THREADS {
                self.threads.push(t.clone());
            }
        }
        self.had_threads |= !self.threads.is_empty();
    }
}

/// `raw` as one line: whitespace runs (line and paragraph separators included)
/// become one space, other control characters are dropped, the ends are
/// trimmed. Empty is `None`. Past [`MAX_MESSAGE_CHARS`] it is cut to one
/// character less plus `…`; the flag says so.
pub fn clean_message(raw: &str) -> (Option<String>, bool) {
    let kept: String = raw.chars().filter(|c| c.is_whitespace() || !c.is_control()).collect();
    let one = kept.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.is_empty() {
        return (None, false);
    }
    if one.chars().count() <= MAX_MESSAGE_CHARS {
        return (Some(one), false);
    }
    let mut cut: String = one.chars().take(MAX_MESSAGE_CHARS - 1).collect();
    cut.push('…');
    (Some(cut), true)
}

type Key = (String, String);

/// The registry. Every method takes the lock once; none blocks on I/O.
pub struct Working {
    clock: Arc<dyn Clock>,
    skew: Mutex<Duration>,
    records: Mutex<BTreeMap<Key, Record>>,
}

impl Working {
    pub fn new(clock: Arc<dyn Clock>) -> Working {
        Working { clock, skew: Mutex::new(Duration::zero()), records: Mutex::new(BTreeMap::new()) }
    }

    /// The registry's time: its clock plus any [`Working::skew`].
    pub fn now(&self) -> DateTime<Utc> {
        self.clock.now() + *self.skew.lock().unwrap()
    }

    /// Moves the registry's time forward by `secs` (the debug build's test route).
    pub fn skew(&self, secs: i64) {
        *self.skew.lock().unwrap() += Duration::seconds(secs);
    }

    fn live(&self, r: &Record, now: DateTime<Utc>) -> bool {
        now - r.heartbeat < Duration::seconds(WORKING_TTL_SECS)
    }

    fn upsert<'a>(map: &'a mut BTreeMap<Key, Record>, who: &Actor, aid: &str, now: DateTime<Utc>) -> &'a mut Record {
        let r = map.entry((who.session_id.clone(), aid.to_string())).or_insert_with(|| Record {
            key: crate::new_ulid(),
            harness: who.harness.clone(),
            message: None,
            threads: Vec::new(),
            had_threads: false,
            started_at: now,
            heartbeat: now,
        });
        r.heartbeat = now;
        r
    }

    fn session_view(sid: &str, aid: &str, r: &Record) -> SessionWorking {
        SessionWorking {
            view: r.view(),
            session_id: sid.to_string(),
            artifact_id: aid.to_string(),
            expires_at: stamp(r.heartbeat + Duration::seconds(WORKING_TTL_SECS)),
        }
    }

    /// Drops lapsed records from `map` first, so a lapsed record is never updated in place.
    fn prune(&self, map: &mut BTreeMap<Key, Record>, now: DateTime<Utc>) -> Changed {
        let mut changed = Changed::default();
        map.retain(|(_, aid), r| {
            let keep = self.live(r, now);
            if !keep {
                changed.0.insert(aid.clone());
            }
            keep
        });
        changed
    }

    /// Creates or updates the record; `Some` fields replace (an empty thread
    /// list clears the threads without clearing the record). Always renews.
    pub fn set(&self, who: &Actor, aid: &str, s: SetWorking) -> (SessionWorking, Changed) {
        let now = self.now();
        let mut map = self.records.lock().unwrap();
        let mut changed = self.prune(&mut map, now);
        let r = Self::upsert(&mut map, who, aid, now);
        if let Some(t) = s.thread_ids {
            r.threads.clear();
            r.had_threads = false;
            r.add(&t);
        }
        if let Some(m) = s.message {
            r.message = Some(m);
        }
        changed.merge(Changed::one(aid));
        (Self::session_view(&who.session_id, aid, r), changed)
    }

    /// Creates or renews the record and adds `threads` (feedback reached the session).
    pub fn mark(&self, who: &Actor, aid: &str, threads: &[String]) -> Changed {
        let now = self.now();
        let mut map = self.records.lock().unwrap();
        let mut changed = self.prune(&mut map, now);
        let before = map.get(&(who.session_id.clone(), aid.to_string())).map(|r| r.threads.clone());
        let r = Self::upsert(&mut map, who, aid, now);
        r.add(threads);
        if before.as_ref() != Some(&r.threads) {
            changed.merge(Changed::one(aid));
        }
        changed
    }

    /// Renews every live record of session `sid`; how many.
    pub fn renew(&self, sid: &str) -> usize {
        let now = self.now();
        let mut map = self.records.lock().unwrap();
        let mut n = 0;
        for ((s, _), r) in map.iter_mut() {
            if s == sid && self.live(r, now) {
                r.heartbeat = now;
                n += 1;
            }
        }
        n
    }

    /// Removes `threads` from the record, or the record when `None`. A record
    /// that named threads and has none left is removed.
    pub fn clear(&self, sid: &str, aid: &str, threads: Option<&[String]>) -> Changed {
        let mut map = self.records.lock().unwrap();
        let key = (sid.to_string(), aid.to_string());
        let Some(r) = map.get_mut(&key) else { return Changed::default() };
        match threads {
            None => {
                map.remove(&key);
            }
            Some(ts) => {
                let n = r.threads.len();
                r.threads.retain(|t| !ts.contains(t));
                if r.threads.len() == n {
                    return Changed::default();
                }
                if r.had_threads && r.threads.is_empty() {
                    map.remove(&key);
                }
            }
        }
        Changed::one(aid)
    }

    /// The session replied to or resolved `tid`: renews the session, then
    /// takes `tid` out of its record on `aid`.
    pub fn thread_done(&self, sid: &str, aid: &str, tid: &str) -> Changed {
        self.renew(sid);
        self.clear(sid, aid, Some(&[tid.to_string()]))
    }

    /// `tid` was resolved by a viewer or deleted: out of every record on `aid`.
    pub fn thread_gone(&self, aid: &str, tid: &str) -> Changed {
        let sessions: Vec<String> = {
            let map = self.records.lock().unwrap();
            map.keys().filter(|(_, a)| a == aid).map(|(s, _)| s.clone()).collect()
        };
        let mut changed = Changed::default();
        for s in sessions {
            changed.merge(self.clear(&s, aid, Some(&[tid.to_string()])));
        }
        changed
    }

    /// Every record of `sid` (turn end or session end).
    pub fn end_session(&self, sid: &str) -> Changed {
        let mut changed = Changed::default();
        self.records.lock().unwrap().retain(|(s, aid), _| {
            let keep = s != sid;
            if !keep {
                changed.0.insert(aid.clone());
            }
            keep
        });
        changed
    }

    /// Every record on `aid`.
    pub fn artifact_gone(&self, aid: &str) -> Changed {
        let mut map = self.records.lock().unwrap();
        let n = map.len();
        map.retain(|(_, a), _| a != aid);
        if map.len() == n { Changed::default() } else { Changed::one(aid) }
    }

    /// Removes lapsed records; the artifacts whose list changed.
    pub fn sweep(&self) -> Changed {
        let now = self.now();
        self.prune(&mut self.records.lock().unwrap(), now)
    }

    /// The live records on `aid`, newest `started_at` first.
    pub fn for_artifact(&self, aid: &str) -> Vec<WorkingView> {
        let now = self.now();
        let map = self.records.lock().unwrap();
        let mut v: Vec<_> = map
            .iter()
            .filter(|((_, a), r)| a == aid && self.live(r, now))
            .map(|(_, r)| (r.started_at, r.view()))
            .collect();
        v.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.key.cmp(&a.1.key)));
        v.into_iter().map(|(_, x)| x).collect()
    }

    /// The live records of `sid`, by artifact ID.
    pub fn for_session(&self, sid: &str) -> Vec<SessionWorking> {
        let now = self.now();
        let map = self.records.lock().unwrap();
        map.iter()
            .filter(|((s, _), r)| s == sid && self.live(r, now))
            .map(|((s, a), r)| Self::session_view(s, a, r))
            .collect()
    }

    /// The threads the live record (`sid`, `aid`) names.
    pub fn threads_of(&self, sid: &str, aid: &str) -> Vec<String> {
        let now = self.now();
        let map = self.records.lock().unwrap();
        map.get(&(sid.to_string(), aid.to_string()))
            .filter(|r| self.live(r, now))
            .map(|r| r.threads.clone())
            .unwrap_or_default()
    }

    /// Every artifact with live records, and its list ([`Working::for_artifact`] order).
    pub fn all(&self) -> BTreeMap<String, Vec<WorkingView>> {
        let aids: BTreeSet<String> = self.records.lock().unwrap().keys().map(|(_, a)| a.clone()).collect();
        aids.into_iter()
            .map(|a| (a.clone(), self.for_artifact(&a)))
            .filter(|(_, v)| !v.is_empty())
            .collect()
    }
}
```

Run: `cargo test -p clax-core working`
Expected: PASS (11 tests).

- [ ] **Step 3: The `working` event**

In `crates/clax-core/src/events.rs`, add the variant after `FeedbackState`:

```rust
    /// The artifact's working list changed; `working` is the whole list
    /// (spec §10 "Working"), never naming a session.
    Working {
        artifact_id: String,
        working: Vec<crate::working::WorkingView>,
    },
```

Add `| Event::Working { artifact_id, .. }` to `artifact_id()` and `Event::Working { .. } => "working",` to `name()`. Add to the `names_match_the_serialised_type` list:

```rust
            Event::Working {
                artifact_id: "a".into(),
                working: vec![],
            },
```

Run: `cargo test -p clax-core`
Expected: PASS.

- [ ] **Step 4: Gates and commit**

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add crates/clax-core/src/working.rs crates/clax-core/src/lib.rs crates/clax-core/src/events.rs
git commit -m "Add the in-memory working registry and the working event"
git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed
```

---

### Task 3: Working routes, events and the sweeper

**Files:**
- Create: `crates/clax-server/src/working.rs`, `crates/clax-server/src/routes/working.rs`, `crates/clax-server/tests/api_working.rs`
- Modify: `crates/clax-server/src/lib.rs`, `crates/clax-server/src/state.rs`, `crates/clax-server/src/feedback.rs` (`FeedbackCtx` gains `working`), `crates/clax-server/src/routes/mod.rs`, `crates/clax-server/src/routes/events.rs`, `crates/clax-server/src/routes/artifacts.rs` (`with_owner`, `list`, `get`), `crates/clax-server/src/boot.rs` (`assemble`), `crates/clax-server/src/daemon.rs`, `crates/clax-server/src/testing.rs`

**Interfaces:**
- Consumes: Task 2's `Working`.
- Produces: `AppState.working: Arc<Working>`, `FeedbackCtx.working: Arc<Working>`, `TestServer.working: Arc<Working>`.
- Produces: `clax_server::working::{announce(events: &EventBus, w: &Working, changed: &Changed), sweep_and_announce(w: &Working, events: &EventBus), SWEEP_INTERVAL: Duration = 5 s}`.
- Produces the routes in the spec §6 amendment. Error codes: 401 `unauthorized`; 404 `not_found` (unknown session or artifact); 400 `unknown_session` (ended session); 400 `invalid_args` (`thread_ids` not ULIDs or more than 20); 400 `unknown_thread` (not a thread of the artifact); 400 `thread_not_open`; 400 `invalid_json`.
- Produces (debug builds only): `POST /api/_test/working/skew` (token) with `{secs}`, answering `{now}` after skewing and sweeping. Playwright uses it in Task 11.
- `GET /api/events?types=working[,…]`.

- [ ] **Step 1: Write the failing route tests**

`crates/clax-server/tests/api_working.rs`:

```rust
mod common;
use clax_core::working::{ManualClock, Working};
use common::TestServer;
use serde_json::{Value, json};
use std::sync::Arc;

async fn server() -> (TestServer, Arc<ManualClock>) {
    let clock = Arc::new(ManualClock::at("2026-09-30T10:00:00Z"));
    let c = clock.clone();
    let ts = TestServer::spawn_with(move |s| s.working = Arc::new(Working::new(c))).await;
    (ts, clock)
}

/// A claude session owning a one-version artifact with one sent thread.
async fn setup(ts: &TestServer) -> (String, String, String) {
    let s = ts.register_session("claude", "w1").await;
    let sid = s["id"].as_str().unwrap().to_string();
    let a = ts.publish_as(&sid, "T", "<main><h2>Quarterly goals</h2></main>").await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let t = ts.thread(&aid, 1, "@agent two columns").await;
    (sid, aid, t["id"].as_str().unwrap().to_string())
}

async fn put(ts: &TestServer, sid: &str, aid: &str, body: Value) -> reqwest::Response {
    ts.authed(ts.client.put(format!("{}/api/sessions/{sid}/working/{aid}", ts.base)))
        .json(&body)
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn setting_needs_the_token_and_reading_does_not() {
    let (ts, _) = server().await;
    let (sid, aid, tid) = setup(&ts).await;
    let url = format!("{}/api/sessions/{sid}/working/{aid}", ts.base);
    assert_eq!(ts.client.put(&url).json(&json!({})).send().await.unwrap().status(), 401);
    let res = put(&ts, &sid, &aid, json!({"thread_ids": [tid], "message": " Two\ncolumns "})).await;
    assert_eq!(res.status(), 200);
    let v: Value = res.json().await.unwrap();
    assert_eq!(v["working"]["message"], "Two columns");
    assert_eq!(v["working"]["session_id"], sid.as_str());
    assert_eq!(v["message_truncated"], false);
    let public: Value = ts.get(&format!("/api/artifacts/{aid}/working")).await.json().await.unwrap();
    let w = &public["working"][0];
    assert_eq!(w["harness"], "claude");
    assert_eq!(w["thread_ids"], json!([tid]));
    assert!(!public.to_string().contains(&sid), "{public}");
    assert!(w.get("session_id").is_none());
    let mine = ts.get_authed(&format!("/api/sessions/{sid}/working")).await;
    assert_eq!(mine.status(), 200);
    assert_eq!(ts.get(&format!("/api/sessions/{sid}/working")).await.status(), 401);
}

#[tokio::test]
async fn artifact_views_carry_the_working_list_without_session_ids() {
    let (ts, _) = server().await;
    let (sid, aid, _) = setup(&ts).await;
    put(&ts, &sid, &aid, json!({"message": "Refactoring"})).await;
    let one: Value = ts.get(&format!("/api/artifacts/{aid}")).await.json().await.unwrap();
    assert_eq!(one["artifact"]["working"][0]["message"], "Refactoring");
    let all: Value = ts.get("/api/artifacts").await.json().await.unwrap();
    assert_eq!(all["artifacts"][0]["working"][0]["harness"], "claude");
    assert!(all["artifacts"][0]["working"][0].get("session_id").is_none());
}

#[tokio::test]
async fn bad_requests_name_their_code() {
    let (ts, _) = server().await;
    let (sid, aid, tid) = setup(&ts).await;
    let code = |r: Value| r["error"]["code"].as_str().unwrap().to_string();
    let r = put(&ts, &sid, &aid, json!({"thread_ids": ["nope"]})).await;
    assert_eq!(r.status(), 400);
    assert_eq!(code(r.json().await.unwrap()), "invalid_args");
    let many: Vec<String> = (0..21).map(|_| clax_core::new_ulid()).collect();
    assert_eq!(code(put(&ts, &sid, &aid, json!({"thread_ids": many})).await.json().await.unwrap()), "invalid_args");
    let r = put(&ts, &sid, &aid, json!({"thread_ids": [clax_core::new_ulid()]})).await;
    assert_eq!(code(r.json().await.unwrap()), "unknown_thread");
    ts.client
        .post(format!("{}/api/artifacts/{aid}/threads/{tid}/resolve", ts.base))
        .send()
        .await
        .unwrap();
    let r = put(&ts, &sid, &aid, json!({"thread_ids": [tid]})).await;
    assert_eq!(code(r.json().await.unwrap()), "thread_not_open");
    let r = put(&ts, &sid, "7q3k9mzx2b4t", json!({})).await;
    assert_eq!(r.status(), 404);
    let r = put(&ts, &clax_core::new_ulid(), &aid, json!({})).await;
    assert_eq!(r.status(), 404);
    ts.authed(ts.client.patch(format!("{}/api/sessions/{sid}", ts.base)))
        .json(&json!({"ended": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(code(put(&ts, &sid, &aid, json!({})).await.json().await.unwrap()), "unknown_session");
}

#[tokio::test]
async fn a_long_message_is_cut_and_flagged() {
    let (ts, _) = server().await;
    let (sid, aid, _) = setup(&ts).await;
    let v: Value = put(&ts, &sid, &aid, json!({"message": "m".repeat(300)})).await.json().await.unwrap();
    assert_eq!(v["message_truncated"], true);
    assert_eq!(v["working"]["message"].as_str().unwrap().chars().count(), 140);
}

#[tokio::test]
async fn delete_clears_one_record_or_its_threads() {
    let (ts, _) = server().await;
    let (sid, aid, tid) = setup(&ts).await;
    put(&ts, &sid, &aid, json!({"thread_ids": [tid], "message": "x"})).await;
    let url = format!("{}/api/sessions/{sid}/working/{aid}", ts.base);
    let v: Value = ts.authed(ts.client.delete(format!("{url}?thread_ids={tid}"))).send().await.unwrap().json().await.unwrap();
    assert_eq!(v["cleared"], true);
    assert_eq!(v["working"], Value::Null, "its last thread went");
    put(&ts, &sid, &aid, json!({"message": "y"})).await;
    let v: Value = ts.authed(ts.client.delete(&url)).send().await.unwrap().json().await.unwrap();
    assert_eq!(v["cleared"], true);
    let v: Value = ts.authed(ts.client.delete(&url)).send().await.unwrap().json().await.unwrap();
    assert_eq!(v["cleared"], false);
}

#[tokio::test]
async fn renew_and_end_act_on_every_record_of_the_session() {
    let (ts, clock) = server().await;
    let (sid, aid, _) = setup(&ts).await;
    put(&ts, &sid, &aid, json!({})).await;
    clock.advance(100);
    let v: Value = ts.post_json(&format!("/api/sessions/{sid}/working/renew"), json!({})).await.json().await.unwrap();
    assert_eq!(v["renewed"], 1);
    clock.advance(100);
    let w: Value = ts.get(&format!("/api/artifacts/{aid}/working")).await.json().await.unwrap();
    assert_eq!(w["working"].as_array().unwrap().len(), 1, "renewed at 100 s, so alive at 200 s");
    let v: Value = ts.post_json(&format!("/api/sessions/{sid}/working/end"), json!({})).await.json().await.unwrap();
    assert_eq!(v["cleared"], 1);
    let w: Value = ts.get(&format!("/api/artifacts/{aid}/working")).await.json().await.unwrap();
    assert_eq!(w["working"], json!([]));
}

#[tokio::test]
async fn expiry_is_hidden_at_once_and_announced_by_the_sweep() {
    let (ts, clock) = server().await;
    let (sid, aid, _) = setup(&ts).await;
    let mut ev = ts.events(&format!("?artifact={aid}&types=working")).await;
    put(&ts, &sid, &aid, json!({"message": "x"})).await;
    let set = ev.next_named("working").await;
    assert_eq!(set["working"][0]["message"], "x");
    clock.advance(120);
    let w: Value = ts.get(&format!("/api/artifacts/{aid}/working")).await.json().await.unwrap();
    assert_eq!(w["working"], json!([]));
    clax_server::working::sweep_and_announce(&ts.working, &ts.events);
    let gone = ev.next_named("working").await;
    assert_eq!(gone, json!({"type": "working", "artifact_id": aid, "working": []}));
}

#[tokio::test]
async fn a_types_filter_drops_other_events() {
    let (ts, _) = server().await;
    let (sid, aid, _) = setup(&ts).await;
    let mut ev = ts.events("?types=working").await;
    ts.thread(&aid, 1, "plain note").await;
    put(&ts, &sid, &aid, json!({})).await;
    let (name, data) = ev.next().await;
    assert_eq!(name, "working");
    assert_eq!(data["artifact_id"], aid.as_str());
}
```

Run: `cargo test -p clax-server --test api_working`
Expected: FAIL to compile (`working` is not a field of `AppState`).

- [ ] **Step 2: State, announce, sweeper**

In `state.rs` add `pub working: Arc<clax_core::working::Working>,` with the doc comment `/// Working records (spec §10 "Working"), in memory.`. In `feedback.rs`, add `pub working: Arc<clax_core::working::Working>,` to `FeedbackCtx` and `working: self.working.clone(),` to `feedback_ctx()`. In `daemon.rs` and `testing.rs`, initialise `working: Arc::new(clax_core::working::Working::new(Arc::new(clax_core::working::SystemClock))),`, and give `TestServer` a `pub working: Arc<clax_core::working::Working>` field copied from the state after `f(&mut state)`.

`crates/clax-server/src/working.rs`:

```rust
//! Fan-out of working-record changes (`working` events), the sweeper, and
//! the automatic marks made when feedback reaches a session.

use clax_core::working::{Changed, Working};
use clax_core::{Event, EventBus};
use std::time::Duration;

/// How often lapsed records are removed and announced.
pub const SWEEP_INTERVAL: Duration = Duration::from_secs(5);

/// Sends each changed artifact's whole list.
pub fn announce(events: &EventBus, w: &Working, changed: &Changed) {
    for aid in &changed.0 {
        events.publish(Event::Working { artifact_id: aid.clone(), working: w.for_artifact(aid) });
    }
}

/// Removes lapsed records and announces the artifacts they were on.
pub fn sweep_and_announce(w: &Working, events: &EventBus) {
    let changed = w.sweep();
    announce(events, w, &changed);
}
```

Add `pub mod working;` to `lib.rs`. In `daemon.rs::serve`, after the reaper task, spawn the sweeper and abort it with the reaper:

```rust
    let (sweep_working, sweep_events) = (state_working.clone(), state_events.clone());
    let sweeper = tokio::spawn(async move {
        let mut every = tokio::time::interval(crate::working::SWEEP_INTERVAL);
        loop {
            every.tick().await;
            crate::working::sweep_and_announce(&sweep_working, &sweep_events);
        }
    });
```

Here `state_working` and `state_events` are clones of `state.working` and `state.events`, taken before `state` moves into `build_router_with_shutdown`. Add `sweeper.abort();` next to `reaper.abort();`. In the reaper's blocking closure, after `ctx.waiters.forget(id);`, add `crate::working::announce(&ctx.events, &ctx.working, &ctx.working.end_session(id));`.

- [ ] **Step 3: The routes**

`crates/clax-server/src/routes/working.rs`:

```rust
//! Working routes (spec §6, §10 "Working").

use super::artifacts::{body, parse_id, path};
use crate::auth::RequireToken;
use crate::error::ApiError;
use crate::state::AppState;
use crate::working::announce;
use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use clax_core::model::Session;
use clax_core::working::{Actor, MAX_WORKING_THREADS, SetWorking, clean_message};
use clax_core::{ArtifactId, CoreError, Store};
use serde::Deserialize;
use serde_json::{Value, json};

/// The live session `sid`: 404 when unknown, 400 `unknown_session` when ended.
fn live(st: &Store, sid: &str) -> clax_core::Result<Session> {
    match st.get_session(sid)? {
        None => Err(CoreError::NotFound),
        Some(s) if s.ended_at.is_some() => Err(CoreError::invalid("unknown_session", "the session has ended")),
        Some(s) => Ok(s),
    }
}

/// Checks `ids` (at most [`MAX_WORKING_THREADS`] ULIDs) are open threads of `aid`.
pub(crate) fn check_threads(st: &Store, aid: &ArtifactId, ids: &[String]) -> clax_core::Result<()> {
    if ids.len() > MAX_WORKING_THREADS {
        return Err(CoreError::invalid("invalid_args", format!("at most {MAX_WORKING_THREADS} thread_ids")));
    }
    for tid in ids {
        if !clax_core::is_ulid(tid) {
            return Err(CoreError::invalid("invalid_args", format!("'{tid}' is not a thread ID")));
        }
        match st.get_thread(tid)? {
            Some(t) if t.artifact_id == aid.as_str() => {
                if t.status != "open" {
                    return Err(CoreError::invalid("thread_not_open", format!("thread {tid} is resolved")));
                }
            }
            _ => return Err(CoreError::invalid("unknown_thread", format!("{tid} is not a thread of {aid}"))),
        }
    }
    Ok(())
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct SetBody {
    #[serde(default)]
    thread_ids: Option<Vec<String>>,
    #[serde(default)]
    message: Option<String>,
}

/// `PUT /api/sessions/<sid>/working/<aid>` (W): creates or updates the
/// record; given fields replace. `{working, message_truncated}`.
pub async fn put(
    State(s): State<AppState>,
    _t: RequireToken,
    p: Result<Path<(String, String)>, PathRejection>,
    req: Result<Json<SetBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let (sid, aid) = path(p)?;
    let id = parse_id(&aid)?;
    let b = body(req)?;
    let (message, truncated) = match b.message.as_deref() {
        Some(m) => clean_message(m),
        None => (None, false),
    };
    let (w, events) = (s.working.clone(), s.events.clone());
    let view = s
        .store_call(move |st| {
            let sess = live(st, &sid)?;
            st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            if let Some(t) = &b.thread_ids {
                check_threads(st, &id, t)?;
            }
            let who = Actor { session_id: sess.id, harness: sess.harness };
            let (view, changed) = w.set(&who, id.as_str(), SetWorking { thread_ids: b.thread_ids, message });
            announce(&events, &w, &changed);
            Ok(view)
        })
        .await?;
    Ok(Json(json!({"working": view, "message_truncated": truncated})))
}

#[derive(Deserialize)]
pub struct ClearQuery {
    thread_ids: Option<String>,
}

/// `DELETE /api/sessions/<sid>/working/<aid>` (W): the record, or with
/// `?thread_ids=a,b` only those threads. `{cleared, working}` (`working` is
/// what remains, or null).
pub async fn delete(
    State(s): State<AppState>,
    _t: RequireToken,
    p: Result<Path<(String, String)>, PathRejection>,
    q: Result<Query<ClearQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let (sid, aid) = path(p)?;
    let id = parse_id(&aid)?;
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let threads: Option<Vec<String>> =
        q.thread_ids.map(|t| t.split(',').filter(|x| !x.is_empty()).map(str::to_string).collect());
    let (w, events) = (s.working.clone(), s.events.clone());
    let (cleared, left) = s
        .store_call(move |st| {
            live(st, &sid)?;
            let changed = w.clear(&sid, id.as_str(), threads.as_deref());
            announce(&events, &w, &changed);
            let left = w.for_session(&sid).into_iter().find(|r| r.artifact_id == id.as_str());
            Ok((!changed.is_empty(), left))
        })
        .await?;
    Ok(Json(json!({"cleared": cleared, "working": left})))
}

/// `GET /api/sessions/<sid>/working` (token): `{working}` with session fields.
pub async fn for_session(
    State(s): State<AppState>,
    _t: RequireToken,
    p: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let sid = path(p)?;
    let w = s.working.clone();
    let list = s
        .store_call(move |st| {
            st.get_session(&sid)?.ok_or(CoreError::NotFound)?;
            Ok(w.for_session(&sid))
        })
        .await?;
    Ok(Json(json!({"working": list})))
}

/// `GET /api/artifacts/<aid>/working` (no token): `{working}`, never naming a session.
pub async fn for_artifact(
    State(s): State<AppState>,
    p: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&path(p)?)?;
    let w = s.working.clone();
    let list = s
        .store_call(move |st| {
            st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            Ok(w.for_artifact(id.as_str()))
        })
        .await?;
    Ok(Json(json!({"working": list})))
}

/// `POST /api/sessions/<sid>/working/renew` (W): `{renewed}`.
pub async fn renew(
    State(s): State<AppState>,
    _t: RequireToken,
    p: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let sid = path(p)?;
    let w = s.working.clone();
    let n = s.store_call(move |st| { live(st, &sid)?; Ok(w.renew(&sid)) }).await?;
    Ok(Json(json!({"renewed": n})))
}

/// `POST /api/sessions/<sid>/working/end` (W; the turn ended): `{cleared}`.
pub async fn end(
    State(s): State<AppState>,
    _t: RequireToken,
    p: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let sid = path(p)?;
    let (w, events) = (s.working.clone(), s.events.clone());
    let n = s
        .store_call(move |st| {
            live(st, &sid)?;
            let changed = w.end_session(&sid);
            announce(&events, &w, &changed);
            Ok(changed.0.len())
        })
        .await?;
    Ok(Json(json!({"cleared": n})))
}

#[cfg(debug_assertions)]
#[derive(Deserialize)]
pub struct SkewBody {
    secs: i64,
}

/// Debug builds only: moves the working clock forward and sweeps, so browser
/// tests can observe expiry without waiting. `{now}`.
#[cfg(debug_assertions)]
pub async fn skew(
    State(s): State<AppState>,
    _t: RequireToken,
    req: Result<Json<SkewBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let b = body(req)?;
    s.working.skew(b.secs);
    crate::working::sweep_and_announce(&s.working, &s.events);
    Ok(Json(json!({"now": s.working.now().to_rfc3339()})))
}
```

In `routes/mod.rs`, add `pub mod working;` and these routes to `api_fast`:

```rust
        .route("/api/artifacts/{aid}/working", get(working::for_artifact))
        .route("/api/sessions/{id}/working", get(working::for_session))
        .route("/api/sessions/{id}/working/renew", post(working::renew))
        .route("/api/sessions/{id}/working/end", post(working::end))
        .route(
            "/api/sessions/{id}/working/{aid}",
            axum::routing::put(working::put).delete(working::delete),
        )
```

and, after the `#[cfg(feature = "test-routes")]` block:

```rust
    #[cfg(debug_assertions)]
    let api_fast = api_fast.route("/api/_test/working/skew", post(working::skew));
```

- [ ] **Step 4: Artifact views carry `working`; the `types` filter**

In `routes/artifacts.rs`, give `with_owner` a third parameter `working: &[clax_core::working::WorkingView]` and set `v["working"] = json!(working);`. `list` passes `s.working.all()` lookups (`all.get(&a.id).map(Vec::as_slice).unwrap_or(&[])`, with `all` computed once before the loop and moved into the closure). `get` passes `s.working.for_artifact(id.as_str())`. The port made `with_owner` `pub(crate)` for the bootstrap block. In `crates/clax-server/src/boot.rs` `assemble`, pass `s.working.for_artifact(<the artifact ID>)` as well, so the embedded artifact carries its `working` list. Add `crates/clax-server/src/boot.rs` to this task's files and its `git add`. Expected JSON in `crates/clax-server/tests/shell_boot.rs` gains `"working": []`.

In `routes/events.rs`, add `types: Option<String>` to `EventsQuery` (doc: ``When set, only events whose name is in this comma list are sent; `ready` and `resync` always are.``). Parse it once into `Option<BTreeSet<String>>`, and in the `filter_map` after the artifact check add:

```rust
        if let Some(t) = &types
            && !t.contains(ev.name())
        {
            return None;
        }
```

Update the doc comment's event list to include `working`.

Run: `cargo test -p clax-server --test api_working`
Expected: PASS (8 tests). Then `cargo test -p clax-server` must pass too. Existing tests comparing exact artifact JSON gain `"working": []`: add it to their expected values, and change nothing else.

- [ ] **Step 5: Gates and commit**

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add crates/clax-server/src/working.rs crates/clax-server/src/routes/working.rs crates/clax-server/tests/api_working.rs \
  crates/clax-server/src/lib.rs crates/clax-server/src/state.rs crates/clax-server/src/feedback.rs crates/clax-server/src/routes/mod.rs \
  crates/clax-server/src/routes/events.rs crates/clax-server/src/routes/artifacts.rs crates/clax-server/src/daemon.rs crates/clax-server/src/testing.rs \
  crates/clax-server/src/boot.rs
git add -u crates/clax-server/tests
git commit -m "Serve working records over REST and SSE, and sweep lapsed ones"
git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed
```

---

### Task 4: Automatic marks, renewals and clears in the daemon

**Files:**
- Modify: `crates/clax-server/src/working.rs`, `crates/clax-server/src/routes/feedback.rs`, `crates/clax-server/src/push.rs`, `crates/clax-server/src/routes/threads.rs`, `crates/clax-server/src/routes/artifacts.rs` (`publish`, `delete`), `crates/clax-server/src/routes/sessions.rs` (`patch`)
- Create: `crates/clax-server/tests/api_working_auto.rs`

**Interfaces:**
- Produces: `clax_server::working::{mark_items(ctx: &FeedbackCtx, st: &Store, session_id: &str, items: &[FeedbackItem]) -> clax_core::Result<()>, renew_for_tier(ctx: &FeedbackCtx, session_id: &str, tier: Tier)}`.
- The event table under "Design: the working record" is the contract of this task.

- [ ] **Step 1: Write the failing tests**

`crates/clax-server/tests/api_working_auto.rs`:

```rust
mod common;
use clax_core::working::{ManualClock, Working};
use common::TestServer;
use serde_json::{Value, json};
use std::sync::Arc;

async fn server() -> (TestServer, Arc<ManualClock>) {
    let clock = Arc::new(ManualClock::at("2026-09-30T10:00:00Z"));
    let c = clock.clone();
    (TestServer::spawn_with(move |s| s.working = Arc::new(Working::new(c))).await, clock)
}

async fn setup(ts: &TestServer, harness: &str) -> (String, String, String) {
    let s = ts.register_session(harness, &format!("{harness}-auto")).await;
    let sid = s["id"].as_str().unwrap().to_string();
    let a = ts.publish_as(&sid, "T", "<main><h2>Quarterly goals</h2></main>").await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let t = ts.thread(&aid, 1, "@agent two columns").await;
    (sid, aid, t["id"].as_str().unwrap().to_string())
}

async fn working(ts: &TestServer, aid: &str) -> Vec<Value> {
    let v: Value = ts.get(&format!("/api/artifacts/{aid}/working")).await.json().await.unwrap();
    v["working"].as_array().unwrap().clone()
}

async fn take(ts: &TestServer, sid: &str, tier: &str) -> Value {
    ts.get_authed(&format!("/api/sessions/{sid}/feedback?tier={tier}")).await.json().await.unwrap()
}

#[tokio::test]
async fn each_delivering_tier_marks_the_session_working_on_the_thread() {
    for tier in ["piggyback", "stop_hook", "prompt_hook", "wait", "inject"] {
        let (ts, _) = server().await;
        let (sid, aid, tid) = setup(&ts, if tier == "inject" { "pi" } else { "claude" }).await;
        assert!(working(&ts, &aid).await.is_empty(), "{tier}");
        let got = take(&ts, &sid, tier).await;
        assert_eq!(got["feedback"].as_array().unwrap().len(), 1, "{tier}");
        let w = working(&ts, &aid).await;
        assert_eq!(w.len(), 1, "{tier}");
        assert_eq!(w[0]["thread_ids"], json!([tid]), "{tier}");
    }
}

#[tokio::test]
async fn an_empty_hook_or_tool_take_renews_but_a_heartbeat_and_a_wait_poll_do_not() {
    let (ts, clock) = server().await;
    let (sid, aid, _) = setup(&ts, "claude").await;
    take(&ts, &sid, "piggyback").await;
    clock.advance(100);
    ts.authed(ts.client.patch(format!("{}/api/sessions/{sid}", ts.base)))
        .json(&json!({"heartbeat": true})).send().await.unwrap();
    take(&ts, &sid, "wait").await;
    clock.advance(20);
    assert!(working(&ts, &aid).await.is_empty(), "neither the heartbeat nor the wait poll renewed");
    let (ts, clock) = server().await;
    let (sid, aid, _) = setup(&ts, "claude").await;
    take(&ts, &sid, "piggyback").await;
    clock.advance(100);
    take(&ts, &sid, "stop_hook").await;
    clock.advance(100);
    assert_eq!(working(&ts, &aid).await.len(), 1, "the empty stop_hook take renewed at 100 s");
}

#[tokio::test]
async fn the_agent_reply_to_the_last_named_thread_clears() {
    let (ts, _) = server().await;
    let (sid, aid, tid) = setup(&ts, "claude").await;
    take(&ts, &sid, "piggyback").await;
    let mut ev = ts.events(&format!("?artifact={aid}&types=working")).await;
    let res = ts.authed(ts.client.post(format!("{}/api/artifacts/{aid}/threads/{tid}/comments", ts.base)))
        .header("x-clax-session", &sid)
        .json(&json!({"body": "Done.", "author_kind": "agent"}))
        .send().await.unwrap();
    assert_eq!(res.status(), 201);
    assert_eq!(ev.next_named("working").await["working"], json!([]));
    assert!(working(&ts, &aid).await.is_empty());
}

#[tokio::test]
async fn an_agent_resolve_and_a_viewer_resolve_take_the_thread_out() {
    let (ts, _) = server().await;
    let (sid, aid, tid) = setup(&ts, "claude").await;
    take(&ts, &sid, "piggyback").await;
    ts.authed(ts.client.post(format!("{}/api/artifacts/{aid}/threads/{tid}/resolve", ts.base)))
        .header("x-clax-session", &sid).json(&json!({"as": "agent"})).send().await.unwrap();
    assert!(working(&ts, &aid).await.is_empty());
    let t2 = ts.thread(&aid, 1, "@agent again").await;
    take(&ts, &sid, "piggyback").await;
    assert_eq!(working(&ts, &aid).await.len(), 1);
    ts.client.post(format!("{}/api/artifacts/{aid}/threads/{}/resolve", ts.base, t2["id"].as_str().unwrap()))
        .send().await.unwrap();
    assert!(working(&ts, &aid).await.is_empty());
}

#[tokio::test]
async fn a_publish_by_the_session_clears_its_record_on_that_artifact() {
    let (ts, _) = server().await;
    let (sid, aid, _) = setup(&ts, "claude").await;
    take(&ts, &sid, "piggyback").await;
    let res = ts.authed(ts.client.post(format!("{}/api/artifacts/{aid}/versions", ts.base)))
        .header("x-clax-session", &sid)
        .json(&json!({"if_version": 1, "files": {"index.html": {"content": "<p>2</p>", "encoding": "utf8"}}}))
        .send().await.unwrap();
    assert_eq!(res.status(), 201);
    assert!(working(&ts, &aid).await.is_empty());
}

#[tokio::test]
async fn session_end_and_artifact_delete_clear() {
    let (ts, _) = server().await;
    let (sid, aid, _) = setup(&ts, "claude").await;
    take(&ts, &sid, "piggyback").await;
    ts.authed(ts.client.patch(format!("{}/api/sessions/{sid}", ts.base)))
        .json(&json!({"ended": true})).send().await.unwrap();
    assert!(working(&ts, &aid).await.is_empty());
    let (ts, _) = server().await;
    let (sid, aid, _) = setup(&ts, "claude").await;
    take(&ts, &sid, "piggyback").await;
    let mut ev = ts.events("?types=working").await;
    ts.authed(ts.client.delete(format!("{}/api/artifacts/{aid}", ts.base))).send().await.unwrap();
    assert_eq!(ev.next_named("working").await["working"], json!([]));
}

#[tokio::test]
async fn a_thread_delete_takes_it_out() {
    let (ts, _) = server().await;
    let (sid, aid, tid) = setup(&ts, "claude").await;
    take(&ts, &sid, "piggyback").await;
    let named = ts.viewer(Some("Alex")).await;
    let res = ts.client.delete(format!("{}/api/artifacts/{aid}/threads/{tid}", ts.base))
        .header("cookie", format!("clax_viewer={}", named.cookie))
        .send().await.unwrap();
    assert_eq!(res.status(), 200);
    assert!(working(&ts, &aid).await.is_empty());
}
```

Run: `cargo test -p clax-server --test api_working_auto`
Expected: FAIL (nothing marks yet).

- [ ] **Step 2: Mark and renew in the feedback route and in push**

Append to `crates/clax-server/src/working.rs`:

```rust
use crate::feedback::FeedbackCtx;
use clax_core::working::Actor;
use clax_core::{FeedbackItem, Store, Tier};
use std::collections::BTreeMap;

/// Tiers whose takes are hook runs or tool calls: each renews the session's records.
pub fn renew_for_tier(ctx: &FeedbackCtx, session_id: &str, tier: Tier) {
    if matches!(tier, Tier::Piggyback | Tier::StopHook | Tier::PromptHook) {
        ctx.working.renew(session_id);
    }
}

/// Feedback reached `session_id`: marks it working on each item's artifact
/// and thread, and announces the changes.
pub fn mark_items(ctx: &FeedbackCtx, st: &Store, session_id: &str, items: &[FeedbackItem]) -> clax_core::Result<()> {
    if items.is_empty() {
        return Ok(());
    }
    let Some(sess) = st.get_session(session_id)? else { return Ok(()) };
    let who = Actor { session_id: sess.id, harness: sess.harness };
    let mut by_artifact: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for i in items {
        by_artifact.entry(&i.artifact_id).or_default().push(i.thread_id.clone());
    }
    let mut changed = clax_core::working::Changed::default();
    for (aid, tids) in by_artifact {
        changed.merge(ctx.working.mark(&who, aid, &tids));
    }
    announce(&ctx.events, &ctx.working, &changed);
    Ok(())
}
```

In `routes/feedback.rs::poll`, inside the `store_call` closure right after `apply(&ctx, st, &touched);`:

```rust
                crate::working::renew_for_tier(&ctx, &t.session_id, t.tier);
                if t.tier != Tier::Queue {
                    crate::working::mark_items(&ctx, st, &t.session_id, &items)?;
                }
```

In `push.rs::dispatch`, clone `items` into the spawned task (`let marked = items.clone();` before the spawn), and in the blocking settle closure, inside `if error.is_none()` (add that branch next to the failure branch):

```rust
                if error.is_none()
                    && let Err(e) = crate::working::mark_items(&ctx, &store, &sid, &marked)
                {
                    tracing::warn!(session = %sid, error = %e, "marking a queued session working failed");
                }
```

- [ ] **Step 3: Clear on reply, resolve, delete, publish, session end**

`routes/threads.rs`:
- `comment`, agent branch, after `touched.merge(st.acknowledge(...))`: `let changed = ctx.working.thread_done(&sess.id, &aid, &tid); crate::working::announce(&ctx.events, &ctx.working, &changed);`
- `resolve`: in the agent branch, after the resolve succeeds, the same `thread_done` with the agent session's ID. In the viewer branch, `ctx.working.thread_gone(&aid, &tid)`, announced.
- `delete` (viewer or agent): after the delete succeeds, `thread_gone`, announced.

`routes/artifacts.rs`:
- `publish`: inside `if let Some(sid) = &session`, after `ensure_watch`, add `let changed = ctx.working.clear(sid, artifact.id.as_str(), None); crate::working::announce(&ctx.events, &ctx.working, &changed);`. Task 13 inserts the linking just before this clear.
- `delete`: after `st.delete_artifact(&id)?`, add `crate::working::announce(&events, &working, &working.artifact_gone(id.as_str()));`, with `working` cloned from `s.working` before the closure.

`routes/sessions.rs::patch`, `ended` branch, after `ctx.waiters.forget(&id);`: `crate::working::announce(&ctx.events, &ctx.working, &ctx.working.end_session(&id));`.

Run: `cargo test -p clax-server --test api_working_auto`
Expected: PASS (7 tests). Then `cargo test -p clax-server`: PASS.

- [ ] **Step 4: A Codex queue success marks**

Append to `crates/clax-server/tests/api_push.rs` a test that reuses that file's fake-`codex` helper (the script that exits 0) to set up a Codex session with `codex_home`, a published artifact and a sent thread. It waits for the push the way that file's existing success test does, then asserts `GET /api/artifacts/<aid>/working` lists one `codex` record naming the thread. A second test with the failing fake asserts the list stays empty. Name them `a_queue_that_exits_0_marks_the_session_working` and `a_failed_queue_marks_nothing`.

Run: `cargo test -p clax-server --test api_push`
Expected: PASS.

- [ ] **Step 5: Gates and commit**

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add crates/clax-server/src/working.rs crates/clax-server/src/routes/feedback.rs crates/clax-server/src/push.rs \
  crates/clax-server/src/routes/threads.rs crates/clax-server/src/routes/artifacts.rs crates/clax-server/src/routes/sessions.rs \
  crates/clax-server/tests/api_working_auto.rs crates/clax-server/tests/api_push.rs
git commit -m "Mark sessions working when feedback reaches them, and clear on reply, publish and end"
git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed
```

---

### Task 5: The `working` tool (MCP and Pi) and the twenty-three tool lists

The MCP tool and Pi's `clax_working` land together. `scripts/test-plugins.sh` requires every fixture description in both `tools.rs` and `clax.ts`.

**Files:**
- Modify: `plugins/pi/test/fixtures/contract.json`, `crates/clax-mcp/src/tools.rs`, `crates/clax-mcp/src/client.rs`, `crates/clax-mcp/tests/comments.rs`, `crates/clax-mcp/tests/shim.rs` (count 22 → 23), `plugins/pi/src/clax.ts`, `plugins/pi/src/client.ts`, `plugins/pi/test/clax.test.ts`, `scripts/test-plugins.sh`, `docs/contract.md`, `README.md`, `plugins/claude-code/README.md`, `plugins/clax/README.md`, `plugins/pi/README.md`, `plugins/*/skills/clax/SKILL.md` (generated block and "Comment loop")

**Interfaces:**
- Produces: `clax_mcp::tools::WorkingArgs { url_or_id: String, thread_ids: Option<Vec<String>>, message: Option<String>, done: Option<bool> }` and `ClaxTools::working`.
- Produces: `DaemonClient::{set_working(id: &str, body: &Value) -> Result<Value>, clear_working(id: &str, threads: Option<&[String]>) -> Result<Value>, renew_working() -> Result<Value>, end_working() -> Result<Value>}` and the same four on Pi's `DaemonClient` (`setWorking`, `clearWorking`, `renewWorking`, `endWorking`).
- The description string, verbatim everywhere:

```text
Tell the person you are working on an artifact: its page shows `<harness> is working: <message>` in the header, a badge on its gallery card, and `working…` on each thread in `thread_ids`. Comments sent to you mark you working automatically; call this for other work or to add a short `message` (at most 140 characters). It clears when you reply to those threads, publish the artifact, end your turn, or go 2 minutes without a tool call; `done: true` clears it now.
```

- Result: `{artifact_id, url, working: true, message, thread_ids, started_at, expires_in_s: 120, message_truncated}`, or `{artifact_id, url, working: false, cleared}`. Errors are the daemon's codes, passed through (`invalid_args`, `not_found`, `unknown_thread`, `thread_not_open`, `unknown_session`), plus the tool's own `invalid_id`, `invalid_args` and `no_session`.

- [ ] **Step 1: The fixture and the failing MCP test**

In `plugins/pi/test/fixtures/contract.json`, insert into `tools` after `wait_for_feedback`:

```json
    {"name": "working", "description": "Tell the person you are working on an artifact: its page shows `<harness> is working: <message>` in the header, a badge on its gallery card, and `working…` on each thread in `thread_ids`. Comments sent to you mark you working automatically; call this for other work or to add a short `message` (at most 140 characters). It clears when you reply to those threads, publish the artifact, end your turn, or go 2 minutes without a tool call; `done: true` clears it now."},
```

Append to `crates/clax-mcp/tests/comments.rs` (add `WorkingArgs` to the `use clax_mcp::tools::{...}` list):

```rust
#[tokio::test]
async fn working_marks_the_artifact_and_done_clears_it() {
    let ts = TestServer::spawn().await;
    let (tools, _sid) = session_tools(&ts).await;
    let (pub_, _) = blocks(&tools.publish(Parameters(PublishArgs { html: Some("<h2>Goals</h2>".into()), title: Some("T".into()), ..Default::default() })).await.unwrap());
    let aid = pub_["artifact_id"].as_str().unwrap().to_string();
    let tid = ts.thread(&aid, 1, "@agent columns").await["id"].as_str().unwrap().to_string();
    let (v, _) = blocks(&tools.working(Parameters(WorkingArgs {
        url_or_id: aid.clone(),
        thread_ids: Some(vec![tid.clone()]),
        message: Some("Two columns".into()),
        done: None,
    })).await.unwrap());
    assert_eq!(v["working"], true);
    assert_eq!(v["message"], "Two columns");
    assert_eq!(v["thread_ids"], json!([tid]));
    assert_eq!(v["expires_in_s"], 120);
    assert_eq!(v["message_truncated"], false);
    let public: Value = ts.get(&format!("/api/artifacts/{aid}/working")).await.json().await.unwrap();
    assert_eq!(public["working"][0]["message"], "Two columns");
    let (v, _) = blocks(&tools.working(Parameters(WorkingArgs { url_or_id: aid.clone(), done: Some(true), ..Default::default() })).await.unwrap());
    assert_eq!(v, json!({"artifact_id": aid, "url": v["url"], "working": false, "cleared": true, "feedback": v["feedback"]}));
}

#[tokio::test]
async fn working_refuses_bad_threads_and_the_sessionless_endpoint() {
    let ts = TestServer::spawn().await;
    let (tools, _) = session_tools(&ts).await;
    let (pub_, _) = blocks(&tools.publish(Parameters(PublishArgs { html: Some("<p>".into()), title: Some("T".into()), ..Default::default() })).await.unwrap());
    let aid = pub_["artifact_id"].as_str().unwrap().to_string();
    let err = |r: CallToolResult| -> String {
        assert_eq!(r.is_error, Some(true));
        let v: Value = serde_json::from_str(&r.content[0].as_text().unwrap().text).unwrap();
        v["error"]["code"].as_str().unwrap().to_string()
    };
    let r = tools.working(Parameters(WorkingArgs { url_or_id: aid.clone(), thread_ids: Some(vec!["x".into()]), ..Default::default() })).await.unwrap();
    assert_eq!(err(r), "invalid_args");
    let r = tools.working(Parameters(WorkingArgs { url_or_id: aid.clone(), thread_ids: Some(vec![clax_core::new_ulid()]), ..Default::default() })).await.unwrap();
    assert_eq!(err(r), "unknown_thread");
    let sessionless = ClaxTools::new(
        DaemonClient::new(ts.base.clone(), ts.token.clone(), None),
        ts.base.clone(), None, ts.home.log_path(),
    );
    let r = sessionless.working(Parameters(WorkingArgs { url_or_id: aid, ..Default::default() })).await.unwrap();
    assert_eq!(err(r), "no_session");
}
```

Run: `cargo test -p clax-mcp --test comments working`
Expected: FAIL to compile (`WorkingArgs` not found).

- [ ] **Step 2: The MCP tool**

In `crates/clax-mcp/src/tools.rs`, change the header comment to `twenty-three tools`, and add:

```rust
#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkingArgs {
    /// Artifact URL or ID.
    pub url_or_id: String,
    /// Threads of the artifact you are acting on (at most 20); replaces the ones named before.
    pub thread_ids: Option<Vec<String>>,
    /// What you are doing, in a few words (at most 140 characters).
    pub message: Option<String>,
    /// Clear it now (with `thread_ids`, only those threads).
    pub done: Option<bool>,
}
```

and in `impl ClaxTools`:

```rust
    async fn do_working(&self, a: WorkingArgs) -> Outcome {
        let id = artifact_id(&a.url_or_id)?;
        if let Some(t) = &a.thread_ids {
            if t.len() > clax_core::working::MAX_WORKING_THREADS {
                return Err(invalid("at most 20 thread_ids"));
            }
            for tid in t {
                check_thread_id(tid)?;
            }
        }
        self.require_session().await?;
        if a.done.unwrap_or(false) {
            let r = self.client.clear_working(&id, a.thread_ids.as_deref()).await.map_err(|e| self.fail(e))?;
            let still = !r["working"].is_null();
            return Ok(json!({"artifact_id": id, "url": self.artifact_url(&id), "working": still, "cleared": r["cleared"]}));
        }
        let mut body = json!({});
        if let Some(t) = &a.thread_ids {
            body["thread_ids"] = json!(t);
        }
        if let Some(m) = &a.message {
            body["message"] = json!(m);
        }
        let r = self.client.set_working(&id, &body).await.map_err(|e| self.fail(e))?;
        let w = &r["working"];
        Ok(json!({
            "artifact_id": id,
            "url": self.artifact_url(&id),
            "working": true,
            "message": w["message"],
            "thread_ids": w["thread_ids"],
            "started_at": w["started_at"],
            "expires_in_s": clax_core::working::WORKING_TTL_SECS,
            "message_truncated": r["message_truncated"],
        }))
    }
```

and in the `#[tool_router]` block after `wait_for_feedback`:

```rust
    #[tool(
        description = "Tell the person you are working on an artifact: its page shows `<harness> is working: <message>` in the header, a badge on its gallery card, and `working…` on each thread in `thread_ids`. Comments sent to you mark you working automatically; call this for other work or to add a short `message` (at most 140 characters). It clears when you reply to those threads, publish the artifact, end your turn, or go 2 minutes without a tool call; `done: true` clears it now."
    )]
    pub async fn working(
        &self,
        Parameters(args): Parameters<WorkingArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.finish(self.do_working(args).await).await
    }
```

In `crates/clax-mcp/src/client.rs`, after `unwatch`:

```rust
    /// `PUT /api/sessions/<sid>/working/<id>`: `{working, message_truncated}`.
    pub async fn set_working(&self, id: &str, body: &Value) -> Result<Value> {
        self.json(|c| c.request(reqwest::Method::PUT, &format!("{}/working/{id}", c.session_path())).json(body))
            .await
    }

    /// `DELETE /api/sessions/<sid>/working/<id>[?thread_ids=…]`: `{cleared, working}`.
    pub async fn clear_working(&self, id: &str, threads: Option<&[String]>) -> Result<Value> {
        let q: Vec<(&str, String)> = threads.map(|t| vec![("thread_ids", t.join(","))]).unwrap_or_default();
        self.json(|c| c.request(reqwest::Method::DELETE, &format!("{}/working/{id}", c.session_path())).query(&q))
            .await
    }

    /// `POST /api/sessions/<sid>/working/renew`: `{renewed}`.
    pub async fn renew_working(&self) -> Result<Value> {
        self.json(|c| c.request(reqwest::Method::POST, &format!("{}/working/renew", c.session_path())).json(&json!({})))
            .await
    }

    /// `POST /api/sessions/<sid>/working/end`: `{cleared}`.
    pub async fn end_working(&self) -> Result<Value> {
        self.json(|c| c.request(reqwest::Method::POST, &format!("{}/working/end", c.session_path())).json(&json!({})))
            .await
    }
```

In `crates/clax-mcp/tests/shim.rs`, change `.len(), 22)` to `.len(), 23)`.

Run: `cargo test -p clax-mcp`
Expected: PASS.

- [ ] **Step 3: Pi's `clax_working`**

In `plugins/pi/src/client.ts`, after `unwatch`:

```ts
  /** `PUT /api/sessions/<sid>/working/<id>`: `{working, message_truncated}`. */
  setWorking(id: string, body: { thread_ids?: string[]; message?: string }): Promise<any> {
    return this.json(() => `${this.sessionPath()}/working/${id}`, this.jsonBody("PUT", body));
  }

  /** `DELETE /api/sessions/<sid>/working/<id>`: `{cleared, working}`. */
  clearWorking(id: string, threadIds?: string[]): Promise<any> {
    const q = threadIds ? `?${new URLSearchParams({ thread_ids: threadIds.join(",") })}` : "";
    return this.json(() => `${this.sessionPath()}/working/${id}${q}`, { method: "DELETE" });
  }

  /** `POST /api/sessions/<sid>/working/renew`: `{renewed}`. Never starts a daemon. */
  renewWorking(): Promise<any> {
    return this.json(() => `${this.sessionPath()}/working/renew`, this.jsonBody("POST", {}), this.discoverFn);
  }

  /** `POST /api/sessions/<sid>/working/end`: `{cleared}`. Never starts a daemon. */
  endWorking(): Promise<any> {
    return this.json(() => `${this.sessionPath()}/working/end`, this.jsonBody("POST", {}), this.discoverFn);
  }
```

In `plugins/pi/src/clax.ts`, change the header comment to `twenty-three`, add the schema after `WaitArgs`:

```ts
const WorkingArgs = Type.Object({
  url_or_id: urlOrId,
  thread_ids: opt(Type.Array(Type.String(), { description: "Threads of the artifact you are acting on (at most 20); replaces the ones named before." })),
  message: opt(str("What you are doing, in a few words (at most 140 characters).")),
  done: opt(Type.Boolean({ description: "Clear it now (with `thread_ids`, only those threads)." })),
}, strict);
```

the method on `Tools`, after `watch`:

```ts
  async working(ctx: ExtensionContext, a: Static<typeof WorkingArgs>): Promise<Json> {
    const { id } = artifactRef(a.url_or_id);
    if (a.thread_ids) {
      if (a.thread_ids.length > 20) throw invalid("at most 20 thread_ids");
      for (const t of a.thread_ids) checkThreadId(t);
    }
    const c = this.clientFor(ctx);
    const url = this.artifactUrl(c, id);
    if (a.done) {
      const r = await this.call(() => c.clearWorking(id, a.thread_ids));
      return { artifact_id: id, url, working: r.working !== null, cleared: r.cleared };
    }
    const body: { thread_ids?: string[]; message?: string } = {};
    if (a.thread_ids) body.thread_ids = a.thread_ids;
    if (a.message !== undefined) body.message = a.message;
    const r = await this.call(() => c.setWorking(id, body));
    return {
      artifact_id: id, url, working: true, message: r.working.message, thread_ids: r.working.thread_ids,
      started_at: r.working.started_at, expires_in_s: 120, message_truncated: r.message_truncated,
    };
  }
```

and register it after `watch`:

```ts
    define("working", "Clax working",
      "Tell the person you are working on an artifact: its page shows `<harness> is working: <message>` in the header, a badge on its gallery card, and `working…` on each thread in `thread_ids`. Comments sent to you mark you working automatically; call this for other work or to add a short `message` (at most 140 characters). It clears when you reply to those threads, publish the artifact, end your turn, or go 2 minutes without a tool call; `done: true` clears it now.",
      "Show the person you are working on an Clax artifact, or clear it",
      WorkingArgs, (ctx, a) => tools.working(ctx, a));
```

In `plugins/pi/test/clax.test.ts`, rename the test to `registers the twenty-three tools with one-line prompt snippets, and the clax command`, and add to `describe("comments", ...)`:

```ts
  it("clax_working marks the artifact and done clears it, as the MCP tool does", async () => {
    const { pi, ctx } = load(daemon.home, "pi-working");
    await pi.emit("session_start", { type: "session_start", reason: "startup" }, ctx);
    const pub = JSON.parse((await pi.callTool("clax_publish", { html: "<h2>Goals</h2>", title: "W" }, ctx)).content[0].text!);
    const set = await pi.callTool("clax_working", { url_or_id: pub.artifact_id, message: "Two columns" }, ctx);
    expect(set.isError).toBe(false);
    expect(JSON.parse(set.content[0].text!)).toMatchObject({ working: true, message: "Two columns", expires_in_s: 120, message_truncated: false });
    const seen = await (await fetch(`${daemon.base}/api/artifacts/${pub.artifact_id}/working`)).json();
    expect(seen.working[0]).toMatchObject({ harness: "pi", message: "Two columns" });
    expect(JSON.stringify(seen)).not.toContain("session_id");
    const done = await pi.callTool("clax_working", { url_or_id: pub.artifact_id, done: true }, ctx);
    expect(JSON.parse(done.content[0].text!)).toMatchObject({ working: false, cleared: true });
    const bad = await pi.callTool("clax_working", { url_or_id: pub.artifact_id, thread_ids: ["x"] }, ctx);
    expect(bad.isError).toBe(true);
    expect(JSON.parse(bad.content[0].text!).error.code).toBe("invalid_args");
  });
```

`daemon.base` is `TestDaemon.base` from `plugins/pi/test/daemon-fixture.ts`.

Run: `cd plugins/pi && npm run typecheck && npm test`
Expected: PASS.

- [ ] **Step 4: The tool lists, the skills and the plugin check**

- `scripts/test-plugins.sh`: change the comment to `twenty-three`, `len(tools) != 22` to `!= 23`, `not 22` to `not 23`, and the PASS text to `the twenty-three tool descriptions match in tools.rs and clax.ts`.
- `docs/contract.md`: `Twenty-two tools:` becomes `Twenty-three tools:`, and after `` `wait_for_feedback` `` insert `` `working` `` in that list. `the same twenty-two tools` becomes `the same twenty-three tools`.
- `README.md`, `plugins/claude-code/README.md`, `plugins/clax/README.md`: `twenty-two tools` becomes `twenty-three tools`, with `` `working` `` inserted after `` `wait_for_feedback` ``. `plugins/pi/README.md`: `Twenty-two tools:` becomes `Twenty-three tools:`, with `` `clax_working` `` inserted after `` `clax_wait_for_feedback` ``.
- In each of the three `SKILL.md` files, in `## Comment loop`, insert this before the paragraph that starts `When the person wants to iterate live`. The text is identical in all three files:

```markdown
Showing that you are working: when a comment reaches you, the person's page
already shows you as working on its thread, and it clears when you reply to
that thread, publish the artifact, or end your turn. Call `working` yourself
when you start on an artifact for another reason (the person asked in the
terminal, or you are acting on several threads at once) and when a short
`message` would help ("Rebuilding the chart", at most 140 characters). Reply
to each thread after publishing the change, so the version lists the thread
as addressed before your reply clears it. Call `working` with `done: true`
when you stop without replying or publishing.

- `working` (`url_or_id`; optional `thread_ids`, `message`, `done`):
  `working: true` with the stored `message`, `thread_ids` and `expires_in_s`,
  or `working: false` after `done`.
```

Then:

```bash
python3 scripts/sync-skill-tools.py
bash scripts/test-plugins.sh | tail -1
```

Expected: `plugin checks passed`.

- [ ] **Step 5: Gates and commit**

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add plugins/pi/test/fixtures/contract.json crates/clax-mcp/src/tools.rs crates/clax-mcp/src/client.rs crates/clax-mcp/tests/comments.rs \
  crates/clax-mcp/tests/shim.rs plugins/pi/src/clax.ts plugins/pi/src/client.ts plugins/pi/test/clax.test.ts scripts/test-plugins.sh \
  docs/contract.md README.md plugins/claude-code/README.md plugins/clax/README.md plugins/pi/README.md \
  plugins/claude-code/skills/clax/SKILL.md plugins/clax/skills/clax/SKILL.md plugins/pi/skills/clax/SKILL.md
git commit -m "Add the working tool to the MCP server and the Pi extension"
git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed
```

---

### Task 6: Pi renews on every tool call and ends the turn on `agent_end`

**Files:**
- Modify: `plugins/pi/src/clax.ts`, `plugins/pi/test/clax.test.ts`

**Interfaces:**
- Consumes: `renewWorking`, `endWorking` (Task 5).
- Produces: `RENEW_EVERY_MS = 15_000` exported from `clax.ts` (the throttle), and handlers for `tool_call` and `agent_end` (Pi 0.73.1: `AgentEndEvent { type: "agent_end"; messages }`, `ToolCallEvent`).

- [ ] **Step 1: Failing tests**

Append to `describe("comments", ...)` in `plugins/pi/test/clax.test.ts`:

```ts
  it("tool calls renew working records at most every 15 s, and agent_end ends them", async () => {
    const { pi, ctx } = load(daemon.home, "pi-renew");
    await pi.emit("session_start", { type: "session_start", reason: "startup" }, ctx);
    const pub = JSON.parse((await pi.callTool("clax_publish", { html: "<p>r</p>", title: "R" }, ctx)).content[0].text!);
    await pi.callTool("clax_working", { url_or_id: pub.artifact_id, message: "m" }, ctx);
    const renews: number[] = [];
    const spy = vi.spyOn(DaemonClient.prototype, "renewWorking").mockImplementation(async function (this: DaemonClient) { renews.push(Date.now()); return { renewed: 1 }; });
    vi.useFakeTimers({ toFake: ["Date"] });
    try {
      await pi.emit("tool_call", { type: "tool_call", toolName: "bash", toolCallId: "1", input: {} }, ctx);
      await pi.emit("tool_call", { type: "tool_call", toolName: "edit", toolCallId: "2", input: {} }, ctx);
      expect(renews.length).toBe(1);
      vi.setSystemTime(Date.now() + RENEW_EVERY_MS);
      await pi.emit("tool_call", { type: "tool_call", toolName: "read", toolCallId: "3", input: {} }, ctx);
      expect(renews.length).toBe(2);
    } finally {
      vi.useRealTimers();
      spy.mockRestore();
    }
    await pi.emit("agent_end", { type: "agent_end", messages: [] }, ctx);
    const seen = await (await fetch(`${daemon.base}/api/artifacts/${pub.artifact_id}/working`)).json();
    expect(seen.working).toEqual([]);
  });
```

Add `RENEW_EVERY_MS` to the import from `../src/clax`, `DaemonClient` from `../src/client`, and `vi` from `vitest` where missing.

Run: `cd plugins/pi && npm test -- -t "renew working"`
Expected: FAIL.

- [ ] **Step 2: Implement**

In `claxExtension`, after the `session_shutdown` handler:

```ts
    // Working records (spec §10 "Working"): any tool call renews them, at most
    // every RENEW_EVERY_MS; the end of the agent loop ends the turn's records.
    // Neither starts a daemon, registers a session, or delays the tool.
    let lastRenew = 0;
    pi.on("tool_call", async () => {
      const c = tools.existingClient();
      if (!c?.session() || Date.now() - lastRenew < RENEW_EVERY_MS) return;
      lastRenew = Date.now();
      void c.renewWorking().catch(() => undefined);
    });
    pi.on("agent_end", async () => {
      const c = tools.existingClient();
      if (!c?.session()) return;
      lastRenew = 0;
      await c.endWorking().catch(() => undefined);
    });
```

and at module level, with the other constants:

```ts
/** The shortest gap between two renewals of the session's working records. */
export const RENEW_EVERY_MS = 15_000;
```

Run: `cd plugins/pi && npm run typecheck && npm test`
Expected: PASS.

- [ ] **Step 3: Gates and commit**

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add plugins/pi/src/clax.ts plugins/pi/test/clax.test.ts
git commit -m "Renew working records on Pi tool calls and end them when the agent loop ends"
git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed
```

---

### Task 7: Hooks: the Stop hook ends the turn, and a `tool` hook renews

**Files:**
- Modify: `crates/clax-hooks/src/events.rs`, `crates/clax-cli/src/commands/hook.rs`, `crates/clax-hooks/tests/golden.rs`, `plugins/claude-code/hooks/hooks.json`, `plugins/clax/hooks/hooks.json`, `scripts/test-plugins.sh`, `scripts/quality_gates.sh`, `scripts/smoke-codex.sh`, `plugins/claude-code/README.md`, `plugins/clax/README.md`
- Create: `crates/clax-hooks/tests/fixtures/claude-post-tool-use.json`, `crates/clax-hooks/tests/fixtures/codex-post-tool-use.json`, `scripts/tool-hook.sh`, `plugins/claude-code/scripts/tool-hook.sh`, `plugins/clax/scripts/tool-hook.sh`, `scripts/test-tool-hook.sh`

**Interfaces:**
- Produces: `clax_hooks::events::tool(harness: &str, input: &HookInput, daemon: &dyn Daemon) -> anyhow::Result<HookOutput>` (always `HookOutput::none()`).
- Changes: `clax_hooks::events::stop` posts `/api/sessions/<sid>/working/end` when it allows the stop.
- Produces: `clax hook --agent <claude|codex> tool`, with a budget of 2 s for the whole run and 1 s per request.
- Produces: `scripts/tool-hook.sh <claude|codex>`, the throttled `PostToolUse` command (Step 4). The stamp is at `${CLAX_HOME:-$HOME/.clax}/run/tool-hook/<harness>-<session ID>`, and the script always exits 0.

- [ ] **Step 1: Failing unit tests**

In `crates/clax-hooks/src/events.rs` tests, make `FeedbackFake` record posts: add `posts: RefCell<Vec<String>>` to the struct and to `fake()`, and have `post` push `path.to_string()` before answering. Then add:

```rust
    #[test]
    fn stop_ends_the_turn_only_when_it_allows_the_stop() {
        let d = fake(Some("[clax] 1 comment sent to you:\nX"));
        stop("claude", &HookInput::parse(r#"{"session_id":"s1"}"#), &d).unwrap();
        assert!(d.posts.borrow().is_empty(), "a blocked stop continues the turn");
        let d = fake(None);
        assert_eq!(stop("claude", &HookInput::parse(r#"{"session_id":"s1","stop_hook_active":true}"#), &d).unwrap(), HookOutput::none());
        assert_eq!(*d.posts.borrow(), ["/api/sessions/S/working/end"]);
    }

    #[test]
    fn tool_renews_and_prints_nothing() {
        let d = fake(None);
        assert_eq!(tool("claude", &HookInput::parse(r#"{"session_id":"s1"}"#), &d).unwrap(), HookOutput::none());
        assert_eq!(*d.posts.borrow(), ["/api/sessions/S/working/renew"]);
        let d = fake(None);
        assert_eq!(tool("claude", &HookInput::parse(r#"{"session_id":"other"}"#), &d).unwrap(), HookOutput::none());
        assert!(d.posts.borrow().is_empty(), "no live session: nothing to renew");
    }
```

Run: `cargo test -p clax-hooks --lib`
Expected: FAIL to compile (`tool` not found).

- [ ] **Step 2: Implement**

In `stop`, replace the final `Ok(match ... )` with:

```rust
    match feedback_text(daemon, &sid, &format!("tier=stop_hook&resends={resends}"))? {
        Some(text) => Ok(HookOutput::block(&text)),
        None => {
            // The turn really ends: its working records end with it.
            daemon.post(&format!("/api/sessions/{sid}/working/end"), &json!({}))?;
            Ok(HookOutput::none())
        }
    }
```

and extend its doc comment: ``When it allows the stop, it ends the session's working records (the turn is over).`` Add:

```rust
/// `PostToolUse`: renews the session's working records. Prints nothing.
///
/// # Errors
/// When the input has no `session_id` or a daemon request fails.
pub fn tool(harness: &str, input: &HookInput, daemon: &dyn Daemon) -> anyhow::Result<HookOutput> {
    if let Some(sid) = live_session(harness, input, daemon)? {
        daemon.post(&format!("/api/sessions/{sid}/working/renew"), &json!({}))?;
    }
    Ok(HookOutput::none())
}
```

In `crates/clax-cli/src/commands/hook.rs`: add `const TOOL_DEADLINE: Duration = Duration::from_secs(2);` and `const TOOL_REQUEST_TIMEOUT: Duration = Duration::from_secs(1);` with doc comments. Add the variant `/// A tool call finished; renew the session's working records.\n    Tool,` to `Event`, `Event::Tool => (TOOL_DEADLINE, TOOL_REQUEST_TIMEOUT)` to `budget`, `Event::Tool => "tool"` to `name`, and `Event::Tool => events::tool(agent.harness(), &input, &client),` to `handle`.

Run: `cargo test -p clax-hooks --lib`
Expected: PASS.

- [ ] **Step 3: Golden tests against a real daemon**

`crates/clax-hooks/tests/fixtures/claude-post-tool-use.json`:

```json
{"session_id":"cc-hook-1","transcript_path":"/tmp/t.jsonl","cwd":"/tmp/project","permission_mode":"default","hook_event_name":"PostToolUse","tool_name":"Edit","tool_input":{"file_path":"/tmp/project/a.txt"},"tool_response":{"success":true},"tool_use_id":"toolu_1"}
```

`crates/clax-hooks/tests/fixtures/codex-post-tool-use.json`:

```json
{"session_id":"cx-hook-1","transcript_path":null,"cwd":"/tmp/project","hook_event_name":"PostToolUse","model":"gpt-5","tool_name":"shell","tool_input":{"command":["ls"]},"tool_response":"ok"}
```

Add `"tool"` to the event lists in `unusable_stdin_prints_nothing` and `no_daemon_prints_nothing_and_starts_none`. Append:

```rust
impl Daemon {
    fn working(&self, aid: &str) -> Vec<Value> {
        let v: Value = self.http().get(format!("{}/api/artifacts/{aid}/working", self.base())).send().unwrap().json().unwrap();
        v["working"].as_array().unwrap().clone()
    }
    fn skew(&self, secs: i64) {
        let res = self.http().post(format!("{}/api/_test/working/skew", self.base()))
            .bearer_auth(self.token()).json(&serde_json::json!({"secs": secs})).send().unwrap();
        assert_eq!(res.status(), 200);
    }
}

fn working_through_a_turn(agent: &str, hsid: &str) {
    let d = Daemon::start();
    let (_sid, aid) = d.session_with_artifact(agent, hsid);
    d.sent_thread(&aid, "tighten the spacing");
    let mut stop_in: Value = serde_json::from_slice(&fixture(&format!("{agent}-stop.json"))).unwrap();
    stop_in["session_id"] = hsid.into();
    let r = hook(&d.home(), agent, "stop", stop_in.to_string().as_bytes());
    assert_eq!(one_line_json(&r.stdout)["decision"], "block");
    assert_eq!(d.working(&aid).len(), 1, "the blocked stop handed the comment over and marked the session");
    d.skew(100);
    let mut tool_in: Value = serde_json::from_slice(&fixture(&format!("{agent}-post-tool-use.json"))).unwrap();
    tool_in["session_id"] = hsid.into();
    let r = hook(&d.home(), agent, "tool", tool_in.to_string().as_bytes());
    assert_eq!((r.code, r.stdout.as_str()), (Some(0), ""));
    d.skew(100);
    assert_eq!(d.working(&aid).len(), 1, "the tool hook renewed at 100 s");
    stop_in["stop_hook_active"] = true.into();
    let r = hook(&d.home(), agent, "stop", stop_in.to_string().as_bytes());
    assert_eq!(r.stdout, "");
    assert!(d.working(&aid).is_empty(), "the turn ended");
}

#[test]
fn claude_working_through_a_turn() {
    working_through_a_turn("claude", "cc-work-1");
}

#[test]
fn codex_working_through_a_turn() {
    working_through_a_turn("codex", "cx-work-1");
}
```

`session_with_artifact` registers with `harness_session_id` = `hsid`, so the hooks find the session by the fixture's re-keyed `session_id`.

Run: `cargo test -p clax-hooks --test golden`
Expected: PASS.

- [ ] **Step 4: The throttle gate, test first**

The person's ruling: the `PostToolUse` hook runs a tiny shell check first and starts `clax` only when this session's last renewal is at least 60 s old. The gate is one POSIX `sh` script, `scripts/tool-hook.sh`, copied byte for byte into both plugins (as `ensure-clax.sh` is).

- **Where the stamp lives:** `${CLAX_HOME:-$HOME/.clax}/run/tool-hook/<harness>-<session ID>`, one file per harness session. The session ID comes from the hook input's `session_id`, and only the characters `A-Z a-z 0-9 . _ -` are accepted, so a hostile value cannot leave that directory. It never lives under `~/.claude` or `~/.codex`. The `session-end` hook deletes it.
- **Portability:** the gate compares the stamp's modification time with the clock using `date -r FILE +%s` (a file's mtime in epoch seconds) and `date +%s`. BSD date (macOS), GNU coreutils and BusyBox all support both. It does not use `stat`, whose flags differ (`-f %m` against `-c %Y`), or `find -mmin`, which rounds the age up to whole minutes on BSD and not on GNU. A stamp that is missing, unreadable or dated in the future counts as old.
- **Cost of a skipped call:** one `sh`, one `cat` and two `date` runs. No `clax`, no network.
- **Always exits 0 and prints nothing.** When the gate runs `clax`, its output goes to `/dev/null` and its status is ignored: `clax hook` logs its own failures to `hooks.log`.

`scripts/test-tool-hook.sh`:

```bash
#!/usr/bin/env bash
# Tests the PostToolUse gate (scripts/tool-hook.sh) against a fake
# ensure-clax.sh that records each run. Uses a scratch HOME and CLAX_HOME.
set -uo pipefail
cd "$(dirname "$0")/.."
FAILED=0
fail() { echo "FAIL: $1"; FAILED=1; }
pass() { echo "PASS: $1"; }
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
mkdir -p "$T/bin" "$T/fakehome"
cp scripts/tool-hook.sh "$T/bin/tool-hook.sh"
cat > "$T/bin/ensure-clax.sh" <<'SH'
#!/bin/sh
{ printf '%s|' "$*"; cat; echo; } >> "$CALLS"
echo '{"noise": true}'
echo 'noise' >&2
exit 3
SH
chmod +x "$T/bin/tool-hook.sh" "$T/bin/ensure-clax.sh"
export HOME="$T/fakehome" CLAX_HOME="$T/home" CALLS="$T/calls"
STAMPS="$CLAX_HOME/run/tool-hook"
calls() { if [ -f "$CALLS" ]; then wc -l < "$CALLS" | tr -d ' '; else echo 0; fi; }
gate() { printf '%s' "$2" | "$T/bin/tool-hook.sh" "$1" 2>"$T/err"; }
age() { python3 -c 'import os, sys, time; t = time.time() + float(sys.argv[2]); os.utime(sys.argv[1], (t, t))' "$1" "$2"; }
IN='{"session_id":"cc-1","hook_event_name":"PostToolUse","tool_name":"Edit"}'

out="$(gate claude "$IN")"; code=$?
if [ "$code" = 0 ] && [ -z "$out" ] && [ ! -s "$T/err" ]; then pass "exits 0 and prints nothing, even when clax fails and writes output"
else fail "exit $code, stdout '$out', stderr '$(cat "$T/err")'"; fi
if [ "$(calls)" = 1 ] && grep -qF "exec hook --agent claude tool|$IN" "$CALLS"; then pass "the first call runs clax hook ... tool with the hook input"
else fail "first call: $(cat "$CALLS" 2>/dev/null)"; fi
if [ -f "$STAMPS/claude-cc-1" ]; then pass "the stamp is under CLAX_HOME/run/tool-hook"; else fail "no stamp at $STAMPS/claude-cc-1"; fi
if [ -z "$(ls -A "$HOME")" ]; then pass "nothing is written under HOME (no ~/.claude, no ~/.codex)"; else fail "HOME holds: $(ls -A "$HOME")"; fi

gate claude "$IN" >/dev/null
if [ "$(calls)" = 1 ]; then pass "a call within 60 s starts no clax"; else fail "throttle: $(calls) runs"; fi
age "$STAMPS/claude-cc-1" -59
gate claude "$IN" >/dev/null
if [ "$(calls)" = 1 ]; then pass "a stamp 59 s old still skips"; else fail "59 s: $(calls) runs"; fi
age "$STAMPS/claude-cc-1" -61
gate claude "$IN" >/dev/null
if [ "$(calls)" = 2 ]; then pass "a stamp 61 s old renews again"; else fail "61 s: $(calls) runs"; fi
age "$STAMPS/claude-cc-1" 3600
gate claude "$IN" >/dev/null
if [ "$(calls)" = 3 ]; then pass "a stamp dated in the future counts as old"; else fail "future: $(calls) runs"; fi

gate claude '{"session_id": "cc-2"}' >/dev/null
gate codex '{"session_id":"cc-1"}' >/dev/null
if [ "$(calls)" = 5 ] && [ -f "$STAMPS/claude-cc-2" ] && [ -f "$STAMPS/codex-cc-1" ]; then pass "stamps are per harness and per session (spaced JSON too)"
else fail "per session: $(calls) runs, $(ls "$STAMPS")"; fi

gate claude '{"hook_event_name":"PostToolUse"}' >/dev/null
gate claude '{"hook_event_name":"PostToolUse"}' >/dev/null
gate claude '{"session_id":"../../escape"}' >/dev/null
if [ "$(calls)" = 8 ] && [ "$(ls "$STAMPS" | sort | tr '\n' ' ')" = "claude-cc-1 claude-cc-2 codex-cc-1 " ] && [ ! -e "$CLAX_HOME/escape" ]; then
  pass "without a usable session_id every call runs clax and no stamp is written"
else fail "no session: $(calls) runs, $(ls -R "$CLAX_HOME")"; fi

out="$(gate nope "$IN")"; code=$?
if [ "$code" = 0 ] && [ -z "$out" ] && [ "$(calls)" = 8 ]; then pass "an unknown harness does nothing and exits 0"; else fail "unknown harness: $code '$out' $(calls)"; fi

chmod 000 "$STAMPS"
out="$(gate claude '{"session_id":"cc-3"}')"; code=$?
chmod 755 "$STAMPS"
if [ "$code" = 0 ] && [ -z "$out" ] && [ "$(calls)" = 9 ]; then pass "an unwritable stamp directory still renews and exits 0"; else fail "unwritable: $code '$out' $(calls)"; fi

if [ "$FAILED" -ne 0 ]; then echo "tool hook gate checks failed"; exit 1; fi
echo "tool hook gate checks passed"
```

Run: `chmod +x scripts/test-tool-hook.sh && scripts/test-tool-hook.sh`
Expected: FAIL (`scripts/tool-hook.sh` does not exist).

`scripts/tool-hook.sh`:

```sh
#!/bin/sh
# PostToolUse gate for `clax hook --agent <harness> tool` (spec §13).
# Working records lapse 120 s after their last renewal, so renewing once a
# minute is enough: this starts clax only when this session's stamp file is
# at least 60 s old, and otherwise exits after a cat and two date calls.
# It always exits 0 and prints nothing, so it can never fail or steer the
# harness; clax logs its own failures to hooks.log.
#
# Usage: tool-hook.sh <claude|codex>     (the hook's JSON on stdin)
# Stamp: ${CLAX_HOME:-$HOME/.clax}/run/tool-hook/<harness>-<session ID>
# Portable: `date +%s` and `date -r FILE +%s` (a file's mtime) behave the
# same in BSD date (macOS), GNU coreutils and BusyBox; `stat` and
# `find -mmin` do not.
agent="$1"
case "$agent" in claude|codex) ;; *) exit 0 ;; esac
input="$(cat)"
sid=""
for pat in '"session_id":"' '"session_id": "'; do
  rest="${input#*"$pat"}"
  if [ "$rest" != "$input" ]; then
    sid="${rest%%\"*}"
    break
  fi
done
case "$sid" in ''|*[!A-Za-z0-9._-]*) sid="" ;; esac
if [ -n "$sid" ]; then
  dir="${CLAX_HOME:-$HOME/.clax}/run/tool-hook"
  stamp="$dir/$agent-$sid"
  now="$(date +%s)"
  last="$(date -r "$stamp" +%s 2>/dev/null)" || last=0
  age=$((now - last))
  if [ "$age" -ge 0 ] && [ "$age" -lt 60 ]; then
    exit 0
  fi
  { mkdir -p "$dir" && : > "$stamp"; } 2>/dev/null
fi
printf '%s' "$input" | "${0%/*}/ensure-clax.sh" exec hook --agent "$agent" tool >/dev/null 2>&1
exit 0
```

Then:

```bash
chmod +x scripts/tool-hook.sh
cp scripts/tool-hook.sh plugins/claude-code/scripts/tool-hook.sh
cp scripts/tool-hook.sh plugins/clax/scripts/tool-hook.sh
scripts/test-tool-hook.sh
```

Expected: every line `PASS`, and the last line `tool hook gate checks passed`.

In `scripts/quality_gates.sh`, after the `installer` line, add `run "tool hook gate"          scripts/test-tool-hook.sh`.

The stamp is touched before `clax` runs. Concurrent tool calls within the minute then start at most one `clax`, and a failed renewal is retried a minute later. The record's 120 s lifetime covers that gap.

- [ ] **Step 5: `session-end` removes the stamp**

In `crates/clax-cli/src/commands/hook.rs`, `handle`, change the `Event::SessionEnd` arm to:

```rust
        Event::SessionEnd => {
            let out = events::session_end(agent.harness(), &input, &client);
            if let Some(sid) = input.session_id.as_deref().filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))) {
                let _ = std::fs::remove_file(home.root().join("run/tool-hook").join(format!("{}-{sid}", agent.harness())));
            }
            out
        }
```

In `crates/clax-hooks/tests/golden.rs` `lifecycle`, before running `session-end`, create `d.home().join("run/tool-hook").join(format!("{agent}-{harness_session_id}"))` (with `create_dir_all` on its parent). After the hook, assert that it no longer exists.

- [ ] **Step 6: Wire the hooks**

`plugins/claude-code/hooks/hooks.json`, add after `Stop`:

```json
    "PostToolUse": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "\"${CLAUDE_PLUGIN_ROOT}/scripts/tool-hook.sh\" claude",
            "timeout": 5
          }
        ]
      }
    ],
```

`plugins/clax/hooks/hooks.json`, add after `Stop`:

```json
    "PostToolUse": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "bash \"${PLUGIN_ROOT}/scripts/tool-hook.sh\" codex",
            "timeout": 5
          }
        ]
      }
    ],
```

`scripts/test-plugins.sh`:
- In the installer-copy loop, add a loop that checks each plugin's `scripts/tool-hook.sh` is byte-identical to `scripts/tool-hook.sh` (`cmp -s`) and executable, with PASS/FAIL lines worded like the `ensure-clax.sh` ones.
- In the "Stop and prompt hooks are wired" check, add:

```python
ok = ok and [h["command"] for h in cmds(claude, "PostToolUse")] == ['"${CLAUDE_PLUGIN_ROOT}/scripts/tool-hook.sh" claude'] and all(h["timeout"] == 5 for h in cmds(claude, "PostToolUse"))
ok = ok and [h["command"] for h in cmds(codex, "PostToolUse")] == ['bash "${PLUGIN_ROOT}/scripts/tool-hook.sh" codex'] and all(h["timeout"] == 5 for h in cmds(codex, "PostToolUse"))
```

  and rename its PASS text to `Stop, prompt and PostToolUse hooks are wired`. The existing Codex check (`SessionStart`, `SessionEnd`, `Stop` run `exec hook --agent codex`) and the Claude quoting check keep their event lists: the `PostToolUse` command is the gate, checked above.

`plugins/claude-code/README.md`: after the `Stop` item, add

```markdown
  - `PostToolUse` (`scripts/tool-hook.sh claude`, 5 s): keeps the page's
    "working" status alive while the agent uses tools. A shell check skips
    starting `clax` unless this session's stamp file
    (`~/.clax/run/tool-hook/`) is at least a minute old, so most tool calls
    cost only a few small shell commands. It always exits 0.
```

and append to the `Stop` item: ``When nothing blocks, the turn is over: the page stops showing the agent as working.``

`plugins/clax/README.md`: in the Hooks paragraph, after `hands over comments sent to the session at the end of a turn`, insert ``, and when nothing is waiting ends the page's "working" status; `PostToolUse` (`scripts/tool-hook.sh codex`, 5 s) keeps that status alive, starting `clax` at most once a minute (not yet measured on Codex; see `docs/contract.md`)``.

- [ ] **Step 7: The manual Codex check (edit only; not run by this task)**

In `scripts/smoke-codex.sh`, inside the `--hooks` branch of the session check (after the `with_id` check), add to the Python block:

```python
if hooks:
    log = open(os.path.join(os.environ["CLAX_HOME"], "logs", "hooks.log")).read()
    ran = "agent=codex event=tool" in log  # the gate lets the first call of a session through
    print("smoke: " + ("the PostToolUse hook ran (renewal on every tool call works on this Codex)"
          if ran else "the PostToolUse hook did not run: Codex renews working records only on clax tool calls and the Stop hook"))
```

This prints the finding and fails nothing. The person runs `scripts/smoke-codex.sh --hooks` (it reads `~/.codex/auth.json`, which this plan must not), and the controller records the result in `docs/contract.md` ("Working" table, Codex row). Until then the contract keeps "not yet measured".

- [ ] **Step 8: Gates and commit**

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add crates/clax-hooks/src/events.rs crates/clax-cli/src/commands/hook.rs crates/clax-hooks/tests/golden.rs \
  crates/clax-hooks/tests/fixtures/claude-post-tool-use.json crates/clax-hooks/tests/fixtures/codex-post-tool-use.json \
  plugins/claude-code/hooks/hooks.json plugins/clax/hooks/hooks.json scripts/test-plugins.sh scripts/smoke-codex.sh \
  plugins/claude-code/README.md plugins/clax/README.md scripts/quality_gates.sh scripts/tool-hook.sh \
  plugins/claude-code/scripts/tool-hook.sh plugins/clax/scripts/tool-hook.sh scripts/test-tool-hook.sh
git commit -m "End working records when the Stop hook allows the stop, and renew them after tool calls at most once a minute"
git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed
```

---

### Task 8: The comment-loop smoke shows working on delivery and clearing on reply

**Files:**
- Modify: `scripts/smoke-comment-loop.sh`

- [ ] **Step 1: Assert the working state around tiers 1, 2 and the reply**

In the Python block, add a helper after `http(...)`:

```python
def working(aid):
    return http("GET", f"/api/artifacts/{aid}/working")["working"]
```

After step 3's `ok("tier 1: delivered once")`, add:

```python
w = working(aid)
if len(w) != 1 or w[0]["harness"] != "claude" or w[0]["thread_ids"] != [t1["id"]] or "session_id" in w[0]:
    fail(f"working after tier 1 delivery: {w}")
ok(f"working: 'Claude Code is working on 1 comment' appeared when the comment was delivered (key {w[0]['key']})")
```

After step 4's `ok(f"tier 2: the Stop hook blocked with thread ...")` line, the `stop(True)` call has already allowed the stop, so add:

```python
if working(aid):
    fail(f"working after the Stop hook allowed the stop: {working(aid)}")
ok("working: cleared when the Stop hook allowed the stop (the turn ended)")
```

In step 5, after the `wait_for_feedback` assertion `ok(...)`, add:

```python
w = working(aid)
if [x["thread_ids"] for x in w] != [[sent_at["thread"]]]:
    fail(f"working after wait_for_feedback: {w}")
ok("working: wait_for_feedback returning a comment marked its thread")
```

In step 6, before `reply, _ = shim.call("comments_reply", ...)`, re-mark t1 explicitly and check the reply clears it:

```python
set_, _ = shim.call("working", {"url_or_id": aid, "thread_ids": [t1["id"]], "message": "Two columns"})
if not set_["working"] or set_["message"] != "Two columns":
    fail(f"working tool: {set_}")
ok("working tool: the header now reads 'Claude Code is working: Two columns'")
```

and after `ok(f"agent reply shown as ...")`:

```python
if any(t1["id"] in x["thread_ids"] for x in working(aid)):
    fail(f"working still names thread 1 after the reply: {working(aid)}")
ok("working: the agent's reply to thread 1 took it out of the working record")
```

- [ ] **Step 2: Run it**

Run: `scripts/smoke-comment-loop.sh`
Expected: every step prints `PASS`, including the five new `working` lines, and the last line is `comment loop smoke passed`.

- [ ] **Step 3: Gates and commit**

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add scripts/smoke-comment-loop.sh
git commit -m "Show the working signal in the comment-loop smoke"
git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed
```

---

### Task 9: The shell shows who is working: header, gallery badge, thread marker

The design questions (the header text when several sessions work, the badge, the marker, phone width) are settled in "Design: the working record". There is nothing left to ask before starting. This task targets the post-port Svelte shell: the controller owns the list, a framework-free model computes the text, and the islands render it.

**Files:**
- Create: `web/shell/src/view/working-model.ts`, `web/shell/src/view/working-model.test.ts`, `web/shell/src/ui/WorkingStatus.svelte`, `web/shell/src/ui/WorkingBadge.svelte`, `web/shell/src/ui/working-feed.svelte.ts`, `web/shell/src/working-status.test.ts`
- Modify: `web/shell/src/api.ts`, `web/shell/src/events.ts`, `web/shell/src/events.test.ts`, `web/shell/src/view/artifact-controller.ts`, `web/shell/src/view/artifact-controller.test.ts`, `web/shell/src/ui/TopbarIsland.svelte`, `web/shell/src/ui/SidebarIsland.svelte`, `web/shell/src/ui/Sidebar.svelte`, `web/shell/src/ui/ThreadCard.svelte`, `web/shell/src/sidebar.test.ts`, `web/shell/src/ui/Gallery.svelte`, `web/shell/src/gallery.test.ts`, `web/shell/src/theme.css`

**Interfaces:**
- Produces (`view/working-model.ts`, no `svelte` import): `type Working = { key: string; harness: string; message: string | null; thread_ids: string[]; started_at: string; last_heartbeat: string }`, `harnessLabel(h: string): string`, `recordLine(w: Working): string`, `statusParts(list: Working[]): { who: string; detail: string; more: string; title: string } | null`, `badgeText(list: Working[]): string | null`, `threadMarker(list: Working[], threadId: string): string | null`, `newestFirst(list: Working[]): Working[]`.
- Produces (`events.ts`): the `working` member of `ArtifactEvent` (`{ type: "working"; artifact_id: string; working: Working[] }`), a `working` listener in `subscribe`, and `subscribeWorking(onEvent: (e: ArtifactEvent) => void): () => void`. `subscribeWorking` opens `/api/events?types=working`, and is a no-op returning a no-op when `EventSource` is undefined.
- Produces (`api.ts`): `Artifact.working?: Working[]`.
- Produces (`ArtifactController`): `ViewState.working: Working[]` (initially `[]`).
- Produces (`ui/working-feed.svelte.ts`): `class WorkingFeed { byId: Record<string, Working[]> ($state); seed(list: Artifact[]): void; start(onResync: () => void): void; stop(): void }`.
- Components: `WorkingStatus` (`{ list: Working[] }`), `WorkingBadge` (`{ list: Working[] }`). `ThreadCard` gains the prop `marker?: string | null`, and `Sidebar` gains `working?: Working[]`.

- [ ] **Step 1: Pure logic, test first**

`web/shell/src/view/working-model.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { badgeText, harnessLabel, statusParts, threadMarker, type Working } from "./working-model";

const w = (over: Partial<Working>): Working => ({
  key: "k", harness: "claude", message: null, thread_ids: [], started_at: "2026-09-30T10:00:00.000Z", last_heartbeat: "2026-09-30T10:00:00.000Z", ...over,
});

describe("working-model", () => {
  it("labels harnesses", () => {
    expect([harnessLabel("claude"), harnessLabel("codex"), harnessLabel("pi"), harnessLabel("zed")]).toEqual(["Claude Code", "Codex", "Pi", "zed"]);
  });

  it("reads one record with a message, with threads, or bare", () => {
    expect(statusParts([w({ message: "Two columns" })])).toEqual({ who: "Claude Code is working", detail: ": Two columns", more: "", title: "Claude Code is working: Two columns" });
    expect(statusParts([w({ thread_ids: ["a"] })])!.detail).toBe(" on 1 comment");
    expect(statusParts([w({ thread_ids: ["a", "b"] })])!.detail).toBe(" on 2 comments");
    expect(statusParts([w({})])!.detail).toBe("");
    expect(statusParts([])).toBeNull();
  });

  it("shows the newest of several and lists all in the title", () => {
    const list = [w({ key: "a", message: "old" }), w({ key: "b", harness: "codex", message: "new", started_at: "2026-09-30T10:05:00.000Z" })];
    expect(statusParts(list)).toEqual({ who: "Codex is working", detail: ": new", more: " (+1 more)", title: "Codex is working: new\nClaude Code is working: old" });
  });

  it("badges one record by harness and several by count", () => {
    expect(badgeText([w({})])).toBe("Claude Code working");
    expect(badgeText([w({}), w({ key: "b", harness: "pi" })])).toBe("2 agents working");
    expect(badgeText([])).toBeNull();
  });

  it("marks a thread named by a record", () => {
    const list = [w({ thread_ids: ["t1"] }), w({ key: "b", harness: "codex", thread_ids: ["t1"], started_at: "2026-09-30T10:01:00.000Z" })];
    expect(threadMarker(list, "t1")).toBe("Codex is working…");
    expect(threadMarker(list, "t2")).toBeNull();
  });
});
```

Run: `cd web && npx vitest run shell/src/view/working-model.test.ts`
Expected: FAIL (module not found).

`web/shell/src/view/working-model.ts`:

```ts
// The working signal as the shell shows it (spec §8): pure functions over the
// daemon's working views, shared by the header, the gallery and the sidebar.

/** A working record as `GET /api/artifacts/<id>` and the `working` event carry it. */
export type Working = { key: string; harness: string; message: string | null; thread_ids: string[]; started_at: string; last_heartbeat: string };

const LABELS: Record<string, string> = { claude: "Claude Code", codex: "Codex", pi: "Pi" };

export function harnessLabel(h: string): string {
  return LABELS[h] ?? h;
}

/** Newest `started_at` first; the daemon sends them so, this keeps it true. */
export function newestFirst(list: Working[]): Working[] {
  return [...list].sort((a, b) => b.started_at.localeCompare(a.started_at) || b.key.localeCompare(a.key));
}

function detail(w: Working): string {
  if (w.message) return `: ${w.message}`;
  const n = w.thread_ids.length;
  return n ? ` on ${n} comment${n === 1 ? "" : "s"}` : "";
}

/** One record as the header reads it. */
export function recordLine(w: Working): string {
  return `${harnessLabel(w.harness)} is working${detail(w)}`;
}

/** The header's parts for `list`; `title` lists every record. Null when nobody works. */
export function statusParts(list: Working[]): { who: string; detail: string; more: string; title: string } | null {
  const sorted = newestFirst(list);
  const top = sorted[0];
  if (!top) return null;
  return {
    who: `${harnessLabel(top.harness)} is working`,
    detail: detail(top),
    more: sorted.length > 1 ? ` (+${sorted.length - 1} more)` : "",
    title: sorted.map(recordLine).join("\n"),
  };
}

/** The gallery card's badge. */
export function badgeText(list: Working[]): string | null {
  if (list.length === 0) return null;
  return list.length === 1 ? `${harnessLabel(list[0].harness)} working` : `${list.length} agents working`;
}

/** The marker on a thread card: the newest record naming the thread. */
export function threadMarker(list: Working[], threadId: string): string | null {
  const w = newestFirst(list).find(x => x.thread_ids.includes(threadId));
  return w ? `${harnessLabel(w.harness)} is working…` : null;
}
```

Run: `cd web && npx vitest run shell/src/view/working-model.test.ts`
Expected: PASS.

- [ ] **Step 2: Types, events, controller state**

`api.ts`: `import type { Working } from "./view/working-model";`. Add to `Artifact`: ``/** From `GET /api/artifacts`, `GET /api/artifacts/<id>` and the bootstrap block: who is working on it now (never a session ID). */ working?: Working[];``.

`events.ts`: add `| { type: "working"; artifact_id: string; working: Working[] }` to `ArtifactEvent`, and `"working"` to the listener name list in `subscribe`. Add:

```ts
/** The gallery's stream: `working` events for every artifact (no artifact
 * filter), plus `ready`, `resync` and `stream_down` as in `subscribe`. Opened
 * after the gallery has rendered; a no-op where EventSource is missing. */
export function subscribeWorking(onEvent: (e: ArtifactEvent) => void): () => void {
  if (typeof EventSource === "undefined") return () => {};
  const es = new EventSource("/api/events?types=working");
  es.addEventListener("working", (e: MessageEvent) => { try { onEvent(JSON.parse(e.data)); } catch { /* ignore malformed */ } });
  es.addEventListener("ready", () => onEvent({ type: "ready" }));
  es.addEventListener("error", () => onEvent({ type: "stream_down" }));
  es.addEventListener("resync", () => onEvent({ type: "resync", dropped: 0 }));
  return () => es.close();
}
```

In `events.test.ts`, extend the existing listener test's expected name list with `"working"`. Add a test for `subscribeWorking` in the same style as that file's `subscribe` tests: a stub `EventSource` class records its URL (`/api/events?types=working`) and its listeners, and a dispatched `working` message reaches `onEvent` parsed.

`view/artifact-controller.ts`:
- `ViewState` gains `/** Working records on this artifact, newest first (spec §8). */ working: Working[];`, initial `[]`.
- Every `this.set({ data: … })` (the load, and the bootstrap seed from the port's Task 12) also sets `working: <that data>.artifact.working ?? []`.
- `onEvent`: add `if (e.type === "working") this.set({ working: e.working });`. In the `resync`/`ready` refetch's `.then(d => …)`, add `this.set({ working: d.artifact.working ?? [] });`.
- `working` does not take part in `react()`. A working change must never re-resolve anchors or refocus the frame.

In `view/artifact-controller.test.ts`, make `FakeES` dispatchable:

```ts
class FakeES {
  static last: FakeES | null = null;
  private ls = new Map<string, (e: MessageEvent) => void>();
  constructor() { FakeES.last = this; }
  addEventListener(n: string, f: (e: MessageEvent) => void) { this.ls.set(n, f); }
  close() {}
  emit(n: string, d: unknown) { this.ls.get(n)?.({ data: JSON.stringify(d) } as MessageEvent); }
}
```

and add:

```ts
  it("keeps the working list from the artifact and from working events, without re-resolving anchors", async () => {
    const rec = { key: "k", harness: "codex", message: "Chart", thread_ids: [], started_at: "s", last_heartbeat: "s" };
    (loaded.artifact as Record<string, unknown>).working = [rec];
    try {
      const { ctl } = await started();
      expect(ctl.state.get().working).toEqual([rec]);
      await vi.waitFor(() => expect(FakeES.last).not.toBeNull());
      const data = ctl.state.get().data;
      FakeES.last!.emit("working", { type: "working", artifact_id: ID, working: [] });
      expect(ctl.state.get().working).toEqual([]);
      expect(ctl.state.get().data).toBe(data);
      ctl.dispose();
    } finally {
      delete (loaded.artifact as Record<string, unknown>).working;
    }
  });
```

Run: `cd web && npx vitest run shell/src/view/artifact-controller.test.ts shell/src/events.test.ts`
Expected: PASS.

- [ ] **Step 3: Components**

`web/shell/src/ui/WorkingStatus.svelte`:

```svelte
<script lang="ts">
  import { statusParts, type Working } from "../view/working-model";

  let { list }: { list: Working[] } = $props();
  const p = $derived(statusParts(list));
</script>

<!-- Always in the DOM, so a change is announced; its text changes only when
     the records change, so renewals are silent. -->
<p class={["working-status", p && "on"]} role="status" aria-live="polite" aria-atomic="true" title={p?.title}>
  {#if p}<span class="working-dot" aria-hidden="true"></span><span class="working-who">{p.who}</span><span class="working-detail">{p.detail}</span><span class="working-more">{p.more}</span>{/if}
</p>
```

`web/shell/src/ui/WorkingBadge.svelte`:

```svelte
<script lang="ts">
  import { badgeText, type Working } from "../view/working-model";

  let { list }: { list: Working[] } = $props();
  const text = $derived(badgeText(list));
</script>

{#if text}<span class="working-badge"><span class="working-dot" aria-hidden="true"></span>{text}</span>{/if}
```

`web/shell/src/ui/working-feed.svelte.ts`:

```ts
// The gallery's live working lists, by artifact: seeded from
// `GET /api/artifacts`, then kept current by the `working` event stream,
// which opens only after the gallery has rendered.
import type { Artifact } from "../api";
import { subscribeWorking } from "../events";
import type { Working } from "../view/working-model";

export class WorkingFeed {
  byId = $state<Record<string, Working[]>>({});
  #stop: (() => void) | null = null;

  seed(list: Artifact[]): void {
    this.byId = Object.fromEntries(list.map(a => [a.id, a.working ?? []]));
  }

  /** Opens the stream once; `onResync` refetches after a reconnect or a drop. */
  start(onResync: () => void): void {
    if (this.#stop) return;
    this.#stop = subscribeWorking(e => {
      if (e.type === "working") this.byId = { ...this.byId, [e.artifact_id]: e.working };
      else if (e.type === "ready" || e.type === "resync") onResync();
    });
  }

  stop(): void {
    this.#stop?.();
    this.#stop = null;
  }
}
```

`web/shell/src/working-status.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { mount } from "./test/svelte";
import WorkingStatus from "./ui/WorkingStatus.svelte";

describe("WorkingStatus", () => {
  it("is an empty polite live region while nobody works, and fills in place", () => {
    const m = mount(WorkingStatus, { list: [] });
    const p = m.root.querySelector("p.working-status")!;
    expect(p.getAttribute("role")).toBe("status");
    expect(p.getAttribute("aria-live")).toBe("polite");
    expect(p.textContent).toBe("");
    m.update({ list: [{ key: "k", harness: "pi", message: "Tidying", thread_ids: [], started_at: "s", last_heartbeat: "s" }] });
    expect(m.root.querySelector("p.working-status")).toBe(p);
    expect(p.textContent).toBe("Pi is working: Tidying");
    expect(p.querySelector(".working-dot")!.getAttribute("aria-hidden")).toBe("true");
    m.unmount();
  });
});
```

- [ ] **Step 4: Wire the islands, the sidebar and the gallery**

`ui/TopbarIsland.svelte`: import `WorkingStatus`, and render `<WorkingStatus list={s.working} />` as the first child inside `{#if !s.error && s.data}`. The island mounts right after the skeleton's `<h1>`, so the line sits between the title and the controls.

`ui/SidebarIsland.svelte`: pass `working={s.working}` to `Sidebar`.

`ui/Sidebar.svelte`: add `working?: Working[]` to `Props`, and pass `marker={t.status === "open" ? threadMarker(p.working ?? [], t.id) : null}` to each `ThreadCard`.

`ui/ThreadCard.svelte`: add `marker?: string | null` to `Props`. Replace `{#if label}<p class="waiting">{label}</p>{/if}` with:

```svelte
  {#if marker}
    <p class="working-marker"><span class="working-dot" aria-hidden="true"></span>{marker}</p>
  {:else if label}
    <p class="waiting">{label}</p>
  {/if}
```

`sidebar.test.ts`, add (the file imports `mount` from `./test/svelte` and `Sidebar` from `./ui/Sidebar.svelte`):

```ts
  it("shows the working marker in place of the waiting indicator, until the record drops the thread", () => {
    const t: Thread = { ...base, id: "w", anchor, status: "open", sent_to_agent: true, comments: [comment("1", "viewer", "Alex", "two columns")],
      feedback_state: { thread_id: "w", state: "delivered", tier: "stop_hook", since: base.created_at, resends: 0, exhausted: false } };
    const rec = { key: "k", harness: "codex", message: null, thread_ids: ["w"], started_at: base.created_at, last_heartbeat: base.created_at };
    const props = { threads: [t], resolved: {}, working: [rec], now: new Date(base.created_at), selected: null,
      onSelect: vi.fn(), onSend: vi.fn(), onResolve: vi.fn(), onReply: vi.fn() };
    const m = mount(Sidebar, props);
    expect(m.root.querySelector(".working-marker")!.textContent).toBe("Codex is working…");
    expect(m.root.querySelector(".waiting")).toBeNull();
    m.update({ ...props, working: [] });
    expect(m.root.querySelector(".working-marker")).toBeNull();
    expect(m.root.querySelector(".waiting")!.textContent).toContain("delivered via the Stop hook");
    m.unmount();
  });
```

`ui/Gallery.svelte`:
- `import { onDestroy } from "svelte";`, `import WorkingBadge from "./WorkingBadge.svelte";`, `import { WorkingFeed } from "./working-feed.svelte";`, and `const feed = new WorkingFeed(); onDestroy(() => feed.stop());`.
- `refresh` becomes `() => listArtifacts().then(a => { error = null; artifacts = a; feed.seed(a); feed.start(refresh); }, e => { error = describe(e); })`. The stream opens after the first list has rendered, never before.
- In the card's `.meta`, after the publisher span, add `<WorkingBadge list={feed.byId[a.id] ?? []} />`.

`gallery.test.ts`: add `working: [{ key: "k", harness: "claude", message: null, thread_ids: [], started_at: "2026-09-28T11:00:00Z", last_heartbeat: "2026-09-28T11:00:00Z" }]` to the first `ARTIFACTS` entry. Add a test in the file's style that mounts the gallery, waits for the cards, and asserts that the first card's `.working-badge` reads `Claude Code working` and the second card has none. jsdom has no `EventSource`, so `subscribeWorking` is a no-op there.

- [ ] **Step 5: Styles (tokens for both themes, phone width, reduced motion)**

Append to `web/shell/src/theme.css` (the port inlines it into both entries at build time):

```css
/* Working signal (spec §8). */
:root { --working: #0e7490; --working-soft: #e0f2f7; }
:root:not([data-theme="light"]) { @media (prefers-color-scheme: dark) { --working: #22d3ee; --working-soft: #0b3440; } }
:root[data-theme="dark"] { --working: #22d3ee; --working-soft: #0b3440; }
.working-dot { display: inline-block; width: 8px; height: 8px; border-radius: 50%; background: var(--working); flex: none; animation: working-pulse 1.6s ease-in-out infinite; }
@keyframes working-pulse { 0%, 100% { opacity: 1; transform: scale(1); } 50% { opacity: .35; transform: scale(.8); } }
@media (prefers-reduced-motion: reduce) { .working-dot { animation: none; } }
.working-status { display: none; margin: 0; min-width: 0; flex: 0 1 auto; max-width: 45%; align-items: center; gap: 6px; font-size: 13px; color: var(--fg); white-space: nowrap; overflow: hidden; }
.working-status.on { display: flex; }
.working-status .working-detail { overflow: hidden; text-overflow: ellipsis; min-width: 0; color: var(--muted); }
.working-status .working-more { color: var(--muted); flex: none; }
.working-badge { display: inline-flex; align-items: center; gap: 5px; padding: 1px 8px; border-radius: 999px; background: var(--working-soft); color: var(--fg); }
.card .meta { flex-wrap: wrap; }
.working-marker { display: flex; align-items: center; gap: 6px; font-size: 12px; color: var(--working); margin: 6px 0; }
@media (max-width: 480px) {
  .working-status { max-width: 50%; }
  .working-status .working-detail, .working-status .working-more { display: none; }
}
```

At 480 px and below, the port's top bar wraps the title onto its own row. The status line then sits on the controls row, showing only the dot and `<Harness> is working`. Check this in Task 11's phone test, and do not change the port's `.topbar h1` rules.

- [ ] **Step 6: Run and commit**

Run: `cd web && npm run lint && npm run typecheck && npx vitest run && npm run build && node scripts/bundle-size.mjs; echo "exit=$?"`
Expected: PASS and `exit=0`. If the bundle-size gate fails, stop and report the sizes. Do not record a new budget.

Run: `grep -rlE "from \"svelte" web/shell/src/view web/shell/src/caps`
Expected: no output.

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add web/shell/src/view/working-model.ts web/shell/src/view/working-model.test.ts web/shell/src/ui/WorkingStatus.svelte web/shell/src/ui/WorkingBadge.svelte \
  web/shell/src/ui/working-feed.svelte.ts web/shell/src/working-status.test.ts web/shell/src/api.ts web/shell/src/events.ts web/shell/src/events.test.ts \
  web/shell/src/view/artifact-controller.ts web/shell/src/view/artifact-controller.test.ts web/shell/src/ui/TopbarIsland.svelte web/shell/src/ui/SidebarIsland.svelte \
  web/shell/src/ui/Sidebar.svelte web/shell/src/ui/ThreadCard.svelte web/shell/src/sidebar.test.ts web/shell/src/ui/Gallery.svelte web/shell/src/gallery.test.ts web/shell/src/theme.css
git commit -m "Show who is working in the header, on gallery cards and on thread cards"
git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed
```

---

### Task 10: The page capability: `working()` and `onWorking(fn)`

**Files:**
- Modify: `web/contract/0.2.61/comments.d.ts`, `web/shell/src/caps/host.ts` (`CapEnv.working`), `web/shell/src/caps/comments.ts`, `web/shell/src/caps/comments.test.ts`, `web/bridge/src/caps/comments.ts`, `web/bridge/test/comments.test.ts`, `web/shell/src/view/artifact-controller.ts` (passes `working` into the host env)

**Interfaces:**
- Produces (contract, in `namespace comments`):

```ts
    /** Clax extension: not part of claude.ai's comments capability. One
     * agent session working on this artifact now. */
    interface WorkingAgent {
      /** `"claude"`, `"codex"`, `"pi"`, or another harness name. */
      harness: string;
      /** The harness as people read it ("Claude Code"). */
      label: string;
      /** The agent's own words, at most 140 characters; treat as untrusted text. */
      message: string | null;
      /** When it started (ISO 8601). */
      since: string;
      /** Handles of threads THIS document created that the agent is acting on. */
      threads: string[];
      /** How many other threads it is acting on. */
      otherThreads: number;
    }
    /** Clax extension: not part of claude.ai's comments capability. */
    interface WorkingState {
      working: boolean;
      /** Newest first. */
      agents: WorkingAgent[];
    }
```

and in `interface Comments`:

```ts
    /**
     * Clax extension, not part of claude.ai's comments capability: which
     * agents are working on this artifact now. Read-only. Clax grants it
     * under either declaration form, including `composer_only` (a Clax
     * extension to that form, which otherwise grants only openComposer and
     * anchorFor), with no consent prompt and no gesture. Never names a
     * session or a thread's store ID.
     */
    working(): Promise<WorkingState>;
    /**
     * Clax extension, not part of claude.ai's comments capability: calls `fn`
     * with the current state, then on every change. Resolves a function
     * that stops the calls.
     */
    onWorking(fn: (state: WorkingState) => void): Promise<() => void>;
```

- Shell handler methods: `working` → `WorkingState`; `watchWorking` → `null` (starts pushes on topic `working`); `unwatchWorking` → `null`.
- `CapEnv` gains `working(): Working[]`.
- Pure helper in `caps/comments.ts`: `pageWorking(list: Working[], handleOf: (id: string) => string | undefined): WorkingState`.

- [ ] **Step 1: Failing shell handler tests**

In `web/shell/src/caps/comments.test.ts`, give the harness a working list. Add at module level `let workingList: Working[] = [];` (with `import type { Working } from "../view/working-model";`), reset it in the `beforeEach` (`workingList = [];`), and add `working: () => workingList` to the `env` object literal in `setup`. Then add to `describe("comments in the shell", ...)`:

```ts
  const rec = (over: Partial<Working> = {}): Working => ({ key: "k", harness: "codex", message: "Chart", thread_ids: [], started_at: "2026-09-30T10:00:00.000Z", last_heartbeat: "2026-09-30T10:00:00.000Z", ...over });

  it("working() names only this document's own threads, by handle", async () => {
    const { h, posted } = setup({ comments: {} });
    daemon(T("01J9C"));
    const created = await h.call("create", [{ anchor: T("x").anchor, text: "hi", version: 1 }]) as { threadId: string };
    workingList = [rec({ thread_ids: ["01J9C", "01J9Z"] })];
    const s = await h.call("working", []);
    expect(s).toEqual({ working: true, agents: [{ harness: "codex", label: "Codex", message: "Chart", since: "2026-09-30T10:00:00.000Z", threads: [created.threadId], otherThreads: 1 }] });
    expect(JSON.stringify(s)).not.toContain("01J9C");
    expect(JSON.stringify(s)).not.toContain("\"k\"");
    expect(posted.filter(m => m.type === "clax:event" && m.topic === "working")).toEqual([]);
  });

  it("pushes working state to a watching page on every working event, until it stops watching", async () => {
    const { h, posted } = setup({ comments: {} });
    const pushes = () => posted.filter(m => m.type === "clax:event" && m.topic === "working") as unknown as { data: { working: boolean } }[];
    await h.call("watchWorking", []);
    expect(pushes().at(-1)!.data).toEqual({ working: false, agents: [] });
    workingList = [rec({ harness: "pi", message: null })];
    h.onEvent!({ type: "working", artifact_id: "7q3k9mzx2b4t", working: workingList });
    expect(pushes().at(-1)!.data.working).toBe(true);
    await h.call("unwatchWorking", []);
    const n = pushes().length;
    h.onEvent!({ type: "working", artifact_id: "7q3k9mzx2b4t", working: [] });
    expect(pushes().length).toBe(n);
  });

  it("answers working() under the composer-only form, without consent or gesture", async () => {
    const { h, prompt } = setup({ comments: { composer_only: true } });
    gesture(false);
    expect(await h.call("working", [])).toEqual({ working: false, agents: [] });
    expect(prompt).not.toHaveBeenCalled();
  });
```

Run: `cd web && npx vitest run shell/src/caps/comments.test.ts`
Expected: FAIL (`comments.working is not part of this runtime`).

- [ ] **Step 2: Implement the shell side**

In `caps/host.ts`, add to `CapEnv`: `/** Who is working on the artifact now (the view's latest working list). */ working(): Working[];`. In `view/artifact-controller.ts` `viewChanged()`, add `working: () => this.s.working,` to the env object passed to `new CapabilityHost(...)`. It reads the controller's current state at each call. `onEvent` already forwards every event to `this.host`, so `working` events reach the handler.

In `caps/comments.ts`:

```ts
import { harnessLabel, newestFirst, type Working } from "../view/working-model";

/** The Clax `working()` state for the page: agents newest first; threads as
 * the handles this document holds, the rest counted. No record key, no
 * session, no store ID. */
export function pageWorking(list: Working[], handleOf: (id: string) => string | undefined) {
  const agents = newestFirst(list).map(w => {
    const threads = w.thread_ids.map(handleOf).filter((h): h is string => h !== undefined);
    return { harness: w.harness, label: harnessLabel(w.harness), message: w.message, since: w.started_at, threads, otherThreads: w.thread_ids.length - threads.length };
  });
  return { working: agents.length > 0, agents };
}
```

In `commentsHandler`, add `let watchingWorking = false;`, a `const ownHandle = (id: string) => [...created].find(([, v]) => v === id)?.[0];`, and a `const pushWorking = () => { if (watchingWorking && !disposed) env.post(event("working", pageWorking(env.working(), ownHandle))); };`. Add cases before `default`. These run before any consent or gesture check in the handler, and are allowed under `composer_only`:

```ts
        case "working":
          return pageWorking(env.working(), ownHandle);
        case "watchWorking":
          watchingWorking = true;
          pushWorking();
          return null;
        case "unwatchWorking":
          watchingWorking = false;
          return null;
```

In `onEvent`, add `if (e.type === "working") pushWorking();`. In `reset` and `dispose`, set `watchingWorking = false`.

If the handler's method gate (the place that rejects write verbs under `composer_only` with `not_granted`) runs before the `switch`, add `working`, `watchWorking` and `unwatchWorking` to its allowed list for the composer-only form.

- [ ] **Step 3: The bridge side**

In `web/bridge/src/caps/comments.ts` (after the port's Task 13 it is part of the lazy `caps` part, and `commentsLocals(rpc, config, env)` takes a `CapsEnv`), inside `commentsLocals`, add:

```ts
  const workingFns = new Set<(s: unknown) => void>();
  let offWorking: (() => void) | null = null;
  const working = () => rpc.call("comments", "working", []);
  const onWorking = async (fn: unknown) => {
    if (typeof fn !== "function") throw invalid("onWorking takes a function");
    const f = fn as (s: unknown) => void;
    workingFns.add(f);
    if (!offWorking) {
      offWorking = rpc.on("comments", "working", d => { for (const g of workingFns) { try { g(d); } catch { /* the page's own error */ } } });
      await rpc.call("comments", "watchWorking", []);
    } else {
      try { f(await working()); } catch { /* the page's own error */ }
    }
    return () => {
      workingFns.delete(f);
      if (workingFns.size === 0 && offWorking) {
        offWorking();
        offWorking = null;
        void rpc.call("comments", "unwatchWorking", []).catch(() => {});
      }
    };
  };
```

and add `working` and `onWorking` to the namespace object this function returns, under both declaration forms. Add to `web/bridge/test/comments.test.ts`, with its `fakeRpc`:

```ts
  it("onWorking shares one shell subscription and ends it with the last subscriber", async () => {
    const f = fakeRpc(() => ({ working: false, agents: [] }));
    const c = commentsLocals(f.rpc as never, {}, { ctx: commentsContext, clip: () => import("../src/parts/clip") }) as unknown as { onWorking(fn: (s: unknown) => void): Promise<() => void> };
    const a: unknown[] = [];
    const b: unknown[] = [];
    const offA = await c.onWorking(s => a.push(s));
    const offB = await c.onWorking(s => b.push(s));
    const methods = () => f.rpc.call.mock.calls.map(x => x[1]);
    expect(methods().filter(m => m === "watchWorking")).toHaveLength(1);
    f.emit("working", { working: true, agents: [] });
    expect(a.at(-1)).toEqual({ working: true, agents: [] });
    expect(b.at(-1)).toEqual({ working: true, agents: [] });
    offA();
    expect(methods()).not.toContain("unwatchWorking");
    offB();
    expect(methods()).toContain("unwatchWorking");
  });
```

- [ ] **Step 4: Run and commit**

Run: `cd web && npm run lint && npm run typecheck && npx vitest run`
Expected: PASS.

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add web/contract/0.2.61/comments.d.ts web/shell/src/caps/host.ts web/shell/src/caps/comments.ts web/shell/src/caps/comments.test.ts \
  web/bridge/src/caps/comments.ts web/shell/src/view/artifact-controller.ts
git add web/bridge/test/comments.test.ts
git commit -m "Let pages read who is working through the comments capability (Clax extension)"
git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed
```

---

### Task 11: Browser tests and browser verification for the working signal

**Files:**
- Create: `web/e2e/working.spec.ts`, `web/e2e/pages/working-cap.html`
- Modify: `web/e2e/fixtures.ts` (`setWorking`, `skewWorking`, `postThread` helpers)

**Interfaces:**
- Produces (fixtures): `setWorking(base, token, sid, aid, body): Promise<any>`, `skewWorking(base, token, secs): Promise<void>` (the debug build's `POST /api/_test/working/skew`), `postThread(base, aid, body): Promise<{ id: string }>`.

- [ ] **Step 1: Fixtures**

Append to `web/e2e/fixtures.ts`:

```ts
/** Marks `sid` working on `aid` (`PUT /api/sessions/<sid>/working/<aid>`). */
export async function setWorking(base: string, token: string, sid: string, aid: string, body: { thread_ids?: string[]; message?: string }) {
  return api(base, token, `/api/sessions/${sid}/working/${aid}`, { method: "PUT", body: JSON.stringify(body) });
}

/** Creates a viewer thread on `aid` v1 anchored to `body > main > h2`, as the shell posts it (multipart); returns the thread. */
export async function postThread(base: string, aid: string, body: string) {
  const form = new FormData();
  form.set("anchor", JSON.stringify({ kind: "element", selector: "body > main > h2", quote: "Quarterly goals", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null }));
  form.set("body", body);
  form.set("version", "1");
  const res = await fetch(`${base}/api/artifacts/${aid}/threads`, { method: "POST", body: form });
  if (!res.ok) throw new Error(`${res.status} ${await res.text()}`);
  return (await res.json()).thread as { id: string };
}

/** Moves the daemon's working clock forward and sweeps (debug builds only). */
export async function skewWorking(base: string, token: string, secs: number) {
  await api(base, token, "/api/_test/working/skew", { method: "POST", body: JSON.stringify({ secs }) });
}
```

`web/e2e/pages/working-cap.html`:

```html
<!doctype html><title>Working cap</title>
<main><h2 id="h">Goals</h2><p id="state">none</p><button id="add">Add</button></main>
<script>
  (async () => {
    const c = await window.claude.use("comments");
    const out = document.getElementById("state");
    const show = s => { out.textContent = s.working ? s.agents.map(a => `${a.label}|${a.message}|${a.threads.length}|${a.otherThreads}`).join(",") : "none"; };
    show(await c.working());
    await c.onWorking(show);
    document.getElementById("add").onclick = async () => {
      const r = await c.create({ anchor: { path: "#h" }, text: "from the page" });
      document.body.dataset.handle = r.threadId;
    };
  })();
</script>
```

- [ ] **Step 2: The spec**

`web/e2e/working.spec.ts`:

```ts
import { test, expect } from "@playwright/test";
import { readFileSync } from "node:fs";
import { api, openArtifact, postThread, publishAs, publishWith, reach, registerSession, setWorking, skewWorking, startDaemon } from "./fixtures";

let d: Awaited<ReturnType<typeof startDaemon>>;
test.beforeAll(async () => { test.setTimeout(180_000); d = await startDaemon(); });
test.afterAll(async () => { await d?.stop(); });

const PAGE = "<main><h2>Quarterly goals</h2></main>";

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: the header, the marker and the gallery badge follow the working record`, async ({ page }) => {
    const s = await registerSession(d.base, d.token, "claude", `work-${mode}`);
    const { artifact } = await publishAs(d.base, d.token, s.id, `Working ${mode}`, { "index.html": PAGE });
    const t = { thread: await postThread(d.base, artifact.id, "@agent two columns") };
    await openArtifact(page, d.base, artifact.id, 1, mode);
    const status = page.locator("p.working-status");
    await expect(status).toHaveAttribute("aria-live", "polite");
    await expect(status).toBeHidden();
    await page.getByRole("button", { name: /Threads/ }).click();
    await api(d.base, d.token, `/api/sessions/${s.id}/feedback?tier=piggyback`);
    await expect(status).toHaveText("Claude Code is working on 1 comment");
    const card = page.locator(`.thread-card[data-thread="${t.thread.id}"]`);
    await expect(card.locator(".working-marker")).toHaveText("Claude Code is working…");
    await setWorking(d.base, d.token, s.id, artifact.id, { message: "Two columns" });
    await expect(status).toHaveText("Claude Code is working: Two columns");
    const other = await registerSession(d.base, d.token, "codex", `work-other-${mode}`);
    await setWorking(d.base, d.token, other.id, artifact.id, { message: "Chart" });
    await expect(status).toHaveText("Codex is working: Chart (+1 more)");
    await expect(status).toHaveAttribute("title", "Codex is working: Chart\nClaude Code is working: Two columns");
    await api(d.base, d.token, `/api/artifacts/${artifact.id}/threads/${t.thread.id}/comments`, { method: "POST", session: s.id,
      body: JSON.stringify({ body: "Done: two columns.", author_kind: "agent" }) });
    await expect(card.locator(".working-marker")).toHaveCount(0);
    await expect(card.locator(".comment.agent .body")).toHaveText("Done: two columns.");
    const gallery = await page.context().newPage();
    await gallery.goto(`${d.base}/`);
    const badge = gallery.locator(".card-wrap", { hasText: `Working ${mode}` }).locator(".working-badge");
    await expect(badge).toHaveText("Codex working");
    await api(d.base, d.token, `/api/sessions/${other.id}/working/end`, { method: "POST", body: "{}" });
    await expect(badge).toHaveCount(0, { timeout: 10_000 });
    await expect(status).toBeHidden();
  });

  test(`${mode}: a record lapses 2 minutes after its last renewal`, async ({ page }) => {
    const s = await registerSession(d.base, d.token, "pi", `lapse-${mode}`);
    const { artifact } = await publishAs(d.base, d.token, s.id, `Lapse ${mode}`, { "index.html": PAGE });
    await setWorking(d.base, d.token, s.id, artifact.id, { message: "Tidying" });
    await openArtifact(page, d.base, artifact.id, 1, mode);
    await expect(page.locator("p.working-status")).toHaveText("Pi is working: Tidying");
    await skewWorking(d.base, d.token, 121);
    await expect(page.locator("p.working-status")).toBeHidden();
  });

  test(`${mode}: the page reads working state through the comments capability`, async ({ page }) => {
    const html = readFileSync(new URL("./pages/working-cap.html", import.meta.url), "utf8");
    const s = await registerSession(d.base, d.token, "codex", `cap-${mode}`);
    const { artifact } = await publishWith(d.base, d.token, `Cap ${mode}`, html, { comments: {} });
    const frame = await openArtifact(page, d.base, artifact.id, 1, mode);
    await expect(frame.locator("#state")).toHaveText("none");
    await page.getByLabel("Your name").fill("Alex");
    await page.getByLabel("Your name").press("Enter");
    await reach(page, frame.locator("#add"));
    await frame.locator("#add").click();
    await page.getByRole("dialog").getByRole("button", { name: "Allow", exact: true }).click();
    await expect.poll(() => frame.locator("body").getAttribute("data-handle")).toBeTruthy();
    const threads = await api(d.base, d.token, `/api/artifacts/${artifact.id}/threads`);
    const tid = threads.threads[0].id as string;
    await setWorking(d.base, d.token, s.id, artifact.id, { message: "Chart", thread_ids: [tid] });
    await expect(frame.locator("#state")).toHaveText("Codex|Chart|1|0");
    const handle = await frame.locator("body").getAttribute("data-handle");
    expect(handle).not.toBe(tid);
    await api(d.base, d.token, `/api/sessions/${s.id}/working/${artifact.id}`, { method: "DELETE" });
    await expect(frame.locator("#state")).toHaveText("none");
  });
}

test("the status line fits a phone in dark mode and holds still under reduced motion", async ({ page }) => {
  const s = await registerSession(d.base, d.token, "claude", "phone-work");
  const { artifact } = await publishAs(d.base, d.token, s.id, "A long title for a phone-width working status check", { "index.html": PAGE });
  await setWorking(d.base, d.token, s.id, artifact.id, { message: "Rebuilding the quarterly chart with the new numbers" });
  await page.setViewportSize({ width: 375, height: 812 });
  await page.emulateMedia({ colorScheme: "dark", reducedMotion: "reduce" });
  await openArtifact(page, d.base, artifact.id, 1, "subdomain");
  const status = page.locator("p.working-status");
  await expect(status.locator(".working-who")).toHaveText("Claude Code is working");
  await expect(status.locator(".working-detail")).toBeHidden();
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(375);
  expect(await status.locator(".working-dot").evaluate(e => getComputedStyle(e).animationName)).toBe("none");
  await expect(page.locator(".topbar h1")).toBeVisible();
});
```

Run: `cd web && npx playwright test e2e/working.spec.ts`
Expected: PASS (7 tests).

- [ ] **Step 3: Browser verification (required)**

Start a scratch daemon and look at the UI, both themes and phone width. This is not the real home, and not port 7480:

```bash
export CLAX_HOME="$(mktemp -d)/home"
cargo run -q -p clax-cli -- --port 0 serve
python3 - <<'PY'
import json, os, urllib.request
info = json.load(open(os.path.join(os.environ["CLAX_HOME"], "daemon.json")))
base, tok = f"http://localhost:{info['port']}", info["token"]
def call(method, path, body=None, session=None):
    req = urllib.request.Request(base + path, data=None if body is None else json.dumps(body).encode(), method=method)
    req.add_header("authorization", f"Bearer {tok}"); req.add_header("content-type", "application/json")
    if session: req.add_header("x-clax-session", session)
    return json.load(urllib.request.urlopen(req))
s = call("POST", "/api/sessions", {"harness": "claude", "harness_session_id": "verify", "cwd": "/tmp"})["session"]["id"]
a = call("POST", "/api/artifacts", {"title": "Verify working", "files": {"index.html": {"content": "<h2>Goals</h2>", "encoding": "utf8"}}}, s)["artifact"]["id"]
call("PUT", f"/api/sessions/{s}/working/{a}", {"message": "Rebuilding the chart"})
print(f"{base}/a/{a}  and  {base}/")
PY
```

Open both printed URLs in a browser. Check each of these, and write down what you saw in the task report:
- The header shows the pulsing dot and `Claude Code is working: Rebuilding the chart`. It is styled like the rest of the top bar, in light and in dark mode.
- The gallery card shows the badge.
- At 375 px width (device toolbar) only the dot and `Claude Code is working` show, and nothing scrolls sideways.
- With reduced motion emulated, the dot does not pulse.

Then stop the daemon: `cargo run -q -p clax-cli -- stop` (same `CLAX_HOME`).

- [ ] **Step 4: Gates and commit**

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add web/e2e/working.spec.ts web/e2e/pages/working-cap.html web/e2e/fixtures.ts
git commit -m "Test the working signal in the browser in both frame modes"
git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed
```

---

### Task 12: Changelog storage: notes, links and seen marks

**Files:**
- Create: `crates/clax-core/src/changelog.rs`, `crates/clax-core/src/store/changelog.rs`
- Modify: `crates/clax-core/src/lib.rs`, `crates/clax-core/src/store/mod.rs`, `crates/clax-core/src/store/migrations.rs`, `crates/clax-core/src/model.rs` (`Version`), `crates/clax-core/src/publish.rs`, `crates/clax-core/src/store/artifacts.rs`, `crates/clax-core/src/store/threads.rs` (`delete_thread_touched`), `crates/clax-core/src/working.rs` (`clean_line`)

**Interfaces:**
- Produces: `clax_core::working::clean_line(raw: &str, max: usize) -> (Option<String>, bool)`; `clean_message(raw)` becomes `clean_line(raw, MAX_MESSAGE_CHARS)`.
- Produces: `clax_core::changelog::{MAX_NOTE_CHARS: usize = 280, MAX_ADDRESSES: usize = 50, MAX_SEEN_PER_VIEWER: usize = 200, LinkSource { Working, Explicit, Resolve } (as_str: "working" | "explicit" | "resolve")}`.
- Changes: `PublishRequest` gains `note: Option<String>` and `addresses: Option<Vec<String>>`. `ValidatedPublish` gains `note: Option<String>`, `note_truncated: bool`, `addresses: Vec<String>`, and `working_threads: Vec<String>` (empty from `validate`; the publish route fills it). `Version` gains `note: Option<String>` and `addresses: Vec<String>` (`#[serde(default)]`).
- Produces (`Store`): `addressed_in(thread_id: &str) -> Result<Vec<u32>>`, `link_on_resolve(thread_id: &str) -> Result<Option<u32>>`, `seen(viewer_id: &str, aid: &ArtifactId) -> Result<Option<u32>>`, `mark_seen(viewer_id: &str, aid: &ArtifactId, n: u32) -> Result<u32>`.
- `publish_version` and `create_artifact` store the note and the links in the version's transaction. An explicit address that is not a thread of the artifact fails the publish with `unknown_thread`. A `working_threads` entry that no longer exists is skipped.

- [ ] **Step 1: Failing tests**

`crates/clax-core/src/store/changelog.rs`, test module first:

```rust
#[cfg(test)]
mod tests {
    use crate::publish::{PublishRequest, validate};
    use crate::store::test_util::{anchor, artifact, store};
    use crate::{ArtifactId, NewThread, Store};

    fn thread(st: &Store, id: &ArtifactId) -> String {
        st.create_thread(id, NewThread { version_n: 1, anchor: anchor(), body: "@agent x".into(), author_name: "Alex".into(), clip: None, via_page: false })
            .unwrap()
            .id
    }

    fn v2(st: &Store, id: &ArtifactId, note: Option<&str>, addresses: &[&str], working: &[String]) -> crate::Result<crate::model::Version> {
        let req: PublishRequest = serde_json::from_value(serde_json::json!({
            "if_version": 1, "note": note, "addresses": addresses,
            "files": {"index.html": {"content": "<main><h2>Quarterly goals</h2></main>", "encoding": "utf8"}}
        }))
        .unwrap();
        let mut p = validate(req)?;
        p.working_threads = working.to_vec();
        st.publish_version(id, p, None).map(|(_, v)| v)
    }

    #[test]
    fn a_publish_stores_its_note_and_links_explicit_and_working_threads_once() {
        let (_d, st) = store();
        let id = artifact(&st, None);
        let (t1, t2) = (thread(&st, &id), thread(&st, &id));
        let v = v2(&st, &id, Some("  Two\ncolumns  "), &[&t1], &[t1.clone(), t2.clone()]).unwrap();
        assert_eq!(v.note.as_deref(), Some("Two columns"));
        assert_eq!(v.addresses, [t1.clone(), t2.clone()]);
        assert_eq!(st.list_versions(&id).unwrap()[1].addresses, [t1.clone(), t2.clone()]);
        assert_eq!(st.addressed_in(&t1).unwrap(), [2]);
        assert_eq!(st.get_thread(&t1).unwrap().unwrap().status, "open", "linking never resolves");
    }

    #[test]
    fn an_unknown_address_fails_the_publish_and_a_vanished_working_thread_is_skipped() {
        let (_d, st) = store();
        let id = artifact(&st, None);
        let e = v2(&st, &id, None, &[&crate::new_ulid()], &[]).unwrap_err();
        assert!(matches!(e, crate::CoreError::Invalid { code, .. } if code == "unknown_thread"), "{e:?}");
        assert_eq!(st.get_artifact(&id).unwrap().unwrap().current_version, 1);
        let v = v2(&st, &id, None, &[], &[crate::new_ulid()]).unwrap();
        assert!(v.addresses.is_empty());
    }

    #[test]
    fn a_long_note_is_cut_and_flagged_and_too_many_addresses_are_refused() {
        let req: PublishRequest = serde_json::from_value(serde_json::json!({
            "note": "n".repeat(400), "files": {"index.html": {"content": "<p>", "encoding": "utf8"}}
        })).unwrap();
        let p = validate(req).unwrap();
        assert!(p.note_truncated);
        assert_eq!(p.note.unwrap().chars().count(), 280);
        let many: Vec<String> = (0..51).map(|_| crate::new_ulid()).collect();
        let req: PublishRequest = serde_json::from_value(serde_json::json!({
            "addresses": many, "files": {"index.html": {"content": "<p>", "encoding": "utf8"}}
        })).unwrap();
        assert!(validate(req).is_err());
    }

    #[test]
    fn an_agent_resolve_links_only_a_thread_with_no_link() {
        let (_d, st) = store();
        let id = artifact(&st, None);
        let (t1, t2) = (thread(&st, &id), thread(&st, &id));
        v2(&st, &id, None, &[&t1], &[]).unwrap();
        assert_eq!(st.link_on_resolve(&t1).unwrap(), None);
        assert_eq!(st.link_on_resolve(&t2).unwrap(), Some(2));
        assert_eq!(st.addressed_in(&t2).unwrap(), [2]);
    }

    #[test]
    fn deleting_a_thread_deletes_its_links() {
        let (_d, st) = store();
        let id = artifact(&st, None);
        let t1 = thread(&st, &id);
        v2(&st, &id, None, &[&t1], &[]).unwrap();
        st.delete_thread(&t1).unwrap();
        assert!(st.list_versions(&id).unwrap()[1].addresses.is_empty());
    }

    #[test]
    fn seen_marks_only_move_forward_and_are_bounded_per_viewer() {
        let (_d, st) = store();
        let viewer = st.upsert_viewer(&crate::new_ulid(), None).unwrap().id;
        let id = artifact(&st, None);
        assert_eq!(st.seen(&viewer, &id).unwrap(), None);
        assert_eq!(st.mark_seen(&viewer, &id, 3).unwrap(), 3);
        assert_eq!(st.mark_seen(&viewer, &id, 2).unwrap(), 3);
        let ids: Vec<ArtifactId> = (0..crate::changelog::MAX_SEEN_PER_VIEWER).map(|_| artifact(&st, None)).collect();
        for a in &ids {
            st.mark_seen(&viewer, a, 1).unwrap();
        }
        assert_eq!(st.seen(&viewer, &id).unwrap(), None, "the least recently updated row was pruned");
        assert_eq!(st.seen(&viewer, &ids[0]).unwrap(), Some(1));
        }
}
```

A viewer's ID is its cookie value; `upsert_viewer` creates the row the seen mark refers to.

Register the module: `pub mod changelog;` in `store/mod.rs`; `pub mod changelog;` in `lib.rs` for `crates/clax-core/src/changelog.rs`.

Run: `cargo test -p clax-core changelog`
Expected: FAIL to compile.

- [ ] **Step 2: Migration 10**

Append to `MIGRATIONS` in `store/migrations.rs`:

```rust
    // 10: the version changelog: each version's note, the threads it
    // addressed, and each viewer's last decided version per artifact. No
    // foreign key to `artifacts`: the doctor's hard deletes of broken artifact
    // rows must not trip on them; artifact deletion removes them explicitly.
    "ALTER TABLE versions ADD COLUMN note TEXT;
    CREATE TABLE version_threads (
        artifact_id TEXT NOT NULL,
        version_n INTEGER NOT NULL,
        thread_id TEXT NOT NULL REFERENCES threads(id),
        source TEXT NOT NULL CHECK (source IN ('working', 'explicit', 'resolve')),
        created_at TEXT NOT NULL,
        PRIMARY KEY (artifact_id, version_n, thread_id)
    );
    CREATE INDEX version_threads_by_thread ON version_threads(thread_id);
    CREATE TABLE viewer_seen (
        viewer_id TEXT NOT NULL REFERENCES viewers(id),
        artifact_id TEXT NOT NULL,
        seen_n INTEGER NOT NULL,
        updated_at TEXT NOT NULL,
        PRIMARY KEY (viewer_id, artifact_id)
    );
    CREATE INDEX viewer_seen_by_age ON viewer_seen(viewer_id, updated_at);",
```

- [ ] **Step 3: Implement**

`crates/clax-core/src/working.rs`: rename the body of `clean_message` into `pub fn clean_line(raw: &str, max: usize) -> (Option<String>, bool)`, with `MAX_MESSAGE_CHARS` replaced by `max`. Keep `pub fn clean_message(raw: &str) -> (Option<String>, bool) { clean_line(raw, MAX_MESSAGE_CHARS) }`.

`crates/clax-core/src/changelog.rs`:

```rust
//! The version changelog (spec §10 "Version changelog"): bounds and link sources.

/// Longest version note, in characters, after [`crate::working::clean_line`].
pub const MAX_NOTE_CHARS: usize = 280;
/// Most threads one publish may name in `addresses`.
pub const MAX_ADDRESSES: usize = 50;
/// Most artifacts a viewer's seen marks are kept for.
pub const MAX_SEEN_PER_VIEWER: usize = 200;

/// Why a thread is linked to a version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkSource {
    /// The publishing session was marked working on it.
    Working,
    /// Named in `addresses`.
    Explicit,
    /// Resolved by an agent with no earlier link.
    Resolve,
}

impl LinkSource {
    pub fn as_str(self) -> &'static str {
        match self {
            LinkSource::Working => "working",
            LinkSource::Explicit => "explicit",
            LinkSource::Resolve => "resolve",
        }
    }
}
```

`publish.rs`: add to `PublishRequest` `#[serde(default)] pub note: Option<String>,` and `#[serde(default)] pub addresses: Option<Vec<String>>,`. Add to `ValidatedPublish` `pub note: Option<String>, pub note_truncated: bool, pub addresses: Vec<String>, pub working_threads: Vec<String>,`. In `validate`, before the files loop:

```rust
    let (note, note_truncated) = match req.note.as_deref() {
        Some(n) => crate::working::clean_line(n, crate::changelog::MAX_NOTE_CHARS),
        None => (None, false),
    };
    let addresses = req.addresses.clone().unwrap_or_default();
    if addresses.len() > crate::changelog::MAX_ADDRESSES {
        return Err(CoreError::invalid("invalid_args", format!("at most {} addresses", crate::changelog::MAX_ADDRESSES)));
    }
    if let Some(bad) = addresses.iter().find(|t| !crate::is_ulid(t)) {
        return Err(CoreError::invalid("invalid_args", format!("'{bad}' is not a thread ID")));
    }
```

and fill the new fields in the returned struct (`working_threads: Vec::new()`).

`model.rs`, `Version`: add `pub note: Option<String>,` and `#[serde(default)] pub addresses: Vec<String>,`. In `store/artifacts.rs`, `row_to_version` reads `note: r.get("note")?` and sets `addresses: Vec::new()`. `SELECT_VERSION` gains `note`. The versions `INSERT` in both `create_artifact` and `publish_version` gains the `note` column (`p.note`). After the insert, in the same transaction, call the new `link_version(tx, id.as_str(), n, &p)?`. `get_version` and `list_versions` fill `addresses` with `fill_addresses(c, id, &mut versions)`.

`store/changelog.rs`, above the tests:

```rust
//! Version notes' links to threads and viewers' seen marks (spec §5).

use super::Store;
use crate::changelog::{LinkSource, MAX_SEEN_PER_VIEWER};
use crate::model::Version;
use crate::publish::ValidatedPublish;
use crate::{ArtifactId, CoreError, Result};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

fn insert_link(tx: &Transaction<'_>, aid: &str, n: u32, tid: &str, source: LinkSource) -> Result<()> {
    tx.execute(
        "INSERT OR IGNORE INTO version_threads (artifact_id, version_n, thread_id, source, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![aid, n, tid, source.as_str(), Store::now()],
    )?;
    Ok(())
}

fn thread_on(tx: &Transaction<'_>, aid: &str, tid: &str) -> Result<bool> {
    Ok(tx
        .query_row("SELECT 1 FROM threads WHERE id = ?1 AND artifact_id = ?2", params![tid, aid], |_| Ok(()))
        .optional()?
        .is_some())
}

/// Links version `n` to `p.addresses` (each must be a thread of `aid`, else
/// `unknown_thread`) and then to `p.working_threads` (skipping any that no
/// longer exist). Runs inside the version's transaction.
pub(crate) fn link_version(tx: &Transaction<'_>, aid: &str, n: u32, p: &ValidatedPublish) -> Result<()> {
    for tid in &p.addresses {
        if !thread_on(tx, aid, tid)? {
            return Err(CoreError::invalid("unknown_thread", format!("{tid} is not a thread of {aid}")));
        }
        insert_link(tx, aid, n, tid, LinkSource::Explicit)?;
    }
    for tid in &p.working_threads {
        if thread_on(tx, aid, tid)? {
            insert_link(tx, aid, n, tid, LinkSource::Working)?;
        }
    }
    Ok(())
}

/// Sets each version's `addresses`, in link order.
pub(crate) fn fill_addresses(c: &Connection, aid: &ArtifactId, versions: &mut [Version]) -> Result<()> {
    let mut stmt = c.prepare(
        "SELECT version_n, thread_id FROM version_threads WHERE artifact_id = ?1 ORDER BY created_at, rowid",
    )?;
    let rows = stmt.query_map(params![aid.as_str()], |r| Ok((r.get::<_, u32>(0)?, r.get::<_, String>(1)?)))?;
    for row in rows {
        let (n, tid) = row?;
        if let Some(v) = versions.iter_mut().find(|v| v.n == n) {
            v.addresses.push(tid);
        }
    }
    Ok(())
}

impl Store {
    /// The versions `thread_id` is linked to, ascending.
    pub fn addressed_in(&self, thread_id: &str) -> Result<Vec<u32>> {
        self.with_conn(|c| {
            let mut stmt = c.prepare("SELECT version_n FROM version_threads WHERE thread_id = ?1 ORDER BY version_n")?;
            Ok(stmt.query_map(params![thread_id], |r| r.get(0))?.collect::<rusqlite::Result<Vec<u32>>>()?)
        })
    }

    /// An agent resolved `thread_id`: links it to its artifact's current
    /// version when it has no link yet. The version linked, or `None`.
    pub fn link_on_resolve(&self, thread_id: &str) -> Result<Option<u32>> {
        self.with_tx(|tx| {
            let linked: bool = tx
                .query_row("SELECT 1 FROM version_threads WHERE thread_id = ?1 LIMIT 1", params![thread_id], |_| Ok(()))
                .optional()?
                .is_some();
            if linked {
                return Ok(None);
            }
            let (aid, n): (String, u32) = tx.query_row(
                "SELECT a.id, a.current_version FROM threads t JOIN artifacts a ON a.id = t.artifact_id WHERE t.id = ?1",
                params![thread_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            ).optional()?.ok_or(CoreError::NotFound)?;
            insert_link(tx, &aid, n, thread_id, LinkSource::Resolve)?;
            Ok(Some(n))
        })
    }

    /// The viewer's seen mark on `aid`.
    pub fn seen(&self, viewer_id: &str, aid: &ArtifactId) -> Result<Option<u32>> {
        self.with_conn(|c| {
            Ok(c.query_row(
                "SELECT seen_n FROM viewer_seen WHERE viewer_id = ?1 AND artifact_id = ?2",
                params![viewer_id, aid.as_str()],
                |r| r.get(0),
            ).optional()?)
        })
    }

    /// Raises the viewer's seen mark on `aid` to `n` (never lowers it) and
    /// prunes the viewer's rows past [`MAX_SEEN_PER_VIEWER`], least recently
    /// updated first. The mark after the write.
    pub fn mark_seen(&self, viewer_id: &str, aid: &ArtifactId, n: u32) -> Result<u32> {
        self.with_tx(|tx| {
            tx.execute(
                "INSERT INTO viewer_seen (viewer_id, artifact_id, seen_n, updated_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (viewer_id, artifact_id) DO UPDATE SET seen_n = MAX(seen_n, excluded.seen_n), updated_at = excluded.updated_at",
                params![viewer_id, aid.as_str(), n, Store::now()],
            )?;
            tx.execute(
                "DELETE FROM viewer_seen WHERE viewer_id = ?1 AND artifact_id NOT IN
                   (SELECT artifact_id FROM viewer_seen WHERE viewer_id = ?1 ORDER BY updated_at DESC, rowid DESC LIMIT ?2)",
                params![viewer_id, MAX_SEEN_PER_VIEWER as i64],
            )?;
            Ok(tx.query_row(
                "SELECT seen_n FROM viewer_seen WHERE viewer_id = ?1 AND artifact_id = ?2",
                params![viewer_id, aid.as_str()],
                |r| r.get(0),
            )?)
        })
    }
}
```

`Store::now()` has millisecond resolution, so rows written in the same millisecond tie on `updated_at`. The `rowid DESC` tiebreak keeps the most recently inserted, and an update of an existing row keeps its original `rowid`. In the bound test every row is inserted once, so insertion order decides.

`store/threads.rs::delete_thread_touched`: before `DELETE FROM feedback`, add `tx.execute("DELETE FROM version_threads WHERE thread_id = ?1", params![thread_id])?;`. `store/artifacts.rs::delete_artifact`: in its transaction add `DELETE FROM viewer_seen WHERE artifact_id = ?1` and `DELETE FROM version_threads WHERE artifact_id = ?1`.

Run: `cargo test -p clax-core`
Expected: PASS. Update existing tests whose expected `Version` JSON is exact by adding `"note": null, "addresses": []`, and change nothing else.

- [ ] **Step 4: Gates and commit**

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add crates/clax-core/src/changelog.rs crates/clax-core/src/store/changelog.rs crates/clax-core/src/lib.rs crates/clax-core/src/store/mod.rs \
  crates/clax-core/src/store/migrations.rs crates/clax-core/src/model.rs crates/clax-core/src/publish.rs crates/clax-core/src/store/artifacts.rs \
  crates/clax-core/src/store/threads.rs crates/clax-core/src/working.rs
git add -u crates/clax-core
git commit -m "Store version notes, the threads each version addressed, and viewers' seen marks"
git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed
```

---

### Task 13: Changelog routes: automatic links at publish and resolve, seen marks

**Files:**
- Modify: `crates/clax-server/src/routes/artifacts.rs` (`create`, `publish`), `crates/clax-server/src/routes/threads.rs` (`resolve`), `crates/clax-server/src/feedback.rs` (`thread_view`), `crates/clax-server/src/routes/viewers.rs`, `crates/clax-server/src/routes/mod.rs`
- Create: `crates/clax-server/tests/api_changelog.rs`

**Interfaces:**
- Consumes: Task 12's store API and `Working::threads_of`.
- Produces: publish responses carry `version.note`, `version.addresses` and a top-level `note_truncated`. Thread views carry `addressed_in`. A `thread` event goes out for each thread a publish or resolve linked.
- Produces: `GET /api/viewers/me/seen?artifact=<aid>` → `{seen: n | null}`, and `PUT /api/viewers/me/seen` `{artifact_id, version}` → `{seen}`. No token. `SameOrigin`. PUT without a cookie is 400 `no_viewer`. An unknown artifact is 404.

- [ ] **Step 1: Failing tests**

`crates/clax-server/tests/api_changelog.rs`:

```rust
mod common;
use common::TestServer;
use serde_json::{Value, json};

async fn setup(ts: &TestServer) -> (String, String, String) {
    let s = ts.register_session("claude", "cl1").await;
    let sid = s["id"].as_str().unwrap().to_string();
    let a = ts.publish_as(&sid, "T", "<main><h2>Quarterly goals</h2></main>").await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let t = ts.thread(&aid, 1, "@agent two columns").await;
    (sid, aid, t["id"].as_str().unwrap().to_string())
}

async fn publish(ts: &TestServer, sid: &str, aid: &str, extra: Value) -> reqwest::Response {
    let mut body = json!({"if_version": 1, "files": {"index.html": {"content": "<main><h2>Quarterly goals</h2></main>", "encoding": "utf8"}}});
    body.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    ts.authed(ts.client.post(format!("{}/api/artifacts/{aid}/versions", ts.base)))
        .header("x-clax-session", sid).json(&body).send().await.unwrap()
}

#[tokio::test]
async fn a_publish_links_the_threads_the_session_was_working_on_then_clears() {
    let ts = TestServer::spawn().await;
    let (sid, aid, tid) = setup(&ts).await;
    ts.get_authed(&format!("/api/sessions/{sid}/feedback?tier=piggyback")).await;
    let mut ev = ts.events(&format!("?artifact={aid}&types=thread")).await;
    let res = publish(&ts, &sid, &aid, json!({"note": "Two columns"})).await;
    assert_eq!(res.status(), 201);
    let v: Value = res.json().await.unwrap();
    assert_eq!(v["version"]["note"], "Two columns");
    assert_eq!(v["version"]["addresses"], json!([tid]));
    assert_eq!(v["note_truncated"], false);
    assert_eq!(ev.next_named("thread").await["thread"]["addressed_in"], json!([2]));
    let t: Value = ts.get(&format!("/api/artifacts/{aid}/threads/{tid}")).await.json().await.unwrap();
    assert_eq!(t["thread"]["status"], "open", "linking leaves the thread open");
    let w: Value = ts.get(&format!("/api/artifacts/{aid}/working")).await.json().await.unwrap();
    assert_eq!(w["working"], json!([]));
    let a: Value = ts.get(&format!("/api/artifacts/{aid}")).await.json().await.unwrap();
    assert_eq!(a["versions"][1]["addresses"], json!([tid]));
}

#[tokio::test]
async fn explicit_addresses_are_checked_before_anything_is_published() {
    let ts = TestServer::spawn().await;
    let (sid, aid, tid) = setup(&ts).await;
    let res = publish(&ts, &sid, &aid, json!({"addresses": [clax_core::new_ulid()]})).await;
    assert_eq!(res.status(), 400);
    assert_eq!(res.json::<Value>().await.unwrap()["error"]["code"], "unknown_thread");
    let a: Value = ts.get(&format!("/api/artifacts/{aid}")).await.json().await.unwrap();
    assert_eq!(a["artifact"]["current_version"], 1);
    let v: Value = publish(&ts, &sid, &aid, json!({"addresses": [tid]})).await.json().await.unwrap();
    assert_eq!(v["version"]["addresses"], json!([tid]));
}

#[tokio::test]
async fn an_agent_resolve_links_to_the_current_version_once() {
    let ts = TestServer::spawn().await;
    let (sid, aid, tid) = setup(&ts).await;
    ts.send_thread(&aid, &tid).await;
    let res = ts.authed(ts.client.post(format!("{}/api/artifacts/{aid}/threads/{tid}/resolve", ts.base)))
        .header("x-clax-session", &sid).json(&json!({"as": "agent"})).send().await.unwrap();
    let v: Value = res.json().await.unwrap();
    assert_eq!(v["thread"]["addressed_in"], json!([1]));
}

#[tokio::test]
async fn seen_marks_are_per_viewer_monotonic_and_refuse_foreign_origins() {
    let ts = TestServer::spawn().await;
    let (_sid, aid, _) = setup(&ts).await;
    let alex = ts.viewer(Some("Alex")).await;
    let other = ts.viewer(None).await;
    let get = |cookie: String| {
        let (ts, aid) = (&ts, aid.clone());
        async move {
            ts.client.get(format!("{}/api/viewers/me/seen?artifact={aid}", ts.base))
                .header("cookie", format!("clax_viewer={cookie}")).send().await.unwrap().json::<Value>().await.unwrap()
        }
    };
    let put = |cookie: Option<String>, n: u32, origin: Option<&'static str>| {
        let (ts, aid) = (&ts, aid.clone());
        async move {
            let mut r = ts.client.put(format!("{}/api/viewers/me/seen", ts.base)).json(&json!({"artifact_id": aid, "version": n}));
            if let Some(c) = cookie { r = r.header("cookie", format!("clax_viewer={c}")); }
            if let Some(o) = origin { r = r.header("origin", o); }
            r.send().await.unwrap()
        }
    };
    assert_eq!(get(alex.cookie.clone()).await, json!({"seen": null}));
    assert_eq!(put(Some(alex.cookie.clone()), 3, None).await.json::<Value>().await.unwrap(), json!({"seen": 3}));
    assert_eq!(put(Some(alex.cookie.clone()), 2, None).await.json::<Value>().await.unwrap(), json!({"seen": 3}));
    assert_eq!(get(other.cookie.clone()).await, json!({"seen": null}));
    assert_eq!(put(None, 1, None).await.status(), 400);
    assert_eq!(put(Some(alex.cookie.clone()), 4, Some("http://evil.example")).await.status(), 403);
}
```

Run: `cargo test -p clax-server --test api_changelog`
Expected: FAIL.

- [ ] **Step 2: Implement**

`routes/artifacts.rs::publish`: `validate` returns `p`, so make it `let mut p`. Inside the `store_call` closure, after `publishing_session`, add:

```rust
            if let Some(sid) = &session {
                p.working_threads = ctx.working.threads_of(sid, id.as_str());
            }
            let links_before: Vec<String> = p.addresses.iter().chain(&p.working_threads).cloned().collect();
            let truncated = p.note_truncated;
```

The working record must be read before the clear that Task 4 added after `ensure_watch`. After the publish succeeds and the version event is sent, publish a `thread` event for each linked thread that exists (`st.get_thread(tid)?` then `crate::routes::threads::publish_thread(&ctx, st, &t)?`; make `publish_thread` `pub(crate)`). Return `truncated` alongside and add `"note_truncated": truncated` to the response JSON. `create` does the same for `note_truncated`. `addresses` there can only fail, since no thread exists yet.

`feedback.rs::thread_view`: add `v["addressed_in"] = json!(st.addressed_in(&t.id)?);`.

`routes/threads.rs::resolve`, agent branch: after the resolve succeeds, `st.link_on_resolve(&tid)?;`, before the thread view is built and published.

`routes/viewers.rs`: add these (the file already imports `SameOrigin` and `ViewerCookie`, whose `.0` is the cookie, `Option<String>`):

```rust
#[derive(Deserialize)]
pub struct SeenQuery {
    artifact: String,
}

/// `GET /api/viewers/me/seen?artifact=<aid>`: `{seen}`, the highest version
/// whose changelog this viewer has had decided; null for none or no cookie.
pub async fn seen(
    State(s): State<AppState>,
    _o: SameOrigin,
    viewer: ViewerCookie,
    q: Result<Query<SeenQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let id = parse_id(&q.artifact)?;
    let n = s.store_call(move |st| {
        st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
        match viewer.0.as_deref() {
            Some(cookie) => match st.get_viewer(cookie)? {
                Some(v) => st.seen(&v.id, &id),
                None => Ok(None),
            },
            None => Ok(None),
        }
    }).await?;
    Ok(Json(json!({"seen": n})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeenBody {
    artifact_id: String,
    version: u32,
}

/// `PUT /api/viewers/me/seen`: raises the mark (never lowers it); `{seen}`.
/// 400 `no_viewer` without a viewer cookie.
pub async fn set_seen(
    State(s): State<AppState>,
    _o: SameOrigin,
    viewer: ViewerCookie,
    req: Result<Json<SeenBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let b = body(req)?;
    let id = parse_id(&b.artifact_id)?;
    let cookie = viewer.0.ok_or_else(|| ApiError::bad_request("no_viewer", "open /api/viewers/me first"))?;
    let n = s.store_call(move |st| {
        st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
        let v = st.upsert_viewer(&cookie, None)?;
        st.mark_seen(&v.id, &id, b.version)
    }).await?;
    Ok(Json(json!({"seen": n})))
}
```

`routes/mod.rs`: `.route("/api/viewers/me/seen", get(viewers::seen).put(viewers::set_seen))`, placed before the `/api/viewers/me` route.

Run: `cargo test -p clax-server`
Expected: PASS. Existing exact thread-view assertions gain `"addressed_in": []`, and publish-response assertions gain `note_truncated`.

- [ ] **Step 3: Gates and commit**

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add crates/clax-server/src/routes/artifacts.rs crates/clax-server/src/routes/threads.rs crates/clax-server/src/feedback.rs \
  crates/clax-server/src/routes/viewers.rs crates/clax-server/src/routes/mod.rs crates/clax-server/tests/api_changelog.rs
git add -u crates/clax-server/tests
git commit -m "Link versions to the threads they addressed, and keep each viewer's seen mark"
git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed
```

---

### Task 14: `note` and `addresses` on the publish tool, the CLI and Pi

**Files:**
- Modify: `plugins/pi/test/fixtures/contract.json` (`publish` and `comments_resolve` descriptions), `crates/clax-mcp/src/tools.rs`, `crates/clax-mcp/tests/tools.rs`, `crates/clax-mcp/tests/comments.rs`, `crates/clax-cli/src/commands/publish.rs`, `crates/clax-cli/tests/cli.rs`, `plugins/pi/src/clax.ts`, `plugins/pi/test/clax.test.ts`, `plugins/*/skills/clax/SKILL.md` (the `### publish` argument list in each, and the shared "Comment loop"), `docs/contract.md` (`### Tools` row for `comments_resolve`)

**Interfaces:**
- `PublishArgs` (Rust and Pi) gains `note: Option<String>` ("A short change note for the person, at most 280 characters.") and `addresses: Option<Vec<String>>` ("IDs of the comment threads this version addresses.").
- The publish result gains `note` (string or null), `note_truncated` (bool) and `addressed` (thread IDs linked to the new version).
- `clax publish` gains `--note <TEXT>` and `--addresses <ID,ID,…>` (`value_delimiter = ','`). Its `--json` output gains `note` and `addressed`.
- New descriptions, verbatim in the fixture, `tools.rs` and `clax.ts`:

```text
Publish an HTML page as a new artifact, or as a new version of an existing one (pass `id` or `url`, with `if_version`). Give the page as `html` or `file_path`, plus optional supporting `files`. Returns the artifact ID, its URL for the person, and the new version number. Add a short `note` (at most 280 characters) saying what changed, and list the comment threads this version addresses in `addresses`; threads you were marked working on are added for you. The person sees both as the version's changelog; nothing is resolved by it.
```

```text
Resolve a comment thread that was sent to you, once you have acted on it and replied. Threads not sent to the agent are left alone (`resolved: false` with `guidance`). A thread no version lists yet is listed as addressed in the artifact's current version.
```

- [ ] **Step 1: Failing tests**

Append to `crates/clax-mcp/tests/comments.rs`:

```rust
#[tokio::test]
async fn publish_carries_a_note_and_links_addressed_and_working_threads() {
    let ts = TestServer::spawn().await;
    let (tools, sid) = session_tools(&ts).await;
    let (v1, _) = blocks(&tools.publish(Parameters(PublishArgs { html: Some("<h2>Goals</h2>".into()), title: Some("T".into()), ..Default::default() })).await.unwrap());
    let aid = v1["artifact_id"].as_str().unwrap().to_string();
    let t1 = ts.thread(&aid, 1, "@agent a").await["id"].as_str().unwrap().to_string();
    let t2 = ts.thread(&aid, 1, "plain b").await["id"].as_str().unwrap().to_string();
    ts.get_authed(&format!("/api/sessions/{sid}/feedback?tier=piggyback")).await;
    let (v2, _) = blocks(&tools.publish(Parameters(PublishArgs {
        id: Some(aid.clone()), html: Some("<h2>Goals</h2><p>2</p>".into()),
        note: Some("Two columns".into()), addresses: Some(vec![t2.clone()]), ..Default::default()
    })).await.unwrap());
    assert_eq!(v2["note"], "Two columns");
    assert_eq!(v2["note_truncated"], false);
    assert_eq!(v2["addressed"], json!([t2, t1]));
    let t: Value = ts.get(&format!("/api/artifacts/{aid}/threads/{t1}")).await.json().await.unwrap();
    assert_eq!(t["thread"]["status"], "open");
}
```

Append to `crates/clax-cli/tests/cli.rs`:

```rust
#[test]
fn publish_takes_a_note_and_addresses() {
    let e = Env::new();
    let index = write(e.dir.path(), "n/index.html", "<title>Noted</title><main><h2>Goals</h2></main>");
    let v: serde_json::Value = serde_json::from_slice(
        &e.cmd().args(["publish", "--json", "--port", "0"]).arg(&index).assert().success().get_output().stdout,
    ).unwrap();
    let id = v["id"].as_str().unwrap().to_string();
    let bad = e.cmd().args(["publish", "--json", "--id", &id, "--addresses", "01J9ZZZZZZZZZZZZZZZZZZZZZZ"]).arg(&index).assert().failure();
    assert!(String::from_utf8_lossy(&bad.get_output().stderr).contains("unknown_thread"));
    let v2: serde_json::Value = serde_json::from_slice(
        &e.cmd().args(["publish", "--json", "--id", &id, "--note", "Tighter spacing"]).arg(&index).assert().success().get_output().stdout,
    ).unwrap();
    assert_eq!(v2["version"], 2);
    assert_eq!(v2["note"], "Tighter spacing");
    assert_eq!(v2["addressed"], serde_json::json!([]));
}
```

Append to `plugins/pi/test/clax.test.ts`, in `describe("comments", ...)`:

```ts
  it("clax_publish carries a note and addresses, as the MCP tool does", async () => {
    const { pi, ctx } = load(daemon.home, "pi-note");
    await pi.emit("session_start", { type: "session_start", reason: "startup" }, ctx);
    const v1 = JSON.parse((await pi.callTool("clax_publish", { html: "<h2>Goals</h2>", title: "N" }, ctx)).content[0].text!);
    const form = new FormData();
    form.set("anchor", JSON.stringify({ kind: "element", selector: "body > h2", quote: "Goals", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null }));
    form.set("body", "plain");
    form.set("version", "1");
    const tid = (await (await fetch(`${daemon.base}/api/artifacts/${v1.artifact_id}/threads`, { method: "POST", body: form })).json()).thread.id;
    const v2 = JSON.parse((await pi.callTool("clax_publish", { id: v1.artifact_id, html: "<h2>Goals</h2>", note: "Done", addresses: [tid] }, ctx)).content[0].text!);
    expect(v2).toMatchObject({ version: 2, note: "Done", note_truncated: false, addressed: [tid] });
  });
```

Run: `cargo test -p clax-mcp --test comments publish_carries && cargo test -p clax-cli --test cli publish_takes && (cd plugins/pi && npm test -- -t "carries a note")`
Expected: FAIL.

- [ ] **Step 2: Implement**

- Fixture: replace the `publish` and `comments_resolve` descriptions with the new strings above. `tools.rs` and `clax.ts`: replace the same two description literals.
- `tools.rs` `PublishArgs`: add the two fields with the doc comments above. In `do_publish`, after the `("label", a.label)` loop, add `if let Some(n) = a.note { body["note"] = json!(n); }` and `if let Some(t) = a.addresses { for tid in &t { check_thread_id(tid)?; } body["addresses"] = json!(t); }`. Add to the result object: `"note": res["version"]["note"], "note_truncated": res["note_truncated"], "addressed": res["version"]["addresses"],`. Update `crates/clax-mcp/tests/tools.rs` wherever it compares a whole publish result.
- `clax.ts` `PublishArgs`: add `note: opt(str("A short change note for the person, at most 280 characters.")),` and `addresses: opt(Type.Array(Type.String(), { description: "IDs of the comment threads this version addresses." })),`. In `publish`, extend the copy loop to `["description", "icon", "label", "note"]`, add `if (a.addresses) { for (const t of a.addresses) checkThreadId(t); body.addresses = a.addresses; }`, and add `note: res.version?.note ?? null, note_truncated: res.note_truncated ?? false, addressed: res.version?.addresses ?? []` to the result.
- `crates/clax-cli/src/commands/publish.rs` `Args`:

```rust
    /// A short change note shown to people as this version's changelog.
    #[arg(long)]
    pub note: Option<String>,
    /// IDs of comment threads this version addresses (comma-separated).
    #[arg(long, value_delimiter = ',')]
    pub addresses: Vec<String>,
```

Add `("note", &a.note)` to the body loop and `if !a.addresses.is_empty() { body["addresses"] = serde_json::json!(a.addresses); }`. Add `"note": res["version"]["note"], "addressed": res["version"]["addresses"]` to the printed JSON. For the text output, append `\naddresses N comment(s)` when `addressed` is non-empty.
- Each `SKILL.md` `### publish` argument list: after the `label` bullet, add

```markdown
- `note` (string, optional): a short change note for the person, at most 280
  characters ("Two columns; third bullet dropped"). Shown as this version's
  changelog.
- `addresses` (array of thread IDs, optional): the comment threads this
  version addresses. Threads you were marked working on are added for you.
  Listing a thread does not resolve it.
```

  In the shared `## Comment loop`, replace step 2's first sentence with ``2. Make the change, usually by publishing a new version of the same artifact (`publish` with its `id` or `url`), with a `note` saying what changed and `addresses` naming the threads it handles.`` Apply it to all three files identically.
- `docs/contract.md` `### Tools` table: the `comments_resolve` row's Result cell gains `; a thread no version lists yet is listed as addressed in the current version`.

Run: `python3 scripts/sync-skill-tools.py && bash scripts/test-plugins.sh | tail -1 && cargo test -p clax-mcp && cargo test -p clax-cli && (cd plugins/pi && npm run typecheck && npm test)`
Expected: `plugin checks passed`, then every suite PASS.

- [ ] **Step 3: Gates and commit**

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add plugins/pi/test/fixtures/contract.json crates/clax-mcp/src/tools.rs crates/clax-mcp/tests/tools.rs crates/clax-mcp/tests/comments.rs \
  crates/clax-cli/src/commands/publish.rs crates/clax-cli/tests/cli.rs plugins/pi/src/clax.ts plugins/pi/test/clax.test.ts \
  plugins/claude-code/skills/clax/SKILL.md plugins/clax/skills/clax/SKILL.md plugins/pi/skills/clax/SKILL.md docs/contract.md
git commit -m "Take a change note and addressed threads on publish, in the tools, the CLI and Pi"
git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed
```

---

### Task 15: The comment-loop smoke shows a publish linking its threads

**Files:**
- Modify: `scripts/smoke-comment-loop.sh`

- [ ] **Step 1: Assert the links**

In step 6b of the Python block, the agent's reply in step 6 emptied its working record. Mark it working on thread 3 (the one `wait_for_feedback` returned in step 5), then publish the second page with a note, naming the plain thread explicitly. Replace the `v2, _ = shim.call("publish", {"id": aid, ...})` line with:

```python
shim.call("working", {"url_or_id": aid, "thread_ids": [sent_at["thread"]]})
v2, _ = shim.call("publish", {"id": aid, "html": page, "note": "Added the team page",
                              "addresses": [plain["id"]],
                              "files": {"about.html": {"content": "<main><h2>Our team</h2></main>"}}})
if v2["note"] != "Added the team page" or v2["addressed"] != [plain["id"], sent_at["thread"]]:
    fail(f"publish note and addresses: {v2}")
for tid in (plain["id"], sent_at["thread"]):
    linked = http("GET", f"/api/artifacts/{aid}/threads/{tid}")["thread"]
    if linked["addressed_in"] != [v2["version"]] or linked["status"] != "open":
        fail(f"addressed_in after publish: {linked}")
ok(f"publish v{v2['version']} carried its note and listed the named thread and the one it was working on, both still open")
if working(aid):
    fail(f"working after the publish: {working(aid)}")
ok("working: the publish cleared the session's working record")
```

Keep the lines that follow unchanged. `plain` is defined in step 6 above this point, and `working` in Task 8.

- [ ] **Step 2: Run, gates, commit**

Run: `scripts/smoke-comment-loop.sh`
Expected: all `PASS`, ending with `comment loop smoke passed`.

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add scripts/smoke-comment-loop.sh
git commit -m "Show a publish listing its addressed threads in the comment-loop smoke"
git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed
```

---

### Task 16: Changelog logic in a framework-free model

**Files:**
- Create: `web/shell/src/view/changelog-model.ts`, `web/shell/src/view/changelog-model.test.ts`
- Modify: `web/shell/src/api.ts` (`Version.note`, `Version.addresses`, `getSeen`, `putSeen`), `web/shell/src/threads.ts` (`Thread.addressed_in`)

**Interfaces:**
- `api.ts`: `Version` gains `note?: string | null; addresses?: string[]`. Adds `getSeen(aid: string): Promise<number | null>` (`GET /api/viewers/me/seen?artifact=`; null on any failure) and `putSeen(aid: string, n: number): Promise<void>` (`PUT /api/viewers/me/seen`; failures ignored).
- `threads.ts`: `Thread` gains `addressed_in?: number[]`.
- `view/changelog-model.ts` (no `svelte` import):
  - `decideBanner(versions: Version[], seen: number | null, latest: number, pinned: boolean): { banner: Banner | null; write: number | null }`, with `type Banner = { text: string; versions: number[]; addressed: number }`.
  - `changeGroups(versions: Version[], threads: Thread[], expand: number[]): Group[]`, with `type Group = { n: number; note: string | null; threads: Thread[]; open: boolean }`. It covers versions that address at least one thread still present, newest first, at most 10.
  - `versionRows(versions: Version[], latest: number, shown: number, now: Date): Row[]`, with `type Row = { n: number; current: boolean; latest: boolean; label: string | null; note: string | null; addressed: number; when: string }`, newest first.
  - `excerpt(t: Thread): string`: the first comment's body, whitespace collapsed, cut to 80 characters plus `…`.
  - `plural(n: number, word: string): string`.

- [ ] **Step 1: Tests first**

`web/shell/src/view/changelog-model.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import type { Version } from "../api";
import type { Thread } from "../threads";
import { changeGroups, decideBanner, excerpt, versionRows } from "./changelog-model";

const V = (n: number, note: string | null = null, addresses: string[] = []): Version =>
  ({ artifact_id: "a", n, label: null, created_at: `2026-09-30T10:0${n}:00.000Z`, files: {}, note, addresses });
const T = (id: string, status: "open" | "resolved" = "open", body = "Make this  two\ncolumns"): Thread => ({
  id, artifact_id: "a", version_n: 1, status, sent_to_agent: true, has_clip: false, clip_url: null, created_at: "x", resolved_at: null,
  resolved_by: null, feedback_state: null, comments: [{ id: `${id}c`, thread_id: id, author_kind: "viewer", author_name: "Alex", via_harness: null, body, created_at: "x" }],
  anchor: { kind: "element", selector: "h2", quote: "Goals", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" },
});

describe("decideBanner", () => {
  const vs = [V(1), V(2, "Two columns", ["t1", "t2", "t3"]), V(3, null, ["t3", "t4"]), V(4, "Spacing")];
  it("says nothing on a first visit, but records it", () => {
    expect(decideBanner(vs, null, 4, false)).toEqual({ banner: null, write: 4 });
  });
  it("summarises one new version with its note", () => {
    expect(decideBanner(vs.slice(0, 2), 1, 2, false)).toEqual({ banner: { text: "v2 addressed 3 comments: Two columns", versions: [2], addressed: 3 }, write: 2 });
    expect(decideBanner(vs, 3, 4, false).banner!.text).toBe("v4: Spacing");
    expect(decideBanner([V(1), V(2, null, ["t1"])], 1, 2, false).banner!.text).toBe("v2 addressed 1 comment");
    expect(decideBanner([V(1), V(2)], 1, 2, false)).toEqual({ banner: null, write: 2 });
  });
  it("summarises several versions by distinct threads", () => {
    expect(decideBanner(vs, 1, 4, false).banner).toEqual({ text: "3 new versions, 4 comments addressed", versions: [2, 3, 4], addressed: 4 });
    expect(decideBanner([V(1), V(2), V(3, "Tidy")], 1, 3, false).banner!.text).toBe("2 new versions: Tidy");
  });
  it("stays quiet and writes nothing when pinned or already seen", () => {
    expect(decideBanner(vs, 1, 4, true)).toEqual({ banner: null, write: null });
    expect(decideBanner(vs, 4, 4, false)).toEqual({ banner: null, write: null });
  });
});

describe("changeGroups and versionRows", () => {
  it("groups present threads by version, newest first, opening the requested ones", () => {
    const vs = [V(1), V(2, "Two columns", ["t1", "gone"]), V(3, null, ["t2"])];
    const g = changeGroups(vs, [T("t1"), T("t2", "resolved")], [2]);
    expect(g.map(x => [x.n, x.threads.map(t => t.id), x.open])).toEqual([[3, ["t2"], false], [2, ["t1"], true]]);
    expect(changeGroups(vs, [T("t1"), T("t2")], [])[0].open).toBe(true);
  });
  it("lists versions newest first with notes and counts", () => {
    const rows = versionRows([V(1), V(2, "Two columns", ["t1"])], 2, 1, new Date("2026-09-30T10:05:00.000Z"));
    expect(rows).toEqual([
      { n: 2, current: false, latest: true, label: null, note: "Two columns", addressed: 1, when: "3 min ago" },
      { n: 1, current: true, latest: false, label: null, note: null, addressed: 0, when: "4 min ago" },
    ]);
  });
  it("excerpts the first comment on one line", () => {
    expect(excerpt(T("t"))).toBe("Make this two columns");
    expect(excerpt(T("t", "open", "x".repeat(90)))).toBe(`${"x".repeat(80)}…`);
  });
});
```

Run: `cd web && npx vitest run shell/src/view/changelog-model.test.ts`
Expected: FAIL (module not found).

- [ ] **Step 2: Implement**

`web/shell/src/view/changelog-model.ts`:

```ts
// The version changelog as the shell shows it (spec §8, §10): the once-per-
// viewer banner, the sidebar's "Addressed in vN" groups and the version menu's
// rows, as pure functions.
import type { Version } from "../api";
import { relativeTime } from "../format";
import type { Thread } from "../threads";

export type Banner = { text: string; versions: number[]; addressed: number };
export type Group = { n: number; note: string | null; threads: Thread[]; open: boolean };
export type Row = { n: number; current: boolean; latest: boolean; label: string | null; note: string | null; addressed: number; when: string };

/** Most groups the sidebar lists (older ones are in the version menu). */
export const MAX_GROUPS = 10;

export const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

/** What to show a viewer arriving at `latest`, and the seen mark to write
 * (null: write nothing). A pinned view neither shows nor writes. */
export function decideBanner(versions: Version[], seen: number | null, latest: number, pinned: boolean): { banner: Banner | null; write: number | null } {
  if (pinned) return { banner: null, write: null };
  if (seen === null) return { banner: null, write: latest };
  if (seen >= latest) return { banner: null, write: null };
  const fresh = versions.filter(v => v.n > seen && v.n <= latest).sort((a, b) => a.n - b.n);
  const threads = new Set(fresh.flatMap(v => v.addresses ?? []));
  const k = threads.size;
  const lastNote = [...fresh].reverse().find(v => v.note)?.note ?? null;
  const ns = fresh.map(v => v.n);
  let text: string | null = null;
  if (fresh.length === 1) {
    const v = fresh[0];
    if (k > 0) text = `v${v.n} addressed ${plural(k, "comment")}${v.note ? `: ${v.note}` : ""}`;
    else if (v.note) text = `v${v.n}: ${v.note}`;
  } else if (fresh.length > 1) {
    if (k > 0) text = `${fresh.length} new versions, ${plural(k, "comment")} addressed`;
    else if (lastNote) text = `${fresh.length} new versions: ${lastNote}`;
  }
  return { banner: text ? { text, versions: ns, addressed: k } : null, write: latest };
}

/** "Addressed in vN" groups for versions that address a thread still present,
 * newest first; `expand` names the versions shown open (the newest when empty). */
export function changeGroups(versions: Version[], threads: Thread[], expand: number[]): Group[] {
  const byId = new Map(threads.map(t => [t.id, t]));
  const groups = [...versions]
    .sort((a, b) => b.n - a.n)
    .map(v => ({ n: v.n, note: v.note ?? null, threads: (v.addresses ?? []).map(id => byId.get(id)).filter((t): t is Thread => !!t), open: false }))
    .filter(g => g.threads.length > 0)
    .slice(0, MAX_GROUPS);
  const want = expand.length ? new Set(expand) : new Set(groups.slice(0, 1).map(g => g.n));
  for (const g of groups) g.open = want.has(g.n);
  return groups;
}

/** The version menu, newest first. */
export function versionRows(versions: Version[], latest: number, shown: number, now: Date): Row[] {
  return [...versions].sort((a, b) => b.n - a.n).map(v => ({
    n: v.n, current: v.n === shown, latest: v.n === latest, label: v.label, note: v.note ?? null,
    addressed: (v.addresses ?? []).length, when: relativeTime(v.created_at, now),
  }));
}

/** The thread's first comment, on one line, at most 80 characters. */
export function excerpt(t: Thread): string {
  const s = (t.comments[0]?.body ?? "").split(/\s+/).filter(Boolean).join(" ");
  return s.length > 80 ? `${s.slice(0, 80)}…` : s;
}
```

`api.ts`:

```ts
/** This viewer's seen mark on `aid`; null when none, or when the request fails. */
export async function getSeen(aid: string): Promise<number | null> {
  try {
    const r = await fetch(`/api/viewers/me/seen?artifact=${encodeURIComponent(aid)}`);
    return r.ok ? ((await r.json()) as { seen: number | null }).seen : null;
  } catch { return null; }
}
/** Raises this viewer's seen mark on `aid` to `n`; failures are ignored (the banner may show again). */
export async function putSeen(aid: string, n: number): Promise<void> {
  try {
    await fetch("/api/viewers/me/seen", { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify({ artifact_id: aid, version: n }) });
  } catch { /* the next load decides again */ }
}
```

Run: `cd web && npx vitest run shell/src/view/changelog-model.test.ts && npm run typecheck`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add web/shell/src/view/changelog-model.ts web/shell/src/view/changelog-model.test.ts web/shell/src/api.ts web/shell/src/threads.ts
git commit -m "Decide the changelog banner, groups and version rows in a framework-free model"
git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed
```

---

### Task 17: Changelog UI: banner, Changes section, version menu, jump with highlight

**Design spec.** The person asked for a "slick changelog". Build to this, and verify it in the browser (Task 18).

- **Typography.** The shell's system UI stack. Banner text 14px/1.4. The version number (`v5`) is semibold with `font-variant-numeric: tabular-nums`. The note is regular, in `--muted` after a colon, and clamped to 2 lines with `-webkit-line-clamp`. Sidebar group heading: the sidebar `h2` style (13px, uppercase, `.04em` tracking), reading `Addressed in v5` plus a muted count. Rows are 13px. Version menu rows: `v5` semibold, then a `latest` chip (11px, `--working-soft`-style pill using the accent), the time in `--muted` 12px, and the note 13px on its own line, clamped to 2 lines.
- **Spacing.** An 8 px rhythm. Banner padding 10px 14px, gap 12px, radius 12px, max width `min(560px, calc(100vw - 32px))`, 12 px from the stage's top, centred. A 3 px accent rule on its left edge (`box-shadow: inset 3px 0 0 var(--accent)`). Shadow `0 8px 28px rgba(0,0,0,.18)`. Group rows padding 6px 8px, radius 8px, hover background `--bg`. Version panel width 340px, rows padding 10px 12px, separated by 1 px `--border`.
- **Motion.** The banner enters over 180 ms `cubic-bezier(.2,.8,.2,1)` from `translateY(-6px)`, opacity 0 to 1, and leaves over 120 ms with opacity only. The version panel opens over 140 ms from `scale(.98)` and opacity 0, with origin at the top right. Group disclosure uses native `<details>`, with no animation. Under `prefers-reduced-motion: reduce` there are no transforms and no transitions (instant), and the bridge's jump highlight is a static outline for 1.2 s with an instant scroll.
- **Colour.** Only existing tokens (`--card`, `--fg`, `--muted`, `--border`, `--accent`, `--on-accent`, `--bg`) plus `--chip: color-mix(in srgb, var(--accent) 14%, transparent)`, which works in both themes because it derives from `--accent`.
- **Phone width (≤480px).** The banner spans the stage minus 16 px gutters. Its buttons sit on a second row, right-aligned. The version panel becomes a sheet fixed under the top bar (`left: 8px; right: 8px; max-height: 70vh; overflow: auto`). Nothing scrolls sideways.
- **Accessibility.** The banner is `role="status"` (polite). Show and Dismiss are real buttons: `Show changes` and `Dismiss` (`aria-label="Dismiss changelog"`), and focus is never moved to the banner. The version menu button has `aria-haspopup="dialog"` and `aria-expanded`. The panel is `role="dialog" aria-label="Versions"`. Opening it focuses the current version's link. Escape or a click outside closes it and returns focus to the button. Rows are links (`aria-current="page"` on the shown version). Group rows: the jump is a button labelled with the anchor and excerpt, and Resolve is a button labelled `Resolve`.

**Files:**
- Create: `web/shell/src/ui/ChangelogBanner.svelte`, `web/shell/src/ui/AddressedGroups.svelte`, `web/shell/src/ui/VersionMenu.svelte`, `web/shell/src/changelog-ui.test.ts`
- Modify: `web/shell/src/view/artifact-controller.ts`, `web/shell/src/view/artifact-controller.test.ts`, `web/shell/src/ui/TopbarIsland.svelte`, `web/shell/src/ui/StageIsland.svelte`, `web/shell/src/ui/SidebarIsland.svelte`, `web/shell/src/ui/Sidebar.svelte`, `web/shell/src/sidebar.test.ts`, `web/shell/src/theme.css`, `web/bridge/src/comment-mode.ts`, `web/bridge/src/bridge.ts`, `web/e2e/viewer.spec.ts`

**Interfaces:**
- `ArtifactController`:
  - `ViewState` gains `changelog: Banner | null`, `changesOpen: number[]` (the versions whose groups start expanded) and `changesFocus: number` (bumped to move focus to the Changes heading). They start as `null`, `[]` and `0`.
  - New methods: `dismissChangelog(): void` and `showChanges(): void` (clears the banner, opens the panel, bumps `changesFocus`).
  - A private `decideChangelog()`. `start()` runs it once, after the viewer lookup the controller already awaits in `openStream`, and after `data` is present. It is never on the path to first paint.
- `ChangelogBanner.svelte` `{ banner: Banner; onShow(): void; onDismiss(): void }`. The stage island loads it by dynamic `import()` only when a banner is decided.
- `AddressedGroups.svelte` `{ groups: Group[]; focus: number; onJump(t: Thread): void; onResolve(t: Thread): void }`. `Sidebar` loads it by dynamic `import()` only when there are groups.
- `VersionMenu.svelte` `{ rows: Row[]; shown: number; latest: number; hrefFor(n: number): string; onChoose(n: number): void }`. It is eager and replaces the `<select>` in `TopbarIsland.svelte`.
- `Sidebar` gains `changes?: Group[]` and `changesFocus?: number`.

- [ ] **Step 1: Tests first**

`web/shell/src/changelog-ui.test.ts`:

```ts
import { describe, expect, it, vi } from "vitest";
import { flush, mount } from "./test/svelte";
import ChangelogBanner from "./ui/ChangelogBanner.svelte";
import VersionMenu from "./ui/VersionMenu.svelte";

describe("changelog UI", () => {
  it("the banner is a polite status with Show and Dismiss", () => {
    const onShow = vi.fn();
    const onDismiss = vi.fn();
    const m = mount(ChangelogBanner, { banner: { text: "v2 addressed 3 comments: Two columns", versions: [2], addressed: 3 }, onShow, onDismiss });
    const el = m.root.querySelector(".changelog-banner")!;
    expect(el.getAttribute("role")).toBe("status");
    expect(el.textContent).toContain("v2 addressed 3 comments: Two columns");
    flush(() => (m.root.querySelector("button[aria-label='Dismiss changelog']") as HTMLButtonElement).click());
    expect(onDismiss).toHaveBeenCalled();
    flush(() => Array.from(m.root.querySelectorAll("button")).find(b => b.textContent === "Show changes")!.click());
    expect(onShow).toHaveBeenCalled();
    m.unmount();
  });

  it("the version menu opens a dialog of links, focuses the shown version, chooses through the controller, and closes on Escape", async () => {
    const rows = [
      { n: 2, current: false, latest: true, label: null, note: "Two columns", addressed: 1, when: "just now" },
      { n: 1, current: true, latest: false, label: "first", note: null, addressed: 0, when: "5 min ago" },
    ];
    const onChoose = vi.fn();
    const m = mount(VersionMenu, { rows, shown: 1, latest: 2, hrefFor: (n: number) => `/a/x${n === 2 ? "" : `/v/${n}`}`, onChoose });
    const button = m.root.querySelector("button.version-button") as HTMLButtonElement;
    expect(button.textContent).toBe("v1 of 2");
    expect(button.getAttribute("aria-expanded")).toBe("false");
    flush(() => button.click());
    const dialog = m.root.querySelector("[role=dialog]")!;
    expect(dialog.getAttribute("aria-label")).toBe("Versions");
    const links = Array.from(dialog.querySelectorAll("a"));
    expect(links.map(a => a.getAttribute("href"))).toEqual(["/a/x", "/a/x/v/1"]);
    expect(dialog.textContent).toContain("Two columns");
    expect(dialog.textContent).toContain("addressed 1");
    await vi.waitFor(() => expect(document.activeElement).toBe(links[1]));
    expect(links[1].getAttribute("aria-current")).toBe("page");
    flush(() => links[0].click());
    expect(onChoose).toHaveBeenCalledWith(2);
    flush(() => button.click());
    const again = m.root.querySelector("[role=dialog]")!;
    flush(() => again.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })));
    expect(m.root.querySelector("[role=dialog]")).toBeNull();
    expect(document.activeElement).toBe(button);
    m.unmount();
  });
});
```

Add to `sidebar.test.ts`:

```ts
  it("lists addressed threads by version, with jump and one-click Resolve, loaded after the threads", async () => {
    const t: Thread = { ...base, id: "a1", anchor, status: "open", sent_to_agent: true, comments: [comment("1", "viewer", "Alex", "two columns")] };
    const onSelect = vi.fn();
    const onResolve = vi.fn();
    const m = mount(Sidebar, { threads: [t], resolved: {}, changes: [{ n: 2, note: "Two columns", threads: [t], open: true }], changesFocus: 0,
      selected: null, onSelect, onSend: vi.fn(), onResolve, onReply: vi.fn() });
    const section = await vi.waitFor(() => { const s = m.root.querySelector(".section-changes"); if (!s) throw new Error("not loaded yet"); return s; });
    expect(section.querySelector("summary")!.textContent).toContain("Addressed in v2");
    flush(() => (section.querySelector(".change-jump") as HTMLButtonElement).click());
    expect(onSelect).toHaveBeenCalledWith(t);
    flush(() => Array.from(section.querySelectorAll("button")).find(b => b.textContent === "Resolve")!.click());
    expect(onResolve).toHaveBeenCalledWith(t);
    expect(m.root.querySelectorAll(".section-open .thread-card")).toHaveLength(1);
    m.unmount();
  });
```

Add to `view/artifact-controller.test.ts`. Extend the harness's `fetch` stub to answer `/api/viewers/me/seen` with `{ seen: 1 }` and record `PUT` bodies. The shared `loaded` fixture is at version 2; give its `versions` a `v2` with `addresses: ["t1"]`, `note: "Two columns"`, and a `v1`, for this test only:

```ts
  it("decides the changelog after load, writes the seen mark, and Show opens the Changes section", async () => {
    const saved = loaded.versions;
    loaded.versions = [{ artifact_id: ID, n: 1, label: null, created_at: "x", files: {} }, { artifact_id: ID, n: 2, label: null, created_at: "x", files: {}, note: "Two columns", addresses: ["t1"] }] as typeof saved;
    try {
      const { ctl } = await started();
      await vi.waitFor(() => expect(ctl.state.get().changelog?.text).toBe("v2 addressed 1 comment: Two columns"));
      const puts = (fetch as unknown as ReturnType<typeof vi.fn>).mock.calls.filter(([u, i]) => String(u).startsWith("/api/viewers/me/seen") && (i as RequestInit | undefined)?.method === "PUT");
      expect(JSON.parse((puts[0][1] as RequestInit).body as string)).toEqual({ artifact_id: ID, version: 2 });
      ctl.showChanges();
      expect(ctl.state.get()).toMatchObject({ changelog: null, panel: true, changesOpen: [2], changesFocus: 1 });
      ctl.dispose();
    } finally {
      loaded.versions = saved;
    }
  });
```

In the harness's `fetch` stub, answer `url.startsWith("/api/viewers/me/seen")` with `{ seen: 1 }` before the generic `/api/viewers` branch.

Run: `cd web && npx vitest run shell/src/changelog-ui.test.ts shell/src/sidebar.test.ts shell/src/view/artifact-controller.test.ts`
Expected: FAIL.

- [ ] **Step 2: Components**

`web/shell/src/ui/ChangelogBanner.svelte`:

```svelte
<script lang="ts">
  import type { Banner } from "../view/changelog-model";

  let { banner, onShow, onDismiss }: { banner: Banner; onShow(): void; onDismiss(): void } = $props();
</script>

<div class="changelog-banner" role="status">
  <p class="changelog-text">{banner.text}</p>
  <div class="changelog-actions">
    <button type="button" class="primary" onclick={onShow}>Show changes</button>
    <button type="button" aria-label="Dismiss changelog" onclick={onDismiss}>Dismiss</button>
  </div>
</div>
```

`web/shell/src/ui/AddressedGroups.svelte`:

```svelte
<script lang="ts">
  import { anchorLabel, type Thread } from "../threads";
  import { excerpt, type Group } from "../view/changelog-model";

  let { groups, focus, onJump, onResolve }: { groups: Group[]; focus: number; onJump(t: Thread): void; onResolve(t: Thread): void } = $props();
  let heading: HTMLElement | undefined = $state();
  // Show changes bumps `focus`; the heading takes focus, never on first render.
  $effect(() => { if (focus > 0) heading?.focus(); });
</script>

<section class="section-changes" aria-label="Changes">
  <h2 tabindex="-1" bind:this={heading}>Changes</h2>
  {#each groups as g (g.n)}
    <details class="change-group" open={g.open}>
      <summary>Addressed in v{g.n} <span class="muted">{g.threads.length}</span></summary>
      {#if g.note}<p class="change-note">{g.note}</p>{/if}
      <ul>
        {#each g.threads as t (t.id)}
          <li class="change-row">
            <button type="button" class="change-jump" onclick={() => onJump(t)}>
              <span class="anchor-label">{anchorLabel(t.anchor)}</span>
              <span class="change-excerpt muted">{excerpt(t)}</span>
            </button>
            {#if t.status === "open"}
              <button type="button" onclick={() => onResolve(t)}>Resolve</button>
            {:else}
              <span class="muted small">Resolved</span>
            {/if}
          </li>
        {/each}
      </ul>
    </details>
  {/each}
</section>
```

`web/shell/src/ui/VersionMenu.svelte`:

```svelte
<script lang="ts">
  import { tick } from "svelte";
  import type { Row } from "../view/changelog-model";

  let { rows, shown, latest, hrefFor, onChoose }: { rows: Row[]; shown: number; latest: number; hrefFor(n: number): string; onChoose(n: number): void } = $props();
  let open = $state(false);
  let button: HTMLButtonElement | undefined = $state();
  let panel: HTMLDivElement | undefined = $state();

  async function toggle() {
    open = !open;
    if (open) { await tick(); panel?.querySelector<HTMLElement>("a[aria-current=page]")?.focus(); }
  }
  function close() { open = false; button?.focus(); }
  function outside(e: PointerEvent) {
    if (open && !panel?.contains(e.target as Node) && !button?.contains(e.target as Node)) open = false;
  }
  function choose(e: MouseEvent, n: number) {
    // A plain click moves through the controller, as the select did; a
    // modified click keeps the link's own behaviour (a new tab).
    if (e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
    e.preventDefault();
    open = false;
    onChoose(n);
  }
</script>

<svelte:window onpointerdown={outside} />

<div class="version-menu">
  <button type="button" class="version-button" bind:this={button} aria-haspopup="dialog" aria-expanded={open} onclick={toggle}>v{shown} of {latest}</button>
  {#if open}
    <div class="version-panel" role="dialog" aria-label="Versions" tabindex="-1" bind:this={panel}
      onkeydown={e => { if (e.key === "Escape") { e.preventDefault(); close(); } }}>
      <ol>
        {#each rows as r (r.n)}
          <li>
            <a href={hrefFor(r.n)} aria-current={r.current ? "page" : undefined} onclick={e => choose(e, r.n)}>
              <span class="version-head"><strong>v{r.n}</strong>{#if r.latest}<span class="chip">latest</span>{/if}<span class="muted">{r.when}</span>{#if r.label}<span class="muted">· {r.label}</span>{/if}</span>
              {#if r.note}<span class="version-note">{r.note}</span>{/if}
              {#if r.addressed > 0}<span class="version-count muted">addressed {r.addressed}</span>{/if}
            </a>
          </li>
        {/each}
      </ol>
    </div>
  {/if}
</div>
```

If `svelte-check` reports `a11y_no_noninteractive_element_interactions` for the dialog's `onkeydown`, put `<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->` directly above the `<div class="version-panel" …>`, with a comment saying Escape closes a dialog. Do not remove the handler.

- [ ] **Step 3: Wire the controller and the islands**

`view/artifact-controller.ts`:

```ts
  private decided = false;

  /** The changelog banner for this load (spec §8): once, after the viewer
   * lookup and the artifact, never on the path to first paint. */
  private decideChangelog(): void {
    const s = this.s;
    if (this.decided || !s.data) return;
    this.decided = true;
    const latest = s.data.artifact.current_version;
    const pinned = this.pinnedVersion !== null || this.shown() !== latest;
    const versions = s.data.versions;
    void getSeen(this.id).then(seen => {
      if (this.disposed) return;
      const { banner, write } = decideBanner(versions, seen, latest, pinned);
      if (write !== null) void putSeen(this.id, write);
      if (banner) this.set({ changelog: banner, changesOpen: banner.versions });
    });
  }

  dismissChangelog(): void { this.set({ changelog: null }); }

  showChanges(): void {
    this.set(s => ({ changelog: null, panel: true, changesFocus: s.changesFocus + 1 }));
  }
```

Call `this.decideChangelog()` in `openStream`'s `first` (after `getViewer()` settles), and also wherever `data` is first set. The `decided` flag makes it run once, whichever comes last. Import `getSeen`, `putSeen` from `../api` and `decideBanner`, `type Banner` from `./changelog-model`.

`ui/TopbarIsland.svelte`: replace the `<select …>…</select>` with:

```svelte
  <VersionMenu rows={versionRows(s.data.versions, latest, shown, new Date())} {shown} {latest}
    hrefFor={n => ctl.here(n === latest ? null : n, s)} onChoose={n => ctl.chooseVersion(n)} />
```

and import `VersionMenu` and `versionRows`. `ctl.chooseVersion` is the method the select called, so moving between versions behaves exactly as before.

`ui/StageIsland.svelte`: before the `{#if s.newer && !s.deleted}` banner, add:

```svelte
  {#if s.changelog && !s.newer && !s.deleted}
    {#await import("./ChangelogBanner.svelte") then { default: ChangelogBanner }}
      <ChangelogBanner banner={s.changelog} onShow={() => ctl.showChanges()} onDismiss={() => ctl.dismissChangelog()} />
    {/await}
  {/if}
```

`ui/SidebarIsland.svelte`: pass `changes={changeGroups(s.data.versions, s.threads, s.changesOpen)}` and `changesFocus={s.changesFocus}` to `Sidebar`.

`ui/Sidebar.svelte`: add the two props. After `{@render p.header?.()}`, and before the Open section, add:

```svelte
  {#if p.changes?.length}
    {#await import("./AddressedGroups.svelte") then { default: AddressedGroups }}
      <AddressedGroups groups={p.changes} focus={p.changesFocus ?? 0} onJump={p.onSelect} onResolve={p.onResolve} />
    {/await}
  {/if}
```

`onSelect` is the controller's `selectThread`: it selects the thread, scrolls the frame to the anchor and flashes it, and first navigates to the thread's page when it is on another one. The existing sections are unchanged, so pin numbering is untouched.

- [ ] **Step 4: Reduced-motion highlight in the bridge**

In `web/bridge/src/comment-mode.ts`, append to `CSS`: `@media (prefers-reduced-motion: reduce){.o.flash,.f.flash{animation:none}}`. In `flash`, use a 1200 ms timeout instead of 1800 when `matchMedia("(prefers-reduced-motion: reduce)").matches`. In `web/bridge/src/bridge.ts`, in the `clax:scroll-to` case (inside its `withComment(l => …)`), compute `const behavior: ScrollBehavior = matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth";` and pass it to both `scrollBy` and `scrollIntoView`.

- [ ] **Step 5: Styles**

Append to `web/shell/src/theme.css`:

```css
/* Version changelog (spec §8). */
:root { --chip: color-mix(in srgb, var(--accent) 14%, transparent); }
.changelog-banner { position: absolute; top: 12px; left: 50%; transform: translateX(-50%); z-index: 5; width: max-content; max-width: min(560px, calc(100% - 32px));
  display: flex; align-items: center; gap: 12px; padding: 10px 14px; border-radius: 12px; background: var(--card); color: var(--fg);
  border: 1px solid var(--border); box-shadow: inset 3px 0 0 var(--accent), 0 8px 28px rgba(0,0,0,.18); animation: changelog-in 180ms cubic-bezier(.2,.8,.2,1); }
@keyframes changelog-in { from { opacity: 0; transform: translate(-50%, -6px); } to { opacity: 1; transform: translate(-50%, 0); } }
.changelog-text { margin: 0; font-size: 14px; line-height: 1.4; font-variant-numeric: tabular-nums; display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden; }
.changelog-actions { display: flex; gap: 8px; flex: none; }
.section-changes h2:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
.change-group { margin-bottom: 8px; }
.change-group summary { cursor: pointer; font-size: 13px; font-weight: 600; font-variant-numeric: tabular-nums; padding: 4px 0; }
.change-note { margin: 2px 0 6px; font-size: 13px; color: var(--muted); }
.change-group ul { list-style: none; margin: 0; padding: 0; }
.change-row { display: flex; align-items: center; gap: 8px; padding: 6px 8px; border-radius: 8px; }
.change-row:hover { background: var(--bg); }
.change-jump { flex: 1; min-width: 0; display: flex; flex-direction: column; align-items: flex-start; gap: 2px; background: none; border: 0; padding: 0; text-align: left; font-size: 13px; }
.change-excerpt { max-width: 100%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 12px; }
.change-row > button:not(.change-jump) { flex: none; padding: 2px 8px; font-size: 12px; }
.version-menu { position: relative; flex: none; }
.version-button { font-variant-numeric: tabular-nums; }
.version-panel { position: absolute; right: 0; top: calc(100% + 6px); z-index: 20; width: 340px; max-height: 70vh; overflow: auto; background: var(--card);
  border: 1px solid var(--border); border-radius: 12px; box-shadow: 0 12px 32px rgba(0,0,0,.2); transform-origin: top right; animation: version-in 140ms ease-out; }
@keyframes version-in { from { opacity: 0; transform: scale(.98); } to { opacity: 1; transform: none; } }
.version-panel ol { list-style: none; margin: 0; padding: 0; }
.version-panel li + li { border-top: 1px solid var(--border); }
.version-panel a { display: flex; flex-direction: column; gap: 4px; padding: 10px 12px; }
.version-panel a:hover, .version-panel a:focus-visible { background: var(--bg); outline: none; }
.version-panel a[aria-current=page] { box-shadow: inset 3px 0 0 var(--accent); }
.version-head { display: flex; align-items: baseline; gap: 8px; font-size: 13px; font-variant-numeric: tabular-nums; }
.version-head .muted { font-size: 12px; }
.chip { font-size: 11px; padding: 0 6px; border-radius: 999px; background: var(--chip); color: var(--fg); }
.version-note { font-size: 13px; display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden; }
.version-count { font-size: 12px; }
@media (max-width: 480px) {
  .changelog-banner { width: calc(100% - 16px); max-width: none; flex-wrap: wrap; }
  .changelog-actions { width: 100%; justify-content: flex-end; }
  .version-panel { position: fixed; left: 8px; right: 8px; width: auto; top: 56px; }
}
@media (prefers-reduced-motion: reduce) { .changelog-banner, .version-panel { animation: none; } }
```

- [ ] **Step 6: Update the one e2e step that used the `<select>`**

In `web/e2e/viewer.spec.ts`, replace `await page.selectOption("select", "1");` with:

```ts
  await page.getByRole("button", { name: "v2 of 2" }).click();
  await page.getByRole("dialog", { name: "Versions" }).getByRole("link", { name: /^v1\b/ }).click();
```

The assertions that follow it (URL `/v/1`, frame shows `v1`) stay as they are.

- [ ] **Step 7: Run, check the budgets, commit**

Run: `cd web && npm run lint && npm run typecheck && npx vitest run && npm run build && node scripts/bundle-size.mjs && npx playwright test e2e/viewer.spec.ts; echo "exit=$?"`
Expected: `exit=0`. `ChangelogBanner` and `AddressedGroups` are separate chunks outside `artifact.html`'s closure: `grep -l "Show changes" dist/_clax/shell/*.js` names a chunk that is not in `manifest["artifact.html"].imports`. If the bundle budget fails, stop and report the sizes. Never record a higher budget.

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add web/shell/src/ui/ChangelogBanner.svelte web/shell/src/ui/AddressedGroups.svelte web/shell/src/ui/VersionMenu.svelte web/shell/src/changelog-ui.test.ts \
  web/shell/src/view/artifact-controller.ts web/shell/src/view/artifact-controller.test.ts web/shell/src/ui/TopbarIsland.svelte web/shell/src/ui/StageIsland.svelte \
  web/shell/src/ui/SidebarIsland.svelte web/shell/src/ui/Sidebar.svelte web/shell/src/sidebar.test.ts web/shell/src/theme.css \
  web/bridge/src/comment-mode.ts web/bridge/src/bridge.ts web/e2e/viewer.spec.ts
git commit -m "Show each version's changelog: a once-per-viewer banner, the Changes section and the version menu"
git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed
```

---

### Task 18: Browser tests and verification for the changelog

**Files:**
- Create: `web/e2e/changelog.spec.ts`
- Modify: `web/e2e/fixtures.ts` (`publishNext`, `seenOf`)

**Interfaces:**
- `publishNext(base, token, sid, aid, ifVersion, extra: { note?: string; addresses?: string[] }): Promise<any>` publishes `PAGE` as the next version with `X-Clax-Session`.
- `seenOf(page, aid): Promise<number | null>` reads this page's viewer's seen mark from inside the page, so the request carries its cookie.

- [ ] **Step 1: Fixtures**

Append to `web/e2e/fixtures.ts`:

```ts
/** Publishes `<main><h2>Quarterly goals</h2></main>` as the next version of `aid`, as session `sid`, with a note or addresses. */
export async function publishNext(base: string, token: string, sid: string, aid: string, ifVersion: number, extra: { note?: string; addresses?: string[] } = {}) {
  return api(base, token, `/api/artifacts/${aid}/versions`, { method: "POST", session: sid,
    body: JSON.stringify({ if_version: ifVersion, ...extra, files: { "index.html": { content: "<main><h2>Quarterly goals</h2></main>", encoding: "utf8" } } }) });
}

/** The seen mark of the viewer whose shell is `page`. */
export async function seenOf(page: Page, aid: string): Promise<number | null> {
  return page.evaluate(async id => (await (await fetch(`/api/viewers/me/seen?artifact=${id}`)).json()).seen, aid);
}
```

- [ ] **Step 2: The spec**

`web/e2e/changelog.spec.ts`:

```ts
import { test, expect } from "@playwright/test";
import { contentFrame, openArtifact, postThread, publishAs, publishNext, reach, registerSession, seenOf, startDaemon } from "./fixtures";

let d: Awaited<ReturnType<typeof startDaemon>>;
test.beforeAll(async () => { test.setTimeout(180_000); d = await startDaemon(); });
test.afterAll(async () => { await d?.stop(); });

const PAGE = "<main><h2>Quarterly goals</h2></main>";

async function fresh(title: string) {
  const s = await registerSession(d.base, d.token, "claude", `cl-${title}`);
  const { artifact } = await publishAs(d.base, d.token, s.id, title, { "index.html": PAGE });
  return { sid: s.id, aid: artifact.id };
}

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: the banner shows once per viewer per version, and Show opens the Changes section`, async ({ page, browser }) => {
    const { sid, aid } = await fresh(`Banner ${mode}`);
    const t1 = await postThread(d.base, aid, "@agent two columns");
    await openArtifact(page, d.base, aid, 1, mode);
    await expect.poll(() => seenOf(page, aid)).toBe(1);
    await expect(page.locator(".changelog-banner")).toHaveCount(0);
    await publishNext(d.base, d.token, sid, aid, 1, { note: "Two columns", addresses: [t1.id] });
    await page.getByRole("button", { name: "Reload" }).click();
    await contentFrame(page, aid, 2);
    const banner = page.locator(".changelog-banner");
    await expect(banner).toHaveText(/v2 addressed 1 comment: Two columns/);
    await expect(banner).toHaveAttribute("role", "status");
    await banner.getByRole("button", { name: "Show changes" }).click();
    await expect(banner).toHaveCount(0);
    const changes = page.locator(".section-changes");
    await expect(changes.locator("summary")).toContainText("Addressed in v2");
    await expect(changes.locator("h2")).toBeFocused();
    await page.reload();
    await contentFrame(page, aid, 2);
    await expect.poll(() => seenOf(page, aid)).toBe(2);
    await expect(page.locator(".changelog-banner")).toHaveCount(0);
    const other = await (await browser.newContext()).newPage();
    await openArtifact(other, d.base, aid, 2, mode);
    await expect.poll(() => seenOf(other, aid)).toBe(2);
    await expect(other.locator(".changelog-banner")).toHaveCount(0);
    await other.context().close();
  });

  test(`${mode}: a returning viewer gets a summary of every version since the last visit`, async ({ page }) => {
    const { sid, aid } = await fresh(`Summary ${mode}`);
    const [t1, t2] = [await postThread(d.base, aid, "@agent one"), await postThread(d.base, aid, "@agent two")];
    await openArtifact(page, d.base, aid, 1, mode);
    await expect.poll(() => seenOf(page, aid)).toBe(1);
    await page.goto(`${d.base}/`);
    await publishNext(d.base, d.token, sid, aid, 1, { addresses: [t1.id] });
    await publishNext(d.base, d.token, sid, aid, 2, { addresses: [t1.id, t2.id], note: "Spacing" });
    await publishNext(d.base, d.token, sid, aid, 3, {});
    await openArtifact(page, d.base, aid, 4, mode);
    await expect(page.locator(".changelog-banner")).toHaveText(/3 new versions, 2 comments addressed/);
  });

  test(`${mode}: a changelog row jumps to its anchor with a highlight, and resolves in one click`, async ({ page }) => {
    const { sid, aid } = await fresh(`Jump ${mode}`);
    const t1 = await postThread(d.base, aid, "@agent two columns");
    await publishNext(d.base, d.token, sid, aid, 1, { addresses: [t1.id], note: "Two columns" });
    const frame = await openArtifact(page, d.base, aid, 2, mode);
    await page.getByRole("button", { name: /Threads/ }).click();
    const row = page.locator(".section-changes .change-row").first();
    await reach(page, row.locator(".change-jump"));
    await row.locator(".change-jump").click();
    await expect(frame.locator("clax-overlay .o.flash")).toHaveCount(1);
    await row.getByRole("button", { name: "Resolve" }).click();
    await expect(row).toContainText("Resolved");
    await expect(page.locator(`.section-resolved .thread-card[data-thread="${t1.id}"]`)).toHaveCount(1);
  });

  test(`${mode}: the version menu reads as a changelog and moves between versions`, async ({ page }) => {
    const { sid, aid } = await fresh(`Menu ${mode}`);
    const t1 = await postThread(d.base, aid, "@agent two columns");
    await publishNext(d.base, d.token, sid, aid, 1, { addresses: [t1.id], note: "Two columns" });
    await openArtifact(page, d.base, aid, 2, mode);
    await page.getByRole("button", { name: "v2 of 2" }).click();
    const dialog = page.getByRole("dialog", { name: "Versions" });
    await expect(dialog.getByRole("link").first()).toContainText("Two columns");
    await expect(dialog.getByRole("link").first()).toContainText("addressed 1");
    await expect(dialog.locator("a[aria-current=page]")).toBeFocused();
    await page.keyboard.press("Escape");
    await expect(dialog).toHaveCount(0);
    await expect(page.getByRole("button", { name: "v2 of 2" })).toBeFocused();
    await page.getByRole("button", { name: "v2 of 2" }).click();
    await dialog.getByRole("link", { name: /^v1\b/ }).click();
    await expect(page).toHaveURL(new RegExp(`/a/${aid}/v/1$`));
  });
}

test("the changelog fits a phone in dark mode and holds still under reduced motion", async ({ page }) => {
  const { sid, aid } = await fresh("Phone changelog");
  const t1 = await postThread(d.base, aid, "@agent two columns");
  await page.setViewportSize({ width: 375, height: 812 });
  await page.emulateMedia({ colorScheme: "dark", reducedMotion: "reduce" });
  await openArtifact(page, d.base, aid, 1, "subdomain");
  await expect.poll(() => seenOf(page, aid)).toBe(1);
  await publishNext(d.base, d.token, sid, aid, 1, { addresses: [t1.id], note: "A longer note that has to wrap on a phone-width screen without pushing anything sideways" });
  await page.reload();
  const banner = page.locator(".changelog-banner");
  await expect(banner).toBeVisible();
  expect(await banner.evaluate(e => getComputedStyle(e).animationName)).toBe("none");
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(375);
  await page.getByRole("button", { name: "v2 of 2" }).click();
  const panel = page.getByRole("dialog", { name: "Versions" });
  const box = (await panel.boundingBox())!;
  expect(box.x).toBeGreaterThanOrEqual(0);
  expect(box.x + box.width).toBeLessThanOrEqual(375);
});
```

Run: `cd web && npx playwright test e2e/changelog.spec.ts e2e/viewer.spec.ts e2e/working.spec.ts && npm run perf; echo "exit=$?"`
Expected: PASS and `exit=0`. The time-to-usable budgets hold: the banner and the Changes section load after first paint.

- [ ] **Step 3: Browser verification (required)**

With a scratch `CLAX_HOME` and `--port 0`, as in Task 11 Step 3, create an artifact as a session, open it once in the browser, then post a thread and publish v2 with `note` and `addresses`:

```bash
python3 - <<'PY'
import json, os, urllib.request, uuid
info = json.load(open(os.path.join(os.environ["CLAX_HOME"], "daemon.json")))
base, tok = f"http://localhost:{info['port']}", info["token"]
def call(method, path, body=None, session=None):
    req = urllib.request.Request(base + path, data=None if body is None else json.dumps(body).encode(), method=method)
    req.add_header("authorization", f"Bearer {tok}"); req.add_header("content-type", "application/json")
    if session: req.add_header("x-clax-session", session)
    return json.load(urllib.request.urlopen(req))
s = call("POST", "/api/sessions", {"harness": "claude", "harness_session_id": "verify-cl", "cwd": "/tmp"})["session"]["id"]
a = call("POST", "/api/artifacts", {"title": "Verify changelog", "files": {"index.html": {"content": "<main><h2>Quarterly goals</h2></main>", "encoding": "utf8"}}}, s)["artifact"]["id"]
print(f"open {base}/a/{a} now, then press Enter"); input()
b = uuid.uuid4().hex
anchor = json.dumps({"kind": "element", "selector": "body > main > h2", "quote": "Quarterly goals", "prefix": None, "suffix": None, "html_hash": None, "rect": None, "custom_name": None})
form = "".join(f"--{b}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n" for k, v in [("anchor", anchor), ("body", "Make this two columns"), ("version", "1")]) + f"--{b}--\r\n"
req = urllib.request.Request(f"{base}/api/artifacts/{a}/threads", data=form.encode(), method="POST")
req.add_header("content-type", f"multipart/form-data; boundary={b}")
tid = json.load(urllib.request.urlopen(req))["thread"]["id"]
call("POST", f"/api/artifacts/{a}/versions", {"if_version": 1, "note": "Two columns; third bullet dropped", "addresses": [tid],
     "files": {"index.html": {"content": "<main><h2>Quarterly goals</h2><p>v2</p></main>", "encoding": "utf8"}}}, s)
print("published v2; reload the tab")
PY
```

In the browser, check each of these and write down what you saw in the task report:
- The banner enters smoothly and reads `v2 addressed 1 comment: Two columns; third bullet dropped`.
- Show changes opens the sidebar at "Addressed in v2", and a row click scrolls to and flashes the heading.
- Resolve works in one click.
- The version menu lists both versions with the note.
- Both themes look right, and at 375 px nothing scrolls sideways and the menu is a sheet.
- A reload shows no banner.
- With reduced motion emulated there is no slide and no pulse.

Stop the daemon with `cargo run -q -p clax-cli -- stop`.

- [ ] **Step 4: Gates and commit**

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add web/e2e/changelog.spec.ts web/e2e/fixtures.ts
git commit -m "Test the version changelog in the browser in both frame modes"
git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed
```

---

### Task 19: Batch send in the store: one transaction, one batch, grouped payload

Decisions: `.superpowers/sdd/2026-09-30-agent-working/decisions.md`, "Batch send to agent". The rules this task implements are in "Design: batch send to agent" above.

**Files:**
- Create: `crates/clax-core/src/store/batches.rs`
- Modify: `crates/clax-core/src/store/mod.rs`, `crates/clax-core/src/store/migrations.rs` (migration 11), `crates/clax-core/src/store/feedback.rs` (`send_to_agent` split, `take_feedback` fills `batch`), `crates/clax-core/src/store/threads.rs` (`delete_thread_touched`), `crates/clax-core/src/store/artifacts.rs` (`delete_artifact`), `crates/clax-core/src/feedback.rs` (`FeedbackItem.batch`, `FeedbackBatch`, `render_items`), `crates/clax-core/src/lib.rs`

**Interfaces:**
- `clax_core::feedback::FeedbackBatch { id: String, size: u32, note: Option<String>, sent_by: String }`. `FeedbackItem` gains `batch: Option<FeedbackBatch>` (`#[serde(default)]`).
- `clax_core::store::batches::{MAX_BATCH_THREADS: usize = 20, MAX_BATCH_NOTE_CHARS: usize = 280, SendBatch { thread_ids: Vec<String>, note: Option<String>, sent_by: String }, BatchResult { batch: FeedbackBatch, sent: Vec<String>, unchanged: Vec<String>, touched: Touched }, ThreadSend { batch_id: String, size: u32, note: Option<String>, sent_by: String, sent_at: String }}`.
- `Store::send_batch(&self, aid: &ArtifactId, b: SendBatch) -> Result<BatchResult>` and `Store::thread_sends(&self, thread_id: &str) -> Result<Vec<ThreadSend>>`.
- `render_items` output. A run of items from one batch is led by `[clax] N comments on "<title>", sent together by <name>.`, followed by ` Note: "<note>"` when the batch has a note. N counts that batch's items in this delivery. Items with no batch render as before.

- [ ] **Step 1: Failing tests**

`crates/clax-core/src/store/batches.rs`, tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::test_util::{anchor, artifact, session, store};
    use crate::{NewThread, Store, TakeFeedback, Tier};

    fn thread(st: &Store, id: &ArtifactId, body: &str) -> String {
        st.create_thread(id, NewThread { version_n: 1, anchor: anchor(), author_name: "Alex".into(), body: body.into(), clip: None, via_page: false })
            .unwrap()
            .id
    }
    fn batch(ids: &[&String], note: Option<&str>) -> SendBatch {
        SendBatch { thread_ids: ids.iter().map(|s| s.to_string()).collect(), note: note.map(String::from), sent_by: "Alex".into() }
    }
    fn code(e: crate::CoreError) -> &'static str {
        match e { crate::CoreError::Invalid { code, .. } => code, other => panic!("{other:?}") }
    }

    #[test]
    fn a_batch_sends_every_thread_in_one_transaction_and_one_delivery() {
        let (_d, st) = store();
        let sid = session(&st, "claude", "b1");
        let id = artifact(&st, Some(&sid));
        st.ensure_watch(&sid, &id).unwrap();
        let ts: Vec<String> = (0..3).map(|i| thread(&st, &id, &format!("c{i}"))).collect();
        let r = st.send_batch(&id, batch(&ts.iter().collect::<Vec<_>>(), Some("  Before the demo "))).unwrap();
        assert_eq!(r.sent, ts);
        assert_eq!((r.batch.size, r.batch.note.as_deref()), (3, Some("Before the demo")));
        for t in &ts {
            assert!(st.get_thread(t).unwrap().unwrap().sent_to_agent);
            assert_eq!(st.thread_sends(t).unwrap()[0].size, 3);
        }
        let (items, _) = st.take_feedback(&TakeFeedback { session_id: sid.clone(), tier: Tier::Piggyback, artifact_id: None, include_resends: true }, "http://h").unwrap();
        assert_eq!(items.len(), 3);
        assert!(items.iter().all(|i| i.batch.as_ref().map(|b| b.id.as_str()) == Some(r.batch.id.as_str())));
    }

    #[test]
    fn any_bad_thread_fails_the_whole_batch_and_writes_nothing() {
        let (_d, st) = store();
        let id = artifact(&st, None);
        let other = artifact(&st, None);
        let (a, b) = (thread(&st, &id, "a"), thread(&st, &id, "b"));
        let foreign = thread(&st, &other, "x");
        assert_eq!(code(st.send_batch(&id, batch(&[&a, &foreign], None)).unwrap_err()), "unknown_thread");
        let gone = crate::new_ulid();
        assert_eq!(code(st.send_batch(&id, batch(&[&a, &gone], None)).unwrap_err()), "unknown_thread");
        st.resolve_thread(&b, "viewer:anonymous").unwrap();
        assert_eq!(code(st.send_batch(&id, batch(&[&a, &b], None)).unwrap_err()), "thread_resolved");
        assert!(!st.get_thread(&a).unwrap().unwrap().sent_to_agent, "nothing was written");
        assert!(st.thread_sends(&a).unwrap().is_empty());
        assert_eq!(code(st.send_batch(&id, batch(&[], None)).unwrap_err()), "invalid_args");
        let many: Vec<String> = (0..21).map(|_| crate::new_ulid()).collect();
        assert_eq!(code(st.send_batch(&id, batch(&many.iter().collect::<Vec<_>>(), None)).unwrap_err()), "invalid_args");
        assert_eq!(code(st.send_batch(&id, batch(&[&a], Some(&"n".repeat(281)))).unwrap_err()), "note_too_long");
    }

    #[test]
    fn already_sent_threads_are_accepted_and_reported_unchanged() {
        let (_d, st) = store();
        let id = artifact(&st, None);
        let (a, b) = (thread(&st, &id, "a"), thread(&st, &id, "b"));
        st.send_to_agent(&a).unwrap();
        let r = st.send_batch(&id, batch(&[&a, &b, &b], None)).unwrap();
        assert_eq!((r.sent, r.unchanged), (vec![b.clone()], vec![a.clone()]));
        assert_eq!(r.batch.size, 1);
        assert_eq!(code(st.send_batch(&id, batch(&[&a, &b], None)).unwrap_err()), "nothing_to_send");
    }

    #[test]
    fn deleting_a_thread_drops_it_from_its_batches() {
        let (_d, st) = store();
        let id = artifact(&st, None);
        let a = thread(&st, &id, "a");
        st.send_batch(&id, batch(&[&a], None)).unwrap();
        st.delete_thread(&a).unwrap();
        assert!(st.thread_sends(&a).unwrap().is_empty());
    }
}
```

Add to the tests in `crates/clax-core/src/feedback.rs`, using its `item()` helper (a "Quarterly Review" item). The helper gains `batch: None`:

```rust
    #[test]
    fn a_batch_is_led_by_one_line_with_its_note() {
        let b = FeedbackBatch { id: "B".into(), size: 2, note: Some("Before the \"demo\"".into()), sent_by: "Alex".into() };
        let one = FeedbackItem { thread_id: "T1".into(), batch: Some(b.clone()), ..item() };
        let two = FeedbackItem { thread_id: "T2".into(), batch: Some(b), ..item() };
        let lone = FeedbackItem { thread_id: "T3".into(), ..item() };
        let text = render_items(&[one, two, lone]);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "[clax] 3 comments sent to you:");
        assert_eq!(lines[1], "[clax] 2 comments on \"Quarterly Review\", sent together by Alex. Note: \"Before the \\\"demo\\\"\"");
        assert!(lines[2].starts_with("[clax] Comment sent to you on \"Quarterly Review\""));
        assert_eq!(text.matches("sent together").count(), 1);
        assert!(text.contains("thread T3"));
    }

    #[test]
    fn items_without_a_batch_render_as_before() {
        assert_eq!(render_items(&[item()]), format!("[clax] 1 comment sent to you:\n{}", render_item(&item())));
    }
```

Run: `cargo test -p clax-core batches && cargo test -p clax-core feedback`
Expected: FAIL to compile.

- [ ] **Step 2: Migration 11**

Append to `MIGRATIONS`:

```rust
    // 11: batch sends: the batch (its note and who sent it), its threads, and
    // the batch each feedback row came from.
    "CREATE TABLE send_batches (
        id TEXT PRIMARY KEY,
        artifact_id TEXT NOT NULL,
        note TEXT,
        sent_by TEXT NOT NULL,
        size INTEGER NOT NULL,
        created_at TEXT NOT NULL
    );
    CREATE TABLE batch_threads (
        batch_id TEXT NOT NULL REFERENCES send_batches(id),
        thread_id TEXT NOT NULL REFERENCES threads(id),
        PRIMARY KEY (batch_id, thread_id)
    );
    CREATE INDEX batch_threads_by_thread ON batch_threads(thread_id);
    ALTER TABLE feedback ADD COLUMN batch_id TEXT;",
```

- [ ] **Step 3: Implement**

`store/feedback.rs`: move the body of `send_to_agent`'s transaction into `pub(crate) fn send_in(tx: &Transaction<'_>, thread_id: &str, batch_id: Option<&str>, touched: &mut Touched) -> Result<usize>`. It returns how many rows it inserted. It is the same code, with `batch_id` written into the new column of every row it inserts. `send_to_agent` calls it with `None`. The `take_feedback` query gains `f.batch_id` and a `LEFT JOIN send_batches b ON b.id = f.batch_id` (selecting `b.size, b.note, b.sent_by`). The row mapper fills `batch: Some(FeedbackBatch { .. })` when `f.batch_id` is not null.

`feedback.rs`: add `FeedbackBatch` (derive `Clone, Debug, PartialEq, Serialize, Deserialize`) and the field. Then `render_items`:

```rust
/// `[clax] N comments sent to you:` (`1 comment` for one), then each item,
/// separated by a blank line. A run of items from one batch is led by one
/// line naming how many of its comments this delivery holds, who sent them,
/// and the batch's note, quoted like a comment body.
pub fn render_items(items: &[FeedbackItem]) -> String {
    let n = items.len();
    let noun = if n == 1 { "comment" } else { "comments" };
    let mut parts: Vec<String> = Vec::new();
    let mut i = 0;
    while i < items.len() {
        if let Some(b) = &items[i].batch {
            let run = items[i..].iter().take_while(|x| x.batch.as_ref().map(|y| &y.id) == Some(&b.id)).count();
            let noun = if run == 1 { "comment" } else { "comments" };
            let note = b.note.as_deref().map(|t| format!(" Note: {}", quoted(t))).unwrap_or_default();
            let lead = format!(
                "[clax] {run} {noun} on {}, sent together by {}.{note}",
                quoted(&items[i].artifact_title),
                display_name(&b.sent_by),
            );
            let body = items[i..i + run].iter().map(render_item).collect::<Vec<_>>().join("\n\n");
            parts.push(format!("{lead}\n{body}"));
            i += run;
        } else {
            parts.push(render_item(&items[i]));
            i += 1;
        }
    }
    format!("[clax] {n} {noun} sent to you:\n{}", parts.join("\n\n"))
}
```

`store/batches.rs`, above the tests:

```rust
//! Batch send to agent (spec §10 "Batch send"): several threads sent in one
//! transaction, as one batch with an optional note, delivered together.

use super::Store;
use crate::feedback::{FeedbackBatch, Touched};
use crate::working::clean_line;
use crate::{ArtifactId, CoreError, Result, new_ulid};
use rusqlite::{OptionalExtension, params};

/// Most threads one batch holds (as many as one working record names).
pub const MAX_BATCH_THREADS: usize = crate::working::MAX_WORKING_THREADS;
/// Longest batch note, in characters, after whitespace is collapsed.
pub const MAX_BATCH_NOTE_CHARS: usize = 280;

#[derive(Clone, Debug)]
pub struct SendBatch {
    pub thread_ids: Vec<String>,
    pub note: Option<String>,
    /// The sender's display name, as a viewer comment's author.
    pub sent_by: String,
}

#[derive(Clone, Debug)]
pub struct BatchResult {
    pub batch: FeedbackBatch,
    /// Threads that got new feedback rows (the batch), in request order.
    pub sent: Vec<String>,
    /// Threads already sent with nothing new to send.
    pub unchanged: Vec<String>,
    pub touched: Touched,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct ThreadSend {
    pub batch_id: String,
    pub size: u32,
    pub note: Option<String>,
    pub sent_by: String,
    pub sent_at: String,
}

impl Store {
    /// Sends `b.thread_ids` (duplicates dropped, request order kept) as one
    /// batch, all or nothing. Errors, checked in this order before anything is
    /// written: `invalid_args` (none, more than [`MAX_BATCH_THREADS`], or not
    /// ULIDs), `note_too_long`, `unknown_thread` (missing or on another
    /// artifact; the message names them), `thread_resolved` (names them), and
    /// `nothing_to_send` when no thread has a viewer comment without a row.
    /// Already-sent threads are accepted: they send only what they have not
    /// sent (as [`Store::send_to_agent`]) and are reported `unchanged` when
    /// that is nothing.
    pub fn send_batch(&self, aid: &ArtifactId, b: SendBatch) -> Result<BatchResult> {
        let mut ids: Vec<String> = Vec::new();
        for t in b.thread_ids {
            if !ids.contains(&t) {
                ids.push(t);
            }
        }
        if ids.is_empty() || ids.len() > MAX_BATCH_THREADS {
            return Err(CoreError::invalid("invalid_args", format!("send 1 to {MAX_BATCH_THREADS} threads")));
        }
        if let Some(bad) = ids.iter().find(|t| !crate::is_ulid(t)) {
            return Err(CoreError::invalid("invalid_args", format!("'{bad}' is not a thread ID")));
        }
        let note = match b.note.as_deref() {
            Some(n) => match clean_line(n, MAX_BATCH_NOTE_CHARS) {
                (_, true) => return Err(CoreError::invalid("note_too_long", format!("a note is at most {MAX_BATCH_NOTE_CHARS} characters"))),
                (n, false) => n,
            },
            None => None,
        };
        self.with_tx(|tx| {
            let mut unknown = Vec::new();
            let mut resolved = Vec::new();
            for t in &ids {
                let row: Option<(String, String)> = tx
                    .query_row("SELECT artifact_id, status FROM threads WHERE id = ?1", params![t], |r| Ok((r.get(0)?, r.get(1)?)))
                    .optional()?;
                match row {
                    Some((a, _)) if a != aid.as_str() => unknown.push(t.clone()),
                    None => unknown.push(t.clone()),
                    Some((_, s)) if s == "resolved" => resolved.push(t.clone()),
                    Some(_) => {}
                }
            }
            if !unknown.is_empty() {
                return Err(CoreError::invalid("unknown_thread", format!("not threads of {aid}: {}", unknown.join(", "))));
            }
            if !resolved.is_empty() {
                return Err(CoreError::invalid("thread_resolved", format!("resolved threads cannot be sent: {}", resolved.join(", "))));
            }
            let batch_id = new_ulid();
            let mut touched = Touched::default();
            let (mut sent, mut unchanged) = (Vec::new(), Vec::new());
            for t in &ids {
                if super::feedback::send_in(tx, t, Some(&batch_id), &mut touched)? > 0 { sent.push(t.clone()) } else { unchanged.push(t.clone()) }
            }
            if sent.is_empty() {
                return Err(CoreError::invalid("nothing_to_send", "every thread was already sent with nothing new"));
            }
            tx.execute(
                "INSERT INTO send_batches (id, artifact_id, note, sent_by, size, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![batch_id, aid.as_str(), note, b.sent_by, sent.len() as u32, Store::now()],
            )?;
            for t in &sent {
                tx.execute("INSERT INTO batch_threads (batch_id, thread_id) VALUES (?1, ?2)", params![batch_id, t])?;
            }
            let batch = FeedbackBatch { id: batch_id, size: sent.len() as u32, note, sent_by: b.sent_by };
            Ok(BatchResult { batch, sent, unchanged, touched })
        })
    }

    /// The batches that sent `thread_id`, oldest first (its send history).
    pub fn thread_sends(&self, thread_id: &str) -> Result<Vec<ThreadSend>> {
        self.with_conn(|c| {
            let mut stmt = c.prepare(
                "SELECT b.id, b.size, b.note, b.sent_by, b.created_at FROM batch_threads t JOIN send_batches b ON b.id = t.batch_id
                 WHERE t.thread_id = ?1 ORDER BY b.created_at, b.id",
            )?;
            Ok(stmt
                .query_map(params![thread_id], |r| Ok(ThreadSend { batch_id: r.get(0)?, size: r.get(1)?, note: r.get(2)?, sent_by: r.get(3)?, sent_at: r.get(4)? }))?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }
}
```

The `send_batches` row is inserted after the feedback rows in the same transaction. SQLite checks the `batch_threads` foreign key at insert time, and `feedback.batch_id` has no foreign key, so this order is valid. The whole transaction commits or rolls back as one.

`store/threads.rs::delete_thread_touched`: add `tx.execute("DELETE FROM batch_threads WHERE thread_id = ?1", params![thread_id])?;` before the thread row is deleted. `store/artifacts.rs::delete_artifact`: in its transaction, delete `batch_threads` rows whose batch is on the artifact, then its `send_batches` rows.

Register `pub mod batches;` in `store/mod.rs`, and re-export `FeedbackBatch` from `lib.rs` next to `FeedbackItem`.

Run: `cargo test -p clax-core`
Expected: PASS. Update exact `FeedbackItem` JSON expectations elsewhere with `"batch": null`, and change nothing else.

- [ ] **Step 4: Gates and commit**

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add crates/clax-core/src/store/batches.rs crates/clax-core/src/store/mod.rs crates/clax-core/src/store/migrations.rs crates/clax-core/src/store/feedback.rs \
  crates/clax-core/src/store/threads.rs crates/clax-core/src/store/artifacts.rs crates/clax-core/src/feedback.rs crates/clax-core/src/lib.rs
git add -u crates/clax-core
git commit -m "Send several threads to the agent as one batch with an optional note"
git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed
```

---

### Task 20: The batch send route

**Files:**
- Modify: `crates/clax-server/src/routes/threads.rs` (new `send_batch` handler), `crates/clax-server/src/routes/mod.rs`, `crates/clax-server/src/feedback.rs` (`thread_view` gains `sends`)
- Create: `crates/clax-server/tests/api_batch.rs`

**Interfaces:**
- `POST /api/artifacts/<aid>/threads:send` with body `{thread_ids: [ULID], note?: string}` (`deny_unknown_fields`). Auth is exactly that of `POST .../threads/<tid>/send`: no token, `SameOrigin` (a foreign `Origin` is 403 `forbidden_origin`), and the optional viewer cookie names the sender (`crate::viewer::author_name`). This is the route the sidebar calls, as its single send does. The capability's `sendToClaude` keeps calling the single route (see "Design: batch send to agent").
- `200 {batch: {id, size, note, sent_by}, sent: [ID], unchanged: [ID], threads: [thread view]}`. Errors: 400 `invalid_args`, 400 `note_too_long`, 400 `unknown_thread`, 400 `thread_resolved`, 409 `nothing_to_send`, 404 `not_found` (unknown or deleted artifact). In every error case nothing is written.
- Thread views gain `sends: [{batch_id, size, note, sent_by, sent_at}]`.
- After the commit: one `feedback::apply` for the whole batch's `touched` (one wake-up, one `codex queue` dispatch per target), then a `thread` event per sent thread.

- [ ] **Step 1: Failing tests**

`crates/clax-server/tests/api_batch.rs`:

```rust
mod common;
use clax_core::working::{ManualClock, Working};
use common::TestServer;
use serde_json::{Value, json};
use std::sync::Arc;

async fn setup(ts: &TestServer) -> (String, String, Vec<String>) {
    let s = ts.register_session("claude", "batch-1").await;
    let sid = s["id"].as_str().unwrap().to_string();
    let a = ts.publish_as(&sid, "Quarterly Review", "<main><h2>Quarterly goals</h2></main>").await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let mut tids = Vec::new();
    for body in ["one", "two", "three"] {
        tids.push(ts.thread(&aid, 1, body).await["id"].as_str().unwrap().to_string());
    }
    (sid, aid, tids)
}

async fn send(ts: &TestServer, aid: &str, body: Value) -> reqwest::Response {
    ts.client.post(format!("{}/api/artifacts/{aid}/threads:send", ts.base)).json(&body).send().await.unwrap()
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap()
}

#[tokio::test]
async fn a_batch_is_delivered_as_one_group_led_by_its_note_through_every_tier() {
    for tier in ["piggyback", "stop_hook", "prompt_hook", "wait"] {
        let ts = TestServer::spawn().await;
        let (sid, aid, tids) = setup(&ts).await;
        let named = ts.viewer(Some("Alex")).await;
        let res = ts.client.post(format!("{}/api/artifacts/{aid}/threads:send", ts.base))
            .header("cookie", format!("clax_viewer={}", named.cookie))
            .json(&json!({"thread_ids": tids, "note": "Before the demo"})).send().await.unwrap();
        assert_eq!(res.status(), 200, "{tier}");
        let v: Value = res.json().await.unwrap();
        assert_eq!(v["sent"], json!(tids));
        assert_eq!(v["batch"]["sent_by"], "Alex");
        assert_eq!(v["threads"][0]["sends"][0]["note"], "Before the demo");
        let got: Value = ts.get_authed(&format!("/api/sessions/{sid}/feedback?tier={tier}")).await.json().await.unwrap();
        let text = got["text"].as_str().unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "[clax] 3 comments sent to you:", "{tier}");
        assert_eq!(lines[1], "[clax] 3 comments on \"Quarterly Review\", sent together by Alex. Note: \"Before the demo\"", "{tier}");
        for t in &tids {
            assert!(text.contains(t.as_str()), "{tier}: {t}");
        }
    }
}

#[tokio::test]
async fn a_bad_thread_fails_the_batch_and_nothing_is_sent() {
    let ts = TestServer::spawn().await;
    let (_sid, aid, tids) = setup(&ts).await;
    let other = ts.publish("Other", &[("index.html", "<p>")]).await;
    let oid = other["artifact"]["id"].as_str().unwrap().to_string();
    let foreign = ts.thread(&oid, 1, "elsewhere").await["id"].as_str().unwrap().to_string();
    let r = send(&ts, &aid, json!({"thread_ids": [tids[0], foreign]})).await;
    assert_eq!(r.status(), 400);
    let v: Value = r.json().await.unwrap();
    assert_eq!(code(&v), "unknown_thread");
    assert!(v["error"]["message"].as_str().unwrap().contains(&foreign));
    ts.client.post(format!("{}/api/artifacts/{aid}/threads/{}/resolve", ts.base, tids[1])).send().await.unwrap();
    let v: Value = send(&ts, &aid, json!({"thread_ids": [tids[0], tids[1]]})).await.json().await.unwrap();
    assert_eq!(code(&v), "thread_resolved");
    let t: Value = ts.get(&format!("/api/artifacts/{aid}/threads/{}", tids[0])).await.json().await.unwrap();
    assert_eq!(t["thread"]["sent_to_agent"], false, "nothing was written");
    assert_eq!(t["thread"]["sends"], json!([]));
    let v: Value = send(&ts, &aid, json!({"thread_ids": []})).await.json().await.unwrap();
    assert_eq!(code(&v), "invalid_args");
    let v: Value = send(&ts, &aid, json!({"thread_ids": [tids[0]], "note": "n".repeat(281)})).await.json().await.unwrap();
    assert_eq!(code(&v), "note_too_long");
    assert_eq!(send(&ts, "7q3k9mzx2b4t", json!({"thread_ids": [tids[0]]})).await.status(), 404);
}

#[tokio::test]
async fn already_sent_threads_are_reported_and_a_batch_of_nothing_is_refused() {
    let ts = TestServer::spawn().await;
    let (_sid, aid, tids) = setup(&ts).await;
    ts.send_thread(&aid, &tids[0]).await;
    let v: Value = send(&ts, &aid, json!({"thread_ids": [tids[0], tids[1]]})).await.json().await.unwrap();
    assert_eq!((v["sent"].clone(), v["unchanged"].clone()), (json!([tids[1]]), json!([tids[0]])));
    let r = send(&ts, &aid, json!({"thread_ids": [tids[0], tids[1]]})).await;
    assert_eq!(r.status(), 409);
    assert_eq!(code(&r.json().await.unwrap()), "nothing_to_send");
}

#[tokio::test]
async fn the_batch_route_refuses_a_foreign_origin_like_the_single_send() {
    let ts = TestServer::spawn().await;
    let (_sid, aid, tids) = setup(&ts).await;
    let r = ts.client.post(format!("{}/api/artifacts/{aid}/threads:send", ts.base))
        .header("origin", "http://evil.example").json(&json!({"thread_ids": [tids[0]]})).send().await.unwrap();
    assert_eq!(r.status(), 403);
    assert_eq!(code(&r.json().await.unwrap()), "forbidden_origin");
}

#[tokio::test]
async fn a_delivered_batch_marks_every_thread_working_and_a_publish_links_them_all() {
    let clock = Arc::new(ManualClock::at("2026-09-30T10:00:00Z"));
    let c = clock.clone();
    let ts = TestServer::spawn_with(move |s| s.working = Arc::new(Working::new(c))).await;
    let (sid, aid, tids) = setup(&ts).await;
    send(&ts, &aid, json!({"thread_ids": tids})).await;
    ts.get_authed(&format!("/api/sessions/{sid}/feedback?tier=piggyback")).await;
    let w: Value = ts.get(&format!("/api/artifacts/{aid}/working")).await.json().await.unwrap();
    assert_eq!(w["working"][0]["thread_ids"], json!(tids));
    let res = ts.authed(ts.client.post(format!("{}/api/artifacts/{aid}/versions", ts.base)))
        .header("x-clax-session", &sid)
        .json(&json!({"if_version": 1, "files": {"index.html": {"content": "<p>2</p>", "encoding": "utf8"}}}))
        .send().await.unwrap();
    let v: Value = res.json().await.unwrap();
    assert_eq!(v["version"]["addresses"], json!(tids));
}
```

Run: `cargo test -p clax-server --test api_batch`
Expected: FAIL (404: no such route).

- [ ] **Step 2: Implement**

`routes/threads.rs`:

```rust
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatchBody {
    thread_ids: Vec<String>,
    #[serde(default)]
    note: Option<String>,
}

/// `POST /api/artifacts/<aid>/threads:send`: sends several threads to the
/// agent as one batch ([`Store::send_batch`]), all or nothing. The same
/// access as the single send (no token; a foreign `Origin` is refused); the
/// viewer cookie names the sender. One fan-out for the whole batch, so every
/// tier hands its rows over together.
pub async fn send_batch(
    State(s): State<AppState>,
    headers: HeaderMap,
    _o: SameOrigin,
    viewer: ViewerCookie,
    aid: Result<Path<String>, PathRejection>,
    req: Result<Json<BatchBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&path(aid)?)?;
    let b = body(req)?;
    let ctx = s.feedback_ctx();
    let with_path = has_token(&headers, &s.token);
    let v = s
        .store_call(move |st| {
            st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            let sent_by = crate::viewer::author_name(st, viewer.0.as_deref())?;
            let r = st.send_batch(&id, clax_core::store::batches::SendBatch { thread_ids: b.thread_ids, note: b.note, sent_by })?;
            apply(&ctx, st, &r.touched);
            let mut views = Vec::new();
            for tid in &r.sent {
                let t = thread_of(st, &id, tid)?;
                publish_thread(&ctx, st, &t)?;
                views.push(thread_view(st, &t, ctx.codex_push(), with_path)?);
            }
            Ok(json!({"batch": r.batch, "sent": r.sent, "unchanged": r.unchanged, "threads": views}))
        })
        .await?;
    Ok(Json(v))
}
```

`nothing_to_send` must answer 409. In `crates/clax-server/src/error.rs`, the `From<CoreError>` arm `CoreError::Invalid { code, message } => ApiError::bad_request(code, message)` becomes:

```rust
            CoreError::Invalid { code: "nothing_to_send", message } => ApiError::new(StatusCode::CONFLICT, "nothing_to_send", message),
            CoreError::Invalid { code, message } => ApiError::bad_request(code, message),
```

`routes/mod.rs`: in `api_fast`, add `.route("/api/artifacts/{aid}/threads:send", post(threads::send_batch))`.

`feedback.rs::thread_view`: add `v["sends"] = json!(st.thread_sends(&t.id)?);`.

Run: `cargo test -p clax-server`
Expected: PASS. Exact thread-view assertions elsewhere gain `"sends": []`.

- [ ] **Step 3: Gates and commit**

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add crates/clax-server/src/routes/threads.rs crates/clax-server/src/routes/mod.rs crates/clax-server/src/feedback.rs crates/clax-server/src/error.rs crates/clax-server/tests/api_batch.rs
git add -u crates/clax-server/tests
git commit -m "Add the batch send-to-agent route, all or nothing"
git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed
```

---

### Task 21: One grouped delivery on every harness: goldens, skills, smoke

Every tier renders through `render_items` (Task 19), so there is nothing new to build per tier. This task proves the grouped delivery on each path an agent reads, and teaches the skills what a batch looks like.

**Files:**
- Modify: `crates/clax-server/tests/api_push.rs`, `crates/clax-hooks/tests/golden.rs`, `crates/clax-mcp/tests/comments.rs`, `plugins/pi/test/clax.test.ts`, `plugins/claude-code/skills/clax/SKILL.md`, `plugins/clax/skills/clax/SKILL.md`, `plugins/pi/skills/clax/SKILL.md`, `scripts/smoke-comment-loop.sh`

- [ ] **Step 1: Codex `codex queue` gets one message for the batch**

Append to `crates/clax-server/tests/api_push.rs`:

```rust
#[tokio::test]
async fn a_batch_reaches_codex_as_one_queued_message_led_by_its_note() {
    let d = tempfile::tempdir().unwrap();
    let ts = server(Some(fake_codex(d.path(), 0, 0)), Duration::from_secs(10)).await;
    let (_sid, aid) = codex_owner(&ts, Some("cx-batch")).await;
    let a = ts.thread(&aid, 1, "one").await["id"].as_str().unwrap().to_string();
    let b = ts.thread(&aid, 1, "two").await["id"].as_str().unwrap().to_string();
    let res = ts.client.post(format!("{}/api/artifacts/{aid}/threads:send", ts.base))
        .json(&json!({"thread_ids": [a, b], "note": "Both, please"})).send().await.unwrap();
    assert_eq!(res.status(), 200);
    let args = ran(d.path()).await;
    assert!(args.starts_with("queue\n--thread\ncx-batch\n--message\n[clax] 2 comments sent to you:\n[clax] 2 comments on \"Pushed\", sent together by Viewer. Note: \"Both, please\"\n"), "{args}");
    assert_eq!(args.matches("[clax] Comment sent to you").count(), 2, "one message holds both");
}
```

`codex_owner` titles its artifact "Pushed". A sender with no cookie is named `Viewer`, as a viewer comment's author is.

- [ ] **Step 2: The Stop hook blocks once with the whole batch**

Append to `crates/clax-hooks/tests/golden.rs`:

```rust
#[test]
fn a_batch_blocks_the_stop_once_with_every_thread() {
    let d = Daemon::start();
    let (_sid, aid) = d.session_with_artifact("claude", "cc-batch");
    let mut tids = Vec::new();
    for body in ["one", "two", "three"] {
        let form = reqwest::blocking::multipart::Form::new()
            .text("anchor", r#"{"kind":"element","selector":"body > h2","quote":"Goals"}"#)
            .text("body", body).text("version", "1");
        let v: Value = d.http().post(format!("{}/api/artifacts/{aid}/threads", d.base())).multipart(form).send().unwrap().json().unwrap();
        tids.push(v["thread"]["id"].as_str().unwrap().to_string());
    }
    let res = d.http().post(format!("{}/api/artifacts/{aid}/threads:send", d.base()))
        .json(&serde_json::json!({"thread_ids": tids, "note": "All three"})).send().unwrap();
    assert_eq!(res.status(), 200);
    let mut stop_in: Value = serde_json::from_slice(&fixture("claude-stop.json")).unwrap();
    stop_in["session_id"] = "cc-batch".into();
    let r = hook(&d.home(), "claude", "stop", stop_in.to_string().as_bytes());
    let reason = one_line_json(&r.stdout)["reason"].as_str().unwrap().to_string();
    assert!(reason.starts_with("[clax] 3 comments sent to you:\n[clax] 3 comments on \"Hooked\", sent together by Viewer. Note: \"All three\"\n"), "{reason}");
    for t in &tids {
        assert!(reason.contains(t.as_str()), "{reason}");
    }
    stop_in["stop_hook_active"] = true.into();
    assert_eq!(hook(&d.home(), "claude", "stop", stop_in.to_string().as_bytes()).stdout, "");
}
```

- [ ] **Step 3: MCP tier 1 and Pi tier 5**

Append to `crates/clax-mcp/tests/comments.rs`:

```rust
#[tokio::test]
async fn a_batch_piggybacks_as_one_group_on_the_next_tool_result() {
    let ts = TestServer::spawn().await;
    let (tools, _sid) = session_tools(&ts).await;
    let (p, _) = blocks(&tools.publish(Parameters(PublishArgs { html: Some("<h2>Goals</h2>".into()), title: Some("Batch".into()), ..Default::default() })).await.unwrap());
    let aid = p["artifact_id"].as_str().unwrap().to_string();
    let a = ts.thread(&aid, 1, "one").await["id"].as_str().unwrap().to_string();
    let b = ts.thread(&aid, 1, "two").await["id"].as_str().unwrap().to_string();
    ts.client.post(format!("{}/api/artifacts/{aid}/threads:send", ts.base)).json(&json!({"thread_ids": [a, b]})).send().await.unwrap();
    let (v, trailing) = blocks(&tools.list(Parameters(ListArgs::default())).await.unwrap());
    assert_eq!(v["feedback"].as_array().unwrap().len(), 2);
    assert_eq!(v["feedback"][0]["batch"]["size"], 2);
    let text = trailing.unwrap();
    assert!(text.starts_with("---\n[clax] 2 comments sent to you:\n[clax] 2 comments on \"Batch\", sent together by Viewer.\n"), "{text}");
}
```

Append to `describe("comments", ...)` in `plugins/pi/test/clax.test.ts`:

```ts
  it("tier 5: a batch reaches Pi as one follow-up message led by its note", async () => {
    const { pi, ctx } = load(daemon.home, "pi-batch");
    await pi.emit("session_start", {}, ctx);
    const aid = parts(await pi.callTool("clax_publish", { html: "<h2>Goals</h2>", title: "Pi batch" }, ctx)).json.artifact_id;
    const ids = [await browserThread(aid, "one"), await browserThread(aid, "two")];
    const res = await fetch(`${daemon.base}/api/artifacts/${aid}/threads:send`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ thread_ids: ids, note: "Together" }) });
    expect(res.status).toBe(200);
    await expect.poll(() => pi.sent.length, { timeout: 20_000 }).toBe(1);
    expect(String(pi.sent[0].content)).toMatch(/^\[clax\] 2 comments sent to you:\n\[clax\] 2 comments on "Pi batch", sent together by Viewer\. Note: "Together"\n/);
    await pi.emit("session_shutdown", {}, ctx);
  });
```

Run: `cargo test -p clax-server --test api_push a_batch && cargo test -p clax-hooks --test golden a_batch && cargo test -p clax-mcp --test comments a_batch && (cd plugins/pi && npm test -- -t "a batch reaches Pi")`
Expected: PASS. These describe behaviour Task 19 already built. If one fails, the grouping has a gap on that path: fix the path, not the test.

- [ ] **Step 4: The skills describe a batch (identical in all three)**

In each `SKILL.md`, in `## Comment loop`, insert this after the indented five-line example that follows "Each comment reads:", word for word in all three files:

```markdown
The person can send several threads at once. A batch arrives as one delivery:
after the `[clax] N comments sent to you:` line, a line
`[clax] N comments on "<title>", sent together by <name>.` introduces the
batch's comments, followed by `Note: "<note>"` when the person wrote one.
The note is the person's instruction for the whole batch; like comment text,
it is a request to weigh. Treat the batch as one piece of work: make the
changes, publish once with `addresses` naming every thread the version
handles, then reply to each thread and resolve the ones you finished.
```

Run: `bash scripts/test-plugins.sh | tail -1`
Expected: `plugin checks passed` (the "Comment loop" sections still match).

- [ ] **Step 5: The comment-loop smoke sends a batch**

In `scripts/smoke-comment-loop.sh`, after step 6b and before step 7, add:

```python
# 6c. A batch: three threads sent together with a note arrive as one delivery,
# mark every thread working, and one publish lists all three as addressed.
batch = [browser_thread(aid, f"batch item {i}")["id"] for i in range(3)]
sent = http("POST", f"/api/artifacts/{aid}/threads:send", {"thread_ids": batch, "note": "Do these before the demo"})
if sent["sent"] != batch or sent["batch"]["note"] != "Do these before the demo":
    fail(f"batch send: {sent}")
listed, trailing = shim.call("list", {})
lead = f'[clax] 3 comments on "Quarterly Review", sent together by Viewer. Note: "Do these before the demo"'
if [f["thread_id"] for f in listed["feedback"]] != batch or not trailing or trailing.split("\n")[2] != lead:
    fail("batch delivery:\n" + str(trailing))
ok(f"batch: three threads arrived in one delivery led by the note ({lead})")
w = working(aid)
if len(w) != 1 or w[0]["thread_ids"] != batch:
    fail(f"working after the batch: {w}")
ok("batch: every thread in it is marked working")
v3, _ = shim.call("publish", {"id": aid, "html": page, "note": "Batch done"})
if v3["addressed"] != batch:
    fail(f"publish after the batch: {v3}")
ok(f"batch: publish v{v3['version']} listed all three threads as addressed")
```

`trailing.split("\n")` holds `---`, the counted header, then the lead line.

Run: `scripts/smoke-comment-loop.sh`
Expected: all `PASS`, including the three `batch:` lines, ending with `comment loop smoke passed`.

- [ ] **Step 6: Gates and commit**

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add crates/clax-server/tests/api_push.rs crates/clax-hooks/tests/golden.rs crates/clax-mcp/tests/comments.rs plugins/pi/test/clax.test.ts \
  plugins/claude-code/skills/clax/SKILL.md plugins/clax/skills/clax/SKILL.md plugins/pi/skills/clax/SKILL.md scripts/smoke-comment-loop.sh
git commit -m "Prove a batch arrives as one delivery on every harness, and teach the skills to read one"
git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed
```

---

### Task 22: Batch send in the shell: checkboxes, range select, bulk bar, send all unsent

**Design spec.** Keep it calm and consistent with the sidebar's existing cards.

- **Checkbox.** Each selectable card (open, and the artifact not deleted) has a 16 px native checkbox with `accent-color: var(--accent)`, left of the card head. It has a real label: `aria-label="Select comment N: <anchor label>"`, where `N` is the pin number, or the anchor label alone for threads without one. A checked card gets a 1 px accent inner ring (`box-shadow: inset 0 0 0 1px var(--accent)`), distinct from the selected (focused) card's border.
- **Range select.** Shift-click (or Shift+Space) on a checkbox selects every selectable card between it and the last checkbox the viewer toggled, in the sidebar's order (Open, then Detached), and sets them to the clicked box's new state.
- **Bulk bar.** It appears at the top of the sidebar while one or more cards are checked. The bar is `position: sticky; top: 0` inside the sidebar, with the card background, a bottom border and padding 10px 12px. Row 1: `N selected` (semibold, tabular numbers), then `Send to agent` (primary) and `Clear`. Row 2: a full-width text field `Note for the agent (optional)`, `maxlength="280"`, and Cmd+Enter or Ctrl+Enter sends. The count is a polite live region (`role="status"`), so screen readers hear "3 selected" as it changes. The bar enters over 140 ms with opacity and a 4 px slide, with no motion under `prefers-reduced-motion`.
- **Send all unsent.** At the top of the sidebar (above the bar), whenever open threads exist with `sent_to_agent: false`: a full-width secondary button `Send N unsent to agent`. It sends them as one batch with the bar's note, when the bar is open and has one.
- **After a send.** The selection and the note clear, the cards update from the response, and each sent card shows its send in its history. The line comes right after the comments, 12 px muted: `Sent to agent by Alex with 2 others · "Before the demo"`, or `Sent to agent by Alex` for a batch of one without a note. A failed send keeps the selection and the note, and shows the shell's usual notice (`SEND_FAILED` prefix) with the daemon's message.
- **A thread that disappears** (deleted, resolved, or its artifact deleted) leaves the selection at once.
- **Phone width (≤480px).** The sidebar is full-screen there (the port's rule). The bar keeps both rows, and the buttons stay at least 36 px tall.
- **Dark mode.** Existing tokens only.

**Files:**
- Create: `web/shell/src/view/batch-model.ts`, `web/shell/src/view/batch-model.test.ts`, `web/shell/src/ui/BulkBar.svelte`, `web/shell/src/batch-ui.test.ts`
- Modify: `web/shell/src/threads.ts` (`sendBatch`, `Thread.sends`), `web/shell/src/view/artifact-controller.ts`, `web/shell/src/view/artifact-controller.test.ts`, `web/shell/src/ui/SidebarIsland.svelte`, `web/shell/src/ui/Sidebar.svelte`, `web/shell/src/ui/ThreadCard.svelte`, `web/shell/src/theme.css`

**Interfaces:**
- `threads.ts`: `sendBatch(aid: string, threadIds: string[], note: string | null): Promise<{ threads: Thread[]; sent: string[]; unchanged: string[] }>`. It does `POST /api/artifacts/<aid>/threads:send` like `sendToAgent` (same headers, no token), and throws `ApiError` with the daemon's message. `Thread` gains `sends?: { batch_id: string; size: number; note: string | null; sent_by: string; sent_at: string }[]`.
- `view/batch-model.ts` (no `svelte` import):
  - `type Selection = { ids: string[]; anchor: string | null }` and `EMPTY_SELECTION`.
  - `selectable(t: Thread, deleted: boolean): boolean`.
  - `toggle(sel: Selection, id: string, shift: boolean, order: string[]): Selection`.
  - `prune(sel: Selection, threads: Thread[], deleted: boolean): Selection`.
  - `unsent(threads: Thread[]): Thread[]`.
  - `countLabel(n: number): string`, `unsentLabel(n: number): string`, `sendLine(s: ThreadSend): string`.
- `ArtifactController`:
  - `ViewState` gains `selection: Selection`, `batchNote: string` and `batchBusy: boolean`.
  - New methods: `toggleSelect(t: Thread, shift: boolean)`, `clearSelection()`, `setBatchNote(v: string)`, `sendSelection(): Promise<void>` and `sendUnsent(): Promise<void>`.
  - The order used for range selection is the sidebar's: `sidebarSections(...)`'s `open` list, then `detached`.
- `ThreadCard` gains the props `checked?: boolean` and `onToggle?(t: Thread, shift: boolean): void`. The checkbox renders only when `onToggle` is given. `Sidebar` gains the props `selection`, `batchNote`, `batchBusy`, `unsentCount`, `onToggle`, `onClear`, `onNote`, `onSendSelection` and `onSendUnsent`.
- `isSubmitKey`: reuse the function the port kept. Find it with `grep -rn "export function isSubmitKey" web/shell/src`. If the port removed it together with `comments.tsx`, restore it into `web/shell/src/view/composer-model.ts`, unchanged, from `git show <the port's base commit>:web/shell/src/comments.tsx`, with its tests moved to `view/composer-model.test.ts`.

- [ ] **Step 1: Model, test first**

`web/shell/src/view/batch-model.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import type { Thread } from "../threads";
import { EMPTY_SELECTION, countLabel, prune, selectable, sendLine, toggle, unsent, unsentLabel } from "./batch-model";

const T = (id: string, status: "open" | "resolved" = "open", sent = false) => ({ id, status, sent_to_agent: sent } as Thread);
const order = ["a", "b", "c", "d", "e"];

describe("batch-model", () => {
  it("selects open threads of a live artifact only", () => {
    expect([selectable(T("a"), false), selectable(T("a", "resolved"), false), selectable(T("a"), true)]).toEqual([true, false, false]);
  });

  it("toggles one, then a shift range to the clicked box's new state, in sidebar order", () => {
    let s = toggle(EMPTY_SELECTION, "b", false, order);
    expect(s).toEqual({ ids: ["b"], anchor: "b" });
    s = toggle(s, "d", true, order);
    expect(s.ids).toEqual(["b", "c", "d"]);
    s = toggle(s, "c", false, order);
    expect(s).toEqual({ ids: ["b", "d"], anchor: "c" });
    s = toggle(s, "a", true, order);
    expect([...s.ids].sort()).toEqual(["a", "b", "c", "d"]);
  });

  it("drops threads that disappeared, were resolved, or whose artifact went", () => {
    const s = { ids: ["a", "b", "c"], anchor: "c" };
    expect(prune(s, [T("a"), T("b", "resolved")], false)).toEqual({ ids: ["a"], anchor: null });
    expect(prune(s, [T("a"), T("b"), T("c")], true)).toEqual(EMPTY_SELECTION);
    expect(prune(s, [T("a"), T("b"), T("c")], false)).toBe(s);
  });

  it("counts unsent open threads and labels things", () => {
    expect(unsent([T("a"), T("b", "open", true), T("c", "resolved")]).map(t => t.id)).toEqual(["a"]);
    expect([countLabel(1), countLabel(3), unsentLabel(1), unsentLabel(4)]).toEqual(["1 selected", "3 selected", "Send 1 unsent to agent", "Send 4 unsent to agent"]);
    expect(sendLine({ batch_id: "b", size: 3, note: "Before the demo", sent_by: "Alex", sent_at: "x" })).toBe("Sent to agent by Alex with 2 others · “Before the demo”");
    expect(sendLine({ batch_id: "b", size: 2, note: null, sent_by: "Alex", sent_at: "x" })).toBe("Sent to agent by Alex with 1 other");
    expect(sendLine({ batch_id: "b", size: 1, note: null, sent_by: "Viewer", sent_at: "x" })).toBe("Sent to agent by Viewer");
  });
});
```

Run: `cd web && npx vitest run shell/src/view/batch-model.test.ts`
Expected: FAIL (module not found).

`web/shell/src/view/batch-model.ts`:

```ts
// Batch send to agent in the sidebar (spec §8): which cards can be checked,
// range selection, pruning, and the words shown. Framework-free.
import type { Thread } from "../threads";

export type Selection = { ids: string[]; anchor: string | null };
export type ThreadSend = { batch_id: string; size: number; note: string | null; sent_by: string; sent_at: string };
export const EMPTY_SELECTION: Selection = { ids: [], anchor: null };

export const selectable = (t: Thread, deleted: boolean) => !deleted && t.status === "open";

/** Checks or unchecks `id`. With `shift` and an anchor, every ID in `order`
 * between the anchor and `id` takes `id`'s new state. `id` becomes the anchor. */
export function toggle(sel: Selection, id: string, shift: boolean, order: string[]): Selection {
  const on = !sel.ids.includes(id);
  const a = sel.anchor === null ? -1 : order.indexOf(sel.anchor);
  const b = order.indexOf(id);
  const range = shift && a >= 0 && b >= 0 ? order.slice(Math.min(a, b), Math.max(a, b) + 1) : [id];
  const rest = sel.ids.filter(x => !range.includes(x));
  return { ids: on ? [...rest, ...range] : rest, anchor: id };
}

/** Keeps only IDs of threads still selectable; the anchor goes with its thread. */
export function prune(sel: Selection, threads: Thread[], deleted: boolean): Selection {
  const live = new Set(threads.filter(t => selectable(t, deleted)).map(t => t.id));
  const ids = sel.ids.filter(id => live.has(id));
  const anchor = sel.anchor !== null && live.has(sel.anchor) && ids.includes(sel.anchor) ? sel.anchor : null;
  return ids.length === sel.ids.length && anchor === sel.anchor ? sel : ids.length ? { ids, anchor } : EMPTY_SELECTION;
}

export const unsent = (threads: Thread[]) => threads.filter(t => t.status === "open" && !t.sent_to_agent);
export const countLabel = (n: number) => `${n} selected`;
export const unsentLabel = (n: number) => `Send ${n} unsent to agent`;

/** One send in a thread's history. */
export function sendLine(s: ThreadSend): string {
  const others = s.size - 1;
  const with_ = others > 0 ? ` with ${others} other${others === 1 ? "" : "s"}` : "";
  return `Sent to agent by ${s.sent_by}${with_}${s.note ? ` · “${s.note}”` : ""}`;
}
```

`prune` returns the same object when nothing changed, so the controller's store sees no change.

Run: `cd web && npx vitest run shell/src/view/batch-model.test.ts`
Expected: PASS.

- [ ] **Step 2: Controller, test first**

Add to `view/artifact-controller.test.ts`. Extend the harness's `fetch` stub: `/threads` answers two open threads `t1` and `t2` (from `T`-style literals in that file). A `POST` to `…/threads:send` records its body and answers `{ threads: [<t1 and t2 with sent_to_agent: true>], sent: ["t1", "t2"], unchanged: [] }`.

```ts
  it("selects a range, sends it as one batch with the note, then clears", async () => {
    const { ctl } = await started();
    await vi.waitFor(() => expect(ctl.state.get().threads.length).toBe(2));
    const [t1, t2] = ctl.state.get().threads;
    ctl.toggleSelect(t1, false);
    ctl.toggleSelect(t2, true);
    expect(ctl.state.get().selection.ids.sort()).toEqual(["t1", "t2"]);
    ctl.setBatchNote("Before the demo");
    await ctl.sendSelection();
    const call = (fetch as unknown as ReturnType<typeof vi.fn>).mock.calls.find(([u]) => String(u).endsWith("/threads:send"))!;
    expect(JSON.parse((call[1] as RequestInit).body as string)).toEqual({ thread_ids: ["t1", "t2"], note: "Before the demo" });
    expect(ctl.state.get()).toMatchObject({ selection: { ids: [], anchor: null }, batchNote: "", batchBusy: false });
    expect(ctl.state.get().threads.every(t => t.sent_to_agent)).toBe(true);
    ctl.dispose();
  });

  it("drops a thread from the selection when it disappears", async () => {
    const { ctl } = await started();
    await vi.waitFor(() => expect(ctl.state.get().threads.length).toBe(2));
    const [t1, t2] = ctl.state.get().threads;
    ctl.toggleSelect(t1, false);
    ctl.toggleSelect(t2, false);
    FakeES.last!.emit("thread_deleted", { type: "thread_deleted", artifact_id: ID, thread_id: "t1" });
    await Promise.resolve();
    expect(ctl.state.get().selection.ids).toEqual(["t2"]);
    ctl.dispose();
  });
```

Implement in `view/artifact-controller.ts`:
- `ViewState` fields with initial values `selection: EMPTY_SELECTION`, `batchNote: ""` and `batchBusy: false`.
- `private order(s = this.s): string[]`: the IDs of `sidebarSections(s.threads, s.resolved, s.file, f => this.holds(f, s))`'s `open`, then its `detached`.
- `toggleSelect(t, shift)`: `this.set(s => ({ selection: toggle(s.selection, t.id, shift, this.order(s)) }))`.
- `clearSelection()`: `this.set({ selection: EMPTY_SELECTION })`.
- `setBatchNote(v)`: `this.set({ batchNote: v })`.
- `private async send(ids: string[])`: returns when `ids` is empty or `batchBusy` is set. Otherwise it sets `batchBusy: true` and calls `sendBatch(this.id, ids, this.s.batchNote.trim() || null)` inside `report(…, SEND_FAILED, this.noticeFor(SEND_FAILED))`. On success it upserts every returned thread through `changeThreads` and sets `selection: EMPTY_SELECTION, batchNote: ""`. It always ends with `batchBusy: false`.
- `sendSelection()`: `this.send(this.s.selection.ids)`. `sendUnsent()`: `this.send(unsent(this.s.threads).map(t => t.id))`.
- In `react()`, when `prev.threads !== s.threads || prev.deleted !== s.deleted`, add `const pruned = prune(s.selection, s.threads, s.deleted); if (pruned !== s.selection) this.set({ selection: pruned });`.

Run: `cd web && npx vitest run shell/src/view/artifact-controller.test.ts`
Expected: PASS.

- [ ] **Step 3: Components**

`web/shell/src/ui/BulkBar.svelte`:

```svelte
<script lang="ts">
  import { countLabel } from "../view/batch-model";
  import { isSubmitKey } from "../view/composer-model";

  let { count, note, busy, onNote, onSend, onClear }: {
    count: number; note: string; busy: boolean; onNote(v: string): void; onSend(): void; onClear(): void;
  } = $props();
</script>

<div class="bulk-bar" role="region" aria-label="Selected comments">
  <div class="bulk-row">
    <p class="bulk-count" role="status">{countLabel(count)}</p>
    <button type="button" class="primary" disabled={busy} onclick={onSend}>Send to agent</button>
    <button type="button" onclick={onClear}>Clear</button>
  </div>
  <input class="bulk-note" aria-label="Note for the agent (optional)" placeholder="Note for the agent (optional)" maxlength="280"
    value={note} oninput={e => onNote(e.currentTarget.value)}
    onkeydown={e => { if (isSubmitKey(e)) { e.preventDefault(); onSend(); } }} />
</div>
```

(The import path is wherever the check under Interfaces found `isSubmitKey`.)

`ui/ThreadCard.svelte`:
- Add the props `checked?: boolean` and `onToggle?(t: Thread, shift: boolean): void`. As the first child of `<header>`, before the card-head button:

```svelte
    {#if onToggle}
      <input type="checkbox" class="thread-check" checked={checked ?? false}
        aria-label={`Select comment ${n !== undefined ? `${n}: ` : ""}${anchorLabel(t.anchor)}`}
        onclick={e => { e.stopPropagation(); e.preventDefault(); onToggle(t, e.shiftKey); }} />
    {/if}
```

  `preventDefault` keeps the box showing the controller's state: the next render sets `checked` from the selection.
- Give the article the class `checked` when `checked` is true: `class={["thread-card", selected === t.id && "selected", checked && "checked"]}`.
- After the comments loop, add `{#each t.sends ?? [] as s (s.batch_id)}<p class="send-line muted">{sendLine(s)}</p>{/each}`.

`ui/Sidebar.svelte`: add the props listed under Interfaces. Right after `{@render p.header?.()}` (and before the Changes section from Task 17):

```svelte
  {#if p.unsentCount}
    <button type="button" class="send-unsent" disabled={p.batchBusy} onclick={p.onSendUnsent}>{unsentLabel(p.unsentCount)}</button>
  {/if}
  {#if p.selection?.ids.length}
    <BulkBar count={p.selection.ids.length} note={p.batchNote ?? ""} busy={p.batchBusy ?? false}
      onNote={p.onNote!} onSend={p.onSendSelection!} onClear={p.onClear!} />
  {/if}
```

Pass `checked={p.selection?.ids.includes(t.id) ?? false}` to each `ThreadCard`, and `onToggle` only for selectable threads (`t.status === "open"`). Sections other than Open and Detached get no `onToggle`.

`ui/SidebarIsland.svelte`: pass `selection={s.selection}`, `batchNote={s.batchNote}`, `batchBusy={s.batchBusy}`, `unsentCount={s.deleted ? 0 : unsent(s.threads).length}`, `onToggle={(t, shift) => ctl.toggleSelect(t, shift)}`, `onClear={() => ctl.clearSelection()}`, `onNote={v => ctl.setBatchNote(v)}`, `onSendSelection={() => void ctl.sendSelection()}` and `onSendUnsent={() => void ctl.sendUnsent()}`. When `s.deleted` is set, pass no `onToggle`.

`web/shell/src/batch-ui.test.ts`:

```ts
import { describe, expect, it, vi } from "vitest";
import { flush, mount } from "./test/svelte";
import BulkBar from "./ui/BulkBar.svelte";

describe("BulkBar", () => {
  it("announces the count politely and sends on Cmd or Ctrl+Enter from the note", () => {
    const onSend = vi.fn();
    const onNote = vi.fn();
    const m = mount(BulkBar, { count: 3, note: "", busy: false, onNote, onSend, onClear: vi.fn() });
    const count = m.root.querySelector(".bulk-count")!;
    expect(count.textContent).toBe("3 selected");
    expect(count.getAttribute("role")).toBe("status");
    const input = m.root.querySelector("input.bulk-note") as HTMLInputElement;
    expect(input.getAttribute("aria-label")).toBe("Note for the agent (optional)");
    flush(() => input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true })));
    expect(onSend).not.toHaveBeenCalled();
    flush(() => input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", ctrlKey: true, bubbles: true })));
    flush(() => input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", metaKey: true, bubbles: true })));
    expect(onSend).toHaveBeenCalledTimes(2);
    m.update({ count: 2, note: "", busy: true, onNote, onSend, onClear: vi.fn() });
    expect(count.textContent).toBe("2 selected");
    expect((m.root.querySelector("button.primary") as HTMLButtonElement).disabled).toBe(true);
    m.unmount();
  });
});
```

Add to `sidebar.test.ts`:

```ts
  it("shows checkboxes on open threads only, the unsent button, and each thread's sends", () => {
    const open: Thread = { ...base, id: "o", anchor, status: "open", sent_to_agent: true, comments: [comment("1", "viewer", "Alex", "x")],
      sends: [{ batch_id: "b", size: 2, note: "Soon", sent_by: "Alex", sent_at: base.created_at }] };
    const fresh: Thread = { ...base, id: "f", anchor, status: "open", sent_to_agent: false, comments: [comment("2", "viewer", "Alex", "y")] };
    const done: Thread = { ...base, id: "d", anchor, status: "resolved", sent_to_agent: false, comments: [comment("3", "viewer", "Alex", "z")] };
    const onToggle = vi.fn();
    const onSendUnsent = vi.fn();
    const m = mount(Sidebar, { threads: [open, fresh, done], resolved: {}, selected: null, selection: { ids: ["o"], anchor: "o" }, batchNote: "", batchBusy: false,
      unsentCount: 1, onToggle, onClear: vi.fn(), onNote: vi.fn(), onSendSelection: vi.fn(), onSendUnsent,
      onSelect: vi.fn(), onSend: vi.fn(), onResolve: vi.fn(), onReply: vi.fn() });
    expect(m.root.querySelectorAll(".section-open .thread-check")).toHaveLength(2);
    expect(m.root.querySelectorAll(".section-resolved .thread-check")).toHaveLength(0);
    expect(m.root.querySelector(`.thread-card[data-thread="o"]`)!.classList.contains("checked")).toBe(true);
    expect(m.root.querySelector(".bulk-count")!.textContent).toBe("1 selected");
    expect(m.root.querySelector(`.thread-card[data-thread="o"] .send-line`)!.textContent).toBe("Sent to agent by Alex with 1 other · “Soon”");
    flush(() => (m.root.querySelector("button.send-unsent") as HTMLButtonElement).click());
    expect(onSendUnsent).toHaveBeenCalled();
    const box = m.root.querySelector(`.thread-card[data-thread="f"] .thread-check`) as HTMLInputElement;
    flush(() => box.dispatchEvent(new MouseEvent("click", { bubbles: true, shiftKey: true })));
    expect(onToggle).toHaveBeenCalledWith(fresh, true);
    m.unmount();
  });
```

- [ ] **Step 4: Styles**

Append to `web/shell/src/theme.css`:

```css
/* Batch send to agent (spec §8). */
.thread-card header { display: flex; align-items: center; gap: 8px; }
.thread-check { width: 16px; height: 16px; margin: 0; flex: none; accent-color: var(--accent); cursor: pointer; }
.thread-card.checked { box-shadow: inset 0 0 0 1px var(--accent); }
.send-line { font-size: 12px; margin: 4px 0; }
.send-unsent { width: 100%; }
.bulk-bar { position: sticky; top: -12px; z-index: 2; margin: -12px -12px 0; padding: 10px 12px; background: var(--card); border-bottom: 1px solid var(--border);
  display: flex; flex-direction: column; gap: 8px; animation: bulk-in 140ms ease-out; }
@keyframes bulk-in { from { opacity: 0; transform: translateY(-4px); } to { opacity: 1; transform: none; } }
.bulk-row { display: flex; align-items: center; gap: 8px; }
.bulk-count { margin: 0 auto 0 0; font-weight: 600; font-variant-numeric: tabular-nums; }
.bulk-note { font: inherit; width: 100%; background: var(--bg); color: var(--fg); border: 1px solid var(--border); border-radius: 8px; padding: 6px 10px; }
@media (max-width: 480px) { .bulk-row button, .send-unsent { min-height: 36px; } }
@media (prefers-reduced-motion: reduce) { .bulk-bar { animation: none; } }
```

The sidebar has `padding: 12px`. The bar's negative margins and `top: -12px` let it span the sidebar edge to edge and stick flush to its top.

- [ ] **Step 5: Run and commit**

Run: `cd web && npm run lint && npm run typecheck && npx vitest run && npm run build && node scripts/bundle-size.mjs; echo "exit=$?"`
Expected: `exit=0`. If the bundle budget fails, stop and report the sizes.

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add web/shell/src/view/batch-model.ts web/shell/src/view/batch-model.test.ts web/shell/src/ui/BulkBar.svelte web/shell/src/batch-ui.test.ts \
  web/shell/src/threads.ts web/shell/src/view/artifact-controller.ts web/shell/src/view/artifact-controller.test.ts web/shell/src/ui/SidebarIsland.svelte \
  web/shell/src/ui/Sidebar.svelte web/shell/src/ui/ThreadCard.svelte web/shell/src/sidebar.test.ts web/shell/src/theme.css
git commit -m "Select several threads in the sidebar and send them to the agent as one batch"
git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed
```

---

### Task 23: Browser tests and verification for batch send

**Files:**
- Create: `web/e2e/batch.spec.ts`

- [ ] **Step 1: The spec**

`web/e2e/batch.spec.ts`:

```ts
import { test, expect } from "@playwright/test";
import { api, openArtifact, postThread, publishAs, reach, registerSession, startDaemon } from "./fixtures";

let d: Awaited<ReturnType<typeof startDaemon>>;
test.beforeAll(async () => { test.setTimeout(180_000); d = await startDaemon(); });
test.afterAll(async () => { await d?.stop(); });

async function fresh(title: string, n: number) {
  const s = await registerSession(d.base, d.token, "claude", `batch-${title}`);
  const { artifact } = await publishAs(d.base, d.token, s.id, title, { "index.html": "<main><h2>Quarterly goals</h2></main>" });
  const ids: string[] = [];
  for (let i = 0; i < n; i++) ids.push((await postThread(d.base, artifact.id, `item ${i}`)).id);
  return { sid: s.id, aid: artifact.id, ids };
}

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: select three with a shift range, send with a note, and the agent gets one delivery`, async ({ page }) => {
    const { sid, aid, ids } = await fresh(`Batch ${mode}`, 4);
    await openArtifact(page, d.base, aid, 1, mode);
    await page.getByRole("button", { name: /Threads/ }).click();
    const box = (id: string) => page.locator(`.thread-card[data-thread="${id}"] .thread-check`);
    await box(ids[0]).click();
    await box(ids[2]).click({ modifiers: ["Shift"] });
    const bar = page.getByRole("region", { name: "Selected comments" });
    await expect(bar.getByRole("status")).toHaveText("3 selected");
    await expect(box(ids[3])).not.toBeChecked();
    await expect(page.getByRole("button", { name: "Send 4 unsent to agent" })).toBeVisible();
    await bar.getByLabel("Note for the agent (optional)").fill("Before the demo");
    await bar.getByLabel("Note for the agent (optional)").press("ControlOrMeta+Enter");
    await expect(bar).toHaveCount(0);
    const got = await api(d.base, d.token, `/api/sessions/${sid}/feedback?tier=piggyback`);
    expect(got.feedback.map((f: { thread_id: string }) => f.thread_id)).toEqual(ids.slice(0, 3));
    expect(got.text.split("\n")[1]).toBe(`[clax] 3 comments on "Batch ${mode}", sent together by Viewer. Note: "Before the demo"`);
    await expect(page.locator(`.thread-card[data-thread="${ids[1]}"] .send-line`)).toHaveText("Sent to agent by Viewer with 2 others · “Before the demo”");
    await expect(page.locator(`.thread-card[data-thread="${ids[1]}"] .working-marker`)).toHaveText("Claude Code is working…");
    await page.getByRole("button", { name: "Send 1 unsent to agent" }).click();
    await expect(page.getByRole("button", { name: /unsent to agent/ })).toHaveCount(0);
    const rest = await api(d.base, d.token, `/api/sessions/${sid}/feedback?tier=piggyback`);
    expect(rest.feedback.map((f: { thread_id: string }) => f.thread_id)).toEqual([ids[3]]);
  });

  test(`${mode}: a selected thread that disappears leaves the selection`, async ({ page }) => {
    const { aid, ids } = await fresh(`Prune ${mode}`, 2);
    await openArtifact(page, d.base, aid, 1, mode);
    await page.getByRole("button", { name: /Threads/ }).click();
    for (const id of ids) await page.locator(`.thread-card[data-thread="${id}"] .thread-check`).click();
    const count = page.getByRole("region", { name: "Selected comments" }).getByRole("status");
    await expect(count).toHaveText("2 selected");
    await page.request.post(`${d.base}/api/artifacts/${aid}/threads/${ids[0]}/resolve`);
    await expect(count).toHaveText("1 selected");
    await reach(page, page.locator(".bulk-bar"));
    await page.getByRole("button", { name: "Clear" }).click();
    await expect(page.locator(".bulk-bar")).toHaveCount(0);
  });
}

test("the bulk bar fits a phone in dark mode and holds still under reduced motion", async ({ page }) => {
  const { aid, ids } = await fresh("Phone batch", 3);
  await page.setViewportSize({ width: 375, height: 812 });
  await page.emulateMedia({ colorScheme: "dark", reducedMotion: "reduce" });
  await openArtifact(page, d.base, aid, 1, "subdomain");
  await page.getByRole("button", { name: /Threads/ }).click();
  await page.locator(`.thread-card[data-thread="${ids[0]}"] .thread-check`).click();
  const bar = page.locator(".bulk-bar");
  await expect(bar).toBeVisible();
  expect(await bar.evaluate(e => getComputedStyle(e).animationName)).toBe("none");
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(375);
  const send = (await bar.getByRole("button", { name: "Send to agent" }).boundingBox())!;
  expect(send.height).toBeGreaterThanOrEqual(36);
  expect(send.x + send.width).toBeLessThanOrEqual(375);
});
```

Run: `cd web && npx playwright test e2e/batch.spec.ts`
Expected: PASS (5 tests).

- [ ] **Step 2: Browser verification (required)**

With a scratch `CLAX_HOME` and `--port 0`, as in Task 11 Step 3, publish an artifact as a session and post four threads (the Task 18 Step 3 script's multipart helper, called four times). Open it and check each of these, writing down what you saw in the task report:
- The checkboxes sit neatly beside each card head.
- Shift-click selects the range.
- The bar sticks to the sidebar top with `N selected · Send to agent · Clear` and the note field.
- Cmd+Enter sends.
- The send line appears in each card's history.
- `Send N unsent to agent` shows and hides correctly.
- Both themes look right, and at 375 px nothing scrolls sideways.
- With a screen reader (VoiceOver: Cmd+F5), the count is announced when it changes, and each checkbox reads its label.

Stop the daemon afterwards.

- [ ] **Step 3: Gates and commit**

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add web/e2e/batch.spec.ts
git commit -m "Test batch send to agent in the browser in both frame modes"
git cat-file commit HEAD | grep -q '^gpgsig ' && echo signed
```
