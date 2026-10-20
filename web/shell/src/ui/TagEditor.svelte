<svelte:options css="injected" />

<script lang="ts">
  // The artifact's tag editor, under its Edit tags button (ui/tag-editor.ts):
  // every tag there is, the ones it carries checked; checking or unchecking
  // one changes the artifact's tags at once (`PATCH
  // /api/artifacts/<id>/tags`), and a refusal (twenty tags at most, say)
  // shows under the list. The list is fetched again whenever the artifact's
  // tags change, here or elsewhere (a `tags` event). The owner, who keeps
  // the list, is linked to the Tags page.
  import { fromStore } from "svelte/store";
  import type { TagRef } from "../api";
  import type { ArtifactController } from "../view/artifact-controller";
  import { type TagList, changeArtifactTags, failure, listTags } from "../view/tags-api";
  import { tagsOf } from "../view/title-tags";
  import TagPicker from "./TagPicker.svelte";

  const p: { ctl: ArtifactController; onClose(): void } = $props();
  // svelte-ignore state_referenced_locally
  const view = fromStore(p.ctl.state);
  const mine = $derived(tagsOf(view.current).tags);
  const changed = $derived(view.current.tags);
  let list = $state<TagList | null>(null);
  let error = $state<string | null>(null);
  let busy = false;
  $effect(() => {
    void changed;
    listTags().then(l => { list = l; }, e => { error = `Could not load the tags: ${failure(e)}`; });
  });
  const has = (t: TagRef) => mine.some(x => x.id === t.id);
  async function toggle(t: TagRef) {
    if (busy) return;
    busy = true;
    error = null;
    const on = has(t);
    try {
      const r = await changeArtifactTags(p.ctl.id, on ? [] : [t.id], on ? [t.id] : []);
      if (!p.ctl.disposed) p.ctl.set({ tags: { tags: r.tags, may: true } });
    } catch (e) {
      error = failure(e);
    }
    busy = false;
  }
</script>

<TagPicker tags={list?.tags ?? []} chosen={has} onToggle={toggle} label="Tags" {error} onClose={p.onClose}>
  {#snippet empty()}<p class="tn">{list ? "There are no tags yet." : "Loading…"}{#if list?.may_manage}{" "}<a href="/tags">Make one</a>{/if}</p>{/snippet}
  {#snippet foot()}{#if list?.may_manage && list.tags.length}<a href="/tags">Manage tags</a>{/if}{/snippet}
</TagPicker>
