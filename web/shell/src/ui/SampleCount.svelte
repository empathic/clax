<script lang="ts">
  // The running count of this artifact's calls to Claude today on this
  // machine's API key (spec §14), in the owner's browser once the page has
  // asked for sample and the daemon has a provider. Hidden at phone width.
  import { fromStore } from "svelte/store";
  import type { ArtifactController } from "../view/artifact-controller";
  import { callCountText } from "../view/sample-count";

  // An island's controller is fixed for its lifetime: the mount passes it once.
  let { ctl }: { ctl: ArtifactController } = $props();
  // svelte-ignore state_referenced_locally
  const view = fromStore(ctl.state);
  const calls = $derived(view.current.sampleCalls);
</script>

{#if calls}
  <span class="sample-count hide-sm" title="Calls this artifact made to Claude today with this machine's API key">{callCountText(calls.n, calls.cap)}</span>
{/if}

<style>
  .sample-count { font-size: 13px; color: var(--muted); white-space: nowrap; }
</style>
