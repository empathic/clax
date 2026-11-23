<svelte:options css="injected" />

<script lang="ts">
  // The top bar's Share button (an icon, named Share), on a hosted Clax for whoever may share the
  // artifact (the owner and its maker: the artifact's `may_share`, which the
  // bar shows it for): it opens the Share dialog (its own lazy chunk), with
  // the sharing read afresh. Elsewhere (a local daemon; a reader who may not
  // share) the bar shows none. At phone width the button is hidden and the more menu
  // lists Share instead (`setShareOpener`). Its chunk is asked as the page
  // starts (`more-menu.svelte.ts`). `onComments` hears the dialog's Comments switch
  // change, so the view follows at once.
  import type { Component } from "svelte";
  import * as api from "../hosted/api";
  import { setShareOpener } from "./more-menu.svelte";
  import { partFailed } from "../conn-notice";

  type Open = Extract<api.Sharing, { can_share: true }>;
  let { id, title, onComments }: { id: string; title: string; onComments?(on: boolean): void } = $props();
  let sharing = $state<Open | null>(null);
  let Dialog: Component<{ id: string; title: string; initial: Open; onClose(): void; onComments?(on: boolean): void }> | null = $state(null);
  let open = $state(false);

  async function show() {
    // Fresh each time: someone else may have changed it since.
    try { const s = await api.sharing(id); if (s.can_share) sharing = s; else return; } catch { return; }
    if (!Dialog) import("./ShareDialog.svelte").then(m => { Dialog = m.default; open = true; }, partFailed);
    else open = true;
  }

  $effect(() => {
    setShareOpener(show);
    return () => setShareOpener(null);
  });
</script>

<button type="button" class="icon share hide-sm" aria-haspopup="dialog" aria-label="Share" title="Share this artifact" onclick={show}>
  <svg viewBox="0 0 16 16" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M8 10V2M5 4.75l3-3 3 3M5.5 7H4v7h8V7h-1.5"/></svg>
</button>
{#if open && Dialog && sharing}<Dialog {id} {title} initial={sharing} {onComments} onClose={() => { open = false; }} />{/if}
