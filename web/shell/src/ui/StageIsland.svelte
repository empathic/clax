<script lang="ts">
  // The stage island: the overlays over the frame (the empty states, the
  // gesture shield and hint, agents' drafts, the pins, the composer, the
  // spectator banner, the approval bar, the failure notice and the consent dialog). In the
  // Clax app the host may draw the drafts' banner, the notice and the
  // dialog itself (`s.native`), and then they are not drawn here. Nothing else goes over the page: a newer version
  // and an older one shown are said in the top bar (decided: Q12). Shown
  // once the artifact is loaded and the frame mode decided. A spectator (no
  // live seat free, spec 2026-10-11-clax-dev-hosted-design §16.1) sees a
  // banner saying so, with Join live once a seat is free. The drafts' banner
  // and the spectator's are lazy chunks (view/drafts.ts, view/spectator.ts),
  // loaded the first time there is a draft or the view is a spectator; so
  // is the notice that offers the page in a tab when the frame could not
  // open it (view/open-in-tab.svelte).
  import type { Component } from "svelte";
  import { fromStore } from "svelte/store";
  import { INDEX_FILE } from "../../../bridge/src/protocol";
  import { registerShield } from "../caps/gesture";
  import { shellPath } from "../route";
  import { type ArtifactController, viewReady } from "../view/artifact-controller";
  import ApprovalBar from "./ApprovalBar.svelte";
  import ApprovalDialog from "./ApprovalDialog.svelte";
  import Composer from "./Composer.svelte";
  import Pins from "./Pins.svelte";
  import PromptDialog from "./PromptDialog.svelte";

  // An island's controller is fixed for its lifetime: the mount passes it once.
  let { ctl }: { ctl: ArtifactController } = $props();
  // svelte-ignore state_referenced_locally
  const view = fromStore(ctl.state);
  const s = $derived(view.current);
  const shown = $derived(ctl.shown(s));
  const missing = $derived(ctl.missing(s));
  // What waits for this viewer's approval, once the daemon says they may approve it.
  const pending = $derived(ctl.pending(s));
  let reviewing = $state(false);
  // Agents' drafts (hosted spec §16.4): the list names one only once their
  // code (view/drafts.ts, with the banner) has loaded.
  const drafts = $derived(s.drafts.length ? ctl.drafts : null);
  // The group's threads, until this viewer looks at one after its version came out.
  const addressed = $derived.by(() => {
    const d = s.decided;
    if (!d?.ids.length) return undefined;
    const at = s.data?.versions.find(v => v.n === d.n)?.created_at ?? "";
    return new Map(d.ids.filter(id => !(s.looked[id] > at)).map(id => [id, d.n]));
  });
  // The keys sheet's code loads the first time it is asked for; if it cannot
  // load, the sheet closes with a notice rather than leaving the keys off.
  let KeysSheet: Component<{ onClose(): void }> | null = $state(null);
  let loading = false;
  $effect(() => {
    if (s.sheet !== "keys" || KeysSheet || loading) return;
    loading = true;
    import("./KeysSheet.svelte").then(m => { KeysSheet = m.default; }, () => ctl.sheetFailed()).finally(() => { loading = false; });
  });
  // The page in a tab, the first time the frame could not open it.
  let OpenInTab: Component<{ href: string | null }> | null = $state(null);
  $effect(() => { if ((s.openInTab || s.elsewhere) && !OpenInTab) import("../view/open-in-tab.svelte").then(m => { OpenInTab = m.default; }, () => {}); });
  // A spectator's banner loads with spectator mode, the first time the view is one.
  let SpectatorBanner: Component<{ code: string; joinable: boolean; onJoin(): void }> | null = $state(null);
  $effect(() => { if (s.spectator && !SpectatorBanner) import("../view/spectator").then(m => { SpectatorBanner = m.SpectatorBanner; }, () => {}); });
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
  {:else if (s.openInTab || s.elsewhere) && OpenInTab}
    <OpenInTab href={s.openInTab} />
  {/if}
  {#if !s.deleted && !missing && !s.openInTab && !s.elsewhere}
    <div class="frame-shield" aria-hidden="true" {@attach shield}><div></div><div></div><div></div><div></div></div>
  {/if}
  <!-- The stage's foot: what sits at the bottom of the stage stacks here,
       newest-kind last, so no two of them cover each other (theme.css .stage-foot). -->
  <div class="stage-foot">
    {#if s.hint}<p class="gesture-hint" role="status">{s.hint}</p>{/if}
    {#if drafts && !s.deleted && !s.native?.includes("draft")}
      <drafts.Banner {s} {shown} onShow={a => ctl.showDraftOf(a)} onBack={() => ctl.backFromDraft()} />
    {/if}
    {#if pending && !s.deleted}
      <ApprovalBar {pending} onReview={() => { reviewing = true; }} onNotNow={() => ctl.notNow()} />
    {/if}
    {#if s.spectator && SpectatorBanner}
      <SpectatorBanner code={s.spectator.code} joinable={s.spectator.joinable} onJoin={() => ctl.joinLive()} />
    {/if}
  </div>
  {#if !s.deleted && !missing}
    <Pins threads={s.threads} resolved={s.resolved} file={s.file} onit={new Set(s.working.flatMap(w => w.thread_ids))} {addressed} onSelect={t => ctl.openPin(t)} onHover={t => ctl.hover(t)} />
  {/if}
  {#if s.draft}
    {@const draft = s.draft}
    {#key draft.pickId}
      <Composer {draft} onText={v => ctl.composerInput(v)} onFocused={() => ctl.composerFocused(draft.pickId)}
        onCancel={() => ctl.cancelDraft()} onSubmit={body => ctl.submitDraft(body, draft)} onDropped={() => ctl.app?.dropped()} send={!!ctl.app} />
    {/key}
  {/if}
  {#if s.notice && !s.native?.includes("notice")}
    <div class="banner notice" role="alert"><span>{s.notice}</span><button onclick={() => ctl.dismissNotice()}>Dismiss</button></div>
  {/if}
  {#if reviewing && pending}
    <ApprovalDialog {pending} title={s.data?.artifact.title ?? ""} busy={s.approving} onApprove={() => { void ctl.approve(); }} onClose={() => { reviewing = false; }} />
  {/if}
  {#if s.ask && !s.native?.includes("ask")}<PromptDialog ask={s.ask} />{/if}
  {#if s.sheet === "keys" && KeysSheet}<KeysSheet onClose={() => ctl.closeSheet()} />{/if}
{/if}
