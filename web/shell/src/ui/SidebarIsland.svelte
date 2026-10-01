<script lang="ts">
  // The sidebar island: the comment threads, and the "Your name" field on
  // narrow screens, once the artifact is loaded and the frame mode decided.
  import { fromStore } from "svelte/store";
  import type { ArtifactController } from "../view/artifact-controller";
  import Sidebar from "./Sidebar.svelte";
  import ViewerName from "./ViewerName.svelte";

  // An island's controller is fixed for its lifetime: the mount passes it once.
  let { ctl }: { ctl: ArtifactController } = $props();
  // svelte-ignore state_referenced_locally
  const view = fromStore(ctl.state);
  const s = $derived(view.current);
</script>

{#snippet nameField()}
  <ViewerName setNotice={ctl.setNotice} onViewer={v => ctl.setMe(v)} />
{/snippet}

{#if !s.error && s.data && s.origin !== undefined && s.panel}
  <Sidebar threads={s.threads} resolved={s.resolved} selected={s.selected} file={s.file} holds={f => ctl.holds(f, s)} me={s.me}
    header={s.narrow ? nameField : undefined}
    onSelect={t => ctl.selectThread(t)} onHover={t => ctl.hover(t)} onSend={t => ctl.sendThread(t)}
    onResolve={t => ctl.resolveThread(t)} onReply={(t, body) => ctl.reply(t, body)} />
{/if}
