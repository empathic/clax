<script lang="ts">
  // Merging pages (owner decision 2026-10-06): a pattern of the site's paths
  // with `:name` segments (or a last `*`) makes the pages it matches one
  // page, whose threads are then listed together. The pattern is checked
  // and its pages previewed here, as the daemon matches them; the merge
  // runs in batches, repeated while the daemon says threads remain, with
  // its progress shown. A rule deleted un-merges the same way.
  import type { SiteRule, SiteView } from "../messages";
  import { checkPattern, preview } from "./site-model";

  type Step = { moved: number; remaining: number };
  type Ask = { t: "rule"; pattern: string } | { t: "unrule"; ruleId: string };
  let { site, request }: { site: SiteView | null; request(m: Ask): Promise<Step> } = $props();
  let pattern = $state("");
  const p = $derived(pattern.trim());
  const error = $derived(p ? checkPattern(p) : null);
  const matched = $derived(error ? [] : preview(site, p));
  /** The batch run under way: what it is, how many threads moved, how many are left. */
  let busy = $state<{ what: string; moved: number; remaining: number | null } | null>(null);
  let note = $state<string | null>(null);
  const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

  async function run(what: string, ask: Ask, done: (moved: number) => string): Promise<void> {
    note = null;
    busy = { what, moved: 0, remaining: null };
    try {
      for (;;) {
        const r = await request(ask);
        busy = { what, moved: busy.moved + r.moved, remaining: r.remaining };
        if (!r.remaining) break;
      }
      note = done(busy.moved);
      if (ask.t === "rule") pattern = "";
    } catch (e) {
      note = e instanceof Error ? e.message : String(e);
    } finally {
      busy = null;
    }
  }
  const merge = () => run(`Merging ${p}`, { t: "rule", pattern: p }, n => `Merged: ${plural(n, "thread")} moved to ${p}.`);
  const unmerge = (r: SiteRule) => run(`Un-merging ${r.pattern}`, { t: "unrule", ruleId: r.id }, n => `Un-merged ${r.pattern}: ${plural(n, "thread")} moved back.`);
</script>

<details class="merge">
  <summary><h2>Merge pages</h2></summary>
  <p class="hint">Pages whose paths match a pattern become one page, and their comments are listed together. Use <code>:name</code> for a path segment that varies, such as <code>/users/:id</code>, or a last <code>*</code> for the rest of a path.</p>
  <label class="field"><span>Pattern</span>
    <input aria-label="Pattern" placeholder="/users/:id" spellcheck="false" autocomplete="off" value={pattern} oninput={e => (pattern = e.currentTarget.value)}
      onkeydown={e => { if (e.key === "Enter" && p && !error && !busy) void merge(); }} />
  </label>
  {#if error}
    <p class="err" role="alert">{error}</p>
  {:else if p}
    <p class="small">{matched.length ? `Matches ${plural(matched.length, "page")} with comments:` : "Matches no page with comments yet."}</p>
    {#if matched.length}<ul class="paths">{#each matched as m (m)}<li title={m}>{m}</li>{/each}</ul>{/if}
  {/if}
  <div class="row"><button type="button" class="primary" disabled={!p || !!error || !!busy} onclick={merge}>Merge pages</button></div>
  {#if busy}
    <p class="small" role="status">{busy.what}… {plural(busy.moved, "thread")} moved{busy.remaining !== null ? `, ${busy.remaining} left` : ""}</p>
  {:else if note}
    <p class="small" role="status">{note}</p>
  {/if}
  {#if site?.rules.length}
    <h3>Merged pages</h3>
    <ul class="rules">
      {#each site.rules as r (r.id)}
        <li>
          <span class="path" title={r.pattern}>{r.pattern}</span>
          {#if r.deleting}<span class="small">un-merging</span>{/if}
          <button type="button" class="ghost" disabled={!!busy} onclick={() => unmerge(r)} aria-label={`Un-merge ${r.pattern}`}>Un-merge</button>
        </li>
      {/each}
    </ul>
  {/if}
</details>

<style>
  .merge { padding: 4px var(--gutter) 14px; border-top: 1px solid var(--border); }
  summary { list-style: none; cursor: pointer; }
  summary::-webkit-details-marker { display: none; }
  summary h2 { display: flex; align-items: center; gap: 8px; margin: 12px 2px 4px; font: 600 14px/1.2 var(--font); }
  summary h2::before { content: "▸"; color: var(--muted); font-size: 11px; }
  details[open] summary h2::before { content: "▾"; }
  .hint, .small { margin: 6px 0; font-size: 12.5px; color: var(--muted); overflow-wrap: anywhere; }
  code { font: 12px var(--mono); padding: 0 4px; border-radius: var(--radius-xs); background: var(--hover); white-space: nowrap; }
  .field { display: grid; gap: 4px; margin: 8px 0; font-size: 12.5px; color: var(--muted); }
  .field input { width: 100%; min-width: 0; font: 13px var(--mono); }
  .err { margin: 6px 0; font-size: 12.5px; color: var(--danger); overflow-wrap: anywhere; }
  .paths { margin: 4px 0 8px; padding: 0; list-style: none; display: grid; gap: 2px; }
  .paths li, .path { font: 12.5px/1.5 var(--mono); min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .row { display: flex; justify-content: flex-end; }
  h3 { margin: 14px 2px 6px; font-size: 13px; color: var(--muted); }
  .rules { margin: 0; padding: 0; list-style: none; }
  .rules li { display: flex; align-items: center; gap: 8px; padding: 4px 0 4px 2px; border-bottom: 1px solid var(--border); }
  .rules .path { flex: 1; }
  .rules button { min-height: 28px; padding: 2px 8px; font-size: 12.5px; }
</style>
