<svelte:options css="injected" />

<script lang="ts">
  // One working record at the top of the sidebar: who works on which threads,
  // or the agent's message, as text; then a haiku that holds still while the
  // record lives (never in comment mode, never animated).
  import type { Working } from "../view/working-model";
  import HaikuLine from "./HaikuLine.svelte";
  let { w, text, commenting }: { w: Working; text: string; commenting: boolean } = $props();
</script>

<div class="strip ag">
  <span class="tok a work" aria-hidden="true">{w.harness.slice(0, 2)}</span><b>{text}</b>
  {#if !commenting}<HaikuLine seed={w.key} />{/if}
</div>

<style>
  :global {
    .strip { margin: 0 0 4px; padding: 10px 12px; background: var(--card); border: 1px solid var(--border); display: grid; grid-template-columns: auto 1fr; gap: 3px 10px; align-items: center; }
    .strip.ag { box-shadow: inset 3px 0 0 var(--agent); } .strip b { font: 600 15px/1.2 var(--grot); color: var(--agent-ink); overflow-wrap: anywhere; } .strip .hk { grid-column: 2; }
  }
</style>
