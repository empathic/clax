<svelte:options css="injected" />

<script lang="ts">
  // A list of tags to check and uncheck, with a search over their names:
  // the artifact's tag editor (`TagEditor.svelte`) and the inbox's Tag
  // filter. The search takes focus when it opens; Down and Up move between
  // it and the tags, Home and End to the first and last tag, Space checks
  // one, Enter in the search checks the only tag it shows, and Escape
  // closes (`onClose`). A check asks for the change (`onToggle`), and once
  // that settles the box shows `chosen` again, so a refused change undoes
  // itself. While it is open no printable key
  // typed in it reaches the shell's keys. Its styles travel with its chunk.
  import { type Snippet, onMount } from "svelte";
  import type { TagRef } from "../api";
  import { matchTags, step } from "../view/tag-picker-model";

  const p: { tags: TagRef[]; chosen(t: TagRef): boolean; onToggle(t: TagRef): void | Promise<void>; label: string; error?: string | null; onClose(): void; empty?: Snippet; foot?: Snippet } = $props();
  let q = $state("");
  let root: HTMLDivElement | undefined = $state();
  const shown = $derived(matchTags(p.tags, q));
  const field = () => root?.querySelector<HTMLInputElement>("input[type=search]");
  const boxes = () => [...(root?.querySelectorAll<HTMLInputElement>("input[type=checkbox]") ?? [])];

  function key(e: KeyboardEvent) {
    if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); p.onClose(); return; }
    const list = boxes();
    const at = list.indexOf(document.activeElement as HTMLInputElement);
    if (e.key === "Enter" && at < 0 && shown.length === 1) { e.preventDefault(); e.stopPropagation(); p.onToggle(shown[0]); return; }
    const to = step(at, list.length, e.key);
    if (to === null) {
      // A printable key (C, ?) is the picker's, not the shell's.
      if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey) e.stopPropagation();
      return;
    }
    e.preventDefault();
    e.stopPropagation();
    (to < 0 ? field() : list[to])?.focus();
  }
  onMount(() => field()?.focus());
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="tpick" bind:this={root} onkeydown={key}>
  <input type="search" class="tq" placeholder="Find a tag" aria-label="Find a tag" autocomplete="off" bind:value={q} />
  {#if shown.length}
    <ul class="tl" role="group" aria-label={p.label}>
      {#each shown as t (t.id)}
        <li><label><input type="checkbox" checked={p.chosen(t)} onchange={async e => { const box = e.currentTarget; await p.onToggle(t); box.checked = p.chosen(t); }} /><span class="tag" data-c={t.colour}>{t.name}</span></label></li>
      {/each}
    </ul>
  {:else if p.tags.length}
    <p class="tn">No tag matches “{q.trim()}”.</p>
  {:else}
    {@render p.empty?.()}
  {/if}
  {#if p.error}<p class="te" role="alert">{p.error}</p>{/if}
  {@render p.foot?.()}
</div>

<style>
  .tpick { display: flex; flex-direction: column; gap: 8px; min-width: 0; }
  .tq { width: 100%; }
  .tl { list-style: none; margin: 0; padding: 0; max-height: min(280px, 50vh); overflow-y: auto; }
  .tl label { display: flex; align-items: center; gap: 8px; padding: 4px 6px; border-radius: var(--radius-xs); cursor: pointer; min-width: 0; }
  .tl label:hover { background: var(--hover); }
  .tl input { flex: none; width: 16px; height: 16px; min-height: 0; margin: 0; accent-color: var(--accent); }
  .tl .tag { min-width: 0; }
  .tpick :global(.tn), .te { margin: 0; font-size: 12.5px; color: var(--muted); }
  .te { color: var(--danger); }
  /* The popovers that hold it: the artifact's tag editor (ui/tag-editor.ts) and the inbox's Tag filter. */
  :global(.tag-pop) { position: fixed; z-index: 30; width: 280px; padding: 10px; background: var(--raised); color: var(--fg); border: 1px solid var(--border-hover); border-radius: var(--radius); box-shadow: var(--elev-lg); font-size: 13px; }
  :global(.tag-pop a) { color: var(--muted); text-decoration: underline; font-size: 12.5px; }
  :global(.tag-pop a:hover) { color: var(--fg); }
</style>
