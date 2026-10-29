<script lang="ts">
  // "What's sent" (spec 2026-10-29-feedback-loop §4.5), read from the very
  // values a send carries (`reportOf`, `reopenOf`): the page, and where on it
  // the report points (a summary, then the whole anchor as its JSON), the exact context JSON, the deployer's metadata, the
  // recent errors' lines when they are ticked, and whether the screenshot goes.
  import type { NewReport, Reopening } from "../../sdk/src/reporter";
  import { contextPreview } from "./model";

  type Props = { sent: NewReport | Reopening; marked: boolean };
  let { sent, marked }: Props = $props();

  const page = $derived("url" in sent ? sent : null);
  /** The anchor's fields that say where it points, those it has. */
  const where = $derived.by(() => {
    const a = (page?.anchor ?? null) as Record<string, unknown> | null;
    if (!a) return [];
    const rows: [string, string][] = [["Kind", String(a.kind)]];
    for (const [k, label] of [["selector", "Selector"], ["name", "Name"], ["quote", "Text"], ["prefix", "Text before"], ["suffix", "Text after"]] as const) {
      if (typeof a[k] === "string" && a[k] !== "") rows.push([label, a[k] as string]);
    }
    return rows;
  });
</script>

<details class="sent">
  <summary>What's sent</summary>
  <p class="small muted">{sent.clip ? (marked ? "The screenshot, with your marks drawn in." : "The screenshot.") : "No screenshot."}</p>
  {#if page}
    <h3>Page</h3>
    <dl class="page">
      <dt>Title</dt><dd>{page.title}</dd>
      <dt>Address</dt><dd>{page.url}</dd>
      {#if page.externalId !== undefined}<dt>Artifact</dt><dd>{page.externalId}</dd>{/if}
      {#each where as [k, v] (k)}<dt>{k}</dt><dd>{v}</dd>{/each}
    </dl>
    <h3>Where it points</h3>
    <pre class="anchor">{JSON.stringify(page.anchor, null, 2)}</pre>
  {/if}
  <h3>Context</h3>
  {#if sent.context}
    <pre class="context">{contextPreview(sent.context)}</pre>
  {:else}
    <p class="small muted" role="status">Reading the page's context…</p>
  {/if}
  {#if page && page.metadata !== undefined}
    <h3>From this site</h3>
    <pre class="metadata">{JSON.stringify(page.metadata, null, 2)}</pre>
  {/if}
  <h3>Recent errors</h3>
  {#if sent.diagnostics}
    <ol class="diag">
      {#each sent.diagnostics as l, i (i)}
        <li><span class="muted">{l.at} {l.level} {l.kind}</span> {l.text}</li>
      {/each}
    </ol>
  {:else}
    <p class="small muted">Recent errors are not sent.</p>
  {/if}
</details>

<style>
  .sent { margin-top: 8px; font-size: 12px; }
  summary { cursor: pointer; font-weight: 600; color: var(--muted); }
  h3 { margin: 8px 0 2px; font: 600 12px/1.3 var(--font); }
  .page { display: grid; grid-template-columns: max-content 1fr; gap: 2px 8px; margin: 0; }
  .page dt { color: var(--muted); }
  .page dd { margin: 0; overflow-wrap: anywhere; }
  pre, .diag { margin: 0; max-height: 160px; overflow: auto; padding: 6px 8px; background: var(--bg); border: 1px solid var(--border); border-radius: var(--radius-xs); font: 11px/1.4 ui-monospace, SFMono-Regular, Menlo, monospace; white-space: pre-wrap; overflow-wrap: anywhere; }
  .diag { list-style: none; }
  .diag li + li { margin-top: 2px; }
</style>
