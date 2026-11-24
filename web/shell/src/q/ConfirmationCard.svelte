<svelte:options css="injected" />

<script lang="ts">
  // A decision waiting for the owner's click (docs/contract.md, "Browser
  // confirmations"): "Confirm: <decision in words>", "Confirm only if you
  // asked for this.", how many agent sessions are live now (any of which
  // could have made the call with the daemon token; the line opens to
  // name them), and **Confirm** / **Decline**. Answered, the card says how,
  // and keeps the keyboard's focus. One the daemon answers has expired
  // says "Expired", drops its buttons and leaves after a moment; any other
  // refusal (a browser session that may not confirm) shows the daemon's
  // own words and keeps the buttons, and one answered elsewhere or unknown
  // has the feed read the open ones again. Every word shown is text. Its styles
  // travel with it (injected on mount): the inbox and the toasts both
  // show it. Where the owner requires a passkey, **Confirm** asks the
  // browser for one first, and the card says so. On first use, where a
  // passkey can be used here and none is set up, the card offers "Require
  // a passkey to confirm" (**Set up** / **Don't ask again**,
  // remembered; Settings still sets one up). While a passkey registration
  // waits for its confirmation, the first other open card carries its line
  // (**Confirm adding it** / **Cancel**) in place of its card. Settings
  // lists the passkeys registered and how each was added.
  import { after } from "../clock";
  import { relativeTime } from "../format";
  import { Refused } from "../held";
  import { type ConfirmationView, agentLabel, answerConfirmation, carriesSetup, confirmations, registersPasskey } from "./confirm.svelte";
  import { type PasskeySettings, assertFor, passkeySettings, putSettings, unavailableHere } from "./passkey";
    import PasskeySetup, { ended, setup } from "./PasskeySetup.svelte";

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
  // A setup finished while this card is shown, wherever it was started.
  const seen = setup.finished;
  const setUp = $derived(setup.finished > seen);
  // A passkey registration waiting: the first other open card carries its line, and no card offers another.
  const open = $derived(confirmations().open);
  const carries = $derived(status === "open" && carriesSetup(open, c));
  const waiting = $derived(open.some(registersPasskey));
  const notNow = () => { void putSettings({ offer_dismissed: true }).then(s => { keys = s; }, () => {}); };
  const running = $derived(`${c.agents.length} agent${c.agents.length === 1 ? "" : "s"} running now could have asked as you`);
  /** How long a card found expired here stays, saying so, where no one else removes it. */
  const LEAVE_MS = 2500;
  const OUTCOME: Record<string, string> = { approved: "Confirmed", declined: "Declined", expired: "Expired: make the call again for a new one" };

  /** The card's element, found by query, as the inbox's other cards find theirs. */
  const self = () => [...document.querySelectorAll<HTMLElement>(".cfcard")].find(e => e.dataset.confirmation === c.id);
  async function answer(approve: boolean) {
    if (busy) return;
    busy = true; err = null;
    try {
      const s = approve ? await passkeySettings(true) : null;
      if (s) keys = s;
      const assertion = s?.required ? await assertFor({ purpose: "confirm", confirmation_id: c.id }) : undefined;
      const answered = await answerConfirmation(c.id, approve, assertion);
      made = answered.status;
      onanswered?.(answered);
      // A passkey registration's: the daemon added it on approval.
      void ended(answered);
      queueMicrotask(() => self()?.focus({ preventScroll: true }));
    } catch (x) {
      if (x instanceof Refused && x.status === 410) {
        // Past its time: nothing to answer. The toasts remove it (`onanswered`); elsewhere the feed does.
        const over: ConfirmationView = { ...c, status: "expired" };
        made = "expired";
        if (onanswered) onanswered(over); else after(LEAVE_MS, () => confirmations().take(over));
        return;
      }
      err = x instanceof Error ? x.message : String(x);
      if (x instanceof Refused && (x.status === 409 || x.status === 404)) void confirmations().refresh();
    } finally { busy = false; }
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<article class="cfcard" data-confirmation={c.id} data-status={status} tabindex="-1" aria-label={`Confirm: ${c.words}`}>
  <header class="chead">
    <span class="cwhat">Confirm: {c.words}</span>
    <time datetime={c.created_at}>{relativeTime(c.created_at, now ?? new Date())}</time>
  </header>
  {#if status === "open"}
    <p class="cwhy">Confirm only if you asked for this.</p>
    {#if c.agents.length}
      <details class="cagents">
        <summary>{running}</summary>
        <ul aria-label="Agents running now">{#each c.agents as a (a.handle)}<li>{agentLabel(a)}</li>{/each}</ul>
      </details>
    {/if}
    <div class="acts">
      <button type="button" class="primary" disabled={busy} onclick={() => answer(true)}>Confirm</button>
      <button type="button" class="ghost" disabled={busy} onclick={() => answer(false)}>Decline</button>
      {#if keys?.required}<span class="ckey">Confirm asks for your passkey.</span>{/if}
    </div>
  {:else}
    <p class="outcome" role="status">{OUTCOME[status] ?? status}</p>
  {/if}
  {#if err}<p class="err" role="alert">{err}</p>{/if}
  {#if setUp}
    <p class="coffer" role="status">Passkey added: Confirm now asks for your passkey.</p>
  {:else if carries}
    <div class="coffer"><PasskeySetup onDone={s => { keys = s; }} /></div>
  {:else if offer && !waiting && !registersPasskey(c)}
    <div class="coffer">
      <span>Require a passkey to confirm</span>
      <PasskeySetup onDone={s => { keys = s; }} />
      <button type="button" class="ghost" onclick={notNow}>Don't ask again</button>
    </div>
  {/if}
</article>

<style>
  :global {
    .cfcard { position: relative; display: grid; gap: 6px; background: var(--card); border: 1px solid var(--border); border-left: 3px solid var(--danger); border-radius: var(--radius); padding: 11px 12px 10px 13px; min-width: 0; font-size: 13.5px; }
    .cfcard:focus { outline: none; }
    .cfcard:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; }
    .cfcard:not([data-status="open"]) { border-left-color: var(--border-strong); }
    .cfcard p { margin: 0; }
    .cfcard .chead { display: flex; flex-wrap: wrap; align-items: baseline; gap: 2px 8px; min-width: 0; font-weight: 600; overflow-wrap: anywhere; }
    .cfcard .cwhat { min-width: 0; }
    .cfcard time { margin-left: auto; font: 400 11.5px var(--font); color: var(--muted); white-space: nowrap; font-variant-numeric: tabular-nums; }
    .cfcard .cwhy { color: var(--muted); font-size: 12.5px; }
    .cfcard .cagents { font-size: 12.5px; color: var(--muted); min-width: 0; }
    .cfcard .cagents summary { cursor: pointer; width: fit-content; max-width: 100%; }
    .cfcard .cagents ul { margin: 4px 0 0; padding-left: 18px; max-height: 12.5em; overflow-y: auto; overflow-wrap: anywhere; }
    .cfcard .acts { display: flex; flex-wrap: wrap; align-items: center; gap: 6px; margin-top: 4px; }
    .cfcard .ckey { margin-left: 4px; font-size: 12.5px; color: var(--muted); }
    .cfcard .outcome { font-weight: 600; font-size: 13px; }
    .cfcard .err { color: var(--danger); font-size: 13px; }
    .cfcard .coffer { display: flex; flex-wrap: wrap; align-items: center; gap: 6px 8px; margin-top: 4px; padding-top: 8px; border-top: 1px solid var(--border); font-size: 12.5px; color: var(--muted); }
    @media (max-width: 520px) { .cfcard .acts button { flex: 1 1 100%; } }
  }
</style>
