<script lang="ts">
  // The top bar island: the artifact view's controls (in the owner's browser,
  // this artifact's count of calls to Claude today, first; the roster and the
  // working summary, loaded after the first paint, which opens the people
  // panel (its own lazy chunk, holding the viewer's name); Reload when a newer
  // version is out; Comment, Threads, the version
  // button with its changelog menu and a Latest link beside it on an older
  // version, Clone where the viewer may clone the artifact with git (loaded
  // after the first paint; at phone width the more menu lists it), and the
  // more menu with open raw and copy link, and the theme switch), and Share
  // on a hosted Clax where the viewer may share the artifact (loaded after
  // the first paint; at phone width the more menu lists it),
  // once the artifact is loaded and the frame mode decided. At phone width the
  // bar keeps the mark, the title, Comment and the more menu, whose Versions
  // item opens the version menu as a sheet. On a live page, whose versions
  // are snapshots, Comment is off (comments on it come from Chrome with the
  // Clax extension) and Open page, the page itself, comes with the roster (on a
  // phone, in the more menu). For a spectator (no live seat free), Comment is off.
  // Fork (the shown version, into an artifact of the viewer's) comes with the
  // more menu, for the owner's shell and a viewer who may fork (`may_fork`); on a phone it is a more
  // menu item. Neither shows on a live page or a deleted one; a spectator,
  // who reads, forks too. Publish (to a public Clax this Clax is signed in
  // to) comes with the more menu too, in the owner's shell of a local daemon
  // only; on a phone it is a more menu item.
  import { fromStore } from "svelte/store";
  import { getToken } from "../api";
  import { type ArtifactController, viewReady } from "../view/artifact-controller";
  import { clone, cloneOpen, legal, moreMenu, publish, publishOpen, share, shareOpen, who } from "./more-menu.svelte";
  import SampleCount from "./SampleCount.svelte";
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
  const Share = $derived(share.current);
  const Publish = $derived(publish.current);
  let forking = $state(false);
  // The owner's shell (it holds the token) may always fork; anyone else as
  // the daemon says (`may_fork`).
  let ownerShell = $state(false);
  void getToken().then(t => { ownerShell = !!t; });
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
  <SampleCount {ctl} />
  {#if Who}<Who {ctl} {s} open={ctl.openCount(s)} />{/if}
  {#if s.newer && !s.deleted}<button class="primary reload" onclick={() => ctl.reloadLatest()}>Reload</button>{/if}
  <button class="comment" aria-pressed={s.commenting} disabled={s.deleted || !!live || !!s.spectator || s.showDraft !== null}
    title={live ? "Comment on the live page with the Clax extension" : s.spectator ? "Commenting is paused while you're viewing" : s.showDraft !== null ? "Comments go on published versions" : undefined}
    onclick={() => ctl.toggleComment()}>Comment <span class="kc" aria-hidden="true">C</span></button>
  <button class="threads hide-sm" aria-pressed={s.panel} onclick={() => ctl.togglePanel()}>Threads <span class="cnt">{ctl.openCount(s)}</span></button>
  <VersionMenu {shown} {latest} live={!!live} dot={s.decided?.dot ?? false} open={s.menu === "versions"} onToggle={() => ctl.openMenu("versions")}
    input={() => ({ versions: s.data.versions, latest, shown, now: new Date(), threads: s.threads, numbers: ctl.numbers(s), me: s.me?.public_id ?? null, live: !!live })}
    hrefFor={n => ctl.here(n === latest ? null : n, s)} onChoose={n => { ctl.closeMenu(); ctl.chooseVersion(n); }} />
  {#if shown < latest && !s.newer && !s.deleted}<a class="latest hide-sm" href={ctl.here(null, s)}>Latest</a>{/if}
  {@const forkable = !!MoreMenu && (ownerShell || !!s.data.artifact.may_fork) && !live && !s.deleted}
  {#if forkable}<button class="fork hide-sm" disabled={forking} title={`Fork v${shown} into an artifact of your own`} onclick={() => fork(shown)}>Fork</button>{/if}
  {#if Share && !s.deleted}<Share id={ctl.id} title={s.data.artifact.title} />{/if}
  {#if Clone && !live && !s.deleted}<Clone id={ctl.id} />{/if}
  {#if Publish && ownerShell && !live && !s.deleted}<Publish id={ctl.id} title={s.data.artifact.title} {latest} />{/if}
  {#if MoreMenu}
    <MoreMenu rawHref={s.deleted ? null : ctl.rawHref(s)} canCopy={!!navigator.clipboard && !s.deleted} onCopy={() => ctl.copyLink()} onVersions={() => ctl.openMenu("versions")} pageHref={live?.page_url} onFork={forkable && !forking ? () => fork(shown) : undefined} onClone={live || s.deleted ? null : cloneOpen.current} onShare={s.deleted ? null : shareOpen.current} onPublish={live || s.deleted ? null : publishOpen.current} legal={legal.current} />
  {:else}
    <span class="more-slot"></span>
  {/if}
  <span class="hide-sm"><ThemeSwitch /></span>
{/if}
