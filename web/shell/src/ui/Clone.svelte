<svelte:options css="injected" />

<script lang="ts">
  // The Clone dialog's host in the top bar (hosted spec §9 "Tokens"), with no
  // button of its own: where this Clax keeps the artifact in git and the
  // viewer's rights allow a token, it asks what they may have
  // (`GET /api/artifacts/<id>/git-token`) and offers the more menu's Clone
  // with git item (`setCloneOpener`), which opens the Clone dialog (its own
  // lazy chunk); on a hosted Clax that includes whoever reads a page shared
  // by link, read-only. Elsewhere (the native daemon, a spectator, a fork
  // whose source the link does not open) it offers nothing. Loaded after the
  // first paint (`more-menu.svelte.ts`).
  import type { Component } from "svelte";
  import { cloneOptions, type CloneOptions } from "./clone-api";
  import { setCloneOpener } from "./more-menu.svelte";
  import { partFailed } from "../conn-notice";

  let { id }: { id: string } = $props();
  let options = $state<CloneOptions | null>(null);
  let Dialog: Component<{ id: string; options: CloneOptions; onClose(): void }> | null = $state(null);
  let open = $state(false);
  const usable = $derived(!!options && options.available && options.scopes.length > 0);

  function show() {
    if (!Dialog) import("./CloneDialog.svelte").then(m => { Dialog = m.default; open = true; }, partFailed);
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

{#if usable && options && open && Dialog}<Dialog {id} {options} onClose={() => { open = false; }} />{/if}
