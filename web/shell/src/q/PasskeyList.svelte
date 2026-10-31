<svelte:options css="injected" />

<script lang="ts">
  // The passkeys registered for confirmations, each with when it was added,
  // from which browser, and the agents that were running then: so a
  // passkey the owner did not add shows, wherever passkeys are named (the
  // first-use offer, a confirmation that requires one, Settings). `onRemove`,
  // when given, puts **Remove** beside each. Every word shown is text.
  import { relativeTime } from "../format";
  import type { PasskeySettings } from "./passkey";

  let { keys, now, busy = false, onRemove }: {
    keys: PasskeySettings["passkeys"];
    now?: Date;
    busy?: boolean;
    onRemove?: (id: string) => void;
  } = $props();
  const agents = (k: PasskeySettings["passkeys"][number]) => (k.added_by?.agents_live?.length ? k.added_by.agents_live.join(", ") : "none");
</script>

{#if keys.length}
  <ul class="pk-list" aria-label="Passkeys">
    {#each keys as k (k.credential_id)}
      <li>
        <span>Passkey added {relativeTime(k.created_at, now ?? new Date())} from {k.added_by?.via ?? "an unknown browser"}; agents running then: {agents(k)}</span>
        {#if onRemove}<button type="button" class="ghost" disabled={busy} onclick={() => onRemove(k.credential_id)}>Remove</button>{/if}
      </li>
    {/each}
  </ul>
{/if}

<style>
  :global {
    .pk-list { margin: 0; padding: 0; list-style: none; display: grid; gap: 6px; }
    .pk-list li { display: flex; flex-wrap: wrap; align-items: center; justify-content: space-between; gap: 6px; font-size: 12.5px; overflow-wrap: anywhere; }
  }
</style>
