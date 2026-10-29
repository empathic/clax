<svelte:options css="injected" />

<script lang="ts">
  // A queue's reports (`/queues/<qid>`, spec 2026-10-29-feedback-loop-design
  // §8), the latest filed first, by tab: Active (open or claimed), Fix
  // proposed, Verified, Resolved and All; the tab is kept in the address
  // (`?state=`, which each row's link carries to the report's view). Each
  // row links to the report's view and shows its note,
  // its clip's thumbnail, who filed it, the release they ran, how long ago,
  // its state and how many fixes it has had. Load more fetches the next
  // page from the listing's cursor. A tab's list is fetched again, as many
  // pages as it shows, whenever the queue's topic says a report changed and
  // when the stream comes back (one fetch at a time, one more after it when
  // another change came meanwhile, none while Load more is fetching): the
  // rows stay until the new ones replace them, so the page keeps its place,
  // and what changed is said politely.
  import { onMount } from "svelte";
  import type { EventStream } from "../stream";
  import { relativeTime } from "../format";
  import * as api from "./api";
  import { STATE_LABEL, TABS, noteOf, queueTopic, refusal, reportHref, reporterName } from "./model";

  const p: { qid: string; queue?: api.Queue | null; nav: (e: MouseEvent) => void; stream?: EventStream } = $props();
  const fromUrl = () => TABS.find(t => t.key === new URLSearchParams(location.search).get("state"))?.key ?? "active";
  let tab = $state(fromUrl());
  let reports = $state<api.Report[] | null>(null);
  let next = $state<string | null>(null);
  let err = $state<string | null>(null);
  let more = $state(false);
  // What changed since the list was last fetched, said politely.
  let notice = $state("");
  // How many pages the tab shows.
  let pages = 1;
  // Each fetch's turn: an answer for an earlier tab is dropped.
  let turn = 0;

  async function load(cursor: string | null = null) {
    const mine = ++turn;
    const t = TABS.find(x => x.key === tab)!;
    if (cursor) more = true; else { reports = null; next = null; }
    err = null;
    try {
      const page = await api.reportPage(p.qid, cursor, { state: t.state, order: "newest" });
      if (mine !== turn) return;
      reports = cursor ? [...(reports ?? []), ...page.reports] : page.reports;
      next = page.next;
      pages = cursor ? pages + 1 : 1;
    } catch (x) {
      if (mine === turn) err = `Could not load the reports: ${refusal(x)}`;
    } finally {
      if (mine === turn) {
        more = false;
        // A change heard while this fetch was out.
        if (again && !fetching) void refresh();
      }
    }
  }
  /** What changed between the rows `was` and `now`, as a sentence. */
  function changed(was: api.Report[], now: api.Report[]): string {
    const added = now.filter(r => !was.some(w => w.id === r.id)).length;
    const moved = now.filter(r => was.some(w => w.id === r.id && w.state !== r.state)).length;
    return [added && `${added} new ${added === 1 ? "report" : "reports"}.`, moved && `${moved} ${moved === 1 ? "report" : "reports"} changed.`].filter(Boolean).join(" ");
  }
  /** Fetches the tab's pages again in place, as many as it shows. */
  async function reload() {
    const mine = ++turn;
    const t = TABS.find(x => x.key === tab)!;
    const got: api.Report[] = [];
    let cursor: string | null = null;
    let n = 0;
    try {
      do {
        const page = await api.reportPage(p.qid, cursor, { state: t.state, order: "newest" });
        if (mine !== turn) return;
        got.push(...page.reports);
        cursor = page.next;
        n++;
      } while (cursor && n < pages);
    } catch (x) {
      if (mine === turn) err = `Could not load the reports: ${refusal(x)}`;
      return;
    }
    const said = changed(reports ?? [], got);
    if (said) notice = said;
    reports = got;
    next = cursor;
    err = null;
  }
  // One refetch at a time; a change heard meanwhile fetches once more after it.
  let fetching = false;
  let again = false;
  async function refresh() {
    // The first page is still coming, or Load more is: it refreshes once it answers.
    if (fetching || more || reports === null) { again = true; return; }
    fetching = true;
    try { do { again = false; await reload(); } while (again); } finally { fetching = false; }
  }
  function choose(key: string) {
    if (key === tab) return;
    tab = key;
    const u = new URL(location.href);
    if (key === "active") u.searchParams.delete("state"); else u.searchParams.set("state", key);
    history.replaceState(history.state, "", `${u.pathname}${u.search}${u.hash}`);
    void load();
  }
  function keys(e: KeyboardEvent) {
    const i = TABS.findIndex(t => t.key === tab);
    const j = ({ ArrowRight: i + 1, ArrowLeft: i - 1, Home: 0, End: TABS.length - 1 } as Record<string, number>)[e.key];
    if (j === undefined) return;
    e.preventDefault();
    const k = TABS[(j + TABS.length) % TABS.length].key;
    choose(k);
    (e.currentTarget as HTMLElement).querySelector<HTMLElement>(`[data-tab="${k}"]`)?.focus();
  }
  const fixes = (r: api.Report) => r.fixes?.length ?? 0;

  onMount(() => {
    void load();
    return p.stream?.watch([queueTopic(p.qid)], e => {
      if (e.type === "ready" || e.type === "resync" || e.type === "report") void refresh();
    });
  });
</script>

<section class="qview" aria-labelledby="qv-h">
  <p class="back"><a href="/queues" onclick={p.nav}>Queues</a></p>
  <header class="qh">
    <h2 id="qv-h">{p.queue?.name ?? "Queue"}</h2>
    {#if p.queue}<span class="role">{p.queue.role === "admin" ? "Admin" : "Triage"}</span>{/if}
  </header>
  {#if p.queue === undefined}<p class="qerr" role="alert">No queue {p.qid} is yours to triage.</p>{/if}
  <div class="tabs" role="tablist" aria-label="Reports by state" tabindex="-1" onkeydown={keys}>
    {#each TABS as t (t.key)}
      <button type="button" role="tab" class="tab" data-tab={t.key} aria-selected={tab === t.key} tabindex={tab === t.key ? 0 : -1} aria-controls="qv-list" onclick={() => choose(t.key)}>{t.label}</button>
    {/each}
  </div>
  <p class="sr" aria-live="polite">{notice}</p>
  <div id="qv-list" role="tabpanel" aria-label={TABS.find(t => t.key === tab)?.label}>
    {#if err}<p class="qerr" role="alert">{err}</p>{/if}
    {#if reports?.length}
      <ul class="rlist">
        {#each reports as r (r.id)}
          {@const n = fixes(r)}
          <li class="rrow" data-id={r.id}>
            <a class="rlink" href={`${reportHref(p.qid, r.id)}${tab === "active" ? "" : `?state=${tab}`}`} onclick={p.nav}>
              {#if r.has_clip}<img class="thumb" src={api.clipUrl(p.qid, r.id)} alt="" loading="lazy" />{:else}<span class="thumb none" aria-hidden="true"></span>{/if}
              <span class="main">
                <span class="note">{noteOf(r) || "(no note)"}</span>
                <span class="meta">
                  <span class="who">{reporterName(r)}</span>
                  {#if r.context?.release?.version}<span class="rel">{r.context.release.version}</span>{/if}
                  <time datetime={r.created_at} title={r.created_at}>{relativeTime(r.created_at)}</time>
                </span>
              </span>
              <span class="side">
                <span class={["state", r.state]}>{STATE_LABEL[r.state] ?? r.state}</span>
                {#if n}<span class="nfix">{n} {n === 1 ? "fix" : "fixes"}</span>{/if}
              </span>
            </a>
          </li>
        {/each}
      </ul>
      {#if next}<p class="more"><button type="button" class="ghost" disabled={more} onclick={() => void load(next)}>{more ? "Loading…" : "Load more"}</button></p>{/if}
    {:else if reports}
      <p class="qnone">No {tab === "all" ? "" : `${TABS.find(t => t.key === tab)?.label.toLowerCase()} `}reports.</p>
    {:else if !err}
      <p class="qnone" role="status">Loading…</p>
    {/if}
  </div>
</section>

<style>
  .qview { max-width: 880px; margin: 0 auto; min-width: 0; }
  .back { margin: 0 0 6px; font-size: 13px; }
  .back a::before { content: "← "; }
  .qh { display: flex; align-items: center; gap: 8px 12px; flex-wrap: wrap; }
  .qh h2 { margin: 0; font-size: 22px; line-height: 1.2; overflow-wrap: anywhere; min-width: 0; }
  .role { padding: 1px 8px; border: 1px solid var(--border); border-radius: 999px; color: var(--muted); font-size: 12px; }
  .qerr { margin: 12px 0 0; color: var(--danger); font-size: 13px; }
  .tabs { display: flex; gap: 2px; margin: 14px 0 12px; border-bottom: 1px solid var(--border); overflow-x: auto; }
  .tab { min-height: 34px; padding: 4px 12px; border: 0; border-bottom: 2px solid transparent; border-radius: 0; background: none; color: var(--muted); white-space: nowrap; }
  .tab[aria-selected="true"] { color: var(--fg); border-bottom-color: var(--accent); font-weight: 600; }
  .rlist { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 8px; }
  .rlink { display: grid; grid-template-columns: 72px minmax(0, 1fr) auto; gap: 12px; align-items: center; padding: 10px 12px; background: var(--card); border: 1px solid var(--border); border-radius: var(--radius); color: inherit; text-decoration: none; }
  .rlink:hover { border-color: var(--border-hover); }
  .thumb { width: 72px; height: 48px; object-fit: cover; object-position: left top; border: 1px solid var(--border); border-radius: var(--radius-xs); background: var(--bg); }
  .thumb.none { display: block; background: var(--hover); }
  .main { display: flex; flex-direction: column; gap: 3px; min-width: 0; }
  .note { font-size: 14px; overflow: hidden; display: -webkit-box; -webkit-line-clamp: 2; line-clamp: 2; -webkit-box-orient: vertical; overflow-wrap: anywhere; }
  .meta { display: flex; flex-wrap: wrap; gap: 2px 10px; color: var(--muted); font-size: 12.5px; }
  .rel { font: 12px var(--mono); }
  .side { display: flex; flex-direction: column; align-items: flex-end; gap: 4px; }
  .state { padding: 1px 8px; border-radius: 999px; font-size: 12px; white-space: nowrap; border: 1px solid var(--tag-gray-bd); background: var(--tag-gray-bg); color: var(--tag-gray-fg); }
  .state.open { border-color: var(--tag-blue-bd); background: var(--tag-blue-bg); color: var(--tag-blue-fg); }
  .state.claimed { border-color: var(--tag-purple-bd); background: var(--tag-purple-bg); color: var(--tag-purple-fg); }
  .state.fix_proposed { border-color: var(--tag-orange-bd); background: var(--tag-orange-bg); color: var(--tag-orange-fg); }
  .state.verified { border-color: var(--tag-green-bd); background: var(--tag-green-bg); color: var(--tag-green-fg); }
  .nfix { color: var(--muted); font-size: 12px; white-space: nowrap; }
  .more { text-align: center; margin: 12px 0; }
  .qnone { color: var(--muted); text-align: center; padding: 48px 0; }
  .sr { position: absolute; width: 1px; height: 1px; margin: -1px; padding: 0; overflow: hidden; clip: rect(0 0 0 0); white-space: nowrap; border: 0; }

  @media (max-width: 560px) {
    .rlink { grid-template-columns: 56px minmax(0, 1fr); }
    .thumb { width: 56px; height: 40px; }
    .side { grid-column: 2; flex-direction: row; align-items: center; }
  }
  @media (pointer: coarse) { .tab { min-height: 44px; } }
</style>
