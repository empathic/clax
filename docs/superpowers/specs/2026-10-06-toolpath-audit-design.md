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
| L1 | **One source of truth: an `audit_events` table** (migration 21, §5.1). A row is written in the same SQLite transaction as the change it records. The journal and every export are pure projections of this table. | The row is atomic with the change, so no event is lost or invented. Crash recovery means "re-project from the last sequence number". Export covers history from before the journal existed (L12). The `EventBus` is not used: its one tap slot is taken, its broadcast drops events on lag, and its events carry no actor. |
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
| L12 | **Migration 21 backfills** existing history into `audit_events`, marked `backfilled: true`. That covers versions (hashing their stored files), threads, comments, resolves, sends, addressed links, moves and sessions. Backfilled steps have no git context, no tool calls and no working records. | Export covers the whole install from day one, and the journal's first segment is complete. It is a one-time pass at daemon start (§13). |
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

### 5.1 Migration 21: the audit journal

Migration 19 is the questions spec's and migration 20 is the inbox's; this
work takes 21 (owner, 2026-10-06). If the numbers shift before landing,
whichever lands later renumbers. No branch edits a migration that has
shipped.

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

ALTER TABLE versions ADD COLUMN content_sha256 TEXT;     -- §5.3; NULL only before backfill
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
  "git": { … } | absent,
  "git_capture": "ok|not-a-repo|timeout|unavailable|no-cwd|invalid" | absent,
  "call": {"call_id": "01JB…", "tool": "publish", "args_sha256": "sha256:…"} | absent
}
```

`git` appears on agent actions whose agent side captured it (O2, §9). `call`
appears on every event made under a tool call (§6.7).

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
| `thread.move` | `from_artifact_id, from_url, to_artifact_id, to_url, move_kind (move\|merge\|unmerge), rule_id, move_id` | each `thread_moves` row (`artifact_id` is the source; `artifact2_id` is the target) |
| `live.rule` | `rule_id, op (set\|delete)`, then the `LiveRule` fields | rule changes |

### 6.4 Watches

| Kind | Body |
|---|---|
| `watch.start` | `target (artifact\|page\|scope), replies_armed, source (direct\|scope), origin?, path?` |
| `watch.stop` | the same |

### 6.5 Working records

| Kind | Body |
|---|---|
| `working.start` | `key, message, thread_ids[]` |
| `working.stop` | `key, reason (explicit\|ttl\|session_end), duration_ms` |

Working state stays in memory. Start and stop are recorded; heartbeats are
not. A TTL expiry is recorded with actor `system:ttl`, and the original agent
goes in `body.for_actor`.

### 6.6 Questions

The questions spec owns the `questions` table and its transitions. This
design records one event per transition, keyed by `question_id`:

| Kind | Body |
|---|---|
| `question.ask` | `source (ask\|hook), tool_use_id, questions (the stored questions_json)` |
| `question.answer` | `answers (answers_json), answered_via` (actor: the owner) |
| `question.decline`, `question.release`, `question.withdraw` | `reason?` |

The inbox ("Waiting on you") is a view of open questions, and has no events
of its own. Question and answer text follows the comment text rules (§11).
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
end time and the outcome. The daemon records `tool.call`, filling `produced`
from the `call_id` index. A `tool.call` belongs to the artifact its produced
events touched, or that its arguments named (the shim resolves the
artifact). Otherwise it belongs to the install path.

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
| `session.join` | the same, when a hook joins a harness session ID to a registered session |
| `session.end` | `reason` |

These are steps in the install path. A session is an actor, never a path
(O7).

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
extension credential grants, or daemon start and stop (`daemon.log` has
these).

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

A segment rolls at the first event whose `at` falls on a new UTC day, or
when the file would exceed `[toolpath] segment_max_mb` (default 64).
`[toolpath] journal_retain_days` (default 0, meaning forever) removes whole
segments older than that at rotation time. It never touches the table.

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
  before that actor's first step. Each definition is complete, because the
  JSONL RFC overwrites rather than merges.
- **`Head` and `PathClose`:** written only when a segment closes, at
  rotation or graceful shutdown. While a segment is open, its single-tip
  linear chain makes the head unambiguous.
- **`PathMeta`:** never written.
- **Parents:** each `Step`'s only parent is the segment's previous step.
  Segments link by the `continues` ref, because the base RFC allows no
  parents across paths.

### 7.3 Writing

The appender holds the only handle on the open segment. For each batch it:

1. reads the rows with `seq > cursor` (at most 512) through `with_read`;
2. renders each row as one complete line ending in `\n`;
3. calls `write_all` once for the whole batch;
4. advances the cursor.

A row that cannot be rendered (a bug) is logged and becomes a `Step` of type
`clax.unrenderable` that carries its `seq` and `kind`. The chain never
breaks.

### 7.4 Durability

`sync_data` runs:

- after a batch, when more than 1 s has passed since the last sync;
- on a 1 s timer while unsynced bytes exist;
- before rotation closes a segment;
- at shutdown.

The directory is fsynced after a segment is created. The JSONL RFC leaves
fsync to the writer, and this is Clax's policy. The worst case on power loss
is the last second of lines, which are re-appended on the next start.

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
4. If the last complete line does not parse, which means something other than
   Clax wrote it, rename the file to `<name>.damaged`. It is never deleted or
   linked. Open a new segment after the highest `seq` that parses in the
   damaged file.
5. Append every row with `seq > cursor`. Rendering is a pure function of the
   row, so lost lines come back byte-identical.

There is no cursor file, so the cursor can never disagree with the file, and
no step is ever duplicated.

### 7.6 Determinism

The renderer is a pure function of (row, install ID, render options):

- `serde_json`'s default `Map` is a `BTreeMap`, so keys come out sorted;
- there are no clock reads and no `HashMap` iteration;
- the output is compact.

### 7.7 The journal and text

The journal records text (O3). `[toolpath] journal_text = false` makes the
appender render with `--no-text` rules (§11). Export always reads the table
and is unaffected.

## 8. Export

### 8.1 CLI

```
clax toolpath export [--artifact <ID|URL>]... [--live <page URL>]...
                     [--by-session <Clax session ID | harness session ID>]...
                     [--since <RFC 3339 | YYYY-MM-DD>] [--until <…>]
                     [--shape artifacts|journal] [--format json|jsonl]
                     [--no-text] [--no-names] [--no-paths]
                     [--pretty] [-o <file>]
clax toolpath status [--json]
```

- **Selection:** no selector means the whole install.
  - `--artifact` and `--live` choose artifacts.
  - `--by-session` keeps only steps whose actor is that agent session, plus
    the owner and viewer steps on the same artifacts. It filters steps; it
    does not change the shape (O7).
  - Selectors of one kind union; selectors of different kinds intersect.
  - `--since` is inclusive and `--until` exclusive, both on `at`. A bare
    date means 00:00 UTC.
- **`--shape artifacts`** (the default): the paths of §8.2.
- **`--shape journal`:** one linear audit-trail path of every selected step,
  shaped like a segment, with ID `clax-export-<digest>`.
- **`--format jsonl`:** allowed only when the result is exactly one path,
  which means `--shape journal` or a single selected artifact. Otherwise the
  command fails with an error naming `--shape journal`, because the JSONL RFC
  puts one path in each file.
- **Output:** stdout by default. With `-o`, the export is written to
  `<file>.tmp-<ULID>` and then renamed.

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
- **Refs to objects outside the selection** keep their `clax://` form. Refs
  inside the graph also get a `toolpath:` form.
- **The graph ID's digest** is the first 12 hex characters of the SHA-256 of
  the canonical selection JSON.
- **Correlation:** none, so the graph carries no `correlates` marker (O6).

### 8.3 Daemon routes

`GET /api/toolpath/export?<the CLI options as query parameters>`:

- **Who:** owner only (the daemon token or the owner cookie). A LAN viewer
  gets 403.
- **Response:** streamed `application/json`, or `application/x-ndjson` for
  JSONL.
- **Consistency:** the export reads one snapshot, in a single read
  transaction.
- **Order:** artifact paths in artifact ID order, then the install path;
  steps in `seq` order.
- **Determinism:** the same database and the same arguments give
  byte-identical output. The export has no `exported_at`.

`GET /api/toolpath/status` returns:

```json
{"journal": true, "dir": "…", "segment": "…", "cursor": 5930, "newest_seq": 5931, "lag_ms": 12, "last_error": null}
```

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
- **`dirty`:** `git status --porcelain=v1 -z --untracked-files=normal`
  prints anything.
- **`diff_sha256`:** the SHA-256 of the stdout of
  `git diff HEAD --binary --no-color --no-ext-diff --no-textconv --full-index`.
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

Each git command runs with `-C <cwd>` and the environment
`GIT_OPTIONAL_LOCKS=0 GIT_TERMINAL_PROMPT=0 LC_ALL=C GIT_PAGER=cat`:

1. `rev-parse` runs first. If it fails, the outcome is `not-a-repo`.
2. The rest run as concurrent children under a 300 ms deadline (L10). If the
   deadline passes, every child is killed and the outcome is `timeout`.
3. If git is missing, the outcome is `unavailable`.

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
Captures are never cached: an unstaged edit changes nothing git can see
cheaply, so a cached result could describe a state the tree has already
left.

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
| agent with a harness session ID | `agent:claude-code/<harness session ID>` | `name` "Claude Code"; `provider`: anthropic for claude, openai for codex, xai for grok, absent for pi. `identities`: `{system:"clax-session", id:<ULID>}`, `{system:"claude-code-session", id:<harness session ID>}`, `{system:"clax-agent", id:<agent handle>}`, and `{system:"claude-code-transcript", id:<transcript path>}` when known |
| agent without one | `agent:claude-code/clax-<session ULID>` | the same, minus the harness identities |
| sessionless `/mcp` | `agent:clax-mcp` | `name` "MCP client (no session)" |
| owner | `human:clax-owner` | `identities`: `{system:"clax", id:"<install ID>/<owner public ID>"}` (opaque) |
| LAN viewer | `human:clax-viewer/<public ID>` | `name`: the display name (redactable). `identities`: `{system:"clax", id:"<install ID>/<public ID>"}` |
| anonymous viewer | `human:clax-anonymous` | none |
| Clax itself | `tool:clax/<version>` | `identities`: `{system:"clax-build", id:<commit>}` |

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
```

Browser URLs depend on the port and host, so each step records them as a
`view` ref rather than as a key.

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

Owner and viewer steps have the same shape with no `git`, `call`,
`agent-session` or `transcript`. A `tool.call` step's change key is its
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
| `view` | a step or path → its browser URL |
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
| `--no-text` | comment `body`, version `note` and `label`, artifact `title` and `description`, question and answer text, working `message` | `{"redacted":"text","sha256":"sha256:<hex of the UTF-8 original>"}` |
| `--no-names` | viewer `display_name`, `author_name` | `{"redacted":"name"}` (public IDs stay) |
| `--no-paths` | `cwd`, `repo_root`, `transcript_path`, `file://` refs, the transcript identity | `{"redacted":"path","sha256":…}`; `file://` refs are dropped |

- **Hashes stay joinable.** Redaction hashes are unsalted (O5), so redacted
  exports still join each other and the original can be verified. A short,
  guessable text can be confirmed by guessing; that is the owner's accepted
  trade.
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
- **Tool-call records.** One background POST per tool call, sent after the
  result has returned, plus one `INSERT`. The argument hash is SHA-256 over
  the arguments. A large `publish` HTML string costs well under 1 ms per MiB.
- **Agent side.** Git capture costs at most 300 ms per mutating tool call.
  The expected cost is 10–40 ms on a warm repository, because the commands
  run concurrently; plan Task 12 measures it. The Claude Code PostToolUse
  hook costs about 20 ms per call (accepted, O4). Time to usable is measured
  from the link, after the publish has returned.
- **Backfill.** It runs once, in migration 21's transaction. The plan's
  budget is under 3 s at the perf-seed scale, with progress logged every
  10,000 rows.
- **Export.** It streams. Memory is bounded by one path's steps plus the
  `ActorDef` set.

## 14. Failure modes

| Failure | Behaviour |
|---|---|
| Daemon crash mid-append | The partial last line is truncated on start, and appending resumes from the file's last `seq` (§7.5). |
| Power loss | Unsynced lines are re-appended on start. A table row is durable from commit (WAL). |
| Disk full or `EIO` on append | Log, back off (1 s up to 60 s), and retry the same batch. Writes continue. `status` and `doctor` show the error. |
| Journal directory removed | Re-create it, then open a new segment at `cursor + 1` with a `continues` ref. |
| Segment edited by hand | Rename it `.damaged` and open a new segment (§7.5). |
| Clock steps backwards | Ordering uses `seq`, so it is unaffected. A segment may hold two days. |
| `x-clax-git` or `x-clax-call` malformed or over 2 KiB | Ignored and recorded as `invalid`. The request succeeds. |
| git missing, slow, or not a repo | `git_capture` says which, and the action proceeds (L10). |
| Tool-call POST lost (shim exits) | The events made under the call still carry `call`. `tool.call` is missing, so its end time and outcome are unknown. A reader still has the name, hash and start time. |
| PostToolUse hook finds no match | `tool.call_id` is recorded with `call_id: null`, and the harness ID is still kept. |
| Unrenderable row (a bug) | A `clax.unrenderable` step is written, and the failure is logged. |
| `journal = false` | The table is still written, and the journal catches up when turned back on. |
| Questions spec not landed | Its kinds simply never occur. |

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
  - Every golden export is validated against it with the `jsonschema`
    dev-dependency.
  - Every golden journal segment is first sealed by Clax's own test-side
    reader, which implements the JSONL RFC's "Reading JSONL" algorithm, and
    then validated against the schema.
  - A test re-checks the actor pattern and the timestamp format against the
    vendored schema, so a schema update that changes them fails loudly.
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
