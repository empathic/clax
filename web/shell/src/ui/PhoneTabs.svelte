<script lang="ts">
  let { panel, open, onPage, onThreads }: { panel: boolean; open: number; onPage(): void; onThreads(): void } = $props();
</script>

<nav class="phone-tabs" aria-label="Page or threads">
  <button type="button" aria-pressed={!panel} onclick={onPage}>Page</button>
  <button type="button" aria-pressed={panel} onclick={onThreads}>Threads <span class="cnt">{open}</span></button>
</nav>
