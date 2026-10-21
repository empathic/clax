<svelte:options css="injected" />

<script lang="ts">
  // The Tags page (`/tags`): every tag, by name, with its colour, its
  // description and how many artifacts the viewer sees carry it; a tag
  // opens the gallery filtered by it. The owner (`may_manage`) makes one
  // with New tag, edits one in place, and deletes one after confirming how
  // many artifacts carry it, which removes it from all of them; anyone else
  // reads; the line under the heading says what this viewer may do (the
  // owner; someone who makes artifacts, and so tags their own; or a reader).
  // A refused change says why until the next one. Closing a form returns
  // focus to the button that opened it, and a delete to the next row. The
  // list is fetched again on the `tags` event, and when the stream comes
  // back.
  import { onMount, tick } from "svelte";
  import type { EventStream } from "../stream";
  import { COLOURS } from "../view/tags-model";
  import { type TagFields, type TagList, type TagView, createTag, deleteTag, failure, listTags, updateTag } from "../view/tags-api";
  import TagForm from "./TagForm.svelte";

  const p: { stream: EventStream } = $props();
  let list = $state<TagList | null>(null);
  // Why the list could not load, and why the last change was refused.
  let loadError = $state<string | null>(null);
  let error = $state<string | null>(null);
  // Whether this viewer makes artifacts (`GET /api/viewers/me` `may_create`).
  let mayCreate = $state(false);
  let root: HTMLElement | undefined = $state();
  let status = $state("");
  // The form open: a tag's ID, "new", or none.
  let editing = $state<string | null>(null);
  const may = $derived(!!list?.may_manage);
  const colours = $derived(list?.colours.length ? list.colours : COLOURS);
  const load = () => listTags().then(l => { list = l; loadError = null; }, e => { loadError = `Could not load the tags: ${failure(e)}`; });
  /** Focuses what `sel` finds once the page has drawn, else the New tag button. */
  const focusSoon = (sel: string) => void tick().then(() => (root?.querySelector<HTMLElement>(sel) ?? root?.querySelector<HTMLElement>(".th button"))?.focus());
  /** Closes the form open for `t` (none: New tag's), focus back on what opened it. */
  const closeForm = (t?: TagView) => { editing = null; focusSoon(t ? `li[data-id="${t.id}"] .tedit` : ".th button"); };
  const open = (what: string) => { editing = what; status = ""; error = null; };
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
    await load();
    closeForm(t);
    return null;
  }
  async function remove(t: TagView) {
    const on = t.count ? `It is on ${count(t.count)}; deleting it removes it from ${t.count === 1 ? "it" : "all of them"}.` : "No artifact carries it.";
    if (!confirm(`Delete the tag "${t.name}"? ${on}`)) return;
    error = null;
    const at = list?.tags.findIndex(x => x.id === t.id) ?? -1;
    try {
      const r = await deleteTag(t.id);
      status = `Deleted ${t.name}${r.artifacts ? `, and took it off ${count(r.artifacts)}` : ""}.`;
    } catch (e) {
      status = "";
      error = `Could not delete ${t.name}: ${failure(e)}`;
    }
    await load();
    // Focus on the row now in its place, else the one before it.
    const rows = list?.tags ?? [];
    const next = rows.find(x => x.id === t.id) ?? rows[at] ?? rows[at - 1];
    focusSoon(next ? `li[data-id="${next.id}"] .${next.id === t.id ? "tdel" : "tedit"}` : ".th button");
  }

  onMount(() => {
    document.title = "Tags · Clax";
    void load();
    void fetch("/api/viewers/me").then(r => (r.ok ? r.json() : null)).then((b: { may_create?: boolean } | null) => { mayCreate = !!b?.may_create; }, () => {});
    return p.stream.watch(["gallery"], e => { if (e.type === "tags" || e.type === "ready" || e.type === "resync") void load(); });
  });
</script>

<section class="tagsp" aria-labelledby="tags-h" bind:this={root}>
  <header class="th">
    <h2 id="tags-h">Tags</h2>
    {#if list}<span class="tc">{list.tags.length}</span>{/if}
    <span class="sp"></span>
    {#if may && editing !== "new"}<button type="button" class="primary" onclick={() => open("new")}>New tag</button>{/if}
  </header>
  {#if list}<p class="tsub">{may ? "Tags sort artifacts in the gallery and the inbox. You, the people who made an artifact, and their agents put them on it." : mayCreate ? "The owner keeps this list. You can put these tags on the artifacts you made." : "The owner keeps this list."}</p>{/if}
  {#if loadError || error}<p class="terr" role="alert">{error ?? loadError}</p>{/if}
  <p class="tstat" role="status">{status}</p>
  {#if editing === "new"}<TagForm {colours} onSave={f => save(f)} onCancel={() => closeForm()} />{/if}
  {#if list && list.tags.length}
    <ul class="tlist">
      {#each list.tags as t (t.id)}
        <li class="trow" data-id={t.id}>
          {#if editing === t.id}
            <TagForm tag={t} {colours} onSave={f => save(f, t)} onCancel={() => closeForm(t)} />
          {:else}
            <a class="tag" data-c={t.colour} href={href(t)} title={`The gallery, filtered by tag ${t.name}`}>{t.name}</a>
            <span class="tdesc">{t.description ?? ""}</span>
            <span class="tmeta">{cap(t.colour)} · <a href={href(t)}>{count(t.count)}</a></span>
            {#if may}
              <span class="tact">
                <button type="button" class="ghost tedit" aria-label={`Edit tag ${t.name}`} onclick={() => open(t.id)}>Edit</button>
                <button type="button" class="ghost tdel" aria-label={`Delete tag ${t.name}`} onclick={() => void remove(t)}>Delete</button>
              </span>
            {/if}
          {/if}
        </li>
      {/each}
    </ul>
  {:else if list}
    <p class="tnone">There are no tags yet.{may ? " Make one with New tag." : ""}</p>
  {:else if !loadError}
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
