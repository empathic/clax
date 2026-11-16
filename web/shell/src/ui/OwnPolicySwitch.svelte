<svelte:options css="injected" />

<script lang="ts">
  // The content policy switch (spec 2026-11-15-site-own-content-policy), in
  // the hosted Share dialog (ShareDialog) and the Permissions panel
  // (PermissionsPanel): one control and one wording (own-policy-model.ts).
  // `held` is what the daemon holds. Turning it on first shows what that
  // lets the clax's pages do, and sends `own` (`set`) only once the person
  // confirms; Cancel sends nothing. Turning it off sends `locked` at once.
  // A change `set` answers false for (refused, or another in flight) leaves
  // the box at what the daemon holds; `set` is the caller's call, which
  // carries the shell's confirmation flow (`held.ts`, `heldFetch`). While it
  // is `own` the line under it says so.
  import { tick } from "svelte";
  import type { ContentPolicy } from "../hosted/api";
  import { OWN_POLICY_EXPLAIN, OWN_POLICY_LABEL, OWN_POLICY_NOTE } from "./own-policy-model";

  let { held, busy = false, set, note = null }: { held: ContentPolicy; busy?: boolean; set(p: ContentPolicy): Promise<boolean>; note?: string | null } = $props();
  /** Whether the switch is asking the person to confirm turning it on. */
  let asking = $state(false);
  let box: HTMLInputElement | undefined = $state();
  let ask: HTMLElement | undefined = $state();
  const uid = $props.id();
  const explainId = `own-explain-${uid}`;
  const isOwn = () => held === "own";

  async function change(b: HTMLInputElement) {
    if (b.checked) {
      b.checked = isOwn();
      asking = true;
      await tick();
      ask?.querySelector<HTMLElement>("button.primary")?.focus();
      return;
    }
    asking = false;
    if (!(await set("locked"))) b.checked = isOwn();
  }
  async function confirm() {
    asking = false;
    const ok = await set("own");
    if (box) { if (!ok) box.checked = isOwn(); box.focus(); }
  }
  function cancel() {
    asking = false;
    if (box) { box.checked = isOwn(); box.focus(); }
  }
</script>

<label class="own-sw"><input type="checkbox" name="own_policy" checked={held === "own"} bind:this={box}
  onchange={e => { void change(e.currentTarget); }} />{OWN_POLICY_LABEL}</label>
{#if asking}
  <div class="own-confirm" role="group" aria-labelledby={explainId} data-testid="own-confirm" bind:this={ask}>
    <p id={explainId}>{OWN_POLICY_EXPLAIN}</p>
    <div class="own-actions">
      <button type="button" class="primary" disabled={busy} onclick={() => { void confirm(); }}>Confirm</button>
      <button type="button" onclick={cancel}>Cancel</button>
    </div>
  </div>
{/if}
{#if held === "own"}<p class="own-meta" data-testid="own-note">{OWN_POLICY_NOTE}</p>{:else if note}<p class="own-meta">{note}</p>{/if}

<style>
  .own-sw { display: flex; align-items: center; gap: 8px; font-size: 14px; }
  .own-confirm { margin: 8px 0 0; padding: 10px 12px; border: 1px solid var(--border-hover); border-radius: var(--radius-sm); font-size: 13px; line-height: 1.5; }
  .own-confirm p { margin: 0 0 8px; }
  .own-actions { display: flex; gap: 8px; flex-wrap: wrap; }
  .own-meta { margin: 8px 0 0; font-size: 12.5px; color: var(--muted); line-height: 1.5; }
</style>
