<script lang="ts">
  // The top bar island: the artifact view's controls (an empty slot for the
  // roster and summary, Comment, Threads, the name field on wide screens, the
  // version, the more menu with open raw and copy link, and the theme switch),
  // and the phone tab bar, once the artifact is loaded and the frame mode decided.
  import { fromStore } from "svelte/store";
  import { type ArtifactController, viewReady } from "../view/artifact-controller";
  import { moreMenu } from "./more-menu.svelte";
  import PhoneTabs from "./PhoneTabs.svelte";
  import ThemeSwitch from "./ThemeSwitch.svelte";
  import ViewerName from "./ViewerName.svelte";

  // An island's controller is fixed for its lifetime: the mount passes it once.
  let { ctl }: { ctl: ArtifactController } = $props();
  // svelte-ignore state_referenced_locally
  const view = fromStore(ctl.state);
  const s = $derived(view.current);
  const MoreMenu = $derived(moreMenu.current);
</script>

{#if viewReady(s)}
  {@const shown = ctl.shown(s)}
  {@const latest = ctl.latest(s)}
  <div class="who-slot"></div>
  <button class="comment" aria-pressed={s.commenting} disabled={s.deleted} onclick={() => ctl.toggleComment()}>Comment <span class="kc" aria-hidden="true">C</span></button>
  <button class="threads hide-sm" aria-pressed={s.panel} onclick={() => ctl.togglePanel()}>Threads <span class="cnt">{ctl.openCount(s)}</span></button>
  {#if !s.narrow}<ViewerName setNotice={ctl.setNotice} onViewer={v => ctl.setMe(v)} />{/if}
  <select class="version hide-sm" value={shown} disabled={s.deleted} aria-label="Version" onchange={e => ctl.chooseVersion(Number(e.currentTarget.value))}>
    {#each s.data.versions as v (v.n)}
      <option value={v.n}>v{v.n}{v.n === latest ? ` of ${latest}` : ""}{v.label ? ` · ${v.label}` : ""}</option>
    {/each}
  </select>
  {#if MoreMenu}
    <MoreMenu rawHref={s.deleted ? null : ctl.rawHref(s)} canCopy={!!navigator.clipboard && !s.deleted} onCopy={() => ctl.copyLink()} />
  {:else}
    <span class="more-slot hide-sm"></span>
  {/if}
  <span class="hide-sm"><ThemeSwitch /></span>
  <PhoneTabs panel={s.panel} open={ctl.openCount(s)} onPage={() => { if (s.panel) ctl.togglePanel(); }} onThreads={() => { if (!s.panel) ctl.togglePanel(); }} />
{/if}
