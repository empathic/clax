<svelte:options css="injected" />

<script lang="ts">
  // A gallery card's markers (spec §8), in the order `markers` gives them:
  // addressed, new version, new replies, working, open. It carries the styles
  // of the markers and of the version last seen, which load with the gallery's lazy module rather than its first
  // paint. Props are read off `p`, and every class is static.
  import type { Marker } from "../view/attention-model";

  const p: { list: Marker[] } = $props();
</script>

<!-- Static classes only, so this lazy module shares no class runtime with the artifact entry. -->
{#each p.list as m, i (i)}{#if m.kind === "you"}<span class="chip you">{m.text}</span>{:else if m.kind === "new"}<span class="chip new">{m.text}</span>{:else if m.kind === "rep"}<span class="chip rep">{m.text}</span>{:else if m.kind === "ag"}<span class="chip ag">{m.text}</span>{:else}<span class="chip oth">{m.text}</span>{/if}{/each}

<style>
  :global {
    .chip.you { background: var(--you); color: var(--on-you); }
    .chip.new { box-shadow: inset 0 0 0 1.5px var(--agent); color: var(--agent-ink); }
    .chip.new::before { content: ""; width: 6px; height: 6px; border-radius: 50%; background: var(--agent); }
    .chip.rep { box-shadow: inset 0 0 0 1.5px var(--border-strong); }
    .chip.oth { box-shadow: inset 0 0 0 1.5px var(--you); }
    .card .ft .seen { font-size: 11px; color: var(--muted); margin-left: auto; }
  }
</style>
