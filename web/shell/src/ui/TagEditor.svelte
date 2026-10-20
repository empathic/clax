<svelte:options css="injected" />

<script lang="ts">
  // The artifact's tag editor, under its Edit tags button (ui/tag-editor.ts):
  // every tag there is, the ones it carries checked. Each check or uncheck
  // shows at once and is sent in turn (`PATCH /api/artifacts/<id>/tags`),
  // one after another, so none is lost however fast they come; a refusal
  // (twenty tags at most, say) shows under the list and its box goes back.
  // Each answer goes through the view's ordering (`setTags`), so an older
  // answer never replaces a newer one. The list is fetched again whenever
  // the artifact's tags change, here or elsewhere (a `tags` event). The
  // owner, who keeps the list, is linked to the Tags page.
  import { SvelteMap } from "svelte/reactivity";
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
  // The state asked for each tag whose change is still on its way.
  const asked = new SvelteMap<string, boolean>();
  let queue: Promise<void> = Promise.resolve();
  $effect(() => {
    void changed;
    listTags().then(l => { list = l; }, e => { error = `Could not load the tags: ${failure(e)}`; });
  });
  const has = (t: TagRef) => asked.get(t.id) ?? mine.some(x => x.id === t.id);
  /** Asks for `t` the other way from how it shows, after every change asked before. */
  function toggle(t: TagRef): Promise<void> {
    const on = !has(t);
    asked.set(t.id, on);
    error = null;
    const run = async () => {
      const tt = p.ctl.tagTicket();
      try {
        const r = await changeArtifactTags(p.ctl.id, on ? [t.id] : [], on ? [] : [t.id]);
        if (!p.ctl.disposed) p.ctl.setTags(tt, { tags: r.tags, may: true });
      } catch (e) {
        error = failure(e);
      }
      // Asked again meanwhile: that later change settles it.
      if (asked.get(t.id) === on) asked.delete(t.id);
    };
    queue = queue.then(run);
    return queue;
  }
</script>

<TagPicker tags={list?.tags ?? []} chosen={has} onToggle={toggle} label="Tags" {error} onClose={p.onClose}>
  {#snippet empty()}<p class="tn">{list ? "There are no tags yet." : "Loading…"}{#if list?.may_manage}{" "}<a href="/tags">Make one</a>{/if}</p>{/snippet}
  {#snippet foot()}{#if list?.may_manage && list.tags.length}<a href="/tags">Manage tags</a>{/if}{/snippet}
</TagPicker>
