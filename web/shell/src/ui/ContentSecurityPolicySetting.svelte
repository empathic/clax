<svelte:options css="injected" />

<script lang="ts">
  // The Content Security Policy setting (spec
  // 2026-11-15-site-own-content-policy), in the hosted Share dialog
  // (ShareDialog) and the Permissions panel (PermissionsPanel): one control
  // and one wording (content-security-policy-model.ts). `held` is what the
  // daemon holds; a line says it (`note` while `self`, when the caller
  // gives one), and one button under it changes it. While `self` the
  // button opens what serving unrestricted lets the clax's pages do, and
  // `unrestricted` is sent (`set`) only once the person confirms; Cancel
  // sends nothing. While `unrestricted` the button sends `self` at once.
  // A change `set` answers false for (refused, or another in flight) leaves
  // the setting at what the daemon holds; `set` is the caller's call, which
  // carries the shell's confirmation flow (`held.ts`, `heldFetch`).
  import { tick } from "svelte";
  import type { ContentSecurityPolicy } from "../hosted/api";
  import { CSP_EXPLAIN, CSP_NOTE, CSP_RESTORE, CSP_UNRESTRICT } from "./content-security-policy-model";

  let { held, busy = false, set, note = null }: { held: ContentSecurityPolicy; busy?: boolean; set(p: ContentSecurityPolicy): Promise<boolean>; note?: string | null } = $props();
  /** Whether the setting is asking the person to confirm turning it on. */
  let asking = $state(false);
  let button: HTMLButtonElement | undefined = $state();
  let ask: HTMLElement | undefined = $state();
  const uid = $props.id();
  const explainId = `csp-explain-${uid}`;
  const confirmId = `csp-confirm-${uid}`;

  async function press() {
    if (held === "unrestricted") { await set("self"); return; }
    asking = !asking;
    if (!asking) return;
    await tick();
    ask?.querySelector<HTMLElement>("button.primary")?.focus();
  }
  async function confirm() {
    asking = false;
    await set("unrestricted");
    button?.focus();
  }
  function cancel() {
    asking = false;
    button?.focus();
  }
</script>

{#if held === "unrestricted"}<p class="csp-meta" data-testid="csp-note">{CSP_NOTE}</p>{:else if note}<p class="csp-meta">{note}</p>{/if}
<button type="button" class="csp-change" name="content_security_policy" disabled={busy}
  aria-expanded={held === "unrestricted" ? undefined : asking} aria-controls={held === "unrestricted" ? undefined : confirmId}
  bind:this={button} onclick={() => { void press(); }}>{held === "unrestricted" ? CSP_RESTORE : CSP_UNRESTRICT}</button>
{#if asking && held !== "unrestricted"}
  <div class="csp-confirm" id={confirmId} role="group" aria-labelledby={explainId} data-testid="csp-confirm" bind:this={ask}>
    <p id={explainId}>{CSP_EXPLAIN}</p>
    <div class="csp-actions">
      <button type="button" class="primary" disabled={busy} onclick={() => { void confirm(); }}>Confirm</button>
      <button type="button" onclick={cancel}>Cancel</button>
    </div>
  </div>
{/if}

<style>
  .csp-change { margin: 8px 0 0; }
  .csp-confirm { margin: 8px 0 0; padding: 10px 12px; border: 1px solid var(--border-hover); border-radius: var(--radius-sm); font-size: 13px; line-height: 1.5; }
  .csp-confirm p { margin: 0 0 8px; }
  .csp-actions { display: flex; gap: 8px; flex-wrap: wrap; }
  .csp-meta { margin: 0; font-size: 12.5px; color: var(--muted); line-height: 1.5; }
</style>
