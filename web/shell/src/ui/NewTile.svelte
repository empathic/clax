<svelte:options css="injected" />

<script lang="ts" module>
  import { mount, unmount } from "svelte";
  import type { Me } from "../view/new-artifact";

  /** The dialog open, mounted apart from the tile: the gallery moves the
   * tile (from the empty gallery into the grid, as the artifact made lands)
   * while the dialog stays. */
  let shown: HTMLElement | null = null;
  async function openDialog(me: Me): Promise<void> {
    const { default: Dialog } = await import("./NewArtifactDialog.svelte");
    if (shown?.isConnected) return;
    const target = document.createElement("div");
    document.body.append(target);
    shown = target;
    let view: Record<string, unknown> | null = null;
    const close = () => { if (view) void unmount(view); target.remove(); };
    view = mount(Dialog, { target, props: { me, onClose: close } });
  }
</script>

<script lang="ts">
  // The gallery's New artifact tile (docs/contract.md "Starting an artifact
  // from a brief"; spec 2026-10-21-claxbot D8, §6.4), first in the grid,
  // shown when `GET /api/viewers/me` says this browser may start an
  // artifact (`may`, its `may_create`: the owner's, or a connected
  // person's whose rights create), and the gallery's first render knows it,
  // so the tile is there from that render. Its dialog loads on the first
  // press, apart from the tile, which the gallery may move while it is
  // open. Its line says ClaxBot can build it when ClaxBot is on for this
  // person (`bot`, `GET /api/me/bot`'s `enabled`). Props are read off `p`.
  const p: { empty?: boolean; me: Me; may: boolean; bot: boolean } = $props();
  let failed = $state(false);
  const show = () => { openDialog(p.me).then(() => { failed = false; }, () => { failed = true; }); };
</script>

{#if p.may}
  <div class={p.empty ? "card-wrap new-tile empty" : "card-wrap new-tile"}>
    <button type="button" class="card" aria-haspopup="dialog" aria-label="New artifact" aria-describedby="new-tile-l" onclick={show}>
      <span class="plus" aria-hidden="true">+</span>
      <span class="nt"><b>New artifact</b><small id="new-tile-l">{failed ? "The dialog did not load: try again" : p.bot ? "Name it; ClaxBot can build it" : "Name it and say what you want built"}</small></span>
    </button>
  </div>
{/if}

<style>
  :global {
    .new-tile > .card { display: flex; align-items: center; justify-content: flex-start; gap: 14px; padding: 14px; min-height: 92px; height: 100%; border-style: dashed; border-color: var(--border-strong); background: none; text-align: start; white-space: normal; font: inherit; color: var(--fg); }
    .new-tile > .card:hover { background: var(--card); }
    .new-tile .plus { width: 36px; height: 36px; flex: none; display: grid; place-items: center; border-radius: 50%; background: var(--hover); font: 400 24px/1 var(--font); color: var(--muted); }
    .new-tile .nt { display: grid; gap: 2px; min-width: 0; }
    .new-tile b { font-size: 15px; }
    .new-tile small { font-size: 12.5px; color: var(--muted); overflow-wrap: anywhere; }
    .new-tile.empty { width: min(320px, 100%); }
  }
</style>
