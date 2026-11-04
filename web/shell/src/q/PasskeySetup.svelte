<svelte:options css="injected" />

<script module lang="ts">
  import type { Held } from "../held";
  import { type ConfirmationView, answerConfirmation, confirmations } from "./confirm.svelte";
  import { type PasskeySettings, type Registration, assertFor, makePasskey, passkeySettings, putSettings, register } from "./passkey";

  /** The page's passkey setup, one for every card and setting that offers
   * it: a registration held for its confirmation (`pending`) outlives the
   * card it was started on, which leaves the inbox once answered.
   * `finished` counts setups done on this page, and `settings` are the
   * settings after the last. */
  export const setup = $state<{
    pending: { reg: Registration; held: Held } | null;
    busy: boolean;
    err: string | null;
    finished: number;
    settings: PasskeySettings | null;
  }>({ pending: null, busy: false, err: null, finished: 0, settings: null });

  // While a registration waits, the page's confirmation feed tells how its
  // confirmation ended, wherever it was answered.
  let unheard: (() => void) | null = null;
  function wait(p: typeof setup.pending) {
    setup.pending = p;
    unheard?.();
    unheard = p ? confirmations().onEnded(ended) : null;
  }
  function ended(c: ConfirmationView) {
    if (setup.pending?.held.id !== c.id) return;
    if (c.status === "approved") { void finishSetup(c.id); return; }
    wait(null);
    setup.err = c.status === "declined" ? "The passkey was not added: its confirmation was declined." : "The passkey was not added: its confirmation expired.";
  }

  async function finish(reg: Registration, confirm?: string) {
    const r = await register(reg, confirm);
    if ("held" in r) { wait({ reg, held: r.held }); return; }
    wait(null);
    setup.settings = await putSettings({ required: true });
    setup.finished++;
  }

  /** Whether confirmation `id` holds the registration this page started:
   * the setup line answers it, so lists leave its card out. */
  export const heldHere = (id: string) => setup.pending?.held.id === id;

  /** Answers the held registration's confirmation from the setup line, as
   * its card would (with a passkey, where one is required); approved, the
   * registration is sent again and the setup finishes. */
  const answerHeld = (approve: boolean) => {
    const p = setup.pending;
    if (!p) return Promise.resolve();
    return run(async () => {
      if (!approve) { wait(null); await answerConfirmation(p.held.id, false); return; }
      const s = await passkeySettings(true);
      await answerConfirmation(p.held.id, true, s?.required ? await assertFor({ purpose: "confirm", confirmation_id: p.held.id }) : undefined);
      // A refused answer (Touch ID cancelled) keeps the wait; a refused registration ends it.
      try { await finish(p.reg, p.held.id); } catch (e) { wait(null); throw e; }
    });
  };

  async function run(f: () => Promise<void>, refused?: () => void) {
    if (setup.busy) return;
    setup.busy = true; setup.err = null;
    try { await f(); } catch (e) { refused?.(); setup.err = e instanceof Error ? e.message : String(e); } finally { setup.busy = false; }
  }

  /** The browser makes a passkey and the daemon registers it; while an
   * agent is live the registration waits for its own confirmation. */
  export const startSetup = () => run(async () => { await finish(await makePasskey()); });

  /** Sends the held registration again, presenting its confirmation; a
   * refusal (declined, expired or used) ends the wait. With `id`, only
   * when the held registration waits for confirmation `id`: what a card
   * calls once it approves one. */
  export function finishSetup(id?: string): Promise<void> {
    const p = setup.pending;
    if (!p || (id !== undefined && p.held.id !== id)) return Promise.resolve();
    return run(() => finish(p.reg, p.held.id), () => { wait(null); });
  }
</script>

<script lang="ts">
  // Setting up a passkey for confirmations: the browser makes one (Touch
  // ID, or another passkey, user verified), the daemon registers it, and
  // confirming then requires it. While an agent is live the registration
  // is itself a confirmed decision: the line says so and answers it in
  // place (**Confirm adding it** / **Cancel**), standing in for its card,
  // which this page's lists leave out (`heldHere`). Approving that card
  // anywhere else this page hears of finishes the setup too. `onDone`
  // hears the settings after a setup that finishes while this is shown.
  let { label = "Set up", onDone }: { label?: string; onDone?: (s: PasskeySettings) => void } = $props();
  const seen = setup.finished;
  $effect(() => { if (setup.finished > seen && setup.settings) onDone?.(setup.settings); });
</script>

<span class="pk-setup">
  {#if setup.pending}
    <span class="pk-held" role="status">Agents are running, so adding this passkey needs your confirmation too.</span>
    <button type="button" class="primary" disabled={setup.busy} onclick={() => answerHeld(true)}>Confirm adding it</button>
    <button type="button" class="ghost" disabled={setup.busy} onclick={() => answerHeld(false)}>Cancel</button>
  {:else}
    <button type="button" class="primary" disabled={setup.busy} onclick={startSetup}>{label}</button>
  {/if}
  {#if setup.err}<span class="err" role="alert">{setup.err}</span>{/if}
</span>

<style>
  :global {
    .pk-setup { display: inline-flex; flex-wrap: wrap; align-items: center; gap: 6px 8px; min-width: 0; }
    .pk-setup .pk-held { font-size: 12.5px; color: var(--muted); overflow-wrap: anywhere; }
    .pk-setup .err { color: var(--danger); font-size: 13px; }
  }
</style>
