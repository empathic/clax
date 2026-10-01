// The artifact view's three islands in Preact: the topbar's controls, the
// stage's overlays and the sidebar. Each renders the controller's state and
// calls its intents; the page around them is the skeleton.
import { type ComponentType, h, render } from "preact";
import { INDEX_FILE } from "../../../bridge/src/protocol";
import { registerShield } from "../caps/gesture";
import { Composer, Pins } from "../comments";
import { PromptDialog } from "../prompt";
import { shellPath } from "../route";
import { Sidebar } from "../sidebar";
import type { ArtifactController, ViewState } from "../view/artifact-controller";
import { ViewerName } from "../viewer-name";
import type { Islands } from "./index";

type IslandProps = { ctl: ArtifactController; s: ViewState };

/** The artifact is loaded and the frame mode decided: the view shows. */
const ready = (s: ViewState): s is ViewState & { data: NonNullable<ViewState["data"]> } => !s.error && !!s.data && s.origin !== undefined;

function Topbar({ ctl, s }: IslandProps) {
  if (!ready(s)) return null;
  const { deleted } = s;
  const shown = ctl.shown(s);
  const latest = ctl.latest(s);
  return (
    <>
      <button aria-pressed={s.commenting} class={s.commenting ? "primary" : ""} disabled={deleted} onClick={() => ctl.toggleComment()}>Comment</button>
      <button aria-pressed={s.panel} onClick={() => ctl.togglePanel()}>Threads ({ctl.openCount(s)})</button>
      {!s.narrow && <ViewerName setNotice={ctl.setNotice} onViewer={v => ctl.setMe(v)} />}
      <select value={shown} disabled={deleted} onChange={e => ctl.chooseVersion(Number((e.target as HTMLSelectElement).value))}>
        {s.data.versions.map(v => <option value={v.n} key={v.n}>v{v.n}{v.n === latest ? ` of ${latest}` : ""}{v.label ? ` · ${v.label}` : ""}</option>)}
      </select>
      {deleted
        ? <span class="hide-sm muted">open raw</span>
        : <a class="hide-sm" href={ctl.rawHref(s)} target="_blank" rel="noopener">open raw</a>}
      {navigator.clipboard && (
        <button disabled={deleted} onClick={() => ctl.copyLink()}>copy link</button>
      )}
    </>
  );
}

function Stage({ ctl, s }: IslandProps) {
  if (!ready(s)) return null;
  const { deleted, draft, newer, notice, ask } = s;
  const shown = ctl.shown(s);
  const latest = ctl.latest(s);
  const missing = ctl.missing(s);
  return (
    <>
      {deleted
        ? <p class="empty">This artifact was deleted.</p>
        : missing && <p class="empty">v{shown} has no page {missing}. <a href={shellPath(ctl.id, s.pinnedVersion, INDEX_FILE)}>Open the index</a></p>}
      {!deleted && !missing && <div class="frame-shield" aria-hidden="true" ref={registerShield}><div /><div /><div /><div /></div>}
      {s.hint && <p class="gesture-hint" role="status">{s.hint}</p>}
      {!deleted && !missing && <Pins threads={s.threads} resolved={s.resolved} file={s.file} onSelect={t => ctl.openPin(t)} onHover={t => ctl.hover(t)} />}
      {draft && <Composer key={draft.pickId} draft={draft} onText={v => ctl.composerInput(v)}
        onFocused={() => ctl.composerFocused(draft.pickId)} onCancel={() => ctl.cancelDraft()} onSubmit={body => ctl.submitDraft(body, draft)} />}
      {newer && !deleted && (
        <div class="banner"><span>v{newer} published</span><button class="primary" onClick={() => ctl.reloadLatest()}>Reload</button></div>
      )}
      {shown < latest && !newer && !deleted && <div class="banner"><span class="muted">viewing v{shown}; latest is v{latest}</span><a href={ctl.here(null, s)}>latest</a></div>}
      {notice && (
        <div class="banner notice" role="alert"><span>{notice}</span><button onClick={() => ctl.dismissNotice()}>Dismiss</button></div>
      )}
      {ask && <PromptDialog ask={ask} />}
    </>
  );
}

function SidebarPanel({ ctl, s }: IslandProps) {
  if (!ready(s) || !s.panel) return null;
  return (
    <Sidebar threads={s.threads} resolved={s.resolved} selected={s.selected} file={s.file} holds={f => ctl.holds(f, s)}
      me={s.me} header={s.narrow ? <ViewerName setNotice={ctl.setNotice} onViewer={v => ctl.setMe(v)} /> : undefined}
      onSelect={t => ctl.selectThread(t)}
      onHover={t => ctl.hover(t)}
      onSend={t => ctl.sendThread(t)}
      onResolve={t => ctl.resolveThread(t)}
      onReply={(t, body) => ctl.reply(t, body)} />
  );
}

// Each island renders at once when mounted, then once per turn after the
// controller's state changed (in a microtask, as Preact batches a component's
// state updates), with the latest state.
const island = (C: ComponentType<IslandProps>) => (target: HTMLElement, ctl: ArtifactController) => {
  let live = true;
  let queued = false;
  const draw = () => {
    queued = false;
    if (live) render(h(C, { ctl, s: ctl.state.get() }), target);
  };
  let first = true;
  const off = ctl.state.subscribe(() => {
    if (first) { first = false; draw(); return; }
    if (!queued) { queued = true; queueMicrotask(draw); }
  });
  return () => { live = false; off(); render(null, target); };
};

export const preactIslands: Islands = { topbar: island(Topbar), stage: island(Stage), sidebar: island(SidebarPanel) };
