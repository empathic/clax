<svelte:options css="injected" />

<script lang="ts">
  // A queue's own page (`/queues/<qid>`, its title the queue's name) and a
  // report's own page (`/queues/<qid>/<tid>`): the shared triage views
  // (web/overlay/src/triage) over the browser's own session, live on the
  // page's stream. /queues stays the index of queues.
  import { onMount } from "svelte";
  import QueueView from "../../../overlay/src/triage/QueueView.svelte";
  import ReportView from "../../../overlay/src/triage/ReportView.svelte";
  import { type Queue, failureOf, queuePath, reportPath, routeOf } from "../../../overlay/src/triage/triage";
  import type { EventStream } from "../stream";
  import { shellTriage } from "./triage-api";

  const p: { stream?: EventStream } = $props();
  const route = routeOf(location.pathname);
  // The page's stream is the one it was mounted with (gallery-working's `page`).
  const api = shellTriage((() => p.stream)());
  let queue = $state<Queue | null | undefined>(undefined);
  let error = $state<string | null>(null);
  onMount(() => {
    if (!route) return;
    void api.queues().then(qs => {
      queue = qs.find(q => q.id === route.qid) ?? null;
      if (queue && !route.tid) document.title = `${queue.name} · Clax`;
    }, x => { error = failureOf(x); });
  });
</script>

<section class="qr">
  <nav class="crumbs" aria-label="Queues">
    <a href="/queues">Queues</a>{#if queue && route?.tid} › <a href={queuePath(queue.id)}>{queue.name}</a>{/if}
  </nav>
  {#if !route}
    <p class="hint">No such page.</p>
  {:else if error}
    <p class="hint" role="alert">Could not read the queues: {error}</p>
  {:else if queue === null}
    <p class="hint">No queue of yours has this ID. <a href="/queues">See your queues</a>.</p>
  {:else if queue && route.tid}
    <ReportView {api} qid={queue.id} tid={route.tid} back={{ label: queue.name, href: queuePath(queue.id) }} />
  {:else if queue}
    <QueueView {api} {queue} href={r => reportPath(queue!.id, r.id)} />
  {:else}
    <p class="hint">Reading the queue…</p>
  {/if}
</section>

<style>
  .qr { display: grid; gap: 12px; max-width: 52rem; margin: 0 auto; padding: 16px var(--gutter, 16px) 32px; }
  .crumbs { font-size: 13px; color: var(--muted); }
  .hint { color: var(--muted); margin: 0; }
</style>
