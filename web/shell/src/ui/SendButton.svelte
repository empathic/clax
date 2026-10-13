<svelte:options css="injected" />

<script lang="ts">
  // A Send that names its agent (spec §10). The caret, shown when more than
  // one agent is live or the one live agent is another person's, opens the
  // menu (`SendMenu`, loaded on first open) that picks another; the choice
  // is remembered. `me` is this viewer's public ID (`undefined`: unknown,
  // the owner's), `firsts` the thread's `approves_first`. `onSend` hears
  // the activation, so the caller can guard it (`guardedAction`).
  import type { AgentView } from "../view/working-model";

  let { label, agents, names, target, me, firsts = [], disabled = false, onSend, onChoose }: {
    label: string; agents: AgentView[]; names?: Map<string, string>; target: string | null; me?: string | null; firsts?: (string | null)[]; disabled?: boolean; onSend(e: Event): void; onChoose(h: string): void;
  } = $props();
  let open = $state(false);
  let root: HTMLElement | undefined = $state();
  const live = $derived(agents.filter(a => a.live));
  const others = $derived(live.some(a => a.person && (me === undefined ? !a.person.owner : a.person.public_id !== me)));
  // A press outside the menu closes it.
  const outside = (e: PointerEvent) => { if (open && root && !root.contains(e.target as Node)) open = false; };
</script>

<svelte:window onpointerdown={outside} />

<span class="send" bind:this={root}>
  <button type="button" class="primary" {disabled} onclick={onSend}>{label}</button>
  {#if live.length > 1 || others}
    <button type="button" class="primary caret" aria-label="Choose the agent" aria-haspopup="menu" aria-expanded={open} onclick={() => { open = !open; }}>▾</button>
    {#if open}
      {#await import("./SendMenu.svelte") then { default: SendMenu }}
        <SendMenu {agents} {names} {me} {firsts} {target} {onChoose} onClose={() => { open = false; }} />
      {/await}
    {/if}
  {/if}
</span>

<!-- The Send's styles travel with it (injected on mount): the shell's sidebar and the side panel's lists. -->
<style>
  :global {
    .send button.primary { background: var(--accent); color: var(--on-accent); border-color: var(--accent); }
    .send button.primary:not(:disabled):hover { background: var(--accent-hover); border-color: var(--accent-hover); }
    .send { display: inline-flex; position: relative; }
    .send .caret { min-width: 28px; padding: 0 6px; border-left: 1px solid color-mix(in srgb, var(--on-accent) 35%, transparent); font-family: var(--mono); }
    .send > button.primary:not(:last-child) { border-top-right-radius: 0; border-bottom-right-radius: 0; }
    .send .caret { border-top-left-radius: 0; border-bottom-left-radius: 0; }
  }
</style>
