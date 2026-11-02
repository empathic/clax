<svelte:options css="injected" />

<script lang="ts">
  // A report's fixes, the latest first (spec 2026-10-29-feedback-loop-design
  // §6.2, §6.3): each with its number, what became of it, who proposed it
  // and when, its summary, its links (the change, the commit, the release
  // it ships in, where and when the reporter checks it) and, once given,
  // the verdict with its note and the release it was given on. A link is
  // one only when it is http or https.
  import type { FixAttempt } from "../../../sdk/src/types";
  import { relativeTime } from "../format";
  import { OUTCOME_LABEL, httpUrl } from "./model";

  const p: { fixes: FixAttempt[] } = $props();
  const latest = $derived([...p.fixes].reverse());
</script>

<section class="fixes" aria-label="Fixes">
  <h3>Fixes <span class="n">{p.fixes.length}</span></h3>
  {#if !p.fixes.length}
    <p class="muted">No fix was proposed yet.</p>
  {:else}
    <ol>
      {#each latest as f (f.n)}
        {@const url = httpUrl(f.url)}
        {@const check = httpUrl(f.verify_url)}
        <li class="fix" data-n={f.n}>
          <header>
            <b>Fix {f.n}</b>
            <span class={["outcome", f.outcome]}>{OUTCOME_LABEL[f.outcome] ?? f.outcome}</span>
            <span class="muted">{f.proposer_name} · <time datetime={f.created_at} title={f.created_at}>{relativeTime(f.created_at)}</time></span>
          </header>
          <p class="sum">{f.summary}</p>
          {#if url || f.url || f.commit || f.release || check || f.verify_after}
            <ul class="links">
              {#if url}<li><a href={url} target="_blank" rel="noopener noreferrer">{f.url}</a></li>{:else if f.url}<li><code>{f.url}</code></li>{/if}
              {#if f.commit}<li>Commit <code>{f.commit}</code></li>{/if}
              {#if f.release}<li>Ships in {f.release}</li>{/if}
              {#if check}<li>Check at <a href={check} target="_blank" rel="noopener noreferrer">{f.verify_url}</a></li>{/if}
              {#if f.verify_after}<li>Checkable after {f.verify_after}</li>{/if}
            </ul>
          {/if}
          {#if f.verdict_at}
            <p class="verdict">
              {f.outcome === "verified" ? "Verified" : "Still broken"}
              <time datetime={f.verdict_at} title={f.verdict_at}>{relativeTime(f.verdict_at)}</time>{#if f.verdict_context?.release?.version}, on {f.verdict_context.release.version}{/if}{#if f.verdict_note}: <q>{f.verdict_note}</q>{/if}
            </p>
          {/if}
        </li>
      {/each}
    </ol>
  {/if}
</section>

<style>
  .fixes h3 { margin: 0 0 8px; font-size: 14px; }
  .n { color: var(--muted); font-weight: 400; font-size: 12px; }
  .muted { color: var(--muted); font-size: 13px; }
  ol { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 10px; }
  .fix { padding: 10px 12px; border: 1px solid var(--border); border-radius: var(--radius-sm); min-width: 0; }
  header { display: flex; flex-wrap: wrap; align-items: baseline; gap: 4px 10px; font-size: 13px; }
  .outcome { padding: 0 7px; border-radius: 999px; font-size: 12px; background: var(--tag-gray-bg); color: var(--tag-gray-fg); border: 1px solid var(--tag-gray-bd); }
  .outcome.pending { background: var(--tag-orange-bg); color: var(--tag-orange-fg); border-color: var(--tag-orange-bd); }
  .outcome.verified { background: var(--tag-green-bg); color: var(--tag-green-fg); border-color: var(--tag-green-bd); }
  .outcome.rejected { background: var(--tag-red-bg); color: var(--tag-red-fg); border-color: var(--tag-red-bd); }
  .sum { margin: 6px 0 0; font-size: 13.5px; white-space: pre-wrap; overflow-wrap: anywhere; }
  .links { margin: 6px 0 0; padding: 0; list-style: none; display: flex; flex-wrap: wrap; gap: 2px 14px; font-size: 12.5px; overflow-wrap: anywhere; min-width: 0; }
  .links li { min-width: 0; }
  code { font: 12px var(--mono); }
  .verdict { margin: 6px 0 0; font-size: 13px; color: var(--muted); overflow-wrap: anywhere; }
</style>
