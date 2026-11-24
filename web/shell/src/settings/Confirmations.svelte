<script lang="ts">
  // Settings, Confirmations: whether confirming a decision takes a passkey,
  // and the passkeys registered (`q/PasskeySettings.svelte`).
  import { onMount } from "svelte";
  import BotPage from "../bot/BotPage.svelte";
  import PasskeySettings from "../q/PasskeySettings.svelte";

  onMount(() => { document.title = "Confirmations · Settings · Clax"; });
</script>

<BotPage title="Confirmations" lede="How the decisions that wait for you in this browser are confirmed.">
  <PasskeySettings />
</BotPage>
