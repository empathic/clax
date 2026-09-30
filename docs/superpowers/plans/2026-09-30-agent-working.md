# Agent Working Signal and Version Changelog Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let an agent tell the person, through Clax, that it is working on an artifact and on named comment threads, set automatically when comment feedback reaches it and explicitly through a new `working` tool. Show that state in the artifact header, on gallery cards, on sidebar thread cards and to the page (a Clax extension of the `comments` capability). Then use the same data to give every version a changelog: the threads it addressed and a short note from the agent. The changelog appears as a once-per-viewer banner, an "Addressed in vN" group in the sidebar with one-click Resolve, and a version menu that reads as a changelog across versions.

**Architecture:** The working state is an in-memory registry in the daemon (`clax_core::working::Working`), keyed by (session, artifact), with an injected clock and a 2 minute heartbeat expiry. The daemon marks work itself whenever it hands feedback to a session: every delivery tier already runs through `take_feedback`, so one hook point serves the Stop hook, the prompt hook, `SessionStart`, tier 1 piggyback, `wait_for_feedback`, Pi injection and Codex `codex queue`. Hooks and the Pi extension only renew (every tool call) and end the turn. Changes go out as one SSE event, `working`, carrying the artifact's whole list, and ride along on `GET /api/artifacts` and `GET /api/artifacts/<id>`, which the shell already loads. The changelog is persisted: a nullable `versions.note` column, a `version_threads` link table and a bounded `viewer_seen` table. Notes and links ride on the version and thread views the shell already loads. Only the viewer's seen mark is fetched after load. UI logic lives in plain TypeScript modules (`working.ts`, `changelog.ts`) so the later Svelte port moves only thin components.

**Tech Stack:** Rust 2024 (axum 0.8, rusqlite, chrono, serde, rmcp), Preact 10 + TypeScript + Vite 6, Vitest 3 + jsdom, Playwright (Chromium), Python 3 (scripts), TypeBox (Pi extension, Pi 0.73.1).

**Spec:** `docs/superpowers/specs/2026-09-28-clax-design.md`. Task 1 amends §5 Storage, §6 HTTP API, §8 Shell UI, §9 Runtime bridge and capabilities, §10 Comments and the feedback loop, §11 Sessions, §12 MCP tool surface, §13 Plugins, §14 Security model and §16 Testing, and `docs/contract.md`.

**Decisions (binding):** `.superpowers/sdd/2026-09-30-agent-working/decisions.md`, both sections ("Agent working signal" and "Version changelog").

**Precondition:** the rename plan (`docs/superpowers/plans/2026-09-29-clax-rename.md`) is merged, and the Svelte port (`docs/superpowers/plans/2026-09-29-svelte-port.md`) has not started. Check both before Task 1:

```bash
test -f web/shell/src/artifact.tsx && test ! -f .svelte-port-base && echo ok
```

Expected: `ok`. If `.svelte-port-base` exists or `artifact.tsx` is gone, stop: the port has started, and this plan's UI tasks were written against the Preact shell.

## Global Constraints

- Rust edition 2024. `cargo clippy --workspace --all-targets -- -D warnings` and `RUSTFLAGS=-Dwarnings cargo check --workspace` pass.
- `oxlint --deny-warnings` passes over `shell bridge e2e` and the config files (`npm run lint` in `web/`).
- Every e2e spec that opens an artifact runs in both frame modes, `subdomain` and `sandbox` (`for (const mode of ["subdomain", "sandbox"] as const)`).
- Commits are signed. Commit with plain `git commit` (the repository's signing configuration applies). Never pass `--no-gpg-sign`. Stage with `git add` and explicit paths only. After each commit, `git log --show-signature -1` must show a good signature.
- Never bind or connect to port 7480. Tests and smokes start daemons with `--port 0` and a temporary `CLAX_HOME`. Never read, write or delete the real `~/.clax`, `~/.claude` or `~/.codex`, nor the home directory Clax used before its rename (spec D15). `scripts/smoke-codex.sh` reads `~/.codex/auth.json`, so no task runs it; Task 7 only edits it, for the person to run.
- In prose, comments, doc comments and commit messages, write "ID", never "id", except as a literal symbol in code.
- Doc comments and commit messages describe the contract or the change. They never mention this plan, the conversation, or the history of names.
- Every task ends with `bash scripts/quality_gates.sh; echo "exit=$?"` printing `exit=0`. Check the status itself, not only the last line of output.
- The three skill copies stay word for word identical in their shared sections (`scripts/test-plugins.sh`). Tool blocks are regenerated with `python3 scripts/sync-skill-tools.py`, never edited by hand. A tool description is one string that appears verbatim in `plugins/pi/test/fixtures/contract.json`, `crates/clax-mcp/src/tools.rs` and `plugins/pi/src/clax.ts`.
- A working view never carries a session ID, a working directory or a PID, except the token-only `GET /api/sessions/<id>/working`. No thread, comment, event or page ever carries a session ID (spec §5, §14).
- The page never learns a thread's store ID. The capability hands it opaque handles only, and only for threads the page created in its current document.
- Time to usable does not regress. Nothing new is fetched before the shell's first paint. Working state and changelog notes ride on responses the shell already awaits. The seen mark and the gallery's event stream start after load.
- UI changes are verified in a real browser (Task 11 and Task 18 have explicit steps). A passing test run is not verification for frontend work.
- Keep UI logic in `web/shell/src/working.ts` and `web/shell/src/changelog.ts` (no `preact` import). Components stay thin. Check with `grep -lE "from \"preact" web/shell/src/working.ts web/shell/src/changelog.ts`, which must print nothing.
- Rust tests never sleep on the wall clock to observe expiry. They use `ManualClock` and call `sweep` directly.

## Review Focus

1. **A stale "working" after the agent stopped.** Every path that ends work must clear it: the reply to the last named thread, a publish of the artifact by that session, the Stop hook allowing the stop, Pi's `agent_end`, session end (PATCH, the reaper, the `SessionEnd` hook, the shim exiting) and artifact deletion. A path that is missed shows "working" for up to 2 minutes. Tests: `api_working_auto.rs` (one test per path) and the working steps Task 8 adds to the comment-loop smoke.
2. **Renewal that never lapses.** The shim's 60 s session heartbeat, the Pi injection long-poll, `wait_for_feedback` polls and `codex queue` must not renew a record. Otherwise a session that is merely alive shows "working" forever. Test: "a heartbeat and a wait poll do not renew".
3. **Session IDs leaking.** `GET /api/artifacts/<id>/working`, the `working` SSE event, `GET /api/artifacts` and the capability must never carry `session_id`. The record's `key` is a fresh ULID per record, not derived from the session. Test: "working views carry no session ID" (Rust) and the capability test.
4. **The capability leaking store IDs.** `working()` and `onWorking` name threads by the handles the page already holds, and count the rest. Test: "working() names only this document's own threads, by handle".
5. **The automatic changelog link.** A publish links the threads in the publishing session's record *before* the publish clears the record. Linking never resolves a thread. An agent resolve links to the current version only when the thread has no link yet. Tests: "a publish links the threads the session was working on, then clears", and "linking leaves the thread open".
6. **The banner showing twice, or never.** The seen mark moves forward only, only on unpinned views of the latest version, and is written as soon as the banner is decided (shown or not). A first visit sets the mark and shows nothing. Tests: e2e "seen state across reloads and viewers".
7. **The version menu replacing the native `<select>`.** Keyboard (Escape returns focus, rows are links in tab order), phone width (sheet under the top bar, no horizontal scroll) and the pinned-version URL behaviour must match what the select did (`viewer.spec.ts` is updated in Task 17, not weakened).

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

Web:

| Path | Responsibility |
|---|---|
| `web/shell/src/working.ts` | Pure: types, harness labels, header, badge and marker text, event folding (Task 9) |
| `web/shell/src/working-status.tsx` | `WorkingStatus`, `WorkingBadge`, `WorkingMarker` (thin) (Task 9) |
| `web/shell/src/changelog.ts` | Pure: banner decision and text, sidebar groups, version rows (Task 16) |
| `web/shell/src/changelog-ui.tsx` | `ChangelogBanner`, `AddressedGroups`, `VersionMenu` (thin) (Task 17) |
| `web/shell/src/caps/comments.ts` | `working`, `watchWorking`, `unwatchWorking` calls (Task 10) |
| `web/bridge/src/caps/comments.ts` | `working()`, `onWorking(fn)` (Task 10) |
| `web/contract/0.2.61/comments.d.ts` | The typed Clax extension (Task 10) |
| `web/e2e/working.spec.ts`, `web/e2e/changelog.spec.ts` | Browser tests (Tasks 11, 18) |

Plugins and scripts: `plugins/pi/src/clax.ts`, `plugins/pi/src/client.ts`, `plugins/pi/test/clax.test.ts`, `plugins/pi/test/fixtures/contract.json`, `plugins/*/hooks/hooks.json`, `plugins/*/skills/clax/SKILL.md`, `plugins/*/README.md`, `scripts/test-plugins.sh`, `scripts/smoke-comment-loop.sh`, `scripts/smoke-codex.sh`.

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
| Claude Code | Marked when comments arrive by any tier. Renewed by every tool call (the `PostToolUse` hook) and every hook. Cleared by reply, publish, the Stop hook at the real end of a turn, and `SessionEnd`. | Work the person asks for in the terminal, not through a comment, is never marked: no hook knows which artifact a prompt is about. The agent must call `working`. Automatic marks carry no message. When the person interrupts a turn (Esc), Claude Code runs no Stop hook, so the mark stays until it lapses, up to 2 minutes later. Without the plugin's hooks (a plain `.mcp.json` install), only clax tool calls renew, so long work with other tools lapses after 2 minutes. |
| Codex | Marked when comments arrive (Stop hook, `SessionStart`, tier 1, `wait_for_feedback`, and `codex queue` on exit 0). Cleared by reply, publish, the Stop hook at turn end, and `SessionEnd`. | `codex queue` exiting 0 means "queued", not "seen". For a session with no TUI attached the mark is false and lapses after 2 minutes. Renewal by every tool call depends on Codex running the plugin's `PostToolUse` hook. Codex 0.159.0 names the event, but this is not measured yet: Task 7 adds the check to `scripts/smoke-codex.sh --hooks` for the person to run. Until then, only clax tool calls and the Stop hook renew. Hooks run only with `features.hooks = true` and after the person trusts them. `codex exec` skips untrusted hooks, so there is no turn-end clear: the mark lapses. No prompt hook is wired, so terminal requests are never marked. |
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

### Banner decision (`changelog.ts` `decideBanner`)

Inputs: the versions, the viewer's `seen`, the latest version, and whether the view is pinned.

- Pinned view: no banner, and the mark is not written.
- `seen` is null (first visit): no banner. Write `seen = latest`.
- `seen >= latest`: no banner.
- One new version `vN`: with k addressed threads, `vN addressed k comment(s)`, followed by `: <note>` when there is a note. With no addressed threads and a note: `vN: <note>`. With neither: no banner.
- Several new versions (m): `m new versions, k comments addressed`. With k = 0 and at least one note: `m new versions: <latest note>`. With neither: no banner.
- Whatever the outcome, write `seen = latest` once decided. So the banner shows once per viewer per version.

### Time to usable

Notes and addresses ride on `GET /api/artifacts/<id>`, which the shell already awaits before it renders. The cost is one indexed query of `version_threads` per artifact and one more column. The seen mark is fetched after the viewer lookup that already runs after load, and the banner appears when it arrives. It is never on the path to first paint or to comment mode. When the Svelte port's Task 12 embeds the artifact in the bootstrap block, notes and addresses come along with it. The seen mark stays an after-load fetch.

---

### Task 1: Spec and contract amendments

Docs only. The tool-count lists (`Twenty-two tools:` in `docs/contract.md` and the READMEs) are not touched here. `scripts/sync-skill-tools.py --check` compares them with the fixture, which gains `working` only in Task 5.

**Files:**
- Modify: `docs/superpowers/specs/2026-09-28-clax-design.md` (§5, §6, §8, §9, §10, §11, §12, §13, §14, §15, §16)
- Modify: `docs/contract.md` (`### publish`, `## Sessions`, `### The comments capability`, `### Tools` under "Comments and feedback", `### What the person sees`)

**Interfaces:** none (documentation of Tasks 2–18).

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
  ends the turn's working records); `PostToolUse` → `hook --agent claude tool`
  (renews working records, timeout 5 s; gives up after 2 s); ``. In the Codex `hooks/hooks.json` bullet, after `` `Stop` → the same with `stop` `` add `` (which also ends the turn's working records when it allows the stop), `PostToolUse` → the same with `tool` (renews working records; not yet measured on Codex, see `docs/contract.md`) ``. In the Pi bullets, replace `for the fourteen tools` with `for the twenty-three tools`, and add a bullet: ``- Working: `tool_call` renews the session's working records (at most once every 15 s), and `agent_end` ends them.``

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
`tool` hook (`PostToolUse`, 2 s, 1 s per request) renews them
(`POST /api/sessions/<id>/working/renew`).
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
| Claude Code | every tool call (`PostToolUse` hook), every hook, clax tool calls | work asked for in the terminal (call `working`); a turn you interrupt with Esc runs no Stop hook, so its mark lapses within 2 minutes |
| Codex | clax tool calls, the Stop hook; every tool call once the `PostToolUse` hook is measured to run (`scripts/smoke-codex.sh --hooks`) | a `codex queue` delivery to a session with no TUI marks it for up to 2 minutes; `codex exec` without trusted hooks never ends the turn; terminal requests |
| Pi | every tool call (at most every 15 s) | terminal requests; a Pi process killed without `session_shutdown` |
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

- [ ] **Step 11: Check and commit**

Run: `grep -n "fourteen tools" docs/superpowers/specs/2026-09-28-clax-design.md; bash scripts/test-plugins.sh | tail -1`
Expected: no `fourteen tools` line; `plugin checks passed`.

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add docs/superpowers/specs/2026-09-28-clax-design.md docs/contract.md
git commit -m "Specify the agent working signal and the version changelog"
git log --show-signature -1 | head -3
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
```

---

### Task 3: Working routes, events and the sweeper

**Files:**
- Create: `crates/clax-server/src/working.rs`, `crates/clax-server/src/routes/working.rs`, `crates/clax-server/tests/api_working.rs`
- Modify: `crates/clax-server/src/lib.rs`, `crates/clax-server/src/state.rs`, `crates/clax-server/src/feedback.rs` (`FeedbackCtx` gains `working`), `crates/clax-server/src/routes/mod.rs`, `crates/clax-server/src/routes/events.rs`, `crates/clax-server/src/routes/artifacts.rs` (`with_owner`, `list`, `get`), `crates/clax-server/src/daemon.rs`, `crates/clax-server/src/testing.rs`

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

In `routes/artifacts.rs`, give `with_owner` a third parameter `working: &[clax_core::working::WorkingView]` and set `v["working"] = json!(working);`. `list` passes `s.working.all()` lookups (`all.get(&a.id).map(Vec::as_slice).unwrap_or(&[])`, with `all` computed once before the loop and moved into the closure). `get` passes `s.working.for_artifact(id.as_str())`.

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
  crates/clax-server/src/routes/events.rs crates/clax-server/src/routes/artifacts.rs crates/clax-server/src/daemon.rs crates/clax-server/src/testing.rs
git add -u crates/clax-server/tests
git commit -m "Serve working records over REST and SSE, and sweep lapsed ones"
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
```

---

### Task 7: Hooks: the Stop hook ends the turn, and a `tool` hook renews

**Files:**
- Modify: `crates/clax-hooks/src/events.rs`, `crates/clax-cli/src/commands/hook.rs`, `crates/clax-hooks/tests/golden.rs`, `plugins/claude-code/hooks/hooks.json`, `plugins/clax/hooks/hooks.json`, `scripts/test-plugins.sh`, `scripts/smoke-codex.sh`, `plugins/claude-code/README.md`, `plugins/clax/README.md`
- Create: `crates/clax-hooks/tests/fixtures/claude-post-tool-use.json`, `crates/clax-hooks/tests/fixtures/codex-post-tool-use.json`

**Interfaces:**
- Produces: `clax_hooks::events::tool(harness: &str, input: &HookInput, daemon: &dyn Daemon) -> anyhow::Result<HookOutput>` (always `HookOutput::none()`).
- Changes: `clax_hooks::events::stop` posts `/api/sessions/<sid>/working/end` when it allows the stop.
- Produces: `clax hook --agent <claude|codex> tool`, with a budget of 2 s for the whole run and 1 s per request.

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

- [ ] **Step 4: Wire the hooks**

`plugins/claude-code/hooks/hooks.json`, add after `Stop`:

```json
    "PostToolUse": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "\"${CLAUDE_PLUGIN_ROOT}/scripts/ensure-clax.sh\" exec hook --agent claude tool",
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
            "command": "bash \"${PLUGIN_ROOT}/scripts/ensure-clax.sh\" exec hook --agent codex tool",
            "timeout": 5
          }
        ]
      }
    ],
```

`scripts/test-plugins.sh`: in the Codex check, change `("SessionStart", "SessionEnd", "Stop")` to `("SessionStart", "SessionEnd", "Stop", "PostToolUse")`. In the Claude Code quoting check, add `"PostToolUse"` to the event tuple. In the "Stop and prompt hooks are wired" check, add:

```python
ok = ok and all(h["command"].endswith("exec hook --agent claude tool") and h["timeout"] == 5 for h in cmds(claude, "PostToolUse")) and cmds(claude, "PostToolUse")
ok = ok and all('exec hook --agent codex tool' in h["command"] and h["timeout"] == 5 for h in cmds(codex, "PostToolUse")) and cmds(codex, "PostToolUse")
```

and rename its PASS text to `Stop, prompt and PostToolUse hooks are wired`.

`plugins/claude-code/README.md`: after the `Stop` item, add:

```markdown
  - `PostToolUse` (`tool`, 5 s; gives up after 2 s): after every tool call,
    keeps the page's "working" status alive (it lapses 2 minutes after the
    last renewal). One short request to the daemon per tool call.
```

and append to the `Stop` item: ``When nothing blocks, the turn is over: the page stops showing the agent as working.``

`plugins/clax/README.md`: in the Hooks paragraph, after `hands over comments sent to the session at the end of a turn`, insert `, and when nothing is waiting ends the page's "working" status; `PostToolUse` (`tool`, 5 s) keeps that status alive after each tool call (not yet measured on Codex; see `docs/contract.md`)`.

- [ ] **Step 5: The manual Codex check (edit only; not run by this task)**

In `scripts/smoke-codex.sh`, inside the `--hooks` branch of the session check (after the `with_id` check), add to the Python block:

```python
if hooks:
    log = open(os.path.join(os.environ["CLAX_HOME"], "logs", "hooks.log")).read()
    ran = "agent=codex event=tool" in log
    print("smoke: " + ("the PostToolUse hook ran (renewal on every tool call works on this Codex)"
          if ran else "the PostToolUse hook did not run: Codex renews working records only on clax tool calls and the Stop hook"))
```

This prints the finding and fails nothing. The person runs `scripts/smoke-codex.sh --hooks` (it reads `~/.codex/auth.json`, which this plan must not), then records the result in `docs/contract.md` ("Working" table, Codex row).

- [ ] **Step 6: Gates and commit**

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add crates/clax-hooks/src/events.rs crates/clax-cli/src/commands/hook.rs crates/clax-hooks/tests/golden.rs \
  crates/clax-hooks/tests/fixtures/claude-post-tool-use.json crates/clax-hooks/tests/fixtures/codex-post-tool-use.json \
  plugins/claude-code/hooks/hooks.json plugins/clax/hooks/hooks.json scripts/test-plugins.sh scripts/smoke-codex.sh \
  plugins/claude-code/README.md plugins/clax/README.md
git commit -m "End working records when the Stop hook allows the stop, and renew them after each tool call"
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
```

---

### Task 9: The shell shows who is working: header, gallery badge, thread marker

The design questions (the header text when several sessions work, the badge, the marker, phone width) are settled in "Design: the working record". There is nothing left to ask before starting.

**Files:**
- Create: `web/shell/src/working.ts`, `web/shell/src/working.test.ts`, `web/shell/src/working-status.tsx`, `web/shell/src/working-status.test.tsx`
- Modify: `web/shell/src/api.ts`, `web/shell/src/events.ts`, `web/shell/src/events.test.ts`, `web/shell/src/artifact.tsx`, `web/shell/src/sidebar.tsx`, `web/shell/src/sidebar.test.tsx`, `web/shell/src/gallery.tsx`, `web/shell/src/gallery.test.tsx`, `web/shell/src/theme.css`

**Interfaces:**
- Produces (`working.ts`, no Preact import): `type Working = { key: string; harness: string; message: string | null; thread_ids: string[]; started_at: string; last_heartbeat: string }`, `harnessLabel(h: string): string`, `recordLine(w: Working): string`, `statusParts(list: Working[]): { who: string; detail: string; more: string; title: string } | null`, `badgeText(list: Working[]): string | null`, `threadMarker(list: Working[], threadId: string): string | null`, `newestFirst(list: Working[]): Working[]`.
- Produces (`events.ts`): the `working` member of `ArtifactEvent` (`{ type: "working"; artifact_id: string; working: Working[] }`), a `working` listener in `subscribe`, and `subscribeWorking(onEvent: (e: ArtifactEvent) => void): () => void` (opens `/api/events?types=working`; a no-op returning a no-op when `EventSource` is undefined).
- Produces (`api.ts`): `Artifact.working?: Working[]`.
- Produces (`working-status.tsx`): `WorkingStatus({ list })`, `WorkingBadge({ list })`, `WorkingMarker({ text })`.

- [ ] **Step 1: Pure logic, test first**

`web/shell/src/working.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { badgeText, harnessLabel, statusParts, threadMarker, type Working } from "./working";

const w = (over: Partial<Working>): Working => ({
  key: "k", harness: "claude", message: null, thread_ids: [], started_at: "2026-09-30T10:00:00.000Z", last_heartbeat: "2026-09-30T10:00:00.000Z", ...over,
});

describe("working", () => {
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

Run: `cd web && npx vitest run shell/src/working.test.ts`
Expected: FAIL (module not found).

`web/shell/src/working.ts`:

```ts
// The working signal as the shell shows it (spec §8): pure functions over the
// daemon's working views, shared by the header, the gallery and the sidebar.
// No framework import: the Svelte port reuses this module as is.

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

Run: `cd web && npx vitest run shell/src/working.test.ts`
Expected: PASS.

- [ ] **Step 2: Types and events**

`api.ts`: `import type { Working } from "./working";` and add to `Artifact`: `/** From `GET /api/artifacts` and `GET /api/artifacts/<id>`: who is working on it now (never a session ID). */ working?: Working[];`.

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

In `events.test.ts`, extend the existing listener test's expected name list with `"working"`, and add a test for `subscribeWorking` in the same style as that file's `subscribe` tests: a stub `EventSource` class records its URL (`/api/events?types=working`) and the listeners, and a dispatched `working` message reaches `onEvent` parsed.

- [ ] **Step 3: Components**

`web/shell/src/working-status.tsx`:

```tsx
import { badgeText, statusParts, type Working } from "./working";

/** The header's status line: a polite live region that is always present, so
 * a change is announced; its text changes only when the records change. */
export function WorkingStatus({ list }: { list: Working[] }) {
  const p = statusParts(list);
  return (
    <p class={`working-status${p ? " on" : ""}`} role="status" aria-live="polite" aria-atomic="true" title={p?.title}>
      {p && <>
        <span class="working-dot" aria-hidden="true" />
        <span class="working-who">{p.who}</span>
        <span class="working-detail">{p.detail}</span>
        <span class="working-more">{p.more}</span>
      </>}
    </p>
  );
}

export function WorkingBadge({ list }: { list: Working[] }) {
  const text = badgeText(list);
  return text ? <span class="working-badge"><span class="working-dot" aria-hidden="true" />{text}</span> : null;
}

export function WorkingMarker({ text }: { text: string }) {
  return <p class="working-marker"><span class="working-dot" aria-hidden="true" />{text}</p>;
}
```

`web/shell/src/working-status.test.tsx`:

```tsx
import { render } from "preact";
import { describe, expect, it } from "vitest";
import { WorkingStatus } from "./working-status";

describe("WorkingStatus", () => {
  it("is an empty polite live region while nobody works, and fills in", () => {
    const root = document.createElement("div");
    render(<WorkingStatus list={[]} />, root);
    const p = root.querySelector("p.working-status")!;
    expect(p.getAttribute("role")).toBe("status");
    expect(p.getAttribute("aria-live")).toBe("polite");
    expect(p.textContent).toBe("");
    render(<WorkingStatus list={[{ key: "k", harness: "pi", message: "Tidying", thread_ids: [], started_at: "s", last_heartbeat: "s" }]} />, root);
    expect(root.querySelector("p.working-status")).toBe(p);
    expect(p.textContent).toBe("Pi is working: Tidying");
    expect(p.querySelector(".working-dot")!.getAttribute("aria-hidden")).toBe("true");
    render(null, root);
  });
});
```

- [ ] **Step 4: Wire the artifact view, sidebar and gallery**

`artifact.tsx`:
- State: `const [working, setWorking] = useState<Working[]>([]);`. Where the initial `getArtifact` result is stored (`setData`), also `setWorking(d.artifact.working ?? [])`. In the `ready`/`resync` refetch's `.then(d => …)`, add `setWorking(d.artifact.working ?? [])`.
- In `onEventRef.current`: `if (e.type === "working") setWorking(e.working);`.
- `Shell` gains `status?: ComponentChildren`, rendered right after `<h1>{title}</h1>`. The loaded view passes `status={<WorkingStatus list={working} />}`.
- `Sidebar` gets `working={working}`.

`sidebar.tsx`: add `working?: Working[]` to `Props`. In `Card`, compute `const marker = t.status === "open" ? threadMarker(p.working ?? [], t.id) : null;` (thread `working` into `Card` through the spread props it already receives), and render `{marker ? <WorkingMarker text={marker} /> : label && <p class="waiting">{label}</p>}` in place of the existing `{label && …}` line.

`sidebar.test.tsx`, add:

```tsx
  it("shows the working marker in place of the waiting indicator, until the record drops the thread", () => {
    const t: Thread = { ...base, id: "w", anchor, status: "open", sent_to_agent: true, comments: [comment("1", "viewer", "Alex", "two columns")],
      feedback_state: { thread_id: "w", state: "delivered", tier: "stop_hook", since: base.created_at, resends: 0, exhausted: false } };
    const rec = { key: "k", harness: "codex", message: null, thread_ids: ["w"], started_at: base.created_at, last_heartbeat: base.created_at };
    const root = document.createElement("div");
    document.body.appendChild(root);
    const draw = (working: typeof rec[]) => render(<Sidebar threads={[t]} resolved={{}} working={working} now={new Date(base.created_at)} selected={null}
      onSelect={vi.fn()} onSend={vi.fn()} onResolve={vi.fn()} onReply={vi.fn()} />, root);
    draw([rec]);
    expect(root.querySelector(".working-marker")!.textContent).toBe("Codex is working…");
    expect(root.querySelector(".waiting")).toBeNull();
    draw([]);
    expect(root.querySelector(".working-marker")).toBeNull();
    expect(root.querySelector(".waiting")!.textContent).toContain("delivered via the Stop hook");
    render(null, root);
    root.remove();
  });
```

`gallery.tsx`:
- In the card's `.meta`, after the publisher span, render `<WorkingBadge list={a.working ?? []} />`.
- After the first successful `listArtifacts()`, open the stream once:

```tsx
  const [streaming, setStreaming] = useState(false);
  useEffect(() => {
    if (!streaming) return;
    return subscribeWorking(e => {
      if (e.type === "working") setArtifacts(list => list && list.map(a => (a.id === e.artifact_id ? { ...a, working: e.working } : a)));
      if (e.type === "ready" || e.type === "resync") refresh();
    });
  }, [streaming]);
```

and in `refresh`'s success branch add `setStreaming(true)`. The stream opens after the first render with data, never before.

`gallery.test.tsx`: add `working: [{ key: "k", harness: "claude", message: null, thread_ids: [], started_at: "2026-09-28T11:00:00Z", last_heartbeat: "2026-09-28T11:00:00Z" }]` to the first `ARTIFACTS` entry, and a test in the file's style asserting that the first card's `.working-badge` reads `Claude Code working`, and the second card has none.

- [ ] **Step 5: Styles (tokens for both themes, phone width, reduced motion)**

Append to `web/shell/src/theme.css`:

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
  .topbar h1 { flex-basis: auto; flex: 1 1 0; }
}
```

`display: none` keeps an idle status out of the layout. A `role="status"` element hidden this way is still in the DOM, and screen readers announce the text when it is shown.

- [ ] **Step 6: Run and commit**

Run: `cd web && npm run lint && npm run typecheck && npx vitest run`
Expected: PASS.

Run: `grep -lE "from \"preact" web/shell/src/working.ts`
Expected: no output.

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add web/shell/src/working.ts web/shell/src/working.test.ts web/shell/src/working-status.tsx web/shell/src/working-status.test.tsx \
  web/shell/src/api.ts web/shell/src/events.ts web/shell/src/events.test.ts web/shell/src/artifact.tsx web/shell/src/sidebar.tsx \
  web/shell/src/sidebar.test.tsx web/shell/src/gallery.tsx web/shell/src/gallery.test.tsx web/shell/src/theme.css
git commit -m "Show who is working in the header, on gallery cards and on thread cards"
```

---

### Task 10: The page capability: `working()` and `onWorking(fn)`

**Files:**
- Modify: `web/contract/0.2.61/comments.d.ts`, `web/shell/src/caps/host.ts` (`CapEnv.working`), `web/shell/src/caps/comments.ts`, `web/shell/src/caps/comments.test.ts`, `web/bridge/src/caps/comments.ts`, `web/bridge/test/comments.test.ts`, `web/shell/src/artifact.tsx` (passes `working` into the host env)

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
     * agents are working on this artifact now. Read-only; available under
     * either declaration form (`composer_only` included) with no consent and
     * no gesture. Never names a session or a thread's store ID.
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

In `web/shell/src/caps/comments.test.ts`, give the harness a working list. Add at module level `let workingList: Working[] = [];` (with `import type { Working } from "../working";`), reset it in the `beforeEach` (`workingList = [];`), and add `working: () => workingList` to the `env` object literal in `setup`. Then add to `describe("comments in the shell", ...)`:

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

In `caps/host.ts`, add to `CapEnv`: `/** Who is working on the artifact now (the view's latest working list). */ working(): Working[];`. In `artifact.tsx`, where the host env is built, pass `working: () => workingRef.current`, with `workingRef` a ref mirrored from the `working` state (`workingRef.current = working` on each render).

In `caps/comments.ts`:

```ts
import { harnessLabel, newestFirst, type Working } from "../working";

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

In `web/bridge/src/caps/comments.ts`, inside `commentsLocals`, add:

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
    const c = commentsLocals(f.rpc as never, {}) as unknown as { onWorking(fn: (s: unknown) => void): Promise<() => void> };
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
  web/bridge/src/caps/comments.ts web/shell/src/artifact.tsx
git add web/bridge/test/comments.test.ts
git commit -m "Let pages read who is working through the comments capability (Clax extension)"
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
```

---

### Task 16: Changelog logic in plain TypeScript

**Files:**
- Create: `web/shell/src/changelog.ts`, `web/shell/src/changelog.test.ts`
- Modify: `web/shell/src/api.ts` (`Version.note`, `Version.addresses`, `getSeen`, `putSeen`), `web/shell/src/threads.ts` (`Thread.addressed_in`)

**Interfaces:**
- `api.ts`: `Version` gains `note?: string | null; addresses?: string[]`. Adds `getSeen(aid: string): Promise<number | null>` (`GET /api/viewers/me/seen?artifact=`; null on any failure) and `putSeen(aid: string, n: number): Promise<void>` (`PUT /api/viewers/me/seen`; failures ignored).
- `threads.ts`: `Thread` gains `addressed_in?: number[]`.
- `changelog.ts` (no Preact import):
  - `decideBanner(versions: Version[], seen: number | null, latest: number, pinned: boolean): { banner: Banner | null; write: number | null }`, with `type Banner = { text: string; versions: number[]; addressed: number }`.
  - `changeGroups(versions: Version[], threads: Thread[], expand: number[]): Group[]`, with `type Group = { n: number; note: string | null; threads: Thread[]; open: boolean }`. It covers versions that address at least one thread still present, newest first, at most 10.
  - `versionRows(versions: Version[], latest: number, shown: number, now: Date): Row[]`, with `type Row = { n: number; current: boolean; latest: boolean; label: string | null; note: string | null; addressed: number; when: string }`, newest first.
  - `excerpt(t: Thread): string`: the first comment's body, whitespace collapsed, cut to 80 characters plus `…`.
  - `plural(n: number, word: string): string`.

- [ ] **Step 1: Tests first**

`web/shell/src/changelog.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import type { Version } from "./api";
import { changeGroups, decideBanner, excerpt, versionRows } from "./changelog";
import type { Thread } from "./threads";

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

Run: `cd web && npx vitest run shell/src/changelog.test.ts`
Expected: FAIL (module not found).

- [ ] **Step 2: Implement**

`web/shell/src/changelog.ts`:

```ts
// The version changelog as the shell shows it (spec §8, §10): the once-per-
// viewer banner, the sidebar's "Addressed in vN" groups and the version menu's
// rows, as pure functions. No framework import: the Svelte port reuses it.
import type { Version } from "./api";
import { relativeTime } from "./format";
import type { Thread } from "./threads";

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

Run: `cd web && npx vitest run shell/src/changelog.test.ts && npm run typecheck`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add web/shell/src/changelog.ts web/shell/src/changelog.test.ts web/shell/src/api.ts web/shell/src/threads.ts
git commit -m "Decide the changelog banner, groups and version rows in plain TypeScript"
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
- Create: `web/shell/src/changelog-ui.tsx`, `web/shell/src/changelog-ui.test.tsx`
- Modify: `web/shell/src/artifact.tsx`, `web/shell/src/sidebar.tsx`, `web/shell/src/sidebar.test.tsx`, `web/shell/src/theme.css`, `web/bridge/src/comment-mode.ts`, `web/bridge/src/bridge.ts`, `web/e2e/viewer.spec.ts`

**Interfaces:**
- `ChangelogBanner({ banner, onShow, onDismiss })`.
- `AddressedGroups({ groups, onJump(t), onResolve(t) })`, rendered by `Sidebar` above the Open section, inside `<section class="section-changes">`, with a heading that takes `tabIndex={-1}` (the Show target).
- `VersionMenu({ rows, hrefFor(n): string, shown, latest })`: replaces the `<select>`.
- `Sidebar` gains `changes?: Group[]` and `changesRef?: (el: HTMLElement | null) => void`.

- [ ] **Step 1: Component tests first**

`web/shell/src/changelog-ui.test.tsx`:

```tsx
import { render } from "preact";
import { describe, expect, it, vi } from "vitest";
import { ChangelogBanner, VersionMenu } from "./changelog-ui";

describe("changelog UI", () => {
  it("the banner is a polite status with Show and Dismiss", () => {
    const root = document.createElement("div");
    const onShow = vi.fn();
    const onDismiss = vi.fn();
    render(<ChangelogBanner banner={{ text: "v2 addressed 3 comments: Two columns", versions: [2], addressed: 3 }} onShow={onShow} onDismiss={onDismiss} />, root);
    const el = root.querySelector(".changelog-banner")!;
    expect(el.getAttribute("role")).toBe("status");
    expect(el.textContent).toContain("v2 addressed 3 comments: Two columns");
    (root.querySelector("button[aria-label='Dismiss changelog']") as HTMLButtonElement).click();
    expect(onDismiss).toHaveBeenCalled();
    (Array.from(root.querySelectorAll("button")).find(b => b.textContent === "Show changes")!).click();
    expect(onShow).toHaveBeenCalled();
    render(null, root);
  });

  it("the version menu opens a dialog of links, focuses the shown version, and closes on Escape", async () => {
    const root = document.createElement("div");
    document.body.appendChild(root);
    const rows = [
      { n: 2, current: false, latest: true, label: null, note: "Two columns", addressed: 1, when: "just now" },
      { n: 1, current: true, latest: false, label: "first", note: null, addressed: 0, when: "5 min ago" },
    ];
    render(<VersionMenu rows={rows} shown={1} latest={2} hrefFor={n => `/a/x${n === 2 ? "" : `/v/${n}`}`} />, root);
    const button = root.querySelector("button.version-button") as HTMLButtonElement;
    expect(button.textContent).toBe("v1 of 2");
    expect(button.getAttribute("aria-expanded")).toBe("false");
    button.click();
    const settle = () => new Promise(r => setTimeout(r, 50)); // Preact renders and runs effects after a task
    await settle();
    const dialog = root.querySelector("[role=dialog]")!;
    expect(dialog.getAttribute("aria-label")).toBe("Versions");
    const links = Array.from(dialog.querySelectorAll("a"));
    expect(links.map(a => a.getAttribute("href"))).toEqual(["/a/x", "/a/x/v/1"]);
    expect(dialog.textContent).toContain("Two columns");
    expect(dialog.textContent).toContain("addressed 1");
    expect(document.activeElement).toBe(links[1]);
    expect(links[1].getAttribute("aria-current")).toBe("page");
    dialog.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await settle();
    expect(root.querySelector("[role=dialog]")).toBeNull();
    expect(document.activeElement).toBe(button);
    render(null, root);
    root.remove();
  });
});
```

Add to `sidebar.test.tsx`:

```tsx
  it("lists addressed threads by version, with jump and one-click Resolve", () => {
    const t: Thread = { ...base, id: "a1", anchor, status: "open", sent_to_agent: true, comments: [comment("1", "viewer", "Alex", "two columns")] };
    const onSelect = vi.fn();
    const onResolve = vi.fn();
    const root = document.createElement("div");
    document.body.appendChild(root);
    render(<Sidebar threads={[t]} resolved={{}} changes={[{ n: 2, note: "Two columns", threads: [t], open: true }]} selected={null}
      onSelect={onSelect} onSend={vi.fn()} onResolve={onResolve} onReply={vi.fn()} />, root);
    const section = root.querySelector(".section-changes")!;
    expect(section.querySelector("summary")!.textContent).toContain("Addressed in v2");
    (section.querySelector(".change-jump") as HTMLButtonElement).click();
    expect(onSelect).toHaveBeenCalledWith(t);
    (Array.from(section.querySelectorAll("button")).find(b => b.textContent === "Resolve")!).click();
    expect(onResolve).toHaveBeenCalledWith(t);
    expect(root.querySelectorAll(".section-open .thread-card")).toHaveLength(1);
    render(null, root);
    root.remove();
  });
```

Run: `cd web && npx vitest run shell/src/changelog-ui.test.tsx shell/src/sidebar.test.tsx`
Expected: FAIL.

- [ ] **Step 2: Components**

`web/shell/src/changelog-ui.tsx`:

```tsx
import { useEffect, useRef, useState } from "preact/hooks";
import { anchorLabel, type Thread } from "./threads";
import { type Banner, excerpt, type Group, type Row } from "./changelog";

export function ChangelogBanner({ banner, onShow, onDismiss }: { banner: Banner; onShow(): void; onDismiss(): void }) {
  return (
    <div class="changelog-banner" role="status">
      <p class="changelog-text">{banner.text}</p>
      <div class="changelog-actions">
        <button type="button" class="primary" onClick={onShow}>Show changes</button>
        <button type="button" aria-label="Dismiss changelog" onClick={onDismiss}>Dismiss</button>
      </div>
    </div>
  );
}

export function AddressedGroups({ groups, onJump, onResolve, headingRef }: { groups: Group[]; onJump(t: Thread): void; onResolve(t: Thread): void; headingRef?: (el: HTMLElement | null) => void }) {
  if (groups.length === 0) return null;
  return (
    <section class="section-changes" aria-label="Changes">
      <h2 tabIndex={-1} ref={headingRef}>Changes</h2>
      {groups.map(g => (
        <details key={g.n} open={g.open} class="change-group">
          <summary>Addressed in v{g.n} <span class="muted">{g.threads.length}</span></summary>
          {g.note && <p class="change-note">{g.note}</p>}
          <ul>
            {g.threads.map(t => (
              <li key={t.id} class="change-row">
                <button type="button" class="change-jump" onClick={() => onJump(t)}>
                  <span class="anchor-label">{anchorLabel(t.anchor)}</span>
                  <span class="change-excerpt muted">{excerpt(t)}</span>
                </button>
                {t.status === "open"
                  ? <button type="button" onClick={() => onResolve(t)}>Resolve</button>
                  : <span class="muted small">Resolved</span>}
              </li>
            ))}
          </ul>
        </details>
      ))}
    </section>
  );
}

export function VersionMenu({ rows, shown, latest, hrefFor }: { rows: Row[]; shown: number; latest: number; hrefFor(n: number): string }) {
  const [open, setOpen] = useState(false);
  const button = useRef<HTMLButtonElement>(null);
  const panel = useRef<HTMLDivElement>(null);
  const close = () => { setOpen(false); button.current?.focus(); };
  useEffect(() => {
    if (!open) return;
    panel.current?.querySelector<HTMLElement>("a[aria-current=page]")?.focus();
    const outside = (e: PointerEvent) => {
      if (!panel.current?.contains(e.target as Node) && !button.current?.contains(e.target as Node)) setOpen(false);
    };
    addEventListener("pointerdown", outside, true);
    return () => removeEventListener("pointerdown", outside, true);
  }, [open]);
  return (
    <div class="version-menu">
      <button type="button" class="version-button" ref={button} aria-haspopup="dialog" aria-expanded={open} onClick={() => setOpen(o => !o)}>v{shown} of {latest}</button>
      {open && (
        <div class="version-panel" role="dialog" aria-label="Versions" ref={panel}
          onKeyDown={e => { if (e.key === "Escape") { e.preventDefault(); close(); } }}>
          <ol>
            {rows.map(r => (
              <li key={r.n}>
                <a href={hrefFor(r.n)} aria-current={r.current ? "page" : undefined}>
                  <span class="version-head"><strong>v{r.n}</strong>{r.latest && <span class="chip">latest</span>}<span class="muted">{r.when}</span>{r.label && <span class="muted">· {r.label}</span>}</span>
                  {r.note && <span class="version-note">{r.note}</span>}
                  {r.addressed > 0 && <span class="version-count muted">addressed {r.addressed}</span>}
                </a>
              </li>
            ))}
          </ol>
        </div>
      )}
    </div>
  );
}
```

`sidebar.tsx`: accept `changes?: Group[]` and `changesRef?`. Render `<AddressedGroups groups={p.changes ?? []} onJump={p.onSelect} onResolve={p.onResolve} headingRef={p.changesRef} />` after `{p.header}` and before the Open section. The existing sections are unchanged, so pin numbering is untouched.

- [ ] **Step 3: Wire into the artifact view**

In `artifact.tsx`:
- Replace the `<select …>…</select>` in the top bar with `<VersionMenu rows={versionRows(versions, latest, shown, new Date())} shown={shown} latest={latest} hrefFor={n => here(n === latest ? null : n)} />`. Following a link is a normal navigation, as `nav.assign(here(…))` was for the select.
- State: `const [changelog, setChangelog] = useState<Banner | null>(null);` and `const [expand, setExpand] = useState<number[]>([]);`.
- After the viewer lookup that already runs (the effect with `getViewer().then(first, first)`), once `data` is loaded, run the decision once per load. It is not on the path to first paint:

```tsx
  const decided = useRef(false);
  useEffect(() => {
    if (!data || decided.current) return;
    decided.current = true;
    const latest = data.artifact.current_version;
    void getViewer().catch(() => null).then(() => getSeen(id)).then(seen => {
      const { banner, write } = decideBanner(data.versions, seen, latest, pinnedVersion !== null || shown !== latest);
      if (write !== null) void putSeen(id, write);
      if (banner) { setChangelog(banner); setExpand(banner.versions); }
    });
  }, [data]);
```

- Render in the stage, before the `newer` banner: `{changelog && !newer && !deleted && <ChangelogBanner banner={changelog} onDismiss={() => setChangelog(null)} onShow={() => { setChangelog(null); setPanel(true); requestAnimationFrame(() => changesHeading.current?.focus()); }} />}`, where `changesHeading` is a ref set through `Sidebar`'s `changesRef`.
- Pass `changes={changeGroups(versions, threads, expand)}` and `changesRef={el => { changesHeading.current = el; }}` to `Sidebar`.
- The jump is `scrollTo(t)`, unchanged: it selects the thread, scrolls the frame to the anchor and flashes it. For a thread on another page it navigates there first.

- [ ] **Step 4: Reduced-motion highlight in the bridge**

In `web/bridge/src/comment-mode.ts`, append to `CSS`: `@media (prefers-reduced-motion: reduce){.o.flash,.f.flash{animation:none}}`. In `flash`, use a 1200 ms timeout instead of 1800 when `matchMedia("(prefers-reduced-motion: reduce)").matches`. In `web/bridge/src/bridge.ts` `clax:scroll-to`, compute `const behavior: ScrollBehavior = matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth";` and pass it to both `scrollBy` and `scrollIntoView`.

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

Its following assertions (URL `/v/1`, frame shows `v1`) stay as they are.

- [ ] **Step 7: Run and commit**

Run: `cd web && npm run lint && npm run typecheck && npx vitest run && npx playwright test e2e/viewer.spec.ts`
Expected: PASS.

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add web/shell/src/changelog-ui.tsx web/shell/src/changelog-ui.test.tsx web/shell/src/artifact.tsx web/shell/src/sidebar.tsx \
  web/shell/src/sidebar.test.tsx web/shell/src/theme.css web/bridge/src/comment-mode.ts web/bridge/src/bridge.ts web/e2e/viewer.spec.ts
git commit -m "Show each version's changelog: a once-per-viewer banner, the Changes section and the version menu"
```

---

### Task 18: Browser tests and verification for the changelog, and the port hand-off

**Files:**
- Create: `web/e2e/changelog.spec.ts`
- Modify: `web/e2e/fixtures.ts` (`publishNext`, `seenOf`), `docs/superpowers/plans/2026-09-29-svelte-port.md` (a hand-off note)

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

Run: `cd web && npx playwright test e2e/changelog.spec.ts e2e/viewer.spec.ts e2e/working.spec.ts`
Expected: PASS.

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

- [ ] **Step 4: Hand-off note in the Svelte port plan**

Append to `docs/superpowers/plans/2026-09-29-svelte-port.md`, at the end of its "Port strategy" section:

```markdown
**Added before the port (agent working signal and version changelog,
`docs/superpowers/plans/2026-09-30-agent-working.md`).** Their logic is
already framework-free (`web/shell/src/working.ts`, `web/shell/src/changelog.ts`);
port only the thin components. Topbar island: `WorkingStatus`, `VersionMenu`
(`working-status.tsx`, `changelog-ui.tsx`). Stage island: `ChangelogBanner`.
Sidebar island: `AddressedGroups` and the thread card's `WorkingMarker`.
Gallery: `WorkingBadge` and its `subscribeWorking` stream. The controller
owns the `working` list (from the artifact response and `working` events),
the banner decision (after the viewer lookup, never before first paint) and
the expanded groups. Task 12's bootstrap block carries the artifact
response, so `working`, version notes and `addresses` come with it; the seen
mark stays an after-load fetch.
```

- [ ] **Step 5: Final gates and commit**

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add web/e2e/changelog.spec.ts web/e2e/fixtures.ts docs/superpowers/plans/2026-09-29-svelte-port.md
git commit -m "Test the version changelog in the browser in both frame modes"
git log --show-signature -1 | head -3
```
