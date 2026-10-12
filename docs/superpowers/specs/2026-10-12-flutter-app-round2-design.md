# Clax app, round 2: design addendum

Date: 2026-10-12. Branch `flutter-app`. Extends
`docs/superpowers/specs/2026-10-12-flutter-app-design.md` (the base spec,
"§n" below refers to it unless marked "R§n" for this document). Status: the
owner's round-2 decisions (R§1) are binding; everything else is this
design's, and its open points are qualifier records tagged
`status:needs-decision` on this file (R§13).

## R§1. Owner decisions (binding)

From `.superpowers/sdd/flutter-app/progress.md`, "Round 2":

- RO1 New app: the app builds live; the output streams and the page renders
  as its HTML arrives.
- RO2 Changes: narration in the Working chip; the current version stays
  usable until the change lands.
- RO3 Background: a progress notification while a run goes, then "<title>
  is ready", tapping it opens that app; a foreground service during runs;
  a run cut by the background network block resumes instead of failing.
- RO4 Agent: a tool loop that checks its work. Tools read files and
  documents, edit or write, publish, render the page and inspect a
  screenshot plus console errors, fix, repeat. Capped by turns and cost;
  each step streamed as narration. On the desktop `claude -p` gets the same
  tools over MCP. This supersedes the single structured call (base §3.6
  `EditResult`); P1's `steps` schema field is dropped, P1 keeps the
  streaming transport and progress plumbing.
- RO5 Gesture: a floating comment button (tap: comment mode; it also opens
  the threads). The two-finger swipe stays as a shortcut.
- RO6 Selection: hold, then drag. Over text the drag selects a text range;
  elsewhere it draws an area box. Release to comment.
- RO7 Clax machinery, native, in stages: (1) an inbox across apps, (2) the
  agent's questions to the owner, (3) version history and diff.
- RO8 Straightforward: a thread detail view with reply; "Comment on the
  whole app"; a visible Retry on a failed change; a Settings toggle for
  voice auto-start (default on, as today) with a haptic and a start/stop
  sound.

In flight when this was written, integrated here and not redone:

- **P1** (lib/agent): `AgentRequest.onProgress`, `progress.dart`
  (`PartialFile`, `PartialEdit`, `OutputTokens`), `json_stream.dart`
  (tolerant incremental JSON scanner), `message_stream.dart` (SSE decoder
  and a `MessageStream` that rebuilds `tool_use` input from
  `input_json_delta`), streaming in `MessagesApiBackend` and
  `ClaudeCliBackend` (`stream-json`).
- **P4** (android/, lib/platform, lib/app.dart): `BackgroundWork` (a
  `dataSync` foreground service, progress and ready/failed notifications,
  `opened` taps), `BackgroundRuns` (`begin`/`step`/`end`), `AppPresence`,
  `ResumingBackend` (waits out a background network cut and runs again).

P2 (live making preview) and P3 (Working-chip narration) are re-planned
here on the tool loop: the preview comes from the streamed input of the
`write_file` tool, narration from tool calls and short assistant text.

## R§2. Architecture

```
            ┌──────────── Toolbox (one per run, Dart) ────────────┐
 Runner ──► │ Draft: files, title, capabilities, staged db ops,   │──► ClaxApi ──► daemon
 (loop)     │        doc pins, render state                       │      (drafts, docs, publish,
            │ Budget: turns, renders, tokens, cost                │       threads, questions)
            │ Tools: list/read/write/str_replace/delete_file,     │
            │  set_meta, db_*, db_write, render, ask_owner,       │──► Inspector (WebView in the
            │  finish, give_up                                    │      app, loads the draft)
            └─────────────────────────────────────────────────────┘
 Runner = MessagesApiRunner (phone: the Dart loop over the Messages API)
        | ClaudeCliRunner   (desktop: claude -p, the same Toolbox served as
                             an MCP server on loopback by the app)
        | ScriptedRunner    (tests: scripted tool calls against the real Toolbox)
```

- **One Toolbox, two transports.** Every rule (what a tool may do, the
  publish gate, the caps, the data-safety rules of base §3.6 and the change
  ledger) lives in the Toolbox. A runner only moves tool calls in and
  results out, so the phone and the desktop behave the same, and the
  deterministic tests drive the real Toolbox.
- **Drafts.** The agent edits a draft, never the live version. The live
  version stays usable (RO2). Rendering serves the draft through the daemon
  as a hidden **draft artifact** (R§5), so the page runs under the real page
  contract, bridge and `db`, against a copy of the app's documents.
- **One publish per run**, at `finish` (R§4.5). Nothing half-done is ever a
  version.
- **AgentLoop and FeedbackWorker stay** the entry points (`newApp`,
  `change`, recovery, queue merging, `WorkerEvent`s); their body is now a
  runner over a Toolbox instead of one backend call.

## R§3. The tool set

Tool names are snake_case, fixed, and declared in one fixed order every
turn (cache prefix, R§6.3). Schemas are strict (`additionalProperties:
false`, every property required, optional ones `anyOf [..., null]`) except
the two streamed write tools (R§6.2). Every tool result is JSON text; the
render result adds one image. A refused call is a `tool_result` with
`is_error: true` and a one-sentence reason the model can act on; it never
throws out of the loop.

| Tool | Input | Result | Notes |
|---|---|---|---|
| `list_files` | `{}` | `{base_version, files: [{path, bytes, lines}], title, capabilities_json}` | The draft's view. |
| `read_file` | `{path, from_line: int\|null, max_lines: int\|null}` | `{path, from_line, lines: [..], total_lines, truncated}` | Text files only; at most 1,500 lines or 64 KiB per call. |
| `write_file` | `{path, content}` | `{path, bytes, lines, created}` | Create or replace in the draft. Streamed (eager). Path rules of base §8 (`validateResult`'s). At most 2 MiB per file, 64 files. |
| `str_replace` | `{path, old_str, new_str}` | `{path, replaced: 1}` | `old_str` must occur exactly once (`apply.dart`'s `EditMiss` rule), else an error naming the count. Streamed (eager). |
| `delete_file` | `{path}` | `{path, deleted: true}` | Never `index.html`. |
| `set_meta` | `{title: string\|null, capabilities_json: string\|null}` | `{title, capabilities_json}` | A capability declaration is the full set, as `publish`'s; it is parsed here and checked by the daemon at the first render. A new app needs a title before `finish`. |
| `db_collections` | `{app: string\|null}` | `{app, collections: [{path, count}]}` | `app` null: this app; else another app's ID from the prompt's list. |
| `db_list` | `{collection, app, limit: int\|null}` | `{docs: [{path, version, data}], truncated}` | At most 50 documents, 32 KiB. |
| `db_get` | `{path, app}` | `{exists, doc}` | |
| `db_query` | `{collection, app, where: [[field, op, value_json]]\|null, order_by, limit}` | as `db_list` | The daemon's query (`db_query` in `docs/contract.md`). |
| `db_write` | `{ops: [{op: "set"\|"update"\|"delete", collection, doc_id, data_json: string\|null}]}` | `{staged: n}` | This app only. **Staged**, not written: applied to the draft's document copy for rendering and to the live app at `finish`. An op on a document that exists must name one this run read (`db_get`/`db_list`/`db_query`, or the prompt's samples); its pin is the version read. At most 200 ops per run. |
| `render` | `{file: string\|null}` | image + `{console_errors: [..], failed_requests: [..], text: "...", title, height, settled_ms, docs_copied, docs_truncated}` | R§5. At most 4 per run. |
| `ask_owner` | `{questions: [...]}` (the `ask` shape of `docs/contract.md`) | `{status, answers}` | R§8. At most 1 per run. |
| `finish` | `{reply, resolve: bool}` | `{published: version\|null, replied: [thread]}` | R§4.5. Ends the run. |
| `give_up` | `{reason}` | `{}` | Nothing is published; the threads get "Couldn't make that change: <reason>" and stay open. Ends the run. |

A change's threads are named in the first user message; there is no
separate reply or resolve tool: the reply and the resolve travel with the
version in `finish`, so the change ledger (`lib/agent/ledger.dart`) can
replay them after a crash exactly as today.

Other apps' documents are read with `db_*` and an `app` ID; they are read
at the owner's level (the agent is the owner's), never written, and
`data/users/<id>/` documents are left out, as in base §3.8.

## R§4. The run

### R§4.1 First message

- **System prompt** (cached): `assets/prompt/system.md` rewritten for the
  tool loop, plus the synced contract copies (base §3.6, unchanged sync
  script). It says how to work: narrate one short present-tense line before
  each group of tool calls ("Adding the weekly total"); for a new app, write
  `index.html` once, render once, fix only what the render shows, finish;
  for a change, edit with `str_replace`, render, finish; read a document
  before writing it; keep the reply to one or two sentences; ask the owner
  only when the request is genuinely ambiguous and a wrong guess would cost
  their data or a rewrite.
- **User message**: the request, or the comments (each with its anchor line
  and the clip image of the first, as base §3.6); the app's title, its file
  list, and **the text of every file inline while they total at most 24
  KiB** (saving the read turn that most changes would spend), else
  `index.html` only up to 24 KiB; the app's collections with up to two
  samples each and the other apps' summaries (base §3.8 limits).

### R§4.2 The loop (MessagesApiRunner)

```
messages = [user(first)]
loop:
  check cancel; check caps (R§4.4)
  turn = backend.turn(tools, system, messages)        # streamed (R§6)
  if turn.stop == refusal → fail("The model declined: <category>")
  if turn.stop == max_tokens → resend the turn once with max_tokens 64000,
                               else fail; never run a cut-off tool_use
  messages += assistant(turn.content)                  # verbatim, thinking included
  if no tool_use in turn → messages += user(text "Call finish or give_up.") ; continue (counts a turn)
  results = [toolbox.call(u) for u in turn.tool_uses]  # in order, one at a time
  messages += user(results [+ budget note text, R§4.4])
  if toolbox.ended → return toolbox.outcome
```

History is **append-only**: earlier turns are never edited, thinking
blocks go back byte for byte (preserved thinking), budget notes are
appended after the tool results, and the tool list never changes within a
run.

### R§4.3 ClaudeCliRunner (desktop)

The app serves the run's Toolbox as an MCP server (Streamable HTTP, JSON
responses, `initialize`, `tools/list`, `tools/call`) on `127.0.0.1:<free
port>` with a random bearer token, one server per run, closed at its end.
The render result goes back as MCP `image` content beside a `text` item.

```
claude -p --output-format stream-json --verbose --include-partial-messages
  --mcp-config <scratch>/mcp.json --strict-mcp-config
  --tools "" --allowedTools mcp__clax_app__list_files,…,mcp__clax_app__give_up
  --safe-mode --model <model> --effort low --no-session-persistence
  --system-prompt-file <scratch>/system.md --max-budget-usd <cost cap>
```

`mcp.json` is `{"mcpServers": {"clax_app": {"type": "http", "url":
"http://127.0.0.1:<p>/mcp", "headers": {"Authorization": "Bearer <t>"}}}}`.
The request goes on stdin, the environment is scrubbed as today (base
§3.6), progress comes from P1's stream-json parsing (assistant text →
narration; `tool_use` with partial input → `PartialFile`). The run's
outcome is the Toolbox's (`finish`/`give_up`), never claude's text: once a
terminal tool returns, the child gets 10 s to exit and is then killed; a
child that exits with no terminal call is `give_up("the agent stopped
without finishing")`. Caps (R§4.4) are the Toolbox's: past them every tool
answers the cap error, and the child is killed after two more calls.

This is not `clax mcp`: R§13 Q1 and the alternative record on this section
say why.

### R§4.4 Caps

| | New app | Change |
|---|---|---|
| Model turns | 14 | 12 |
| `render` calls | 4 | 4 |
| Output tokens (sum) | 60,000 | 40,000 |
| Cost (from `usage` × the price table) | $1.00 | $1.00 |
| Wall clock (injected clock) | 6 min | 5 min |

- The price table (`lib/agent/pricing.dart`) holds input, output, cache
  write and cache read prices for the three offered models
  (`claude-haiku-5-5` $0.10/$0.50 per MTok, `claude-sonnet-5-5` $2/$10,
  `claude-opus-5-5` $4/$20; cache reads 0.1× input, writes 1.25×).
- **Soft limit.** When two turns remain, or 75% of the output tokens or the
  cost is spent, the next tool-result message ends with a text block:
  "Budget: <n> turns left. Call finish with what works now, or give_up."
- **Hard limit.** The loop stops calling the model and ends the run as
  `give_up("it ran out of its budget: <which cap>")`. Nothing is published,
  even a clean draft (alternative record). The failed change shows Retry
  (R§9.5), which runs again with a fresh budget.
- Messages API runs also send `output_config.task_budget` (`{type:
  "tokens", total: <output cap>}`, beta `task-budgets-2026-03-13`) so the
  model paces itself.

### R§4.5 `finish`, the only write to the live app

`finish` refuses (an error result the model can act on) when:

- the draft has a write after its last `render`, or never rendered while
  it changed a file (**every finish that publishes follows a render of
  that exact draft**, R§13 Q2);
- the last render reported uncaught errors (`console_errors` of kind
  `error` or `unhandledrejection`) and no write followed it;
- a new app has no title or no `index.html`.

Otherwise it does what `AgentLoop._change` does today, moved into
`lib/agent/tools/finish.dart` unchanged in substance:

1. Threads with a newer viewer comment are left out of `addresses` and of
   the reply (base §3.6, `_newerComments`).
2. When files, title or capabilities changed: write the `ChangeRecord`
   (base, addresses, answered comment IDs, reply, resolve, staged db ops,
   pins), then `publish` with `if_version: base` and `addresses`.
   - 409 conflict: re-read the latest version. If, for every path the
     draft changed, the latest file equals the draft's base file, carry the
     draft over the latest and publish again once; else reset the draft to
     the latest version, keep the run going, and answer the tool call "The
     app changed while you worked (now v<n>); your edits were dropped:
     re-apply them". A second conflict fails the run.
3. Write the staged db ops in one batch, each pinned to the version read
   (`_writeDocs`: one re-read and retry on a document conflict). A refused
   batch restores the version before the change (`restoreVersion`) and
   fails with "its data couldn't be updated: <reason>" (base rule).
4. Reply on the threads, resolve when `resolve`, acknowledge, clear the
   working mark.
5. Delete the draft artifact.

A new app's `finish` is `createArtifact` (session header, title,
capabilities, files) then the db batch, as `newApp` today.

Recovery is unchanged: the worker's `settle` reads the ledger after an
interruption (base §3.6, record d359fbc3's rule).

### R§4.6 Cancellation

A run's `cancelled` future (base `AgentRequest.cancelled`) aborts the turn
in flight (HTTP abort or child kill), makes every later tool call answer
"cancelled", withdraws an open `ask_owner` question, and deletes the draft
artifact. `finish` checks it before step 2; once step 2 starts the run
completes (as "before the publish" today). Outcomes map to today's
`ChangeOutcome`s: `ChangeLanded`, `ChangeAnswered` (finish with no file
change), `ChangeFailed` (give_up, caps, errors), `ChangeCancelled`,
`ChangeInterrupted` (a daemon call without an answer), `ChangeDropped`.

### R§4.7 Background (P4)

- Each turn is one request; a background network cut fails a turn before
  any tool of it ran, so sending that turn again is a transport retry.
  `ResumingBackend` becomes `ResumingTurns`, wrapping the turn backend: the
  same rule (the app left the foreground during the turn, the error is a
  network cut, at most 3 resumes), per turn, so a resumed run keeps every
  turn before the cut.
- Narration steps feed `BackgroundRuns.step`; the notification shows the
  latest step. A change's run that lands posts "<title> changed"; a new
  app's "<title> is ready".
- `ask_owner` while the app is in the background posts a notification
  "<title> has a question"; tapping it opens the questions sheet (R§10.2).

## R§5. Drafts and `render`

### R§5.1 Draft artifacts (server)

`POST /api/drafts` (token) with `{of: <aid>|null, title, capabilities,
files}` creates a **draft artifact**: an ordinary artifact row with
`draft_of` (the app's ID, or empty for a new app) and `hidden = 1`
(migration 25, `ALTER TABLE artifacts ADD COLUMN …`). It answers
`{draft: {id, of, version, docs_copied, docs_truncated}}`.

- **Documents.** For `of`, the app's documents are copied into the draft at
  the owner's level, `data/users/<id>/` excluded, up to 2,000 documents or
  8 MiB (`docs_truncated` says when the copy stopped). Writes the page
  makes while rendering land in the copy only.
- **Files.** Later renders publish to the draft as to any artifact (`POST
  /api/artifacts/<draft>/versions`, token, no session, so no watch).
- **Reads of other apps.** A draft reads another app's documents with the
  grants of `of` (the reader check maps a draft to `of`, read-only), so a
  render never asks for consent.
- **Hidden.** A draft is left out of `GET /api/artifacts`, the gallery
  topic, `/api/events` unless its ID is named in `artifact=`, the `list`
  tool, inbox items, sessions' watches and feedback; a thread on it is 403
  `draft`.
- **Lifetime.** `DELETE /api/drafts/<id>` removes it, its versions' files
  and its documents for good. `GET /api/drafts` lists them. The daemon
  removes drafts older than 1 hour at start and hourly; the app removes its
  own at start.

### R§5.2 The inspector (app)

`render` pushes the draft (create on first render, publish after), applies
the staged db ops to the draft's copy (best effort, unpinned), then asks
the **Inspector** to load `<base>/a/<draft>?app=1&inspect=1`:

- The `WebViewInspector` is a WebView kept in the app's root `Stack`,
  under an opaque cover, at the size the app is shown in (the app screen's
  size; 390 × 844 logical px when no app screen is up). It is never
  `Offstage` (a WebView not laid out does not run), and it is told it is
  visible (`claxHost.setVisible(true)`), since a hidden WebView document
  stops `requestAnimationFrame` (record 9e96bca7).
- In `inspect=1` mode the shell shows the page with no comment UI, follows
  no events, answers every consent prompt "Don't allow" without showing it,
  and on `claxHost.inspect({settleMs, maxBytes})` asks the bridge for an
  inspection. The bridge waits until the DOM has been quiet for 300 ms (a
  `MutationObserver`, timers only, no `requestAnimationFrame`), at most
  `settleMs` (3,000), then returns:
  - `png`: the viewport rendered with the clip renderer (`clip.ts`,
    modern-screenshot) at scale 1, re-rendered at half scale while over
    `maxBytes` (1 MiB);
  - `errors`: a ring buffer (20) of `error` events, `unhandledrejection`s
    and `console.error` calls since the page's first script (the bridge
    installs it first, R§11 W6);
  - `failed`: resources that failed to load (`error` events on elements);
  - `text`: a 3 KiB outline of the visible page (headings, buttons,
    labels, inputs with placeholders, list lengths) from `text-walk.ts`;
  - `title`, `height`.
- The shell posts it to the app as host message `inspection` (with the
  nonce). One render at a time; a render that gets no `ready` in 10 s or no
  `inspection` in 10 s more returns an error result ("the page did not
  load: <reason>") the model can act on.

## R§6. Streaming, narration, preview

### R§6.1 Turn requests (Messages API)

```json
{"model": "<model>", "max_tokens": 32000, "stream": true,
 "system": [{"type": "text", "text": "<system>", "cache_control": {"type": "ephemeral"}}],
 "tools": [ …fixed order… ],
 "tool_choice": {"type": "auto"},
 "thinking": {"type": "adaptive"},
 "output_config": {"effort": "low", "task_budget": {"type": "tokens", "total": 60000}},
 "cache_control": {"type": "ephemeral"},
 "messages": [ … ]}
```

Headers as today plus `anthropic-beta: task-budgets-2026-03-13`; for
`claude-sonnet-5-5` and `claude-opus-5-5` also
`server-side-fallback-2026-07-01` with `"fallbacks": "default"` (a refusal
falls back server-side; Haiku has no fallback). Forced `tool_choice` stays
unused (400 on the 5.5 models). A `refusal` stop fails the run with its
category.

### R§6.2 Streamed tool input

`write_file` and `str_replace` carry `eager_input_streaming: true` and are
not `strict` (the API does not validate eagerly streamed input), so the
Toolbox validates their parsed input against the schema itself and answers
a mismatch as an error result. While their input streams, P1's
`JsonStreamScanner` on the `input_json_delta`s yields the growing
`content` (`PartialFile(path, text)`) and `new_str` (`PartialEdit`).

### R§6.3 Caching and cost

Order is `tools` → `system` → `messages`; tools and system never change in
a run; the system block carries a breakpoint and the top-level
`cache_control` moves the second one to the end of the growing history, so
each turn reads the previous turns from cache. `usage` (input, cache
write, cache read, output) is summed per run and recorded with the run's
timing record (`agent-new`, `agent-change`: turns, tokens, cost).

### R§6.4 Narration

`AgentProgress` gains (in P1's `progress.dart`):

- `Narration(text)`: the first sentence (at most 90 characters) of each
  assistant text block, once it ends with `.`, `…` or a newline, or at
  `content_block_stop`;
- `ToolStarted(name, label)`: at each `tool_use` block start, with a label
  from the tool and its first input fields ("Writing index.html",
  "Changing index.html", "Reading your runs", "Checking how it looks",
  "Asking you a question", "Publishing");
- `Inspected(png, errors)`: after each `render`.

The latest of `Narration`/`ToolStarted` is the run's step: the making
screen, the Working chip and the notification show it.

### R§6.5 The two numbers

The defended numbers keep their definitions (base §1). New measured
points: **say→first-output** (submit to the first preview frame of
streamed HTML) and **send→first-step** (Send to the first narration step
in the chip). Targets on `claude-haiku-5-5` at effort low, checked by the
live run (R§12): say→first-output ≤ 4 s; say→usable ≤ 35 s;
send→first-step ≤ 4 s; long-press→landed ≤ 20 s for a one-element change.
Speed comes from the prompt (one write, one render), inline files (no read
turn), the cached prefix, and streaming the preview from the first bytes
of `index.html`.

## R§7. The making screen (P2 re-planned)

- Before any file streams: the stage and the latest step, with elapsed
  seconds and Cancel (as today).
- While `write_file index.html` streams: a sandboxed preview WebView
  (`loadHtmlString`, JavaScript off, no network: `NavigationDelegate`
  refuses every navigation but the first) shows the HTML so far, updated at
  most 4 times a second; the narration line sits under it.
- After each `render`: the inspection PNG replaces the preview (the real
  page, scripts run, at the app's size), with "Checking how it looks" and,
  when it has errors, their count.
- A question (`ask_owner`) shows inline as the questions sheet (R§10.2).
- `finish` → create → the app screen opens; say→usable stops at its first
  `ready` (unchanged). Cancel still cancels (R§4.6).

## R§8. `ask_owner`

- Creates a question through the session route (`POST
  /api/sessions/<sid>/questions`, `source: "ask"`, `artifact_id` the app or
  null for a new app) and waits on it (`GET …/questions/<qid>?wait=`).
- **New app:** waits up to 5 minutes, shown inline on the making screen;
  Cancel withdraws it.
- **Change:** waits up to 60 s, shown at once as the questions sheet when
  the app is in front, else by notification. Unanswered by then, the run
  **parks**: the draft is deleted, the threads get "I asked you a question
  about this: <header>. I'll carry on when you answer.", the feedback is
  acknowledged, and a `ParkedRun {questionId, artifactId, threadIds,
  commentIds}` is kept in `<support>/parked-runs.json`. The answer comes
  back as a late answer on the worker's feedback poll (`answers`, "Late
  answers" in `docs/contract.md`); the worker then runs the change again
  with the same comments and the answer in the first message. A declined
  question runs it with "The owner skipped the question: use your best
  judgement". A parked run whose threads got a newer comment is dropped
  (that comment's run answers it).
- Answers are the owner's words: data, quoted as JSON in the prompt.

## R§9. App screen UX

### R§9.1 Floating comment button

A round button (56 dp) over the page, bottom right by default, draggable to
any edge (snaps to the nearest left or right edge; its vertical position is
kept in settings). Hidden while a composer is open (the peek's controls take
over) and while the panel is open on a narrow screen.

- **Tap:** comment mode on, and a **comment bar** slides up: "Hold on what
  you want changed" · **Whole app** · **Threads (n)** · ✕. Tap again or ✕:
  comment mode off.
- **Long-press:** opens the threads panel (comment mode on, as the swipe).
- The two-finger swipe and ⌘K stay.

### R§9.2 Hold, then drag (bridge)

`long-press.ts` today abandons a hold that moves beyond 10 px. In comment
mode on touch:

- Down, 450 ms within 10 px: **armed** (the target is outlined, a light
  haptic comes from the app on the shell's `state`).
- Release without moving: pick the target (today).
- Move beyond 10 px **after** arming: **drag**. From then on `touchmove` is
  `preventDefault`ed (the listener is registered `passive: false`) so the
  page does not scroll.
  - Text was under the finger at arming (`caretAt` within `TEXT_NEAR`):
    the selection runs from that caret to the caret under the finger
    (`Selection.setBaseAndExtent`, since `selectstart` stays cancelled for
    the user's own selection), shown with the outline tint.
  - Otherwise: an area rectangle from the arming point to the finger
    (`dragRect`, the existing `.a` box).
  - Release: a text drag picks the range as a mouse drag selection does
    (`clax:pick-start` with a range anchor); an area drag picks the area
    (`pickArea`); one under `AREA_MIN` picks the target as a tap would.
  - A second finger or `touchcancel` abandons, clearing the selection or
    box.
- Before arming, movement beyond the slop is a scroll (today).
- No edge auto-scroll while dragging (alternative record).

### R§9.3 Whole-app comment

**Whole app** in the comment bar calls `claxHost.composeArtifact()`: the
shell opens its composer with the `artifact` anchor ("The whole artifact"
label is shown as "The whole app" in app mode), no screenshot; dictation
starts as for a pick.

### R§9.4 Thread detail

Tapping a thread in the panel opens a Flutter page:

- the anchor label, status, its clip (`ClaxApi.clip`), every comment
  (author, relative time, agent replies marked "Clax"), the versions that
  addressed it ("Changed in v5" opens a preview of v5, `loadVersion`);
- a reply box with the mic (dictation per the auto-start setting) and
  **Send**: `POST /api/artifacts/<aid>/threads/<tid>/comments` as the owner;
  on a sent thread the daemon sends it to the agent (`docs/contract.md`,
  "from then on, every later viewer comment on it is sent too"); on a plain
  thread the page offers **Send to Clax** (the send route);
- **Resolve** / **Reopen**; **Retry** when the last agent comment is a
  failure reply (R§9.5).

### R§9.5 Retry on a failed change

A change that ends `ChangeFailed` keeps the comment IDs it answered in the
worker (`_failed[threadId] = commentIds`, also rebuilt on start from the
threads: an open thread whose last comment is an agent comment opening
"Couldn't make that change:"). **Retry** (the Working chip's failed state,
and the thread detail) calls `FeedbackWorker.retry(aid, tid)`, which
queues a job with those comments; nothing is posted. The chip shows the
change as running again.

### R§9.6 Working chip narration (P3 re-planned)

The chip shows the latest step for the change (bound by its thread IDs,
`WorkerProgress(artifactId, threads, step)`) and its elapsed seconds;
tapped, it expands to the last three steps and, after a render, the
inspection thumbnail. Failed: the reason and **Retry**; the current
version stays as it is.

### R§9.7 Voice

- Settings: **Start dictation automatically** (the existing
  `dictateComments`, now also used by the New sheet; default on) and
  **Dictation sounds and vibration** (default on).
- On start: `HapticFeedback.mediumImpact()` and a start tone; on stop:
  `HapticFeedback.lightImpact()` and a stop tone. Tones come from the
  platform through the method channel `dev.empathic.clax/cues` (`play`
  `{cue: "start"|"stop"}`): Android `ToneGenerator`
  (`STREAM_NOTIFICATION`, `TONE_PROP_BEEP` / `TONE_PROP_ACK`, 120 ms),
  macOS `NSSound(named: "Tink")` / `"Pop"`. No new dependency.

## R§10. Clax machinery, native

### R§10.1 Stage 1: the inbox

- The launcher's top bar gets an inbox button with the unread count.
- The inbox screen lists `GET /api/inbox` (newest first, 50 per page,
  `before` cursor), with **Unread / All**, kind chips (Replies, Versions,
  New apps, Questions, Finished) and a search field (`q`).
- Each row: the app's title, the kind's line (the reply's text, "v5:
  <note>", "New app", the question's header, "Finished: <message>"), time,
  unread dot. Tap: mark read (`POST /api/inbox/<id>/read`) and open its
  target: the app screen at the thread (`openThread`) or the version
  (`loadVersion`), or the questions sheet. **Mark all read** sends
  `{all: true, filter, upto: <newest seq shown>}`.
- Live: a `ClaxStream` (the `/api/stream` protocol: open, `POST
  /api/stream/<id>` `{subscribe: ["inbox", "questions"]}`, then fetch,
  then apply `inbox_item` / `inbox_read` / `question` events; reconnect
  with `Last-Event-ID`). Holding `questions` also makes the app an owner
  surface, so `ask` questions are created in `wait` mode.

### R§10.2 Stage 2: questions

- A questions sheet shows open questions (`GET /api/questions`, live from
  the `questions` topic), each with its header, question, options (label,
  description, preview in monospace, the recommended one marked) and
  **Other** (a text field with the mic), **Answer** (`POST
  /api/questions/<qid>/answer`) and **Skip** (`…/decline`).
- It opens by itself when a question of the app's own agent arrives while
  the app is in front (a light haptic), from the inbox, and from the
  question notification (R§4.7). Questions from other agents (none on a
  phone; Claude Code on the desktop's own daemon is a different home) show
  the same way.

### R§10.3 Stage 3: version history and diff

- The Versions tab rows open a version page: number, time, note,
  label, the threads it addressed, and the files changed against the
  version before (added, removed, changed, with sizes).
- A changed text file opens a line diff (Myers, `lib/ui/line_diff.dart`,
  no dependency): unified, 3 lines of context, long unchanged runs folded,
  additions and removals coloured with the theme's tokens. A file over
  4,000 lines or 256 KiB shows sizes only.
- **Compare with current**, **Preview** (`loadVersion`) and **Restore**
  (today's) on the version page.

## R§11. Web additions

- **W5, bridge hold-then-drag** (R§9.2): `long-press.ts`,
  `comment-mode.ts`.
- **W6, inspect and whole-app compose**: bridge `inspect.ts` (the error
  ring buffer installed at bridge start, before page scripts; the settle
  wait; the screenshot; the outline) and the bridge message
  `clax:inspect` / `clax:inspection`; shell `?inspect=1` mode, host method
  `inspect({settleMs, maxBytes})`, host message `inspection {png, errors,
  failed, text, title, height}`, host method `composeArtifact()`. The
  host protocol header in `web/shell/src/app/host.ts` stays the source of
  truth.

## R§12. Testing

Deterministic only. No test or gate calls a model; one final live task runs
at most three `claude -p` calls.

| Layer | What |
|---|---|
| Rust | drafts: create with and without `of`, the document copy (cap, private documents excluded), hidden from every listing route and topic (one audit test listing them), threads refused, reads of other apps through `of`'s grants, delete and sweep (injected clock), migration 25. |
| Web unit | hold-then-drag: arm, drag text range, drag area, small drag = tap, second finger, cancel, scroll before arming untouched, `touchmove` cancelled only when dragging; inspect: ring buffer order and cap, settle with an injected timer, outline limits, half-scale retry; host `inspect`, `inspection`, `composeArtifact`, consent auto-decline in inspect mode. |
| Web e2e | touch hold-drag selects a range and picks it; area drag picks an area; `?inspect=1` returns a PNG and an error a page threw at load. |
| Dart unit | Toolbox per tool (paths, sizes, exactly-once replace, staged writes need a read, pins, caps, gate), `finish` (every branch of R§4.5 with `FakeClaxApi`, the ledger written before the publish, restore on refusal, newer comment left out), Messages runner against the fake Messages API (scripted streamed turns: tool loop, append-only history byte-equal across turns, cache fields, eager fields, max_tokens resend, refusal, soft and hard caps, cost sums, cancel mid-turn, network-cut resume per turn), MCP server (initialize, list, call, image content, bearer refused), CLI runner against `fake-claude.sh` driving the MCP server, `ask_owner` (answer, decline, park, late answer re-run), worker (retry, progress events), `ClaxStream`, line diff. |
| Widget | making screen (preview throttle, inspection image, inline question, cancel), chip (narration, expand, retry), FAB and comment bar, thread detail, inbox, questions sheet, version page and diff, settings toggles and cues (fake channel). |
| macOS integration | the full loop with `ScriptedRunner` over the real Toolbox, the real daemon (drafts) and the real `WebViewInspector`: new app (write, render, finish), change (str_replace, render, finish) landing in place with a document kept. |
| Live (last) | ≤ 3 `claude -p` calls with `ClaudeCliRunner`: a new app, a change, and one spare; timings and costs recorded. |

## R§13. Open questions for the owner

Each is a `status:needs-decision` record on this file, with the choice this
design takes until the owner answers.

- **Q1** The desktop gets the tools from an MCP server the app serves, not
  from `clax mcp` (R§4.3). `clax mcp` has no drafts, no render, and its
  writes would bypass the change ledger; its session would also end the
  app's session when the transport closes.
- **Q2** Every `finish` that publishes must follow a render of that exact
  draft (R§4.5). Cost: one more turn and ~1–2 s per change.
- **Q3** Caps and what happens at them (R§4.4): nothing is published at a
  hard cap, even a clean draft.
- **Q4** `ask_owner` on a change waits 60 s, then parks the run until the
  answer (R§8).

## R§14. Out of scope

Publishing incrementally during a run; edge auto-scroll during a drag; a
native WebView snapshot; a render on the daemon; the agent's own mid-run
replies; widgets; iOS; the `/api/token` exposure (D-3, unchanged).
