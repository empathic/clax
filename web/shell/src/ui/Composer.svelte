<script lang="ts">
  // The composer for `draft`; `onText` hears the typed text on every input,
  // and "" when it closes; `onFocused` hears that its textarea took focus as
  // it opened.
  import { onDestroy, untrack } from "svelte";
  import { type Anchor, INDEX_FILE } from "../../../bridge/src/protocol";
  import { isSubmitKey, submitKeysLabel } from "../comments";
  import { type Draft, composerQuote } from "../view/composer-model";

  type Props = { draft: Draft; onCancel(): void; onSubmit(body: string): Promise<void>; onText?(text: string): void; onFocused?(): void };
  let { draft, onCancel, onSubmit, onText, onFocused }: Props = $props();
  const uid = $props.id();
  let body = $state("");
  let busy = $state(false);
  let clipUrl = $state<string | null>(null);
  $effect(() => {
    const clip = draft.clip;
    if (!clip) { clipUrl = null; return; }
    const u = URL.createObjectURL(clip);
    clipUrl = u;
    return () => URL.revokeObjectURL(u);
  });
  // The composer is keyed by its pick: closing it tells the owner the text is gone.
  onDestroy(() => onText?.(""));

  let textarea: HTMLTextAreaElement | undefined;
  // The viewer types at once: focus moves from the page to the textarea as
  // the composer is first rendered, before the next paint (a script focus,
  // not the viewer's input to the shell), so the hand-off from the page is as
  // short as it can be; then the owner hears it.
  const focus = (el: HTMLTextAreaElement) => {
    textarea = el;
    el.focus();
    untrack(() => onFocused?.());
  };

  const canPost = $derived(!busy && !!body.trim() && !draft.capturing);
  // Set before the first await, so a second Post or shortcut in the same
  // turn cannot post twice.
  let posting = false;
  async function post() {
    if (!canPost || posting) return;
    posting = true;
    busy = true;
    // `onSubmit` reports a failure in the stage's notice banner and rethrows;
    // the draft stays so the viewer can retry.
    try { await onSubmit(body); } catch { posting = false; busy = false; }
  }

  // Post (a click, Enter on it, or the shortcut) while the screenshot is
  // still being taken posts once it is in (or the wait for it ends): the
  // composer opens at the pick, so a viewer who types at once can post before
  // then. It posts the text the textarea showed when it was asked, on the
  // anchor shown then: an edit after it, or a move of the composer to
  // another anchor (a page's area), cancels it, and the viewer posts again.
  let queued = $state(false);
  let queuedFor: Anchor | null = null;
  $effect(() => {
    const { anchor, capturing } = draft;
    if (!queued) return;
    untrack(() => {
      // Only on the anchor shown when it was asked.
      if (queuedFor !== anchor) { queued = false; return; }
      if (capturing) return;
      queued = false;
      // And only with the text shown then.
      if (textarea?.value === body) void post();
    });
  });
  /** Posts now, or once the screenshot is in. */
  function request() {
    if (!draft.capturing) { void post(); return; }
    if (busy || !body.trim()) return;
    queuedFor = draft.anchor;
    queued = true;
  }
  const status = $derived(draft.capturing
    ? queued ? "Posting once the screenshot is taken…" : "Taking the screenshot…"
    : `No screenshot${draft.clipError ? `: ${draft.clipError}` : ""}`);
</script>

<form class="composer" onsubmit={e => { e.preventDefault(); request(); }}>
  <p class="composer-quote">{composerQuote(draft)}</p>
  {#if draft.anchor.file !== INDEX_FILE}<p class="file-label muted small">on {draft.anchor.file}</p>{/if}
  {#if clipUrl}
    <img class="clip" src={clipUrl} alt="Screenshot of the selected region" />
  {:else}
    <p class="muted small" id={`${uid}-status`} role="status">{status}</p>
  {/if}
  <textarea {@attach focus} rows={3} placeholder="Comment… (@agent sends it to the agent)" value={body}
    oninput={e => { const v = e.currentTarget.value; queued = false; body = v; onText?.(v); }}
    onkeydown={e => {
      if (e.key === "Escape") onCancel();
      else if (isSubmitKey(e)) {
        e.preventDefault();
        request();
      }
    }}></textarea>
  <div class="actions">
    <button type="button" onclick={onCancel}>Cancel</button>
    <!-- While the screenshot is taken, Post stays focusable (Tab order does
         not change) and waits: pressing it queues the post, which the status
         line it is described by announces. -->
    <button type="submit" class="primary" title={draft.capturing ? "Posts once the screenshot is taken" : `Post comment (${submitKeysLabel()})`} disabled={busy || !body.trim()}
      aria-disabled={draft.capturing ? "true" : undefined} aria-describedby={draft.capturing && !clipUrl ? `${uid}-status` : undefined}>Post comment</button>
  </div>
</form>
