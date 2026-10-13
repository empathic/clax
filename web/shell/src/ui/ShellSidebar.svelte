<script lang="ts">
  // The artifact view's thread sidebar: the controller's state and intents
  // wired to the sidebar. It loads with the sidebar's chunk, after the first
  // paint (sidebar-chunk.ts), so none of this is in the artifact entry.
  // Above a thread waiting for this viewer's approval, their approval card
  // (spec 2026-10-08-multi-agent-multi-person §9.5), loaded once one is.
  import type { Thread } from "../threads";
  import type { ArtifactController, Loaded, ViewState } from "../view/artifact-controller";
  import Sidebar from "./Sidebar.svelte";

  let { ctl, s }: { ctl: ArtifactController; s: ViewState & { data: Loaded } } = $props();
  // The threads whose approval card shows above them.
  let yours = $state<string[]>([]);
  const shown = (id: string, on: boolean) => { yours = [...yours.filter(x => x !== id), ...(on ? [id] : [])]; };
</script>

{#snippet above(t: Thread, now: Date)}
  {#if t.status === "open" && t.feedback_state?.state === "withheld"}
    {#await import("./ThreadApproval.svelte") then { default: ThreadApproval }}<ThreadApproval {t} {now} onShown={shown} />{/await}
  {/if}
{/snippet}

<Sidebar threads={s.threads} resolved={s.resolved} selected={s.selected} file={s.file} holds={f => ctl.holds(f, s)} me={s.me}
  versions={s.data.versions} shown={ctl.shown(s)} agent={s.data.artifact.owner_harness || "agent"}
  working={s.working} commenting={s.commenting} agents={s.agents} mine={s.attention?.open_in ?? []}
  decided={s.decided} onSeen={t => ctl.look(t)}
  onSelect={t => ctl.selectThread(t)} onHover={t => ctl.hover(t)} onSend={t => ctl.sendThread(t)}
  onResolve={t => ctl.resolveThread(t)} onReply={(t, body) => ctl.reply(t, body)}
  selection={s.selection} batchNote={s.batchNote} batchBusy={s.batchBusy} sendTo={s.sendTo}
  onToggle={(t, shift, order) => ctl.toggleSelect(t, shift, order)} onClear={() => ctl.clearSelection()} onNote={v => ctl.setBatchNote(v)}
  compose={{ version: ctl.shown(s), host: ctl }}
  onSendSelection={() => void ctl.sendSelection()} onSendUnsent={() => void ctl.sendUnsent()} onChoose={h => ctl.chooseTarget(h)} {above} {yours} />
