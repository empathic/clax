<svelte:options css="injected" />

<script lang="ts">
  // The header's feedback menu (spec 2026-10-29-feedback-loop §10.3), for a
  // person who may report problems with Clax, in the gallery's header and
  // the artifact view's top bar: **Report a problem**, with its shortcut
  // (F, as the shell's C and ?), starts the overlay's report;
  // **My reports** opens the person's reports, with the count of those
  // waiting on them (the overlay's badge), which the menu's button shows too.
  // A menu button: opening focuses the first item; Up, Down, Home and End
  // move between the items; Escape closes it with focus back on the button;
  // a press outside, or focus leaving it, closes it. While it is open no
  // printable key typed in it reaches the shell's keys.
  import { tick } from "svelte";
  import { badge, feedback } from "./state.svelte";

  const fb = $derived(feedback.current);
  const n = $derived(badge.current);
  let open = $state(false);
  let button: HTMLButtonElement | undefined = $state();
  let menu: HTMLDivElement | undefined = $state();
  async function toggle() {
    open = !open;
    if (open) { await tick(); menu?.querySelector<HTMLElement>("button")?.focus(); }
  }
  function close() { open = false; button?.focus(); }
  function act(f: () => void) { open = false; f(); }
  function outside(e: PointerEvent) {
    if (open && !menu?.contains(e.target as Node) && !button?.contains(e.target as Node)) open = false;
  }
  function key(e: KeyboardEvent) {
    if (!open) return;
    if (e.key === "Tab") return;
    const items = Array.from(menu?.querySelectorAll<HTMLElement>("button") ?? []);
    const i = items.indexOf(document.activeElement as HTMLElement);
    const to = { ArrowDown: (i + 1) % items.length, ArrowUp: (i - 1 + items.length) % items.length, Home: 0, End: items.length - 1 }[e.key];
    if (e.key !== "Escape" && to === undefined) {
      if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey) e.stopPropagation();
      return;
    }
    e.preventDefault();
    e.stopPropagation();
    if (e.key === "Escape") close(); else items[to!]?.focus();
  }
  function leave(e: FocusEvent) {
    if (open && !(e.currentTarget as HTMLElement).contains(e.relatedTarget as Node | null)) open = false;
  }
  const waiting = (k: number) => (k === 1 ? "1 report waits on you" : `${k} reports wait on you`);
</script>

<svelte:window onpointerdown={outside} />

{#if fb}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="fb" onkeydown={key} onfocusout={leave}>
    <button type="button" class="fb-btn" bind:this={button} aria-haspopup="menu" aria-expanded={open} aria-label={n ? `Feedback: ${waiting(n)}` : "Feedback"} title="Report a problem with Clax" onclick={toggle}>
      <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M2.5 3.5h11v7h-6l-3 2.5v-2.5h-2z" /><path d="M8 5.5v2.2M8 9.3v.01" /></svg>
      {#if n}<span class="fb-count" aria-hidden="true">{n}</span>{/if}
    </button>
    {#if open}
      <div class="fb-menu" role="menu" aria-label="Feedback" bind:this={menu}>
        <button type="button" role="menuitem" tabindex="-1" class="ghost" aria-label="Report a problem" aria-keyshortcuts={fb.shortcut.aria} onclick={() => act(() => fb.report())}>
          <span>Report a problem</span><kbd>{fb.shortcut.label}</kbd>
        </button>
        <button type="button" role="menuitem" tabindex="-1" class="ghost" aria-label={n ? `My reports: ${waiting(n)}` : "My reports"} onclick={() => act(() => fb.showReports())}>
          <span>My reports</span>{#if n}<span class="fb-count" aria-hidden="true">{n}</span>{/if}
        </button>
      </div>
    {/if}
  </div>
{/if}

<style>
  .fb { position: relative; flex: none; }
  .fb-btn { position: relative; display: inline-flex; align-items: center; gap: 4px; min-height: 32px; padding: 0 8px; border: 0; background: none; border-radius: var(--radius-sm); color: var(--fg); }
  .fb-btn:hover, .fb-btn[aria-expanded="true"] { background: var(--hover); }
  .fb-btn svg { width: 16px; height: 16px; fill: none; stroke: currentColor; stroke-width: 1.4; stroke-linecap: round; stroke-linejoin: round; color: var(--muted); }
  .fb-count { min-width: 20px; padding: 0 6px; border-radius: 999px; background: var(--you); color: var(--on-you); font: 600 11.5px/20px var(--font); text-align: center; font-variant-numeric: tabular-nums; }
  .fb-menu { position: absolute; right: 0; top: calc(100% + 8px); z-index: 20; min-width: 220px; background: var(--raised); border: 1px solid var(--border-hover); border-radius: var(--radius); box-shadow: var(--elev-lg); padding: 6px 0; display: flex; flex-direction: column; }
  .fb-menu > button { display: flex; align-items: center; justify-content: space-between; gap: 16px; width: 100%; padding: 8px 14px; text-align: left; font: 500 14px/1.2 var(--font); min-height: 0; border: 0; border-radius: 0; }
  .fb-menu > button:hover { background: var(--hover); }
  .fb-menu kbd { font: 500 12px/18px var(--mono); color: var(--muted); }
  @media (pointer: coarse) { .fb-btn { min-height: 44px; } .fb-menu > button { min-height: 44px; } }
</style>
