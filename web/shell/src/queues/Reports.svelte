<svelte:options css="injected" />

<script lang="ts">
  // The Reports page (`/reports`): every report across the queues the
  // viewer belongs to, the latest filed first. Each row names its queue,
  // its state, its note's first line and how many replies followed, and
  // opens the report's view (`/queues/<qid>/<tid>`). It filters by state
  // (the queue view's tabs) and by queue, both kept in the address
  // (`?state=`, `?queue=`). Each queue's listing is fetched a page at a
  // time and merged (`newestAcross`): a row shows only once no newer one can
  // come from a queue not yet fetched that far, and Load more fetches the
  // next page of every queue that has one. The list is fetched again when
  // the stream comes back.
  import { onMount } from "svelte";
  import type { EventStream } from "../stream";
  import { relativeTime } from "../format";
  import * as api from "./api";
  import { type Feed, STATE_LABEL, TABS, firstLine, newestAcross, refusal, replyCount, reportHref } from "./model";

  const p: { stream?: EventStream } = $props();
  const params = new URLSearchParams(location.search);
  let stateKey = $state(TABS.find(t => t.key === params.get("state"))?.key ?? "all");
  let queue = $state(params.get("queue") ?? "");
  let queues = $state<api.Queue[] | null>(null);
  let feeds = $state<Record<string, Feed>>({});
  let loading = $state(true);
  let more = $state(false);
  let err = $state<string | null>(null);
  let notice = $state("");
  // Each fetch's turn: an answer for an earlier filter is dropped.
  let turn = 0;

  const names = $derived(Object.fromEntries((queues ?? []).map(q => [q.id, q.name])));
  const shown = $derived(queue && !names[queue] && queues ? [] : (queues ?? []).filter(q => !queue || q.id === queue));
  const rows = $derived(newestAcross(shown.map(q => feeds[q.id]).filter(Boolean)));
  const hasMore = $derived(shown.some(q => feeds[q.id]?.next));
  const filter = () => TABS.find(t => t.key === stateKey)?.state ?? null;

  async function fetchPage(qid: string, cursor: string | null): Promise<Feed> {
    const page = await api.reportPage(qid, cursor, { state: filter(), order: "newest" });
    return { reports: [...(cursor ? feeds[qid]?.reports ?? [] : []), ...page.reports], next: page.next };
  }

  async function load() {
    const mine = ++turn;
    loading = true;
    err = null;
    try {
      if (!queues) queues = (await api.listQueues()).queues;
      const got = await Promise.all(shown.map(async q => [q.id, await fetchPage(q.id, null)] as const));
      if (mine !== turn) return;
      feeds = Object.fromEntries(got);
      notice = `${rows.length}${hasMore ? "+" : ""} ${rows.length === 1 && !hasMore ? "report" : "reports"}.`;
    } catch (x) {
      if (mine === turn) err = `Could not load the reports: ${refusal(x)}`;
    } finally {
      if (mine === turn) loading = false;
    }
  }

  async function loadMore() {
    const mine = turn;
    more = true;
    try {
      const got = await Promise.all(shown.filter(q => feeds[q.id]?.next).map(async q => [q.id, await fetchPage(q.id, feeds[q.id].next)] as const));
      if (mine !== turn) return;
      feeds = { ...feeds, ...Object.fromEntries(got) };
    } catch (x) {
      if (mine === turn) err = `Could not load more reports: ${refusal(x)}`;
    } finally {
      more = false;
    }
  }

  function setFilter() {
    const u = new URL(location.href);
    if (stateKey === "all") u.searchParams.delete("state"); else u.searchParams.set("state", stateKey);
    if (!queue) u.searchParams.delete("queue"); else u.searchParams.set("queue", queue);
    history.replaceState(history.state, "", `${u.pathname}${u.search}${u.hash}`);
    void load();
  }

  onMount(() => {
    document.title = "Reports · Clax";
    void load();
    return p.stream?.watch(["gallery"], e => { if (e.type === "resync") { queues = null; void load(); } });
  });
</script>

<section class="reports" aria-labelledby="reports-h">
  <header class="rh">
    <h2 id="reports-h">Reports</h2>
    <span class="sp"></span>
    <a class="toq" href="/queues">Queues</a>
  </header>
  <p class="rsub">Every report filed into your queues, the latest first.</p>
  <form class="filters" aria-label="Filter reports" onsubmit={e => e.preventDefault()}>
    <label>
      <span>State</span>
      <select bind:value={stateKey} onchange={setFilter}>
        {#each TABS as t (t.key)}<option value={t.key}>{t.label}</option>{/each}
      </select>
    </label>
    <label>
      <span>Queue</span>
      <select bind:value={queue} onchange={setFilter}>
        <option value="">All queues</option>
        {#each queues ?? [] as q (q.id)}<option value={q.id}>{q.name}</option>{/each}
      </select>
    </label>
  </form>
  <p class="sr" role="status">{notice}</p>
  <div class="body" aria-busy={loading}>
    {#if err}<p class="rerr" role="alert">{err}</p>{/if}
    {#if queue && queues && !names[queue]}
      <p class="rnone">No queue {queue} is yours to triage.</p>
    {:else if rows.length}
      <ul class="rlist" aria-label="Reports, the latest first">
        {#each rows as r (r.id)}
          {@const n = replyCount(r)}
          {@const line = firstLine(r)}
          <li class="rrow" data-id={r.id}>
            <a class="rlink" href={reportHref(r.queue_id, r.id)}
              aria-label={`${line || "No note"}. ${names[r.queue_id] ?? "Queue"}, ${STATE_LABEL[r.state] ?? r.state}, ${n} ${n === 1 ? "reply" : "replies"}, filed ${relativeTime(r.created_at)}`}>
              <span class="line">{line || "(no note)"}</span>
              <span class="meta">
                <span class="qname">{names[r.queue_id] ?? r.queue_id}</span>
                <time datetime={r.created_at} title={r.created_at}>{relativeTime(r.created_at)}</time>
                <span class="nrep">{n} {n === 1 ? "reply" : "replies"}</span>
              </span>
              <span class={["state", r.state]}>{STATE_LABEL[r.state] ?? r.state}</span>
            </a>
          </li>
        {/each}
      </ul>
      {#if hasMore}<p class="more"><button type="button" class="ghost" disabled={more} onclick={() => void loadMore()}>{more ? "Loading…" : "Load more"}</button></p>{/if}
    {:else if loading}
      <p class="rnone">Loading…</p>
    {:else if !err}
      <p class="rnone">{queues?.length ? "No reports match." : "You are in no queues yet."}</p>
    {/if}
  </div>
</section>

<style>
  .reports { max-width: 880px; margin: 0 auto; min-width: 0; }
  .rh { display: flex; align-items: center; gap: 8px 12px; padding-bottom: 12px; border-bottom: 1px solid var(--border); }
  .rh h2 { margin: 0; font-size: 22px; line-height: 1.2; }
  .sp { flex: 1; }
  .toq { font-size: 13px; }
  .rsub { margin: 10px 0 0; color: var(--muted); font-size: 13px; }
  .filters { display: flex; flex-wrap: wrap; gap: 8px 16px; margin: 12px 0; }
  .filters label { display: inline-flex; align-items: center; gap: 6px; font-size: 13px; color: var(--muted); }
  .filters select { min-height: 32px; max-width: 16em; }
  .body { min-height: 140px; }
  .rerr { margin: 12px 0 0; color: var(--danger); font-size: 13px; }
  .rlist { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 8px; }
  .rlink { display: grid; grid-template-columns: minmax(0, 1fr) auto; grid-template-areas: "line state" "meta state"; gap: 3px 12px; align-items: center; padding: 10px 12px; background: var(--card); border: 1px solid var(--border); border-radius: var(--radius); color: inherit; text-decoration: none; }
  .rlink:hover { border-color: var(--border-hover); }
  .line { grid-area: line; font-size: 14px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .meta { grid-area: meta; display: flex; flex-wrap: wrap; gap: 2px 10px; color: var(--muted); font-size: 12.5px; min-width: 0; }
  .qname { font-weight: 600; overflow-wrap: anywhere; }
  .state { grid-area: state; padding: 1px 8px; border-radius: 999px; font-size: 12px; white-space: nowrap; border: 1px solid var(--tag-gray-bd); background: var(--tag-gray-bg); color: var(--tag-gray-fg); }
  .state.open { border-color: var(--tag-blue-bd); background: var(--tag-blue-bg); color: var(--tag-blue-fg); }
  .state.claimed { border-color: var(--tag-purple-bd); background: var(--tag-purple-bg); color: var(--tag-purple-fg); }
  .state.fix_proposed { border-color: var(--tag-orange-bd); background: var(--tag-orange-bg); color: var(--tag-orange-fg); }
  .state.verified { border-color: var(--tag-green-bd); background: var(--tag-green-bg); color: var(--tag-green-fg); }
  .more { text-align: center; margin: 12px 0; }
  .rnone { color: var(--muted); text-align: center; padding: 48px 0; margin: 0; }
  .sr { position: absolute; width: 1px; height: 1px; margin: -1px; padding: 0; overflow: hidden; clip: rect(0 0 0 0); white-space: nowrap; border: 0; }
  @media (pointer: coarse) { .filters select, .rlink { min-height: 44px; } }
</style>
