<script lang="ts">
  // "What's sent" (spec 2026-10-29-feedback-loop §4.5): the exact context
  // JSON the send carries, the recent errors' lines when they are ticked,
  // the deployer's metadata, and whether the screenshot goes.
  import type { DiagLine, ReportContext } from "../../sdk/src/types";
  import { contextPreview } from "./model";

  type Props = { context: ReportContext | null; diagnostics: DiagLine[] | undefined; metadata: unknown; clip: boolean };
  let { context, diagnostics, metadata, clip }: Props = $props();
</script>

<details class="sent">
  <summary>What's sent</summary>
  <p class="small muted">{clip ? "The screenshot, with your marks drawn in." : "No screenshot."}</p>
  <h3>Context</h3>
  {#if context}
    <pre class="context">{contextPreview(context)}</pre>
  {:else}
    <p class="small muted" role="status">Reading the page's context…</p>
  {/if}
  {#if metadata !== undefined}
    <h3>From this site</h3>
    <pre class="metadata">{JSON.stringify(metadata, null, 2)}</pre>
  {/if}
  <h3>Recent errors</h3>
  {#if diagnostics}
    <ol class="diag">
      {#each diagnostics as l, i (i)}
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
  pre, .diag { margin: 0; max-height: 160px; overflow: auto; padding: 6px 8px; background: var(--bg); border: 1px solid var(--border); border-radius: var(--radius-xs); font: 11px/1.4 ui-monospace, SFMono-Regular, Menlo, monospace; white-space: pre-wrap; overflow-wrap: anywhere; }
  .diag { list-style: none; }
  .diag li + li { margin-top: 2px; }
</style>
