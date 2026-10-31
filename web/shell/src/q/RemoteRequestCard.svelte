<svelte:options css="injected" />

<script lang="ts">
  // An agent's request to connect this Clax to another Clax instance, in
  // the owner's inbox (spec 2026-10-29-remote-instances §3): "<agent> wants
  // to connect to <name> (<origin>) to read <url>", the agent's reason,
  // and **Connect**, **Not now** and **Never for this instance**. Connect
  // starts this Clax's sign-in there and opens the instance's approval page
  // in a new tab, where the person approves as themselves; the agent is
  // told once the sign-in ends, with nothing more to do here. Not now holds
  // the instance's requests off for a day; Never mutes it until it is
  // unmuted in Settings. While an agent is live an answer may wait for the
  // owner's confirmation: the card says where to confirm, and the same
  // answer pressed again presents it. An answered request says how it went. Every
  // answer is the owner's browser's (M20); the daemon's refusals show as
  // they are. Names and URLs are text.
  import type { InboxItem, InboxRemoteRequest } from "../api";
  import { relativeTime } from "../format";
  import { type Held, Refused } from "../held";
  import { refusal } from "../people/refusal";
  import { remoteRequestText } from "./inbox-model";
  import { type RemoteAnswer, answerRemote } from "./remote-api";

  let { item, now }: { item: InboxItem; now?: Date } = $props();
  // The request as an answer made here left it, until the item says so.
  let made = $state<InboxRemoteRequest["status"] | "connecting" | null>(null);
  // The approval page, when the browser kept the new tab from opening.
  let page = $state<string | null>(null);
  let busy = $state(false);
  let err = $state<string | null>(null);
  // An answer held for the owner's confirmation, until it is made again.
  let waiting = $state<{ answer: RemoteAnswer; held: Held } | null>(null);

  const r = $derived(item.remote_request ?? null);
  const status = $derived(r?.status && r.status !== "open" ? r.status : made ?? r?.status ?? null);
  const OUTCOME: Record<string, string> = {
    connected: "Connected: the agent can read it now",
    dismissed: "Not connected",
    muted: "Never for this instance: unmute it in Settings, Remotes",
    expired: "Expired: nobody answered within a day",
  };
  const outcome = $derived(status === "connecting" ? `Approve on ${r?.name ?? "the other instance"} in the new tab; the agent is told once you do.` : status ? OUTCOME[status] ?? "" : "This request is gone");

  async function answer(a: RemoteAnswer) {
    if (!r || busy) return;
    busy = true; err = null; page = null;
    // Opened while the click still counts as the person's, then sent to the approval page.
    const tab = a === "connect" ? window.open("", "_blank") : null;
    try {
      const res = await answerRemote(r.id, a, waiting?.answer === a ? waiting.held.id : undefined);
      waiting = null;
      if (a === "connect") {
        const url = res.verification_url ?? null;
        if (tab && url) { tab.opener = null; tab.location.href = url; } else { tab?.close(); page = url; }
        made = "connecting";
      } else {
        made = res.request.status;
      }
    } catch (x) {
      tab?.close();
      // Only a held answer waits; a confirmation declined, expired or used is gone, and the next press asks afresh.
      waiting = x instanceof Refused && x.held ? { answer: a, held: x.held } : null;
      if (!waiting) err = refusal(x);
    } finally { busy = false; }
  }
</script>

<article class="rcard" data-request={r?.id ?? ""} data-status={status ?? "gone"} tabindex="-1" aria-label="Request to connect a Clax">
  <header class="chead">
    <span>{remoteRequestText(item)}</span>
    <time datetime={item.created_at}>{relativeTime(item.created_at, now ?? new Date())}</time>
  </header>
  {#if r?.reason}<p class="meta reason">“{r.reason}”</p>{/if}
  {#if item.artifact && item.thread}<p class="meta">Asked while working <a class="where" href={`/a/${item.artifact.id}?thread=${item.thread.id}`}>in a thread{item.artifact.title ? ` on ${item.artifact.title}` : ""}</a></p>{/if}
  {#if r && r.askers > 1}<p class="meta">{r.askers} agents asked</p>{/if}
  {#if status === "open"}
    {#if waiting}<p class="meta held" role="status">{waiting.held.message}, then answer again.</p>{/if}
    <div class="acts">
      <button type="button" class="primary" disabled={busy} onclick={() => answer("connect")}>Connect</button>
      <button type="button" class="ghost" disabled={busy} onclick={() => answer("not_now")}>Not now</button>
      <button type="button" class="ghost" disabled={busy} onclick={() => answer("never")}>Never for this instance</button>
    </div>
  {:else}
    <p class="outcome" role="status">{outcome}</p>
    {#if page}<p class="meta">Open the approval page: <a href={page} target="_blank" rel="noopener noreferrer">{page}</a></p>{/if}
  {/if}
  {#if err}<p class="err" role="alert">{err}</p>{/if}
</article>

<style>
  :global {
    .rcard { position: relative; display: grid; gap: 6px; background: var(--card); border: 1px solid var(--border); border-left: 3px solid var(--agent); border-radius: var(--radius); padding: 11px 12px 10px 13px; min-width: 0; font-size: 13.5px; }
    .rcard:focus { outline: none; }
    .rcard:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; }
    .rcard:not([data-status="open"]) { border-left-color: var(--border-strong); }
    .rcard p { margin: 0; }
    .rcard .chead { display: flex; flex-wrap: wrap; align-items: baseline; gap: 2px 8px; min-width: 0; font-weight: 600; overflow-wrap: anywhere; }
    .rcard .chead > span { min-width: 0; }
    .rcard time { margin-left: auto; font: 400 11.5px var(--font); color: var(--muted); white-space: nowrap; font-variant-numeric: tabular-nums; }
    .rcard .meta { color: var(--muted); font-size: 12.5px; overflow-wrap: anywhere; }
    .rcard a { color: var(--fg); text-decoration: underline; text-decoration-color: var(--border-strong); text-underline-offset: 2px; overflow-wrap: anywhere; }
    .rcard .acts { display: flex; flex-wrap: wrap; gap: 6px; margin-top: 4px; }
    .rcard .acts button { max-width: 100%; white-space: normal; overflow-wrap: anywhere; text-align: start; }
    .rcard .outcome { font-weight: 600; font-size: 13px; }
    .rcard .err { color: var(--danger); font-size: 13px; }
    @media (max-width: 520px) { .rcard .acts button { flex: 1 1 100%; justify-content: flex-start; } }
  }
</style>
