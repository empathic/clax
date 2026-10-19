<script lang="ts">
  // The sidebar island: the comment threads, once the artifact is loaded and
  // the frame mode decided; then the phone's Page | Threads switch, after
  // them so the Tab order follows the screen. The thread list's code and
  // styles, and the controller's wiring of it (ShellSidebar), load after the
  // first paint (`loadSidebar`); until then an empty sidebar holds its
  // place, so the frame is laid out once at its final size. Wider than a
  // phone the sidebar sits in the drawer, docked or floating, at the place
  // and size this browser remembers (view/drawer-model.ts), which the
  // page's head script put on the root; its controls, and the floating
  // window's box, come with the thread list's chunk.
  import { fromStore } from "svelte/store";
  import { type ArtifactController, viewReady } from "../view/artifact-controller";
  import { afterPaint } from "../view/after-paint";
  import PhoneTabs from "./PhoneTabs.svelte";
  import type SidebarT from "./ShellSidebar.svelte";
  import { loadSidebar, sidebarPrefetch } from "./sidebar-chunk";

  // An island's controller is fixed for its lifetime: the mount passes it once.
  let { ctl }: { ctl: ArtifactController } = $props();
  // svelte-ignore state_referenced_locally
  const view = fromStore(ctl.state);
  const s = $derived(view.current);
  let Sidebar: typeof SidebarT | null = $state(null);
  let asked = false;
  let cancel: (() => void) | undefined;
  $effect(() => {
    if (asked || !viewReady(s) || !(s.panel || (sidebarPrefetch() && !s.deleted))) return;
    asked = true;
    cancel = afterPaint(() => {
      loadSidebar().then(m => { Sidebar = m.default; }, () => ctl.setNotice("The threads could not load. Reload to try again."));
    });
  });
  $effect(() => () => cancel?.());
</script>

{#if viewReady(s)}
  <!-- Until its controls load, the drawer is this empty sidebar; a floating
       drawer, which takes no room from the page, is not drawn. -->
  {#if s.panel && !Sidebar && document.documentElement.dataset.drawer !== "float"}
    <aside class="sidebar drawer" aria-label="Comment threads" aria-busy="true"></aside>
  {:else if s.panel && Sidebar}
    <Sidebar {ctl} {s} />
  {/if}
  <PhoneTabs panel={s.panel} open={ctl.openCount(s)} onPage={() => { if (s.panel) ctl.togglePanel(); }} onThreads={() => { if (!s.panel) ctl.togglePanel(); }} />
{/if}

<!-- The drawer's box wider than a phone, with the artifact entry (not the
     gallery's): docked right (the default), left or at the bottom of the
     viewer, as the root's `data-drawer` says, sized by the custom
     properties on the root (`applyPrefs`, view/drawer-model.ts) and
     clamped here to the viewer with the limits view/drawer-model.ts clamps
     by (the greatest docked width, 720 px, is the stored width's own), so
     it fits from the first paint and on every resize. The rest of
     its rules, the floating window's box among them, come with its
     controls (DrawerChrome.svelte). At phone width it is the full-screen
     sheet (theme.css). -->
<style>
  :global {
    @media (min-width: 701px) {
      .viewer .drawer { flex: none; border: 0 solid var(--border); border-left-width: 1px; width: min(var(--drawer-w, 392px), 100% - 320px); min-width: 280px; }
      [data-drawer="left"] .drawer { order: -1; border-width: 0 1px 0 0; }
      [data-drawer="bottom"] .viewer { flex-direction: column; }
      [data-drawer="bottom"] .drawer { width: auto; height: min(var(--drawer-h, 280px), 100% - 160px); min-height: 160px; border-width: 1px 0 0; }
    }
  }
</style>
