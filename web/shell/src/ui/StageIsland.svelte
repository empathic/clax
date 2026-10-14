<script lang="ts">
  // The stage island: the overlays over the frame (the empty states, the
  // gesture shield and hint, agents' drafts, the pins, the composer, the
  // spectator banner, the failure notice and the consent dialog). Nothing else goes over the page: a newer version
  // and an older one shown are said in the top bar (decided: Q12). Shown
  // once the artifact is loaded and the frame mode decided. A spectator (no
  // live seat free, spec 2026-10-11-clax-dev-hosted-design §16.1) sees a
  // banner saying so, with Join live once a seat is free.
  import type { Component } from "svelte";
  import { fromStore } from "svelte/store";
  import { INDEX_FILE } from "../../../bridge/src/protocol";
  import { registerShield } from "../caps/gesture";
  import { shellPath } from "../route";
  import { type ArtifactController, viewReady } from "../view/artifact-controller";
  import { bannerText } from "../view/draft-model";
  import { agentNames } from "../view/working-model";
  import Composer from "./Composer.svelte";
  import DraftBanner from "./DraftBanner.svelte";
  import Pins from "./Pins.svelte";
  import PromptDialog from "./PromptDialog.svelte";

  /** What a spectator is told (spec 2026-10-11-clax-dev-hosted-design
   * §16.1): a seat released for want of input says so, rather than that
   * the artifact is full. */
  const spectatorText = (code: string) => code === "seat_idle"
    ? "You were away, so live activity is paused."
    : "Lots of people are here, so you're viewing: comments and live activity are paused.";
  /** Join live shows always after a seat went idle (the viewer's next input
   * asks for it anyway), else once a snapshot says a seat this viewer may
   * take is free. */
  const offersJoin = (sp: { code: string; joinable: boolean }) => sp.code === "seat_idle" || sp.joinable;

  // An island's controller is fixed for its lifetime: the mount passes it once.
  let { ctl }: { ctl: ArtifactController } = $props();
  // svelte-ignore state_referenced_locally
  const view = fromStore(ctl.state);
  const s = $derived(view.current);
  const shown = $derived(ctl.shown(s));
  const missing = $derived(ctl.missing(s));
  // Agents' drafts (hosted spec §16.4), each named as the working signal names its agent.
  const draftBanner = $derived.by(() => {
    if (!s.drafts.length) return null;
    const names = agentNames(s.working, s.agents);
    return bannerText(s.drafts, s.showDraft, s.draftChanged, shown, (h, harness) => names.get(h) ?? harness);
  });
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
  <!-- The stage's foot: what sits at the bottom of the stage stacks here,
       newest-kind last, so no two of them cover each other (theme.css .stage-foot). -->
  <div class="stage-foot">
    {#if s.hint}<p class="gesture-hint" role="status">{s.hint}</p>{/if}
    {#if draftBanner && !s.deleted}
      <DraftBanner text={draftBanner} onShow={a => ctl.showDraftOf(a)} onBack={() => ctl.backFromDraft()} />
    {/if}
    {#if s.spectator}
      <div class="banner spectate" role="status">
        <span>{spectatorText(s.spectator.code)}</span>
        {#if offersJoin(s.spectator)}<button class="primary" onclick={() => ctl.joinLive()}>Join live</button>{/if}
      </div>
    {/if}
  </div>
  {#if !s.deleted && !missing}
    <Pins threads={s.threads} resolved={s.resolved} file={s.file} onit={new Set(s.working.flatMap(w => w.thread_ids))} {addressed} onSelect={t => ctl.openPin(t)} onHover={t => ctl.hover(t)} />
  {/if}
  {#if s.draft}
    {@const draft = s.draft}
    {#key draft.pickId}
      <Composer {draft} onText={v => ctl.composerInput(v)} onFocused={() => ctl.composerFocused(draft.pickId)}
        onCancel={() => ctl.cancelDraft()} onSubmit={body => ctl.submitDraft(body, draft)} />
    {/key}
  {/if}
  {#if s.notice}
    <div class="banner notice" role="alert"><span>{s.notice}</span><button onclick={() => ctl.dismissNotice()}>Dismiss</button></div>
  {/if}
  {#if s.ask}<PromptDialog ask={s.ask} />{/if}
  {#if s.sheet === "keys" && KeysSheet}<KeysSheet onClose={() => ctl.closeSheet()} />{/if}
{/if}
