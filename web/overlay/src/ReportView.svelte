<script lang="ts">
  // One report, opened in "Your reports" (spec 2026-10-29-feedback-loop
  // §6.3): its conversation, its fix attempts with their outcomes, and, while
  // a fix waits for the reporter's verdict, that fix's summary, its change
  // and where to check it, with "It's fixed" and "Still broken". A comment
  // never changes the report's state; only a verdict does. "Still broken"
  // hands over to the overlay, which asks for a note and offers a new capture.
  // While a fix waits, a reply reaches the agent that proposed it while its
  // session lives and its person is a member of the queue (that agent may
  // answer), and the queue's people always; a person's fix is answered by
  // the queue's people alone. The reply box says who will hear it.
  import type { Report } from "../../sdk/src/types";
  import { OUTCOME, anchorText, browserOf, failureText, filedByText, repliesOf, safeLink, timeIn, viewportText, waitingFix } from "./model";

  // `full`: the full view (Reports' "Full view") adds what the panel leaves out, in the shell's own report page's
  // order: who filed it and when (and who claimed it); the clip, at Fit, 1x or 2x; the note, then "On <anchor>" and
  // the page link; "Conversation" headed, replies alone (the note is above); and, last, "Context" (Captured,
  // Browser, Viewport and the page the reporter's browser was actually on at capture).
  type Props = { report: Report; full?: boolean; onReply(id: string, body: string): Promise<void>; onVerify(id: string): Promise<void>; onStillBroken(r: Report): void; onClipUrl?(id: string): Promise<string> };
  let { report: r, full = false, onReply, onVerify, onStillBroken, onClipUrl }: Props = $props();
  const when = (iso: string) => { const d = new Date(iso); return Number.isNaN(d.getTime()) ? iso : d.toLocaleString(); };
  const page = $derived(safeLink(r.artifact?.url));
  const note = $derived(r.comments[0] ?? null);
  const convo = $derived(full ? repliesOf(r) : r.comments);
  const anchorLine = $derived(anchorText(r.anchor));
  const ctx = $derived(r.context);
  const env = $derived(ctx?.environment);
  const capturedLine = $derived(ctx ? timeIn(ctx.captured_at) : null);
  const browserLine = $derived([browserOf(env?.user_agent), env?.platform].filter(Boolean).join(" on ") || null);
  const viewportLine = $derived(viewportText(env?.viewport, env?.color_scheme));
  const ctxPageLink = $derived(safeLink(env?.url));
  const hasCtxPage = $derived(!!(env?.url || env?.title || env?.route));

  let text = $state("");
  let sending = $state(false);
  let error = $state<string | null>(null);
  let verifying = $state(false);

  const waiting = $derived(waitingFix(r));
  const past = $derived((r.fixes ?? []).filter(f => f !== waiting));

  // ---- The clip, Full view alone: fetched once per report (`clipKey`
  // changes value only when the id, `full` or whether a clip can be had
  // changes), never refetched by "Your reports" 15 s poll replacing the
  // report object with an equal one. ----
  const clipKey = $derived(full && r.has_clip && onClipUrl ? r.id : null);
  let clipSrc = $state<string | null>(null);
  let clipFailed = $state(false);
  let zoom = $state<"fit" | 1 | 2>("fit");
  let natural = $state<{ w: number; h: number } | null>(null);
  const dpr = $derived(env?.viewport?.dpr || 1);
  const clipWidth = $derived(zoom !== "fit" && natural ? `${(natural.w / dpr) * zoom}px` : null);
  $effect(() => {
    const id = clipKey;
    clipSrc = null;
    clipFailed = false;
    natural = null;
    zoom = "fit";
    if (!id || !onClipUrl) return;
    let cancelled = false;
    let got: string | null = null;
    onClipUrl(id).then(
      u => { if (cancelled) { URL.revokeObjectURL(u); return; } got = u; clipSrc = u; },
      () => { if (!cancelled) clipFailed = true; },
    );
    return () => { cancelled = true; if (got) URL.revokeObjectURL(got); };
  });

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

<div class="thread" class:full>
  {#if full}
    <p class="by small">
      Filed by <b>{filedByText(r)}</b> · <time datetime={r.created_at}>{when(r.created_at)}</time>
      {#if r.claim}· claimed by {r.claim.holder_name}{/if}
    </p>
    {#if r.has_clip && onClipUrl}
      <figure class="clip">
        {#if clipSrc}
          <div class="zoom" role="group" aria-label="Clip size">
            {#each [["fit", "Fit"], [1, "1×"], [2, "2×"]] as const as [z, label] (z)}
              <button type="button" class="ghost small" aria-pressed={zoom === z} onclick={() => { zoom = z; }}>{label}</button>
            {/each}
          </div>
          <div class="clipbox" class:fit={zoom === "fit"}>
            <img src={clipSrc} alt={anchorLine ?? "The reporter's screenshot"} style:width={clipWidth}
              onload={e => { const i = e.currentTarget as HTMLImageElement; natural = { w: i.naturalWidth, h: i.naturalHeight }; }} />
          </div>
        {:else if clipFailed}
          <p class="small muted">The screenshot could not be loaded.</p>
        {:else}
          <p class="small muted">Loading the screenshot…</p>
        {/if}
      </figure>
    {/if}
    <section class="note" aria-label="Note">
      {#if note}<p class="body">{note.body}</p>{/if}
      {#if anchorLine}<p class="small muted anchor">On {anchorLine}</p>{/if}
      {#if r.artifact?.title || page}
        <p class="small muted where">{#if page}<a href={page} target="_blank" rel="noopener noreferrer">{r.artifact.title || page}</a>{:else}{r.artifact?.title}{/if}</p>
      {/if}
    </section>
    <h5>Conversation</h5>
  {/if}
  {#each convo as c (c.id)}
    <div class="c" data-kind={c.author_kind}>
      <p class="who small">{c.author_kind === "reporter" ? "You" : c.author_name}{#if full} · <time datetime={c.created_at}>{when(c.created_at)}</time>{/if}</p>
      <p class="body">{c.body}</p>
    </div>
  {:else}
    {#if full}<p class="small muted">No replies yet.</p>{/if}
  {/each}

  {#each past as f (f.n)}
    {@const change = full ? safeLink(f.url) : null}
    {@const check = full ? safeLink(f.verify_url) : null}
    <div class="fix" data-outcome={f.outcome}>
      <p class="small"><strong>Fix {f.n}</strong> · {OUTCOME[f.outcome]}{#if full && f.proposer_name} · from {f.proposer_name}{/if}</p>
      {#if full && f.summary}<p class="body">{f.summary}</p>{/if}
      {#if change || check}
        <p class="links small">
          {#if check}<a href={check} target="_blank" rel="noopener noreferrer">Where it was checked</a>{/if}
          {#if change}<a href={change} target="_blank" rel="noopener noreferrer">The change</a>{/if}
        </p>
      {/if}
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
      {#if waiting && waiting.proposer_kind !== "person"}
        <p class="reply-note small muted">A reply goes to {waiting.proposer_name} while they are still at work, and to the queue's people. If the fix did not work, press Still broken.</p>
      {:else if waiting}
        <p class="reply-note small muted">A reply goes to the queue's people, {waiting.proposer_name} among them. If the fix did not work, press Still broken.</p>
      {/if}
      <div class="actions"><button type="submit" class="primary" disabled={sending || !text.trim()}>{sending ? "Sending…" : "Reply"}</button></div>
    </form>
  {/if}

  {#if full && ctx}
    <section class="ctx" aria-label="Context">
      <h5>Context</h5>
      <dl>
        {#if capturedLine}<dt>Captured</dt><dd>{capturedLine}</dd>{/if}
        {#if browserLine}<dt>Browser</dt><dd title={env?.user_agent}>{browserLine}</dd>{/if}
        {#if viewportLine}<dt>Viewport</dt><dd>{viewportLine}</dd>{/if}
        {#if hasCtxPage}
          <dt>Page</dt>
          <dd>{#if env?.title}{env.title}{/if}{#if ctxPageLink}{" "}<a href={ctxPageLink} target="_blank" rel="noopener noreferrer">{env?.url}</a>{:else if env?.url}{" "}{env.url}{/if}{#if env?.route}{" "}<code>{env.route}</code>{/if}</dd>
        {/if}
      </dl>
    </section>
  {/if}
</div>

<style>
  .thread { display: flex; flex-direction: column; gap: 8px; padding: 0 14px 12px; }
  .thread.full { gap: 12px; font-size: 15px; }
  .by { margin: 0; color: var(--muted); }
  .by b { color: var(--fg); font-weight: 600; }
  .clip { margin: 0; min-width: 0; }
  .zoom { display: flex; gap: 2px; margin-bottom: 6px; }
  .zoom button[aria-pressed="true"] { background: var(--hover); color: var(--fg); font-weight: 600; }
  .clipbox { max-height: 60vh; overflow: auto; border: 1px solid var(--border); border-radius: var(--radius-sm); background: var(--bg); }
  .clipbox img { display: block; height: auto; }
  .clipbox.fit img { width: 100%; }
  .note { display: flex; flex-direction: column; gap: 2px; }
  .anchor, .where { margin: 0; }
  .thread.full h5 { margin: 6px 0 0; font: 600 13px/1.3 var(--font); color: var(--muted); text-transform: uppercase; letter-spacing: .03em; }
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
  .reply-note { margin-top: 4px; }
  .actions { display: flex; justify-content: flex-end; margin-top: 6px; }
  .error { color: var(--danger); }
  .ctx dl { display: grid; grid-template-columns: max-content minmax(0, 1fr); gap: 4px 12px; margin: 0; font-size: 13px; }
  .ctx dt { color: var(--muted); }
  .ctx dd { margin: 0; overflow-wrap: anywhere; }
  .ctx code { font: 12px var(--mono, ui-monospace, monospace); }
</style>
