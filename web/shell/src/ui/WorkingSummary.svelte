<script lang="ts">
  import type { Summary } from "../view/working-model";
  let { s }: { s: Summary } = $props();
</script>

<span class="sum">
  <span role="status" aria-live="polite" aria-atomic="true"><b class={["l1", s.agent && "ag"]}>{s.line1}</b><small class="l2">{s.line2}</small></span>{#if s.elapsed}<small class="el" aria-hidden="true">{s.elapsed}</small>{/if}
</span>
