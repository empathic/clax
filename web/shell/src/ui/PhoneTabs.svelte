<script lang="ts">
  // The phone's Page | Threads switch at the foot of the view: two toggle
  // buttons, rendered after the sidebar so the Tab order follows the screen.
  let { panel, open, onPage, onThreads }: { panel: boolean; open: number; onPage(): void; onThreads(): void } = $props();
</script>

<div class="phone-tabs" role="group" aria-label="Page or threads">
  <button type="button" aria-pressed={!panel} onclick={onPage}>Page</button>
  <button type="button" aria-pressed={panel} onclick={onThreads}>Threads <span class="cnt">{open}</span></button>
</div>
