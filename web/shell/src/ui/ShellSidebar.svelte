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
  // the owner's (`sendsFor`). ClaxBot leads the Send menu where the
  // daemon lists it (`bot/send.svelte.ts`); a Send to it starts a run of
  // the threads sent and opens its panel.
  // In the Clax app, whose scheme answers nothing under `/api/`, a thread's
  // clip is fetched over the transport and shown by object URL
  // (`api-image.ts`, loaded only there); a browser loads `clip_url` itself.
  // While the artifact's Comments switch is off, the sidebar has no New
  // comment box and its cards no reply box (`noReply`, which its empty Open
  // list explains); Resolve and Reopen stay, and Send for a viewer who may
  // resolve its threads (`may_resolve`), as the daemon allows them.
  // A spectator's sidebar is read-only.
  // The drawer's controls (DrawerChrome) go around it; the extension's side
  // panel mounts Sidebar alone, without them.
  import { COMMENTS_OFF } from "../failure";
  import type { Thread } from "../threads";
  import { onHost } from "../transport";
  import { ownTarget, sendsFor } from "../view/send-menu";
  import { unsent } from "../view/batch-model";
  import { BOT, botView, sendToBot, withBot } from "../bot/send.svelte";
  import { type ArtifactController, type Loaded, type ViewState, commentsOff } from "../view/artifact-controller";
  import { emptyLayer, writersIn } from "../view/live-layers";
  import DrawerChrome from "./DrawerChrome.svelte";
  import LiveWriters from "./LiveWriters.svelte";
  import Sidebar from "./Sidebar.svelte";

  let { ctl, s }: { ctl: ArtifactController; s: ViewState & { data: Loaded } } = $props();
  // Who writes where now (the live layers), named as the view knows them.
  const live = { writers, onText: (tid: string | null, text: string) => ctl.liveView?.text(tid, text) };
  const agents = $derived(withBot(s.agents));
  const target = $derived(botView.chosen && botView.agent?.live ? BOT : s.sendTo ?? ownTarget(s.agents, sendsFor(s.agents, s.me)));
  // A Send of `ids` (`send` for any agent but ClaxBot).
  const go = (ids: () => string[], send: () => unknown) => {
    if (target === BOT) { void sendToBot(ctl, ids()).then(ok => { if (ok && s.selection.ids.length) ctl.clearSelection(); }); return; }
    if (target && target !== s.sendTo) ctl.chooseTarget(target);
    send();
  };
  const choose = (h: string) => { botView.choose(ctl.id, h === BOT); if (h !== BOT) ctl.chooseTarget(h); };
  const clip = onHost() ? async (t: Thread) => (t.clip_url ? (await import("../api-image")).apiImageSrc(t.clip_url) : null) : undefined;
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
  working={s.working} commenting={s.commenting} {agents} mine={s.attention?.open_in ?? []}
  decided={s.decided} onSeen={t => ctl.look(t)} {clip}
  onSelect={t => ctl.selectThread(t)} onHover={t => ctl.hover(t)} onSend={t => go(() => [t.id], () => ctl.sendThread(t))}
  onResolve={t => ctl.resolveThread(t)} onReply={(t, body) => ctl.reply(t, body)}
  selection={s.selection} batchNote={s.batchNote} batchBusy={s.batchBusy} sendTo={target}
  onToggle={(t, shift, order) => ctl.toggleSelect(t, shift, order)} onClear={() => ctl.clearSelection()} onNote={v => ctl.setBatchNote(v)}
  compose={s.spectator || commentsOff(s) ? undefined : { version: ctl.shown(s), host: ctl }} readOnly={!!s.spectator} noReply={commentsOff(s) ? `${COMMENTS_OFF}.` : undefined}
  noSend={commentsOff(s) && s.data.artifact.may_resolve === false} live={s.spectator ? undefined : live}
  onSendSelection={() => go(() => s.selection.ids, () => ctl.sendSelection())} onSendUnsent={() => go(() => unsent(s.threads).map(t => t.id), () => ctl.sendUnsent())} onChoose={choose} {above} />
</DrawerChrome>
