<script lang="ts">
  // Draws a snippet of the overlay somewhere else: the composer in its own
  // window (the "window" presentation). The snippet keeps the overlay's
  // state, so the anchor and the screenshot never leave the page's memory.
  import type { Snippet } from "svelte";

  let { content }: { content: Snippet } = $props();
</script>

{@render content()}
