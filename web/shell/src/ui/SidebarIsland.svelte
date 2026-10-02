<script lang="ts">
  // The sidebar island: the comment threads, and the "Your name" field on
  // narrow screens, once the artifact is loaded and the frame mode decided;
  // then the phone's Page | Threads switch, after them so the Tab order
  // follows the screen. The thread list's code and styles load after the first
  // paint (`loadSidebar`); until then an empty sidebar holds its width, so the
  // frame is laid out once at its final size.
  import { fromStore } from "svelte/store";
  import { type ArtifactController, viewReady } from "../view/artifact-controller";
  import { afterPaint } from "../view/after-paint";
  import { agentName } from "../view/history-model";
  import PhoneTabs from "./PhoneTabs.svelte";
  import type SidebarT from "./Sidebar.svelte";
  import { loadSidebar, sidebarPrefetch } from "./sidebar-chunk";
  import ViewerName from "./ViewerName.svelte";

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

{#snippet nameField()}
  <ViewerName setNotice={ctl.setNotice} onViewer={v => ctl.setMe(v)} />
{/snippet}

{#if viewReady(s)}
  {#if s.panel && !Sidebar}
    <aside class="sidebar" aria-label="Comment threads" aria-busy="true"></aside>
  {:else if s.panel && Sidebar}
    <Sidebar threads={s.threads} resolved={s.resolved} selected={s.selected} file={s.file} holds={f => ctl.holds(f, s)} me={s.me}
      versions={s.data.versions} shown={ctl.shown(s)} agent={agentName(s.data.artifact.owner_harness)}
      header={s.narrow ? nameField : undefined}
      onSelect={t => ctl.selectThread(t)} onHover={t => ctl.hover(t)} onSend={t => ctl.sendThread(t)}
      onResolve={t => ctl.resolveThread(t)} onReply={(t, body) => ctl.reply(t, body)} />
  {/if}
  <PhoneTabs panel={s.panel} open={ctl.openCount(s)} onPage={() => { if (s.panel) ctl.togglePanel(); }} onThreads={() => { if (!s.panel) ctl.togglePanel(); }} />
{/if}
