<script lang="ts">
  // The sidebar island: the comment threads, once the artifact is loaded and
  // the frame mode decided; then the phone's Page | Threads switch, after
  // them so the Tab order follows the screen. The thread list's code and
  // styles, and the controller's wiring of it (ShellSidebar), load after the
  // first paint (`loadSidebar`); until then an empty sidebar holds its
  // width, so the frame is laid out once at its final size.
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
  {#if s.panel && !Sidebar}
    <aside class="sidebar" aria-label="Comment threads" aria-busy="true"></aside>
  {:else if s.panel && Sidebar}
    <Sidebar {ctl} {s} />
  {/if}
  <PhoneTabs panel={s.panel} open={ctl.openCount(s)} onPage={() => { if (s.panel) ctl.togglePanel(); }} onThreads={() => { if (!s.panel) ctl.togglePanel(); }} />
{/if}
