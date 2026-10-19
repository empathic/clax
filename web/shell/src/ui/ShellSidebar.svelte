<script lang="ts">
  // The artifact view's thread sidebar: the controller's state and intents
  // wired to the sidebar. It loads with the sidebar's chunk, after the first
  // paint (sidebar-chunk.ts), so none of this is in the artifact entry.
  // Above a sent thread with an open approval request of this viewer's
  // (the owner's or a connected person's browser), their approval card
  // (spec 2026-10-08-multi-agent-multi-person §9.5), whichever of the
  // thread's comments it holds; loaded once one is sent.
  // With no target chosen, a Send goes to this viewer's own most recent
  // live agent (`ownTarget`), chosen for it as it sends; a LAN viewer's, to
  // the owner's (`sendsFor`).
  // The drawer's controls (DrawerChrome) go around it; the extension's side
  // panel mounts Sidebar alone, without them.
  import type { Thread } from "../threads";
  import { ownTarget, sendsFor } from "../view/send-menu";
  import type { ArtifactController, Loaded, ViewState } from "../view/artifact-controller";
  import { emptyLayer, writersIn } from "../view/live-layers";
  import DrawerChrome from "./DrawerChrome.svelte";
  import LiveWriters from "./LiveWriters.svelte";
  import Sidebar from "./Sidebar.svelte";

  let { ctl, s }: { ctl: ArtifactController; s: ViewState & { data: Loaded } } = $props();
  // Who writes where now (the live layers), named as the view knows them.
  const live = { writers, onText: (tid: string | null, text: string) => ctl.liveView?.text(tid, text) };
  const target = $derived(s.sendTo ?? ownTarget(s.agents, sendsFor(s.agents, s.me)));
  const go = (send: () => unknown) => { if (target && target !== s.sendTo) ctl.chooseTarget(target); send(); };
</script>

{#snippet writers(tid: string | null)}
  <LiveWriters writers={writersIn(s.live?.layer ?? emptyLayer(), tid, s.live?.now ?? 0).map(w => ({ ...w, name: ctl.liveView?.nameOf(w.person_id) ?? "Viewer" }))} />
{/snippet}

{#snippet above(t: Thread, now: Date)}
  {#if t.status === "open" && t.sent_to_agent && (s.me?.owner || s.me?.person)}
    {#await import("./ThreadApproval.svelte") then { default: ThreadApproval }}<ThreadApproval {t} {now} />{/await}
  {/if}
{/snippet}

<DrawerChrome onClose={() => ctl.togglePanel()}>
<Sidebar threads={s.threads} resolved={s.resolved} selected={s.selected} file={s.file} holds={f => ctl.holds(f, s)} me={s.me}
  versions={s.data.versions} shown={ctl.shown(s)} agent={s.data.artifact.owner_name || s.data.artifact.owner_harness || "agent"}
  working={s.working} commenting={s.commenting} agents={s.agents} mine={s.attention?.open_in ?? []}
  decided={s.decided} onSeen={t => ctl.look(t)}
  onSelect={t => ctl.selectThread(t)} onHover={t => ctl.hover(t)} onSend={t => go(() => ctl.sendThread(t))}
  onResolve={t => ctl.resolveThread(t)} onReply={(t, body) => ctl.reply(t, body)}
  selection={s.selection} batchNote={s.batchNote} batchBusy={s.batchBusy} sendTo={target}
  onToggle={(t, shift, order) => ctl.toggleSelect(t, shift, order)} onClear={() => ctl.clearSelection()} onNote={v => ctl.setBatchNote(v)}
  compose={s.spectator ? undefined : { version: ctl.shown(s), host: ctl }} readOnly={!!s.spectator} live={s.spectator ? undefined : live}
  onSendSelection={() => go(() => ctl.sendSelection())} onSendUnsent={() => go(() => ctl.sendUnsent())} onChoose={h => ctl.chooseTarget(h)} {above} />
</DrawerChrome>
