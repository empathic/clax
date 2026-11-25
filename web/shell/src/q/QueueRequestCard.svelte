<svelte:options css="injected" />

<script lang="ts">
  // An agent's request for a report queue, for its approver, in the inbox
  // and on the Queues page: "<agent> asks for a report queue “<name>”",
  // the origins the queue would accept reports from, what its reporters
  // would see, the agent's note and when the request expires. While
  // pending: **Approve**, which makes the queue with the approver as its
  // admin and gives the agent one service grant for it, once, and
  // **Deny**. A decision may wait for the owner's confirmation while an
  // agent of theirs is live: the card says where to confirm, and the same
  // decision pressed again presents it. A request decided, cancelled or
  // expired says how it went (an approved one links its queue), and a
  // decision on one no longer pending shows how it stands instead.
  // Names, notes and origins are text.
  import type { QueueRequest } from "../api";
  import { relativeTime } from "../format";
  import { type Held, Refused } from "../held";
  import { expiresIn } from "../people/model";
  import { refusal } from "../people/refusal";
  import { queueHref } from "../queues/model";
  import { Closed, type QueueDecision, decideQueueRequest, queueRequestLine } from "./queue-request-api";

  type Props = {
    /** The request; null once it is gone. */
    request: QueueRequest | null;
    now?: Date;
    /** Called with the request as a decision here left it. */
    onDecided?: (r: QueueRequest) => void;
    /** Follows a click on the approved queue's link (the Queues page moves in place). */
    nav?: (e: MouseEvent) => void;
  };
  const p: Props = $props();
  // The request as a decision made here left it, until the request given says it is decided.
  let made = $state<QueueRequest | null>(null);
  let busy = $state(false);
  let err = $state<string | null>(null);
  // A decision held for the owner's confirmation, until it is made again.
  let waiting = $state<{ decision: QueueDecision; held: Held } | null>(null);

  const r = $derived(p.request && p.request.status !== "pending" ? p.request : made ?? p.request);
  const OUTCOME: Record<Exclude<QueueRequest["status"], "pending" | "approved">, string> = {
    denied: "Denied",
    cancelled: "Cancelled: the agent's access ended before you answered",
    expired: "Expired: nobody answered in time",
  };
  const SEES = { own: "Reporters see only their own reports", all: "Reporters see every report in the queue" } as const;

  async function decide(d: QueueDecision) {
    if (!r || busy) return;
    busy = true; err = null;
    try {
      const res = await decideQueueRequest(r.id, d, waiting?.decision === d ? waiting.held.id : undefined);
      waiting = null;
      made = res.request;
      p.onDecided?.(res.request);
    } catch (x) {
      if (x instanceof Closed) {
        waiting = null;
        made = x.request;
        p.onDecided?.(x.request);
        return;
      }
      // Only a held decision waits; a confirmation declined, expired or used is gone, and the next press asks afresh.
      waiting = x instanceof Refused && x.held ? { decision: d, held: x.held } : null;
      if (!waiting) err = refusal(x);
    } finally { busy = false; }
  }
</script>

<article class="qrcard" data-queue-request={r?.id ?? ""} data-status={r?.status ?? "gone"} tabindex="-1" aria-label="Request for a report queue">
  {#if r}
    <header class="chead">
      <span>{queueRequestLine(r)}</span>
      <time datetime={r.created_at}>{relativeTime(r.created_at, p.now ?? new Date())}</time>
    </header>
    {#if r.origins.length}
      <ul class="origins" aria-label="Origins it would accept reports from">{#each r.origins as o (o)}<li>{o}</li>{/each}</ul>
    {:else}
      <p class="meta">No origins: no page could file reports into it yet.</p>
    {/if}
    <p class="meta sees">{SEES[r.reporters_see]}</p>
    {#if r.note}<p class="meta note">“{r.note}”</p>{/if}
    {#if r.status === "pending"}
      {@const left = expiresIn(r.expires_at, p.now ?? new Date())}
      <p class="meta expires">Expires {left === "now" ? "now" : `in ${left}`}</p>
      {#if waiting}<p class="meta held" role="status">{waiting.held.message}, then answer again.</p>{/if}
      <div class="acts">
        <button type="button" class="primary" disabled={busy} onclick={() => decide("approve")}>Approve</button>
        <button type="button" class="ghost" disabled={busy} onclick={() => decide("deny")}>Deny</button>
      </div>
      <p class="meta what">Approve makes the queue with you as its admin and gives the agent one service grant for it, once.</p>
    {:else if r.status === "approved"}
      <p class="outcome" role="status">Approved: the queue is made{#if r.queue_id}{". "}<a class="queue" href={queueHref(r.queue_id)} onclick={p.nav}>Open {r.name}</a>{/if}</p>
    {:else}
      <p class="outcome" role="status">{OUTCOME[r.status]}</p>
    {/if}
  {:else}
    <p class="outcome" role="status">This request is gone</p>
  {/if}
  {#if err}<p class="err" role="alert">{err}</p>{/if}
</article>

<style>
  :global {
    .qrcard { position: relative; display: grid; gap: 6px; background: var(--card); border: 1px solid var(--border); border-left: 3px solid var(--agent); border-radius: var(--radius); padding: 11px 12px 10px 13px; min-width: 0; font-size: 13.5px; }
    .qrcard:focus { outline: none; }
    .qrcard:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; }
    .qrcard:not([data-status="pending"]) { border-left-color: var(--border-strong); }
    .qrcard p { margin: 0; }
    .qrcard .chead { display: flex; flex-wrap: wrap; align-items: baseline; gap: 2px 8px; min-width: 0; font-weight: 600; overflow-wrap: anywhere; }
    .qrcard .chead > span { min-width: 0; }
    .qrcard time { margin-left: auto; font: 400 11.5px var(--font); color: var(--muted); white-space: nowrap; font-variant-numeric: tabular-nums; }
    .qrcard .meta { color: var(--muted); font-size: 12.5px; overflow-wrap: anywhere; }
    .qrcard .origins { margin: 0; padding: 0; list-style: none; display: flex; flex-wrap: wrap; gap: 4px 6px; }
    .qrcard .origins li { padding: 1px 8px; border-radius: var(--radius-sm); background: var(--hover); font: 12.5px var(--mono); overflow-wrap: anywhere; min-width: 0; }
    .qrcard a { color: var(--fg); text-decoration: underline; text-decoration-color: var(--border-strong); text-underline-offset: 2px; overflow-wrap: anywhere; }
    .qrcard .acts { display: flex; flex-wrap: wrap; gap: 6px; margin-top: 4px; }
    .qrcard .acts button { max-width: 100%; white-space: normal; overflow-wrap: anywhere; text-align: start; }
    .qrcard .outcome { font-weight: 600; font-size: 13px; }
    .qrcard .err { color: var(--danger); font-size: 13px; }
    @media (max-width: 520px) { .qrcard .acts button { flex: 1 1 100%; justify-content: flex-start; } }
  }
</style>
