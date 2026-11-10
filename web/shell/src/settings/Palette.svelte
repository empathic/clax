<svelte:options css="injected" />

<script lang="ts">
  // Settings' search as a dialog over any page of the gallery shell
  // (`palette.ts` opens it: the Settings key, or Search in Settings'
  // navigation). It lists every screen until something is typed, so it
  // also takes a person anywhere in Settings by name. A modal dialog:
  // everything else is inert while it is open, the field has the focus and
  // Tab stays inside; Escape or a click outside closes it, and focus
  // returns to where it was.
  import { inertOutside, trapTab } from "../ui/modal";
  import type { Ctx } from "./screens";
  import Search from "./Search.svelte";

  let { ctx, onGo, onClose }: { ctx: Ctx; onGo(href: string): void; onClose(): void } = $props();
  const back = document.activeElement as HTMLElement | null;
  let panel: HTMLElement | undefined = $state();
  const modal = (el: HTMLElement) => {
    const undo = inertOutside(el);
    return () => { undo(); if (back?.isConnected) back.focus?.(); };
  };
  function keydown(e: KeyboardEvent) {
    if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); onClose(); return; }
    if (panel) trapTab(e, panel);
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="spal-backdrop" {@attach modal} onclick={e => { if (e.target === e.currentTarget) onClose(); }}>
  <!-- Escape closes the dialog; Tab cycles inside it. -->
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div class="spal" role="dialog" aria-modal="true" aria-label="Search settings" tabindex="-1" bind:this={panel} onkeydown={keydown}>
    <Search {ctx} all focus onGo={href => { onClose(); onGo(href); }} />
    <p class="spal-keys"><kbd>↑</kbd><kbd>↓</kbd> to move, <kbd>Enter</kbd> to open, <kbd>Esc</kbd> to close</p>
  </div>
</div>

<style>
  .spal-backdrop { position: fixed; inset: 0; z-index: 40; background: var(--scrim); display: grid; justify-items: center; align-items: start; padding: min(12vh, 96px) var(--gutter) var(--gutter); }
  .spal { background: var(--raised); border: 1px solid var(--border-hover); border-radius: var(--radius); box-shadow: var(--elev-lg); width: min(560px, 100%); max-height: min(560px, calc(100dvh - 32px - min(12vh, 96px))); display: flex; flex-direction: column; gap: 10px; padding: 12px; }
  .spal:focus { outline: none; }
  .spal-keys { margin: 0; color: var(--muted); font-size: 12px; }
  .spal-keys kbd { font: 500 11px/18px var(--mono); border: 1px solid var(--border-hover); border-radius: var(--radius-xs); padding: 0 5px; margin-right: 3px; background: var(--card); }
  @media (pointer: coarse) { .spal-keys { display: none; } }
</style>
