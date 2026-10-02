<script lang="ts">
  // A gallery card's `seen vK`: the version this viewer last saw, if any.
  // Its style travels with the markers (Markers.svelte). Props are read off `p`.
  import type { AttentionSummary } from "../api";
  import { seenText } from "../view/attention-model";

  const p: { att?: AttentionSummary } = $props();
</script>

{#if seenText(p.att)}<span class="seen">{seenText(p.att)}</span>{/if}
