<script lang="ts">
  // The gallery header's links that load with its lazy module, after the
  // first paint: ClaxBot's meter (`HeaderBot`, a chunk of its own) and
  // Queues here, and Settings' gear, which this puts at the header's end,
  // after the theme switch: the header's last control. New is the
  // gallery's first tile (`NewTile`).
  import { mount, onMount, unmount } from "svelte";
  import MemberLinks, { type Viewer } from "../people/MemberLinks.svelte";

  let { current, owner, viewer }: { current?: string; owner: boolean; viewer: Viewer } = $props();
  let Bot = $state<typeof import("../bot/HeaderBot.svelte").default | null>(null);
  void import("../bot/HeaderBot.svelte").then(m => { Bot = m.default; }, () => {});
  // The gear's props follow this component's.
  const gear = $state<{ show: "settings"; current?: string; owner: boolean; viewer: Viewer }>({ show: "settings", owner: false, viewer: undefined });
  $effect(() => { gear.current = current; gear.owner = owner; gear.viewer = viewer; });
  onMount(() => {
    const bar = document.querySelector(".gbar");
    if (!bar) return;
    const view = mount(MemberLinks, { target: bar, props: gear });
    return () => { void unmount(view); };
  });
</script>

{#if Bot}<Bot />{/if}
<MemberLinks show="queues" {current} {owner} {viewer} />
