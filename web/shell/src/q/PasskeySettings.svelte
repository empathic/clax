<svelte:options css="injected" />

<script lang="ts">
  // Settings' Confirmations section (docs/contract.md, "Browser
  // confirmations", "Passkeys"): **Require Touch ID or a passkey to
  // confirm** (on/off), the passkeys registered (each with **Remove**), and
  // **Add a passkey**. Turning the switch off, and removing a passkey while
  // it is on, ask the browser for a passkey first. Where no passkey can be
  // used (an address that is no secure context, a browser without
  // passkeys), the switch is off and disabled and says why: confirming is
  // then by any owner browser session. A daemon with no passkeys (hosted)
  // shows nothing.
  import { type PasskeySettings, assertFor, passkeySettings, putSettings, removePasskey, unavailableHere } from "./passkey";
  import PasskeyList from "./PasskeyList.svelte";
  import PasskeySetup from "./PasskeySetup.svelte";
  import Switch from "../bot/Switch.svelte";

  let { now }: { now?: Date } = $props();
  let s = $state<PasskeySettings | null>(null);
  let busy = $state(false);
  let err = $state<string | null>(null);
  void passkeySettings(true).then(x => { s = x; });
  const why = $derived(s ? (s.available ? unavailableHere() : s.unavailable_reason) : null);
  const disabled = $derived(!!why || !s?.passkeys.length);

  async function run(f: () => Promise<PasskeySettings>) {
    if (busy) return;
    busy = true; err = null;
    try { s = await f(); } catch (e) { err = e instanceof Error ? e.message : String(e); } finally { busy = false; }
  }
  const toggle = (on: boolean) => run(async () => on
    ? putSettings({ required: true })
    : putSettings({ required: false, ...s?.required && { assertion: await assertFor({ purpose: "disable" }) } }));
  const remove = (id: string) => run(async () =>
    removePasskey(id, s?.required ? await assertFor({ purpose: "remove", credential_id: id }) : undefined));
</script>

{#if s}
  <section class="bcard pk" aria-labelledby="pk-h">
    <h3 id="pk-h">Confirmations</h3>
    <p class="note">While an agent is running, the decisions that matter made with the daemon token wait for your click in this browser. You can also require Touch ID or a passkey to confirm.</p>
    <div class="field">
      <Switch checked={s.require_passkey && !why} label="Require Touch ID or a passkey to confirm" describedby="pk-why" {disabled} {busy} onchange={toggle} />
      <small id="pk-why">{why ?? (s.passkeys.length ? (s.required ? "Each confirmation asks for your passkey." : "Confirming takes a click in your browser.") : "Add a passkey first.")}</small>
    </div>
    <PasskeyList keys={s.passkeys} {now} {busy} onRemove={remove} />
    <p class="note">Locked out (a passkey you cannot use, or one you did not add)? Stop Clax and run <code>clax passkeys reset</code>.</p>
    {#if !why}<PasskeySetup label="Add a passkey" onDone={x => { s = x; }} />{/if}
    {#if err}<p class="err" role="alert">{err}</p>{/if}
  </section>
{/if}

<style>
  :global {
    .pk .field { display: grid; gap: 4px; }
  }
</style>
