<script lang="ts">
  // The gallery header's links that load with its lazy module, after the
  // first paint: ClaxBot's meter and Admin (`HeaderBot`, a chunk of its
  // own), and People and Queues. New is the gallery's first tile (`NewTile`).
  import PeopleLink, { type Viewer } from "../people/PeopleLink.svelte";

  const p: { current?: string; owner: boolean; viewer: Viewer } = $props();
  let Bot = $state<typeof import("../bot/HeaderBot.svelte").default | null>(null);
  void import("../bot/HeaderBot.svelte").then(m => { Bot = m.default; }, () => {});
</script>

{#if Bot}<Bot />{/if}
<PeopleLink current={p.current} owner={p.owner} viewer={p.viewer} />
