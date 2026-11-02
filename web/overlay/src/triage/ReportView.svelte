<!-- Its CSS travels with it: the shell loads it lazily, and its HTML carries none of it. -->
<svelte:options css="injected" />

<script lang="ts">
  // One report (triage.ts), in fixed zones: its header (name, state, claim),
  // a status line kept in place, what it is about with "Open on the page"
  // (or, in the DevTools panel, `onShow`, which highlights it in the
  // inspected page), its screenshot shown inline, its thread (the
  // reporter's note, then every reply with who wrote it), its details, and
  // what the viewer may do: reply, and resolve while it is open. A claim is
  // an agent's: shown, never taken here. Live: each change the host hears
  // reads it again (one read at a time), said politely, focus left where it
  // is. Everything shown is text.
  import { onMount } from "svelte";
  import { type Report, type TriageApi, STATE_TEXT, aboutOf, claimText, failureOf, pageOf, repliesOf, selectorOf, serial, titleOf } from "./triage";

  type Props = {
    api: TriageApi;
    qid: string;
    tid: string;
    /** The way back to the queue: a link, opened in place by `onclick` when given. */
    back?: { label: string; href: string; onclick?(): void } | null;
    /** Shows the report's element in the inspected page (the DevTools panel); without it, "Open on the page" is a link. */
    onShow?(r: Report): void;
    /** A link to this report's own page in Clax (the DevTools panel's "Open in Clax"). */
    openIn?: string | null;
  };
  let { api, qid, tid, back = null, onShow, openIn = null }: Props = $props();
  let report = $state<Report | null>(null);
  let error = $state<string | null>(null);
  let said = $state("");
  let clip = $state<string | null>(null);
  let text = $state("");
  let busy = $state(false);
  let gone = false;

  const load = serial(async () => {
    try {
      const r = await api.report(qid, tid);
      if (gone) return;
      const was = report;
      report = r;
      error = null;
      if (was && r.comments.length > was.comments.length) said = `${r.comments.length - was.comments.length} new ${r.comments.length - was.comments.length === 1 ? "reply" : "replies"}.`;
      else if (was && r.state !== was.state) said = `Now ${STATE_TEXT[r.state].toLowerCase()}.`;
      if (r.has_clip && clip === null) clip = await api.clip(qid, tid).catch(() => null);
    } catch (e) {
      if (!gone) error = `Could not read the report: ${failureOf(e)}`;
    }
  });
  onMount(() => {
    void load();
    const off = api.watch(() => void load());
    return () => { gone = true; off(); };
  });
  $effect(() => { if (report) document.title = `Report: ${titleOf(report)} · Clax`; });

  async function act(what: string, f: () => Promise<Report>): Promise<void> {
    if (busy) return;
    busy = true;
    try {
      report = await f();
      said = what;
      return;
    } catch (e) {
      said = `Not done: ${failureOf(e)}`;
    } finally {
      busy = false;
    }
  }
  async function reply(): Promise<void> {
    const body = text.trim();
    if (!body) return;
    await act("Reply sent.", () => api.reply(qid, tid, body));
    if (said === "Reply sent.") text = "";
  }
  const who = (k: string) => (k === "reporter" ? "reporter" : k === "agent" ? "agent" : "person");
  const goBack = (e: MouseEvent) => {
    if (!back?.onclick || e.metaKey || e.ctrlKey || e.shiftKey || e.button !== 0) return;
    e.preventDefault();
    back.onclick();
  };
</script>

<article class="rv" aria-labelledby="rv-{tid}">
  {#if back}<a class="back" href={back.href} onclick={goBack}>← {back.label}</a>{/if}
  <header class="rvh">
    <h2 id="rv-{tid}">{report ? titleOf(report) : "Report"}</h2>
    {#if openIn}<a href={openIn} target="_blank" rel="noopener noreferrer">Open in Clax</a>{/if}
  </header>
  {#if report}
    {@const claim = claimText(report)}
    <p class="state"><span class="st st-{report.state}">{STATE_TEXT[report.state]}</span>{#if claim} · {claim}{/if}</p>
  {/if}
  <!-- The status line: reserved, so what is said here moves nothing below. -->
  <p class="said" role="status" aria-live="polite">{error ?? said}</p>
  {#if !report}
    <p class="hint">{error ? "" : "Reading the report…"}</p>
  {:else}
    {@const r = report}
    {@const at = pageOf(r)}
    <section class="zone" aria-label="What it is about">
      <p>{aboutOf(r)}, on <span class="page">{r.artifact.title || r.artifact.url}</span></p>
      {#if onShow && selectorOf(r)}
        <button type="button" onclick={() => onShow(r)}>Show on the page</button>
      {:else if at}
        <a href={at} target="_blank" rel="noopener noreferrer">Open on the page</a>
      {/if}
    </section>
    {#if r.has_clip}
      <figure class="zone shot">
        {#if clip}<img src={clip} alt="Screenshot filed with the report: {aboutOf(r)}" />{:else}<p class="hint">Loading the screenshot…</p>{/if}
      </figure>
    {/if}
    <section class="zone" aria-label="Thread">
      <h3>Thread</h3>
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
    <section class="zone" aria-label="Details">
      <dl>
        <dt>Filed</dt><dd><time datetime={r.created_at}>{new Date(r.created_at).toLocaleString()}</time></dd>
        <dt>Page</dt><dd>{r.artifact.url}</dd>
        {#if r.artifact.revision}<dt>Revision</dt><dd>{r.artifact.revision}</dd>{/if}
        <dt>Report</dt><dd class="mono">{r.id}</dd>
      </dl>
    </section>
    <section class="zone acts" aria-label="Actions">
      <label for="rv-reply-{tid}">Reply</label>
      <textarea id="rv-reply-{tid}" rows="3" value={text} oninput={e => (text = e.currentTarget.value)}
        onkeydown={e => { if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) void reply(); }}></textarea>
      <div class="row">
        <button type="button" class="primary" disabled={busy || !text.trim()} onclick={reply}>Send reply</button>
        {#if r.status === "open"}<button type="button" disabled={busy} onclick={() => act("Resolved.", () => api.resolve(qid, tid))}>Resolve</button>{/if}
      </div>
    </section>
  {/if}
</article>

<style>
  .rv { display: grid; gap: 10px; font: 14px/1.45 var(--font); color: var(--fg); }
  .back { font-size: 13px; color: var(--muted); width: fit-content; }
  .rvh { display: flex; align-items: baseline; gap: 12px; }
  h2 { margin: 0; flex: 1; font-size: 16px; font-weight: 600; overflow-wrap: anywhere; }
  h3 { margin: 0 0 4px; font: 600 12px/1.2 var(--font); color: var(--muted); text-transform: uppercase; letter-spacing: .04em; }
  .state, .hint { margin: 0; font-size: 13px; }
  .hint { color: var(--muted); }
  .said { margin: 0; min-height: 1.45em; font-size: 13px; }
  .st { font-weight: 600; }
  .zone { display: grid; gap: 6px; justify-items: start; margin: 0; padding-top: 8px; border-top: 1px solid var(--border); }
  .zone p { margin: 0; overflow-wrap: anywhere; }
  .page { font-weight: 500; }
  .shot img { max-width: 100%; height: auto; border: 1px solid var(--border); border-radius: var(--radius-sm); }
  .thread { list-style: none; margin: 0; padding: 0; display: grid; gap: 8px; width: 100%; }
  .by { font-size: 12.5px; color: var(--muted); }
  .by strong { color: var(--fg); font-weight: 600; }
  .body { white-space: pre-wrap; }
  dl { display: grid; grid-template-columns: max-content 1fr; gap: 2px 12px; margin: 0; font-size: 13px; }
  dt { color: var(--muted); }
  dd { margin: 0; overflow-wrap: anywhere; }
  .mono { font-family: var(--mono, ui-monospace, monospace); font-size: 12px; }
  .acts { justify-items: stretch; }
  .acts label { font-weight: 500; }
  textarea { width: 100%; box-sizing: border-box; font: inherit; }
  .row { display: flex; gap: 8px; }
</style>
