<svelte:options css="injected" />

<script lang="ts">
  // The strip a closed sidebar leaves on the edge it docks to (spec
  // 2026-11-13, "The collapsed form"): one button that shows the threads.
  // Top to bottom: how many threads are pending, and how many of them wait
  // on a person; on the outer edge a tally of the pending threads, the
  // waiting ones beyond the agents' as in the band, at most the band's
  // MAX_UNITS marks with the waiting share kept, and a dot beside it while
  // an agent works; then the band (band-model.ts), from the counts now at
  // the top down through the timeline's lines newest first, a fixed step per
  // line and a longer one, with a dotted break and the earlier day's short
  // name, where the day changes; labelled with the versions and the counts.
  // A line below `height` (the plot's, as the caller measures it) is not
  // drawn: the band is clipped, not scrolled. Inside the SVG every part is
  // one path, so no element there is added or removed as the counts change;
  // the labels are positioned over it.
  import { type BandRow, MAX_UNITS, band, strokes } from "../view/band-model";
  import { type Timeline, days } from "../view/timeline-model";

  type Props = { tl: Timeline; side: "left" | "right"; working: boolean; height: number; onOpen(): void };
  let p: Props = $props();

  /** The strip's width, one comment's width, where the band starts, where
   * the steps start, a line's step, and the extra step at a day's break. */
  const W = 56, UNIT = 4.5, TOP = 16, FIRST = 14, STEP = 18, BREAK = 20;
  const SHORT = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
  const pending = $derived(p.tl.pending.length);
  const waiting = $derived(p.tl.waiting.length);
  const needs = (n: number) => (n === 1 ? "needs you" : "need you");
  const label = $derived(`Threads: ${pending} pending${waiting ? `, ${waiting} ${needs(waiting)}` : ""}. Show threads`);
  const right = $derived(p.side === "right");
  const x0 = $derived(right ? W : 0);
  const dir = $derived(right ? 1 : -1);
  // The tally: past the cap the waiting marks keep their share, and one shows whenever a thread waits.
  const marks = $derived(Math.min(pending, MAX_UNITS));
  const orange = $derived(!waiting ? 0 : pending <= MAX_UNITS ? waiting : Math.max(1, Math.round((waiting / pending) * marks)));
  const mark = (i: number) => `M${right ? W - (i + 1) * UNIT + 0.75 : i * UNIT + 0.75},3 h${UNIT - 1.5} v10 h${1.5 - UNIT} Z`;
  const tally = $derived({
    agents: Array.from({ length: marks - orange }, (_, i) => mark(i)).join(" "),
    waits: Array.from({ length: orange }, (_, i) => mark(marks - orange + i)).join(" "),
  });
  // A day's short name, from its `YYYY-MM-DD`.
  const short = (date: string) => { const [y, m, d] = date.split("-").map(Number); return SHORT[new Date(y, m - 1, d).getDay()]; };
  const drawn = $derived.by(() => {
    const rows: BandRow[] = [];
    let y = FIRST;
    // The days' bounds are the lines' own; the clock only names days, which the strip does not.
    fit: for (const [i, d] of days(p.tl.lines, new Date()).entries()) {
      for (const [j, l] of d.lines.entries()) {
        const gap = i > 0 && j === 0;
        y += STEP + (gap ? BREAK : 0);
        if (y > p.height) break fit;
        rows.push({ y, pending: l.pending, waiting: l.waiting, v: l.kind === "version" ? l.v : undefined, gap: gap ? short(d.date) : undefined });
      }
    }
    // Above the newest line, the counts now.
    const present = rows.length ? [{ y: (TOP + rows[0].y) / 2, pending, waiting }] : [];
    return band([...present, ...rows], { top: TOP, x0, dir, unit: UNIT, labels: true });
  });
  const lines = $derived(strokes(drawn));
  const gaps = $derived(drawn.gaps.map(g => `M0,${g.y} H${W}`).join(" "));
  const labels = $derived([
    ...drawn.versions.map(v => ({ ...v, kind: "v" })),
    ...drawn.counts.map(c => ({ ...c, kind: "cnt" })),
    ...drawn.gaps.map(g => ({ x: x0 - dir * 30, y: g.y - 7, text: g.text, kind: "gap" })),
  ]);
</script>

<button type="button" class={["rail", p.side]} data-shell-control="threads" aria-label={label} onclick={() => p.onOpen()}>
  <span class="rail-n">{pending}</span><span class="rail-cap">pending</span>
  {#if waiting}<span class="rail-n you">{waiting}</span><span class="rail-cap">{needs(waiting)}</span>{/if}
  <span class="rail-plot" aria-hidden="true">
    <svg width={W} height={p.height}>
      <path class="rail-mark" d={tally.agents} /><path class="rail-mark waits" d={tally.waits} />
      <circle class={["rail-dot", p.working && "on"]} cx={x0 - dir * (marks * UNIT + 6)} cy="8" r="2" />
      <path class="rail-band-a" d={drawn.agent} /><path class="rail-band-w" d={drawn.waiting} /><path class="rail-spine" d={lines.spine} /><path class="rail-tick" d={lines.ticks} /><path class="rail-gap" d={gaps} />
    </svg>
    <span class={["rail-labels", drawn.anchor]}>
      {#each labels as l (`${l.kind}${l.y}`)}<span class={["rail-l", l.kind]} style:left={`${l.x}px`} style:top={`${l.y}px`}>{l.text}</span>{/each}
    </span>
  </span>
</button>

<style>
  :global {
    .viewer aside.rail { display: flex; flex-direction: column; min-height: 0; overflow: hidden; background: var(--bg); }
    button.rail { flex: 1; min-height: 0; width: 100%; display: flex; flex-direction: column; align-items: stretch; justify-content: flex-start; gap: 0;
      padding: 12px 0 0; background: none; border: 0; border-radius: 0; font: 400 11px/1.2 var(--font); color: var(--fg); white-space: normal; text-align: center; }
    button.rail:not(:disabled):hover { background: var(--hover); border-color: transparent; }
    /* Inside the strip, which clips what passes its edge. */
    button.rail:focus-visible { outline-offset: -2px; }
    .rail-n { font: 600 22px/1 var(--font); font-variant-numeric: tabular-nums; }
    .rail-n.you { margin-top: 4px; font-size: 15px; color: var(--you-ink); }
    .rail-cap { margin: 2px 0 6px; color: var(--muted); }
    .rail-plot { position: relative; flex: 1; min-height: 0; overflow: hidden; }
    .rail-plot > svg { position: absolute; left: 0; top: 0; overflow: visible; }
    .rail-mark, .rail-band-a { fill: var(--band); } .rail-mark.waits, .rail-band-w { fill: var(--you); }
    .rail-spine, .rail-tick, .rail-gap { fill: none; stroke-width: 1; }
    .rail-spine, .rail-gap { stroke: var(--border-strong); } .rail-tick { stroke: var(--fg); } .rail-gap { stroke-dasharray: 1 3; }
    .rail-dot { fill: var(--fg); } .rail-dot:not(.on) { display: none; }
    @media (prefers-reduced-motion: no-preference) { .rail-dot { animation: rail-pulse 1.6s ease-in-out infinite; } }
    @keyframes rail-pulse { 50% { opacity: .2; } }
    .rail-l { position: absolute; transform: translate(-100%, -50%); font: 10px/1 var(--font); color: var(--muted); font-variant-numeric: tabular-nums; white-space: nowrap; }
    .rail-labels.start .rail-l { transform: translate(0, -50%); }
    .rail-l.v { color: var(--fg); font-weight: 600; }
  }
</style>
