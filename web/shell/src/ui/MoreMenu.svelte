<svelte:options css="injected" />

<script lang="ts">
  // The top bar's ⋯ menu: open raw and copy link. A menu button: opening
  // focuses the first item; Up, Down, Home and End move between the items;
  // Escape closes it with focus back on the button; a press outside or focus
  // leaving it closes it. Keys it handles do not reach the shell's keys.
  import { tick } from "svelte";

  let { rawHref, canCopy, onCopy }: { rawHref: string | null; canCopy: boolean; onCopy(): void } = $props();
  let open = $state(false);
  let button: HTMLButtonElement | undefined = $state();
  let menu: HTMLDivElement | undefined = $state();
  async function toggle() {
    open = !open;
    if (open) { await tick(); menu?.querySelector<HTMLElement>("a, button")?.focus(); }
  }
  function close() { open = false; button?.focus(); }
  function outside(e: PointerEvent) {
    if (open && !menu?.contains(e.target as Node) && !button?.contains(e.target as Node)) open = false;
  }
  function key(e: KeyboardEvent) {
    if (!open) return;
    const items = Array.from(menu?.querySelectorAll<HTMLElement>("a, button") ?? []);
    const i = items.indexOf(document.activeElement as HTMLElement);
    const n = items.length;
    const to = { ArrowDown: (i + 1) % n, ArrowUp: (i - 1 + n) % n, Home: 0, End: n - 1 }[e.key];
    if (e.key !== "Escape" && (to === undefined || !n)) return;
    e.preventDefault();
    e.stopPropagation();
    if (e.key === "Escape") close(); else items[to!].focus();
  }
  function leave(e: FocusEvent) {
    if (open && !(e.currentTarget as HTMLElement).contains(e.relatedTarget as Node | null)) open = false;
  }
</script>

<svelte:window onpointerdown={outside} />

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="more hide-sm" onkeydown={key} onfocusout={leave}>
  <button type="button" class="icon" bind:this={button} aria-label="Open raw or copy link" aria-haspopup="menu" aria-expanded={open} onclick={toggle}>
    <svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="3" cy="8" r="1.4" fill="currentColor"/><circle cx="8" cy="8" r="1.4" fill="currentColor"/><circle cx="13" cy="8" r="1.4" fill="currentColor"/></svg>
  </button>
  {#if open}
    <div class="more-menu" role="menu" bind:this={menu}>
      {#if rawHref}
        <a role="menuitem" href={rawHref} target="_blank" rel="noopener" onclick={() => { open = false; }}>Open raw</a>
      {:else}
        <span class="muted" role="menuitem" aria-disabled="true">Open raw</span>
      {/if}
      {#if canCopy}<button type="button" role="menuitem" class="ghost" onclick={() => { onCopy(); close(); }}>Copy link</button>{/if}
    </div>
  {/if}
</div>

<!-- The menu's styles travel with its lazy chunk (injected on mount), so the
     artifact entry's eager CSS does not carry them. -->
<style>
  .more { position: relative; }
  .more-menu { position: absolute; right: 0; top: calc(100% + 8px); z-index: 20; min-width: 180px; background: var(--raised); border: 1px solid var(--border-strong); box-shadow: 0 14px 40px var(--shadow); padding: 6px 0; display: flex; flex-direction: column; }
  .more-menu > a, .more-menu > button, .more-menu > span { display: block; width: 100%; padding: 8px 14px; text-align: left; font: 600 14px/1.2 var(--grot); justify-content: flex-start; min-height: 0; border: 0; }
  .more-menu > a:hover, .more-menu > button:hover { background: var(--bg); }
</style>
