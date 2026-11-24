<svelte:options css="injected" />

<script module lang="ts">
  import { type ConfirmationView, answerConfirmation, confirmations, registersPasskey } from "./confirm.svelte";
  import { type PasskeySettings, assertFor, makePasskey, passkeySettings, putSettings, register } from "./passkey";

  /** The page's passkey setup, one for every card and setting that offers
   * it. `finished` counts setups done while the page is open, wherever
   * they were confirmed, and `settings` are the settings after the last. */
  export const setup = $state<{
    busy: boolean;
    err: string | null;
    finished: number;
    settings: PasskeySettings | null;
  }>({ busy: false, err: null, finished: 0, settings: null });

  /** The registration waiting for its confirmation, if any: the daemon
   * holds it, so it outlives the card, the tab and a reload. */
  export const held = (): ConfirmationView | undefined => confirmations().open.find(registersPasskey);

  // How a registration's confirmation ended, heard once each, wherever it
  // was answered: approved, the daemon added the passkey and requires it.
  const heard = new Set<string>();
  const cancelled = new Set<string>();
  /** Takes a registration's confirmation as answered or expired. */
  export async function ended(c: ConfirmationView): Promise<void> {
    if (!registersPasskey(c) || c.status === "open" || heard.has(c.id)) return;
    heard.add(c.id);
    if (c.status === "approved") { await done(); return; }
    if (cancelled.has(c.id)) return;
    setup.err = c.status === "declined" ? "The passkey was not added: its confirmation was declined." : "The passkey was not added: its confirmation expired.";
  }
  confirmations().onEnded(c => { void ended(c); });

  async function done() {
    setup.settings = await passkeySettings(true);
    setup.finished++;
  }

  async function run(f: () => Promise<void>) {
    if (setup.busy) return;
    setup.busy = true; setup.err = null;
    try { await f(); } catch (e) { setup.err = e instanceof Error ? e.message : String(e); } finally { setup.busy = false; }
  }

  /** The browser makes a passkey and the daemon registers it, requiring it
   * from then on; while an agent is live the daemon holds it for its own
   * confirmation, and approving that adds it. */
  export const startSetup = () => run(async () => {
    const r = await register(await makePasskey());
    if ("held" in r) { await confirmations().refresh(); return; }
    await putSettings({ required: true });
    await done();
  });

  /** Answers the held registration's confirmation from the setup line, as
   * its card would (with a passkey, where one is required). */
  const answerHeld = (approve: boolean) => {
    const c = held();
    if (!c) return Promise.resolve();
    return run(async () => {
      if (!approve) cancelled.add(c.id);
      const s = approve ? await passkeySettings(true) : null;
      const after = await answerConfirmation(c.id, approve, s?.required ? await assertFor({ purpose: "confirm", confirmation_id: c.id }) : undefined);
      confirmations().take(after);
    });
  };
</script>

<script lang="ts">
  // Setting up a passkey for confirmations: the browser makes one (Touch
  // ID, or another passkey, user verified), the daemon registers it, and
  // confirming then requires it. While an agent is live the registration
  // is itself a confirmed decision, which the daemon holds: the line says
  // so and answers it in place (**Confirm adding it** / **Cancel**),
  // standing in for its card (`cardsOf`). Approving it anywhere, in any
  // tab or after a reload, adds the passkey. `onDone` hears the settings
  // after a setup that finishes while this is shown.
  let { label = "Set up", onDone }: { label?: string; onDone?: (s: PasskeySettings) => void } = $props();
  const seen = setup.finished;
  $effect(() => { if (setup.finished > seen && setup.settings) onDone?.(setup.settings); });
</script>

<span class="pk-setup">
  {#if held()}
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
