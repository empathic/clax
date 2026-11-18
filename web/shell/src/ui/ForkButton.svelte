<svelte:options css="injected" />

<script lang="ts">
  // The top bar's Fork button, an icon named Fork whose hover text says
  // which version it forks (`shown`). Loaded after the first paint
  // (`more-menu.svelte.ts`); the bar shows it where the viewer may fork.
  let { shown, disabled, onFork }: { shown: number; disabled: boolean; onFork(): void } = $props();
</script>

<button type="button" class="icon fork hide-sm" data-shell-control="fork" {disabled} aria-label="Fork" title={`Fork v${shown} into an artifact of your own`} onclick={onFork}>
  <svg viewBox="0 0 16 16" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"><circle cx="4" cy="3.5" r="1.5"/><circle cx="12" cy="3.5" r="1.5"/><circle cx="8" cy="12.5" r="1.5"/><path d="M4 5v1.5a2 2 0 0 0 2 2h4a2 2 0 0 0 2-2V5M8 8.5V11"/></svg>
</button>
