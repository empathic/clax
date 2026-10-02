<script lang="ts">
  // A Send that names its agent (spec §10). The caret, shown only when more
  // than one agent is live, picks another; the choice is remembered. `onSend`
  // hears the activation, so the caller can guard it (`guardedAction`).
  import type { AgentView } from "../view/working-model";

  let { label, agents, names, target, disabled = false, onSend, onChoose }: {
    label: string; agents: AgentView[]; names: Map<string, string>; target: string | null; disabled?: boolean; onSend(e: Event): void; onChoose(h: string): void;
  } = $props();
  let open = $state(false);
  let root: HTMLElement | undefined = $state();
  const live = $derived(agents.filter(a => a.live));
  // A press outside the menu closes it.
  const outside = (e: PointerEvent) => { if (open && root && !root.contains(e.target as Node)) open = false; };
</script>

<svelte:window onpointerdown={outside} />

<span class="send" bind:this={root}>
  <button type="button" class="primary" {disabled} onclick={onSend}>{label}</button>
  {#if live.length > 1}
    <button type="button" class="primary caret" aria-label="Choose the agent" aria-haspopup="menu" aria-expanded={open} onclick={() => { open = !open; }}>▾</button>
    {#if open}
      <!-- Escape closes the menu. -->
      <!-- svelte-ignore a11y_interactive_supports_focus -->
      <div class="send-menu" role="menu" tabindex="-1" onkeydown={e => { if (e.key === "Escape") { e.stopPropagation(); open = false; } }}>
        {#each live as a (a.handle)}
          <button type="button" role="menuitemradio" aria-checked={a.handle === target} class="ghost" onclick={() => { onChoose(a.handle); open = false; }}>{names.get(a.handle) ?? a.harness}</button>
        {/each}
      </div>
    {/if}
  {/if}
</span>
