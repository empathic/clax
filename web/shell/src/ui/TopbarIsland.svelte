<script lang="ts">
  // The top bar island: the artifact view's controls (in the owner's browser,
  // this artifact's count of calls to Claude today, first; the roster and the
  // working summary, loaded after the first paint, which opens the people
  // panel (its own lazy chunk, holding the viewer's name); Reload when what
  // the page uses changed under it; Comment, Threads, the version control
  // (`Latest ▾`, or a split button whose main part reads `Reload for latest`
  // once a newer version is out, or `v12 of 46` on an older version, and
  // goes to the latest, beside the ▾ that opens its changelog menu), Share
  // (an icon) on a hosted Clax where the viewer may share the artifact
  // (loaded after the first paint; at phone width the more menu lists it),
  // and the more menu with open raw and copy link, Clone with git where the
  // viewer may clone the artifact (the bar has no Clone button; the Clone
  // dialog's host loads after the first paint), Usage with the week's
  // ClaxBot credits while ClaxBot is on for the viewer, and the theme switch,
  // once the artifact is loaded and the frame mode decided. At phone width the
  // bar keeps the mark, the title, Comment, the feedback menu and the more
  // menu, whose Versions item opens the version menu as a sheet and whose
  // People and agents and ClaxBot items stand in for the roster and the
  // ClaxBot button. On a live page, whose versions
  // are snapshots, Comment is off (comments on it come from Chrome with the
  // Clax extension) and Open page, the page itself, comes with the roster (on a
  // phone, in the more menu). For a spectator (no live seat free), Comment is off,
  // and so it is while the artifact's Comments switch is off. On an app
  // (an artifact a version of which carries an app bundle), Get in Clax, its
  // own lazy chunk, links to the app's get page (on a phone, a more menu item),
  // drawn as a ghost for the owner's shell and whoever may share the app.
  // Fork (an icon, its own lazy chunk: the shown version, into an artifact of the viewer's) comes with the
  // more menu, for the owner's shell and a viewer who may fork (`may_fork`); on a phone it is a more
  // menu item. Neither shows on a live page or a deleted one; a spectator,
  // who reads, forks too. Publish (to a public Clax this Clax is signed in
  // to) comes with the more menu too, in the owner's shell of a local daemon
  // only; on a phone it is a more menu item. The more menu's Permissions…
  // (the Permissions panel) comes for a viewer who may approve what the page uses,
  // and its Analytics… (the Analytics panel, its own lazy chunk, which this
  // island mounts) where the artifact carries `analytics`: on a hosted Clax,
  // for whoever administers it, never on a draft.
  // For a person who may report problems with Clax, the feedback menu
  // (`feedback/`) comes before the more menu. For a browser with an inbox
  // (the owner's, or a connected person's), Inbox comes first, with its
  // unread count (at phone width in the tab bar instead).
  // The controls show together, at once, in their final places
  // (`topbar-gate.svelte.ts`): once the view is ready (the artifact with
  // its threads, so every count is drawn once; with the bootstrap block, at
  // the first render), its changelog decided, and the controls' chunks in
  // (modulepreloaded with the page) and, without the block, what decides
  // the optional ones (`barDecided`), each at most `BAR_WAIT_MS` past the
  // view's being ready. No control waits on another's request, and the
  // frame waits on none of them. Each optional control (Inbox, Fork, Share, Publish, Get in
  // Clax, the feedback menu, and ClaxBot's button in `Who.svelte`) is in a
  // slot: what decides it is known at that render wherever the page knows
  // it (the block's `deciders`, the artifact's own fields, an answer asked
  // as the page started: `view/deciders.svelte.ts`), and then it is in its
  // place; one that comes later takes the bar's free end, beside the title,
  // so nothing moves. Until the controls show they are laid out, hidden.
  import { tick } from "svelte";
  import { fromStore } from "svelte/store";
  import { COMMENTS_OFF } from "../failure";
  import { type ArtifactController, commentsOff, viewReady } from "../view/artifact-controller";
  import { after } from "../clock";
  import { BAR_WAIT_MS, barDecided, decided as known } from "../view/deciders.svelte";
  import { analyticsOpen, botOpen, clone, cloneOpen, forkButton, getInClax, legal, loading, moreMenu, permissions, permissionsOpen, publish, publishOpen, share, shareOpen, usage, who } from "./more-menu.svelte";
  import SampleCount from "./SampleCount.svelte";
  import { feedbackDeciding, feedbackMenu } from "../feedback/slot.svelte";
  import { late, markShown } from "./topbar-gate.svelte";
  import InboxAnchor from "./InboxAnchor.svelte";
  import ThemeSwitch from "./ThemeSwitch.svelte";
  import VersionMenu from "./VersionMenu.svelte";

  // An island's controller is fixed for its lifetime: the mount passes it once.
  let { ctl }: { ctl: ArtifactController } = $props();
  // svelte-ignore state_referenced_locally
  const view = fromStore(ctl.state);
  const s = $derived(view.current);
  const MoreMenu = $derived(moreMenu.current);
  const Who = $derived(who.current);
  const Clone = $derived(clone.current);
  const Fork = $derived(forkButton.current);
  const GetInClax = $derived(getInClax.current);
  const Share = $derived(share.current);
  const Publish = $derived(publish.current);
  const Permissions = $derived(permissions.current);
  let forking = $state(false);
  // The owner's shell (it holds the token) may always fork; anyone else as
  // the daemon says (`may_fork`). The block says whose it is; else the
  // token's answer does, which the bar waits for (`barDecided`).
  const ownerShell = $derived(known.owner === true);
  // The view is ready, and the changelog this load decided names what the
  // pill says first (`s.decided`).
  const viewIn = $derived(viewReady(s) && s.decided !== null);
  // What decides the optional controls is in, and their code; or
  // `BAR_WAIT_MS` passed since the view was ready: what comes later takes
  // its late place.
  let answered = $state(false);
  let capped = $state(false);
  let cancelCap = () => {};
  let waiting = false;
  $effect(() => {
    if (!viewIn || waiting) return;
    waiting = true;
    void barDecided().then(() => { answered = true; });
    cancelCap = after(BAR_WAIT_MS, () => { capped = true; });
  });
  $effect(() => () => cancelCap());
  const decided = $derived(viewIn && ((answered && !loading.current) || capped));
  // Once the controls have painted, a slot filled later is late (`topbar-gate.svelte.ts`).
  let marked = false;
  $effect(() => {
    if (!decided || marked) return;
    marked = true;
    // The title's tags are laid out with the controls: they fit their line now, before this frame paints.
    document.querySelector(".page > .topbar")?.dispatchEvent(new Event("clax-bar-shown"));
    void tick().then(markShown);
  });
  /** Forks version `n` into an artifact of the viewer's and opens it. */
  function fork(n: number) {
    forking = true;
    void import("../view/fork").then(m => m.forkVersion(ctl.id, n, ctl.setNotice)).finally(() => { forking = false; });
  }
</script>

{#if viewReady(s)}
  {@const shown = ctl.shown(s)}
  {@const latest = ctl.latest(s)}
  {@const live = s.data.artifact.live}
  {@const off = commentsOff(s)}
  {@const forkable = !!MoreMenu && (ownerShell || !!s.data.artifact.may_fork) && !live && !s.deleted}
  {@const mayApprove = s.data.artifact.may_approve === true && !s.deleted}
  <div class="tools" class:deciding={!decided} aria-busy={decided ? undefined : "true"}>
  <span class="bar-gap" aria-hidden="true"></span>
  <span class="slot by-pill" use:late>{#if known.inbox}<InboxAnchor unread={known.unread} />{/if}</span>
  <SampleCount {ctl} />
  {#if Who}<Who {ctl} {s} open={ctl.openCount(s)} />{/if}
  {#if s.declarationStale && !s.newer && !s.deleted}<button class="primary reload" onclick={() => ctl.reloadHere()}>Reload</button>{/if}
  <button class="comment" data-shell-control="comment" aria-pressed={s.commenting} disabled={s.deleted || !!live || !!s.spectator || off || s.showDraft !== null}
    title={live ? "Comment on the live page with the Clax extension" : s.spectator ? "Commenting is paused while you're viewing" : off ? COMMENTS_OFF : s.showDraft !== null ? "Comments go on published versions" : undefined}
    onclick={() => ctl.toggleComment()}>Comment <span class="kc" aria-hidden="true">C</span></button>
  <button class="threads hide-sm" data-shell-control="threads" aria-pressed={s.panel} onclick={() => ctl.togglePanel()}>Threads <span class="cnt">{ctl.openCount(s)}</span></button>
  <VersionMenu {shown} {latest} newer={s.deleted ? null : s.newer} onLatest={s.deleted ? undefined : () => ctl.reloadLatest()} live={!!live} dot={s.decided?.dot ?? false} open={s.menu === "versions"} onToggle={() => ctl.openMenu("versions")}
    input={() => ({ versions: s.data.versions, latest, shown, now: new Date(), threads: s.threads, numbers: ctl.numbers(s), me: s.me?.public_id ?? null, live: !!live })}
    hrefFor={n => ctl.here(n === latest ? null : n, s)} onChoose={n => { ctl.closeMenu(); ctl.chooseVersion(n); }} />
  <span class="slot" use:late>{#if forkable && Fork}<Fork {shown} disabled={forking} onFork={() => fork(shown)} />{/if}</span>
  <span class="slot" use:late>{#if GetInClax && s.data.artifact.app && !s.deleted}<GetInClax id={ctl.id} ghost={ownerShell || !!s.data.artifact.may_share} />{/if}</span>
  <span class="slot" use:late>{#if Share && s.data.artifact.may_share && !s.deleted}<Share id={ctl.id} title={s.data.artifact.title} onComments={on => ctl.noteComments(on)} />{/if}</span>
  {#if Clone && !live && !s.deleted}<Clone id={ctl.id} />{/if}
  <span class="slot" use:late>{#if Publish && ownerShell && !live && !s.deleted}<Publish id={ctl.id} title={s.data.artifact.title} {latest} />{/if}</span>
  {#if Permissions && mayApprove}<Permissions {ctl} />{/if}
  {#if s.data.artifact.analytics && !s.deleted}{#await import("./AnalyticsPanel.svelte") then { default: AnalyticsPanel }}<AnalyticsPanel {ctl} />{/await}{/if}
  <!-- The feedback menu's place while its integration starts, where the page knows it will (its button's box). -->
  <span class="slot" use:late>{#if feedbackMenu.current}<feedbackMenu.current />{:else if known.feedback && feedbackDeciding.current}<span class="fb fb-wait" aria-hidden="true"></span>{/if}</span>
  {#if MoreMenu}
    <MoreMenu rawHref={s.deleted ? null : ctl.rawHref(s)} canCopy={!!navigator.clipboard && !s.deleted} onCopy={() => ctl.copyLink()} getHref={s.data.artifact.app && !s.deleted ? `/a/${encodeURIComponent(ctl.id)}/get` : null} onVersions={() => ctl.openMenu("versions")} pageHref={live?.page_url} onFork={forkable && !forking ? () => fork(shown) : undefined} onClone={live || s.deleted ? null : cloneOpen.current} onShare={s.deleted ? null : shareOpen.current} onPublish={live || s.deleted ? null : publishOpen.current} onPermissions={mayApprove ? permissionsOpen.current : null} onAnalytics={analyticsOpen.current} onPeople={Who ? () => ctl.openMenu("people") : null} onBot={botOpen.current} usage={usage.current} legal={legal.current} />
  {:else}
    <span class="more-slot"></span>
  {/if}
  <span class="hide-sm hide-nr"><ThemeSwitch /></span>
  </div>
{/if}

<!-- The controls' group and each slot are no box of their own: each
     control is the bar's flex item. While the group decides it is laid out
     and hidden. A late slot's control goes before every control (order 0)
     and after the mark and the title (theme.css). -->
<style>
  /* The bar's order and its widths, the artifact page's alone (not theme.css, which the gallery carries too). */
  :global {
    /* The mark and the title lead the bar: a control that comes late takes the
       place after them (`--late`, ui/topbar-gate.svelte.ts), before the others. */
    .topbar > .home { order: -100001; }
    .topbar > .ttl { order: -100000; }
    /* The title takes the bar's room up to its own width (it never pushes the
       controls: its basis is 0); the room between it and the controls
       (TopbarIsland.svelte's `.bar-gap`) takes the rest, so a control that
       comes late beside it takes from it, and nothing else moves. */
    .topbar .bar-gap { flex: 1 1 0; min-width: 0; order: -100; margin-left: -10px; }
    /* Between phone and 900 px wide the bar keeps to its width: Comment drops
       its key, Inbox keeps its icon and count, the theme switch goes
       (`.hide-nr`) and the bar's gaps narrow. */
    @media (min-width: 701px) and (max-width: 900px) {
      .topbar { gap: 8px; }
      .topbar .bar-gap { margin-left: -8px; }
      .topbar .hide-nr, .topbar .kc { display: none; }
      .topbar .inbox-link .word { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); white-space: nowrap; }
    }
  }
  :global(.topbar .tools), :global(.topbar .slot) { display: contents; }
  :global(.topbar .slot > *) { order: var(--late, 0); }
  /* Inbox, the call count and the pill (`PILL_ORDER`, topbar-gate.svelte.ts): what comes late beside the pill goes after them. */
  :global(.topbar .slot.by-pill) { --late: -50; }
  :global(.topbar .sample-count), :global(.topbar button.who) { order: -50; }
  :global(.topbar .tools.deciding) { visibility: hidden; }
  /* The feedback menu's button's box (FeedbackMenu.svelte), its count a badge over its corner, so the count moves nothing. */
  :global(.topbar .fb-wait) { flex: none; width: 32px; height: 32px; }
  :global(.topbar .fb .fb-btn .fb-count) { position: absolute; top: -4px; right: -6px; min-width: 16px; padding: 0 4px; font-size: 10.5px; line-height: 16px; }
  /* The title's tags fit what the controls leave them: they are laid out, and show, with the controls. */
  :global(.page:has(.tools.deciding) .ttl > .ttags) { display: none; }
</style>
