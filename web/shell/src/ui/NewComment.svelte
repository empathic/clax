<svelte:options css="injected" />

<script lang="ts">
  // The threads panel's New comment box: a comment on the whole artifact,
  // not a spot on it. Pressing it opens the usual composer (Composer.svelte)
  // in place, on the version shown, posting through `host` (`postWhole`);
  // posting closes it, and Cancel or Escape too. It loads apart from the
  // sidebar, with whole-comment.ts (Sidebar.svelte imports it when given
  // `compose`).
  import Composer from "./Composer.svelte";
  import { WHOLE_LABEL, type WholeHost, postWhole, wholeDraft } from "../view/whole-comment";

  // `onText` hears the composer's text as it is typed, and "" as it closes.
  let { version, host, onText }: { version: number; host: WholeHost; onText?(text: string): void } = $props();
  let open = $state(false);
  let opener: HTMLButtonElement | undefined = $state();
  const draft = $derived(wholeDraft(version, `whole-${version}`));
  // Focus goes back to the box when the composer closes.
  const close = () => { open = false; queueMicrotask(() => opener?.focus()); };
</script>

<div class="new-comment">
  {#if open}
    <Composer {draft} {onText} onCancel={close} onSubmit={async body => { await postWhole(host, version, body); close(); }} />
  {:else}
    <button type="button" class="new-comment-open" bind:this={opener} aria-label={`New comment on ${WHOLE_LABEL.toLowerCase()}`}
      onclick={() => { open = true; }}><span class="plus" aria-hidden="true">+</span>New comment<span class="muted small">on the whole artifact</span></button>
  {/if}
</div>

<style>
  :global {
    .new-comment { margin: 0 0 10px; }
    .new-comment .composer { position: static; width: auto; box-shadow: none; z-index: auto; border-radius: var(--radius); border-width: 1px; border-top-width: 3px; }
    .new-comment .composer > p.muted.small { margin: 0 0 8px; }
    .new-comment-open { display: flex; align-items: baseline; gap: 8px; width: 100%; text-align: left; padding: 8px 10px; border: 1px dashed var(--border-strong); border-radius: var(--radius); background: transparent; color: var(--fg); font: 500 13px/1.3 var(--font); cursor: text; }
    .new-comment-open:hover { border-color: var(--you); background: var(--raised); }
    .new-comment-open .plus { color: var(--you); font-weight: 600; }
    .new-comment-open .muted { margin-left: auto; }
    @media (max-width: 700px) { .new-comment-open { min-height: 40px; align-items: center; } }
  }
</style>
