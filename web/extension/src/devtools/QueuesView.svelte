<script lang="ts">
  // The DevTools panel's Queues view (its BRIEF's Addenda 1 and 2): Clax's
  // own queue and report pages (web/overlay/src/triage) pinned to the
  // inspected site. The queue that takes reports from the inspected origin
  // is shown (a choice when several do; any other of the owner's when none
  // does), its reports on the inspected page first; a report opens in
  // place, with "Show on the page", which scrolls to its element in the
  // inspected page, outlines it and selects it in Elements. "Open in Clax"
  // opens the same pages in Clax. Live through the worker's `queue-ping`.
  import QueueView from "../../../overlay/src/triage/QueueView.svelte";
  import ReportView from "../../../overlay/src/triage/ReportView.svelte";
  import { type Queue, type Report, type TriageApi, failureOf, queuePath, queuesFor, reportPath, selectorOf } from "../../../overlay/src/triage/triage";

  let { api, origin, page, show, say }: {
    api: TriageApi & { base: string | null };
    /** The inspected page's origin and address (null off an http(s) page). */
    origin: string | null;
    page: string | null;
    /** Shows `selector`'s element in the inspected page; true when found. */
    show(selector: string): Promise<boolean>;
    say(text: string): void;
  } = $props();
  let queues = $state<Queue[] | null>(null);
  let error = $state<string | null>(null);
  let chosen = $state<string | null>(null);
  let open = $state<string | null>(null);
  $effect(() => {
    void api.queues().then(qs => { queues = qs; error = null; }, e => { error = failureOf(e); });
  });
  const mine = $derived(queues && origin ? queuesFor(queues, origin) : []);
  const queue = $derived(queues?.find(q => q.id === chosen) ?? mine[0] ?? null);
  // Another site's page: its reports start again from the list.
  $effect(() => { void origin; open = null; });
  const inClax = (path: string) => (api.base ? `${api.base}${path}` : null);
  async function showOn(r: Report): Promise<void> {
    const sel = selectorOf(r);
    if (!sel) return;
    say(await show(sel) ? "Shown on the page, and selected in Elements." : "Not found on this page: it may have changed since the report.");
  }
</script>

<div class="qs">
  {#if error}
    <p class="hint" role="alert">Could not read your queues: {error}</p>
  {:else if !queues}
    <p class="hint">Reading your queues…</p>
  {:else if !queues.length}
    <p class="hint">You have no queues. Make one in Clax’s Queues page{#if api.base}: <a href={`${api.base}/queues`} target="_blank" rel="noopener noreferrer">Open Queues in Clax</a>{/if}.</p>
  {:else}
    {#if mine.length !== 1 || (queue && !mine.includes(queue))}
      <fieldset class="pick">
        <legend>{mine.length ? `Queues for ${origin}` : `No queue of yours takes reports from ${origin ?? "this page"}. Your queues:`}</legend>
        {#each mine.length ? mine : queues as q (q.id)}
          <label><input type="radio" name="dt-queue" checked={queue?.id === q.id} onchange={() => { chosen = q.id; open = null; }} /> {q.name}</label>
        {/each}
      </fieldset>
    {/if}
    {#if queue}
      {#key queue.id}
        {#if open}
          <ReportView {api} qid={queue.id} tid={open} back={{ label: queue.name, href: inClax(queuePath(queue.id)) ?? "#", onclick: () => (open = null) }}
            onShow={showOn} openIn={inClax(reportPath(queue.id, open))} />
        {:else}
          <QueueView {api} {queue} {page} href={r => inClax(reportPath(queue.id, r.id)) ?? "#"} onOpen={r => (open = r.id)} openIn={inClax(queuePath(queue.id))} />
        {/if}
      {/key}
    {/if}
  {/if}
</div>

<style>
  .qs { padding: 10px var(--gutter) 18px; display: grid; gap: 10px; }
  .hint { color: var(--muted); margin: 0; }
  .pick { margin: 0; padding: 0; border: 0; display: grid; gap: 4px; }
  .pick legend { padding: 0; margin-bottom: 4px; font-weight: 500; }
  .pick label { display: flex; gap: 6px; align-items: center; }
</style>
