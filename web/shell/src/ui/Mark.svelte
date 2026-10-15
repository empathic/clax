<script lang="ts">
  // The Echo mark. `apart`: the empty gallery's mark. `size` and `apart` are
  // data attributes, and the symbol is written out rather than
  // `{@html MARK_SVG}`: the gallery's eager bundle then needs none of
  // Svelte's prop, class or HTML runtimes. A test holds the symbol to
  // MARK_SVG.
  const p: { size?: "bar" | "hero"; apart?: boolean } = $props();
</script>

<span class="mark" data-size={p.size ?? "bar"} data-apart={p.apart || undefined} role="img" aria-label="Clax"><svg class="mk" viewBox="0 0 40 24" aria-hidden="true" focusable="false"><path class="l" d="M2 2.5a9.5 9.5 0 0 1 0 19" fill="none" stroke-width="4.2"/><path class="r" d="M38 2.5a9.5 9.5 0 0 0 0 19" fill="none" stroke-width="4.2"/><circle cx="20" cy="12" r="2.6"/></svg></span>
