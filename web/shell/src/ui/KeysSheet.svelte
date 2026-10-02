<svelte:options css="injected" />

<script lang="ts">
  // The `?` sheet (spec §8, "Keys"), loaded on first use. A modal dialog:
  // everything else in the document is inert while it is open, focus moves
  // to its Close button and Tab stays inside it; Escape, Close or a click
  // outside the panel closes it, and focus returns to where it was.
  import { KEY_ROWS } from "../view/keys";
  import { inertOutside, trapTab } from "./modal";

  let { onClose }: { onClose(): void } = $props();
  const back = document.activeElement as HTMLElement | null;
  let panel: HTMLElement | undefined = $state();
  const focus = (el: HTMLElement) => { el.focus(); };
  // Everything outside the sheet is inert while it is open; on close that is
  // undone, then focus goes back.
  const modal = (el: HTMLElement) => {
    const undo = inertOutside(el);
    return () => {
      undo();
      if (back?.isConnected) back.focus?.();
    };
  };

  function keydown(e: KeyboardEvent) {
    if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); onClose(); return; }
    if (panel) trapTab(e, panel);
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="keys-backdrop" {@attach modal} onclick={e => { if (e.target === e.currentTarget) onClose(); }}>
  <!-- Escape closes the dialog; Tab cycles inside it. -->
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div class="keys-panel" role="dialog" aria-modal="true" aria-labelledby="keys-title" tabindex="-1" bind:this={panel} onkeydown={keydown}>
    <h2 id="keys-title">Keyboard shortcuts</h2>
    <p class="sub">Keys work while the page does not have focus.</p>
    <dl>
      {#each KEY_ROWS as r (r.keys.join("+"))}
        <dt>{#each r.keys as key (key)}<kbd>{key}</kbd>{/each}</dt><dd>{r.what}</dd>
      {/each}
    </dl>
    <div class="foot"><button type="button" {@attach focus} onclick={onClose}>Close</button></div>
  </div>
</div>

<!-- The sheet's styles travel with its lazy chunk (injected on mount), so
     neither entry's eager CSS carries them. -->
<style>
  .keys-backdrop { position: fixed; inset: 0; z-index: 40; background: rgba(26,13,9,.55); display: grid; place-items: center; padding: var(--gutter); }
  .keys-panel { background: var(--raised); border: 1px solid var(--border-strong); box-shadow: 0 16px 40px var(--shadow); width: min(520px, 100%); max-height: calc(100dvh - 32px); overflow: auto; padding: 18px 20px 20px; }
  .keys-panel h2 { margin: 0 0 4px; font-size: 22px; }
  .keys-panel .sub { margin: 0 0 14px; color: var(--muted); font-size: 12px; }
  .keys-panel dl { display: grid; grid-template-columns: auto 1fr; gap: 8px 16px; margin: 0; font-size: 13px; align-items: baseline; }
  .keys-panel dt { display: flex; gap: 4px; justify-content: flex-end; }
  .keys-panel kbd { font: 600 12px/20px var(--mono); min-width: 24px; text-align: center; border: 1px solid var(--border-strong); border-bottom-width: 2px; padding: 0 6px; background: var(--card); }
  .keys-panel dd { margin: 0; }
  .keys-panel .foot { margin-top: 16px; display: flex; justify-content: flex-end; }
</style>
