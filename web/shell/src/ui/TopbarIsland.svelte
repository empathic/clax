<script lang="ts">
  // The top bar island: the artifact view's controls (the roster and the
  // working summary, loaded after the first paint, which opens the people
  // panel (its own lazy chunk, holding the viewer's name); Reload when a newer
  // version is out; Comment, Threads, the version
  // button with its changelog menu and a Latest link beside it on an older
  // version, the more menu with open raw and copy link, and the theme switch),
  // once the artifact is loaded and the frame mode decided. At phone width the
  // bar keeps the mark, the title, Comment and the more menu, whose Versions
  // item opens the version menu as a sheet.
  import { fromStore } from "svelte/store";
  import { type ArtifactController, viewReady } from "../view/artifact-controller";
  import { moreMenu, who } from "./more-menu.svelte";
  import ThemeSwitch from "./ThemeSwitch.svelte";
  import VersionMenu from "./VersionMenu.svelte";

  // An island's controller is fixed for its lifetime: the mount passes it once.
  let { ctl }: { ctl: ArtifactController } = $props();
  // svelte-ignore state_referenced_locally
  const view = fromStore(ctl.state);
  const s = $derived(view.current);
  const MoreMenu = $derived(moreMenu.current);
  const Who = $derived(who.current);
</script>

{#if viewReady(s)}
  {@const shown = ctl.shown(s)}
  {@const latest = ctl.latest(s)}
  {#if Who}<Who {ctl} {s} open={ctl.openCount(s)} />{/if}
  {#if s.newer && !s.deleted}<button class="primary reload" onclick={() => ctl.reloadLatest()}>Reload</button>{/if}
  <button class="comment" aria-pressed={s.commenting} disabled={s.deleted} onclick={() => ctl.toggleComment()}>Comment <span class="kc" aria-hidden="true">C</span></button>
  <button class="threads hide-sm" aria-pressed={s.panel} onclick={() => ctl.togglePanel()}>Threads <span class="cnt">{ctl.openCount(s)}</span></button>
  <VersionMenu {shown} {latest} dot={s.decided?.dot ?? false} open={s.menu === "versions"} onToggle={() => ctl.openMenu("versions")}
    input={() => ({ versions: s.data.versions, latest, shown, now: new Date(), threads: s.threads, numbers: ctl.numbers(s), me: s.me?.public_id ?? null })}
    hrefFor={n => ctl.here(n === latest ? null : n, s)} onChoose={n => { ctl.closeMenu(); ctl.chooseVersion(n); }} />
  {#if shown < latest && !s.newer && !s.deleted}<a class="latest hide-sm" href={ctl.here(null, s)}>Latest</a>{/if}
  {#if MoreMenu}
    <MoreMenu rawHref={s.deleted ? null : ctl.rawHref(s)} canCopy={!!navigator.clipboard && !s.deleted} onCopy={() => ctl.copyLink()} onVersions={() => ctl.openMenu("versions")} />
  {:else}
    <span class="more-slot"></span>
  {/if}
  <span class="hide-sm"><ThemeSwitch /></span>
{/if}
