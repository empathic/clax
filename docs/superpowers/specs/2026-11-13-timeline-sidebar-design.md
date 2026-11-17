# Timeline sidebar

Status: first build, expected to be iterated. The owner approved the
direction from a mockup and asked for it to be designed, specified, planned
and built without further questions. Every choice below marked **(call)**
was made without the owner and is theirs to overturn.

## Why

The artifact view's sidebar is a list of comment threads grouped by status
(Addressed in vN, Open, Detached, Resolved). That makes every comment a
conversation to manage. The loop people actually run is see, comment, do:
look at the page, say what is wrong, and see that it was fixed. Most
comments need no conversation at all.

The owner's words: "I want to know that something happened and follow up on
it if it wasn't quite what I wanted, but if I see that what I asked for was
fixed, I don't need to worry about the thread."

So the sidebar opens on what happened, newest first. A thread is something
you open from a line of that history when the outcome is not what you
wanted. The thread list stays, one tab away, with everything it does today.

## Goals

- The sidebar's default view is a timeline of the artifact's events, newest
  first. Threads open in place from it, as the same card they are today.
- The sidebar has a collapsed form: a narrow strip on the edge that shows
  how much is pending, how much of it needs a person, and the shape of how
  that got there.
- A band drawn along the sidebar's outer edge, in both forms, shows the
  count of pending comments over the events, and the part of it that waits
  on a person.
- Nothing the sidebar does today is lost. The thread list is reachable as a
  second view, unchanged.

## Non-goals (this build)

- No change to the daemon, its API, or what a thread or version stores.
  Every line of the timeline is derived from what the shell already loads.
- No change to when a thread resolves. See "Open decisions".
- No change to `ThreadCard.svelte`'s behaviour.
- The extension's side panel keeps the thread list alone. It mounts
  `Sidebar.svelte` without the new props and must render exactly as before.
- No timeline on the gallery, the inbox, or live pages.
- No "what changed" diff between versions.

## Anti-goals

- A second place where thread state is computed differently from the first.
  The timeline's notion of "pending" is defined once, in one pure module,
  and both the list of lines and the band read it.
- Layout shift on load. The page's frame is laid out once at its final
  size today (`SidebarIsland.svelte`); the collapsed strip must keep that.
- Colour or decoration that does not carry meaning. The owner rejected a
  noisy first mock and a green band; the accepted look is one ink for text,
  a neutral grey for the band, and the "you" orange (`--you`) only for what
  waits on a person.

## What exists (verified in the code)

- `web/shell/src/ui/Sidebar.svelte` renders the sections from
  `sidebarSections` (`view/sidebar-model.ts`) and holds the one long wiring
  of `ThreadCard` (its `cards` snippet). `ShellSidebar.svelte` wires it to
  the controller inside `DrawerChrome`; `SidebarIsland.svelte` mounts it
  when `s.panel` is true and loads the chunk lazily (`sidebar-chunk.ts`).
  The extension's `web/extension/src/panel/Panel.svelte` mounts `Sidebar`
  directly.
- The drawer already docks right, left, at the bottom, or floats
  (`view/drawer-model.ts`, root `data-drawer`), per browser. At 700 px and
  narrower the sidebar is a full-screen sheet switched by `PhoneTabs`.
  `s.panel` starts true at 900 px and wider.
- A `Thread` (`threads.ts`) carries `created_at`, `comments` (each with
  `author_kind`, `created_at`), `sends` (batches: `batch_id`, `size`,
  `note`, `sent_by`, `sent_at`), `sent_to_agent`, `addressed_in` (version
  numbers), `resolved_at`, `resolved_by`, `status`, `feedback_state`.
  A `Version` (`api.ts`) carries `n`, `created_at`, `note`, `addresses`,
  `agent_harness`. `s.working` lists who works on which threads now.
  `s.attention` has `open_in` (open threads this viewer is in) and
  `s.decided` the unseen addressed threads of the latest version.
- `view/history-model.ts` already words one thread's history
  (`historyOf`); `view/working-model.ts` names agents; `format.ts` has
  `relativeTime`.
- About 40 e2e specs under `web/e2e` find thread cards in the sidebar as
  soon as it opens.

## Terms

- **Pending**: a thread that still wants something done. Defined exactly in
  "The model". It is narrower than `status === "open"`: a thread a version
  addressed is open but not pending. The word "open" is not used for it
  anywhere in the UI, since the Threads view's "Open" keeps its meaning.
- **Waits on a person**: a pending thread whose next move is a person's: it
  was never sent to an agent, or an agent answered it without a version.
- **Outer edge**: the sidebar's edge away from the page. Docked right, its
  right edge; docked left, its left edge; at the bottom, floating, and on a
  phone, its right edge.

## The model (`web/shell/src/view/timeline-model.ts`, new, pure)

One function, no DOM, no clock of its own:

```ts
timeline(input: {
  threads: Thread[]; versions: Version[]; working: Working[];
  claimed: readonly string[];         // threads ClaxBot took (`botView.claimed`)
  me?: Viewer | null;                 // names come from `resolvedByLabel` and `agentName`
}): Timeline
```

```ts
type Timeline = {
  lines: Line[];                       // oldest first
  pending: string[];                   // thread IDs pending now
  waiting: string[];                   // of those, waiting on a person
  stateOf: Map<string, ThreadState>;   // every thread's state now
};
type ThreadState =
  | { kind: "unsent" }                 // pending, waits on a person
  | { kind: "with-agent"; working: boolean }
  | { kind: "answered"; agent: string }// pending, waits on a person
  | { kind: "stalled"; why: string }   // pending, waits on a person
  | { kind: "addressed"; v: number }   // not pending, thread still open
  | { kind: "resolved" };
type Line = {
  key: string;                         // stable across recomputes
  at: string;                          // ISO time
  kind: "version" | "made" | "sent" | "answered" | "followed-up" | "resolved";
  who: string; agent: boolean;
  v?: number; note?: string | null;    // version
  ids: string[];                       // threads the line is about
  here: string[];                      // of `ids`, those whose newest line this is
  pending: number; waiting: number;    // counts just after this line
};
```

### A thread's history

Each thread is read as a sequence of facts in time order (ties keep this
order): made (its first comment), sent (each entry of `sends`), a person's
later comment, an agent's comment,
addressed (one per number in `addressed_in`, at that version's
`created_at`; a number with no known version is skipped), resolved
(`resolved_at`, when `status` is `"resolved"`).

A state machine walks them:

- made: pending, waits on a person (unsent).
- sent: with the agent.
- **A send that left no record.** The daemon writes a `sends` entry only
  for a batch (`crates/clax-store/src/batches.rs`); a single Send, an
  `@agent` mention and a send to ClaxBot set `sent_to_agent` and record no
  time. For a thread with `sent_to_agent` true and no `sends`, or one in
  `claimed`, the thread is with the agent from the moment it was made, and
  no sent line is written for it: its made line stands alone and says
  nothing about sending. The time it spent unsent is not shown. This is a
  known inaccuracy of this build (see "Open decisions").
- addressed: not pending.
- resolved: not pending.
- a person's later comment: pending again; with the agent if the thread
  was ever sent, else waits on a person. (The daemon already delivers a
  later comment on a sent thread to its agent; the shell does nothing new.)
- an agent's comment: if a fact "addressed" for this thread lies between
  the person's comment before it and the person's comment after it (or the
  end), the comment is that version's answer and changes nothing. Otherwise
  the agent answered without a version (a question, a refusal): pending,
  waits on a person, from the first such comment in that stretch.

Two things refine only the present state, never the history:

- `working`: a thread that is with the agent and listed in a working
  record's `thread_ids` has `working: true`.
- `feedback_state`: a thread that is with the agent whose
  `feedback_state.state` is `withheld`, `declined`, `no_access` or
  `agent_ended` waits on a person (`{ kind: "stalled"; why }`, counted in
  `waiting`). These are the states in which the card already asks a person
  to act (an approval, ClaxBot's offer in `ShellSidebar.svelte`).

One walk defines a thread's state: its present state is the walk's final
state (a thread in `claimed` is with the agent from the walk's start),
refined only by the two above. A never-sent thread that a version
addressed is addressed. A resolved thread stays resolved through any later
fact; one with no `resolved_at` is resolved after its last fact, with no
line. So the newest line's `pending` equals `timeline.pending.length`, and
its `waiting` is lower than `timeline.waiting.length` only by the stalled
threads. Agreement with `unsent()` in `view/batch-model.ts` means only
this: every thread whose state is `unsent` is in `unsent(threads,
claimed)`.

### Lines

- **version**: one per version, at its `created_at`. `ids` are its
  `addresses` that are known threads. `who` is the agent's name
  (`agentName(v.agent_harness)`), `note` its note.
- **made**: a thread's first comment. Omitted when the thread's first send
  is the very next line in the merged timeline that concerns that thread
  and no other line lies between the two: the sent line then stands for
  both.
- **sent**: one per batch (`batch_id`). `ids` are the batch's threads
  present in `threads`. `who` is the batch's `sent_by`, a display name.
- **answered**: an agent's comment that is not a version's answer (first of
  its stretch only).
- **followed-up**: a person's comment after the first.
- **resolved**: at `resolved_at`.

Adjacent lines of the same kind and `who`, among made, followed-up and
resolved, merge into one (their `ids` joined, `at` the later). Lines sort
by `at`, stable. `pending` and `waiting` on a line are the counts over all
threads with every fact at or before the line's `at` applied; for a merged
or folded-in line, after all of its facts.

`here` implements the rule that keeps the timeline short: **a thread is
listed once, under the newest line that concerns it.** Older lines that
concern it name it only in their count ("sent 4 comments").

`key` is built from the line's kind and the IDs or version it stands for,
so a recompute that adds a line does not re-key the others.

### Wording (`timelineText(line, ...)`, same module)

Text, never markup, in the style of `historyOf`:

- version: `v6` and its note; without a note, `v6` and "published by
  claude".
- made: "You commented" / "Mia commented"; several: "You made 3 comments".
  Its comments unsent now: all, ", sent to no one"; some, ", 2 not sent".
- sent: "You sent 4 comments to claude" (the count is the batch's `size`);
  with a note, the note follows on the line's second row.
- answered: "claude replied".
- followed-up: "You replied" / "Mia replied".
- resolved: "You resolved 2 comments" / "claude resolved a comment".

The viewer reads "You". A comment is the viewer's when its
`author_public_id` is `me.public_id`; a resolve as `resolvedByLabel`
decides. A send carries only a display name, so it reads "You" only when
the viewer has a display name and it equals `sent_by`; otherwise the name
as given.

## The band (`web/shell/src/view/band-model.ts`, new, pure)

Geometry only: from rows `{ y, pending, waiting, version? }` listed newest
first, a top `y`, an edge `x0`, a direction (+1 grows toward smaller x, -1
toward larger), and a unit width per comment, it returns SVG path data for
two areas and the tick segments:

- the agent area, from the edge to `(pending - waiting) * unit`;
- the waiting area, outside it, to `pending * unit`;
- a tick per version row, from the edge to just past the wider of the
  widths above and below it.

Above a row (toward newer) the band is as wide as that row's count; it
holds that width as a straight run to the next newer row and changes over
a short straight taper (4 px each side of the row's `y`) at the row. Below
the oldest row it tapers to nothing. It also returns, for the collapsed
form, the label positions: one count per run where the count changes and is
not zero, centred on the run, and each version's label at the end of its
tick.

A count wider than the room caps the drawn width (`maxUnits`, default 6
comments) while the written count stays true: past the cap both edges are
scaled by `maxUnits / pending`, so the waiting share keeps its proportion
and shows whenever `waiting` is not zero.

This is the vignette's `band()` and `area()` (see "The vignette"), made a
pure function returning strings so it is tested without a DOM.

## The expanded form

### Two views

`Sidebar.svelte` gains an optional prop:

```ts
/** The view shown and its switch: with it, the sidebar leads with the
 * Timeline | Threads tabs. Without it (the extension's side panel), the
 * thread list alone, as before. */
view?: { now: "timeline" | "threads"; onChange(v: "timeline" | "threads"): void };
```

- With `view`, a tab row (`role="tablist"`, two `role="tab"` buttons,
  arrow keys move between them) sits directly above the list it switches,
  below the items that stay in both views. Its right end shows "N need
  you" when `waiting` is not empty: a button that selects the newest
  waiting thread (`onSelect`). The tab row, the model and the timeline
  load together as one lazy component (`ui/SidebarViews.svelte`), which
  takes today's sections as a snippet, so `Sidebar.svelte` imports nothing
  new statically.
- Everything above the sections today stays in both views, in today's
  order: the questions slot, New comment, the live writers, the working
  strips, Send unsent, the selection bar.
- **Threads** renders today's sections exactly: the Addressed group, Open,
  Detached, Resolved.
- **Timeline** renders `Timeline.svelte` in their place.

The view is remembered per browser (**call**): `localStorage`
`clax.sidebar` = `{"view":"timeline"|"threads"}`, read with try/catch as
`drawer-model.ts` reads its prefs, default `"timeline"`. A small
`ui/sidebar-view.svelte.ts` holds it the way `ui/drawer.svelte.ts` holds
the drawer's prefs. `ShellSidebar.svelte` passes it as `view`.

### `ui/Timeline.svelte` (new, in the sidebar's lazy chunk)

Props: the `Timeline`, the clock's `now`, `selected`, the page-number map,
`card: Snippet<[Thread]>` (the sidebar's existing `cards` wiring for one
thread, so a card in the timeline is the same card with the same props as
in the list), `onSelect`, `onSeen`, the side the band is on, and the
`above` snippet.

- Lines newest first, under day headings ("Today", "Yesterday", then the
  weekday and date). Each line: its text, and its time at the right
  (`relativeTime` for today, clock time before).
- While `working` is not empty the working strips above already say who is
  working; the timeline adds no line for it.
- Under a line, one row per thread in `here`: its page number when it has
  one (`sidebarSections(...).numbers`), the first comment's text in one
  line (`plainText`, ellipsis), and its state at the right:
  `✓ v6` (addressed), `Send` (unsent), `Reply` (answered), `working` or
  `sent` (with the agent), the card's own waiting words shortened for
  `stalled` (in `--you-ink`), `resolved`; for a thread `sidebarSections`
  puts in Detached, a muted `detached ·` before its text, no number. `Send` and
  `Reply` are in `--you-ink`; they are labels on the row's one button, not
  separate controls.
- A row is a button (`aria-expanded`). Activating it expands the thread's
  **card** beneath it, rendered by `card`. Any number may be expanded. The
  set lives in `Sidebar.svelte`'s module state beside `unfolded`, keyed by
  thread ID, and is passed down with its setter, because `Sidebar.svelte`
  needs it for the Shift-range order. A thread of another page, which the
  list shows folded (`fold`), is rendered unfolded in the timeline: the
  row is its fold, and folding the card collapses the row. The `above` snippet (approval card, ClaxBot's offer)
  renders above the row whether or not it is expanded.
- The selected thread (`selected`) is always expanded, and when the
  selection changes its row scrolls into view (`block: "nearest"`). This is
  what a click on a pin does.
- A line whose rows include a thread that waits on a person carries the
  `wants` mark: its text in `--fg` instead of `--muted`. No tint.
- `onSeen(t)` fires for a collapsed row under the same rule a card uses
  (half visible for `SEEN_AFTER_MS`), so the changelog's "looked at"
  bookkeeping (`ctl.look`) keeps working when cards are not expanded. An
  expanded card reports itself as today.
- Days: the newest day that has lines is shown in full. Each earlier day
  is one summary row ("Thursday · 3 versions · 7 addressed"), a button
  that unfolds the day's lines in place (module state). A folded day still
  shows, beneath its summary, the rows of its threads that are pending, so
  nothing that needs someone is hidden by a fold. Selecting a thread in a
  folded day unfolds it.
- Empty (no versions and no threads cannot happen; an artifact has v1):
  with no threads at all, the hint the Open section shows today follows
  the v1 line.
- The band is an `aria-hidden` SVG positioned on the outer edge, the full
  height of the list, drawn from each rendered line's measured vertical
  centre (a `ResizeObserver` on the list redraws it; the path data comes
  from `band-model.ts`). Unit 4.5 px, ticks, no labels. The list leaves
  30 px of padding on that edge for it. A folded day is one row whose
  counts are those after its last line.
- Batch ticking keeps working: expanded cards have their box as today. In
  the Timeline view the Shift-range `order` is the expanded, selectable
  threads in the timeline's order, newest line first. (In the Threads view
  it stays what it is: the sections' threads.)

### What the Timeline view does not show

The "Addressed in vN" group and its sentence, and the Open, Detached and
Resolved headings with their counts. The threads are all still there, each
under its newest line. Their group headings are one tab away.

## The collapsed form

`ui/Rail.svelte` (new, in the sidebar's lazy chunk; `ShellSidebar.svelte`
exports it beside the default).

**When it shows**: wider than 700 px, the drawer docked left or right,
`s.panel` false, the view ready, the artifact not deleted, not in
inspecting mode (where the sidebar's chunk is never prefetched,
`artifact-main.ts`), and at least one thread exists. Docked at the bottom, floating, or on a phone, a closed
panel shows nothing, as today. **(call)** With no threads there is nothing
to draw and the page keeps its full width, as today.

**Box**: 56 px wide, on the side the drawer docks, full height of the
viewer, `aside.rail`, bordered toward the page like `.drawer`. Until the
chunk has loaded, `SidebarIsland.svelte` renders an empty
`<aside class="rail" aria-busy="true">` of the same box under the same
condition, so the frame does not move when the chunk arrives. The box's
rules live with the island's (eager CSS), the drawing's with the chunk.
Docked left, the island is after the stage in the document while the panel
is closed (`DrawerChrome.svelte` moves it only while mounted), so the
strip is placed with `order: -1` and Tab reaches it after the page;
accepted. Where the threads arrive after the view is ready (a load without
boot data), the strip appears when the first thread is known: one late
move of the frame on that path, accepted and recorded.

**Content**, top to bottom:

- the pending count, large, with "pending" under it; the waiting count in
  `--you-ink` with "need you" under it (omitted when zero);
- a tally on the outer edge: one mark per pending thread, orange for those
  waiting, capped at the band's `maxUnits`;
- a small dot beside the tally while `working` is not empty; it pulses only
  under `prefers-reduced-motion: no-preference`;
- the band on the outer edge, newest at the top, one fixed step (18 px) per
  line, unit 4.5 px, with version ticks and labels (`v6`), run counts, and
  a dotted break with the short weekday where the day changes. Lines that
  do not fit the height are not drawn; the band is clipped, not scrolled.

**Behaviour**: the whole strip is one button, `aria-label` such as
"Threads: 3 pending, 2 need you. Show threads", `data-shell-control=
"threads"`. Activating it calls `ctl.togglePanel()`. It is not a tab stop
trap and has no inner controls. The SVG is `aria-hidden`.

The topbar's own threads control and the drawer's "Hide threads" keep
working as they do; hiding the threads while docked left or right now
leaves the strip.

## Styling

Tokens from `theme.css` only: `--fg`, `--muted`, `--border`,
`--border-strong`, `--you`, `--you-ink`, `--font`. The band's grey is
`--border-strong` at reduced opacity or a new token `--band` beside the
others in `theme.css` (light `#bdb9ae`, dark `#5c5a54`) if contrast with
`--card` needs it: builder's choice, checked in both themes. No `--agent`
green in the timeline or the strip. Line text 14 px, times and counts
12 px, hairline `--border` between lines.

## Tests

- `view/timeline-model.test.ts`: a table of cases for the state machine and
  the lines. At least: unsent comment; comment sent at once (made folds
  into sent); comment made, another line between, then sent (made stays);
  batch of three; `sent_to_agent` without `sends`; addressed by a version
  with the agent's comment before the publish and with it after; agent
  answers without a version, person replies, version addresses; follow-up
  after addressed ("not quite"); resolved by a person and by an agent;
  two threads addressed by one version; `here` moves to the newest line;
  counts after each line; a version addressing an unknown thread ID; keys
  stable when a line is added.
- `view/band-model.test.ts`: path data for a three-row case in both
  directions; a zero-width run; the cap; label positions; no rows.
- Component tests beside `sidebar.test.ts`, in its style: tabs present
  only with `view`; Threads view renders today's sections; a row expands
  to a card; the selected thread is expanded; a folded day shows its
  pending rows; `onSeen` for a collapsed row with a fake clock; without
  `view`, the markup is what it was (the extension's contract).
- `web/e2e/timeline.spec.ts` (new): the sidebar opens on the Timeline; a
  comment sent and addressed shows under its version with `✓`; opening
  its row shows the card and a reply reaches the thread; the Threads tab
  shows the card list; the choice survives a reload; closing the panel
  docked right leaves the strip, clicking it reopens; docked left the
  strip is on the left; no strip with no threads.
- **Existing e2e specs keep testing the thread list.** `web/e2e/fixtures.ts`
  seeds `clax.sidebar` = `{"view":"threads"}` in an init script of
  `trustContext` (every context gets it, with or without a proof), only
  when the key is absent, so a test's own choice survives a reload. So the ~40 specs that find cards on open are untouched
  and still cover the Threads view; `timeline.spec.ts` sets the Timeline
  itself. **(call)** A spec that fails only because the strip now takes
  56 px when the panel is closed is updated to the new geometry, not
  silenced.
- No sleeps, injected clocks, per `AGENTS.md`.

## Constraints

- `AGENTS.md` whole: Bazel through `just`, no `unsafe`, no sleeps in
  tests, qualifier before editing a file and for what is deferred,
  conventional commit subjects, unsigned commits, nothing pushed.
- The sidebar's code stays out of the artifact entry: everything new but
  the strip's empty box loads lazily. `web/perf/bundle-budget.json` gates
  entries, not chunks: `artifact` (the island's condition and empty box
  grow it) and `extPanel` (the extension imports `Sidebar.svelte`
  statically, so whatever `Sidebar.svelte` imports statically grows it).
  `extPanel` must not grow beyond a few bytes of markup: `Sidebar.svelte`
  loads `Timeline.svelte`, the tab row and both models through
  `{#await import()}`, as it already loads `AddressedGroup`. If `artifact`
  must rise, raise it to the measured size and record it
  `status:needs-decision`.
- Svelte 5 runes, `css="injected"` components with `:global` rules, as
  the files beside them.
- Phone width and both themes must hold: the tab row fits 320 px.

## Open decisions (recorded for the owner, not built)

1. **Does silence settle a thread?** Today an addressed thread stays open
   until someone resolves it. The mock proposed resolving it when the next
   version publishes without a follow-up. Not built: it changes what
   `status` means for agents and for every other view. The timeline
   instead treats addressed as not pending, which gives the calm without
   changing the data.
2. **"Not quite…" as its own control.** The vignette had a button that
   opened a reply box. The card's reply box already does this, and a reply
   on a sent thread already goes to its agent, so no control was added.
3. **The strip when there are no threads**, and whether the strip can be
   dismissed entirely while docked at a side.
4. **The extension's side panel**: same two views later, or never.
5. **A send's time.** A single Send records no time, so the timeline cannot
   show when a comment was sent or how long it sat unsent. A `sent_at` on
   the thread view would fix it; it is a daemon change, left out here.
6. **True-time spacing** of the band (rejected for now: event order is
   compact and the mock that used clock time read worse).

Each is recorded with qualifier on this file.

## The vignette

The interactive mock the owner approved is published on the owner's Clax
as artifact `b705cnmgar17` (`play.html`), and its source is copied to
`docs/superpowers/specs/2026-11-13-timeline-sidebar-vignette.html` for
reference. It is a sketch: its data is simulated, it calls pending "open",
and it has a "Not quite…" button. Where it and
this document differ, this document holds. What to take from it: the look
of a line and a row, the band's geometry (`area`, `band`), the strip's
layout, and the rule that a thread sits under its newest line.

## Left to the builder

File-internal structure, CSS class names, the exact ARIA wiring within the
patterns named, how `Timeline.svelte` measures rows, test helper reuse,
and the wording of hints not given above.
