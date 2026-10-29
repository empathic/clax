<script lang="ts">
  // One report, opened in "Your reports" (spec 2026-10-29-feedback-loop
  // §6.3): its conversation, its fix attempts with their outcomes, and, while
  // a fix waits for the reporter's verdict, that fix's summary, its change
  // and where to check it, with "It's fixed" and "Still broken". A comment
  // never changes the report's state; only a verdict does. "Still broken"
  // hands over to the overlay, which asks for a note and offers a new capture.
  import type { Report } from "../../sdk/src/types";
  import { OUTCOME, failureText, safeLink, waitingFix } from "./model";

  type Props = { report: Report; onReply(id: string, body: string): Promise<void>; onVerify(id: string): Promise<void>; onStillBroken(r: Report): void };
  let { report: r, onReply, onVerify, onStillBroken }: Props = $props();

  let text = $state("");
  let sending = $state(false);
  let error = $state<string | null>(null);
  let verifying = $state(false);

  const waiting = $derived(waitingFix(r));
  const past = $derived((r.fixes ?? []).filter(f => f !== waiting));

  async function reply(): Promise<void> {
    if (sending || !text.trim()) return;
    sending = true;
    error = null;
    try {
      await onReply(r.id, text.trim());
      text = "";
    } catch (e) {
      error = failureText(e);
    } finally {
      sending = false;
    }
  }

  async function verify(): Promise<void> {
    if (verifying) return;
    verifying = true;
    error = null;
    try {
      await onVerify(r.id);
    } catch (e) {
      error = failureText(e);
    } finally {
      verifying = false;
    }
  }
</script>

<div class="thread">
  {#each r.comments as c (c.id)}
    <div class="c" data-kind={c.author_kind}>
      <p class="who small">{c.author_kind === "reporter" ? "You" : c.author_name}</p>
      <p class="body">{c.body}</p>
    </div>
  {/each}

  {#each past as f (f.n)}
    <div class="fix" data-outcome={f.outcome}>
      <p class="small"><strong>Fix {f.n}</strong> · {OUTCOME[f.outcome]}</p>
      {#if f.verdict_note}<p class="small muted">“{f.verdict_note}”</p>{/if}
    </div>
  {/each}

  {#if waiting}
    {@const change = safeLink(waiting.url)}
    {@const check = safeLink(waiting.verify_url)}
    <section class="fix waiting" aria-label="Fix waiting for your check">
      <p class="small"><strong>Fix {waiting.n}</strong> from {waiting.proposer_name}{waiting.release ? `, in ${waiting.release}` : ""}</p>
      <p class="body">{waiting.summary}</p>
      {#if change || check}
        <p class="links small">
          {#if check}<a data-link="verify" href={check} target="_blank" rel="noopener noreferrer">Check it here</a>{/if}
          {#if change}<a data-link="change" href={change} target="_blank" rel="noopener noreferrer">The change</a>{/if}
        </p>
      {/if}
      {#if waiting.verify_after}<p class="small muted">Ready to check from {waiting.verify_after.replace(/^release:/, "release ")}.</p>{/if}
      <div class="verdict">
        <button type="button" onclick={() => onStillBroken(r)}>Still broken</button>
        <button type="button" class="primary" disabled={verifying} onclick={() => void verify()}>{verifying ? "Sending…" : "It's fixed"}</button>
      </div>
    </section>
  {/if}

  {#if error}<p class="error small" role="alert">Not sent: {error}</p>{/if}

  {#if r.status === "open"}
    <form onsubmit={e => { e.preventDefault(); void reply(); }}>
      <textarea rows={2} aria-label="Reply" placeholder="Reply…" bind:value={text}
        onkeydown={e => { if (e.key === "Enter" && (e.metaKey || e.ctrlKey) && !e.isComposing) { e.preventDefault(); void reply(); } }}></textarea>
      <div class="actions"><button type="submit" class="primary" disabled={sending || !text.trim()}>{sending ? "Sending…" : "Reply"}</button></div>
    </form>
  {/if}
</div>

<style>
  .thread { display: flex; flex-direction: column; gap: 8px; padding: 0 14px 12px; }
  .c { padding-left: 8px; border-left: 2px solid var(--border-hover); }
  .c[data-kind="reporter"] { border-left-color: var(--you); }
  .c[data-kind="agent"] { border-left-color: var(--agent-ink); }
  .who { color: var(--muted); font-weight: 600; }
  .body { white-space: pre-wrap; overflow-wrap: anywhere; }
  .fix { padding: 6px 8px; border: 1px solid var(--border); border-radius: var(--radius-xs); }
  .fix.waiting { border-color: var(--agent-ink); background: var(--accent-tint); }
  .links { display: flex; gap: 12px; margin-top: 4px; }
  .links a { color: var(--agent-ink); font-weight: 600; }
  .verdict { display: flex; gap: 6px; justify-content: flex-end; margin-top: 8px; }
  .actions { display: flex; justify-content: flex-end; margin-top: 6px; }
  .error { color: var(--danger); }
</style>
