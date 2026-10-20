<svelte:options css="injected" />

<script lang="ts">
  // The Tags page (`/tags`): every tag, by name, with its colour, its
  // description and how many artifacts the viewer sees carry it; a tag
  // opens the gallery filtered by it. The owner (`may_manage`) makes one
  // with New tag, edits one in place, and deletes one after confirming how
  // many artifacts carry it, which removes it from all of them; anyone else
  // reads. The list is fetched again on the `tags` event, and when the
  // stream comes back.
  import { onMount } from "svelte";
  import type { EventStream } from "../stream";
  import { COLOURS } from "../view/tags-model";
  import { type TagFields, type TagList, type TagView, createTag, deleteTag, failure, listTags, updateTag } from "../view/tags-api";
  import TagForm from "./TagForm.svelte";

  const p: { stream: EventStream } = $props();
  let list = $state<TagList | null>(null);
  let error = $state<string | null>(null);
  let status = $state("");
  // The form open: a tag's ID, "new", or none.
  let editing = $state<string | null>(null);
  const may = $derived(!!list?.may_manage);
  const colours = $derived(list?.colours.length ? list.colours : COLOURS);
  const load = () => listTags().then(l => { list = l; error = null; }, e => { error = `Could not load the tags: ${failure(e)}`; });
  const href = (t: TagView) => `/?tag=${encodeURIComponent(t.name)}`;
  const count = (n: number) => `${n} artifact${n === 1 ? "" : "s"}`;
  const cap = (c: string) => c[0].toUpperCase() + c.slice(1);

  /** Saves the form's fields (a new tag, or `t`'s); the refusal's message, or null once saved. */
  async function save(f: TagFields, t?: TagView): Promise<string | null> {
    try {
      if (t) await updateTag(t.id, f);
      else await createTag(f);
    } catch (e) {
      return failure(e);
    }
    status = t ? `Saved ${f.name}.` : `Made ${f.name}.`;
    editing = null;
    await load();
    return null;
  }
  async function remove(t: TagView) {
    const on = t.count ? `It is on ${count(t.count)}; deleting it removes it from ${t.count === 1 ? "it" : "all of them"}.` : "No artifact carries it.";
    if (!confirm(`Delete the tag "${t.name}"? ${on}`)) return;
    try {
      const r = await deleteTag(t.id);
      status = `Deleted ${t.name}${r.artifacts ? `, and took it off ${count(r.artifacts)}` : ""}.`;
    } catch (e) {
      error = `Could not delete ${t.name}: ${failure(e)}`;
    }
    await load();
  }

  onMount(() => {
    document.title = "Tags · Clax";
    void load();
    return p.stream.watch(["gallery"], e => { if (e.type === "tags" || e.type === "ready" || e.type === "resync") void load(); });
  });
</script>

<section class="tagsp" aria-labelledby="tags-h">
  <header class="th">
    <h2 id="tags-h">Tags</h2>
    {#if list}<span class="tc">{list.tags.length}</span>{/if}
    <span class="sp"></span>
    {#if may && editing !== "new"}<button type="button" class="primary" onclick={() => { editing = "new"; status = ""; }}>New tag</button>{/if}
  </header>
  {#if list}<p class="tsub">{may ? "Tags sort artifacts in the gallery and the inbox. You, the people who made an artifact, and their agents put them on it." : "The owner keeps this list. You can put these tags on the artifacts you made."}</p>{/if}
  {#if error}<p class="terr" role="alert">{error}</p>{/if}
  <p class="tstat" role="status">{status}</p>
  {#if editing === "new"}<TagForm {colours} onSave={f => save(f)} onCancel={() => { editing = null; }} />{/if}
  {#if list && list.tags.length}
    <ul class="tlist">
      {#each list.tags as t (t.id)}
        <li class="trow">
          {#if editing === t.id}
            <TagForm tag={t} {colours} onSave={f => save(f, t)} onCancel={() => { editing = null; }} />
          {:else}
            <a class="tag" data-c={t.colour} href={href(t)} title={`The gallery, filtered by tag ${t.name}`}>{t.name}</a>
            <span class="tdesc">{t.description ?? ""}</span>
            <span class="tmeta">{cap(t.colour)} · <a href={href(t)}>{count(t.count)}</a></span>
            {#if may}
              <span class="tact">
                <button type="button" class="ghost" aria-label={`Edit tag ${t.name}`} onclick={() => { editing = t.id; status = ""; }}>Edit</button>
                <button type="button" class="ghost tdel" aria-label={`Delete tag ${t.name}`} onclick={() => void remove(t)}>Delete</button>
              </span>
            {/if}
          {/if}
        </li>
      {/each}
    </ul>
  {:else if list}
    <p class="tnone">There are no tags yet.{may ? " Make one with New tag." : ""}</p>
  {:else if !error}
    <p class="tnone">Loading…</p>
  {/if}
</section>

<style>
  .tagsp { max-width: 880px; margin: 0 auto; min-width: 0; }
  .th { display: flex; align-items: center; gap: 8px 12px; padding-bottom: 12px; border-bottom: 1px solid var(--border); }
  .th h2 { margin: 0; font-size: 22px; line-height: 1.2; }
  .tc { color: var(--muted); font: 400 13px var(--mono); }
  .sp { flex: 1; }
  .tsub { margin: 10px 0 0; color: var(--muted); font-size: 13px; }
  .terr { margin: 12px 0 0; color: var(--danger); font-size: 13px; }
  .tstat { margin: 8px 0; min-height: 1.5em; color: var(--muted); font-size: 13px; }
  .tlist { list-style: none; margin: 0; padding: 0; border-top: 1px solid var(--border); }
  .trow { display: grid; grid-template-columns: minmax(0, max-content) minmax(0, 1fr) auto auto; align-items: center; gap: 4px 14px; padding: 12px 4px; border-bottom: 1px solid var(--border); }
  .trow > :global(.tform) { grid-column: 1 / -1; }
  .trow > .tag { font-size: 13px; line-height: 20px; padding: 1px 10px; }
  .tdesc { color: var(--fg); font-size: 13.5px; overflow-wrap: anywhere; }
  .tmeta { color: var(--muted); font-size: 12.5px; white-space: nowrap; }
  .tmeta a { color: inherit; text-decoration: underline; }
  .tmeta a:hover { color: var(--fg); }
  .tact { display: flex; gap: 2px; }
  .tact button { min-height: 28px; padding: 2px 8px; color: var(--muted); }
  .tact .tdel:hover, .tact .tdel:focus-visible { color: var(--danger); }
  .tnone { color: var(--muted); text-align: center; padding: 48px 0; }
  @media (max-width: 700px) {
    .trow { grid-template-columns: minmax(0, 1fr) auto; }
    .trow > .tag { justify-self: start; }
    .tdesc { grid-column: 1 / -1; grid-row: 2; }
    .tdesc:empty { display: none; }
    .tmeta { grid-column: 1; grid-row: 3; }
    .tact { grid-column: 2; grid-row: 1; }
  }
</style>
