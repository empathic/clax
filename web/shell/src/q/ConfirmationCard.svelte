<svelte:options css="injected" />

<script lang="ts">
  // A decision waiting for the owner's click (docs/contract.md, "Browser
  // confirmations"): "Confirm: <decision in words>", the agent sessions
  // live now (any of which could have made the call with the daemon
  // token), and **Confirm** / **Decline**. Answered, the card says how,
  // and keeps the keyboard's focus; a refusal (a browser session that may
  // not confirm, one expired or answered elsewhere) shows the daemon's own
  // words and keeps the buttons. Every word shown is text. Its styles
  // travel with it (injected on mount): the inbox and the toasts both
  // show it. Where the owner requires a passkey, **Confirm** asks the
  // browser for one (Touch ID, or another) first. On first use, where a
  // passkey can be used here and none is set up, the card offers "Require
  // Touch ID or a passkey to confirm" (**Set up** / **Not now**, which is
  // remembered).
  import { relativeTime } from "../format";
  import { type ConfirmationView, agentLabel, answerConfirmation } from "./confirm.svelte";
  import { type PasskeySettings, assertFor, passkeySettings, putSettings, unavailableHere } from "./passkey";
  import PasskeySetup from "./PasskeySetup.svelte";

  let { c, now, onanswered }: {
    c: ConfirmationView;
    now?: Date;
    /** Hears the confirmation as it is once answered here. */
    onanswered?: (c: ConfirmationView) => void;
  } = $props();
  // The answer made here, until the view itself says it.
  let made = $state<ConfirmationView["status"] | null>(null);
  let busy = $state(false);
  let err = $state<string | null>(null);
  const status = $derived(c.status !== "open" ? c.status : made ?? c.status);
  let keys = $state<PasskeySettings | null>(null);
  void passkeySettings().then(s => { keys = s; });
  // The first-use offer: passkeys can be used here, none is required, and it was not put off.
  const offer = $derived(!!keys && keys.available && !keys.require_passkey && !keys.offer_dismissed && unavailableHere() === null);
  let offered = $state<string | null>(null);
  const notNow = () => { void putSettings({ offer_dismissed: true }).then(s => { keys = s; }, () => {}); };
  const OUTCOME: Record<string, string> = { approved: "Confirmed", declined: "Declined", expired: "Expired: make the call again for a new one" };

  /** The card's element, found by query, as the inbox's other cards find theirs. */
  const self = () => [...document.querySelectorAll<HTMLElement>(".ccard")].find(e => e.dataset.confirmation === c.id);
  async function answer(approve: boolean) {
    if (busy) return;
    busy = true; err = null;
    try {
      const s = approve ? await passkeySettings(true) : null;
      if (s) keys = s;
      const assertion = s?.required ? await assertFor({ purpose: "confirm", confirmation_id: c.id }) : undefined;
      const after = await answerConfirmation(c.id, approve, assertion);
      made = after.status;
      onanswered?.(after);
      queueMicrotask(() => self()?.focus({ preventScroll: true }));
    } catch (x) { err = x instanceof Error ? x.message : String(x); } finally { busy = false; }
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<article class="ccard" data-confirmation={c.id} data-status={status} tabindex="-1" aria-label={`Confirm: ${c.words}`}>
  <header class="chead">
    <span class="cwhat">Confirm: {c.words}</span>
    <time datetime={c.created_at}>{relativeTime(c.created_at, now ?? new Date())}</time>
  </header>
  {#if c.agents.length}
    <p class="cwhy">Agents running now could have asked for this as you:</p>
    <ul class="cagents" aria-label="Agents running now">{#each c.agents as a (a.handle)}<li>{agentLabel(a)}</li>{/each}</ul>
  {/if}
  {#if status === "open"}
    <div class="acts">
      <button type="button" class="primary" disabled={busy} onclick={() => answer(true)}>Confirm</button>
      <button type="button" class="ghost" disabled={busy} onclick={() => answer(false)}>Decline</button>
    </div>
  {:else}
    <p class="outcome" role="status">{OUTCOME[status] ?? status}</p>
  {/if}
  {#if err}<p class="err" role="alert">{err}</p>{/if}
  {#if offered}
    <p class="coffer" role="status">{offered}</p>
  {:else if offer}
    <div class="coffer">
      <span>Require Touch ID or a passkey to confirm</span>
      <PasskeySetup onDone={s => { keys = s; offered = "Touch ID or a passkey now confirms these decisions."; }} />
      <button type="button" class="ghost" onclick={notNow}>Not now</button>
    </div>
  {/if}
</article>

<style>
  :global {
    .ccard { position: relative; display: grid; gap: 6px; background: var(--card); border: 1px solid var(--border); border-left: 3px solid var(--danger); border-radius: var(--radius); padding: 11px 12px 10px 13px; min-width: 0; font-size: 13.5px; }
    .ccard:focus { outline: none; }
    .ccard:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; }
    .ccard:not([data-status="open"]) { border-left-color: var(--border-strong); }
    .ccard p { margin: 0; }
    .ccard .chead { display: flex; flex-wrap: wrap; align-items: baseline; gap: 2px 8px; min-width: 0; font-weight: 600; overflow-wrap: anywhere; }
    .ccard .cwhat { min-width: 0; }
    .ccard time { margin-left: auto; font: 400 11.5px var(--font); color: var(--muted); white-space: nowrap; font-variant-numeric: tabular-nums; }
    .ccard .cwhy { color: var(--muted); font-size: 12.5px; }
    .ccard .cagents { margin: 0; padding-left: 18px; font-size: 12.5px; overflow-wrap: anywhere; }
    .ccard .acts { display: flex; flex-wrap: wrap; gap: 6px; margin-top: 4px; }
    .ccard .outcome { font-weight: 600; font-size: 13px; }
    .ccard .err { color: var(--danger); font-size: 13px; }
    .ccard .coffer { display: flex; flex-wrap: wrap; align-items: center; gap: 6px 8px; margin-top: 4px; padding-top: 8px; border-top: 1px solid var(--border); font-size: 12.5px; color: var(--muted); }
    @media (max-width: 520px) { .ccard .acts button { flex: 1 1 100%; } }
  }
</style>
