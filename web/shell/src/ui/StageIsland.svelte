<script lang="ts">
  // The stage island: the overlays over the frame (the empty states, the
  // gesture shield and hint, the pins, the composer, the banners and the
  // consent dialog), once the artifact is loaded and the frame mode decided.
  import type { Component } from "svelte";
  import { fromStore } from "svelte/store";
  import { INDEX_FILE } from "../../../bridge/src/protocol";
  import { registerShield } from "../caps/gesture";
  import { shellPath } from "../route";
  import { type ArtifactController, viewReady } from "../view/artifact-controller";
  import Composer from "./Composer.svelte";
  import Pins from "./Pins.svelte";
  import PromptDialog from "./PromptDialog.svelte";

  // An island's controller is fixed for its lifetime: the mount passes it once.
  let { ctl }: { ctl: ArtifactController } = $props();
  // svelte-ignore state_referenced_locally
  const view = fromStore(ctl.state);
  const s = $derived(view.current);
  const shown = $derived(ctl.shown(s));
  const latest = $derived(ctl.latest(s));
  const missing = $derived(ctl.missing(s));
  // The keys sheet's code loads the first time it is asked for; if it cannot
  // load, the sheet closes with a notice rather than leaving the keys off.
  let KeysSheet: Component<{ onClose(): void }> | null = $state(null);
  let loading = false;
  $effect(() => {
    if (s.sheet !== "keys" || KeysSheet || loading) return;
    loading = true;
    import("./KeysSheet.svelte").then(m => { KeysSheet = m.default; }, () => ctl.sheetFailed()).finally(() => { loading = false; });
  });
  // The shield is registered while it is shown, and unregistered as it goes.
  const shield = (el: HTMLElement) => {
    registerShield(el);
    return () => registerShield(null);
  };
</script>

{#if viewReady(s)}
  {#if s.deleted}
    <p class="empty">This artifact was deleted.</p>
  {:else if missing}
    <p class="empty">v{shown} has no page {missing}. <a href={shellPath(ctl.id, s.pinnedVersion, INDEX_FILE)}>Open the index</a></p>
  {/if}
  {#if !s.deleted && !missing}
    <div class="frame-shield" aria-hidden="true" {@attach shield}><div></div><div></div><div></div><div></div></div>
  {/if}
  {#if s.hint}<p class="gesture-hint" role="status">{s.hint}</p>{/if}
  {#if !s.deleted && !missing}
    <Pins threads={s.threads} resolved={s.resolved} file={s.file} onit={new Set(s.working.flatMap(w => w.thread_ids))} onSelect={t => ctl.openPin(t)} onHover={t => ctl.hover(t)} />
  {/if}
  {#if s.draft}
    {@const draft = s.draft}
    {#key draft.pickId}
      <Composer {draft} onText={v => ctl.composerInput(v)} onFocused={() => ctl.composerFocused(draft.pickId)}
        onCancel={() => ctl.cancelDraft()} onSubmit={body => ctl.submitDraft(body, draft)} />
    {/key}
  {/if}
  {#if s.newer && !s.deleted}
    <div class="banner"><span>v{s.newer} published</span><button class="primary" onclick={() => ctl.reloadLatest()}>Reload</button></div>
  {/if}
  {#if shown < latest && !s.newer && !s.deleted}
    <div class="banner"><span class="muted">viewing v{shown}; latest is v{latest}</span><a href={ctl.here(null, s)}>latest</a></div>
  {/if}
  {#if s.notice}
    <div class="banner notice" role="alert"><span>{s.notice}</span><button onclick={() => ctl.dismissNotice()}>Dismiss</button></div>
  {/if}
  {#if s.ask}<PromptDialog ask={s.ask} />{/if}
  {#if s.sheet === "keys" && KeysSheet}<KeysSheet onClose={() => ctl.closeSheet()} />{/if}
{/if}
