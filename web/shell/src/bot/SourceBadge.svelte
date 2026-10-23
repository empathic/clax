<svelte:options css="injected" />

<script lang="ts">
  // Where a value comes from (spec 2026-10-21-claxbot §8): its default, set
  // here, `config.toml`, or an environment variable; a value from the file
  // or the environment is read-only on the admin screens, and says so.
  import type { Source } from "./api";
  import { readOnlySource, sourceLabel } from "./model";

  let { source, ro: given }: { source: Source | undefined; ro?: boolean } = $props();
  const ro = $derived(given ?? readOnlySource(source));
</script>

{#if source}<span class="bsrc" class:ro title={ro ? "Set outside this screen: read-only here" : undefined}>{sourceLabel(source)}{#if ro}<span class="bot-sr">{" (read-only)"}</span>{/if}</span>{/if}

<style>
  :global {
    .bsrc { display: inline-block; font: 500 11.5px/1.5 var(--mono); color: var(--muted); border: 1px solid var(--border-hover); border-radius: var(--radius-xs); padding: 0 6px; white-space: nowrap; max-width: 100%; overflow: hidden; text-overflow: ellipsis; vertical-align: middle; }
    .bsrc.ro { color: var(--fg); border-color: var(--border-strong); }
    .bot-sr { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); white-space: nowrap; }
  }
</style>
