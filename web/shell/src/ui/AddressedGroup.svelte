<svelte:options css="injected" />

<script lang="ts">
  // The sidebar's "Addressed in vN" group (spec §8): the threads the latest
  // version addressed that this viewer is in and has not looked at, as this
  // load decided them. Loaded only when the group has threads.
  import type { Snippet } from "svelte";
  let { n, agent, count, children }: { n: number; agent: string; count: number; children: Snippet } = $props();
</script>

<section class="section-addressed" aria-label={`Addressed in v${n}`}>
  <h2 class="gh ag"><span class="sw" aria-hidden="true"></span><span class="t">Addressed in v{n}</span> <span class="c">{count}</span></h2>
  <p class="gsub">{agent} addressed these. Have a look, then resolve each one or reply.</p>
  {@render children()}
</section>

<style>
  :global {
    .gsub { margin: -4px 2px 10px 17px; font-size: 12px; color: var(--muted); line-height: 1.5; }
    .section-addressed .thread-card { border-left: 3px solid var(--agent); background: var(--comment-hl); }
    .section-addressed .thread-num { background: var(--card); color: var(--fg); box-shadow: inset 0 0 0 1.5px var(--agent); }
  }
</style>
