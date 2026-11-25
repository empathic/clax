# Timeline Sidebar Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The artifact view's sidebar opens on a newest-first timeline of events with a band of pending work on its outer edge, collapses to a 56 px strip, and keeps today's thread list as a second view.

**Architecture:** Two pure modules (`timeline-model.ts`: threads and versions to lines and states; `band-model.ts`: rows to SVG path data) feed two new components (`Timeline.svelte` inside `Sidebar.svelte`, `Rail.svelte` beside the island). `Sidebar.svelte` keeps its one `ThreadCard` wiring and renders it either in today's sections or under timeline rows. Nothing in the daemon changes.

**Tech Stack:** Svelte 5 (runes, `css="injected"`), TypeScript, Vitest (`//web:unit`), Playwright (`web/e2e`), Bazel through `just`.

**Spec:** `docs/superpowers/specs/2026-11-13-timeline-sidebar-design.md` (read it whole before any task; the vignette beside it shows the look and holds the band's geometry in `area()` and `band()`).

## Global Constraints

- `AGENTS.md` whole. In particular: `qualifier threads <path>` before editing a file; `just web-test` and `just test-affected` (never `cargo`/`pnpm` directly); commits with `git -c commit.gpgsign=false commit`, conventional subjects (`feat(shell): …`), no attribution lines, never pushed; no sleeps or wall-clock waits in tests; "ID" in prose.
- Run git as plain single commands from the worktree root (`/Users/alex/Devel/empathic/clax-timeline`): no `cd … &&` chains around git.
- No daemon, API or `ThreadCard.svelte` behaviour change. The extension's side panel (`web/extension/src/panel/Panel.svelte`) passes no new prop and must render as before.
- `web/perf/bundle-budget.json`: `extPanel` must not grow by more than a few bytes. `Sidebar.svelte` loads everything new through `{#await import()}`. If `artifact` must rise, raise it to the measured size and record it with qualifier, `status:needs-decision`.
- Tokens from `web/shell/src/theme.css` only; one new token allowed: `--band` (light `#bdb9ae`, dark `#5c5a54`, set in every theme block). No `--agent` green in the timeline or the strip. Orange (`--you`, `--you-ink`) only for what waits on a person.
- The UI word for the timeline's count is "pending", never "open".
- Doc comments state the contract, as the files beside them do.

## Review Focus

1. A thread's `addressed_in` names a version missing from `versions` (a pinned old version's view): no crash, the fact is skipped, the thread is not wrongly "addressed". Test in Task 1.
2. Facts with equal timestamps (a batch's threads made in the same millisecond; a version and a comment in the same second): order is stable and line keys are unique. Test in Task 1.
3. No viewer, or a viewer with no display name: nothing reads "You" by accident; sends show the name given. Test in Task 1.
4. More pending comments than `maxUnits`, and more lines than the strip is tall: the band is capped and clipped, the written counts stay true, nothing overflows the 56 px box. Tests in Tasks 2 and 4.
5. A spectator or comments-off view: rows still expand, and the card inside keeps its read-only or no-reply state because it is the same `cards` wiring. Test in Task 3.

---

### Task 1: The timeline model

**Files:**
- Create: `web/shell/src/view/timeline-model.ts`
- Test: `web/shell/src/view/timeline-model.test.ts`
- Modify: `web/shell/src/view/batch-model.ts` only if needed to share the unsent predicate.

**Interfaces:**
- Consumes: `Thread`, `Comment`, `Viewer`, `resolvedByLabel` (`../threads`); `Version` (`../api`); `Working`, `harnessName` (`./working-model`); `agentName` (`./history-model`); `unsent` (`./batch-model`).
- Produces:

```ts
export type ThreadState =
  | { kind: "unsent" } | { kind: "with-agent"; working: boolean }
  | { kind: "answered"; agent: string } | { kind: "stalled"; why: "withheld" | "declined" | "no_access" | "agent_ended" }
  | { kind: "addressed"; v: number } | { kind: "resolved" };
export type LineKind = "version" | "made" | "sent" | "answered" | "followed-up" | "resolved";
export type Line = {
  key: string; at: string; kind: LineKind;
  who: string;        // display name, or the agent's name
  agent: boolean; mine: boolean;   // `mine`: the viewer did it
  v?: number; note?: string | null; size?: number;  // version; a batch's size and note
  ids: string[]; here: string[];
  pending: number; waiting: number;
};
export type Timeline = { lines: Line[]; pending: string[]; waiting: string[]; stateOf: Map<string, ThreadState> };
export type TimelineInput = { threads: Thread[]; versions: Version[]; working: Working[]; claimed: readonly string[]; me?: Viewer | null };
export function timeline(i: TimelineInput): Timeline;
/** The line's words, and a second row when it has one (a batch's note). */
export function lineText(l: Line, stateOf: Map<string, ThreadState>): { text: string; sub?: string };
/** The row's state at the right: its words and whether it asks the viewer to act. */
export function rowStatus(s: ThreadState): { text: string; act: boolean };
export const isPending = (s: ThreadState): boolean;   // unsent, with-agent, answered, stalled
export const isWaiting = (s: ThreadState): boolean;   // unsent, answered, stalled
export type Day = { label: string; date: string; lines: Line[] };   // lines newest first
/** The lines grouped by local day, newest day first: "Today", "Yesterday", else weekday and date. */
export function days(lines: Line[], now: Date): Day[];
/** A folded day's summary: "Thursday · 3 versions · 7 addressed". */
export function daySummary(d: Day): string;
```

Rules: the spec's "The model" section is the contract, every bullet of it. `rowStatus` words: `✓ v6`, `Send`, `Reply`, `working`, `sent`, `resolved`; stalled: `approve` (withheld), `declined`, `no access`, `no agent` (agent_ended). `act` is true for Send, Reply and the stalled ones.

- [ ] **Step 1: Write the failing tests.** One `describe("timeline")` with a small `thread({...})` / `version(n, at, addresses, note?)` builder and these cases, each asserting `lines.map(l => [l.kind, l.ids, l.here, l.pending, l.waiting])` and the relevant `stateOf`:
  - an unsent comment: one `made` line, pending 1, waiting 1, state `unsent`;
  - `sent_to_agent` true with no `sends`: one `made` line, no `sent` line, state `with-agent`, waiting 0;
  - a thread in `claimed`: the same;
  - a batch of three (`sends` sharing a `batch_id`): the three `made` facts fold into one `sent` line with `size` 3 and the note;
  - made, then a version line between, then sent: `made` stays its own line;
  - addressed by a version, agent's comment 2 s before the version's `created_at`: no `answered` line, state `addressed`;
  - the same with the agent's comment 2 s after: the same result;
  - agent comments with no version: `answered` line, state `answered`, waiting 1; then a person's reply: `followed-up`, state `with-agent`; then a version addressing it: `addressed`;
  - a person's comment after addressed (not quite): `followed-up`, pending again, with the agent;
  - resolved by a viewer and by `agent:claude`: `resolved` lines, `mine` true only for the viewer's own;
  - one version addressing two threads: one line, both in `ids` and `here`;
  - `here` moves: a thread sent then addressed is in `here` of the version line only;
  - `addressed_in: [9]` with no version 9: skipped, state not `addressed` (Review Focus 1);
  - two threads made at the identical timestamp by the same author: one merged `made` line, keys unique across all lines (Review Focus 2);
  - `me` null, and `me` with `display_name: null`: no line has `mine` true from a send; a comment whose `author_public_id` matches is `mine` (Review Focus 3);
  - `working` listing a with-agent thread: `working: true`; `feedback_state.state` of `withheld`, `declined`, `no_access`, `agent_ended`: `stalled`, waiting;
  - keys are stable: `timeline(a)` keys are a subset of `timeline(a + one newer thread)` keys.
  Then `describe("lineText")` for each wording in the spec's "Wording" section, `describe("rowStatus")` for each state, and `describe("days")` with a fixed `now`: Today, Yesterday, an older weekday label, and `daySummary`.
- [ ] **Step 2: Run** `just web-test` **and see the new tests fail** (module not found).
- [ ] **Step 3: Implement** `timeline-model.ts` to the interfaces above. Pure; no `Date.now()`; sort with a stable comparison on ISO strings as `history-model.ts` does.
- [ ] **Step 4: Run** `just web-test`**; all pass** (lint and typecheck included).
- [ ] **Step 5: Commit** `feat(shell): the timeline model reads threads and versions as lines and states`.

### Task 2: The band's geometry

**Files:**
- Create: `web/shell/src/view/band-model.ts`
- Test: `web/shell/src/view/band-model.test.ts`

**Interfaces:**
- Produces:

```ts
/** Rows newest first; `y` grows downward. `gap`: the day label where a new day begins below this row's predecessor. */
export type BandRow = { y: number; pending: number; waiting: number; v?: number; gap?: string };
export type BandOpts = { top: number; x0: number; dir: 1 | -1; unit: number; maxUnits?: number; labels?: boolean };
export type Band = {
  agent: string; waiting: string;                        // path data, "" with no rows
  spine: { x: number; y1: number; y2: number } | null;
  ticks: { x1: number; x2: number; y: number }[];        // one per version row
  versions: { x: number; y: number; text: string }[];    // with `labels`
  counts: { x: number; y: number; text: string }[];      // with `labels`
  gaps: { y: number; text: string }[];                   // with `labels`
  anchor: "start" | "end";
};
export const MAX_UNITS = 6;
export function band(rows: BandRow[], o: BandOpts): Band;
```

Geometry (from the vignette's `area()` and `band()`): x for a width `w` is `x0 - dir * min(w, maxUnits) * unit`. Above a row the band is that row's width; it runs straight to the next newer row; at a row's `y` it tapers over `y - 4 … y + 4`; below the oldest row (`y + 12`) it ends at width 0. The agent area spans 0 to `pending - waiting`, the waiting area `pending - waiting` to `pending`. A tick runs from `x0` to 5 px past the wider of the widths above and below the row. A count label sits 3 px past the band, centred on its run, for each run whose count is not 0 and differs from the run below; its text is the true count even when capped. `anchor` is `"end"` for `dir` 1.

- [ ] **Step 1: Write the failing tests:** three rows `[{y:20,pending:3,waiting:2,v:6},{y:40,pending:5,waiting:1},{y:60,pending:1,waiting:1}]` with `{top:0,x0:56,dir:1,unit:4.5}`: assert the exact `agent` and `waiting` strings, one tick `{x1:56, x2: 56 - 5*4.5 - 5, y:20}`; the same rows with `dir:-1, x0:0` mirror every x; a run of width 0 draws no area segment there; `pending: 9` caps the width at 6 units while `counts` holds `"9"` (Review Focus 4); `labels` off gives empty `versions`, `counts`, `gaps`; no rows gives `""`, `null` and empty arrays.
- [ ] **Step 2: Run** `just web-test`**; fails.**
- [ ] **Step 3: Implement** `band()`.
- [ ] **Step 4: Run** `just web-test`**; passes.**
- [ ] **Step 5: Commit** `feat(shell): the band's geometry, from rows to path data`.

### Task 3: Two views in the sidebar

**Files:**
- Create: `web/shell/src/ui/Timeline.svelte`, `web/shell/src/ui/SidebarTabs.svelte`, `web/shell/src/ui/sidebar-view.svelte.ts`, `web/shell/src/timeline.test.ts` (component tests, in the style of `web/shell/src/sidebar.test.ts`)
- Modify: `web/shell/src/ui/Sidebar.svelte`, `web/shell/src/ui/ShellSidebar.svelte`, `web/shell/src/theme.css` (the `--band` token only), `web/e2e/fixtures.ts`

**Interfaces:**
- Consumes: Task 1's and Task 2's exports.
- Produces:

```ts
// ui/sidebar-view.svelte.ts
export type SidebarView = "timeline" | "threads";
export const SIDEBAR_KEY = "clax.sidebar";           // localStorage: {"view": SidebarView}
export const sidebarView: { readonly now: SidebarView; set(v: SidebarView): void };  // default "timeline"; storage errors ignored

// Sidebar.svelte, new optional prop
view?: { now: SidebarView; onChange(v: SidebarView): void };

// Timeline.svelte props
{ tl: Timeline; threads: Thread[]; now: Date; selected: string | null; numbers: Map<string, number>;
  detached: ReadonlySet<string>; side: "left" | "right";
  expanded: readonly string[]; onExpand(id: string, open: boolean): void;
  card: Snippet<[Thread]>; above?: Snippet<[Thread, Date]>;
  onSeen?(t: Thread): void; empty?: Snippet }

// SidebarTabs.svelte props
{ now: SidebarView; waiting: number; onChange(v: SidebarView): void; onNeeds(): void }
```

Decisions:
- `Sidebar.svelte` adds module state `let expanded: string[] = $state([])` beside `unfolded`. With `view`, it computes `tl = timeline({ threads: p.threads, versions: p.versions, working: p.working ?? [], claimed: p.claimed ?? [], me: p.me })` (the model imported lazily: the whole Timeline branch, tab row included, sits inside one `{#await import("./Timeline.svelte")}`-style block so `extPanel` gets none of it; put the model call inside `Timeline.svelte`'s chunk by passing the raw inputs if that is what keeps `Sidebar.svelte` free of a static import, and adjust the prop list above accordingly while keeping the names).
- The `cards` snippet gains a one-thread form the timeline uses; for a thread where `far(t)` is true it passes `fold={{ open: true, onToggle: o => { if (!o) collapse(t.id) } }}`.
- `order` in the Timeline view: the expanded, selectable threads in timeline order, newest line first.
- The band's side: `"left"` when `document.documentElement.dataset.drawer === "left"`, else `"right"`; `ShellSidebar.svelte` passes it from `drawer.prefs.layout`.
- `ShellSidebar.svelte` passes `view={{ now: sidebarView.now, onChange: v => sidebarView.set(v) }}`.
- `onNeeds` selects the newest waiting thread (`p.onSelect`).
- Rows: a `<button aria-expanded>` per thread; selected thread always expanded and scrolled into view (`block: "nearest"`) when `selected` changes; collapsed rows report `onSeen` after `SEEN_AFTER_MS` half visible, using `after` from `../clock` as `ThreadCard.svelte` does.
- Days: newest day full; earlier days folded to `daySummary`, a button; a folded day lists its pending threads' rows; selecting a thread in a folded day unfolds it. Folded-day set is module state.
- `web/e2e/fixtures.ts`: in `trustContext`, an init script that sets `localStorage["clax.sidebar"] = '{"view":"threads"}'` only when the key is absent, wrapped in try/catch.

- [ ] **Step 1: Write the failing component tests** in `web/shell/src/timeline.test.ts`: without `view` the rendered sidebar has no tablist and the same sections as `sidebar.test.ts` expects; with `view.now = "threads"` a tablist and today's sections; with `"timeline"` no `.section-open`, a line per event newest first, a row under the newest line only; clicking a row renders a `.thread-card` for that thread and sets `aria-expanded="true"`; `selected` expands its row; a folded earlier day shows its summary and its pending rows only, and unfolds on click; a collapsed row calls `onSeen` after the fake clock advances `SEEN_AFTER_MS`; with `readOnly` the expanded card has no reply box (Review Focus 5); the tab buttons call `onChange`; "N need you" calls `onSelect` with the newest waiting thread. And `sidebar-view`: default `"timeline"`, `set` persists, a throwing `localStorage` leaves the default.
- [ ] **Step 2: Run** `just web-test`**; the new tests fail.**
- [ ] **Step 3: Implement** the files above. Styling per the spec's "Styling" and the vignette's `.ev`, `.row`, `.day`, `.tab` rules, renamed to fit `Sidebar.svelte`'s `:global` style block conventions.
- [ ] **Step 4: Run** `just web-test`**; all pass, including `//web:bundle_size`.** If `extPanel` grew by more than a few bytes, move the offending import behind the lazy boundary.
- [ ] **Step 5: Commit** `feat(shell): the sidebar opens on a timeline, with the thread list as its second view`.

### Task 4: The collapsed strip

**Files:**
- Create: `web/shell/src/ui/Rail.svelte`, `web/shell/src/ui/ShellRail.svelte`, `web/shell/src/rail.test.ts`
- Modify: `web/shell/src/ui/SidebarIsland.svelte`, `web/shell/src/ui/sidebar-chunk.ts`

**Interfaces:**
- Consumes: `timeline`, `band`, `days` from Tasks 1 and 2; `botView.claimed` (`../bot/send.svelte`).
- Produces:

```ts
// sidebar-chunk.ts
export function loadRail(): Promise<typeof import("./ShellRail.svelte")>;   // once, retried after a failure, as loadSidebar
/** Whether the closed panel shows the strip. */
export function railShows(s: { panel: boolean; threads: unknown[]; deleted: boolean }, layout: string | undefined, wide: boolean, inspecting: boolean): boolean;
// Rail.svelte props
{ tl: Timeline; side: "left" | "right"; working: boolean; height: number; onOpen(): void }
// ShellRail.svelte props
{ ctl: ArtifactController; s: ViewState & { data: Loaded } }
```

Decisions:
- `railShows`: `!s.panel && !s.deleted && s.threads.length > 0 && wide && !inspecting && (layout === "left" || layout === "right" || !layout)` (`""`/absent is docked right). `wide` is `matchMedia("(min-width: 701px)")`, followed live.
- `SidebarIsland.svelte`: when `railShows`, render `<ShellRail>` once loaded, else `<aside class="rail" aria-busy="true"></aside>`. Its eager CSS gains the box: 56 px wide, full height, `border` toward the page as `.drawer`, `order: -1` under `[data-drawer="left"]`. The strip's load is requested under the same `afterPaint` rule as the sidebar's.
- `Rail.svelte`: one `<button class="rail" data-shell-control="threads" aria-label="Threads: 3 pending, 2 need you. Show threads">` (the "need you" clause omitted at zero), calling `onOpen` (`ctl.togglePanel()`). Content per the spec's "The collapsed form": counts, tally (capped at `MAX_UNITS`), working dot (pulse only under `prefers-reduced-motion: no-preference`), band with `labels`, step 18 px per line, an extra 20 px and a `gap` label at each day change, rows that do not fit `height` dropped. Height from a `ResizeObserver` on the plot.
- [ ] **Step 1: Write the failing tests** in `web/shell/src/rail.test.ts`: `railShows` truth table (panel open, no threads, deleted, bottom, float, narrow, inspecting each false; left, right and absent layout true); `Rail` renders the label with and without the waiting clause, a tally of `min(pending, 6)` marks with the waiting ones marked, calls `onOpen` on click and on Enter, draws no row whose `y` exceeds `height` (Review Focus 4), and has an `aria-hidden` SVG; the island renders the busy placeholder before the chunk resolves and the strip after.
- [ ] **Step 2: Run** `just web-test`**; fails.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `just web-test`**; passes.** If `artifact` is over budget, raise it to the measured size in `web/perf/bundle-budget.json` and record it (`qualifier record concern web/perf/bundle-budget.json "…" --tag status:needs-decision`, stating the bytes and what they buy).
- [ ] **Step 5: Commit** `feat(shell): a closed sidebar leaves a strip that shows what is pending`.

### Task 5: End to end, gates and records

**Files:**
- Create: `web/e2e/timeline.spec.ts`
- Modify: whatever existing e2e spec fails only because of the strip's 56 px or the tab row; `docs/usage.md` or the doc that describes the sidebar to users, if one does (search `docs/` for "sidebar" and "threads"); `.qual` files through `qualifier`.

- [ ] **Step 1: Write** `web/e2e/timeline.spec.ts` with the fixtures the neighbouring specs use (`createThread`, publishing a version with `addresses`, see `web/e2e/fixtures.ts` and `changelog.spec.ts`). Each test first sets `clax.sidebar` to `{"view":"timeline"}` before load. Cases: the sidebar opens on the Timeline tab; a thread sent and addressed by v2 shows under the `v2` line with `✓ v2`; opening its row shows its card and a reply posted there appears in the thread; the Threads tab shows the card list and the choice survives a reload; hiding the threads docked right leaves `button.rail` and clicking it reopens the sidebar; docked left (`clax.drawer` = `{"layout":"left"}`) the strip's box is at the viewer's left; an artifact with no threads shows no strip.
- [ ] **Step 2: Run the e2e lanes** (`just web-e2e`, in the background, logged under `target/`) **and fix what fails.** A spec that fails for the new geometry is updated to it; a spec that fails for any other reason is a bug in Tasks 3 and 4: fix the code.
- [ ] **Step 3: Run** `just check` **once** (background, logged, bounded waits). Everything passes, or a lane that fails in code this branch did not touch is rerun alone and both results reported.
- [ ] **Step 4: Record with qualifier** (one `--stdin` batch written to a file outside the repository, dry-run first): the five open decisions of the spec's "Open decisions" on the spec file (`alternative` with a `revisit:` tag, or `concern` with `status:needs-decision` for the two that are the owner's call: silence settling a thread, and a send's time), and a `concern` on `SidebarIsland.svelte` for the strip's late appearance on a load without boot data.
- [ ] **Step 5: Commit** the tests and docs as `test(e2e): the timeline sidebar end to end`, and the `.qual` changes separately as `chore(qual): the timeline sidebar's open decisions`.
