<script lang="ts">
  // The gallery header's links that load with its lazy module, after the
  // first paint: New (on the gallery itself, for a browser that may start
  // an artifact) and People.
  import PeopleLink, { type Viewer } from "../people/PeopleLink.svelte";
  import NewButton from "./NewButton.svelte";

  const p: { current?: boolean; owner: boolean; viewer: Viewer } = $props();
</script>

{#if location.pathname === "/"}<NewButton />{/if}
<PeopleLink current={p.current} owner={p.owner} viewer={p.viewer} />
