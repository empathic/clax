<svelte:options css="injected" />

<script lang="ts">
  // The Queues page (`/queues`, spec 2026-10-27-clax-sdk-design §5.3, §6.2):
  // every report queue the viewer is a member of, with its origins, what
  // its reporters see, the viewer's role and its reports counted by state.
  // New queue makes one (the maker is its admin). A queue's admin edits it
  // in place, deletes it after confirming, and opens Manage for its members
  // and service grants (`QueueAdmin`). A refused change says why until the
  // next one. The list is fetched again after each change, when the stream
  // comes back, and at each `queues` ping (a report filed, claimed,
  // released, replied to or resolved, by anyone's agent); the counts change
  // in place, and what rose is said in a polite status line ("1 report
  // claimed in Bugs."), no focus moved. A claim's lease can lapse with no
  // ping: the list is fetched again when the soonest one shown would. A
  // queue's name and its Reports action open its reports, and each count
  // that state's tab (`?state=`) (`/queues/<qid>`, `QueueView`), and a
  // report its own view (`/queues/<qid>/<tid>`, `ReportView`; spec
  // 2026-10-29-feedback-loop-design §8): the page moves between them in
  // place, through the browser's history.
  import { onMount, tick } from "svelte";
  import { after, wall } from "../clock";
  import type { EventStream } from "../stream";
  import * as api from "./api";
  import { type Counts, STATES, STATE_LABEL, changeText, countReports, countsText, queueHref, queuesRoute, refusal } from "./model";
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
  // What others' changes did, said politely; apart from `status`, which says what the viewer did.
  let heard = $state("");
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
    if (route.view === "list") void load();
  }

  // One fetch at a time: a change heard during one fetches once more after it.
  let running: Promise<void> | null = null;
  let again = false;
  let sayAgain = false;
  let lapse: (() => void) | undefined;
  // The page unmounted: a fetch still in flight sets nothing and plants no timer.
  let gone = false;

  /** Fetches the queues and their counts; `say`: announce what rose. */
  function load(say = false): Promise<void> {
    if (gone) return Promise.resolve();
    if (running) {
      again = true;
      sayAgain ||= say;
      return running;
    }
    running = (async () => {
      try {
        do {
          again = false;
          await fetchAll(say);
          say = sayAgain;
          sayAgain = false;
        } while (again);
      } finally {
        running = null;
      }
    })();
    return running;
  }

  async function fetchAll(say: boolean) {
    let list: api.Queue[];
    try {
      list = (await api.listQueues()).queues;
      if (gone) return;
      loadError = null;
    } catch (x) {
      if (gone) return;
      // A refetch that fails keeps what is shown.
      if (!queues) loadError = `Could not load the queues: ${refusal(x)}`;
      return;
    }
    // The first time, the list shows while its reports are counted.
    queues ??= list;
    const got = await Promise.all(list.map(q => countReports(c => api.reportPage(q.id, c)).catch(() => "failed" as const)));
    if (gone) return;
    const said: string[] = [];
    list.forEach((q, i) => {
      const was = counts[q.id];
      const c = got[i];
      if (say && was && was !== "failed" && c !== "failed") {
        const t = changeText(q.name, was, c);
        if (t) said.push(t);
      }
    });
    queues = list;
    counts = Object.fromEntries(list.map((q, i) => [q.id, got[i]]));
    if (said.length) void announce(said.join(" "));
    lapse?.();
    const soonest = got.reduce<number | null>((m, c) => (c === "failed" || c.lapses === null || (m !== null && m <= c.lapses) ? m : c.lapses), null);
    // A second after it lapses, and never sooner than 5 s: a clock behind
    // the daemon's would otherwise fetch again at once, over and over.
    lapse = soonest === null ? undefined : after(Math.max(soonest - wall() + 1000, 5000), () => void load(true), "queues.lapse");
  }

  /** Says `text` in the status line, again if it is the same as before. */
  async function announce(text: string) {
    heard = "";
    await tick();
    heard = text;
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
    const off = p.stream?.watch(["queues"], e => {
      if (e.type === "queues") void load(true);
      else if (e.type === "ready" || e.type === "resync") void load();
    });
    return () => { gone = true; removeEventListener("popstate", back); off?.(); lapse?.(); };
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
  <p class="qheard" role="status">{heard}</p>
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
              <span class="acts">
                <a class="act" href={queueHref(q.id)} onclick={nav} aria-label={`Reports in ${q.name}`}>Reports</a>
                {#if q.role === "admin"}
                  <button type="button" class="ghost" aria-expanded={!!managing[q.id]} onclick={() => { managing[q.id] = !managing[q.id]; }}>Manage</button>
                  <button type="button" class="ghost" aria-label={`Edit queue ${q.name}`} onclick={() => open(q.id)}>Edit</button>
                  <button type="button" class="ghost danger" aria-label={`Delete queue ${q.name}`} onclick={() => void remove(q)}>Delete</button>
                {/if}
              </span>
            </header>
            {#if c && c !== "failed"}
              <ul class="counts" aria-label={`Reports in ${q.name} by state`}>
                {#each STATES as s, i (s)}
                  {@const n = `${c[s]}${c.more ? "+" : ""}`}
                  <li>{#if i}<span class="dot" aria-hidden="true">{" · "}</span>{/if}<a href={queueHref(q.id, s)} onclick={nav} aria-label={`${n} ${STATE_LABEL[s].toLowerCase()} ${c[s] === 1 && !c.more ? "report" : "reports"} in ${q.name}`}>{n} {STATE_LABEL[s].toLowerCase()}</a></li>
                {/each}
              </ul>
            {:else}
              <p class="counts">{c === "failed" ? "Could not count its reports." : "Counting reports…"}</p>
            {/if}
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
  /* Never display: none, even empty: a live region must be in the tree before it speaks. */
  .qheard { margin: 0 0 8px; color: var(--muted); font-size: 13px; }
  .qlist { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 12px; }
  .qcard { background: var(--card); border: 1px solid var(--border); border-radius: var(--radius); padding: 14px 16px; min-width: 0; }
  .qcard > :global(.qform) { margin: 0; padding: 0; border: 0; background: none; }
  .qrow { display: flex; flex-wrap: wrap; align-items: center; gap: 4px 10px; }
  .qrow h3 { margin: 0; font-size: 16px; overflow-wrap: anywhere; min-width: 0; }
  .qrow h3 a { color: inherit; text-decoration: none; }
  .qrow h3 a:hover, .qrow h3 a:focus-visible { text-decoration: underline; }
  .role { padding: 1px 8px; border: 1px solid var(--border); border-radius: 999px; color: var(--muted); font-size: 12px; }
  .acts { display: flex; flex-wrap: wrap; gap: 2px; }
  .acts button, .acts .act { min-height: 28px; padding: 2px 8px; color: var(--muted); }
  .acts .act { display: inline-flex; align-items: center; border-radius: var(--radius-sm); font-size: 13px; text-decoration: none; }
  .acts .act:hover, .acts .act:focus-visible { color: var(--fg); background: var(--hover); }
  .acts .danger:hover, .acts .danger:focus-visible { color: var(--danger); }
  .counts { margin: 8px 0 0; padding: 0; list-style: none; display: flex; flex-wrap: wrap; font-size: 13.5px; line-height: 1.5; font-variant-numeric: tabular-nums; }
  .counts .dot { white-space: pre; color: var(--muted); }
  .counts a { color: inherit; text-decoration: underline; text-decoration-color: var(--border); text-underline-offset: 2px; }
  .counts a:hover, .counts a:focus-visible { text-decoration-color: currentColor; }
  .meta { margin: 4px 0 0; color: var(--muted); font-size: 12.5px; overflow-wrap: anywhere; }
  .key { font: 12px var(--mono); }
  .origins { margin: 6px 0 0; padding: 0; list-style: none; display: flex; flex-wrap: wrap; gap: 4px 6px; }
  .origins li { padding: 1px 8px; border-radius: var(--radius-sm); background: var(--hover); font: 12.5px var(--mono); overflow-wrap: anywhere; min-width: 0; }
  .qnone { color: var(--muted); text-align: center; padding: 48px 0; }
  @media (max-width: 700px) {
    .qcard { padding: 12px; }
    .acts { width: 100%; }
  }
  @media (pointer: coarse) { .acts button, .acts .act { min-height: 44px; } }
</style>
