<svelte:options css="injected" />

<script lang="ts">
  // The Content Security Policy setting (spec
  // 2026-11-15-site-own-content-policy), in the hosted Share dialog
  // (ShareDialog) and the Permissions panel (PermissionsPanel): one control
  // and one wording (content-security-policy-model.ts). `held` is what the
  // daemon holds. Turning it on first shows what that lets the clax's pages
  // do, and sends `unrestricted` (`set`) only once the person confirms;
  // Cancel sends nothing. Turning it off sends `self` at once.
  // A change `set` answers false for (refused, or another in flight) leaves
  // the box at what the daemon holds; `set` is the caller's call, which
  // carries the shell's confirmation flow (`held.ts`, `heldFetch`). While it
  // is `unrestricted` the line under it says so.
  import { tick } from "svelte";
  import type { ContentSecurityPolicy } from "../hosted/api";
  import { CSP_EXPLAIN, CSP_LABEL, CSP_NOTE } from "./content-security-policy-model";

  let { held, busy = false, set, note = null }: { held: ContentSecurityPolicy; busy?: boolean; set(p: ContentSecurityPolicy): Promise<boolean>; note?: string | null } = $props();
  /** Whether the setting is asking the person to confirm turning it on. */
  let asking = $state(false);
  let box: HTMLInputElement | undefined = $state();
  let ask: HTMLElement | undefined = $state();
  const uid = $props.id();
  const explainId = `csp-explain-${uid}`;
  const isUnrestricted = () => held === "unrestricted";

  async function change(b: HTMLInputElement) {
    if (b.checked) {
      b.checked = isUnrestricted();
      asking = true;
      await tick();
      ask?.querySelector<HTMLElement>("button.primary")?.focus();
      return;
    }
    asking = false;
    if (!(await set("self"))) b.checked = isUnrestricted();
  }
  async function confirm() {
    asking = false;
    const ok = await set("unrestricted");
    if (box) { if (!ok) box.checked = isUnrestricted(); box.focus(); }
  }
  function cancel() {
    asking = false;
    if (box) { box.checked = isUnrestricted(); box.focus(); }
  }
</script>

<label class="csp-sw"><input type="checkbox" name="content_security_policy" checked={held === "unrestricted"} bind:this={box}
  onchange={e => { void change(e.currentTarget); }} />{CSP_LABEL}</label>
{#if asking}
  <div class="csp-confirm" role="group" aria-labelledby={explainId} data-testid="csp-confirm" bind:this={ask}>
    <p id={explainId}>{CSP_EXPLAIN}</p>
    <div class="csp-actions">
      <button type="button" class="primary" disabled={busy} onclick={() => { void confirm(); }}>Confirm</button>
      <button type="button" onclick={cancel}>Cancel</button>
    </div>
  </div>
{/if}
{#if held === "unrestricted"}<p class="csp-meta" data-testid="csp-note">{CSP_NOTE}</p>{:else if note}<p class="csp-meta">{note}</p>{/if}

<style>
  .csp-sw { display: flex; align-items: center; gap: 8px; font-size: 14px; }
  .csp-confirm { margin: 8px 0 0; padding: 10px 12px; border: 1px solid var(--border-hover); border-radius: var(--radius-sm); font-size: 13px; line-height: 1.5; }
  .csp-confirm p { margin: 0 0 8px; }
  .csp-actions { display: flex; gap: 8px; flex-wrap: wrap; }
  .csp-meta { margin: 8px 0 0; font-size: 12.5px; color: var(--muted); line-height: 1.5; }
</style>
