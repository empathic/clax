<!-- Its CSS travels with it: the shell loads it lazily, and its HTML carries none of it. -->
<svelte:options css="injected" />

<script lang="ts">
  // A reporter's view of its queue (`/reporter/<qid>`) and of one report
  // (`/reporter/<qid>/<tid>`), read with the view session's cookie
  // (reporter-api.ts). The list is the reports the queue's "A reporter sees"
  // setting shows this reporter, newest first, each one link to its page;
  // under "Every report in the queue" each names who filed it ("You" on the
  // reader's own). A report's page, in fixed zones: its header and state, a
  // status line kept in place, what it is about, its screenshot, its thread,
  // and a reply. A reporter replies; resolving is the triage team's. Read
  // again when the page is shown again. Everything shown is text.
  import { onMount } from "svelte";
  import { STATE_TEXT, aboutOf, failureOf, pageOf, readAll, repliesOf, serial, titleOf } from "../../../overlay/src/triage/triage";
  import type { Report } from "../../../sdk/src/types";
  import { type ReporterQueue, ViewRefused, filedBy, reporterApi, reporterQueuePath, reporterReportPath, reporterRouteOf } from "./reporter-api";

  const route = reporterRouteOf(location.pathname);
  const api = route ? reporterApi(route.qid) : null;
  let queue = $state<ReporterQueue | null>(null);
  let reports = $state<Report[] | null>(null);
  let more = $state(false);
  let report = $state<Report | null>(null);
  let ended = $state(false);
  let error = $state<string | null>(null);
  let said = $state("");
  let text = $state("");
  let busy = $state(false);

  const failed = (x: unknown, what: string) => {
    if (x instanceof ViewRefused && x.expired) ended = true;
    else error = `Could not read ${what}: ${failureOf(x)}`;
  };
  const load = serial(async () => {
    if (!api || !route) return;
    try {
      queue ??= await api.queue();
      if (route.tid) {
        const was = report;
        report = await api.report(route.tid);
        if (was && report.comments.length > was.comments.length) said = "New replies.";
        document.title = `Report: ${titleOf(report)} · Clax`;
      } else {
        const r = await readAll({ reports: (_q, c) => api.reports(c) }, route.qid);
        reports = [...r.reports].sort((a, b) => b.created_at.localeCompare(a.created_at));
        more = r.more;
        document.title = `${queue.name} · Clax`;
      }
      error = null;
    } catch (x) {
      failed(x, route.tid ? "the report" : "the reports");
    }
  });
  onMount(() => {
    void load();
    const again = () => { if (document.visibilityState === "visible") void load(); };
    document.addEventListener("visibilitychange", again);
    return () => document.removeEventListener("visibilitychange", again);
  });

  async function reply(): Promise<void> {
    const body = text.trim();
    if (!api || !route?.tid || !body || busy) return;
    busy = true;
    try {
      report = await api.reply(route.tid, body);
      text = "";
      said = "Reply sent.";
    } catch (x) {
      if (x instanceof ViewRefused && x.expired) ended = true;
      else said = `Not sent: ${failureOf(x)}`;
    } finally {
      busy = false;
    }
  }
  const who = (k: string) => (k === "reporter" ? "reporter" : "triage");
</script>

<main class="rr">
  {#if !route}
    <p class="hint">No such page.</p>
  {:else if ended}
    <h1>This view has ended</h1>
    <p>Views of your reports last 15 minutes. Open your reports from the app again to get a fresh link.</p>
  {:else if route.tid}
    <a class="back" href={reporterQueuePath(route.qid)}>← {queue ? `Reports in ${queue.name}` : "Your reports"}</a>
    <h1 id="rr-title">{report ? titleOf(report) : "Report"}</h1>
    {#if report}
      {@const by = filedBy(report)}
      <p class="state"><span class="st">{STATE_TEXT[report.state]}</span>{#if by} · filed by {by}{/if} · <time datetime={report.created_at}>{new Date(report.created_at).toLocaleString()}</time></p>
    {/if}
    <!-- The status line: reserved, so what is said here moves nothing below. -->
    <p class="said" role="status" aria-live="polite">{error ?? said}</p>
    {#if report}
      {@const r = report}
      {@const at = pageOf(r)}
      <section class="zone" aria-label="What it is about">
        <p>{aboutOf(r)}, on <span class="page">{r.artifact.title || r.artifact.url}</span></p>
        {#if at}<a href={at} target="_blank" rel="noopener noreferrer">Open on the page</a>{/if}
      </section>
      {#if r.has_clip}
        <figure class="zone shot"><img src={api!.clip(r.id)} alt="Screenshot filed with the report: {aboutOf(r)}" /></figure>
      {/if}
      <section class="zone" aria-labelledby="rr-thread">
        <h2 id="rr-thread">Thread</h2>
        <ol class="thread">
          {#each r.comments as c (c.id)}
            <li>
              <p class="by"><strong>{c.author_name || "Someone"}</strong> <span class="kind">({who(c.author_kind)})</span> · <time datetime={c.created_at}>{new Date(c.created_at).toLocaleString()}</time></p>
              <p class="body">{c.body}</p>
            </li>
          {/each}
        </ol>
        {#if !repliesOf(r).length}<p class="hint">No replies yet.</p>{/if}
      </section>
      <section class="zone acts" aria-label="Your reply">
        <label for="rr-reply">Reply</label>
        <textarea id="rr-reply" rows="3" value={text} oninput={e => (text = e.currentTarget.value)}
          onkeydown={e => { if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) void reply(); }}></textarea>
        <button type="button" class="primary" disabled={busy || !text.trim()} onclick={reply}>Send reply</button>
      </section>
    {:else if !error}
      <p class="hint">Reading the report…</p>
    {/if}
  {:else}
    <h1>{queue ? (queue.reporters_see === "all" ? `Reports in ${queue.name}` : `Your reports in ${queue.name}`) : "Your reports"}</h1>
    {#if queue}
      <p class="sub">{queue.reporters_see === "all" ? "This queue shows every report filed in it. Yours are marked You." : "This queue shows you only the reports you filed."}</p>
    {/if}
    <p class="said" role="status" aria-live="polite">{error ?? said}</p>
    {#if reports}
      {#if reports.length}
        <ul>
          {#each reports as r (r.id)}
            {@const by = filedBy(r)}
            <li>
              <a class="card" href={reporterReportPath(route.qid, r.id)}>
                <span class="t">{titleOf(r)}</span>
                <span class="m"><span class="st">{STATE_TEXT[r.state]}</span>{#if by} · {by === "You" ? "yours" : `filed by ${by}`}{/if} · {aboutOf(r)}</span>
                <span class="m">{r.artifact.title || r.artifact.url} · <time datetime={r.created_at}>{new Date(r.created_at).toLocaleString()}</time></span>
              </a>
            </li>
          {/each}
        </ul>
        {#if more}<p class="hint">Older reports are not shown here.</p>{/if}
      {:else}
        <p class="hint">{queue?.reporters_see === "all" ? "No reports in this queue yet." : "You have not filed a report yet."}</p>
      {/if}
    {:else if !error}
      <p class="hint">Reading the reports…</p>
    {/if}
  {/if}
</main>

<style>
  .rr { display: grid; gap: 10px; max-width: 52rem; margin: 0 auto; padding: 16px var(--gutter, 16px) 32px; font: 14px/1.45 var(--font); color: var(--fg); }
  h1 { margin: 0; font-size: 18px; font-weight: 600; overflow-wrap: anywhere; }
  h2 { margin: 0 0 4px; font: 600 12px/1.2 var(--font); color: var(--muted); text-transform: uppercase; letter-spacing: .04em; }
  .back { font-size: 13px; color: var(--muted); width: fit-content; }
  .sub, .hint, .state { margin: 0; font-size: 13px; }
  .sub, .hint { color: var(--muted); }
  .said { margin: 0; min-height: 1.45em; font-size: 13px; }
  .st { font-weight: 600; }
  ul { list-style: none; margin: 0; padding: 0; display: grid; gap: 6px; }
  .card { display: grid; gap: 2px; padding: 8px 10px; border: 1px solid var(--border); border-radius: var(--radius-sm); background: var(--card); color: var(--fg); text-decoration: none; }
  .card:hover { border-color: var(--border-hover); }
  .card:focus-visible { outline: 2px solid var(--focus, var(--fg)); outline-offset: 2px; }
  .t { font-weight: 500; overflow-wrap: anywhere; }
  .m { color: var(--muted); font-size: 12.5px; overflow-wrap: anywhere; }
  .zone { display: grid; gap: 6px; justify-items: start; margin: 0; padding-top: 8px; border-top: 1px solid var(--border); }
  .zone p { margin: 0; overflow-wrap: anywhere; }
  .page { font-weight: 500; }
  .shot img { max-width: 100%; height: auto; border: 1px solid var(--border); border-radius: var(--radius-sm); }
  .thread { list-style: none; margin: 0; padding: 0; display: grid; gap: 8px; width: 100%; }
  .by { font-size: 12.5px; color: var(--muted); }
  .by strong { color: var(--fg); font-weight: 600; }
  .body { white-space: pre-wrap; }
  .acts { justify-items: stretch; }
  .acts label { font-weight: 500; }
  .acts button { justify-self: start; }
  textarea { width: 100%; box-sizing: border-box; font: inherit; }
  @media (forced-colors: active) { .card { border-color: CanvasText; } }
</style>
