<svelte:options css="injected" />

<script lang="ts">
  // The passkeys registered for confirmations, as Settings lists them: each
  // with when it was added, from which browser, and how many agents were
  // running then (the line opens to name them), so a passkey the owner did
  // not add shows. `onRemove`, when given, puts **Remove** beside each.
  // Every word shown is text.
  import { relativeTime } from "../format";
  import type { PasskeySettings } from "./passkey";

  let { keys, now, busy = false, onRemove }: {
    keys: PasskeySettings["passkeys"];
    now?: Date;
    busy?: boolean;
    onRemove?: (id: string) => void;
  } = $props();
  const agents = (k: PasskeySettings["passkeys"][number]) => k.added_by?.agents_live ?? [];
</script>

{#if keys.length}
  <ul class="pk-list" aria-label="Passkeys">
    {#each keys as k (k.credential_id)}
      <li>
        <span>Passkey added {relativeTime(k.created_at, now ?? new Date())} from {k.added_by?.via ?? "an unknown browser"}{#if agents(k).length}
          <details><summary>{agents(k).length} agent{agents(k).length === 1 ? "" : "s"} running then</summary>{agents(k).join(", ")}</details>
        {:else}; no agents running then{/if}</span>
        {#if onRemove}<button type="button" class="ghost" disabled={busy} onclick={() => onRemove(k.credential_id)}>Remove</button>{/if}
      </li>
    {/each}
  </ul>
{/if}

<style>
  :global {
    .pk-list { margin: 0; padding: 0; list-style: none; display: grid; gap: 6px; }
    .pk-list details { color: var(--muted); }
    .pk-list summary { cursor: pointer; width: fit-content; }
    .pk-list li { display: flex; flex-wrap: wrap; align-items: center; justify-content: space-between; gap: 6px; font-size: 12.5px; overflow-wrap: anywhere; }
  }
</style>
