<svelte:options css="injected" />

<script lang="ts">
  // The top bar's Clone button (hosted spec §9 "Tokens"): where this Clax
  // keeps the artifact in git and the viewer's rights allow a token, it asks
  // what they may have (`GET /api/artifacts/<id>/git-token`) and shows
  // Clone, which opens the Clone dialog (its own lazy chunk); on a hosted
  // Clax that includes whoever reads a page shared by link, read-only.
  // Elsewhere (the native daemon, a spectator, a fork whose source the link
  // does not open) it shows nothing. At phone width
  // the button is hidden and the more menu lists Clone instead
  // (`setCloneOpener`). Loaded after the first paint (`more-menu.svelte.ts`).
  import type { Component } from "svelte";
  import { cloneOptions, type CloneOptions } from "./clone-api";
  import { setCloneOpener } from "./more-menu.svelte";

  let { id }: { id: string } = $props();
  let options = $state<CloneOptions | null>(null);
  let Dialog: Component<{ id: string; options: CloneOptions; onClose(): void }> | null = $state(null);
  let open = $state(false);
  const usable = $derived(!!options && options.available && options.scopes.length > 0);

  function show() {
    if (!Dialog) import("./CloneDialog.svelte").then(m => { Dialog = m.default; open = true; }, () => {});
    else open = true;
  }

  $effect(() => {
    let live = true;
    // svelte-ignore state_referenced_locally
    void cloneOptions(id).then(o => { if (live) options = o; }, () => {});
    return () => { live = false; };
  });
  $effect(() => {
    setCloneOpener(usable ? show : null);
    return () => setCloneOpener(null);
  });
</script>

{#if usable && options}
  <button type="button" class="clone hide-sm" aria-haspopup="dialog" onclick={show}>Clone</button>
  {#if open && Dialog}<Dialog {id} {options} onClose={() => { open = false; }} />{/if}
{/if}
