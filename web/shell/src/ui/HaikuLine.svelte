<script lang="ts">
  // One haiku, loaded after first paint: the list is its own file, fetched
  // rather than imported, so the gallery's eager bundle carries neither the
  // list nor a module preloader. Never in comment mode, never animated: the
  // callers decide where it shows.
  // A list already in (`haiku-list.ts`) gives the text at the first render.
  import { pickHaiku } from "../view/haiku";
  import { haikuList, haikuNow } from "../view/haiku-list";

  const p: { seed?: string; footer?: boolean } = $props();
  const known = haikuNow();
  // svelte-ignore state_referenced_locally
  let text = $state<string | null>(known ? pickHaiku(known, p.seed) : null);
  $effect(() => { if (!text) void haikuList().then(list => { if (list) text = pickHaiku(list, p.seed); }); });
</script>

{#if text}
  {#if p.footer}
    <footer class="gfoot"><pre>{text}</pre><span>clax haiku · a new one each visit</span></footer>
  {:else}
    <span class="hk">{text.replaceAll("\n", " / ")}</span>
  {/if}
{/if}
