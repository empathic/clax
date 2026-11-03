<script lang="ts">
  // The DevTools panel's Network view (first slice, network.svelte.ts): a
  // filter (All / Failed / Fetch/XHR / Documents / Other sites), a search,
  // "Keep log", Clear and "Copy as text"; a table of the requests, each URL a
  // button that selects it; and the details region below, reserved whether or
  // not one is selected (no layout shift), with timing phases, headers
  // folded until asked (credentials always hidden) and the body only on its
  // own click. What it did is said in the host's notices zone (`say`).
  import { FILTERS, type Filter, type NetworkLog, type Row, asText, bytes, loadBody, matches, shownHeaders } from "./network.svelte";

  let { log, say, copy }: { log: Pick<NetworkLog, "rows" | "keep" | "clear">; say(text: string): void; copy(text: string): Promise<boolean> } = $props();
  let filter = $state<Filter>("all");
  let word = $state("");
  let selected = $state<number | null>(null);
  let body = $state<{ n: number; text: string } | null>(null);
  let showReq = $state(false);
  let showRes = $state(false);
  const shown = $derived(log.rows.filter(r => matches(r, filter, word)));
  const sel = $derived(log.rows.find(r => r.n === selected) ?? null);
  // A deselected request's body is dropped, its headers folded again.
  $effect(() => { if (body && body.n !== selected) body = null; });
  function pick(r: Row): void {
    if (selected === r.n) { selected = null; return; }
    selected = r.n;
    showReq = showRes = false;
  }
  async function copyAll(): Promise<void> {
    const ok = await copy(asText(shown));
    say(ok ? `Copied ${shown.length} request${shown.length === 1 ? "" : "s"} as text (no headers, cookies or bodies).` : "Could not copy.");
  }
  function clear(): void { log.clear(); selected = null; say("Network log cleared."); }
  const failedCount = $derived(log.rows.filter(r => r.failed).length);
</script>

<div class="net">
  <div class="bar">
    <div class="seg" role="group" aria-label="Show requests">
      {#each FILTERS as f (f.value)}
        <button type="button" aria-pressed={filter === f.value} onclick={() => (filter = f.value)}>{f.label}</button>
      {/each}
    </div>
    <input type="search" aria-label="Search requests" placeholder="Search method, URL, status" value={word} oninput={e => (word = e.currentTarget.value)} />
    <label class="keep"><input type="checkbox" checked={log.keep} onchange={e => (log.keep = e.currentTarget.checked)} /> Keep log</label>
    <button type="button" class="ghost" onclick={clear}>Clear</button>
    <button type="button" class="ghost" onclick={copyAll} disabled={!shown.length}>Copy as text</button>
  </div>
  <p class="count">{log.rows.length} request{log.rows.length === 1 ? "" : "s"}, {failedCount} failed{#if shown.length !== log.rows.length}; {shown.length} shown{/if}. Kept in this panel only.</p>
  <div class="list">
    {#if !shown.length}
      <p class="hint">{log.rows.length ? "No request matches." : "No requests yet. Reload the page to see them from the start."}</p>
    {:else}
      <table>
        <caption class="sr-only">Requests of the inspected page</caption>
        <thead><tr><th scope="col">Method</th><th scope="col">URL</th><th scope="col">Status</th><th scope="col">Kind</th><th scope="col">Size</th><th scope="col">Time</th></tr></thead>
        <tbody>
          {#each shown as r (r.n)}
            <tr class:failed={r.failed} class:sel={selected === r.n}>
              <td>{r.method}</td>
              <td class="u"><button type="button" class="link" aria-expanded={selected === r.n} aria-controls="net-detail" onclick={() => pick(r)} title={r.url}>{r.url}</button>{#if r.otherSite}<span class="tag">other site</span>{/if}</td>
              <td>{r.status || "—"}{#if r.failed}<span class="sr-only"> failed</span>{/if}</td>
              <td>{r.kind}</td>
              <td class="num">{bytes(r.size)}</td>
              <td class="num">{r.ms} ms</td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}
  </div>
  <section class="detail" id="net-detail" aria-label="Request details">
    {#if !sel}
      <p class="hint">Select a request to see its details.</p>
    {:else}
      <h2>{sel.method} {sel.url}</h2>
      <p>Status {sel.status || "—"}{#if sel.error}, {sel.error}{/if} · {sel.kind} · {bytes(sel.size)} · {sel.ms} ms, started at +{sel.at} ms</p>
      {#if sel.timings.length}
        <dl class="timing">{#each sel.timings as t (t.phase)}<dt>{t.phase}</dt><dd>{t.ms} ms</dd>{/each}</dl>
      {/if}
      <button type="button" class="ghost" aria-expanded={showReq} onclick={() => (showReq = !showReq)}>{showReq ? "Hide" : "Show"} request headers</button>
      {#if showReq}<dl class="hdrs">{#each shownHeaders(sel.entry.request.headers) as h, i (i)}<dt>{h.name}</dt><dd>{h.value}</dd>{/each}</dl>{/if}
      <button type="button" class="ghost" aria-expanded={showRes} onclick={() => (showRes = !showRes)}>{showRes ? "Hide" : "Show"} response headers</button>
      {#if showRes}<dl class="hdrs">{#each shownHeaders(sel.entry.response.headers) as h, i (i)}<dt>{h.name}</dt><dd>{h.value}</dd>{/each}</dl>{/if}
      {#if body?.n === sel.n}
        <pre class="body" aria-label="Response body">{body.text}</pre>
      {:else}
        <button type="button" class="ghost" onclick={() => { const n = sel.n; void loadBody(sel.entry).then(text => { if (selected === n) body = { n, text }; }); }}>Load body</button>
      {/if}
    {/if}
  </section>
</div>

<style>
  .net { display: grid; grid-template-rows: auto auto minmax(0, 1fr) minmax(120px, 38%); height: 100%; min-height: 0; }
  .bar { display: flex; flex-wrap: wrap; gap: 8px; align-items: center; padding: 8px var(--gutter); border-bottom: 1px solid var(--border); }
  .seg { display: flex; border: 1px solid var(--border-strong); border-radius: var(--radius-sm); overflow: hidden; }
  .seg button { min-height: 26px; padding: 2px 8px; border: 0; border-radius: 0; background: var(--card); color: var(--muted); font-size: 12.5px; }
  .seg button + button { border-left: 1px solid var(--border); }
  .seg button[aria-pressed="true"] { background: var(--fg); color: var(--bg); }
  .bar input[type="search"] { flex: 1; min-width: 10rem; font-size: 12.5px; }
  .keep { display: flex; gap: 4px; align-items: center; font-size: 12.5px; }
  .count { margin: 4px var(--gutter); font-size: 12px; color: var(--muted); }
  .list { overflow: auto; min-height: 0; }
  table { width: 100%; border-collapse: collapse; font: 12px/1.4 var(--mono); }
  th { position: sticky; top: 0; background: var(--card); text-align: left; font: 600 11.5px var(--font); color: var(--muted); padding: 4px 6px; border-bottom: 1px solid var(--border); }
  td { padding: 2px 6px; border-bottom: 1px solid var(--border); white-space: nowrap; }
  td.u { max-width: 0; width: 60%; overflow: hidden; text-overflow: ellipsis; }
  td.num { text-align: right; font-variant-numeric: tabular-nums; }
  tr.failed td { color: var(--danger, #b42318); }
  tr.sel td { background: var(--hover); }
  .link { all: unset; cursor: pointer; overflow: hidden; text-overflow: ellipsis; max-width: 100%; display: inline-block; vertical-align: bottom; }
  .link:focus-visible { outline: 2px solid var(--fg); outline-offset: 1px; }
  .tag { margin-left: 6px; font: 11px var(--font); color: var(--muted); }
  .detail { overflow: auto; border-top: 1px solid var(--border); padding: 8px var(--gutter); display: grid; gap: 6px; align-content: start; justify-items: start; }
  .detail h2 { margin: 0; font: 600 12.5px var(--mono); overflow-wrap: anywhere; }
  .detail p { margin: 0; font-size: 12.5px; }
  dl { display: grid; grid-template-columns: max-content 1fr; gap: 2px 10px; margin: 0; font: 12px var(--mono); }
  dt { color: var(--muted); }
  dd { margin: 0; overflow-wrap: anywhere; }
  .body { margin: 0; max-width: 100%; max-height: 16rem; overflow: auto; font: 12px var(--mono); white-space: pre-wrap; overflow-wrap: anywhere; }
  .hint { color: var(--muted); margin: 8px var(--gutter); font-size: 13px; }
  .detail .hint { margin: 0; }
  .sr-only { position: absolute; width: 1px; height: 1px; overflow: hidden; clip: rect(0 0 0 0); white-space: nowrap; }
</style>
