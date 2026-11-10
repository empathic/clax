<svelte:options css="injected" />

<script lang="ts">
  // The gallery header's account menu on a hosted Clax, for whoever the
  // browser is signed in as (the owner, a person or a guest): its button is
  // "seen as <name>" (the name alone at phone width), and its items are
  // **Your account**, People's part for this browser, its devices and agent
  // tokens (`/people`, not for a guest), and **Sign out**, which forgets the
  // person's reporter (`signout.ts`), ends the session and lands on the
  // sign-in page; a refusal shows in the menu. A local daemon has no
  // sign-in, so its header carries no such menu. The menu button behaves
  // as the feedback menu's (`ui/menu-keys.ts`): opening focuses the first
  // item; Up, Down, Home and End move between the items; Escape closes it
  // with focus back on the button; a press outside, or focus leaving it,
  // closes it. The name is the person's text, rendered as text.
  import { tick } from "svelte";
  import { firstItem, menuKey } from "../ui/menu-keys";
  import { refusal } from "../people/refusal";
  import * as api from "./api";
  import { forgetThenSignOut } from "./signout";

  let { name, guest = false, signOut = () => forgetThenSignOut(api.signOut), go = (url: string) => location.assign(url) }: {
    name: string; guest?: boolean; signOut?: () => Promise<unknown>; go?: (url: string) => void;
  } = $props();
  let open = $state(false);
  let busy = $state(false);
  let err = $state<string | null>(null);
  let button: HTMLButtonElement | undefined = $state();
  let menu: HTMLDivElement | undefined = $state();
  async function toggle() {
    open = !open;
    err = null;
    if (open) { await tick(); firstItem(menu)?.focus(); }
  }
  function close() { open = false; button?.focus(); }
  function outside(e: PointerEvent) {
    if (open && !menu?.contains(e.target as Node) && !button?.contains(e.target as Node)) open = false;
  }
  function key(e: KeyboardEvent) { if (open) menuKey(e, menu, close); }
  function leave(e: FocusEvent) {
    if (open && !busy && !(e.currentTarget as HTMLElement).contains(e.relatedTarget as Node | null)) open = false;
  }
  async function out() {
    busy = true; err = null;
    try { await signOut(); go("/signin"); } catch (x) { err = refusal(x); } finally { busy = false; }
  }
</script>

<svelte:window onpointerdown={outside} />

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="acct" onkeydown={key} onfocusout={leave}>
  <button type="button" class="acct-btn" bind:this={button} aria-haspopup="menu" aria-expanded={open} title="Your account" onclick={toggle}>
    <span class="hide-sm">{"seen as "}</span><span class="acct-name">{name}</span>
    <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M4.5 6.5 8 10l3.5-3.5" /></svg>
  </button>
  {#if open}
    <div class="acct-menu" role="menu" aria-label="Account" bind:this={menu}>
      {#if !guest}<a role="menuitem" tabindex="-1" href="/people" class="acct-item">Your account</a>{/if}
      <button type="button" role="menuitem" tabindex="-1" class="ghost acct-item" disabled={busy} onclick={out}>Sign out</button>
      {#if err}<p class="acct-err" role="alert">{err}</p>{/if}
    </div>
  {/if}
</div>

<style>
  .acct { position: relative; flex: none; min-width: 0; }
  .acct-btn { display: inline-flex; align-items: center; gap: 4px; min-height: 32px; max-width: 240px; padding: 0 6px; border: 0; background: none; border-radius: var(--radius-sm); color: var(--muted); font-size: 12.5px; white-space: nowrap; }
  .acct-btn:hover, .acct-btn[aria-expanded="true"] { background: var(--hover); color: var(--fg); }
  .acct-name { overflow: hidden; text-overflow: ellipsis; min-width: 0; }
  .acct-btn svg { flex: none; width: 12px; height: 12px; fill: none; stroke: currentColor; stroke-width: 1.6; stroke-linecap: round; stroke-linejoin: round; }
  .acct-menu { position: absolute; left: 0; top: calc(100% + 8px); z-index: 20; min-width: 200px; background: var(--raised); border: 1px solid var(--border-hover); border-radius: var(--radius); box-shadow: var(--elev-lg); padding: 6px 0; display: flex; flex-direction: column; }
  .acct-item { display: flex; align-items: center; width: 100%; padding: 8px 14px; text-align: left; font: 500 14px/1.2 var(--font); min-height: 0; border: 0; border-radius: 0; color: var(--fg); text-decoration: none; }
  .acct-item:hover { background: var(--hover); }
  .acct-err { margin: 4px 14px; font-size: 12.5px; color: var(--danger); }
  @media (pointer: coarse) { .acct-btn, .acct-item { min-height: 44px; } }
</style>
