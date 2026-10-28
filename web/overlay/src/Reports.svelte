<script lang="ts">
  // "Your reports": the reports this reporter may see, newest first, each
  // with its state and its replies, and a box to answer on one that is open.
  // `reports` is null until the first page arrived.
  import type { Report } from "../../sdk/src/types";
  import { failureText, repliesOf, summaryOf } from "./model";

  type Props = { reports: Report[] | null; error: string | null; onReply(id: string, body: string): Promise<void>; onClose(): void };
  let { reports, error, onReply, onClose }: Props = $props();

  let expanded = $state<string | null>(null);
  let text = $state("");
  let sending = $state(false);
  let replyError = $state<string | null>(null);

  const STATE: Record<Report["state"], string> = { open: "Open", claimed: "Being worked on", resolved: "Resolved" };

  function toggle(id: string): void {
    expanded = expanded === id ? null : id;
    text = "";
    replyError = null;
  }

  async function reply(id: string): Promise<void> {
    if (sending || !text.trim()) return;
    sending = true;
    replyError = null;
    try {
      await onReply(id, text.trim());
      text = "";
    } catch (e) {
      replyError = failureText(e);
    } finally {
      sending = false;
    }
  }
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
          <button type="button" class="row" aria-expanded={expanded === r.id} onclick={() => toggle(r.id)}>
            <span class="sum">{summaryOf(r)}</span>
            <span class="meta small"><span class="state" data-state={r.state}>{STATE[r.state]}</span>{replies.length === 1 ? "1 reply" : `${replies.length} replies`}</span>
          </button>
          {#if expanded === r.id}
            <div class="thread">
              {#each r.comments as c (c.id)}
                <div class="c" data-kind={c.author_kind}>
                  <p class="who small">{c.author_kind === "reporter" ? "You" : c.author_name}</p>
                  <p class="body">{c.body}</p>
                </div>
              {/each}
              {#if r.status === "open"}
                <form onsubmit={e => { e.preventDefault(); void reply(r.id); }}>
                  <textarea rows={2} aria-label="Reply" placeholder="Reply…" bind:value={text}
                    onkeydown={e => { if (e.key === "Enter" && (e.metaKey || e.ctrlKey) && !e.isComposing) { e.preventDefault(); void reply(r.id); } }}></textarea>
                  {#if replyError}<p class="error small" role="alert">Not sent: {replyError}</p>{/if}
                  <div class="actions"><button type="submit" class="primary" disabled={sending || !text.trim()}>{sending ? "Sending…" : "Reply"}</button></div>
                </form>
              {/if}
            </div>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</section>

<style>
  /* Placed by the overlay's dock, above its notice and launcher. */
  .panel { width: min(380px, calc(100vw - 32px)); max-height: 560px; overflow: auto; background: var(--raised); border: 1px solid var(--border-hover); border-radius: var(--radius); box-shadow: var(--elev-lg); }
  header { position: sticky; top: 0; display: flex; align-items: center; justify-content: space-between; padding: 6px 6px 6px 14px; background: var(--raised); border-bottom: 1px solid var(--border); }
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
  .state[data-state="resolved"] { background: none; border: 1px solid var(--border-strong); color: var(--muted); }
  .thread { display: flex; flex-direction: column; gap: 8px; padding: 0 14px 12px; }
  .c { padding-left: 8px; border-left: 2px solid var(--border-hover); }
  .c[data-kind="reporter"] { border-left-color: var(--you); }
  .c[data-kind="agent"] { border-left-color: var(--agent-ink); }
  .who { color: var(--muted); font-weight: 600; }
  .body { white-space: pre-wrap; overflow-wrap: anywhere; }
  .actions { display: flex; justify-content: flex-end; margin-top: 6px; }
  form .error { padding: 6px 0 0; }
</style>
