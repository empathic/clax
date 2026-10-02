<script lang="ts">
  // The topbar island: the artifact view's controls (Comment, Threads, the
  // name field on wide screens, the version, open raw, copy link and the theme
  // switch), once the artifact is loaded and the frame mode decided.
  import { fromStore } from "svelte/store";
  import { type ArtifactController, viewReady } from "../view/artifact-controller";
  import ThemeSwitch from "./ThemeSwitch.svelte";
  import ViewerName from "./ViewerName.svelte";

  // An island's controller is fixed for its lifetime: the mount passes it once.
  let { ctl }: { ctl: ArtifactController } = $props();
  // svelte-ignore state_referenced_locally
  const view = fromStore(ctl.state);
  const s = $derived(view.current);
</script>

{#if viewReady(s)}
  {@const shown = ctl.shown(s)}
  {@const latest = ctl.latest(s)}
  <button aria-pressed={s.commenting} class={s.commenting ? "primary" : ""} disabled={s.deleted} onclick={() => ctl.toggleComment()}>Comment</button>
  <button aria-pressed={s.panel} onclick={() => ctl.togglePanel()}>Threads ({ctl.openCount(s)})</button>
  {#if !s.narrow}<ViewerName setNotice={ctl.setNotice} onViewer={v => ctl.setMe(v)} />{/if}
  <select value={shown} disabled={s.deleted} onchange={e => ctl.chooseVersion(Number(e.currentTarget.value))}>
    {#each s.data.versions as v (v.n)}
      <option value={v.n}>v{v.n}{v.n === latest ? ` of ${latest}` : ""}{v.label ? ` · ${v.label}` : ""}</option>
    {/each}
  </select>
  {#if s.deleted}
    <span class="hide-sm muted">open raw</span>
  {:else}
    <a class="hide-sm" href={ctl.rawHref(s)} target="_blank" rel="noopener">open raw</a>
  {/if}
  {#if navigator.clipboard}<button class="copy-link" disabled={s.deleted} onclick={() => ctl.copyLink()}>copy link</button>{/if}
  <ThemeSwitch />
{/if}
