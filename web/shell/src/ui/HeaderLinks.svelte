<script lang="ts">
  // The gallery header's links that load with its lazy module, after the
  // first paint: ClaxBot's meter and Admin (`HeaderBot`, a chunk of its
  // own), People and Queues, and, for a person who may report problems with
  // Clax, the feedback menu (`feedback/`, started here: the gallery's eager
  // entry carries none of it). New is the gallery's first tile (`NewTile`).
  import { bootFeedback } from "../feedback/boot";
  import { feedbackMenu } from "../feedback/slot.svelte";
  import PeopleLink, { type Viewer } from "../people/PeopleLink.svelte";

  bootFeedback();
  const p: { current?: string; owner: boolean; viewer: Viewer } = $props();
  let Bot = $state<typeof import("../bot/HeaderBot.svelte").default | null>(null);
  void import("../bot/HeaderBot.svelte").then(m => { Bot = m.default; }, () => {});
</script>

{#if Bot}<Bot />{/if}
<PeopleLink current={p.current} owner={p.owner} viewer={p.viewer} />
{#if feedbackMenu.current}<feedbackMenu.current />{/if}
