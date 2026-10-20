<svelte:options css="injected" />

<script lang="ts">
  // A tag's form on the Tags page: its name, its colour (the palette's keys
  // as radio buttons, each showing its name on its own tint) and its
  // description, with a preview. `tag` is the tag edited; none for a new
  // one. Save hands `onSave` the fields, which answers with the daemon's
  // refusal, if any, shown in the form.
  import { onMount } from "svelte";
  import type { TagFields, TagView } from "../view/tags-api";

  const p: { tag?: TagView; colours: readonly string[]; onSave(f: TagFields): Promise<string | null>; onCancel(): void } = $props();
  // svelte-ignore state_referenced_locally
  let name = $state(p.tag?.name ?? "");
  // svelte-ignore state_referenced_locally
  let colour = $state(p.tag?.colour ?? "gray");
  // svelte-ignore state_referenced_locally
  let description = $state(p.tag?.description ?? "");
  let error = $state<string | null>(null);
  let saving = $state(false);
  let first: HTMLInputElement | undefined = $state();
  const id = `tf-${Math.random().toString(36).slice(2, 8)}`;
  const label = (c: string) => c[0].toUpperCase() + c.slice(1);

  async function save(e: SubmitEvent) {
    e.preventDefault();
    saving = true;
    error = await p.onSave({ name: name.trim(), colour, description: description.trim() });
    saving = false;
  }
  function key(e: KeyboardEvent) { if (e.key === "Escape") { e.preventDefault(); p.onCancel(); } }
  onMount(() => first?.focus());
</script>

<!-- Escape anywhere in the form cancels it. -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<form class="tform" onsubmit={save} onkeydown={key} aria-label={p.tag ? `Edit tag ${p.tag.name}` : "New tag"}>
  <label class="tf-row" for={`${id}-n`}>Name</label>
  <input id={`${id}-n`} bind:this={first} bind:value={name} required maxlength="40" autocomplete="off" />
  <span class="tf-row" id={`${id}-c`}>Colour</span>
  <div class="tf-cols" role="radiogroup" aria-labelledby={`${id}-c`}>
    {#each p.colours as c (c)}
      <label class="tf-col"><input type="radio" name={`${id}-colour`} value={c} bind:group={colour} /><span class="tag" data-c={c}>{label(c)}</span></label>
    {/each}
  </div>
  <label class="tf-row" for={`${id}-d`}>Description</label>
  <input id={`${id}-d`} bind:value={description} maxlength="200" placeholder="Optional" autocomplete="off" />
  <span class="tf-row">Preview</span>
  <span><span class="tag" data-c={colour}>{name.trim() || "Tag name"}</span></span>
  {#if error}<p class="tf-err" role="alert">{error}</p>{/if}
  <div class="tf-act">
    <button type="button" onclick={p.onCancel}>Cancel</button>
    <button type="submit" class="primary" disabled={saving || !name.trim()}>{p.tag ? "Save" : "Create tag"}</button>
  </div>
</form>

<style>
  .tform { display: grid; grid-template-columns: max-content minmax(0, 1fr); align-items: center; gap: 10px 14px; padding: 14px; background: var(--card); border: 1px solid var(--border-hover); border-radius: var(--radius); }
  .tf-row { font-size: 12.5px; color: var(--muted); }
  .tf-cols { display: flex; flex-wrap: wrap; gap: 6px; }
  .tf-col { position: relative; display: inline-flex; cursor: pointer; }
  /* The radio stays focusable and announced; its tag shows which is chosen. */
  .tf-col input { position: absolute; opacity: 0; width: 1px; height: 1px; margin: 0; min-height: 0; }
  .tf-col input:checked + .tag { border-color: var(--tf); box-shadow: inset 0 0 0 1px var(--tf); }
  .tf-col input:checked + .tag::before { content: "✓ "; }
  .tf-col input:focus-visible + .tag { outline: 2px solid var(--focus); outline-offset: 2px; }
  .tf-err { grid-column: 1 / -1; margin: 0; color: var(--danger); font-size: 13px; }
  .tf-act { grid-column: 1 / -1; display: flex; justify-content: flex-end; gap: 8px; }
  @media (max-width: 480px) {
    .tform { grid-template-columns: minmax(0, 1fr); gap: 6px; }
    .tf-act { margin-top: 6px; }
  }
</style>
