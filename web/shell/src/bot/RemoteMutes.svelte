<svelte:options css="injected" />

<script lang="ts">
  // Settings, Remotes (spec 2026-10-29-remote-instances §3): the Clax
  // instances the owner answered "Never for this instance", each with
  // **Unmute**, after which its agents may ask again. The owner's browser
  // alone unmutes (M20); a refusal shows as it is. Origins are text.
  import { onMount } from "svelte";
  import { refusal } from "../people/refusal";
  import { type RemoteMute, remoteMutes, unmuteRemote } from "../q/remote-api";

  let mutes = $state<RemoteMute[] | null>(null);
  let err = $state<string | null>(null);
  let busy = $state<string | null>(null);

  async function unmute(origin: string) {
    if (busy) return;
    busy = origin; err = null;
    try {
      await unmuteRemote(origin);
      mutes = (mutes ?? []).filter(m => m.origin !== origin);
    } catch (e) { err = refusal(e); } finally { busy = null; }
  }

  onMount(() => {
    void remoteMutes().then(r => { mutes = r.mutes; }, e => { err = refusal(e); });
  });
</script>

<section class="bcard rmutes" aria-labelledby="sec-remotes">
  <h3 id="sec-remotes">Remotes</h3>
  <p class="note">Clax instances you answered Never for: agents' requests to connect them ask nobody until you unmute them.</p>
  {#if mutes === null && !err}
    <p class="note" role="status">Loading…</p>
  {:else if mutes && !mutes.length}
    <p class="note">No instance is muted.</p>
  {:else if mutes}
    <ul>
      {#each mutes as m (m.origin)}
        <li><span class="origin">{m.origin}</span><button type="button" disabled={busy !== null} onclick={() => unmute(m.origin)}>{busy === m.origin ? "Unmuting…" : "Unmute"}</button></li>
      {/each}
    </ul>
  {/if}
  {#if err}<p class="err" role="alert">{err}</p>{/if}
</section>

<style>
  :global {
    .rmutes ul { margin: 0; padding: 0; display: grid; gap: 6px; }
    .rmutes li { list-style: none; display: flex; flex-wrap: wrap; align-items: center; gap: 6px 12px; min-width: 0; }
    .rmutes .origin { font: 13px var(--mono); overflow-wrap: anywhere; min-width: 0; flex: 1 1 auto; }
  }
</style>
