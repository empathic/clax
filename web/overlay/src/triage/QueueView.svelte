<!-- Its CSS travels with it: the shell loads it lazily, and its HTML carries none of it. -->
<svelte:options css="injected" />

<script lang="ts">
  // A queue's reports (triage.ts): the queue's name, a filter by state, a
  // status line kept in place whether or not it says anything, and the
  // reports newest first, those on `page` (the inspected page, in the
  // DevTools panel) apart and first. Each report is one link to its own
  // page; `onOpen`, when given, opens it in place instead (a modified click
  // still follows the link). Live: each change the host hears is read once
  // (one read at a time), the list changes in place and what changed is said
  // politely, focus left where it is. Everything shown is text.
  import { onMount } from "svelte";
  import { FILTERS, type Queue, type Report, type ReportState, type TriageApi, STATE_TEXT, aboutOf, arrange, changes, claimText, failureOf, readAll, repliesOf, serial, titleOf } from "./triage";

  type Props = {
    api: TriageApi;
    queue: Queue;
    /** The page whose reports come first; null for none. */
    page?: string | null;
    href(r: Report): string;
    onOpen?(r: Report): void;
    /** A link to this queue's own page in Clax (the DevTools panel's "Open in Clax"). */
    openIn?: string | null;
  };
  let { api, queue, page = null, href, onOpen, openIn = null }: Props = $props();
  let reports = $state<Report[] | null>(null);
  let more = $state(false);
  let error = $state<string | null>(null);
  let heard = $state("");
  let filter = $state<ReportState | "all">("all");
  let gone = false;

  const load = serial(async () => {
    try {
      const r = await readAll(api, queue.id);
      if (gone) return;
      const said = reports ? changes(reports, r.reports) : "";
      reports = r.reports;
      more = r.more;
      error = null;
      if (said) heard = said;
    } catch (e) {
      if (gone) return;
      // A read that fails keeps what is shown, and says so.
      error = `Could not read the reports: ${failureOf(e)}`;
    }
  });
  onMount(() => {
    void load();
    const off = api.watch(() => void load());
    return () => { gone = true; off(); };
  });
  const parts = $derived(reports ? arrange(reports, filter, page) : { here: [], rest: [] });
  const count = (s: ReportState | "all") => (reports ?? []).filter(r => s === "all" || r.state === s).length;
  function open(e: MouseEvent, r: Report): void {
    if (!onOpen || e.metaKey || e.ctrlKey || e.shiftKey || e.button !== 0) return;
    e.preventDefault();
    onOpen(r);
  }
</script>

{#snippet card(r: Report)}
  {@const claim = claimText(r)}
  {@const replies = repliesOf(r).length}
  <li>
    <a class="card" href={href(r)} onclick={e => open(e, r)}>
      <span class="t">{titleOf(r)}</span>
      <span class="m"><span class="st st-{r.state}">{STATE_TEXT[r.state]}</span> · {aboutOf(r)} · {replies} {replies === 1 ? "reply" : "replies"}{#if r.has_clip} · screenshot{/if}</span>
      <span class="m">{r.artifact.title || r.artifact.url} · <time datetime={r.created_at}>{new Date(r.created_at).toLocaleString()}</time>{#if claim} · {claim}{/if}</span>
    </a>
  </li>
{/snippet}

<section class="qv" aria-labelledby="qv-{queue.id}">
  <header class="qvh">
    <h2 id="qv-{queue.id}">{queue.name}</h2>
    {#if openIn}<a href={openIn} target="_blank" rel="noopener noreferrer">Open in Clax</a>{/if}
  </header>
  <p class="sub">{queue.origins.join(", ") || "No sites yet"} · you are {queue.role === "admin" ? "an admin" : "on triage"}</p>
  <div class="seg" role="group" aria-label="Show reports">
    {#each FILTERS as f (f.value)}
      <button type="button" aria-pressed={filter === f.value} onclick={() => (filter = f.value)}>{f.label}{#if reports} <span class="n">{count(f.value)}</span>{/if}</button>
    {/each}
  </div>
  <!-- The status line: reserved, so a change said here moves nothing below. -->
  <p class="heard" role="status" aria-live="polite">{error ?? heard}</p>
  {#if !reports}
    <p class="hint">{error ? "" : "Reading the reports…"}</p>
  {:else if !parts.here.length && !parts.rest.length}
    <p class="hint">{filter === "all" ? "No reports in this queue yet." : `No ${STATE_TEXT[filter as ReportState].toLowerCase()} reports.`}</p>
  {:else}
    {#if page}
      <h3>On this page</h3>
      {#if parts.here.length}<ul>{#each parts.here as r (r.id)}{@render card(r)}{/each}</ul>{:else}<p class="hint">No reports on this page{filter === "all" ? "" : " in this state"}.</p>{/if}
      {#if parts.rest.length}<h3>Elsewhere</h3>{/if}
    {/if}
    {#if parts.rest.length}<ul>{#each parts.rest as r (r.id)}{@render card(r)}{/each}</ul>{/if}
    {#if more}<p class="hint">Older reports are not shown here.</p>{/if}
  {/if}
</section>

<style>
  .qv { display: grid; gap: 8px; font: 14px/1.45 var(--font); color: var(--fg); }
  .qvh { display: flex; align-items: baseline; gap: 12px; }
  h2 { margin: 0; font-size: 16px; font-weight: 600; flex: 1; overflow-wrap: anywhere; }
  h3 { margin: 10px 0 0; font: 600 12px/1.2 var(--font); color: var(--muted); text-transform: uppercase; letter-spacing: .04em; }
  .sub, .hint { margin: 0; color: var(--muted); font-size: 13px; }
  .heard { margin: 0; min-height: 1.45em; font-size: 13px; }
  .seg { display: flex; flex-wrap: wrap; border: 1px solid var(--border-strong); border-radius: var(--radius-sm); overflow: hidden; width: fit-content; }
  .seg button { min-height: 28px; padding: 3px 10px; border: 0; border-radius: 0; background: var(--card); color: var(--muted); font-size: 12.5px; }
  .seg button + button { border-left: 1px solid var(--border); }
  .seg button[aria-pressed="true"] { background: var(--fg); color: var(--bg); }
  .n { font-variant-numeric: tabular-nums; }
  ul { list-style: none; margin: 0; padding: 0; display: grid; gap: 6px; }
  .card { display: grid; gap: 2px; padding: 8px 10px; border: 1px solid var(--border); border-radius: var(--radius-sm); background: var(--card); color: var(--fg); text-decoration: none; }
  .card:hover { border-color: var(--border-hover); }
  .card:focus-visible { outline: 2px solid var(--focus, var(--fg)); outline-offset: 2px; }
  .t { font-weight: 500; overflow-wrap: anywhere; }
  .m { color: var(--muted); font-size: 12.5px; overflow-wrap: anywhere; }
  .st { font-weight: 600; }
  .st-open { color: var(--you-ink, var(--fg)); }
  .st-claimed { color: var(--agent-ink, var(--fg)); }
  @media (forced-colors: active) { .card { border-color: CanvasText; } }
</style>
