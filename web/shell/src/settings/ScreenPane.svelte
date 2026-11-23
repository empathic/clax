<script lang="ts">
  // One Settings screen in Settings' body: its chunk, loaded when the pane
  // is made, and the screen mounted with a reveal of its own
  // (`first-data.ts`), so the pane says it is ready (`onready`) once the
  // screen shows its first data, or a line saying its chunk could not
  // load. Settings keeps the screen it shows until the next one's pane is
  // ready (`Settings.svelte`), so moving between screens shows no loading
  // line.
  import type { Component } from "svelte";
  import type { EventStream } from "../stream";
  import { Reveal, provideReveal } from "./first-data";

  type Body = Component<{ stream?: EventStream; part?: string; tab?: string }>;
  const p: { load: () => Promise<{ default: unknown }>; part?: string; tab?: string; stream?: EventStream; onready: () => void } = $props();
  let Shown = $state<Body | null>(null);
  let broken = $state(false);
  // svelte-ignore state_referenced_locally
  const reveal = new Reveal(p.onready);
  provideReveal(reveal);
  // svelte-ignore state_referenced_locally
  void p.load().then(m => { Shown = m.default as Body; }, () => { broken = true; });
  let armed = false;
  $effect(() => { if (!armed && (Shown || broken)) { armed = true; reveal.arm(); } });
</script>

{#if Shown}<Shown stream={p.stream} part={p.part} tab={p.tab} />{:else if broken}<p class="stg-note" role="alert">This screen could not load. <button type="button" class="ghost" onclick={() => location.reload()}>Reload to try again</button></p>{/if}
