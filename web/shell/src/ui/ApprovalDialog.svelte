<svelte:options css="injected" />
<script lang="ts">
  // The approval dialog: what each item the version asks for lets the page
  // do, with Approve and Cancel. A modal like the keys sheet: the rest of the
  // document is inert while it is open, Tab stays inside it, Escape or a
  // click outside the panel closes it, and focus goes back to where it was.
  // It sits at the stage's root, not in the stage's foot, whose pointer
  // events are off.
  import type { PendingCapabilities } from "../api";
  import { pendingItems } from "../caps/approval";
  import { inertOutside, trapTab } from "./modal";

  let { pending, title, busy, onApprove, onClose }: {
    pending: PendingCapabilities; title: string; busy: boolean; onApprove(): void; onClose(): void;
  } = $props();
  const back = document.activeElement as HTMLElement | null;
  let panel: HTMLElement | undefined = $state();
  const focus = (el: HTMLElement) => { el.focus(); };
  const modal = (el: HTMLElement) => {
    const undo = inertOutside(el);
    return () => { undo(); if (back?.isConnected) back.focus?.(); };
  };
  function keydown(e: KeyboardEvent) {
    if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); onClose(); return; }
    if (panel) trapTab(e, panel);
  }
  const items = $derived(pendingItems(pending));
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="approval-backdrop" {@attach modal} onclick={e => { if (e.target === e.currentTarget) onClose(); }}>
  <!-- Escape closes the dialog; Tab cycles inside it. -->
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div class="approval-panel" role="dialog" aria-modal="true" aria-labelledby="approval-title" tabindex="-1" bind:this={panel} onkeydown={keydown}>
    <h2 id="approval-title">Let “{title}” use these?</h2>
    <p class="sub">Version {pending.version} asks for:</p>
    <ul>
      {#each items as i (i.name)}
        <li><strong>{i.label}</strong><span>{i.detail}</span></li>
      {/each}
    </ul>
    <div class="foot">
      <button type="button" onclick={onClose}>Cancel</button>
      <button type="button" class="primary" disabled={busy} {@attach focus} onclick={() => { onClose(); onApprove(); }}>Approve</button>
    </div>
  </div>
</div>

<style>
  .approval-backdrop { position: fixed; inset: 0; z-index: 40; background: var(--scrim); display: grid; place-items: center; padding: var(--gutter); }
  .approval-panel { background: var(--raised); border: 1px solid var(--border-hover); border-radius: var(--radius); box-shadow: var(--elev-lg); width: min(520px, 100%); max-height: calc(100dvh - 32px); overflow: auto; padding: 18px 20px 20px; }
  .approval-panel h2 { margin: 0 0 4px; font-size: 17px; overflow-wrap: anywhere; }
  .approval-panel .sub { margin: 0 0 12px; color: var(--muted); font-size: 12px; }
  .approval-panel ul { list-style: none; margin: 0; padding: 0; display: grid; gap: 10px; font-size: 13px; }
  .approval-panel li { display: grid; gap: 2px; }
  .approval-panel li span { color: var(--muted); }
  .approval-panel .foot { margin-top: 16px; display: flex; gap: 8px; justify-content: flex-end; }
</style>
