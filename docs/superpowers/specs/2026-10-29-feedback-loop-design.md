# The feedback loop: capture, context, fix, verify

Status: design for the owner's review (2026-10-29). Builds on
`2026-10-27-clax-sdk-design.md` (the SDK spec, referred to below as "SDK
§n") and its first milestone (`docs/superpowers/plans/2026-10-27-report-queues-m1.md`).

## 1. Intent

Any person signed in to a product built with Clax (Empathic's employees on
clax.dev first) sees something wrong, points at it, and files it in a few
seconds without leaving the page. The report lands in a queue on the
product's Clax carrying everything a fixer needs: what they pointed at, a
picture, when, which release and commit was running, which feature flags
they had, their browser, and recent errors. An agent pulls the report,
fixes it, and proposes the fix back to the person who filed it; that
person verifies it, or reopens the report with what is still wrong, and
the loop runs again until they say it is fixed.

clax.dev is the first integration and is built as the reference others
copy: Empathic's own products (changelist, pathbase) adopt the same loop
next, the same way. So clax.dev uses only what any integrator can use (the
overlay, the SDK, the public HTTP API), and every gap it meets is closed in
the SDK or the API as a general feature.

## 2. Goals, non-goals, anti-goals

Goals

- G1. Capture in three modes from the overlay: an element (or text range),
  a dragged area, and the whole visible screen; an exact screenshot through
  the browser's screen capture when a DOM render cannot show the page.
- G2. Annotate and redact before sending, and preview exactly what will be
  sent, attachments and context included.
- G3. Every report carries a typed **context** Clax defines and every
  client renders: captured time, release (ID, version, commit), feature
  flags, environment (URL, viewport, device pixel ratio, user agent,
  locale, time zone), and opt-in **diagnostics** (recent console errors,
  uncaught errors, failed requests), scrubbed of secrets on the client and
  again on the server.
- G4. A report has a lifecycle that includes the fix and the reporter's
  verdict: `open → claimed → fix_proposed → verified`, with `reopened`
  going back to `open`; every fix attempt is kept with the report.
- G5. Agents and scripts pull reports with everything above: MCP tools, the
  CLI, and a wake when a report arrives or is reopened.
- G6. People triage in the Clax UI: a queue view and a report view with the
  clip, context, diagnostics, conversation, and fix history.
- G7. Reporters see their reports and what happened to them in the
  overlay, and are told when a fix waits for them.
- G8. clax.dev wires this up for its own people with release, commit and
  flags, through public surfaces only, documented as the worked example of
  an integration guide written for any product.

Non-goals

- The fixing system itself (an agent that picks reports, writes the fix and
  opens the PR). This design provides everything it needs and documents
  the recipe (§9); building it is later work.
- Outbound webhooks (SDK §9). Reporters learn of fixes in the overlay
  (§6.3) and, for the PR, through GitHub's own review request. Webhooks
  remain the next step for integrators who want their own channels.
- A general feature-flag service. Clax records the flags the host app says
  the reporter had; it does not evaluate flags.
- Organisations or teams. Who may file is the integrator's decision, made
  in its backend when it mints reporter tokens (SDK §5.3).
- Moving the extension's capture onto the overlay's.

Anti-goals: it fails, even working, if

- capture leaks what the reporter did not see in the preview (typed field
  values, tokens in URLs or logs, the overlay itself);
- clax.dev needs something an outside product cannot have;
- a report's context can be silently changed after filing (the context is
  what the reporter's browser said at capture; triage writes go in
  `metadata`, never in `context`);
- the reporter is left not knowing whether anything happened.

## 3. What exists (milestone 1)

- Overlay `web/overlay`, built to `/_clax/overlay.js`, global `Clax`,
  `Clax.overlay({instance, queue, auth, metadata?, externalId?})`
  (`web/overlay/src/main.ts`, `model.ts`). Picks through the bridge's
  `CommentMode` (element, range, area), DOM clips by modern-screenshot
  (`web/bridge/src/clip.ts`). "Your reports" panel polled every 15 s.
- SDK `web/sdk/src`: `ReporterClient` (Ed25519-signed, token renewal),
  `ServiceClient.mintReporterToken`.
- Queues (migration 59): queues, members (`admin|triage`), service grants,
  reporter tokens bound to `reporter_id`, origin and key; reports are
  threads with `queue_id`, `reporter_id`, `metadata`; claims are leases
  with fences; states `open|claimed|resolved`.
- Routes `crates/clax-server/src/routes/queues.rs`, `queue_agent.rs`,
  `queue_reporter.rs`; MCP tools `queue_list|claim|renew|release|reply|resolve`.
- `/healthz` returns `version`, `commit`, `build`; per-person boolean flags
  (`crates/clax-store/src/flags.rs`); GitHub sign-in, no email stored.
- Missing, and built here: whole-screen and exact capture, annotation,
  diagnostics, typed context, the fix and verify states, a CLI, wakes on
  new reports, report and per-report triage views (the Queues page
  exists for queue settings), reporter identity beyond an opaque ID, and any
  integration in clax.dev itself.

## 4. Capture (the overlay and the bridge)

### 4.1 Modes

The Report action opens a small mode bar: **Element** (click; Alt widens,
as today; selecting text gives a range), **Area** (drag), **Screen** (the
visible viewport, at once). Keyboard: `E`, `A`, `S` in the bar, Escape
leaves. A host may start a mode directly (§4.6).

- Element and Area keep today's DOM clip (`web/bridge/src/clip.ts`).
- Screen renders `document.documentElement` cropped to the viewport at the
  current scroll, by the same renderer, within the same limits (long side
  1600 px after scaling).
- **Exact screenshot.** Every mode's preview offers "Use exact screenshot":
  `getDisplayMedia({preferCurrentTab: true, video: {displaySurface:
  "browser"}})`, one frame, then the stream stops. It is the default
  suggestion when the DOM render reports omissions (§4.2) that cover the
  picked region — a cross-origin iframe, as clax.dev's artifact frame is,
  canvas, or video. Where `CaptureController`/capture handles allow it the
  overlay checks the captured surface is this tab and says so when it
  cannot; for Element and Area modes the frame is cropped to the picked
  rectangle. Not offered where `getDisplayMedia` is absent (phones).
- The overlay hides itself before any capture and is never in a clip
  (`data-clax-ignore` on its root; the renderer filters any element with
  that attribute, so hosts can mark their own).

### 4.2 Privacy of a DOM render

- Values typed into `input`, `textarea`, `select` and contenteditable
  regions are blanked in the render unless the element carries
  `data-clax-show`. Password fields are always blanked.
- Elements with `data-clax-redact` render as solid blocks.
- The render lists what it could not draw (`omitted: [{kind, rect}]`:
  cross-origin images and iframes, canvas, video), as SDK §8.2 says; the
  preview shows it and the report keeps it in context (§5.1).

### 4.3 Annotate

After capture the preview shows the image with tools: **box**, **arrow**,
**redact** (opaque block), **text**, undo and clear. Annotations are drawn
into the PNG that is sent (flattened; redactions cannot be undone by a
reader). An Element pick is drawn with its outline by default, which the
reporter may remove.

### 4.4 Diagnostics

`Clax.overlay()` (or `Clax.diagnostics.install()` for hosts that want it
before the overlay loads) starts a ring buffer of the last 200 entries:
`console.warn` and `console.error`, `error` and `unhandledrejection`
events, and failed `fetch`/XHR requests (method, URL path without query
values, status, duration, and the response's request-ID header when one of
`x-request-id`, `cf-ray`, `x-clax-request` is present). Each line is
scrubbed before it is stored (§5.3). Nothing leaves the page unless the
reporter keeps "Include recent errors" ticked; the host sets its default
(`diagnostics: {default: true | false}`, default `false`).

### 4.5 The composer and preview

One panel: the note (required), the image with annotation tools, the
attachment toggles (image; recent errors with a count), and "What's sent"
which expands to the exact context JSON and diagnostics lines. Send files
the report; the panel then shows "Filed" with a link to it in "Your
reports".

### 4.6 Host API

```ts
const clax = Clax.overlay({
  instance, queue, auth,
  launcher: "corner" | false,          // default "corner"
  context: async () => ({ release, flags, environment? }), // §5.1, at capture
  metadata: async () => ({ ... }),      // opaque, SDK §6.4; value or function
  diagnostics: { default: false },
  externalId,
});
clax.report({ mode?: "element" | "area" | "screen" });
clax.showReports();                      // open "Your reports"
clax.on("badge", (n) => ...);            // reports waiting on the reporter
clax.on("filed", (report) => ...);
clax.close();
```

`context` and `metadata` are evaluated when the reporter starts a capture,
so they describe that moment. The npm-free script build stays; the same
entry is also built as an ES module (`/_clax/overlay.mjs`) for bundlers.

## 5. Context, diagnostics and reporter identity (the data)

### 5.1 Context

Clax defines it, the SDK fills what the browser knows, the host fills the
rest; Clax validates its shape and stores it with the report, immutable:

```json
{
  "v": 1,
  "captured_at": "2026-10-29T07:12:03.120Z",
  "release": { "id": "…", "version": "0.6.0", "commit": "f7e862ab0…", "channel": "prod" },
  "flags": { "claxbot": true, "new_editor": "variant-b" },
  "environment": {
    "url": "https://clax.dev/a/…", "route": "/a/:id", "title": "…",
    "viewport": { "w": 1440, "h": 900, "dpr": 2 }, "scroll": { "x": 0, "y": 340 },
    "user_agent": "…", "platform": "macOS", "locale": "en-US", "time_zone": "America/Los_Angeles",
    "color_scheme": "dark"
  },
  "capture": { "mode": "screen", "method": "dom" | "display", "omitted": [ … ] },
  "app": { "...": "host-defined, small" }
}
```

- `release` fields are strings; `flags` values are booleans, strings or
  numbers; `app` is opaque. Unknown top-level keys are refused (400
  `invalid_context`), size at most 16 KiB.
- Distinct from thread `metadata` (SDK §6.4): `metadata` is the app's,
  opaque, editable by triage with `if_version`; `context` is what the
  reporter's browser said at capture and never changes.
- The server stamps `received_at` and, on clax.dev and daemons, its own
  `server: {version, commit, build}` (as `/healthz` reports them) beside
  the context it was sent.

### 5.2 Diagnostics

A second multipart part `diagnostics`, JSON lines `{at, level, kind, text}`,
at most 200 lines and 256 KiB, stored beside the clip and served to triage
members and the reporter it belongs to (`GET …/reports/<tid>/diagnostics`).

### 5.3 Scrubbing

One rule set, implemented in the SDK (TypeScript) and in `clax-core`
(Rust, applied again on receipt): bearer and basic credentials, `cx?_`
tokens, JWT-shaped strings, values of query parameters, `password=` /
`token=` / `secret=` / `key=` pairs, email addresses, and long hex or
base64 runs are replaced by `‹redacted›`. The Rust rule set is the source
of truth; a shared table of cases (`web/sdk/test/scrub-cases.json`) is run
by both test suites.

### 5.4 Reporter identity

A reporter token may carry a **profile** the integrator's backend asserts
when minting it (SDK §5.3): `{name?, email?, github?, avatar_url?}`, each
at most 256 bytes. It is stored with each report filed under that token
and shown to triage members, never to other reporters. It is how a fixer
knows whom to ask for review (`github`), and how an integrator's own
notifications reach them. It is the integrator's assertion, as
`reporter_id` is.

## 6. Lifecycle: fix and verify

### 6.1 States

| State | Meaning | Entered by |
|---|---|---|
| `open` | waiting for someone | filing; release; lease expiry; reopen |
| `claimed` | an agent or person holds the lease | claim |
| `fix_proposed` | a fix waits for the reporter's verdict | propose-fix |
| `verified` | the reporter says it is fixed | verify |
| `resolved` | closed by triage without a verdict (duplicate, won't fix, can't reproduce) | resolve, with a reason |

The state is stored as the report's phase (milestone 1 derived `resolved`
from the thread's resolved flag, `crates/clax-core/src/queue.rs`);
`verified` and `resolved` resolve the thread, `fix_proposed` leaves it
open. `resolved` keeps its meaning from milestone 1 and gains a `reason`
(`fixed`, `duplicate`, `wontfix`, `cannot_reproduce`, `other`). Listings
filter by any state; `active` means `open|claimed`; the reporter's badge
counts `fix_proposed` reports and reports with replies newer than the
reporter last read.

### 6.2 A fix

`POST /api/queues/<q>/reports/<tid>/fix` by the lease holder (with its
fence) or a triage person:

```json
{ "summary": "…", "url": "https://github.com/…/pull/123",
  "commit": "…", "release": "…", "verify_url": "https://…", "verify_after": "release:0.6.1" }
```

`summary` is required (plain text, at most 4 KiB); the rest optional.
It appends a fix attempt (table `report_fixes`: number, summary, links,
proposer, created_at, outcome), posts the summary as a comment visible to
the reporter, moves the report to `fix_proposed`, and ends the lease. A
report in `fix_proposed` is not claimable until it is reopened.

### 6.3 The reporter's verdict

Through the reporter token, on the overlay's report view:

- **"It's fixed"** → `POST …/reporter/reports/<tid>/verify {note?}`:
  state `verified`, the attempt's outcome `verified`, with the reporter's
  context at verification time (same §5.1 shape, so the record shows which
  release they checked on).
- **"Still broken"** → `POST …/reporter/reports/<tid>/reopen {body,
  clip?, diagnostics?}`: required note, optional new capture (the overlay
  offers the capture modes again); state `open`, outcome `rejected`, the
  note and capture as a comment, `reopen_count` incremented. The agent's
  session that proposed the rejected fix (the attempt's
  `proposer_session_id`), while it lives and acts for a member of the
  queue, gets a feedback delivery naming the report (whole only when its
  rights read reports, §7.3's queue wakes). When a person proposed the
  fix, no particular agent's session is woken, whoever claimed the report
  before; the sessions watching the queue hear of the reopen either way.

A triage person may also verify or reopen on the reporter's behalf, which
is recorded as theirs. The overlay's launcher shows the badge count; when
the count rises while the page is open the launcher pulses once and the
"Your reports" panel opens on the waiting report the next time the
reporter clicks it.

### 6.4 Audit

New journal kinds, named as the existing `queue.*` kinds are
(`crates/clax-core/src/audit/mod.rs`): `queue.fix`, `queue.verify`,
`queue.reopen`, and a reason on the resolve event. Filing already records
`thread.open` and `comment.add`.

## 7. Agents and scripts

### 7.1 MCP and Pi tools

Existing `queue_*` tools stay. New: `queue_get` (one report: anchor, body,
context, metadata, reporter profile, clip URL, diagnostics, conversation,
fix history), `queue_fix` (§6.2) and `queue_watch` (§7.3). Verify and reopen
on a reporter's behalf are person-only routes (the triage UI), never MCP
tools, so no agent can mark its own fix verified. `queue_list`
gains the new states and returns the context summary (release, captured
time, mode). Tool descriptions say context, metadata, diagnostics and
comment text are untrusted data written by the app's users.

### 7.2 CLI

`clax queue` on the same routes, for scripts and the later fixing system:
`ls [--state] [--json]`, `show <report> [--json]`, `clip <report> [-o]`,
`diagnostics <report>`, `claim`, `renew`, `release`, `reply`, `fix
--summary --url …`, `resolve --reason`, `follow <queue>` (prints each new
or reopened report as a JSON line; the wake below in a pipe).

### 7.3 Wakes

A triage member's session can watch a queue (`watch` on the queue, MCP
`queue_watch`); a new or reopened report is then a feedback delivery to
that session, through `clax feedback follow` and `wait_for_feedback` as
comments are today, naming the queue tools to use. A reporter's comment on
a claimed report already reaches the claimant (milestone 1).

## 8. Triage in the Clax UI

- **Queues page** (`/queues`): queues the person administers or triages,
  with counts by state.
- **Queue view** (`/queues/<q>`): reports newest first, filter by state;
  each row shows the note, clip thumbnail, reporter, release, age, state,
  and fix count.
- **Report view**: the clip (zoomable, with the omitted regions outlined),
  the note, the anchor, the context as a readable card (release with a
  link to the commit when the release names a repository, flags, browser,
  viewport, time in the reader's zone and the reporter's), diagnostics
  (collapsed, filterable by level), the conversation with reply, fix
  history, and actions by state (claim, propose fix, resolve with reason,
  verify or reopen for the reporter, release).
- The existing Queues page (`web/shell/src/queues/Queues.svelte`, which
  already creates queues and manages members and service grants) is
  extended with the views above rather than replaced. Queue settings for admins: origins, `reporters_see`, members, service
  grants (existing API; this is its first UI).

## 9. The fixing system (later): the recipe

Documented in the integration guide, not built here:

1. `clax queue follow <q>` (or `queue_watch`) wakes the fixer.
2. `queue_claim`, `queue_get` → reproduce on `context.release.commit`
   with `context.flags` and `context.environment.viewport`.
3. Fix on a branch; open a PR whose description links the report and
   requests review from `reporter.github` when present.
4. `queue_fix {summary, url, commit, verify_url}`; the reporter is told in
   the overlay and by GitHub.
5. On reopen the fixer is woken with the reporter's note and new capture;
   it updates the same PR or opens another, and proposes again.

## 10. clax.dev as the worked example

### 10.1 Who files

Every signed-in person: the owner and each connected person who is not
revoked; agents and anyone not signed in are refused. No flag gates it.
That is clax.dev's integration choice, the way any integrator decides in
its backend. (An earlier draft gated it on a person flag `feedback` an
admin turned on; the owner chose to open it to everyone.)

### 10.2 The integrator backend inside clax.dev

clax.dev plays both roles: Clax instance and integrating product. Its
integrator half is one small module that does only what an outside
backend would:

- configuration: none on a hosted instance (clax.dev, `just
  clax-dev-local`): the first time a person asks once the owner exists,
  the instance makes its own queue `clax-feedback` (the owner its admin,
  the public origin its origin, `reporters_see: own`) and a service grant
  of it, keeps both in its store, and makes them again when the queue is
  deleted or the grant revoked. `CLAX_FEEDBACK_QUEUE` (queue ID) and
  `CLAX_FEEDBACK_SERVICE_GRANT` (the service grant's credential and key, a
  Worker secret), or `[feedback]` in a daemon's `config.toml`, name a queue
  of the operator's choosing instead and win. A local daemon that is not
  hosted is off without them, and the shell shows no entry point;
- `POST /_app/feedback/token` (session-authenticated, same-origin): for a
  signed-in person, mint a reporter token for `reporter_id` = their
  public ID and profile `{name, github}` from their GitHub identity, by a
  signed request to the public `reporter-tokens` route through the SDK's
  in-process transport (SDK §4.1), the same request an outside backend
  sends over HTTP;
- the module lives apart from the instance code (its own file, depending
  only on the public request types), so the guide can show it whole.

### 10.3 The shell

- "Report a problem" in the account menu, with a shortcut the shell does
  not already bind (`Ctrl/Cmd+Shift+B` unless taken), shown in the menu;
  "My reports" beside it with the badge. The overlay runs with
  `launcher: false`.
- `context()` returns `release` from `/healthz` (`version`, `commit`,
  `build` as `id`, channel from the host name), the person's flags (from a new `GET /api/me/flags`; today flags
  are only on the admin route and inside `/api/me/bot`), and
  `app: {view, artifact}` (the shell's current view and artifact ID).
- On an artifact view the artifact frame is cross-origin, so Screen mode
  suggests the exact screenshot (§4.1).

### 10.4 Setup on prod

None is needed: clax.dev makes its own queue (§10.2), which the owner
finds on the Queues page and adds triage members to. `docs/prod.md` has
the steps for a queue of the operator's choosing instead (create queue
`clax-feedback` for `https://clax.dev` as the owner, add triage members,
create a service grant, store it as the Worker secret). `just
dev-feedback` does the same against `~/.clax-dev` with a dev queue, so
the loop runs locally end to end on a daemon that is not hosted.

## 11. Documentation

- `docs/feedback.md`: the integration guide, for any product: the moving
  parts, the backend token route (TypeScript and Rust snippets), the
  overlay options, context and flags, privacy markers, the triage UI, the
  agent loop (§9), with clax.dev's code as the worked example.
- `docs/contract.md`: context, diagnostics, profile, the new states,
  routes, errors (`invalid_context`, `not_fix_proposed`), tools, CLI, and
  the new journal kinds.

## 12. Constraints

- Migration takes the next free number on main (60 at the time of
  writing).
- Rights tests: a reporter verifying or reopening another reporter's
  report under `own`; an agent verifying; a fix without the lease's fence;
  diagnostics read across queues; profile never served to reporters.
- The overlay's script stays within a size budget recorded in its build
  test (it is one classic script, `web/vite.overlay.config.ts`, so
  nothing loads lazily).
- All tests follow `AGENTS.md`: no sleeps, injected clocks, Bazel.
