<svelte:options css="injected" />

<script lang="ts">
  // The Queues page (`/queues`, spec 2026-10-27-clax-sdk-design §5.3, §6.2):
  // every report queue the viewer is a member of, with its origins, what
  // its reporters see, the viewer's role and its reports counted by state.
  // New queue makes one (the maker is its admin). A queue's admin edits it
  // in place, deletes it after confirming, and opens Manage for its members
  // and service grants (`QueueAdmin`). A refused change says why until the
  // next one. The list is fetched again after each change and when the
  // stream comes back. A queue's name opens its reports (`/queues/<qid>`,
  // `QueueView`), and a report its own view (`/queues/<qid>/<tid>`,
  // `ReportView`; spec 2026-10-29-feedback-loop-design §8): the page moves
  // between them in place, through the browser's history.
  import { onMount } from "svelte";
  import type { EventStream } from "../stream";
  import * as api from "./api";
  import { type Counts, countReports, countsText, queueHref, queuesRoute, refusal } from "./model";
  import QueueAdmin from "./QueueAdmin.svelte";
  import QueueForm from "./QueueForm.svelte";
  import QueueView from "./QueueView.svelte";
  import ReportView from "./ReportView.svelte";

  const p: { stream?: EventStream } = $props();
  let queues = $state<api.Queue[] | null>(null);
  let counts = $state<Record<string, Counts | "failed">>({});
  let loadError = $state<string | null>(null);
  let error = $state<string | null>(null);
  let status = $state("");
  // The form open: a queue's ID, "new", or none; and the queues whose Manage is open.
  let editing = $state<string | null>(null);
  let managing = $state<Record<string, boolean>>({});
  let path = $state(location.pathname);
  const route = $derived(queuesRoute(path));
  // The queue a sub-view is of: `null` while the list loads, `undefined` when the viewer has none of that ID.
  const current = $derived(route.view === "list" ? null : queues ? queues.find(q => q.id === route.qid) : null);

  /** Follows a plain click on a link to another of the page's views in place. */
  function nav(e: MouseEvent) {
    const a = e.currentTarget as HTMLAnchorElement;
    if (e.defaultPrevented || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey || a.target) return;
    e.preventDefault();
    const href = a.getAttribute("href")!;
    if (href !== `${location.pathname}${location.search}`) history.pushState(null, "", href);
    moved();
    window.scrollTo?.(0, 0);
  }
  /** The address changed: show its view, the list counted afresh. */
  function moved() {
    path = location.pathname;
    if (route.view === "list") count();
  }

  async function load() {
    try {
      queues = (await api.listQueues()).queues;
      loadError = null;
    } catch (x) {
      loadError = `Could not load the queues: ${refusal(x)}`;
      return;
    }
    if (route.view === "list") count();
  }
  /** Counts each queue's reports by state, for the list. */
  function count() {
    for (const q of queues ?? []) void countReports(c => api.reportPage(q.id, c)).then(c => { counts[q.id] = c; }, () => { counts[q.id] = "failed"; });
  }

  async function save(f: api.QueueFields, q?: api.Queue): Promise<string | null> {
    try {
      if (q) await api.updateQueue(q.id, f);
      else await api.createQueue(f);
    } catch (x) {
      return refusal(x);
    }
    status = q ? `Saved ${f.name}.` : `Made ${f.name}.`;
    editing = null;
    await load();
    return null;
  }
  async function remove(q: api.Queue) {
    if (!confirm(`Delete the queue "${q.name}"? Its service grants and reporter tokens stop working.`)) return;
    error = null;
    try {
      await api.deleteQueue(q.id);
      status = `Deleted ${q.name}.`;
    } catch (x) {
      status = "";
      error = `Could not delete ${q.name}: ${refusal(x)}`;
    }
    await load();
  }
  const open = (what: string) => { editing = what; status = ""; error = null; };
  const seeText = (q: api.Queue) => (q.reporters_see === "all" ? "Reporters see every report" : "Reporters see their own reports");

  $effect(() => {
    if (route.view === "list") document.title = "Queues · Clax";
    else if (route.view === "queue") document.title = `${current?.name ?? "Queue"} · Queues · Clax`;
  });

  onMount(() => {
    void load();
    const back = () => moved();
    addEventListener("popstate", back);
    const off = p.stream?.watch(["gallery"], e => { if (e.type === "ready" || e.type === "resync") void load(); });
    return () => { removeEventListener("popstate", back); off?.(); };
  });
</script>

{#if route.view === "queue"}
  {#key route.qid}<QueueView qid={route.qid} queue={current} {nav} stream={p.stream} />{/key}
{:else if route.view === "report"}
  {#key `${route.qid}/${route.tid}`}<ReportView qid={route.qid} tid={route.tid} queue={current} {nav} stream={p.stream} />{/key}
{:else}
<section class="queues" aria-labelledby="queues-h">
  <header class="qh">
    <h2 id="queues-h">Queues</h2>
    {#if queues}<span class="qc">{queues.length}</span>{/if}
    <span class="sp"></span>
    {#if editing !== "new"}<button type="button" class="primary" onclick={() => open("new")}>New queue</button>{/if}
  </header>
  <p class="qsub">A queue collects reports filed from your sites' pages, for people and their agents to triage.</p>
  {#if loadError || error}<p class="qerr" role="alert">{error ?? loadError}</p>{/if}
  <p class="qstat" role="status">{status}</p>
  {#if editing === "new"}<QueueForm onSave={f => save(f)} onCancel={() => { editing = null; }} />{/if}
  {#if queues?.length}
    <ul class="qlist">
      {#each queues as q (q.id)}
        {@const c = counts[q.id]}
        <li class="qcard" data-id={q.id}>
          {#if editing === q.id}
            <QueueForm queue={q} onSave={f => save(f, q)} onCancel={() => { editing = null; }} />
          {:else}
            <header class="qrow">
              <h3><a href={queueHref(q.id)} onclick={nav}>{q.name}</a></h3>
              <span class="role">{q.role === "admin" ? "Admin" : "Triage"}</span>
              <span class="sp"></span>
              {#if q.role === "admin"}
                <span class="acts">
                  <button type="button" class="ghost" aria-expanded={!!managing[q.id]} onclick={() => { managing[q.id] = !managing[q.id]; }}>Manage</button>
                  <button type="button" class="ghost" aria-label={`Edit queue ${q.name}`} onclick={() => open(q.id)}>Edit</button>
                  <button type="button" class="ghost danger" aria-label={`Delete queue ${q.name}`} onclick={() => void remove(q)}>Delete</button>
                </span>
              {/if}
            </header>
            <p class="counts">{c === "failed" ? "Could not count its reports." : c ? countsText(c) : "Counting reports…"}</p>
            <p class="meta">{seeText(q)} · <span class="key">{q.id}</span></p>
            {#if q.origins.length}
              <ul class="origins" aria-label={`Origins of ${q.name}`}>{#each q.origins as o (o)}<li>{o}</li>{/each}</ul>
            {:else}
              <p class="meta">No origins yet: no page can file reports into it.</p>
            {/if}
            {#if managing[q.id] && q.role === "admin"}<QueueAdmin queue={q} />{/if}
          {/if}
        </li>
      {/each}
    </ul>
  {:else if queues}
    <p class="qnone">There are no queues yet. Make one with New queue.</p>
  {:else if !loadError}
    <p class="qnone">Loading…</p>
  {/if}
</section>
{/if}

<style>
  .queues { max-width: 880px; margin: 0 auto; min-width: 0; }
  .qh { display: flex; align-items: center; gap: 8px 12px; padding-bottom: 12px; border-bottom: 1px solid var(--border); }
  .qh h2 { margin: 0; font-size: 22px; line-height: 1.2; }
  .qc { color: var(--muted); font: 400 13px var(--mono); }
  .sp { flex: 1; }
  .qsub { margin: 10px 0 0; color: var(--muted); font-size: 13px; }
  .qerr { margin: 12px 0 0; color: var(--danger); font-size: 13px; }
  .qstat { margin: 8px 0; min-height: 1.5em; color: var(--muted); font-size: 13px; }
  .qlist { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 12px; }
  .qcard { background: var(--card); border: 1px solid var(--border); border-radius: var(--radius); padding: 14px 16px; min-width: 0; }
  .qcard > :global(.qform) { margin: 0; padding: 0; border: 0; background: none; }
  .qrow { display: flex; flex-wrap: wrap; align-items: center; gap: 4px 10px; }
  .qrow h3 { margin: 0; font-size: 16px; overflow-wrap: anywhere; min-width: 0; }
  .qrow h3 a { color: inherit; text-decoration: none; }
  .qrow h3 a:hover, .qrow h3 a:focus-visible { text-decoration: underline; }
  .role { padding: 1px 8px; border: 1px solid var(--border); border-radius: 999px; color: var(--muted); font-size: 12px; }
  .acts { display: flex; flex-wrap: wrap; gap: 2px; }
  .acts button { min-height: 28px; padding: 2px 8px; color: var(--muted); }
  .acts .danger:hover, .acts .danger:focus-visible { color: var(--danger); }
  .counts { margin: 8px 0 0; font-size: 13.5px; font-variant-numeric: tabular-nums; }
  .meta { margin: 4px 0 0; color: var(--muted); font-size: 12.5px; overflow-wrap: anywhere; }
  .key { font: 12px var(--mono); }
  .origins { margin: 6px 0 0; padding: 0; list-style: none; display: flex; flex-wrap: wrap; gap: 4px 6px; }
  .origins li { padding: 1px 8px; border-radius: var(--radius-sm); background: var(--hover); font: 12.5px var(--mono); overflow-wrap: anywhere; min-width: 0; }
  .qnone { color: var(--muted); text-align: center; padding: 48px 0; }
  @media (max-width: 700px) {
    .qcard { padding: 12px; }
    .acts { width: 100%; }
  }
  @media (pointer: coarse) { .acts button { min-height: 44px; } }
</style>
