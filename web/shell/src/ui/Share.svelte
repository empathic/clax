<svelte:options css="injected" />

<script lang="ts">
  // The top bar's Share button, on a hosted Clax for whoever may share the
  // artifact (the owner and its maker; the daemon says, `can_share`): it
  // opens the Share dialog (its own lazy chunk). Elsewhere (a local
  // daemon, which answers no sharing route; a reader who may not share) it
  // shows nothing. At phone width the button is hidden and the more menu
  // lists Share instead (`setShareOpener`). Loaded after the first paint
  // (`more-menu.svelte.ts`). `onComments` hears the dialog's Comments switch
  // change, so the view follows at once.
  import type { Component } from "svelte";
  import * as api from "../hosted/api";
  import { setShareOpener } from "./more-menu.svelte";

  type Open = Extract<api.Sharing, { can_share: true }>;
  let { id, title, onComments }: { id: string; title: string; onComments?(on: boolean): void } = $props();
  let sharing = $state<Open | null>(null);
  let Dialog: Component<{ id: string; title: string; initial: Open; onClose(): void; onComments?(on: boolean): void }> | null = $state(null);
  let open = $state(false);

  async function show() {
    // Fresh each time: someone else may have changed it since.
    try { const s = await api.sharing(id); if (s.can_share) sharing = s; else return; } catch { return; }
    if (!Dialog) import("./ShareDialog.svelte").then(m => { Dialog = m.default; open = true; }, () => {});
    else open = true;
  }

  $effect(() => {
    let live = true;
    // svelte-ignore state_referenced_locally
    void api.sharing(id).then(s => { if (live && s.can_share) sharing = s; }, () => {});
    return () => { live = false; };
  });
  $effect(() => {
    setShareOpener(sharing ? show : null);
    return () => setShareOpener(null);
  });
</script>

{#if sharing}
  <button type="button" class="share hide-sm" aria-haspopup="dialog" onclick={show}>Share</button>
  {#if open && Dialog}<Dialog {id} {title} initial={sharing} {onComments} onClose={() => { open = false; }} />{/if}
{/if}
