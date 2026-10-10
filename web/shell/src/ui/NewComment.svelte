<svelte:options css="injected" />

<script lang="ts">
  // The threads panel's New comment box: a comment on the whole artifact,
  // not a spot on it. Pressing it opens the usual composer in place, on the
  // version shown; posting closes it, Cancel or Escape too. It loads apart
  // from the sidebar (Sidebar.svelte imports it when `onCompose` is given).
  import Composer from "./Composer.svelte";
  import { WHOLE_LABEL, wholeDraft } from "../view/whole-comment";

  let { version, onPost }: { version: number; onPost(body: string): Promise<unknown> } = $props();
  let open = $state(false);
  let opener: HTMLButtonElement | undefined = $state();
  const draft = $derived(wholeDraft(version, `whole-${version}`));
  // Focus goes back to the box when the composer closes.
  const close = () => { open = false; queueMicrotask(() => opener?.focus()); };
</script>

<div class="new-comment">
  {#if open}
    <Composer {draft} onCancel={close} onSubmit={async body => { await onPost(body); close(); }} />
  {:else}
    <button type="button" class="new-comment-open" bind:this={opener} aria-label={`New comment on ${WHOLE_LABEL.toLowerCase()}`}
      onclick={() => { open = true; }}><span class="plus" aria-hidden="true">+</span>New comment<span class="muted small">on the whole artifact</span></button>
  {/if}
</div>

<style>
  :global {
    .new-comment { margin: 0 0 10px; }
    .new-comment .composer { position: static; width: auto; box-shadow: none; z-index: auto; border-radius: var(--radius); border-width: 1px; border-top-width: 3px; }
    .new-comment-open { display: flex; align-items: baseline; gap: 8px; width: 100%; text-align: left; padding: 8px 10px; border: 1px dashed var(--border-strong); border-radius: var(--radius); background: transparent; color: var(--fg); font: 500 13px/1.3 var(--font); cursor: text; }
    .new-comment-open:hover { border-color: var(--you); background: var(--raised); }
    .new-comment-open .plus { color: var(--you); font-weight: 600; }
    .new-comment-open .muted { margin-left: auto; }
    @media (max-width: 700px) { .new-comment-open { min-height: 40px; align-items: center; } }
  }
</style>
