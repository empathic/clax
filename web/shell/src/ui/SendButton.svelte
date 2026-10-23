<svelte:options css="injected" />

<script lang="ts">
  // A Send that names its agent (spec §10). The caret, shown when more than
  // one agent is live or the one live agent is another person's, opens the
  // menu (`SendMenu`, loaded on first open) that picks another; the choice
  // is remembered. With no target while agents are live (only other
  // people's: spec 2026-10-08 §10), the button opens the menu too: there is
  // no one-press send to another person's agent. `me` is this viewer's
  // public ID (`undefined`: unknown, the owner's); `own`, whose agents a
  // Send takes as this viewer's (`sendsFor`: the owner's for a LAN
  // viewer), `me` unless given; the menu asks who
  // approves first for `threads` of `aid`. ClaxBot (`builtin`) listed brings the caret too, so it can be
  // chosen or its reason read. `onSend` hears the activation,
  // so the caller can guard it (`guardedAction`).
  import type { AgentView } from "../view/working-model";

  let { label, agents, names, target, me, own = me, aid, threads, disabled = false, onSend, onChoose }: {
    label: string; agents: AgentView[]; names?: Map<string, string>; target: string | null; me?: string | null; own?: string | null; aid?: string; threads?: string[]; disabled?: boolean; onSend(e: Event): void; onChoose(h: string): void;
  } = $props();
  let open = $state(false);
  let root: HTMLElement | undefined = $state();
  const live = $derived(agents.filter(a => a.live));
  const pick = $derived(target === null && live.length > 0);
  const others = $derived(live.some(a => a.person && (own === undefined ? !a.person.owner : a.person.public_id !== own)));
  // A press outside the menu closes it.
  const outside = (e: PointerEvent) => { if (open && root && !root.contains(e.target as Node)) open = false; };
</script>

<svelte:window onpointerdown={outside} />

<span class="send" bind:this={root}>
  <button type="button" class="primary" {disabled} aria-haspopup={pick ? "menu" : undefined} onclick={e => { if (pick) open = !open; else onSend(e); }}>{label}</button>
  {#if live.length > 1 || others || agents.some(a => a.builtin)}
    <button type="button" class="primary caret" aria-label="Choose the agent" aria-haspopup="menu" aria-expanded={open} onclick={() => { open = !open; }}>▾</button>
    {#if open}
      {#await import("./SendMenu.svelte") then { default: SendMenu }}
        <SendMenu {agents} {names} {me} {own} {aid} {threads} {target} {onChoose} onClose={() => { open = false; }} />
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
