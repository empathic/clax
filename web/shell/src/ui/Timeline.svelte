<svelte:options css="injected" />

<script lang="ts" module>
  /** The earlier days unfolded, by local date, for the page's life. */
  let unfolded: string[] = $state([]);
</script>

<script lang="ts">
  // The sidebar's timeline (spec 2026-11-13, "ui/Timeline.svelte"): the
  // artifact's lines newest first under day headings, each thread listed
  // once, as a row under its newest line. A row opens into the thread's
  // card (`card`, the sidebar's own); the selected thread's row opens, and
  // scrolls into view, whenever the selection changes, and the row the
  // previous selection opened closes again unless the viewer took it in hand.
  // The newest day is whole; each earlier one is folded to its summary, with
  // the rows of its threads still pending, and of those open into their
  // cards, beneath it, until it is unfolded or a thread in it is selected.
  // A collapsed row reports its thread looked at as a card does. The band on the list's outer edge shows how many threads are
  // pending at each line, and how many of them wait on a person
  // (band-model.ts): from the top to the first line, the counts now.
  import { type Snippet, tick, untrack } from "svelte";
  import type { Thread } from "../threads";
  import { relativeTime } from "../format";
  import { after } from "../clock";
  import { type Day, type Line, type Timeline, dayKey, daySummary, days, isPending, isWaiting, lineText, rowStatus } from "../view/timeline-model";
  import { type BandRow, band, strokes } from "../view/band-model";
  import { selectable } from "../view/batch-model";
  import { SEEN_AFTER_MS } from "../view/sidebar-model";
  import { plainText } from "../view/md-load.svelte";

  type Props = {
    tl: Timeline; threads: Thread[]; now: Date; selected: string | null;
    /** The pins' numbers; the threads the thread list puts under Detached. */
    numbers: Map<string, number>; detached: ReadonlySet<string>;
    /** The sidebar's outer edge, where the band is drawn. */
    side: "left" | "right";
    /** The rows open into their cards, and their switch. */
    expanded: readonly string[]; onExpand(id: string, open: boolean): void;
    /** A thread's card under its row, given the Shift-range order (the
     * open, selectable rows, newest line first) and the row's close. */
    card: Snippet<[Thread, { order: string[]; close(): void }]>;
    /** What goes above a thread's row, open or not. */
    above?: Snippet<[Thread, Date]>;
    onSeen?(t: Thread): void;
    /** Follows the lines when there are no threads. */
    empty?: Snippet;
  };
  let p: Props = $props();
  const ds = $derived(days(p.tl.lines, p.now));
  const byId = $derived(new Map(p.threads.map(t => [t.id, t])));
  const whole = (i: number, d: Day) => i === 0 || unfolded.includes(d.date);
  // A folded day's rows: those pending, and those open into their cards.
  const keptIn = (d: Day) => d.lines.flatMap(l => l.here).filter(id => p.expanded.includes(id) || isPending(p.tl.stateOf.get(id) ?? { kind: "resolved" }));
  const waits = (l: Line) => l.here.some(id => isWaiting(p.tl.stateOf.get(id) ?? { kind: "resolved" }));
  let list: HTMLElement | undefined = $state();
  // What is shown, in order, as one flat list: day headings, line heads and
  // thread rows, each keyed by what it stands for. A row is keyed by its
  // thread alone, so when the thread's newest line changes its row, and
  // the card open under it, move rather than being made anew. `end` closes
  // a line's group (its head, or its last row).
  type Item = { key: string; d: Day; i: number } & ({ kind: "day" } | { kind: "line"; l: Line; end: boolean } | { kind: "row"; id: string; end: boolean });
  const entries = $derived(ds.flatMap((d, i): Item[] => [
    { key: `day:${d.date}`, kind: "day", d, i },
    ...(whole(i, d)
      ? d.lines.flatMap((l): Item[] => [
        { key: `line:${l.key}`, kind: "line", d, i, l, end: !l.here.length },
        ...l.here.map((id, k): Item => ({ key: `row:${id}`, kind: "row", d, i, id, end: k === l.here.length - 1 })),
      ])
      : keptIn(d).map((id): Item => ({ key: `row:${id}`, kind: "row", d, i, id, end: false }))),
  ]));
  // The rows shown, in order; the open, selectable ones are a Shift range's order.
  const shown = $derived(entries.flatMap(x => (x.kind === "row" ? [x.id] : [])));
  const order = $derived(shown.filter(id => { const t = byId.get(id); return !!t && p.expanded.includes(id) && selectable(t, false); }));
  // A row that holds focus (its button, what is above it, or its card)
  // keeps it when it moves: moving an element can take focus from it to
  // nowhere, and it is given back. `held` is what focus is in inside a
  // row; it is let go once focus has gone elsewhere (to another element,
  // or to nowhere for longer than the move). A moved card holding focus is
  // scrolled into view.
  let held: HTMLElement | null = null;
  const focusin = (e: FocusEvent) => { held = e.target instanceof HTMLElement && e.target.closest(".tl-thread") ? e.target : null; };
  const focusout = (e: FocusEvent) => {
    const t = e.target;
    if (e.relatedTarget) held = null;
    else queueMicrotask(() => { if (held === t && document.activeElement !== t) held = null; });
  };
  let under = new Map<string, string>();
  $effect(() => {
    const now = new Map<string, string>();
    let line = "";
    for (const x of entries) if (x.kind === "row") now.set(x.id, line); else line = x.key;
    const was = under;
    under = now;
    const el = held;
    if (!el?.isConnected) return;
    const a = document.activeElement;
    if (a !== el && (!a || a === document.body)) el.focus({ preventScroll: true });
    const card = el.closest<HTMLElement>(".thread-card");
    const id = card?.dataset.thread;
    if (document.activeElement === el && id && was.has(id) && was.get(id) !== now.get(id)) card.scrollIntoView?.({ block: "nearest" });
  });
  // A line's time: relative today, the clock time on an earlier day.
  const today = $derived(dayKey(p.now));
  const when = (d: Day, l: Line) => (d.date === today ? relativeTime(l.at, p.now) : new Date(l.at).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" }));

  // The row the last selection opened, until the viewer opens or closes it
  // by hand. When the selection moves to another thread it closes again,
  // unless its card holds focus or a reply draft.
  let auto: string | null = null;
  const toggle = (id: string, open: boolean) => { if (auto === id) auto = null; p.onExpand(id, open); };
  const keeps = (id: string) => {
    const box = [...(list?.querySelectorAll<HTMLElement>(".tl-row") ?? [])].find(r => r.dataset.row === id)?.parentElement;
    return !!box && (box.contains(document.activeElement) || !!box.querySelector<HTMLInputElement>('input[aria-label="Reply"]')?.value.trim());
  };
  $effect(() => {
    const sel = p.selected;
    if (!sel) return;
    untrack(() => {
      const d = ds.find((x, i) => i > 0 && x.lines.some(l => l.here.includes(sel)));
      if (d && !unfolded.includes(d.date)) unfolded = [...unfolded, d.date];
      if (auto && auto !== sel && p.expanded.includes(auto) && !keeps(auto)) p.onExpand(auto, false);
      auto = null;
      if (!p.expanded.includes(sel)) { p.onExpand(sel, true); auto = sel; }
      void tick().then(() => [...(list?.querySelectorAll<HTMLElement>(".tl-row") ?? [])].find(r => r.dataset.row === sel)?.scrollIntoView?.({ block: "nearest" }));
    });
  });

  // A collapsed row is looked at once it has been half visible for
  // SEEN_AFTER_MS. The thread is read by ID when that time is up, so an
  // update of the thread neither restarts the wait nor reports it stale.
  const seen = (id: string) => (el: HTMLElement) => {
    if (!untrack(() => p.onSeen) || typeof IntersectionObserver !== "function") return;
    let cancel = () => {};
    const io = new IntersectionObserver(([e]) => {
      cancel();
      if (e.intersectionRatio >= 0.5) cancel = after(SEEN_AFTER_MS, () => untrack(() => { const t = byId.get(id); if (t) p.onSeen?.(t); }));
    }, { threshold: [0, 0.5] });
    io.observe(el);
    return () => { cancel(); io.disconnect(); };
  };

  // The band: one row per line shown and per folded day (its counts after
  // its newest line), at the vertical centre of its heading as measured,
  // remeasured when what is shown changes or the list changes size.
  const marks = $derived(ds.flatMap((d, i) => (whole(i, d)
    ? d.lines.map(l => ({ key: l.key, pending: l.pending, waiting: l.waiting, v: l.kind === "version" ? l.v : undefined }))
    : [{ key: `day:${d.date}`, pending: d.lines[0].pending, waiting: d.lines[0].waiting, v: undefined }])));
  const shape = $derived(`${marks.map(x => x.key).join()}|${p.expanded.join()}`);
  let ys: Map<string, number> = $state.raw(new Map());
  let height = $state(0);
  const measure = () => {
    if (!list) return;
    const top = list.getBoundingClientRect().top;
    ys = new Map([...list.querySelectorAll<HTMLElement>("[data-band]")].map(e => { const r = e.getBoundingClientRect(); return [e.dataset.band!, r.top - top + r.height / 2]; }));
    height = list.scrollHeight;
  };
  $effect(() => { void shape; untrack(measure); });
  $effect(() => {
    if (!list || typeof ResizeObserver !== "function") return;
    const ro = new ResizeObserver(measure);
    ro.observe(list);
    return () => ro.disconnect();
  });
  const W = 30;
  const drawn = $derived.by(() => {
    const rows: BandRow[] = marks.flatMap(x => { const y = ys.get(x.key); return y === undefined ? [] : [{ y, pending: x.pending, waiting: x.waiting, v: x.v }]; });
    const present = rows.length ? [{ y: rows[0].y / 2, pending: p.tl.pending.length, waiting: p.tl.waiting.length }] : [];
    return band([...present, ...rows], { top: 0, x0: p.side === "right" ? W : 0, dir: p.side === "right" ? 1 : -1, unit: 4.5 });
  });
  const lines = $derived(strokes(drawn));
</script>

<!-- A row's ID is its key's, so it never changes; read untracked, the watch
     on a collapsed row is not restarted when the list around it is recomputed. -->
{#snippet row(id: string, end: boolean)}
  {@const t = byId.get(id)}
  {#if t}
    {@const st = rowStatus(p.tl.stateOf.get(id) ?? { kind: "resolved" })}
    {@const open = p.expanded.includes(id)}
    <div class={["tl-thread", end && "tl-end"]}>
      {@render p.above?.(t, p.now)}
      <button type="button" class="tl-row" data-row={id} aria-expanded={open} onclick={() => toggle(id, !open)} {@attach !open && seen(untrack(() => id))}
        ><span class="tl-n">{p.detached.has(id) ? "detached" : (p.numbers.get(id) ?? "")}</span><span class="tl-q">{plainText(t.comments[0]?.body ?? "")}</span><span class={["tl-st", st.act && "act"]}>{st.text}</span></button>
      {#if open}{@render p.card(t, { order, close: () => toggle(id, false) })}{/if}
    </div>
  {/if}
{/snippet}

<div class={["timeline", p.side]} bind:this={list} onfocusin={focusin} onfocusout={focusout}>
  <svg class="tl-band" width={W} height={height} aria-hidden="true">
    <path class="tl-band-a" d={drawn.agent} /><path class="tl-band-w" d={drawn.waiting} /><path class="tl-spine" d={lines.spine} /><path class="tl-tick" d={lines.ticks} />
  </svg>
  {#each entries as e (e.key)}
    {@const d = e.d}
    {#if e.kind === "row"}
      {@render row(e.id, e.end)}
    {:else if e.kind === "line"}
      {@const l = e.l}
      {@const x = lineText(l, p.tl.stateOf)}
      <div class={["tl-line", waits(l) && "wants", e.end && "tl-end"]}>
        <div class="tl-head" data-band={l.key}><p class="tl-text">{x.text}</p><time class="tl-time" datetime={l.at}>{when(d, l)}</time></div>
        {#if x.sub}<p class="tl-sub">{x.sub}</p>{/if}
      </div>
    {:else if whole(e.i, d)}
      <!-- Days are headed at the level of the thread list's sections (h2). -->
      <h2 class="tl-day">{#if e.i}<button type="button" class="tl-fold" aria-expanded="true" onclick={() => (unfolded = unfolded.filter(x => x !== d.date))}>{d.label}</button>{:else}{d.label}{/if}</h2>
    {:else}
      <h2 class="tl-day"><button type="button" class="tl-fold" data-band={`day:${d.date}`} aria-expanded="false" onclick={() => (unfolded = [...unfolded, d.date])}>{daySummary(d)}</button></h2>
    {/if}
  {/each}
  {#if !p.threads.length}{@render p.empty?.()}{/if}
</div>

<style>
  :global {
    .timeline { position: relative; padding-right: 30px; }
    .timeline.left { padding-right: 0; padding-left: 30px; }
    .timeline > .tl-band { position: absolute; top: 0; right: -14px; overflow: visible; pointer-events: none; }
    .timeline.left > .tl-band { right: auto; left: -14px; }
    .tl-band-a { fill: var(--band); } .tl-band-w { fill: var(--you); }
    .tl-spine, .tl-tick { fill: none; stroke-width: 1; } .tl-spine { stroke: var(--border-strong); } .tl-tick { stroke: var(--fg); }
    .tl-day { margin: 16px 0 0; font: 400 12px/1.4 var(--font); color: var(--muted); }
    .timeline > .tl-day:first-of-type { margin-top: 0; }
    .tl-fold { min-height: 24px; padding: 2px 0; gap: 6px; justify-content: flex-start; background: none; border: 0; border-radius: 0; font: inherit; color: inherit; white-space: normal; text-align: left; }
    .tl-fold:not(:disabled):hover { background: none; color: var(--fg); }
    .tl-fold::before { content: "▸"; color: var(--fg); } .tl-fold[aria-expanded="true"]::before { content: "▾"; }
    /* A line's group: its head, then its rows; the last of them closes it. */
    .tl-line { padding-top: 10px; } .tl-end { padding-bottom: 10px; border-bottom: 1px solid var(--border); }
    .tl-head { display: grid; grid-template-columns: minmax(0, 1fr) auto; column-gap: 12px; align-items: baseline; }
    .tl-text { margin: 0; font-size: 14px; line-height: 1.45; color: var(--muted); overflow-wrap: anywhere; }
    .tl-line.wants .tl-text { color: var(--fg); }
    .tl-time { font-size: 12px; color: var(--muted); font-variant-numeric: tabular-nums; white-space: nowrap; }
    .tl-sub { margin: 2px 0 0; font-size: 13px; color: var(--muted); overflow-wrap: anywhere; }
    .tl-thread { margin-top: 4px; }
    .tl-row { display: grid; grid-template-columns: minmax(18px, auto) minmax(0, 1fr) auto; gap: 8px; align-items: baseline; width: 100%; min-height: 28px; padding: 4px 0;
      background: none; border: 0; border-radius: 0; text-align: left; font: 400 13px/1.45 var(--font); color: var(--muted); }
    .tl-row:not(:disabled):hover { background: none; }
    .tl-row:hover .tl-q, .tl-row[aria-expanded="true"] .tl-q { color: var(--fg); }
    .tl-n, .tl-st { font-size: 12px; font-variant-numeric: tabular-nums; white-space: nowrap; }
    .tl-q { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
    .tl-st.act { color: var(--you-ink); }
    .tl-thread > .thread-card { margin: 4px 0 8px; }
    @media (max-width: 700px) { .tl-row, .tl-fold { min-height: 40px; } }
  }
</style>
