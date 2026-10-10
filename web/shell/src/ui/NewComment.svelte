<svelte:options css="injected" />

<script lang="ts">
  // The threads panel's New comment box: a comment on the whole artifact,
  // not a spot on it. Pressing it opens a composer in place, laid out and
  // keyed like the stage's (Composer.svelte: the target line, the status
  // line, the textarea focused at once, Escape or Cancel to close, Post or
  // Cmd/Ctrl+Enter to post), on the version shown, posting through `host`
  // (`postWhole`). It is its own markup rather than Composer.svelte so that
  // the artifact entry, which holds that component, grows no export for this
  // chunk. It loads apart from the sidebar, with whole-comment.ts
  // (Sidebar.svelte imports it when given `compose`).
  import { isSubmitKey } from "../view/composer-model";
  import { WHOLE_LABEL, WHOLE_NO_CLIP, type WholeHost, postWhole } from "../view/whole-comment";

  let { version, host }: { version: number; host: WholeHost } = $props();
  let open = $state(false);
  let body = $state("");
  let busy = $state(false);
  let opener: HTMLButtonElement | undefined = $state();
  const keys = /Mac|iPhone|iPad|iPod/.test(typeof navigator === "undefined" ? "" : navigator.platform) ? "⌘↵" : "Ctrl+Enter";
  // The viewer types at once: the textarea takes focus as it is rendered.
  const focus = (el: HTMLTextAreaElement) => { el.focus(); };
  // Focus goes back to the box when the composer closes.
  const close = () => { open = false; body = ""; queueMicrotask(() => opener?.focus()); };
  async function post() {
    if (busy || !body.trim()) return;
    busy = true;
    // A failure shows in the notice banner (postWhole); the text stays.
    try { await postWhole(host, version, body); close(); } catch { /* shown */ } finally { busy = false; }
  }
</script>

<div class="new-comment">
  {#if open}
    <form class="composer" aria-label="New comment on the whole artifact" onsubmit={e => { e.preventDefault(); void post(); }}>
      <p class="composer-quote">{WHOLE_LABEL}</p>
      <p class="muted small">No screenshot: {WHOLE_NO_CLIP}</p>
      <textarea {@attach focus} rows={3} placeholder="Comment… (@agent sends it to the agent)" value={body}
        oninput={e => { body = e.currentTarget.value; }}
        onkeydown={e => {
          if (e.key === "Escape") close();
          else if (isSubmitKey(e)) { e.preventDefault(); void post(); }
        }}></textarea>
      <div class="actions">
        <button type="button" onclick={close}>Cancel</button>
        <button type="submit" class="primary" title={`Post comment (${keys})`} disabled={busy || !body.trim()}>Post comment</button>
      </div>
    </form>
  {:else}
    <button type="button" class="new-comment-open" bind:this={opener} aria-label="New comment on the whole artifact"
      onclick={() => { open = true; }}><span class="plus" aria-hidden="true">+</span>New comment<span class="muted small">on the whole artifact</span></button>
  {/if}
</div>

<style>
  :global {
    .new-comment { margin: 0 0 10px; }
    .new-comment .composer { position: static; width: auto; box-shadow: none; z-index: auto; border-radius: var(--radius); border-width: 1px; border-top-width: 3px; }
    .new-comment .composer .muted.small { margin: 0 0 8px; }
    .new-comment-open { display: flex; align-items: baseline; gap: 8px; width: 100%; text-align: left; padding: 8px 10px; border: 1px dashed var(--border-strong); border-radius: var(--radius); background: transparent; color: var(--fg); font: 500 13px/1.3 var(--font); cursor: text; }
    .new-comment-open:hover { border-color: var(--you); background: var(--raised); }
    .new-comment-open .plus { color: var(--you); font-weight: 600; }
    .new-comment-open .muted { margin-left: auto; }
    @media (max-width: 700px) { .new-comment-open { min-height: 40px; align-items: center; } }
  }
</style>
