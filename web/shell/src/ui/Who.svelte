<svelte:options css="injected" />

<script lang="ts">
  // The top bar's participants and working summary (spec §8, "Working"): the
  // roster, then two lines saying who works on what, with the elapsed time;
  // its styles carry the sweep under the top bar while anyone works, and the
  // split pins of the threads an agent works on.
  // Not needed for the first paint: the artifact entry loads it after (`more-menu.svelte.ts`).
  import type { Loaded, ViewState } from "../view/artifact-controller";
  import { agentNames, summary } from "../view/working-model";
  import Roster from "./Roster.svelte";
  import { ticker } from "./ticker.svelte";
  import WorkingSummary from "./WorkingSummary.svelte";

  const p: { s: ViewState & { data: Loaded }; open: number } = $props();
  // The elapsed clock ticks each second while anyone works.
  const tick = ticker(() => p.s.working.length > 0, () => undefined);
  const parts = $derived(p.s.data.artifact.participants ?? { people: [], agents: [] });
  const names = $derived(agentNames(p.s.working, parts.agents));
  const busy = $derived(new Set(p.s.working.map(w => w.agent)));
  const sum = $derived(summary({
    working: p.s.working, names, mine: new Set(p.s.attention?.open_in ?? []), open: p.open, now: tick.now,
    idle: parts.agents.filter(a => a.live && !busy.has(a.handle)).map(a => names.get(a.handle) ?? a.harness), addressed: p.s.decided?.line ?? null, published: p.s.deleted ? null : p.s.newer,
  }));
</script>

<div class="who">
  <Roster people={parts.people} agents={parts.agents} working={p.s.working} me={p.s.me?.public_id ?? null} max={p.s.narrow ? 1 : 5} />
  <WorkingSummary s={sum} />
</div>

<style>
  :global {
    .who { display: flex; align-items: center; gap: 10px; height: 40px; padding: 0 10px 0 6px; border: 1px solid var(--border-strong); background: var(--bg); flex: none; min-width: 0; }
    .who .sum { display: flex; align-items: center; gap: 8px; min-width: 0; }
    .who .sum b { display: block; font: 600 14px/1.1 var(--grot); white-space: nowrap; } .who .sum b.ag { color: var(--agent-ink); }
    .who .sum small { display: block; font: 400 11px/1.3 var(--mono); color: var(--muted); white-space: nowrap; }
    .thread-pin.onit { background: linear-gradient(90deg, var(--you) 50%, var(--agent) 50%); color: #fff; text-shadow: 0 0 2px #2f0b04; }
    .topbar.working::after { content: ""; position: absolute; left: 0; bottom: -1px; height: 2px; width: 20%; background: var(--agent); animation: sweep 2.4s cubic-bezier(.4,0,.2,1) infinite; }
    @keyframes sweep { from { transform: translateX(-100%); } to { transform: translateX(400%); } }
    @media (prefers-reduced-motion: reduce) { .topbar.working::after { animation: none; display: none; } }
    @media (max-width: 700px) { .who { height: 36px; padding: 0 6px 0 3px; gap: 0; } .who .sum { display: none; } }
  }
</style>
