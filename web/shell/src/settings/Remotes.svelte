<script lang="ts">
  // Settings, Remotes: the Clax instances the owner answered Never for,
  // each of which they can unmute (`bot/RemoteMutes.svelte`).
  import { onMount } from "svelte";
  import BotPage from "../bot/BotPage.svelte";
  import RemoteMutes from "../bot/RemoteMutes.svelte";

  onMount(() => { document.title = "Remotes · Settings · Clax"; });
</script>

<BotPage title="Remotes" lede="Other Clax instances your agents asked to connect to.">
  <RemoteMutes />
</BotPage>
