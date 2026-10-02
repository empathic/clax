<script lang="ts">
  // The Echo mark. `playful`: a click makes its halves meet, another parts
  // them (an easter egg; no other effect). `apart`: the empty gallery's mark.
  // `size` and `apart` are data attributes, and the symbol is written out
  // in each branch rather than `{@html MARK_SVG}`: the gallery's eager bundle
  // then needs none of Svelte's prop, class or HTML runtimes. A test holds the
  // symbol to MARK_SVG.
  const p: { size?: "bar" | "hero"; apart?: boolean; playful?: boolean } = $props();
  let meet = $state(false);
</script>

{#if p.playful}
  <button type="button" class="mark" data-size={p.size ?? "bar"} data-apart={p.apart || undefined} aria-label="Clax mark" aria-pressed={meet} onclick={() => { meet = !meet; }}><svg class="mk" viewBox="0 0 30 24" aria-hidden="true" focusable="false"><path class="l" d="M2 2.5a9.5 9.5 0 0 1 0 19" fill="none" stroke-width="4.2"/><path class="r" d="M28 2.5a9.5 9.5 0 0 0 0 19" fill="none" stroke-width="4.2"/><circle cx="15" cy="12" r="2.6"/></svg></button>
{:else}
  <span class="mark" data-size={p.size ?? "bar"} data-apart={p.apart || undefined} role="img" aria-label="Clax"><svg class="mk" viewBox="0 0 30 24" aria-hidden="true" focusable="false"><path class="l" d="M2 2.5a9.5 9.5 0 0 1 0 19" fill="none" stroke-width="4.2"/><path class="r" d="M28 2.5a9.5 9.5 0 0 0 0 19" fill="none" stroke-width="4.2"/><circle cx="15" cy="12" r="2.6"/></svg></span>
{/if}
