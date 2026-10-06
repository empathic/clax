<svelte:options css="injected" />

<script lang="ts">
  // The top bar's ⋯ menu: open raw and copy link, and Versions at phone width,
  // where the version button is hidden; Versions closes the menu and opens
  // the version menu (`onVersions`). On a live page (`pageHref`, the page's
  // URL) phone width also lists Open page, and Versions reads Snapshots. A menu button: opening
  // focuses the first item; Up, Down, Home and End move between the items;
  // Escape, Open raw and Copy link close it with focus back on the button; a
  // press outside, or Tab or focus leaving it, closes it. The items are out of
  // the Tab order (`tabindex="-1"`), and a disabled item stays focusable. While
  // it is open no key typed in it reaches the shell's keys: the keys it
  // handles, and every printable key (so C and ? do nothing there).
  import { tick } from "svelte";

  let { rawHref, canCopy, onCopy, onVersions, pageHref = null }: { rawHref: string | null; canCopy: boolean; onCopy(): void; onVersions?(): void; pageHref?: string | null } = $props();
  let open = $state(false);
  // Phone width, as the bar's `hide-sm` rule draws it; read on each open.
  let phone = $state(false);
  let button: HTMLButtonElement | undefined = $state();
  let menu: HTMLDivElement | undefined = $state();
  async function toggle() {
    open = !open;
    phone = !!onVersions && typeof matchMedia === "function" && matchMedia("(max-width: 700px)").matches;
    if (open) { await tick(); menu?.querySelector<HTMLElement>("a, button")?.focus(); }
  }
  function close() { open = false; button?.focus(); }
  function outside(e: PointerEvent) {
    if (open && !menu?.contains(e.target as Node) && !button?.contains(e.target as Node)) open = false;
  }
  function key(e: KeyboardEvent) {
    if (!open) return;
    // Tab leaves for the next control, and focusout closes the menu; Shift+Tab
    // reaches the button, so it closes the menu there.
    if (e.key === "Tab") { if (e.shiftKey && menu?.contains(document.activeElement)) { e.preventDefault(); close(); } return; }
    const items = Array.from(menu?.querySelectorAll<HTMLElement>("a, button") ?? []);
    const i = items.indexOf(document.activeElement as HTMLElement);
    const n = items.length;
    const to = { ArrowDown: (i + 1) % n, ArrowUp: (i - 1 + n) % n, Home: 0, End: n - 1 }[e.key];
    if (e.key !== "Escape" && (to === undefined || !n)) {
      // A printable key (C, ?) is the menu's, not the shell's.
      if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey) e.stopPropagation();
      return;
    }
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
<div class="more" onkeydown={key} onfocusout={leave}>
  <button type="button" class="icon" bind:this={button} aria-label="More" aria-haspopup="menu" aria-expanded={open} onclick={toggle}>
    <svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="3" cy="8" r="1.4" fill="currentColor"/><circle cx="8" cy="8" r="1.4" fill="currentColor"/><circle cx="13" cy="8" r="1.4" fill="currentColor"/></svg>
  </button>
  {#if open}
    <div class="more-menu" role="menu" bind:this={menu}>
      {#if phone && onVersions}<button type="button" role="menuitem" tabindex="-1" class="ghost" onclick={() => { open = false; onVersions(); }}>{pageHref ? "Snapshots" : "Versions"}</button>{/if}
      {#if phone && pageHref}<a role="menuitem" tabindex="-1" href={pageHref} target="_blank" rel="noopener noreferrer" onclick={close}>Open page</a>{/if}
      {#if rawHref}
        <a role="menuitem" tabindex="-1" href={rawHref} target="_blank" rel="noopener" onclick={close}>Open raw</a>
      {:else}
        <button type="button" role="menuitem" tabindex="-1" class="ghost muted" aria-disabled="true">Open raw</button>
      {/if}
      {#if canCopy}<button type="button" role="menuitem" tabindex="-1" class="ghost" onclick={() => { onCopy(); close(); }}>Copy link</button>{/if}
    </div>
  {/if}
</div>

<!-- The menu's styles travel with its lazy chunk (injected on mount), so the
     artifact entry's eager CSS does not carry them. -->
<style>
  .more { position: relative; }
  .more-menu { position: absolute; right: 0; top: calc(100% + 8px); z-index: 20; min-width: 180px; background: var(--raised); border: 1px solid var(--border-hover); border-radius: var(--radius); box-shadow: var(--elev-lg); padding: 6px 0; display: flex; flex-direction: column; }
  .more-menu > a, .more-menu > button { display: block; width: 100%; padding: 8px 14px; text-align: left; font: 500 14px/1.2 var(--font); justify-content: flex-start; min-height: 0; border: 0; border-radius: 0; }
  .more-menu > a:hover, .more-menu > button:not([aria-disabled]):hover { background: var(--hover); }
  .more-menu > button[aria-disabled]:hover { background: none; }
  .more-menu > button[aria-disabled] { cursor: default; }
</style>
