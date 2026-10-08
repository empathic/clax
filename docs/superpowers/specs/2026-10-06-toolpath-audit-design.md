# Toolpath audit: Clax's history as Toolpath provenance

Date: 2026-10-06
Status: draft for review. The owner's decisions and corrections of 2026-10-06 are recorded (§2). No questions are open.

This document designs how Clax records everything that happens to its
artifacts as [Toolpath](https://toolpath.net) provenance. It has three parts:

- a continuous, append-only Toolpath JSONL journal, written as things happen;
- `clax toolpath export`, which writes full Toolpath documents on demand;
- complete cross-link references in every record:
  - the agent's harness, harness session ID and transcript path;
  - the tool call that made the change: its name, the canonical hash of its
    arguments, its time, and the harness's own call ID where Clax gets it
    for free;
  - the git state of the agent's working directory;
  - Clax's own object IDs and URLs.

**Scope is recording only (owner correction, 2026-10-06).** Clax writes
Toolpath-conformant records. Resolving their references into links between
Toolpath paths is work for a future reader. One example is a
`path p import clax` importer with `p correlate` in the Toolpath repo. That
work is kept, not scheduled, in
`docs/superpowers/plans/2026-10-06-toolpath-import-clax.md`. The records are
designed so that such a reader needs nothing more from Clax (§12).

**Clax's paths are their own kind (owner correction, 2026-10-06).** A Clax
path is not an agent coding session. It is the life of an artifact: publish,
then versions, threads and comments, then addressed, then resolved. For a
live page it is the page's review history; for the install as a whole it is
the audit trail. This is Toolpath's general model: a path follows how an
artifact changes across its lifecycle (Toolpath README, "Beyond sessions").
Every Clax path has the kind `clax-audit` (§2 L8). Agent sessions appear in
two ways:

- **as actors:** who made a step;
- **as references to other paths:** `agent://` refs, transcript refs and
  tool-call hashes.

They are never the container of steps.

This document builds on the main spec (`2026-09-28-clax-design.md`, "the main
spec" below), on Echo (`2026-10-01-echo-design.md`), and on Clax in Chrome
(`2026-10-05-chrome-overlay-design.md`). Where this document is silent, they
hold. Agent questions are designed in `2026-10-06-agent-questions-design.md`
(branch `agent-questions`, "the questions spec" below). This document records
their events (§6.6) but does not restate their model.

Toolpath references are to the Toolpath repo at commit `77dc16a5`:

- `RFC.md`, "the base RFC";
- `docs/RFC-jsonl.md`, "the JSONL RFC";
- `docs/RFC-correlation.md`, "the correlation RFC";
- `schema/toolpath.schema.json`, "the schema". Clax's tests validate
  against it (§15).
- the derive crates under `crates/`. They are read only to confirm which IDs
  Toolpath keys harness sessions and tool calls by (§9.4, §12).

## 1. Purpose

Clax holds a valuable record of agent work:

- which agent published which page, from which commit of which repository;
- what the owner and the LAN viewers said about it;
- which version answered which thread;
- what the agent was asked, and what the owner answered.

Today this record lives only in `clax.db`. Some of it lives nowhere: working
records are only in memory, and no git state is captured. Toolpath already
records agents' own sessions and git history. Once Clax writes its record as
Toolpath, a future reader can join the three. It could then answer "which
conversation turn published version 4 of this page", "which commit was the
agent on when the owner asked for this change", or "which threads did this
branch's work address".

### Goals

- Every provenance-relevant change in Clax becomes a Toolpath step. The step
  names its actor (agent, owner or LAN viewer) and its time. It names the
  objects it touched, with content hashes. For an agent action it also names
  the git state of the agent's working directory and the tool call that made
  it.
- A journal is appended continuously under `~/.clax/toolpath/`. The journal
  conforms to the JSONL RFC and is rotated. It survives crashes without
  leaving partial lines, and it never slows a write.
- `clax toolpath export` writes a deterministic Toolpath `Graph` with one
  path per artifact. It can cover the whole install, selected artifacts or
  live pages, the actions of one agent session, or a time range. Options
  redact text.
- Every harness gets the same references, whether or not any Toolpath
  reader understands that harness today.

### Non-goals

- **Importing or correlating.** This applies in Clax and in the Toolpath
  repo. That is future work (owner correction).
- **Paths per agent session.** Clax does not write them; sessions are actors
  and references (owner correction).
- **Signing.** Steps carry no `meta.signatures`. Clax is not a trust anchor
  for the git state the agent side reports (§9).
- **Page content and diff content.** Versions are recorded by content hash,
  and working-tree changes by the hash of their diff only.
- **Operational traffic:** presence, heartbeats, delivery retries and the
  like (§6.11).
- **Writing to the Toolpath cache** (`~/.toolpath/`).
- **A viewer for the journal** in the Clax shell.

## 2. Decisions

The owner decided O1–O3 before this design. O4–O7 are the owner's answers
and corrections on review, all dated 2026-10-06. L1–L16 are this design's
decisions.

| # | Decision | Reason |
|---|---|---|
| O1 | Both: a continuous append-only Toolpath JSONL journal under `~/.clax/toolpath/` (rotated, conforming to the JSONL RFC), and `clax toolpath export`, which writes full Toolpath documents on demand: the whole install, one artifact or live page, or a time range. The importer the owner also asked for is deferred by O6. | Owner decision. |
| O2 | For each agent action, the agent side captures git context from the harness's working directory: the remote URL, the branch, the HEAD commit, a dirty flag, and a hash of the diff. Agent actions are publish and version, reply, addressed, question and answer, working start and stop, and watch. Nothing is captured outside a git repository, and diff contents never leave the agent side. | Owner decision. |
| O3 | Steps carry what Toolpath needs to correlate them with its other paths, following the correlation RFC: the harness name and harness session ID (the IDs Toolpath's importers use), the agent identity, the Clax version and build commit, the Clax object IDs and URLs, the live-page origin and path, an opaque owner identity, and timestamps. Comment text is recorded, because the journal is the owner's local archive. Export has redaction options. | Owner decision. |
| O4 | **Every tool call is recorded, for every harness.** A record holds the tool name, the canonical hash of its arguments (§12.2), and the time. It also holds the harness's exact call ID where Clax gets it for free: Pi, whose Clax extension runs the tool, and Claude Code, through a PostToolUse hook (about 20 ms per call, accepted). A reader joins a Clax step to a transcript's tool call by exact ID when there is one. Otherwise it joins by tool name, argument hash and time window (§12.3). The join works the same for Claude Code, Codex, Pi, Gemini and Grok. | Owner decision (review Q1). It gives step-level links for every harness without per-harness plumbing. |
| O5 | The owner accepted the remaining review answers. Q2: `at-revision` goes into the correlation RFC. Q3: a deleted artifact's history is kept, and purging comes later. Q4: redaction hashes are unsalted. Q5: the kind is hosted at `https://toolpath.net/kinds/clax-audit/v1.0.0`. The RFC amendment and the kind page are Toolpath-repo work, deferred with O6. Clax writes the kind URI and the `at-revision` relation now. | Owner decisions (review Q2–Q5). |
| O6 | **Scope is recording only.** Clax writes the journal and exports with complete cross-link references. Import, correlation and the Toolpath-repo changes are future work and not scheduled. No recorded reference is limited by what Toolpath reads today: every harness, Grok included, gets the same references. | Owner correction: "we're just recording Clax logs as toolpath". |
| O7 | **Clax's paths are not agent coding sessions.** Paths follow artifacts: an artifact's life, a live page's review history, and the install's audit trail. Their kind is `clax-audit`, never `agent-coding-session`. Agent sessions are actors and references to other paths, never containers. | Owner correction: Toolpath's general path model, not its session kind. |
| L1 | **One source of truth: an `audit_events` table** (the audit migration, §5.1). A row is written in the same SQLite transaction as the change it records. The journal and every export are pure projections of this table. | The row is atomic with the change, so no event is lost or invented. Crash recovery means "re-project from the last sequence number". Export covers history from before the journal existed (L12). The `EventBus` is not used: its one tap slot is taken, its broadcast drops events on lag, and its events carry no actor. |
| L2 | **The file append is off the writer path.** A committed transaction only nudges an appender thread through `sync_channel(1)` `try_send`. The appender reads new rows through the reader pool, in batches of at most 512, and appends them. The writer pays one small `INSERT` and nothing more. | This meets the owner's constraint. The queue is bounded at one wake-up plus one batch. Backpressure costs lag, never loss, because rows wait in the table. |
| L3 | **The journal is the install's audit trail: one stream, rotated into segments.** Each segment is one `.path.jsonl` file holding one Toolpath `Path`. A segment's steps form a linear chain in sequence order (§7). A segment rolls at the UTC day boundary or at 64 MiB, whichever comes first. | The JSONL RFC puts one path in each file. One stream gives a total order and the cheapest append: one open file, written sequentially. Rolling at the day boundary makes "what happened on Tuesday" one file. |
| L4 | **Steps chain linearly; structure lives in `meta.refs`.** Within a path, a step's only parent is the previous step of that path. Relationships are typed refs: a reply points at its thread, a version at the threads it addresses, a move at its source and target. | Toolpath calls any step outside the ancestry of `head` a dead end, meaning abandoned work. Clax's natural shape is a version chain with thread branches. That shape would label every unaddressed thread "abandoned", which is false. A linear chain has no dead ends, an unambiguous head and a deterministic order. |
| L5 | **Export is a `Graph` of artifact paths plus one install path** (§8). Each artifact path holds every step of that artifact's life; a live page is an artifact of kind `live`. The install path holds the steps that touch no artifact: sessions joining and ending, rules, scope watches, and tool calls that named no artifact. A step that touches two artifacts (a thread moved between pages) appears in both paths under one step ID, joined by symmetric `same-change` refs. | O7. The correlation RFC defines `same-change` as one event seen from two vantage points. |
| L6 | **Step IDs are `e` followed by the zero-padded 12-digit `audit_events.seq`** (`e000000000123`), the same in the journal and in every export. | The IDs are stable, sortable and unique within any path. A reader can join a journal step to an export step by ID. |
| L7 | **Clax-native records, Toolpath at render time.** `audit_events.body` holds a versioned Clax record (`v: 1`). One pure function in `clax-core::toolpath` renders a record as a Toolpath step, for both the journal and export. | There is one mapping to test. Redaction (§11) is a render option. A mapping change needs no data migration. |
| L8 | **The kind: `https://toolpath.net/kinds/clax-audit/v1.0.0`.** Every Clax path sets `meta.kind` to it (O5, O7). The kind page itself is deferred Toolpath-repo work. | This is what the base RFC's kind mechanism is for. Readers that know the URI can rely on `meta.clax` and on the `clax.*` structural types. |
| L9 | **Git context is captured by the agent side and sent in an `x-clax-git` header** (base64url JSON, §9) on each mutating request. The daemon validates the header's shape and size, and records it as the agent side reported it. | O2 says the context comes from the harness's working directory, and only the agent side knows that directory. A header leaves every endpoint's body schema unchanged. |
| L10 | **Git capture has a 300 ms deadline.** The git commands run concurrently with `GIT_OPTIONAL_LOCKS=0`. When the deadline passes, the action proceeds and `git_capture: "timeout"` is recorded. | Publish latency adds to time to usable when the agent waits on the result. A provenance field must never block or fail the action. |
| L11 | **Harness IDs are recorded as the harness reports them,** together with the transcript path where one is known. Clax does not resolve a Claude session chain. | Claude Code rotates a session across files, and Toolpath keys the chain by its oldest segment. The ID Clax sees may belong to a later segment. The reported ID plus the transcript path is enough for a reader to find the chain (O6). |
| L12 | **A one-time backfill** records existing history into `audit_events`, marked `backfilled: true`, once per home: the install row `backfill` marks it done (never the schema version). It covers sessions, artifacts, versions (hashing their stored files), assets (hashing their blobs), live pages and merged-away pages, joined sites and their answers, threads, comments, resolves, sends, deliveries, addressed links, moves, watches and questions (§6.12). Backfilled steps have no git context, no tool calls and no working records. | Export covers the whole install from day one, and the journal's first segment is complete. It runs in `Store::open` before the daemon serves (§13). |
| L13 | **Versions gain a stored content hash.** `versions.content_sha256` is the SHA-256 of a canonical manifest of each file's path, SHA-256 and size (§5.3), computed at write time. | Provenance must say which bytes were published. The file hashes come from bytes already in memory, at under 1 ms per MiB. |
| L14 | **The build commit is embedded at build time** as `CLAX_BUILD_COMMIT` by `clax-cli/build.rs` and handed to the rest at startup (§5.5). It is `unknown` when there is no git. | O3 requires it, and nothing embeds it today. |
| L15 | **Journal fsync is coalesced.** `sync_data` runs at most once a second while lines arrive, and always on rotation and shutdown (§7.4). | The table is the durable record. A line lost to power failure is re-appended from its sequence number on the next start. |
| L16 | **Clax reads the harness session IDs Toolpath reads** (§9.4), plus the transcript path wherever hook input or the extension API supplies one. The join hook gains `transcript_path`, which is stored on the session. | O3 and O6. A transcript path names the exact session file a reader needs. |

## 3. User flows

### 3.1 Nothing to do

The journal is on by default (`[toolpath] journal = true`). The owner finds
files like this one in `~/.clax/toolpath/journal/2026/10/`:

```
clax-01JB8Q2W-20261006-001.path.jsonl
```

Each file, once sealed (JSONL RFC, "Reading JSONL"), is a `Graph` that
validates against the schema. The Toolpath CLI's `path p validate` and
`path p render md` accept it.

### 3.2 Export one page's history

```
clax toolpath export --artifact k3m9q2w8x1ab -o page.path.json
```

This writes a `Graph` with one path: the artifact's life, step by step. Each
agent step names its harness session by an `agent://` ref and its tool call
by hash. `--live http://localhost:5173/settings` selects a live page by its
URL.

### 3.3 Export a week, without text

```
clax toolpath export --since 2026-10-01 --until 2026-10-08 --no-text -o week.path.json
```

This writes one path per artifact touched that week, plus the install path.
Comment bodies, version notes, labels, and question and answer text are
replaced with their hashes (§11).

### 3.4 Later: a reader joins the paths (future work, not scheduled)

No reader exists yet (O6). The records already carry everything a future
reader needs to:

- link each Clax step to the harness session that acted, through
  `agent://<provider>/<session ID>` and the transcript path;
- link it to the exact transcript tool call that produced it, by call ID or
  by (tool name, argument hash, time window);
- link it to the git commit the agent was on (`at-revision`).

§12 specifies these joins. The deferred Toolpath plan describes such a
reader.

### 3.5 Check health

`clax toolpath status` prints:

- the journal directory;
- the current segment;
- the last journalled sequence number against the newest in the table;
- the lag and the last error.

`clax doctor` gains a journal line. It warns when the lag exceeds 10 s while
the daemon is idle.

## 4. Architecture

```
 agent side (MCP shim / hook / Pi extension)
   ├─ capture git context (≤300 ms, concurrent git, no locks)
   ├─ hash the tool call's arguments (JCS + SHA-256, §12.2)
   └─ HTTP request + x-clax-session + x-clax-git + x-clax-call
        │
 daemon route ──► AuditCtx {actor, via, git, call}
        │
 Store::with_tx ─► mutation rows + audit_events row (same tx)
        │ commit
        └─► audit::nudge()  (sync_channel(1).try_send; never blocks)
                 │
        appender thread ──► reads seq > cursor via reader pool, ≤512 per batch
                 │              renders (clax-core::toolpath::render_step)
                 └─► ~/.clax/toolpath/journal/YYYY/MM/<segment>.path.jsonl
                       (write_all whole lines; coalesced sync_data; rotate)

 after the tool result is returned:
   agent side ──► POST /api/sessions/<sid>/tool-calls (background) ──► tool.call event

 clax toolpath export ─► GET /api/toolpath/export ─► reader ─► project ─► render ─► stream
```

The components are:

- **`clax-core::audit`:** record types, `AuditCtx` and `Store::record_audit`.
- **`clax-core::toolpath`:** the renderer, projections and redaction; the
  segment writer and reader; and argument hashing (`args_hash`).
- **`clax-core::gitctx`:** the git context type, its validation and header
  codec, and the capture runner (`std::process::Command`).
- **`clax-server::audit`:** the appender thread, the export and status
  routes, the `AuditCtx` extractor and the tool-call route.
- **`clax-mcp`, `clax-hooks` and `plugins/pi`:** capture and call-record
  senders.

## 5. Data model

### 5.1 The audit migration

It is migration 23, after joined sites (19), agent questions (20), the
owner's inbox (21) and the gallery's covering indexes (22). No unmerged branch's build runs on the owner's real
home, since a migration that reaches a real home fixes its number. No
branch edits a migration that has shipped.

```sql
CREATE TABLE audit_events (
  seq          INTEGER PRIMARY KEY AUTOINCREMENT,  -- step ID e<seq:012>
  at           TEXT NOT NULL,     -- RFC 3339 ms UTC, Store::now() of the change
  kind         TEXT NOT NULL,     -- §6, e.g. 'version.publish'
  actor        TEXT NOT NULL,     -- JSON, §5.2
  artifact_id  TEXT,              -- no FK: events outlive deletion
  artifact2_id TEXT,              -- second artifact of a thread.move
  thread_id    TEXT,
  session_id   TEXT,              -- the event's Clax session: an agent actor's, or the receiving session of a delivery or release
  question_id  TEXT,
  call_id      TEXT,              -- tool call this event was made under (§6.7)
  origin       TEXT,              -- live-page origin, for --live selection
  body         TEXT NOT NULL,     -- JSON Clax record, v:1 (§6)
  backfilled   INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX audit_events_artifact  ON audit_events(artifact_id, seq);
CREATE INDEX audit_events_artifact2 ON audit_events(artifact2_id, seq) WHERE artifact2_id IS NOT NULL;
CREATE INDEX audit_events_session   ON audit_events(session_id, seq);
CREATE INDEX audit_events_call      ON audit_events(call_id) WHERE call_id IS NOT NULL;
CREATE INDEX audit_events_at        ON audit_events(at);

CREATE TABLE install (k TEXT PRIMARY KEY, v TEXT NOT NULL);
-- ('id', <ULID>) minted once by this migration: the opaque install ID

ALTER TABLE versions ADD COLUMN content_sha256 TEXT;     -- §5.3; NULL before the backfill, or when a file is missing
ALTER TABLE sessions ADD COLUMN transcript_path TEXT;    -- L16
```

Properties of the table:

- **Append-only.** No code path updates or deletes a row. Deleting an
  artifact leaves its rows; the deletion is an event of its own.
- **Kept indefinitely (O5).** There is no pruning. The estimate is about
  1 KB per event; plan Task 2 measures it.
- **Commit-ordered sequence numbers.** There is a single writer, so `seq`
  order is commit order. `AUTOINCREMENT` never reuses a deleted row's
  number, and a rolled-back insert was never visible. A gap therefore means
  nothing. The appender treats `seq` as a cursor, not a count.

### 5.2 Actor

```json
{"type":"agent","session_id":"01JB…","harness":"claude","harness_session_id":"3f2c…","agent_handle":"a_9f…","transcript_path":"/Users/alex/.claude/projects/…/3f2c….jsonl"}
{"type":"owner","public_id":"u_4be1…"}
{"type":"viewer","public_id":"u_77c0…","display_name":"Sam"}
{"type":"anonymous"}
{"type":"system","reason":"ttl|rule|backfill|daemon"}
```

A system actor's event about someone names them in `body.for_actor`, an
actor object as above: an agent for TTL ends and the like, and, under
`system:backfill`, whoever the history says acted (an agent by its session,
a viewer, or the owner).

The route takes the actor from `Identity` and from the `x-clax-session`
session:

- the owner token, owner cookie or extension credential gives `owner`;
- the `clax_viewer` cookie gives `viewer`;
- the shim's session gives `agent`.

The sessionless `/mcp` route records `{"type":"agent","session_id":null}`.
It renders as `agent:clax-mcp` (§10.1). Only the token and `x-clax-via: mcp`
without a known session give that actor: a `hook` or `pi` request with the
token and no known session records the owner, on its own channel. The
agent-side headers (`x-clax-via`, `x-clax-session`, `x-clax-git`,
`x-clax-call`) count only with the token; a browser's are ignored. Every
agent side names its channel in `x-clax-via` (`mcp` from the shim and the
daemon's own `/mcp` client, `hook` from `clax hook`, `pi` from the Pi
extension); without one, a token request with a session is `mcp` and
without one `cli`.

### 5.3 Version content hash

```
manifest  = for each file in path order (BTreeMap order):
              "<path>\0<sha256 hex>\0<size>\n"
content_sha256 = "sha256:" + hex(SHA-256(manifest))
```

`write_version_then` already holds every file's bytes, and hashes them while
staging. A carried-forward file reuses the previous version's file hash from
that version's `version.publish` body. The per-file hashes are kept in the
event body (§6.1).

### 5.4 Sessions

The only change is `transcript_path` (L16). The `session.start` event (§6.8)
records what a reader needs to find the harness session: the harness, the
harness session ID, the working directory, the transcript path and the git
context. The event is a step in the install path. The session is not a path
(O7).

### 5.5 Build commit

`crates/clax-cli/build.rs` sets `CLAX_BUILD_COMMIT`, and the binary hands
it to `clax_core::set_build_commit` at startup (the daemon and the renderer
read `clax_core::build_commit()`). Embedding it in the leaf crate means a
new commit relinks `clax-cli` alone, not the workspace:

1. `$CLAX_BUILD_COMMIT` from the environment, which the release workflow
   sets;
2. otherwise `git rev-parse HEAD` in the manifest directory;
3. otherwise `unknown`.

It reruns when `git rev-parse --git-path HEAD`, the nearest existing
directory on the path of the ref file HEAD names (a commit writes a loose
ref even for a packed branch), or `packed-refs` changes. This works in
worktrees. It does not rerun on index changes, so no dirty flag is recorded.
`clax --version` stays exactly `clax 0.3.0`, which the plugin wrappers and
installers compare. `clax version --verbose` adds `commit <hex>`, `clax
status` names the daemon's commit (from `daemon.json`), and `clax doctor`
has a `build` check.

## 6. Recorded events

Every record body has this envelope. `at`, `actor` and the ID columns are not
repeated in `body`.

```json
{
  "v": 1,
  "via": "mcp|hook|pi|cli|shell|extension|lan|daemon",
  "clax_version": "0.3.1",
  "clax_commit": "<build commit hex>|unknown",
  "git": { … } | absent,
  "git_capture": "ok|not-a-repo|timeout|unavailable|no-cwd|invalid" | absent,
  "call": {"call_id": "01JB…", "tool": "publish", "args_sha256": "sha256:…"} | absent
}
```

`clax_version` and `clax_commit` name the build that recorded the event
(the backfilling build, for a backfilled one), so a rendering names that
build, never the one rendering it (§7.6). `git` appears on agent actions
whose agent side captured it (O2, §9). `call` appears on every event made
under a tool call (§6.7).

**The recording rule.** An event is recorded exactly when the store writes,
in the write's own transaction. A request that changes nothing (a resolve of
a resolved thread, a reopen of an open one, a send with nothing new to send
that keeps the target) records nothing. A request refused by the access
checks, before its audit context is resolved, makes no row and records no
event. A request the store refuses after that may make the requester's
viewer row (the `ensure_viewer` rule for requests that act) but records no
event.

**No duplicate facts.** A thread's link to a version is carried by the event
that made it: `version.publish.addresses`, `live.snapshot.addresses`, or
`thread.resolve.addressed_version`. `thread.addressed` is recorded only for
a link no other event carries.

### 6.1 Artifacts and versions

| Kind | Body | Recorded at |
|---|---|---|
| `artifact.create` | `title, kind (html\|live), icon, capabilities, contract_version` | first publish, or live-page creation |
| `version.publish` | `n, label, note, title, files{path:{sha256,size,content_type}}, content_sha256, carried[paths], addresses[thread IDs], by_page` | `Store::write_version` |
| `artifact.update` | `fields{title?, description?, icon?, pinned?, capabilities?}` | pin, unpin, metadata edits (a patch that sets no field records nothing) |
| `artifact.delete` | `title, current_version` | delete |
| `asset.upload` | `asset_id, path, sha256, size, content_type` | asset store writes |
| `asset.delete` | `asset_id, path, size, content_type` | asset deletes |
| `doc.write` | `collection, doc_id, version, op (set\|update\|delete\|str_replace\|acquire), sha256` (no content; `null` for a delete) | `db_*` writes from the page or the agent; `acquire` is a lease grant that merges data |
| `doc.move` | `from, to, collection, doc_id, version, sha256` (no content) | a private document following a viewer claimed for the owner (`claim_for_owner`); the actor is the owner |
| `viewer.claim` | `from_public_id, to_public_id` | a claim that retires a public ID (the CLI's owner row giving way to a browser, or a viewer merged into the owner), in the claim's transaction; actor the owner. A record naming `from_public_id` resolves to `to_public_id` |

### 6.2 Threads and comments

| Kind | Body | Recorded at |
|---|---|---|
| `thread.open` | `version_n, anchor {kind, selector, quote, prefix, suffix, html_hash, file, route} (the text quote is `quote`, `prefix`, `suffix`; no geometry), live_path, has_clip, first_comment_id` | `create_thread`, followed by the first comment's `comment.add` |
| `comment.add` | `comment_id, body, author_kind, author_name, via_harness, via_page` | `add_comment`, `add_addressed_reply`; a viewer comment that reopens a resolved thread records `comment.add` then `thread.reopen` |
| `thread.resolve` | `resolved_by, addressed_version (n\|null)` | `resolve_thread`, `resolve_thread_addressed`, when the thread was open; `addressed_version` is the version an agent's resolve linked it to |
| `thread.reopen` | none | `reopen_thread`, when the thread was resolved |
| `thread.delete` | `moved` | `delete_thread` |
| `thread.send` | `target ("watchers" \| {session_id, agent_handle}), feedback_ids[], batch_id?, thread_ids[]` | `send_to`, `threads:send` (one event per send that changes something); a comment's automatic forward records its own send |
| `feedback.delivered` | `feedback_id, tier` | a feedback row's hand-over while undelivered, or its acknowledgement while undelivered (tier `piggyback`); resends are not recorded. `session_id` is the receiving session |
| `feedback.release` | `feedback_id, tier (queue), reason` | a failed `codex queue` claim returned to undelivered, in the release's transaction (actor `system:daemon`; `session_id` the target). A later delivery records its own `feedback.delivered` |
| `thread.addressed` | `version_n, source (resolve)` | a link to the current version made by an agent's resolve of a thread already resolved, which records no `thread.resolve`. Every other link rides in the event that made it |

### 6.3 Live pages

| Kind | Body | Recorded at |
|---|---|---|
| `live.page` | `origin, path` | `ensure_live_page` creating a page |
| `live.snapshot` | the `version.publish` body plus `origin, path` | `store_snapshot` (in place of `version.publish`); a version a move or merge copies onto a page also has `source{artifact_id, n}` (and `artifact2_id` the source), with the source's addresses |
| `thread.move` | `from_artifact_id, from_url, to_artifact_id, to_url, move_kind (move\|merge\|unmerge), rule_id, move_id` | each `thread_moves` row, in the move's transaction, by the requester (the owner, for a rule's merge or unmerge too). `artifact_id` is the source, `artifact2_id` the target, with `thread_id` and `origin` |
| `live.rule` | `rule_id, op (set\|delete\|drop), origin, pattern, created_at, deleting` (no separate `id`) | `set`: a new rule, or a re-add of one being deleted (re-adding a rule in force records nothing); `delete`: taking it out of force; `drop`: removing it for good once its threads are back. `origin` column set; no artifact |
| `live.join` | `origin, with, site, joined[origins], rules_moved[rule IDs], rules_dropped[rule IDs]` | a join that changes the sites (the owner's request to join `origin` to `with`'s site, keyed `site`); `origin` column the site's key. Its thread moves are `thread.move` with `move_kind: join` |
| `live.split` | `origin, before_site, site, never_with[origins]` | `origin` split off the site keyed `before_site` (keyed `site` after); the pairs it is not suggested with again are implied |
| `live.page_rekey` | `from_origin, to_origin, path` | a live page keyed under another origin of its site (a join, the settling of one, or a split) |
| `live.page_merge` | `origin, path, merged_into` | a page a join emptied, merged away into its site's page of its path (`artifact2_id`), kept whole; its watchers move with it |
| `live.join_answer` | `origin, with, answer (never\|later), until` | the owner's answer to a suggested join. Marking an origin used is not recorded (§6.11) |

### 6.4 Watches

| Kind | Body |
|---|---|
| `watch.start` | `target (artifact\|page\|scope), replies_armed, source (direct\|scope), origin?, path?, cause?, move_id?` |
| `watch.stop` | `target, replies_armed, source, origin?, path?` |
| `watch.update` | `target, origin?, path?, fields {replies_armed?, source?}, cause?` |

- `target` is `scope` for a scope watch (a `live_watches` row; no
  artifact), `page` for a watch on a live page (with the page's `origin` and
  `path`), else `artifact`. The `session_id` column is the watcher's
  session; `artifact_id` the watched artifact.
- A new watch records `watch.start`; a change of an existing watch's arming,
  or a scope-made watch becoming direct, records `watch.update` with the
  fields that changed; a removal records `watch.stop`. A write that changes
  nothing records nothing.
- A page watch Clax makes or re-arms for a session, rather than the session
  asking for it, carries `cause`: `scope` when a scope watch of the session
  covers the page (on the scope watch itself, when a page is made, or when a
  thread's path brings a page under the scope, recorded after that
  thread's `thread.open` and `comment.add`), or `move` with the
  `move_id` when a move carries the watchers of a thread to its new page
  (recorded after that `thread.move`). Its actor is the requester whose
  change caused it.
- Watches removed with their session (`session.end`) or their artifact
  (`artifact.delete`) are implied by that event and not recorded apart.

### 6.5 Working records

| Kind | Body |
|---|---|
| `working.start` | `key, message, thread_ids[]` |
| `working.stop` | `key, reason (explicit\|ttl\|session_end\|resolved\|deleted), duration_ms` |

Working state stays in memory. Start and stop are recorded; heartbeats,
renewals and updates of a record that goes on are not. Each event has the
record's artifact and session in its columns.

- `reason`: `explicit` when its session cleared it, replied to its last
  thread, published, or ended its turn; `resolved` when its last thread was
  resolved (by its agent or a viewer); `deleted` when its last thread or its
  artifact was deleted; `session_end` when its session ended; `ttl` when it
  lapsed.
- A TTL expiry is recorded with actor `system:ttl`, whether the sweep or a
  later change finds the lapsed record, and `duration_ms` runs to the lapse
  (last heartbeat + 120 s), not to its removal. Any event a system actor
  records for an agent's record names the agent in `body.for_actor`.
- The events are recorded after the in-memory change, in their own
  transaction. They pair by `key`: between records of one (session,
  artifact), `seq` order need not be the registry's order (a stop of one
  record may follow the start of the next).

### 6.6 Questions

The questions spec owns the `questions` table and its transitions. This
design records one event per transition, in the transition's transaction,
keyed by `question_id`, with the asking session in `session_id` and the
question's artifact in `artifact_id`:

| Kind | Body |
|---|---|
| `question.ask` | `source (ask\|hook), tool_use_id, questions (the stored questions_json)` |
| `question.answer` | `answers (answers_json), answered_via` (actor: the owner) |
| `question.decline` | none (actor: the owner) |
| `question.release`, `question.withdraw` | `reason` |

- `question.ask`: the asking session's agent, through the channel its
  request names. A hook question created already handed to the terminal
  (no owner surface open, or the terminal chosen up front) records
  `question.ask` then `question.release` with reason `created`. A repeated
  request for the same `tool_use_id` makes no question and records nothing.
- `question.answer`: the owner, through the shell, the extension or the CLI
  (`answered_via` the same); an answer given in the terminal is the owner's
  too, reported by the hook (`via: hook`, `answered_via: terminal`).
- `question.release` `reason`: `owner` (the owner chose "Answer in the
  terminal"), `timer` (the hook's timer ran out; actor the asking agent), or
  `created`.
- `question.withdraw` `reason`: `explicit` (the asking session withdrew it),
  `unwaited` (a hook question no poll held for the grace; `system:daemon`),
  `session_end` (its session ended: under the end's actor, `system:ttl` for
  the reaper, recorded after the `session.end`), `daemon_start` (a hook
  question its hook was waiting on the previous daemon for;
  `system:daemon`), or `daemon_stop` (a hook question a poll held as the
  daemon shut down; `system:daemon`). A system actor names the asking agent in `for_actor`.
- A reason is one of these fixed phrases, never free text: the builder
  refuses any other value, and a reason of the other kind. It is classed
  safe (§11); a decline has no reason.
- A transition the store refuses (the question already closed, an `ask`
  question's release, answers that do not fit) records nothing. Marking an
  outcome received by its session (`taken_at`) is delivery bookkeeping and
  is not recorded.
- The backfill records `question.ask` and the closing transition with the
  same builders; it has only the final status, so a backfilled release's or
  withdrawal's `reason` is `null`.

The owner's inbox (migration 21) lists what agents sent the owner: replies,
versions, publishes, questions and finished work, each recorded by its own
event. Its items and their read marks have no events of their own (§6.11). Question and answer text follows the comment text rules (§11).
Export is owner-only (§8.3).

### 6.7 Tool calls

| Kind | Body |
|---|---|
| `tool.call` | `call_id` (a ULID minted by the agent side), `tool` (the bare Clax tool name, e.g. `publish`), `harness_tool` (the name the harness used, when the agent side knows it), `args_sha256` (§12.2), `started_at`, `ended_at`, `outcome (ok\|error)`, `harness_call_id` (when free), `produced[seq]` |
| `tool.call_id` | `call_id` (or `null` when unmatched), `harness_call_id`, `harness_tool`, `args_sha256` |

Every Clax tool call is recorded, read-only tools included, for every
harness (O4).

**During the call.** The agent side sends the call's identity in an
`x-clax-call` header, base64url JSON
`{call_id, tool, harness_tool?, args_sha256, started_at, harness_call_id?}`.
It is sent on each request the agent side makes for that call. Every event
recorded under such a request stores `call` in its body and `call_id` in its
column.

**After the call.** Once the tool result has gone back to the harness, the
agent side POSTs `/api/sessions/<sid>/tool-calls` in the background with the
end time and the outcome. The daemon's own `/mcp`, which has no session,
POSTs `/api/tool-calls` with `x-clax-via: mcp`, and its calls are recorded
as the sessionless agent. Both routes take the token and the body
`{call_id, tool, harness_tool?, args_sha256, started_at, harness_call_id?,
ended_at, outcome, artifact_id?}`, checked as the `x-clax-call` header is
(400 `invalid_tool_call` otherwise). The daemon records `tool.call` once
per call (201 `{recorded: true, seq}`; a repeated report records nothing
and answers 200 `{recorded: false}`), filling `produced` from the `call_id`
index. A `tool.call` belongs to the first artifact its produced events
touched, else to the artifact its arguments named when it exists (the
shim resolves it and sends `artifact_id`). Otherwise it belongs to the
install path. A report for a session that never existed is 404
`unknown_session` and records nothing; an ended session's late reports
are kept. A token request to `/api/tool-calls` without `x-clax-via: mcp`
records the owner on `cli`. A call whose handler is dropped before it
returns (the client cancelled it) is reported with `outcome: error`. A
shim that is closing waits, after rmcp's own drain, up to 3 s for calls
still running and for their reports; a call still running after that is
not recorded (rmcp cancels a running handler's context when the transport
closes but does not drop it).

The shim does not know the name its harness gave the tool, so its calls
carry no `harness_tool`; the exact IDs below come from the harness side.

**Exact call IDs, where free:**

- **Pi.** The Clax extension runs the tool and passes its `toolCallId` as
  `harness_call_id`. Its registered tool name is `harness_tool`.
- **Claude Code.** A PostToolUse hook matching Clax's MCP tools reads
  `tool_use_id`, `tool_name` and `tool_input`. It computes the argument hash
  from `tool_input` by §12.2, then POSTs
  `{tool_use_id, tool_name, args_sha256}`. The daemon looks for the session's
  most recent `tool.call` that has the same bare tool name and hash, falls
  within 60 s, and has no harness ID yet. It records `tool.call_id`, naming
  that `call_id`, or `null` when nothing matches. Steps are never rewritten,
  so the link is a later event.
- **Codex and Grok.** They hand Clax no call ID for free. Their calls carry
  the name, hash and time, and a reader joins them by §12.3. If their hook
  input later turns out to carry a call ID for MCP tools, they use the same
  `tool.call_id` path.

### 6.8 Sessions

| Kind | Body |
|---|---|
| `session.start` | `harness, harness_session_id, cwd, transcript_path, pid` (+ `git`) |
| `session.join` | the same, when a hook join or a re-registration changes the session's harness session ID, `cwd`, transcript path or `pid` |
| `session.end` | `reason (explicit\|ttl)` (+ `for_actor` under a system actor) |

These are steps in the install path, with the session in the `session_id`
column and no artifact. A session is an actor, never a path (O7).

- `session.start` and `session.join` are made by the session's own agent
  (read after the write, so the actor carries the new transcript path),
  through the request's channel, git state and tool call.
- A registration or join that only refreshes the session (a shim
  re-registering, a hook joining again) records nothing; heartbeats record
  nothing.
- `session.end` with `explicit` is the agent side ending its session, as its
  agent. The reaper ends idle sessions with `ttl`, as `system:ttl`, naming
  the agent in `for_actor`. Ending an ended session records nothing.
- The transcript path comes from hook input (`transcript_path`, or Grok
  Build's `transcriptPath`) on join, and from the extension API (Pi's
  session file) on registration. A hook joining an older daemon that
  refuses the field joins again without it.

### 6.9 What `via` means

| `via` | Origin |
|---|---|
| `mcp` | stdio shim |
| `hook` | `clax hook` |
| `pi` | Pi extension |
| `cli` | `clax` command |
| `shell` | browser shell (owner) |
| `extension` | Chrome extension |
| `lan` | LAN viewer |
| `daemon` | internal: TTL sweep, rules, backfill |

### 6.10 Ordering

`seq` order is commit order, because each insert happens inside its
transaction under the single writer. Consumers sort by `seq`, not by `at`.

### 6.11 Not recorded

Clax does not record presence, heartbeats, feedback-tier escalations after
the first delivery, stream subscriptions, reads other than tool calls,
extension credential grants, a joined site's last-used marks, the owner's
inbox items and read marks, or daemon start and stop (`daemon.log` has these).

### 6.12 Backfilled events

The backfill (L12, §13) records the history from before the audit with the
kinds above, each body built by the same builder as live recording. A
backfilled event has `backfilled = 1`, actor `system:backfill`, `via:
daemon`, no `git` and no `call`, and names who acted in `for_actor` where
the history keeps it. It also carries:

- `inferred`: the body fields the history holds only as they are now, or
  that the backfill derived:
  - `artifact.create`: `title`, `icon`, `capabilities`, `contract_version`;
  - `version.publish` and `live.snapshot`: `title`, `carried` (paths whose
    hash and size equal the previous version's: a carry and a re-upload of
    the same bytes look alike), `addresses`, and `origin`/`path` when a live
    page's key is gone;
  - `session.start`: `harness_session_id`, `cwd`, `transcript_path`, `pid`
    (a rejoin changes them); `session.end`: `reason` (`ttl` when it ended at
    least the reaper's idle time after it was last seen, else `explicit`);
  - `thread.open`: `version_n`, `live_path`, `has_clip` (moves change
    them); `thread.resolve`: `resolved_by`, `addressed_version`;
    `thread.addressed`: `source`;
  - `thread.send`: `target` (the agent when every row went to the session
    the thread targets now, else `watchers`);
  - `watch.start`: `replies_armed`, `source`, `cause`;
  - `live.page` of a merged-away page: `at` (its artifact's creation);
    `live.page_merge`: `merged_into`; `live.join`: `origin`, `joined` (the
    origins that joined the site at one time).
- `null` for what the history cannot give: `version.publish.by_page`, a
  `live.join`'s `with` and rules, a `question.release`'s or
  `question.withdraw`'s `reason`.
- On a version's file: `missing: true` (`sha256: null`) when the stored
  file cannot be read, `size_mismatch: true` when its length differs from
  the size recorded (its hash is not kept); the version then has no
  `content_sha256`. `files_unreadable: true` when its file list does not
  parse. An asset whose blob is gone has `sha256: null, missing: true`.

A resolve link rides its version's event only on a live page, when the
version made it (the page's pending address, linked by its snapshot, within
a second), else its
thread's `thread.resolve` when made with that resolve, else it is a
`thread.addressed` at the link's time (a reopened thread keeps its link).

| Kind | Body |
|---|---|
| `backfill.skip` | `table, row_id, reason`: a source row the backfill could not convert, recorded in its place. `reason` is a fixed phrase naming the fault, and at most a column and a type, never a value from the row |

Not reconstructed, because the history keeps no trace of them: earlier
resolve and reopen cycles, rows since hard-deleted, `feedback.release`,
rules, splits (apart from the answers they left), working records, document
writes and tool calls.

## 7. The journal

The journal is the install's audit trail (L3). It is one `clax-audit` path
per segment, not a per-session or per-artifact structure.

### 7.1 Files and rotation

Files live in `~/.clax/toolpath/journal/YYYY/MM/` (mode 0700):

```
clax-<install8>-<YYYYMMDD>-<nnn>.path.jsonl          (files 0600)
```

- `install8` is the first eight characters of the install ID.
- `nnn` counts the segments within one UTC day.

A segment is named by the UTC day of its first event. It rolls at the
first event whose `at` falls on a later UTC day than the segment's, or when
that event's lines and the closing `Head` and `PathClose` would take the
file past `[toolpath] segment_max_mb` (default 64, 1 to 4096); a segment
always takes at least one step. Both depend on the rows alone, so a segment
rewritten from the table splits where the first writing did.
`[toolpath] journal_retain_days` (default 0, meaning forever) keeps the
journal to events whose UTC day is no more than that many days before the
clock's. An event older than that is never written (its `seq` still moves
the cursor), so a first start over old history does not write segments it
would then remove. Each time a segment opens, the closed segments whose day
is before that cutoff are removed whole: every event in a segment falls on
its day or earlier, since a later day begins a new segment. Removal is best
effort: a segment that cannot be removed is kept, reported as the status's
`warning` and by `clax doctor`, and tried again at the next open, and the
journal goes on. Retention never touches a `.damaged` file or the table.

`[toolpath] journal` (default true) and `journal_text` (§7.7) are the other
keys; an invalid `[toolpath]` leaves the journal off, recording goes on,
and `clax toolpath status` and `clax doctor` say why.

### 7.2 Segment shape

```
{"PathOpen":{"version":"1","id":"clax-journal-01JB8Q2W-20261006-001",
  "base":{"uri":"clax://01JB8Q2WXYZ…"},
  "graph_ref":"toolpath://clax/01JB8Q2WXYZ…",
  "meta":{"title":"Clax audit trail 2026-10-06 #1",
          "kind":"https://toolpath.net/kinds/clax-audit/v1.0.0",
          "source":"clax://01JB8Q2WXYZ…",
          "refs":[{"rel":"continues","href":"clax-01JB8Q2W-20261005-001.path.jsonl"}],
          "clax":{"projection":"journal","install":"01JB8Q2WXYZ…","segment":"20261006-001",
                  "first_seq":4812,"clax_version":"0.3.0","clax_commit":"abc1234…"}}}}
{"ActorDef":{"actor":"agent:claude-code/3f2c9a1e-…","definition":{…}}}
{"Step":{…}}
…
{"Head":{"step_id":"e000000005930"}}
{"PathClose":{}}
```

- **`ActorDef`:** written the first time an actor appears in a segment,
  before that actor's first step, and again whenever the actor's definition
  grows. One actor string can gather identities over time (a harness
  session's later transcript segment, another Clax session under the same
  harness session), so the writer keeps the definition it last wrote and
  merges each new one into it: the new fields, and the union of both
  identity lists sorted by `(system, id)`. Each definition written is
  complete, because the JSONL RFC overwrites rather than merges. Export
  merges the same way.
- **`Head` and `PathClose`:** written only when a segment closes, at
  rotation or graceful shutdown. While a segment is open, its single-tip
  linear chain makes the head unambiguous.
- **`PathMeta`:** never written.
- **`PathOpen`:** names the writing build in `meta.clax` (`clax_version`,
  `clax_commit`) and the options the segment is rendered under: its
  redaction (`meta.clax.redaction`, the option names, empty by default) and
  its size cap (`meta.clax.segment_max_bytes`). A journal's first segment
  has no `continues` ref.
- **Identities:** a definition is written with its identities sorted by
  `(system, id)` from its first appearance (a first definition is merged
  with nothing), so a definition read back from a file merges with the
  same one to itself, and a resumed segment writes exactly the lines the
  first writing did. Export writes definitions the same way.
- **Parents:** each `Step`'s only parent is the segment's previous step.
  Segments link by the `continues` ref, because the base RFC allows no
  parents across paths.

### 7.3 Writing

The appender holds the only handle on the open segment. For each batch it:

1. reads the rows with `seq > cursor` (at most 512) through `with_read`;
2. renders each row as one complete line ending in `\n`;
3. calls `write_all` once for the whole batch per segment it touches (and
   again past 4 MiB, which bounds a batch's memory);
4. advances the cursor.

The table is the appender's queue: the store's nudge is a `try_send` on a
channel of capacity one, so recording never waits on the journal, and the
appender's memory is one batch plus the open segment's actor definitions.
It drains on each nudge and on a 1 s timer, a second of work at a time. A
write that fails leaves the writer to read its segment back (§7.5) before
it writes again, so a failure mid-batch leaves what a crash would.

A row that cannot be rendered (a bug) is logged and becomes a `Step` of type
`clax.unrenderable` that carries its `seq` and `kind`. The chain never
breaks.

### 7.4 Durability

The table is the source of truth and recovery rebuilds the journal from it,
so the journal is never synced more strictly than the store's own commits.
A sync is a plain `fsync(2)` (never `F_FULLFSYNC`), and it runs:

- on the 1 s timer, when no nudge came in that second and bytes are
  unsynced;
- after a drain, only once bytes have been unsynced for 5 s (so a steady
  stream of events is synced every 5 s, never per drain);
- before rotation closes a segment (recovery reads only the newest
  segment, so an older one must be on the disk);
- at shutdown.

A directory is fsynced after a segment or a directory in it is created,
renamed or removed. The JSONL RFC leaves fsync to the writer, and this is
Clax's policy. The worst case on power loss is the last 5 s of lines, which
are re-appended from the table on the next start. A failed sync makes the
writer read its segment back (§7.5) before it writes again.

**Shutdown.** The appender stops as shutdown begins, beside the connection
drain (at most 5 s): it catches up for at most 0.5 s, syncs, and writes
`Head` and `PathClose`, and the daemon waits for it at most 1.5 s more once
connections have drained. The whole shutdown stays within 6.5 s, inside
the 7 s a replacing client waits before it sends SIGTERM; what the journal
did not write, the next start writes from the table.

### 7.5 Recovery on start

Before the appender handles its first nudge, it recovers:

1. Find the newest segment, by file name order.
2. If the file does not end in `\n`, truncate it to its last `\n`
   (`set_len`) and log the number of bytes dropped. An interrupted
   `write_all` is the only way a partial line can occur.
3. Read the cursor from the last line:
   - `Step`: the cursor is its `meta.clax.seq`;
   - `PathClose`: the segment is closed, and the cursor comes from its last
     `Step`;
   - `PathOpen` alone: the cursor is `first_seq − 1`;
   - empty file: delete it and use the previous segment.
4. If a complete line does not parse, or is not one Clax writes in that
   place, which means something other than Clax wrote it, rename the file
   to `<name>.damaged` (`<name>.damaged.<n>` when that exists). It is never
   deleted or linked. Open a new segment after the highest `seq` that
   parses in the damaged file, with a `continues` ref to the damaged
   file's name; a damaged file with no step falls back to the segment
   before it. A last line of `Head` alone (a close cut short) gets its
   `PathClose`.
5. Append every row with `seq > cursor`. Rendering is a pure function of the
   row, so lost lines come back byte-identical.

The open segment is resumed under the options its `PathOpen` records,
not the configured ones, so a line a crash lost comes back as it was
written even when `journal_text` or `segment_max_mb` changed meanwhile. A
new segment under the configured options begins at the first event
recorded since the restart, that is, since the appender first read the
journal back (an earlier one may be a lost line; the time is taken once,
before the scan, so a later read-back after a failed write does not move
it), or at the segment's next rotation. A line longer than 16 MiB is not
one Clax wrote: recovery measures it without holding it, and treats it as
damage, or, unterminated at the end, as a partial line.

There is no cursor file, so the cursor can never disagree with the file, and
no step is ever duplicated. Recovery runs on the appender's thread, so the
daemon serves before it finishes. The open segment's actor definitions are
read back from its `ActorDef` lines, so a resumed segment re-declares
nothing it already declared.

### 7.6 Determinism

The renderer is a pure function of (row, install ID, render options), plus,
for an export only, the browser base URL:

- the build that recorded an event is stored in its envelope (§6) and
  rendered from there; the rendering build contributes nothing;
- the journal renders no browser URL (no `view` ref, no `meta.clax.url`),
  because the port can change between writes. A line re-appended after a
  restart on another port, or under a newer build, is byte-identical;
- `serde_json`'s default `Map` is a `BTreeMap`, so keys come out sorted;
- there are no clock reads and no `HashMap` iteration;
- the output is compact.

### 7.7 The journal and text

The journal records text (O3). `[toolpath] journal_text = false` makes the
appender render with `--no-text` rules (§11) from the next segment it
opens for an event recorded since the restart (§7.5); each segment
records the redaction it was written under.
Export always reads the table and is unaffected.

## 8. Export

### 8.1 CLI

```
clax toolpath export [--artifact <ID|URL>]... [--live <page URL>]...
                     [--by-session <Clax session ID | harness session ID>]...
                     [--since <RFC 3339 | YYYY-MM-DD>] [--until <…>]
                     [--shape artifacts|journal] [--format json|jsonl]
                     [--no-text] [--no-names] [--no-paths]
                     [--pretty] [-o <file> [--force]]
clax toolpath status [--json]
```

- **Selection:** no selector means the whole install.
  - `--artifact` and `--live` choose artifacts. A live page is matched by
    its origin and path (a query or fragment in the URL is not part of
    it): the page there now, under its joined site's key origin, and any
    page a `live.page` event records there.
  - `--by-session` takes a Clax or harness session ID. It keeps the steps
    of that session (a step whose `session_id` column names it, or whose
    actor, or the agent a system actor acts for in `for_actor`, is it),
    plus the owner, viewer and anonymous steps (a person's, or a system
    step's that names the person in `for_actor`) on the artifacts its
    steps touched.
    It filters steps; it does not change the shape (O7).
  - Selectors of one kind union; selectors of different kinds intersect.
    The kinds are the artifacts chosen (`--artifact` and `--live`
    together), the sessions, and the time range. An artifact selector
    leaves out the install path.
  - `--since` is inclusive and `--until` exclusive, both on `at`. A bare
    date means 00:00 UTC.
  - A selector that names nothing in the history (an unknown artifact,
    live page or session), a time that does not parse, or an empty range
    is refused (400, with a code), before any output.
  - A path with no selected step is left out, since a path needs a head.
- **`--shape artifacts`** (the default): the paths of §8.2.
- **`--shape journal`:** one linear audit-trail path of every selected step,
  shaped like a segment, with ID `clax-export-<digest>`.
- **`--format jsonl`:** allowed only when the result is exactly one path,
  which means `--shape journal` or a single selected artifact. Otherwise the
  command fails with an error naming `--shape journal`, because the JSONL RFC
  puts one path in each file.
  An empty JSONL result is refused too, and `--pretty` with JSONL is
  refused (`invalid_option`).
- **Redaction:** none unless an option is given; the options used are
  listed in the export (§11).
- **Output:** stdout by default. With `-o`:
  - An existing `<file>` (a dangling symlink included) is refused unless
    `--force` is given, before the daemon is asked.
  - Without `--force`, `<file>` is created exclusively (mode 0600), so a
    file that appears after that check is refused, never replaced. The
    export streams into it and is synced. On any failure `<file>` is
    removed. While the export runs, the partial file is visible under its
    name; SIGINT and SIGTERM remove it, and a SIGKILL leaves it, ended
    early (see the end check below).
  - With `--force`, the export is written to `<file>.tmp-<ULID>` (created
    new, mode 0600), synced, and renamed over `<file>`, so `<file>` is the
    old file or the whole new one. A failure or SIGINT or SIGTERM removes
    the temporary file.
  - With `--json`, a written file is reported as `{"file", "bytes"}` on
    stdout; otherwise one line goes to stderr.
- **End check:** the CLI fails (exit 1, "the export ended early") unless
  what it received ends as a whole export does: a JSON document with its
  closing brace (and a newline when indented), JSONL with its `PathClose`
  line. The daemon also aborts an export cut short (§8.3); the check
  catches one whose abort did not reach the client.

### 8.2 Graph shape

```json
{
  "graph": {"id": "clax-01JB8Q2W-<digest12>"},
  "paths": [
    {"path": {"id": "clax-artifact-k3m9q2w8x1ab",
              "base": {"uri": "clax://01JB…/a/k3m9q2w8x1ab"},
              "head": "e000000000311"},
     "steps": ["…artifact.create, version.publish, thread.open, comment.add, tool.call, …"],
     "meta": {"title": "Settings page",
              "kind": "https://toolpath.net/kinds/clax-audit/v1.0.0",
              "source": "clax://01JB…/a/k3m9q2w8x1ab",
              "actors": {"agent:claude-code/3f2c9a1e-…": {"…": "…"}, "human:clax-owner": {"…": "…"}},
              "refs": [{"rel": "view", "href": "http://localhost:7777/a/k3m9q2w8x1ab"}],
              "clax": {"projection": "artifact", "artifact_id": "k3m9q2w8x1ab", "artifact_kind": "live",
                       "origin": "http://localhost:5173", "path": "/settings"}}},
    {"path": {"id": "clax-install-01JB8Q2W", "base": {"uri": "clax://01JB…"}, "head": "…"},
     "steps": ["…session.start, live.rule, watch.start (scope), tool.call (no artifact), …"],
     "meta": {"title": "Clax install audit trail", "kind": "…clax-audit/v1.0.0",
              "clax": {"projection": "install"}}}
  ],
  "meta": {"title": "Clax export",
           "refs": [{"rel": "source", "href": "clax://01JB…"}],
           "clax": {"install": "01JB…", "clax_version": "0.3.0", "clax_commit": "abc1234…",
                    "selection": {"artifacts": ["k3m9q2w8x1ab"], "since": null, "until": null},
                    "first_seq": 1, "last_seq": 5930, "redaction": []}}
}
```

The example lists a path's keys in the schema's order. The export writes
each path as `{"steps": […], "path": {…}, "meta": {…}}`, so its `head` and
`meta.actors` follow the steps it streams; JSON key order carries no
meaning. `graph.meta.clax` also names the `shape`, and its `selection`
always holds `artifacts`, `live`, `by_sessions`, `since` and `until`
(normalized: sorted IDs and page URLs, times as UTC with milliseconds).

The rules:

- **Artifact path:** every step whose `artifact_id` or `artifact2_id` is the
  artifact, in `seq` order, chained linearly (L4). The `base` is the
  artifact's `clax://` URI, never a repository: the path follows the
  artifact, not an agent's checkout (O7). Git context stays on each agent
  step (§9).
- **Install path:** every step with no artifact.
- **Steps on two artifacts:** a `thread.move` appears in both artifact paths
  under the same step ID. Each copy gets
  `{"rel":"same-change","href":"toolpath:<other path ID>/<step ID>"}`.
  The `same-change` ref is written only when the other path is in the
  graph.
- **Artifact path meta:** `title` is the artifact's title (`Artifact <ID>`
  under `--no-text`, or when the artifact is gone), and `meta.clax` names
  its `artifact_kind` and, for a live page, its `origin` and `path`. A
  `view` ref names its browser URL.
- **Refs to objects outside the selection** keep their `clax://` form. A
  ref to another artifact path in the graph also gets a `toolpath:<path
  ID>` form beside it, and a `produced` ref names a step in the graph by
  `toolpath:<path ID>/<step ID>` instead (the first of its paths, in path
  order).
- **Journal shape:** one path, ID `clax-export-<digest>`, based on the
  install, every selected step in `seq` order. As JSONL, `PathOpen.meta.clax`
  carries what `graph.meta.clax` carries in JSON (install, build,
  selection, shape, redaction); so does a single artifact path's.
- **Unrenderable rows** become `clax.unrenderable` steps, as in the
  journal (§7.3).
- **The graph ID's digest** is the first 12 hex characters of the SHA-256 of
  the canonical selection JSON.
- **Correlation:** none, so the graph carries no `correlates` marker (O6).

### 8.3 Daemon routes

`GET /api/toolpath/export?<the CLI options as query parameters>`:

- **Who:** owner only (the daemon token or the owner cookie). Anyone else,
  a LAN viewer included, gets 403, decided from the credentials before
  anything is read or made.
- **Parameters:** `artifact` (an artifact ID; the CLI turns a URL into
  one), `live` and `by_session` repeat; `since`,
  `until`, `shape`, `format` and the flags `no_text`, `no_names`,
  `no_paths` and `pretty` (`true` or `false`) appear at most once. An
  unknown or repeated parameter is refused (400).
- **One at a time:** an export holds a store worker, a reader connection
  and its snapshot, so one runs at a time; another gets 503
  `export_busy` before any work.
- **Streaming:** the export runs on a store worker in one read
  transaction and streams in 64 KiB chunks, querying one path at a time.
  A refusal comes before the first byte, as an error status. After the
  first byte, the export's outcome decides how the body ends: a whole
  export ends its transfer normally, and its last bytes are the graph's
  closing brace (JSON) or its `PathClose` line (JSONL). Anything else (an
  error, a reader that stops reading for 60 s, an export past its
  15-minute limit, a panic) aborts the transfer without its final chunk,
  so a client sees an error, never a short document that looks whole. Measured on
  10× the perf seed (159,506 events, release, load 28–32): the whole
  install exports 202 MiB in 2.4–3.0 s in either shape, at a peak of
  17.5 MB of memory for the whole process against 9.8 MB for opening the
  store alone.
- **Response:** streamed `application/json`, or `application/x-ndjson` for
  JSONL.
- **Consistency:** the export reads one snapshot, in a single read
  transaction.
- **Order:** artifact paths in artifact ID order, then the install path;
  steps in `seq` order.
- **Determinism:** the same database, arguments, exporting build and
  browser base give byte-identical output (the build is named in
  `graph.meta.clax`, the browser base in `view` refs and `meta.clax.url`). The export has no `exported_at`.

`GET /api/toolpath/status` returns:

```json
{"journal": true, "dir": "…", "segment": "…", "cursor": 5930, "newest_seq": 5931, "lag_ms": 12, "last_error": null, "warning": null}
```

It is owner only, as export is. `segment` is the open segment's file name,
else the newest one's; `cursor` is the last journalled `seq`; `lag_ms` is
how long the journal has trailed the table without catching up (0 when
caught up); `last_error` is why it is behind (a failed write, until a
write succeeds) or off (an invalid `[toolpath]`, or an appender that
stopped unexpectedly); `warning` is a problem that does not hold it back
(retention could not remove a segment). While the journal is off,
`journal` is false and `cursor` and `lag_ms` are null. `clax doctor`'s
`journal` line warns on a `last_error`, a `warning`, a lag over 10 s, or a
status it could not read from a running daemon.

## 9. Git capture

### 9.1 What is captured

```json
{
  "repo_root": "/Users/alex/work/app",
  "remote": "origin",
  "remote_url": "https://github.com/empathic/app.git",
  "branch": "feat/settings",
  "head": "9c1e5d2b…(40 hex)",
  "dirty": true,
  "diff_sha256": "sha256:…",
  "diff_bytes": 18234,
  "untracked": 2,
  "captured_at": "2026-10-06T14:03:11.512Z"
}
```

- **`repo_root`:** `git rev-parse --show-toplevel`. A linked worktree reports
  its own root.
- **`remote`:** the current branch's upstream remote; otherwise `origin`;
  otherwise the first remote; otherwise absent.
- **`remote_url`:** `git remote get-url <remote>`, sanitized by dropping
  the query and the fragment, and by stripping userinfo:
  - for non-SSH schemes (`https`, `http`, `git`, …), all userinfo is
    stripped, because hosts accept a bare token as the user
    (`https://<token>@host/…`, `x-access-token:<token>@`);
  - for SSH-family schemes (`ssh`, `git+ssh`, `ssh+git`), a bare user
    (`ssh://git@host/x`) is kept and userinfo with a password (`user:pass@`)
    is stripped.

  Userinfo ends at the last `@` before the first `/` that follows any `@`,
  so a password holding an unencoded `@`, `/`, `?` or `#` is still stripped.
  The scp form `git@host:owner/repo.git` and local paths are kept.
  Normalizing to `github:owner/repo` happens at render time, for the
  `at-revision` ref.
- **`branch`:** `git symbolic-ref -q --short HEAD`. Absent when HEAD is
  detached.
- **`head`:** `git rev-parse HEAD`. Absent on an unborn branch.
- **`dirty`:** `git status --porcelain=v1 -z --untracked-files=normal
  --ignore-submodules=dirty --no-renames` prints anything. Changes inside a
  submodule's work tree are ignored, because checking them would run git
  inside it; a submodule checked out at another commit than the one
  recorded is dirty, and its gitlink change is in the diff.
- **Filtered files.** No filter runs (§9.2), so git compares files as they
  are on disk. In a repository with clean filters (git-lfs, nbstripout,
  git-crypt), `dirty` and `diff_sha256` describe the unfiltered working
  tree, and may differ from what `git status` shows; a stat-changed
  filtered file is dirty, and an LFS file's diff carries its real content
  (the cap and the deadline still bound it). A reader that recomputes
  `diff_sha256` must neutralize filters the same way.
- **`diff_unavailable`:** `true` when the tree is dirty and the diff was
  not hashed because it needed objects a partial clone does not have (the
  diff failed under `GIT_NO_LAZY_FETCH`). `diff_sha256` is then absent;
  like the other diff fields it appears only when `dirty`.
- **`diff_sha256`:** the SHA-256 of the stdout of
  `git diff HEAD --binary --no-color --no-ext-diff --no-textconv
  --full-index --no-relative --src-prefix=a/ --dst-prefix=b/ --no-renames
  --diff-algorithm=myers --indent-heuristic --unified=3
  --inter-hunk-context=0 -O/dev/null --ignore-submodules=dirty
  --no-color-moved`, run with the environment and configuration of §9.2. Each flag pins
  what a user's diff configuration could otherwise change, so the same
  tree hashes the same on every machine and in every agent side (the Pi
  extension runs the same command).
  That covers staged and unstaged changes to tracked files. The output is
  hashed as it streams and capped at 64 MiB (`diff_truncated: true` beyond
  that). Absent when the tree is clean. On an unborn branch, the diff is
  `git diff --cached` against the empty tree.
- **`untracked`:** the count of untracked, unignored paths. Their contents
  are not hashed, because doing so could cost seconds; `dirty` covers them.

The daemon checks the context's shape before recording it:

- `repo_root` is absolute;
- no field holds a control character or a Unicode format character (bidi
  controls, zero-width characters);
- `branch` passes git's refname rules;
- `head` is 40 (or 64) lowercase hex digits;
- `remote_url` is already sanitized;
- the diff fields and a non-zero `untracked` appear only when `dirty`.

Only the hash leaves the agent side. Contents, the paths in the diff, and
file names never do.

### 9.2 How

**Capture never runs a program the repository or the user configured.**
Each git command runs with `-C <cwd>`, no standard input, and an
environment cleared down to `PATH`, `HOME`, `XDG_CONFIG_HOME` and `TMPDIR`
plus `GIT_OPTIONAL_LOCKS=0 GIT_TERMINAL_PROMPT=0 LC_ALL=C GIT_PAGER=cat
GIT_NO_LAZY_FETCH=1`.
So no inherited `GIT_DIR`, `GIT_CONFIG_*`, `GIT_EXEC_PATH` or `GIT_TRACE*`
points git elsewhere or injects configuration. Configuration is pinned
through `GIT_CONFIG_COUNT`:

- `core.fsmonitor=false` and `core.hooksPath=/dev/null`;
- `diff.autoRefreshIndex=false`: otherwise a diff over a stat-changed,
  content-clean file refreshes the index, which takes `index.lock`,
  writes the index and runs `post-index-change`, and a kill at the
  deadline could leave the lock behind;
- `core.quotePath=true` and `diff.suppressBlankEmpty=false`, the diff
  settings no flag pins;
- for every filter driver the configuration names, an empty `clean`,
  `smudge` and `process` and `required=false`, so git compares files as
  they are on disk. A configuration naming more than 64 drivers is
  `unavailable`.

**The configuration scan.** Before anything that reads the tree, and
concurrently with `rev-parse --show-toplevel`, capture runs (in the same
environment) `git config -z --get-regexp
'^(filter\..*|extensions\.partialclone|remote\..*\.promisor)$'`, which
runs nothing. Each NUL-terminated entry is a key, a newline and a value.
The key must be UTF-8 (else `unavailable`); the value is compared as bytes,
so a filter command in any encoding is fine. A key `filter.<driver>.<var>`
names a driver (the driver is everything between `filter.` and the last
`.`). The repository is a partial clone when `extensions.partialclone` has
a non-empty value, or any `remote.<name>.promisor` has a value other than
`false`, `no`, `off` or `0` (compared without case). The Pi extension runs
the same scan.

Diffs pass `--no-ext-diff --no-textconv`, and submodules are compared
only by their recorded commit. No command runs a hook, touches the
network, takes a lock, writes the repository or prompts.

**Git version: 2.44 or later.** Older gits ignore, without any error, the
two features capture's safety rests on: `GIT_CONFIG_COUNT` (from 2.31),
which carries every pin above, so an older git would run the repository's
fsmonitor, hooks and filters; and `GIT_NO_LAZY_FETCH` (from 2.44), without
which `git status` or the diff fetches a partial clone's missing objects
and runs the remote's `uploadpack` or ssh. So the agent side reads
`git version` once per git executable (in the capture environment,
outside any repository, within 2 s, kept for the life of the process),
and under a git older than 2.44, one that does not answer, or one whose
version does not parse, every capture is `unavailable` and runs no other
git command. Users of an older git (Debian 11 and 12, Ubuntu 20.04 and
22.04) get no git context until they upgrade. In a partial clone under a
supported git, a diff that would need a missing object fails instead of
fetching, and is recorded as `diff_unavailable`.
`safe.directory` is never passed, so a repository another user owns is
`not-a-repo` (git's ownership check). A test configures every such program
(clean, smudge and process filters, textconv, external diff, fsmonitor,
index hooks) to leave a marker file and checks that none appears.

1. `rev-parse` runs first. If it fails, the outcome is `not-a-repo`.
2. The rest run as concurrent children under a 300 ms deadline (L10). If the
   deadline passes, every child is killed and the outcome is `timeout`.
3. If git is missing, or a command after `rev-parse` fails, the outcome is
   `unavailable`; an empty or missing working directory is `no-cwd`; a
   repository root or name that is not UTF-8 is `invalid`.
4. The upstream remote is `branch.<branch>.remote` (`.` counts as none),
   then `origin`, then the first name `git remote` prints. The diff runs
   once `rev-parse HEAD` has said whether the branch is born.

**The `x-clax-git` header.** Its value is base64url JSON of at most 2 KiB,
in one of two forms:

- a captured context, as in §9.1;
- `{"git_capture":"<outcome>"}`, with no other field, when there is none.
  The outcome is `not-a-repo`, `timeout`, `unavailable` or `no-cwd`.

The agent side validates a context before sending it. A context that fails
the §9.1 checks, or would exceed 2 KiB, is sent as
`{"git_capture":"invalid"}`. The daemon records a header that fails
decoding or the checks as `git_capture: "invalid"`, and never fails the
request over it. With no header, `git` and `git_capture` are both absent.

### 9.3 Who captures, and when

| Agent side | Working directory | Captures on |
|---|---|---|
| MCP shim (`clax-mcp`) | the session's `cwd` (from `registration()`) | each mutating tool call: `publish`, `comments_reply`, `comments_resolve`, `watch` (start and stop), `asset_upload`, `db_*` writes, `delete`, `pin`, `unpin`, working start and stop, the questions spec's `ask`; also registration |
| `clax hook` | hook input `cwd` | session join, hook-driven questions, the Stop hook's working stop |
| Pi extension | `ctx.cwd` | the same tool set as the shim |
| sessionless `/mcp` | none | nothing (`no-cwd`) |
| shell, extension, CLI (owner) | none | nothing |

Read-only tools are recorded as `tool.call` events, but they capture no git.
`open` only shows a page, so it captures none either. Captures are never
cached: an unstaged edit changes nothing git can see cheaply, so a cached
result could describe a state the tree has already left. Only the git
executable is found once, when the shim starts.

So that a capture adds as little as it can to a call, the shim starts it
when the call arrives, on a blocking thread, and it runs while the tool
does its own work (reading files, resolving the artifact). Every request
of the call except a `GET` waits for it; a `GET` carries it once it is
there. The registration's capture runs while the daemon is being found.

### 9.4 Harness session IDs

These were verified against Toolpath's readers at `77dc16a5`. The IDs are
recorded identically for every harness (O6).

| Clax `harness` | Source of the ID (and transcript path) | Provider ID in refs | How Toolpath keys that harness's sessions |
|---|---|---|---|
| `claude` | `CLAUDE_CODE_SESSION_ID` (shim); hook `session_id`; hook `transcript_path` | `claude-code` | `ConversationView.id` = the chain head's `sessionId`; path `path-claude-code-<first 8>`; tool call IDs are `tool_use.id` (in `meta.extra.tool_uses[].id`). The ID Clax sees may belong to a later chain segment, and the transcript path names that segment's file (L11). |
| `codex` | hook `session_id`; hook `transcript_path` when present | `codex` | `session_meta.payload.id`; path `path-codex-<first 8>`; tool calls are `function_call` items with a `call_id`. A fixture test pins the claim that the hook's `session_id` is the rollout's `session_meta.id` (plan Task 13). |
| `pi` | `ctx.sessionManager.getSessionId()`; the session file, if the API exposes it | `pi` | `session.header.id`; tool calls by their tool-call ID |
| `grok` | `GROK_SESSION_ID`; hook `sessionId`; hook `transcriptPath` | `grok` | Toolpath has no Grok reader today. Clax records Grok exactly like the others. |

Rendering maps `claude` to `claude-code`, and every other harness keeps its
name. Gemini is not a Clax harness today. If it becomes one, it records the
same references under `gemini-cli`, Toolpath's provider ID, and a reader
joins its `functionCall`s by §12.3.

## 10. Step modelling and cross-link references

### 10.1 Actors

Every actor string matches the schema pattern
`^(human|agent|tool|ci):[A-Za-z0-9_.-]+(/[A-Za-z0-9_.-]+)?$`. Characters
outside it become `-`, and the original value is kept in the `ActorDef`.

| Actor | String | `ActorDef` |
|---|---|---|
| agent with a harness session ID | `agent:claude-code/<harness session ID>` | `name` "Claude Code" (Codex, Pi, Grok; "Agent" when the harness is unknown); `provider`: anthropic for claude, openai for codex, xai for grok, absent for pi. `identities`: `{system:"clax-session", id:<ULID>}`, `{system:"claude-code-session", id:<harness session ID>}`, `{system:"clax-agent", id:<agent handle>}`, and `{system:"claude-code-transcript", id:<transcript path>}` when known |
| agent without one | `agent:claude-code/clax-<session ULID>` (`agent:unknown/…` when the history no longer names the harness, or names it empty) | the same, minus the harness identities |
| sessionless `/mcp` | `agent:clax-mcp` | `name` "MCP client (no session)" |
| owner | `human:clax-owner` | `name` "Clax owner"; `identities`: `{system:"clax", id:"<install ID>/<owner public ID>"}` (opaque) |
| LAN viewer | `human:clax-viewer/<public ID>` | `name`: the display name (left out under `--no-names`). `identities`: `{system:"clax", id:"<install ID>/<public ID>"}` |
| anonymous viewer | `human:clax-anonymous` | `name` "Anonymous viewer" |
| Clax itself (any system actor) | `tool:clax/<version>`, the version that recorded the event | `name` "Clax"; `identities`: `{system:"clax-build", id:<commit that recorded it>}`. The reason (`ttl`, `rule`, `backfill`, `daemon`) is `meta.clax.system_reason` |

`agent:<provider>/<session>` is the base RFC's own example. The harness
session is who acted, not where the step lives (O7). Clax does not know the
model, so it never writes `agent:<model>`.

### 10.2 Change keys

A step's `change` maps the URI of each object it changed to a `structural`
perspective of type `clax.<kind>`, which carries the §6 body. There is no
`raw` perspective (§1). The URIs are install-scoped and opaque:

```
clax://<install>                                   the install
clax://<install>/a/<artifact>                      artifact
clax://<install>/a/<artifact>/v/<n>                version
clax://<install>/a/<artifact>/t/<thread>           thread
clax://<install>/a/<artifact>/t/<thread>/c/<comment>
clax://<install>/a/<artifact>/d/<collection>/<doc>
clax://<install>/s/<session>                       agent session (a reference target, not a path)
clax://<install>/s/<session>/call/<call ID>        tool call
clax://<install>/q/<question>                      question
clax://<install>/a/<artifact>/asset/<asset ID>     asset
clax://<install>/u/<public ID>                     a person (viewer.claim's key)
clax://<install>/rule/<rule ID>                    live-page rule
clax://<install>/site/<origin>                     joined site, by its key origin
clax://<install>/call/<call ID>                    tool call of the sessionless /mcp route
clax://<install>/step/<step ID>                    a recorded step (a tool call's produced)
clax://<install>/backfill/<table>/<row ID>         a history row the backfill skipped
```

Every segment is percent-encoded down to RFC 3986's unreserved characters,
and each form starts with its own literal, so a URI decodes one way only (a
document's collection keeps its `/` structure; its last segment is the
document ID). The keys:

- the object named above for each kind: a version for `version.publish`
  and `live.snapshot`, a comment for `comment.add`, a thread for the other
  thread kinds, the document for `doc.*`, the asset for `asset.*`, the
  question for `question.*`, the call for `tool.call` and `tool.call_id`;
- a thread's URI nests under its artifact, so it changes when the thread
  moves: a `thread.move` is keyed by the thread under its target, with a
  `thread` ref to its old URI;
- watch, working-record and session events are keyed by the session; a
  batch `thread.send` by the artifact;
- `live.join`, `live.split` and `live.join_answer` by `/site/<origin>`, the
  site's key origin (for an answer, the asked origin).

Browser URLs depend on the port and host, so a step records them as a
`view` ref rather than as a key, and only in an export: journal steps carry
no `view` ref and no `meta.clax.url` (§7.6). A moved thread's `view` is its
destination artifact's.

### 10.3 Example step: an agent publish

```json
{
  "step": {"id": "e000000000311", "parents": ["e000000000310"],
           "actor": "agent:claude-code/3f2c9a1e-5b7d-4c11-9e0a-2d6f8b1c0e44",
           "timestamp": "2026-10-06T14:03:11.731Z"},
  "change": {
    "clax://01JB8Q2WXYZ/a/k3m9q2w8x1ab/v/4": {"structural": {
      "type": "clax.version.publish", "n": 4, "label": "Tighter spacing",
      "note": "Addresses #2 and #3", "title": "Settings page",
      "files": {"index.html": {"sha256": "sha256:9b…", "size": 18342, "content_type": "text/html"}},
      "content_sha256": "sha256:51…", "carried": [], "addresses": ["01JB9Z…", "01JBA0…"], "by_page": false}}
  },
  "meta": {
    "description": "Published version 4 of Settings page",
    "refs": [
      {"rel": "artifact", "href": "clax://01JB8Q2WXYZ/a/k3m9q2w8x1ab"},
      {"rel": "addresses", "href": "clax://01JB8Q2WXYZ/a/k3m9q2w8x1ab/t/01JB9Z…"},
      {"rel": "addresses", "href": "clax://01JB8Q2WXYZ/a/k3m9q2w8x1ab/t/01JBA0…"},
      {"rel": "agent-session", "href": "agent://claude-code/3f2c9a1e-5b7d-4c11-9e0a-2d6f8b1c0e44"},
      {"rel": "transcript", "href": "file:///Users/alex/.claude/projects/-Users-alex-work-app/3f2c9a1e-….jsonl"},
      {"rel": "tool-call", "href": "clax://01JB8Q2WXYZ/s/01JB9…/call/01JBC…"},
      {"rel": "at-revision", "href": "git:github:empathic/app@9c1e5d2b…"},
      {"rel": "view", "href": "http://localhost:7777/a/k3m9q2w8x1ab/v/4"}
    ],
    "clax": {
      "seq": 311, "kind": "version.publish", "install": "01JB8Q2WXYZ",
      "clax_version": "0.3.0", "clax_commit": "abc1234…", "via": "mcp", "backfilled": false,
      "call": {"call_id": "01JBC…", "tool": "publish", "harness_tool": null,
               "args_sha256": "sha256:c6e6f4…", "started_at": "2026-10-06T14:03:11.402Z"},
      "git": {"repo_root": "/Users/alex/work/app", "remote": "origin",
              "remote_url": "https://github.com/empathic/app.git", "branch": "feat/settings",
              "head": "9c1e5d2b…", "dirty": true, "diff_sha256": "sha256:…", "diff_bytes": 18234,
              "untracked": 2, "captured_at": "2026-10-06T14:03:11.512Z"},
      "url": "http://localhost:7777/a/k3m9q2w8x1ab/v/4"
    }
  }
}
```

`meta.clax` also holds the non-null ID columns (`artifact_id`,
`artifact2_id`, `thread_id`, `session_id`, `question_id`, `call_id`,
`origin`), `system_reason` when the actor is a system actor, and
`for_actor` as an actor string when the body names one; the envelope's
`clax_version` and `clax_commit` are the recording build's (`unknown` when
a row lacks them). Owner and viewer steps have the same shape with no
`git`, `call`, `agent-session` or `transcript`. A `tool.call` step's change key is its
`clax://…/call/<ID>` URI, of type `clax.tool.call`.

### 10.4 `meta.refs` vocabulary

The correlation RFC requires readers to keep unknown `rel` values.

| `rel` | From → to |
|---|---|
| `agent-session` | an agent step → `agent://<provider>/<harness session ID>` (the URI form the correlation RFC reads), or `clax://<install>/s/<session>` when there is no harness ID |
| `transcript` | an agent step → `file://<transcript path>` |
| `tool-call` | a step made under a call → `clax://<install>/s/<session>/call/<call ID>` |
| `tool-use` | a `tool.call` or `tool.call_id` step whose harness call ID is known → `agent://<provider>/<harness session ID>/tool/<harness call ID>` |
| `at-revision` | an agent step with `git.head` → `git:<normalized remote>@<head>` (`git:file://<repo root>@<head>` when there is no remote). The owner accepted adding it to the correlation RFC (O5). |
| `artifact`, `thread`, `version`, `question` | a step → the `clax://` objects it concerns |
| `replies-to` | `comment.add` → its thread |
| `addresses` | `version.publish`, `live.snapshot`, `thread.addressed`, and `thread.resolve` with a non-null `addressed_version` → the thread |
| `resolves` | `thread.resolve` → the thread |
| `moved-from`, `moved-to` | `thread.move` → the artifacts |
| `answers` | `question.answer` → the question |
| `produced` | a `tool.call` step → each step it produced (`toolpath:` within the graph, otherwise `clax://…`) |
| `same-change` | a step ↔ its copy in another artifact path |
| `session` | a step whose `session_id` column names a session (a delivery's receiving session, a release's target, a question's asker) → `clax://<install>/s/<session>`, unless that is already its `agent-session` |
| `copied-from` | a `live.snapshot` a move or merge copied → `clax://<install>/a/<source>/v/<n>` |
| `view` | a step or path → its browser URL, in exports only |
| `continues` | a journal segment → the previous segment's file name |

**HEAD is not `meta.source`.** In the correlation RFC, a shared
`meta.source.revision` means two steps describe the same artifact mutation.
A publish made at HEAD `9c1e` is not commit `9c1e`: it was made on top of
that commit, often with uncommitted changes. If HEAD went into
`meta.source`, any correlating reader would link every Clax step to an
unrelated commit as `same-change`. `at-revision` says what is actually true,
and no step carries `meta.source`.

### 10.5 Timestamps

`step.timestamp` is the event's `at`. `git.captured_at` and
`call.started_at` come from the agent side's clock, which is the same
machine's clock.

## 11. Privacy and redaction

The table and the journal hold:

- comment bodies;
- version notes, labels and titles;
- question and answer text;
- viewer display names;
- local paths (`cwd`, `repo_root`, `transcript_path`).

This is the owner's archive (O3). It lives under `~/.clax` (0700, files
0600), and export is owner-only.

| Option | Replaces | With |
|---|---|---|
| `--no-text` | comment `body`, version `note` and `label`, artifact `title` and `description`, question and answer text, working `message`, an anchor's quoted page text (`quote`, `prefix`, `suffix`), an artifact's declared `capabilities` (open-ended configuration), and free-form reasons (`backfill.skip`, `feedback.release`; a question's release or withdrawal `reason` is one of §6.6's fixed phrases, classed safe) | `{"redacted":"text","sha256":"sha256:<hex of the UTF-8 original>"}`; a structured value (questions, answers) hashes its JCS form (§12.2) |
| `--no-names` | viewer `display_name`, `author_name` | `{"redacted":"name"}` (public IDs stay) |
| `--no-paths` | `cwd`, `repo_root`, `transcript_path`, URLs that can carry a query or fragment (an anchor's `route`, a `thread.move`'s `from_url` and `to_url`; a query can hold a token or a search), a git `remote_url` that is a local path (anything without a non-`file` scheme or the scp form `[user@]host:path`), `file://` refs, the transcript identity | `{"redacted":"path","sha256":…}`; `file://` refs, and an `at-revision` naming a local path, are dropped; the transcript identity becomes `{system:"<provider>-transcript-sha256", id:"sha256:…"}`, the only form the schema's identity (`{system, id}` strings) allows |
| any option | a field with no class: one this build does not know, or any field of a kind it does not know | `{"redacted":"unclassified","sha256":…}` |

- **Hashes stay joinable.** Redaction hashes are unsalted (O5), so redacted
  exports still join each other and the original can be verified. A short,
  guessable text can be confirmed by guessing; that is the owner's accepted
  trade.
- **Deny by default.** Every body field of every kind, and every field of
  the objects nested in one (an anchor, an `artifact.update`'s `fields`, an
  actor in `for_actor`, the envelope's `git` and `call`), has a class:
  safe (IDs, enums, counts, hashes, times, origins, URL paths without a
  query or fragment), text, name
  or path. A field with none is hashed under any option, so a field a
  later change adds can never pass a redaction unseen; a test classifies
  every field the recorders and the backfill write.
- **Argument hashes are never redacted.** They are already hashes, and they
  are the join key (§12).
- **The options used** are listed in `graph.meta.clax.redaction`.
- **Never recorded under any option:** diff contents, page content, `doc`
  content, tool argument values (only their hash), cookies, tokens,
  credentials, and URL userinfo.

LAN viewers' actions are recorded with their viewer identity. A LAN viewer
cannot read the journal, the table or an export. A deleted artifact's events
stay (O5). A purge command comes later.

## 12. References for future readers

This section specifies the joins a reader can make. Nothing in Clax performs
them (O6).

### 12.1 What a reader can join

1. **Agent session.** `agent-session` `agent://<provider>/<ID>` names the
   harness session. For Claude Code, the transcript path names the exact
   segment file. A reader that keys sessions by chain head, as Toolpath does,
   resolves the chain from that file.
2. **Git.** `at-revision git:<uri>@<sha>` names the commit the agent's
   working tree was on. A reader matches it against a git-derived step whose
   `meta.source.revision == <sha>` in a path whose `base.uri == <uri>`.
3. **Tool call.** `tool.call` gives the tool, the argument hash and the
   times, and the exact harness call ID where Clax had it. Every step the
   call produced carries the same `call` and a `tool-call` ref.

### 12.2 Canonical argument hash

Both sides must compute the same hash from the same arguments: Clax, when
the call is made, and a reader, from the transcript later.

1. **Input.** The tool's arguments, as one JSON object exactly as the harness
   passed them:
   - MCP: `tools/call` `params.arguments`;
   - Pi: the tool's `params`;
   - Claude Code transcript: `tool_use.input`;
   - Codex rollout: `function_call.arguments`, which is a JSON string and is
     parsed first;
   - Gemini: `functionCall.args`.

   Missing arguments are `{}`. Nothing is dropped, defaulted or normalized:
   the object is hashed as passed.
2. **Canonical form.** JCS, RFC 8785 (the scheme Toolpath already uses for
   signatures):
   - object keys sorted by their UTF-16 code units;
   - no whitespace;
   - numbers in ECMAScript `Number.prototype.toString` form, so `1.0`
     becomes `1` and `1e21` becomes `1e+21`;
   - strings escaped minimally: `\"`, `\\`, `\b`, `\f`, `\n`, `\r` and `\t`,
     other control characters as lowercase `\u00xx`, everything else as
     literal UTF-8.
3. **Hash.** `"sha256:"` followed by the lowercase hex SHA-256 of the
   canonical form's UTF-8 bytes.

**Test vectors.** Both implementations must reproduce these exactly. Plan
Task 12 commits them as `crates/clax-core/tests/toolpath/args-hash-vectors.json`.

| # | Arguments (as passed) | Canonical form | `args_sha256` |
|---|---|---|---|
| 1 | `{}` | `{}` | `sha256:44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a` |
| 2 | `{"id":"k3m9q2w8x1ab","if_version":3}` | `{"id":"k3m9q2w8x1ab","if_version":3}` | `sha256:c6e6f48ad9e94345a81d22b0fa628e053e81e5785a38f0f61965c9196a4bfe93` |
| 3 | `{"b":[1,2,{"z":null,"a":true}],"a":"x"}` | `{"a":"x","b":[1,2,{"a":true,"z":null}]}` | `sha256:dcfe2a3d2102de1d1e5f2a65d1feaf2f69b60bea4c08409297eb9df544f8bb5b` |
| 4 | `{"body":"Line 1\nLine \"2\"\ttab\u001f","thread_id":"01JB9ZK3"}` | `{"body":"Line 1\nLine \"2\"\ttab\u001f","thread_id":"01JB9ZK3"}` | `sha256:f94816d9ea574279d2a70f9e7f981d99437f7cd24dcd9f0f73bb8e501af9ed94` |
| 5 | `{"é":1,"e":2,"z":3}` | `{"e":2,"z":3,"é":1}` | `sha256:9fd95198b233351043c9e13cbc336297f31bc76a19d0ca2ff093cd1172fa47a1` |
| 6 | `{"n":1.0,"m":-0.5,"big":100000000000000000000}` | `{"big":100000000000000000000,"m":-0.5,"n":1}` | `sha256:ed4f6fe44f96cbbe62384feebf79cb9dbc03ff889ae627811e31bd2ea5b2b557` |
| 7 | `{"":2,"😀":1}` (U+E000 and U+1F600 as literal characters) | `{"😀":1,"`U+E000`":2}` (U+1F600 sorts first by UTF-16: D83D < E000) | `sha256:04208f6cdb854e2ab1b07dd3633a39dec854344fe72824cf7f2fdb4e2e33129e` |

How to read the table:

- The escapes in vector 4's arguments are JSON escapes inside the JSON text,
  and the canonical form keeps them.
- Vector 6 needs a real JCS number formatter. Plain `serde_json` would write
  `1.0` and `1e20`.
- Vector 7 differs from code-point order.

**Known limit.** A harness that rewrites arguments between the model and the
tool, for example by filling defaults, produces a transcript hash that does
not match. The join then finds nothing, which is a false negative. The
correlation RFC prefers false negatives to false positives.

### 12.3 The join rule

Within one harness session, a reader pairs transcript tool calls with
`tool.call` records:

1. **Exact ID first.** A `tool.call` or `tool.call_id` with
   `harness_call_id` pairs with the transcript call that has that ID:
   - Claude `tool_use.id`;
   - Pi tool-call ID;
   - Codex `call_id`, whenever a future hook supplies one.
2. **Then by hash.** Group the remaining calls by (bare tool name,
   `args_sha256`). The bare name of a transcript call is its name after the
   last `__` or `.`, and the part before it must contain `clax` (for example
   `mcp__plugin_clax_clax__publish`). Pi uses the `harness_tool` Clax
   recorded. Within a group, pair transcript calls and Clax records in time
   order. A pair is accepted only when the transcript call's timestamp lies
   within [`started_at` − 30 s, `started_at` + 5 s].
3. **Never guess.** An unpaired call stays unpaired. Each call pairs at most
   once.

This rule is the same for Claude Code, Codex, Pi, Gemini and Grok (O4).

## 13. Performance

- **Writer path.** Each audited mutation costs one `INSERT` of about 1 KB in
  its existing transaction, plus a `try_send`. Version writes also hash bytes
  already in memory. The perf budgets
  (`scripts/perf-daemon-budget.json`, `scripts/perf-clients-budget.json`,
  `web/perf/budget.json`) do not change. The plan runs every gate before and
  after.
- **Appender.** It runs on its own thread and uses one reader connection for
  one batch at a time. Under the perf-clients flood it trails the writers and
  then catches up. It never takes the writer.
- **Journal.** The appender never takes the writer, and recording only
  nudges it (a `try_send`); its syncs are plain `fsync` on a quiet second,
  after 5 s of unsynced bytes, at rotation and at shutdown (§7.4).
  Measured with `perf-daemon --quick` (release, the same binary with
  `[toolpath] journal = false`, 3 runs each): every probe within budget
  either way; the slowest big publish per round had a median of 74 ms
  with the journal on against 83 ms off, and cheap-probe p95 under load
  2.19 ms against 2.31 ms. A first start that journals the whole history
  (10× the perf seed, 110,514 events, journal directory removed) serves
  at once and catches up 2.3–3.5 s after it is healthy, with request p95
  of 0.9–2.5 ms meanwhile against 1.3–1.4 ms with the journal off.
- **Tool-call records.** One background POST per tool call, sent after the
  result has returned, plus one `INSERT`. The argument hash is SHA-256 over
  the arguments. A large `publish` HTML string costs well under 1 ms per MiB.
- **Agent side.** Git capture costs at most 300 ms per mutating tool call,
  overlapped with the tool's own work (§9.3). A call that also registers
  the session (the first call while the daemon was down, or a
  re-registration after a 401) waits for the registration's capture and
  then its own: up to about 600 ms. Measured by plan Task 12 on
  the Clax repository (50 captures, release build): p50 59 ms, p95 66 ms,
  max 73 ms. The call ID and argument hash cost 2–3 µs for a small call
  and about 1.5 ms for a 1 MB `publish` HTML string dense with characters
  to escape (SHA-256 alone is 0.4 ms of that); the hash streams the
  canonical form without copying the arguments. Parsing the arguments
  with correctly rounded numbers (`serde_json`'s `float_roundtrip`) is
  what makes the hash match a reader's (§12.2). The Claude Code PostToolUse
  hook costs about 20 ms per call (accepted, O4). Time to usable is measured
  from the link, after the publish has returned.
- **Backfill.** It runs once per home in `Store::open`, after the
  migrations and before the daemon serves (owner ruling T8-1): one
  transaction plans it (a staging table numbered in history order), then
  short transactions of at most 500 rows or 64 MiB of hashed files record
  it, each deleting what it recorded, so an interrupted backfill resumes
  without duplicates and memory holds one batch. The plan's budget is under
  3 s at the perf-seed scale (measured: 0.22 s release, 1.6 s of CPU in a
  debug build); 10× the perf seed (159,506 events) takes 2.1–3.8 s, a
  hash-heavy home of 150 versions and 500 MiB 0.4 s (warm cache), at a peak
  of 16–31 MB of memory. The log names the phase (planning, hashing files,
  recording) with rows done of rows staged and bytes hashed every 2 s and
  after each batch that hashed 16 MiB. The daemon publishes the same in
  `starting.json`, and the client that started it waits while it moves on
  (§14).
- **Export.** It streams (§8.3). Memory holds up to six 64 KiB chunks
  (one filling, four queued, one being sent), the selection's sets, the
  list of paths and the `ActorDef` set of the path being written, never a
  path's steps. It holds one snapshot, so the WAL cannot reset past it and
  grows by the writes made meanwhile; writers never wait on it, since
  every checkpoint is passive; an artifact path reads its two indexes in `seq` order and
  merges them, so SQLite sorts nothing either.

## 14. Failure modes

| Failure | Behaviour |
|---|---|
| Daemon crash mid-append | The partial last line is truncated on start, and appending resumes from the file's last `seq` (§7.5). |
| Power loss | Unsynced lines are re-appended on start. A table row is durable from commit (WAL). |
| Disk full, `EIO` or a permission error on append, create or sync | Truncate back to the last whole line (or read the segment back), log, back off (1 s, doubling to 60 s), and retry the same events. Recording continues. `status` and `doctor` show the error until a write succeeds. |
| Journal directory removed | Re-create it, then open a new segment at `cursor + 1` with a `continues` ref. |
| Segment edited by hand | Rename it `.damaged` and open a new segment (§7.5). |
| Clock steps backwards | Ordering uses `seq`, so it is unaffected. A segment may hold two days. |
| `x-clax-git` or `x-clax-call` malformed or over 2 KiB | Ignored and recorded as `invalid`. The request succeeds. |
| git missing, slow, or not a repo | `git_capture` says which, and the action proceeds (L10). |
| Tool-call POST lost (shim exits) | The events made under the call still carry `call`. `tool.call` is missing, so its end time and outcome are unknown. A reader still has the name, hash and start time. |
| PostToolUse hook finds no match | `tool.call_id` is recorded with `call_id: null`, and the harness ID is still kept. |
| The appender panics | The status says the journal stopped (`journal: false`, `last_error`), `clax doctor` warns, and recording goes on; the next start recovers the journal. |
| Retention cannot remove a segment | The segment is kept and reported as the status's `warning` (and by `clax doctor`), removal is tried again at the next open, and the journal goes on. |
| A disk that never answers (a wedged appender) | Recording goes on; at shutdown the daemon waits at most 1.5 s for the appender and exits without it, and the next start recovers the journal. |
| Unrenderable row (a bug) | A `clax.unrenderable` step is written, and the failure is logged. |
| `journal = false` | The table is still written, and the journal catches up when turned back on. |
| Questions spec not landed | Its kinds simply never occur. |
| A backfill source row cannot be converted | `backfill.skip {table, row_id, reason}` is recorded in its place, and the backfill goes on. |
| Database or disk error during the backfill | The daemon does not start; the next start resumes from the last committed batch, with no duplicate. |
| A long first start (the backfill) | The daemon reports its progress in `starting.json`; the client waits while it moves on (30 s without progress, or 10 minutes in a heartbeat-only phase, gives up), clients queued on the start lock say they are waiting, and `clax status` shows it. |
| A source time that is not text | That row records `backfill.skip`, and the backfill goes on. |
| The schema version cannot be read before an upgrade's swap | The swap is refused and the running daemon kept. |
| An upgrade whose new daemon fails after moving the schema on | The previous build, which cannot open the database, is not restarted; the error says so, and the next start of the new build carries on. |

## 15. Testing

- **Rust, `clax-core`:**
  - record and renderer tests for each kind;
  - redaction;
  - projections: artifact and install paths, linear chains, `same-change` on
    moves, and head correctness;
  - determinism (render twice, get identical bytes);
  - golden histories under `crates/clax-core/tests/toolpath/`.
- **Argument hash:** all §12.2 vectors, in Rust (`clax-core`) and in
  TypeScript (Pi extension), from one shared JSON vectors file.
- **Conformance, against Toolpath's published schema, not an importer
  (O6):**
  - `schema/toolpath.schema.json` is vendored from the Toolpath repo at a
    named commit, read-only, into `crates/clax-core/tests/toolpath/schema/`,
    with a `SOURCE` file naming that commit.
  - The Rust tests write golden documents byte for byte under
    `crates/clax-core/tests/toolpath/expected/`: one export per redaction
    option set (every kind in each), the sealed journal segment, and
    unrenderable steps. The journal segment is sealed by Clax's own
    test-side reader, which implements the JSONL RFC's "Reading JSONL"
    algorithm.
  - The web unit gate validates every golden document against the vendored
    schema with Ajv (draft 2020-12, formats asserted;
    `web/scripts/toolpath-schema.test.ts`). Clax takes no Rust schema
    validator: `jsonschema` changed features under rusqlite and made
    `clax-core` compile twice per run (owner ruling, Task 9 re-review).
  - Renderings the Rust tests cannot fix as goldens (the store's recorders
    and the backfill) are checked in Rust: actor strings against the
    schema's pattern, timestamps as RFC 3339, and the shapes of steps,
    changes, refs and actor definitions.
  - A test re-checks the actor pattern and the timestamp format against the
    vendored schema, so a schema update that changes them fails loudly.
  - Negative controls, in both: broken documents (a bad actor, a bad
    timestamp, a structural perspective without `type`) fail.
  - The events the store's recorders and the backfill write, rendered
    under every redaction option, conform, every field they hold has a
    redaction class, and their texts appear without options and never
    under all of them.
  - Re-rendering is byte-identical across a port change and a newer build.
- **Appender:**
  - `ManualClock` and an injected `JournalFs`;
  - rotation;
  - truncating a partial line;
  - `.damaged` handling;
  - resuming after a crash;
  - fsync coalescing.

  No sleeps; `drain_now()` serves the tests.
- **Server:** `TestServer` tests for each recorded action:
  - kind and actor;
  - git and call headers;
  - tool-call records and `produced`;
  - the PostToolUse match;
  - export selection, ordering and determinism;
  - export is owner-only.
- **Git capture:** fixture repositories (clean, dirty, staged, untracked,
  detached, unborn, no remote, userinfo URL, linked worktree, not a repo). A
  blocking fake `git` tests the timeout.
- **Perf:** the existing gates, with unchanged budgets.

## 16. Review questions: resolved

All five review questions were answered by the owner on 2026-10-06 (O4, O5),
and the scope was corrected twice (O6, O7). No questions are open.
