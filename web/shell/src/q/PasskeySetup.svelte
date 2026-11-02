<svelte:options css="injected" />

<script lang="ts">
  // Setting up a passkey for confirmations: the browser makes one (Touch
  // ID, or another passkey, user verified), the daemon registers it, and
  // confirming then requires it. While an agent is live the registration
  // is itself a confirmed decision: the line says so, and **Finish setup**
  // sends it again once that confirmation is approved. `onDone` hears the
  // settings after.
  import type { Held } from "../held";
  import { type PasskeySettings, type Registration, makePasskey, putSettings, register } from "./passkey";

  let { label = "Set up", onDone }: { label?: string; onDone?: (s: PasskeySettings) => void } = $props();
  let busy = $state(false);
  let err = $state<string | null>(null);
  let waiting = $state<{ reg: Registration; held: Held } | null>(null);

  async function finish(r: Awaited<ReturnType<typeof register>>, reg: Registration) {
    if ("held" in r) { waiting = { reg, held: r.held }; return; }
    waiting = null;
    onDone?.(await putSettings({ required: true }));
  }
  async function start() {
    if (busy) return;
    busy = true; err = null;
    try {
      const reg = await makePasskey();
      await finish(await register(reg), reg);
    } catch (e) { err = e instanceof Error ? e.message : String(e); } finally { busy = false; }
  }
  async function again() {
    if (busy || !waiting) return;
    busy = true; err = null;
    const { reg, held } = waiting;
    // A refusal (the confirmation declined, expired or used) ends the wait: Set up starts again.
    try { await finish(await register(reg, held.id), reg); } catch (e) { waiting = null; err = e instanceof Error ? e.message : String(e); } finally { busy = false; }
  }
</script>

<span class="pk-setup">
  {#if waiting}
    <span class="pk-held" role="status">{waiting.held.message}</span>
    <button type="button" class="primary" disabled={busy} onclick={again}>Finish setup</button>
  {:else}
    <button type="button" class="primary" disabled={busy} onclick={start}>{label}</button>
  {/if}
  {#if err}<span class="err" role="alert">{err}</span>{/if}
</span>

<style>
  :global {
    .pk-setup { display: inline-flex; flex-wrap: wrap; align-items: center; gap: 6px 8px; min-width: 0; }
    .pk-setup .pk-held { font-size: 12.5px; color: var(--muted); overflow-wrap: anywhere; }
    .pk-setup .err { color: var(--danger); font-size: 13px; }
  }
</style>
