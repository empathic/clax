<script lang="ts">
  // One haiku, loaded after first paint: the list is its own file, fetched
  // rather than imported, so the gallery's eager bundle carries neither the
  // list nor a module preloader. Never in comment mode, never animated: the
  // callers decide where it shows.
  import { pickHaiku } from "../view/haiku";
  import listUrl from "../view/haiku.json?url&no-inline";

  const p: { seed?: string; footer?: boolean } = $props();
  let text = $state<string | null>(null);
  $effect(() => { void fetch(listUrl).then(r => r.json()).then((list: string[]) => { text = pickHaiku(list, p.seed); }, () => {}); });
</script>

{#if text}
  {#if p.footer}
    <footer class="gfoot"><pre>{text}</pre><span>clax haiku · a new one each visit</span></footer>
  {:else}
    <span class="hk">{text.replaceAll("\n", " / ")}</span>
  {/if}
{/if}
