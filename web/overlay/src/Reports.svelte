<script lang="ts">
  // "Your reports": the reports this reporter may see, newest first, each
  // with its state and reply count; the open one shows as `ReportView`.
  // `reports` is null until the first page arrived. Which report is open is
  // the overlay's (`expanded`), so it can open the one waiting on the reporter.
  import type { Report } from "../../sdk/src/types";
  import { repliesOf, stateLabel, summaryOf } from "./model";
  import ReportView from "./ReportView.svelte";

  type Props = {
    reports: Report[] | null;
    error: string | null;
    expanded: string | null;
    onExpand(id: string | null): void;
    onReply(id: string, body: string): Promise<void>;
    onVerify(id: string): Promise<void>;
    onStillBroken(r: Report): void;
    onClose(): void;
  };
  let { reports, error, expanded, onExpand, onReply, onVerify, onStillBroken, onClose }: Props = $props();
</script>

<section class="panel" aria-label="Your reports">
  <header>
    <h2>Your reports</h2>
    <button type="button" class="ghost" onclick={onClose} aria-label="Close your reports">✕</button>
  </header>
  {#if error}<p class="error small" role="alert">{error}</p>{/if}
  {#if reports === null}
    {#if !error}<p class="muted small empty">Loading…</p>{/if}
  {:else if reports.length === 0}
    <p class="muted small empty">No reports yet. Press Report and pick what on the page your report is about.</p>
  {:else}
    <ul>
      {#each reports as r (r.id)}
        {@const replies = repliesOf(r)}
        <li class:open={expanded === r.id}>
          <button type="button" class="row" aria-expanded={expanded === r.id} onclick={() => onExpand(expanded === r.id ? null : r.id)}>
            <span class="sum">{summaryOf(r)}</span>
            <span class="meta small"><span class="state" data-state={r.state}>{stateLabel(r)}</span>{replies.length === 1 ? "1 reply" : `${replies.length} replies`}</span>
          </button>
          {#if expanded === r.id}
            <ReportView report={r} {onReply} {onVerify} {onStillBroken} />
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</section>

<style>
  /* Placed by the overlay's dock, above its notice and launcher. */
  .panel { width: min(380px, calc(100vw - 32px)); max-height: 560px; overflow: auto; background: var(--raised); border: 1px solid var(--border-hover); border-radius: var(--radius); box-shadow: var(--elev-lg); }
  header { position: sticky; top: 0; z-index: 1; display: flex; align-items: center; justify-content: space-between; padding: 6px 6px 6px 14px; background: var(--raised); border-bottom: 1px solid var(--border); }
  h2 { margin: 0; font: 600 15px/1.3 var(--font); }
  .empty { padding: 16px 14px; }
  .error { padding: 6px 14px 0; color: var(--danger); }
  ul { list-style: none; margin: 0; padding: 0; }
  li { border-bottom: 1px solid var(--border); }
  li:last-child { border-bottom: 0; }
  .row { display: flex; flex-direction: column; align-items: stretch; gap: 2px; width: 100%; min-height: 0; padding: 10px 14px; border: 0; border-radius: 0; background: none; text-align: left; white-space: normal; }
  .sum { overflow-wrap: anywhere; }
  .meta { display: flex; gap: 8px; align-items: center; color: var(--muted); font-weight: 400; }
  .state { padding: 0 6px; border-radius: 999px; background: var(--hover); color: var(--fg); font-weight: 500; }
  .state[data-state="claimed"] { background: var(--accent-tint); color: var(--agent-ink); }
  .state[data-state="fix_proposed"] { background: var(--you); color: var(--on-you); }
  .state[data-state="verified"] { background: var(--accent-tint); color: var(--agent-ink); }
  .state[data-state="resolved"] { background: none; border: 1px solid var(--border-strong); color: var(--muted); }
</style>
