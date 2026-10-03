// The artifact view's state and behaviour, framework-free. Components render
// `state` and call the intent methods; the frame is a FrameHost the mount
// gives it.
import { type AnchorResult, INDEX_FILE, type ShellToBridge } from "../../../bridge/src/protocol";
import { ApiError, type Artifact, type Attention, type SampleStatus, type Version, getArtifact, getSampleStatus, getToken, putLooked, putSeen } from "../api";
import { acceptByeFromFrame, acceptFromFrame, helloMatches, sendToFrame } from "../bridge-link";
import type { Declared } from "../caps/availability";
import { HINT_MS, frameGesture, onShieldPress, pickHintAllowed, setForwardedKeys } from "../caps/gesture";
import { CapabilityHost, type CommentsUi } from "../caps/host";
import { type ArtifactEvent, subscribe } from "../events";
import { LOAD_FAILED, OPEN_FAILED, PART_FAILED, POST_FAILED, RESOLVE_FAILED, SEND_FAILED, SHEET_FAILED, report, scopedNotice } from "../failure";
import { nav } from "../nav";
import { artifactOrigin, cachedOriginOk, pageSrc, probeOrigin } from "../origin";
import { parseShellPath, shellPath } from "../route";
import { type Thread, type Viewer, addComment, createThread, currentViewer, getViewer, listThreads, onViewer, resolveThread, seedViewer, sendBatch, sendToAgent, upsert } from "../threads";
import { afterPaint } from "./after-paint";
import { EMPTY_SELECTION, type Selection, prune, toggle, unsent } from "./batch-model";
import { AnchorHandles } from "./anchor-handles";
import type { Decided } from "./changelog-model";
import { rallyOnce } from "./rally";
import { type Boot, rememberFrameMode } from "./boot";
import { CAPTURE_LATE, type Draft, MAX_CLIP_BYTES, captureWait, nextDraft, withClip } from "./composer-model";
import { FrameGate } from "./frame-gate";
import type { PresenceView } from "./presence-model";
import type { PresenceReporter } from "./presence-reporter";
import type { FrameHost } from "./frame-host";
import { type KeyAction, holdKeysAcrossLoad, keyAction, keysHeldAtLoad } from "./keys";
import { keyboardTrail } from "./trail";
import { type Ask, promptQueue } from "./prompt-queue";
import { defaultTarget, rememberTarget } from "./send-target";
import { Store } from "./store";
import { type ThreadChange, ThreadSync } from "./thread-sync";
import { setUrl, validHash } from "./url";
import type { SetNotice } from "./viewer-name-model";
import type { AgentView, Working } from "./working-model";

/** `file` is the page the frame opens on, from the shell URL (`index.html` when it names none). */
export type ArtifactProps = { id: string; pinnedVersion: number | null; file?: string };
/** `attention` is the viewer's, when the request carried the viewer cookie. */
export type Loaded = { artifact: Artifact; versions: Version[]; attention?: Attention };
export type ViewState = {
  /** The version the shell URL pins, null for the latest (a prop). */
  pinnedVersion: number | null;
  /** The page the frame opens on, from the shell URL (a prop). */
  startFile: string;
  data: Loaded | null;
  error: string | null;
  /** The artifact origin, null in sandbox mode, undefined until decided. */
  origin: string | null | undefined;
  newer: number | null;
  deleted: boolean;
  commenting: boolean;
  panel: boolean;
  narrow: boolean;
  threads: Thread[];
  resolved: Record<string, AnchorResult>;
  draft: Draft | null;
  selected: string | null;
  /** The thread whose card or pin the pointer is over; it, else the selected
   * thread, is the frame's focus (a drawn area is outlined dashed). */
  hovered: string | null;
  /** Posts and sends to the agent in flight (an area compose waits for none). */
  busy: number;
  notice: string | null;
  /** A brief hint when the viewer's press did not count as their gesture in
   * the page (a mouse press on the shield, always; a refused pick in comment
   * mode, only when it can be the viewer's own press, `pickHintAllowed`): the
   * shell could not see the pointer move there since their input to it. */
  hint: string | null;
  me: Viewer | null;
  ask: Ask | null;
  /** The published file of the page in the frame, from its latest matching
   * hello (the URL's file until then): pins, anchor resolution, and scroll-to
   * apply to its threads only, and the shell URL names it. Null while the
   * frame shows a document that did not greet: no pins are drawn over it. */
  file: string | null;
  /** The sheet over the view: the keys (spec §8), or none. */
  sheet: "keys" | null;
  /** Who is working on the artifact now, from its view and then its `working` events. */
  working: Working[];
  /** The viewer's attention on this artifact; null without a viewer. */
  attention: Attention | null;
  /** The changelog this load decided (spec §8): the Addressed group, the
   * version button's dot and the summary line; null until the view is ready. */
  decided: Decided | null;
  /** "Rally of 10" (spec §8, "Look"): this load is this browser's first view of the artifact's tenth version. */
  rally: boolean;
  /** Who has the artifact open (spec §10, "Presence"), from the daemon. */
  presence: PresenceView[];
  /** Whether this viewer shares where they look ("Share where I'm looking"; per browser, on by default). */
  shareWhere: boolean;
  /** The menu open from the top bar. */
  menu: "versions" | "people" | null;
  /** This viewer's looked-at marks on this artifact's threads (thread ID to when). */
  looked: Record<string, string>;
  /** The artifact's agents, live first and most recently active first, as
   * the daemon lists them: from the load, then from each refetch. */
  agents: AgentView[];
  /** The threads ticked for a batch send (spec §8), in the order ticked. */
  selection: Selection;
  /** The note the selection bar sends with the batch. */
  batchNote: string;
  /** A batch send is in flight. */
  batchBusy: boolean;
  /** The agent handle every Send goes to (`defaultTarget`, or the viewer's
   * pick); null when no agent is live, and a Send then goes without `to`. */
  sendTo: string | null;
  /** This artifact's calls to Claude today and the cap, once the owner's
   * browser has asked the daemon (null until then, and on a LAN view). */
  sampleCalls: { n: number; cap: number | null } | null;
};

/** The artifact is loaded and the frame mode decided: the islands show. */
export function viewReady(s: ViewState): s is ViewState & { data: Loaded } {
  return !s.error && !!s.data && s.origin !== undefined;
}

/** How long a pick's start stays valid for its pick (longer than the
 * longest clip render, an area's 12 s). */
const PICK_WAIT_MS = 20_000;

/** The notice kind for a thread the daemon kept without its screenshot. */
const CLIP_DROPPED = "Posted without its screenshot";

/** How long a page the shell sent the frame to may take to greet before the
 * jump is given up (settable for tests). */
export const pageWait = { ms: 5000 };

export const MOVE_TO_PICK = "Move the pointer to pick";
export const MOVE_TO_CLICK = "Move the pointer, then click again";

/** A load of the content frame, kept in `held`. */
const LOADED = Symbol("loaded");

const media = (q: string) => typeof matchMedia === "function" && matchMedia(q).matches;
const SHARE_WHERE_KEY = "clax.shareWhere";
/** The stored "Share where I'm looking" choice; on unless turned off. */
function readShareWhere(): boolean {
  try { return localStorage.getItem(SHARE_WHERE_KEY) !== "0"; } catch { return true; }
}
const threadIds = (ts: Thread[]) => ts.map(t => t.id).join(",");

export class ArtifactController {
  readonly state: Store<ViewState>;
  readonly id: string;
  /** The URL fragment the frame opens at. */
  readonly startHash: string;
  /** The content frame; the mount sets it before `start`. */
  frame: FrameHost | null = null;
  readonly commentsUi: CommentsUi;

  private disposed = false;
  /** The event stream may (re)open: from `start` until `dispose`. */
  private live = false;
  private latestKnown = 0;
  /** The artifact, version and origin the gate was last reset for. */
  private gateFor = "";
  /** The view (artifact, version, origin and data) the capability host was made for. */
  private hostFor: { key: string; data: Loaded } | null = null;
  // Whether the frame's latest hello named the shown artifact and version
  // (see FrameGate).
  private readonly gate = new FrameGate();
  private readonly anchorIds = new AnchorHandles();
  private readonly threadLoad: ThreadSync;
  // The pick whose start arrived with the viewer's gesture in the frame and
  // opened the composer, while its screenshot is still to come, with when
  // the start arrived.
  private pendingPick: { pickId: string; at: number } | null = null;
  /** The bridge's lazy parts the greeted page reported it could not load
   * (`clax:degraded`); forgotten whenever the gate closes (another page,
   * version or origin, or a document that loaded without greeting) and at
   * the next greeting. */
  private failedParts = new Set<keyof typeof PART_FAILED>();
  // The pick ID of the open composer when a pick made in comment mode opened
  // it: comment mode, off while that composer is open, comes back on when it
  // closes.
  private resumeAfter: string | null = null;
  // What the open composer holds, so a page's open never replaces typed text.
  private composerText = "";
  // A page anchors threads itself (comments.customAnchors): pins come from
  // its placements only, and the frame is not asked to resolve anchors.
  private customLive = false;
  private host: CapabilityHost | null = null;
  // How many of this view's own page publishes are in flight (the `artifact`
  // handler counts them; one that ends in a reload keeps its count). A page
  // publish by another view that arrives meanwhile is remembered in
  // `deferredPublish` (its version) and applied once the count drops to 0
  // without a reload: a reload to the latest, or Reload in the top bar when pinned.
  private readonly ownPublish: { active: number; settled?(): void } = { active: 0 };
  private deferredPublish: number | null = null;
  // A thread on another page the viewer opened: the frame was sent to that
  // page, and it is scrolled to once that page greets, or given up after
  // `pageWait.ms` with a notice.
  private pendingScroll: { thread: Thread; timer: ReturnType<typeof setTimeout> } | null = null;
  // The frame's latest known fragment.
  private frameHash: string;
  // The pending animation frame that copies `frameHash` into the address bar.
  private hashFrame = 0;
  private hintTimer: ReturnType<typeof setTimeout> | undefined;
  private captureTimer: ReturnType<typeof setTimeout> | undefined;
  private stream: (() => void) | null = null;
  private readonly offs: (() => void)[] = [];
  /** The snapshot before this turn's first change, until the turn renders. */
  private turnFrom: ViewState | null = null;
  /** Rendered changes (from, to, the host at that render) whose reactions
   * have not run yet. */
  private readonly reactions: [ViewState, ViewState, CapabilityHost | null][] = [];
  private flushScheduled = false;
  /** The host the latest reaction pass told of the UI. */
  private toldHost: CapabilityHost | null = null;
  private cancelFlush: () => void = () => {};
  /** Whether this load was one the page caused (`holdKeysAcrossLoad`), read once. */
  private readonly heldAtLoad = keysHeldAtLoad();
  /** Whether the viewer's keys are meant for the shell, so the shell's keys
   * (C and ?) may act.
   *
   * It starts set on a fresh load, and clear on a load the page caused (its
   * publish reloading the shell, `holdKeysAcrossLoad`), since the viewer may
   * still be typing for the page. It is cleared when the shell window loses
   * focus (to the page's frame or anywhere else), and when a prompt or
   * composer the page raised opens or closes, so focus that falls to the body
   * while the viewer is still typing for the page never makes their keys the
   * shell's.
   *
   * It is set again on a trusted press in the shell, or on focus landing on a
   * specific shell control: not the body, the frame, a dialog or the
   * composer. The page cannot put focus on such a control by script: its
   * `parent.focus()` reaches only the body, and the shell hands that focus
   * back to the frame (below). The shell's own
   * script focus lands on the prompt and the composer, which do not count;
   * on the keys sheet's Close, inside its dialog; and, when the sheet
   * closes, back where focus was before it opened, which may be a shell
   * control. The sheet also closes when a page's prompt opens, and that
   * give-back sets it while the prompt is open, but the prompt's close
   * clears it again.
   *
   * Focus the page pushes out of its frame to the shell's body goes back to
   * the frame (`listen`): on the viewer's next key, which is swallowed, or a
   * task later. Their next Tab, typed for the page, does not land on a shell
   * control: it neither sets this nor reaches the shell's buttons, where a
   * Space would press them natively. */
  private keysOwned = !this.heldAtLoad;
  // A prompt closes the keys sheet, so nothing covers or disables the prompt.
  private readonly prompt = promptQueue(ask => this.set(ask ? { ask, sheet: null } : { ask }));
  /** While a guessed subdomain frame awaits the probe (`decideOrigin`), the
   * messages to the shell and the frame's loads (`LOADED`), in order. */
  private held: (MessageEvent | typeof LOADED)[] | null = null;

  /** `init.boot`: the daemon's first-load data for this artifact, read in
   * place of the first requests (the artifact, its threads, the viewer). */
  constructor({ id, pinnedVersion, file: startFile = INDEX_FILE }: ArtifactProps, private readonly init: { boot?: Boot | null } = {}) {
    this.id = id;
    this.startHash = location.hash;
    this.frameHash = this.startHash;
    this.state = new Store<ViewState>({
      pinnedVersion, startFile,
      data: null, error: null, origin: undefined, newer: null, deleted: false, commenting: false,
      panel: media("(min-width: 900px)"), narrow: media("(max-width: 480px)"),
      threads: [], resolved: {}, draft: null, selected: null, hovered: null, busy: 0,
      notice: null, hint: null, me: null, ask: null, file: startFile, sheet: null, working: [], attention: null,
      decided: null, rally: false, menu: null, looked: {}, presence: [], shareWhere: readShareWhere(),
      agents: [], selection: EMPTY_SELECTION, batchNote: "", batchBusy: false, sendTo: null, sampleCalls: null,
    });
    this.threadLoad = new ThreadSync(f => this.set(s => ({ threads: f(s.threads) })));
    this.ownPublish.settled = () => {
      const n = this.deferredPublish;
      this.deferredPublish = null;
      if (n === null) return;
      if (this.pinnedVersion === null) { holdKeysAcrossLoad(); nav.assign(this.here(null)); }
      else if (n > this.latestKnown) { this.latestKnown = n; this.set({ newer: n }); }
    };
    // The comment UI as the `comments` capability drives it; it reads the
    // controller's current state, so the host always sees the latest.
    this.commentsUi = {
      openComposer: (d, opts) => {
        const next = nextDraft(this.s.draft, this.composerText, d, opts);
        if (!next) return false;
        this.set({ draft: { ...next, byPage: true } });
        return true;
      },
      attachClip: (token, clip, clipError) => this.set(s => ({ draft: withClip(s.draft, token, clip, clipError) })),
      upsert: t => this.changeThreads(ts => upsert(ts, t)),
      remove: tid => { this.changeThreads(ts => ts.filter(t => t.id !== tid)); this.set(s => ({ selected: s.selected === tid ? null : s.selected })); },
      setCustom: live => {
        this.customLive = live;
        if (live) this.set({ resolved: {} });
        else this.resolveAll();
      },
      place: rects => this.set({ resolved: Object.fromEntries(Object.entries(rects).map(([tid, rect]) => [tid, { id: tid, found: true, method: "custom" as const, rect }])) }),
      select: tid => this.set({ panel: true, selected: tid }),
      exitMode: () => { if (!this.composerText.trim()) this.set({ commenting: false }); },
      dismiss: () => {
        if (this.s.draft) {
          if (this.composerText.trim()) return false;
          this.set({ draft: null });
          return true;
        }
        if (this.s.selected === null) return false;
        this.set({ selected: null });
        return true;
      },
      enterMode: () => this.set({ commenting: true }),
      state: () => ({ mode: this.s.commenting, composing: this.s.draft !== null, threads: this.s.threads, selected: this.s.selected, busy: this.s.busy > 0 }),
    };
  }

  private get s(): ViewState {
    return this.state.get();
  }

  /** The version the shell URL pins, null for the latest. */
  get pinnedVersion(): number | null {
    return this.s.pinnedVersion;
  }

  /** The page the frame opens on, from the shell URL. */
  get startFile(): string {
    return this.s.startFile;
  }

  // ---- derived values (pure over a snapshot, so components can derive them) ----

  shown(s: ViewState = this.s): number {
    return s.pinnedVersion ?? s.data?.artifact.current_version ?? 0;
  }

  latest(s: ViewState = this.s): number {
    return s.data?.artifact.current_version ?? 0;
  }

  private version(s: ViewState = this.s): Version | undefined {
    return s.data?.versions.find(v => v.n === this.shown(s));
  }

  /** A page the URL names that the shown version does not hold (the index is
   * always there): it gets a message instead of the daemon's 404 in the frame. */
  missing(s: ViewState = this.s): string | null {
    const version = this.version(s);
    return s.startFile !== INDEX_FILE && version !== undefined && !Object.hasOwn(version.files, s.startFile) ? s.startFile : null;
  }

  /** Whether the shown version holds `f` (the index always; any file while the
   * versions are unknown). */
  holds(f: string, s: ViewState = this.s): boolean {
    const v = this.version(s);
    return f === INDEX_FILE || !v || Object.hasOwn(v.files, f);
  }

  /** Whether `f` is an HTML page of the shown version. */
  private isPage(f: string): boolean {
    const v = this.version();
    if (f === INDEX_FILE || !v) return true;
    return Object.hasOwn(v.files, f) && v.files[f].content_type.split(";")[0].trim().toLowerCase() === "text/html";
  }

  /** The page the frame shows, else the one the shell URL names. */
  private pageNow(s: ViewState): string {
    const r = parseShellPath(location.pathname);
    return s.file ?? (r.kind === "artifact" ? r.file : INDEX_FILE);
  }

  /** The shell URL of the current page in `version` (null: the latest). */
  here(version: number | null, s: ViewState = this.s): string {
    return shellPath(this.id, version, this.pageNow(s), this.shown(s));
  }

  /** The "open raw" link: the current page in the shown version. */
  rawHref(s: ViewState = this.s): string {
    return pageSrc(this.id, this.shown(s), s.origin ?? null, this.pageNow(s));
  }

  openCount(s: ViewState = this.s): number {
    return s.threads.filter(t => t.status === "open").length;
  }

  /** The pin numbering the sidebar uses (`sidebarSections(...).numbers`,
   * `pinPlaces`), by thread ID: the open threads found on the page shown. */
  numbers(s: ViewState = this.s): Map<string, number> {
    const attached = s.threads.filter(t => t.status === "open" && t.anchor.file === s.file && !(s.resolved[t.id] && !s.resolved[t.id].found));
    return new Map(attached.map((t, i) => [t.id, i + 1]));
  }

  // ---- state and reactions ----

  private set(patch: Partial<ViewState> | ((s: ViewState) => Partial<ViewState>)): void {
    if (this.disposed) return;
    const prev = this.s;
    this.state.set(patch);
    // A prompt or composer comes from the page (a capability, or the bridge's
    // pick). Its opening and its closing, however it closed (a key, a press
    // on its buttons or backdrop), give the keys up: a press that closed it
    // came before the close, so the viewer's next keys may still be meant for
    // the page (`keysOwned`).
    if (!!prev.ask !== !!this.s.ask || !!prev.draft !== !!this.s.draft) this.keysOwned = false;
    // Comment mode cannot come on in a page whose comment part did not load,
    // however it was asked for: the viewer is told again instead.
    if (this.s.commenting && this.failedParts.has("comment")) this.state.set({ commenting: false, notice: `${PART_FAILED.comment}.` });
    // A ticked thread that was deleted or resolved, or whose artifact went, leaves the selection.
    if (prev.threads !== this.s.threads || prev.deleted !== this.s.deleted) {
      const sel = prune(this.s.selection, this.s.threads, this.s.deleted);
      if (sel !== this.s.selection) this.state.set({ selection: sel });
    }
    if (this.turnFrom === null && this.s !== prev) {
      this.turnFrom = prev;
      queueMicrotask(() => this.rendered());
    }
  }

  /** The end of a turn's changes: a render. The reactions to the previous
   * render still pending run first, and this render's run after the next
   * paint, with the pass already scheduled, if any (one is scheduled when the
   * queue of reactions stops being empty, and kept until it ran). */
  private rendered(): void {
    const from = this.turnFrom;
    this.turnFrom = null;
    if (!from || this.disposed) return;
    const to = this.s;
    this.runReactions(false);
    // Render-time work, as the frame unmounted in the render that showed the
    // deletion: a deleted artifact's frame goes before anything paints, so
    // its gate never stays open beside the message.
    if (from.deleted !== to.deleted) this.showFrame();
    this.reactions.push([from, to, this.host]);
    this.schedulePass();
  }

  private schedulePass(): void {
    if (this.flushScheduled) return;
    this.flushScheduled = true;
    this.cancelFlush = afterPaint(() => { this.flushScheduled = false; this.runReactions(true); });
  }

  /** Runs the pending reactions; `painted` is false for the run at the
   * start of a render, which only flushes the previous renders' reactions. */
  private runReactions(painted: boolean): void {
    for (const [prev, next, host] of this.reactions.splice(0)) if (!this.disposed) this.react(prev, next, host);
    // A host made since the last pass, by no rendered change, hears of the UI
    // once, in the pass after the paint.
    if (painted && !this.disposed && this.host && this.host !== this.toldHost) {
      this.toldHost = this.host;
      this.host.uiChanged();
    }
  }

  /** The reactions after the render that went from `prev` to `next`: values
   * taken from that render come from `next`, others from the current state,
   * and `host` is the one that render made or kept. */
  private react(prev: ViewState, next: ViewState, host: CapabilityHost | null): void {
    if (threadIds(prev.threads) !== threadIds(next.threads) || this.shown(prev) !== this.shown(next) || prev.origin !== next.origin) this.resolveAll();
    if (prev.commenting !== next.commenting) this.send({ type: "clax:comment-mode", on: next.commenting });
    if (prev.draft !== next.draft) this.draftChanged(next.draft, next.deleted);
    if (prev.hovered !== next.hovered || prev.selected !== next.selected || prev.threads !== next.threads) this.sendFocus();
    if (prev.draft?.clipToken !== next.draft?.clipToken || prev.draft?.capturing !== next.draft?.capturing) this.armCapture(next.draft);
    if (prev.data !== next.data || prev.origin !== next.origin) this.decideChangelog();
    if (prev.me !== next.me || prev.data !== next.data || prev.origin !== next.origin) this.writeSeen();
    if (prev.me !== next.me || prev.deleted !== next.deleted) this.reporter?.report(true);
    else if (prev.selected !== next.selected || prev.draft?.anchor !== next.draft?.anchor) this.reporter?.report();
    if (prev.commenting !== next.commenting || prev.draft !== next.draft || prev.selected !== next.selected || prev.threads !== next.threads || prev.file !== next.file || prev.busy !== next.busy || host !== this.toldHost) {
      this.toldHost = host;
      host?.uiChanged();
    }
  }

  // The pick's composer closed (posted, cancelled, or dismissed): comment mode
  // comes back on, so the viewer can pick the next target at once. One
  // replaced by another composer, or closed after the viewer pressed Comment
  // or the artifact was deleted, does not.
  private draftChanged(draft: Draft | null, deleted: boolean): void {
    const pick = this.resumeAfter;
    if (pick === null || draft?.pickId === pick) return;
    this.resumeAfter = null;
    if (!draft && !deleted) this.set({ commenting: true });
  }

  // A screenshot still being taken that never arrives: the composer says so,
  // and a post it queued goes ahead without one.
  private armCapture(draft: Draft | null): void {
    clearTimeout(this.captureTimer);
    const token = draft?.capturing ? draft.clipToken : undefined;
    if (!token) return;
    this.captureTimer = setTimeout(() => this.set(s => ({ draft: withClip(s.draft, token, null, CAPTURE_LATE) })), captureWait.ms);
  }

  private changeThreads(f: ThreadChange): void {
    this.threadLoad.change(f);
  }

  readonly setNotice: SetNotice = u => this.set(s => ({ notice: typeof u === "function" ? u(s.notice) : u }));

  // A success clears only a notice its own kind of call raised, so the viewer
  // lookup finishing after a failed thread load cannot hide that failure.
  private noticeFor(prefix: string): (text: string | null) => void {
    return scopedNotice(this.setNotice, prefix);
  }

  /** The notice for a part the greeted page could not load. Comment mode's
   * outranks the others: without it no pick, so no clip, can happen. */
  private showPartFailed(part: keyof typeof PART_FAILED): void {
    if (part !== "comment" && this.failedParts.has("comment")) return;
    const text = PART_FAILED[part];
    this.noticeFor(text)(`${text}.`);
  }

  private showHint(text: string): void {
    this.set({ hint: text });
    clearTimeout(this.hintTimer);
    this.hintTimer = setTimeout(() => this.set({ hint: null }), HINT_MS);
  }

  private whileBusy<T>(p: Promise<T>): Promise<T> {
    this.set(s => ({ busy: s.busy + 1 }));
    return p.finally(() => this.set(s => ({ busy: s.busy - 1 })));
  }

  // ---- the frame ----

  private frameWin(): Window | null {
    return this.frame?.el?.contentWindow ?? null;
  }

  /** Posts `m` to the frame, except to a frame still held for the probe
   * (`held`), and, in sandbox mode, where the post cannot name its receiver's
   * origin, to a frame whose document has not greeted (`gate`). Nothing is
   * lost: the hello that opens the gate sends the welcome, the anchors and
   * the focus again. */
  private send(m: ShellToBridge): void {
    if (this.held || (!this.s.origin && !this.gate.open)) return;
    sendToFrame(this.frameWin(), this.s.origin ?? null, m);
  }

  /** Runs when the artifact or the frame mode becomes known, before the frame
   * for them exists: the gate is reset and the capability host made first, so
   * a hello or a request that arrives as soon as the frame is inserted is
   * judged against the shown artifact and version. */
  private viewChanged(): void {
    const s = this.s;
    if (!s.data || s.origin === undefined) return;
    const shown = this.shown();
    // Reset before the frame for a new artifact, version, or origin is
    // inserted: its hello can arrive as soon as it is.
    const key = `${this.id}/${shown}/${s.origin}`;
    if (this.gateFor !== key) {
      this.gateFor = key;
      this.gate.reset();
      this.failedParts.clear();
    }
    // Made before the frame it serves is inserted: a hello and a capability
    // request can arrive as soon as it is.
    if (this.hostFor?.key !== key || this.hostFor.data !== s.data) {
      this.hostFor = { key, data: s.data };
      // A replaced host (another artifact, version, or view) and the host at
      // dispose are disposed, so their timers and late results never reach a frame.
      this.host?.dispose();
      const data = s.data;
      this.host = new CapabilityHost(getToken().then(token => {
        let status: Promise<SampleStatus | null> | null = null;
        return {
          aid: this.id,
          version: shown,
          pinned: this.pinnedVersion !== null,
          token,
          viewer: currentViewer,
          declared: (data.artifact.capabilities ?? {}) as Declared,
          prompt: this.prompt,
          post: m => { if (this.gate.open) this.send(m); },
          reload: () => { holdKeysAcrossLoad(); nav.assign(this.here(null)); },
          ownPublish: this.ownPublish,
          page: () => this.s.file,
          comments: this.commentsUi,
          working: () => this.s.working,
          files: data.versions.find(v => v.n === shown)?.files,
          sampleStatus: token === null ? undefined : () => status ??= getSampleStatus(this.id, token),
          onSampleCalls: (n: number, cap: number | null) => this.set({ sampleCalls: { n, cap } }),
        };
      }));
      // It hears of the UI in the next reaction pass, as the effect that
      // depended on the host did after the render that made it.
      this.schedulePass();
    }
    this.showFrame();
  }

  /** The content frame for the shown version and frame mode, opened on the
   * URL's page at its fragment; none once the artifact is deleted or when the
   * version lacks that page. */
  private showFrame(): void {
    const s = this.s;
    if (!this.frame || !s.data || s.origin === undefined) return;
    if (s.deleted || this.missing()) { this.frame.remove(); return; }
    this.frame.show(pageSrc(this.id, this.shown(), s.origin, this.startFile) + this.startHash, s.origin === null, `${this.shown()}-${s.origin ? "o" : "s"}`);
  }

  /** The frame loaded a document; one that never greeted since the previous
   * load closes the gate and loses its page and pins. */
  frameLoaded(): void {
    if (this.disposed) return;
    if (this.held) { this.held.push(LOADED); return; }
    if (this.gate.load()) { this.leaveDocument(); this.failedParts.clear(); this.set({ file: null, resolved: {} }); }
  }

  private resolveAll(): void {
    if (this.customLive) return;
    const file = this.s.file;
    this.send({ type: "clax:resolve-anchors", requestId: `r${Date.now()}`, anchors: this.s.threads.filter(t => t.anchor.file === file).map(t => ({ id: this.anchorIds.handle(t.id), anchor: t.anchor, sameVersion: t.version_n === this.shown() })) });
  }

  /** Tells the frame which thread's drawn area to outline dashed: the
   * hovered, else the selected thread, by the handle the frame knows it by
   * (none when it was not sent to this page). */
  private sendFocus(): void {
    const tid = this.s.hovered ?? this.s.selected;
    this.send({ type: "clax:focus", id: tid === null ? null : this.anchorIds.known(tid) });
  }

  private clearPending(): void {
    if (this.pendingScroll) clearTimeout(this.pendingScroll.timer);
    this.pendingScroll = null;
  }

  /** Sends the frame to the page published at `target` (at `hash`, a fragment
   * with its `#`); `replace` keeps the frame's history entry (the shell URL
   * made or moved it). The outgoing document is done: the gate closes and
   * its page and pins are forgotten until the next page greets. */
  private navigateFrame(target: string, replace: boolean, hash = ""): void {
    const frame = this.frame?.el;
    if (!frame) return;
    this.frameHash = hash;
    this.gate.close();
    this.leaveDocument();
    this.failedParts.clear();
    this.set({ file: null, resolved: {} });
    const url = pageSrc(this.id, this.shown(), this.s.origin ?? null, target) + hash;
    if (replace && frame.contentWindow) {
      try { frame.contentWindow.location.replace(url); return; } catch { /* fall back to src */ }
    }
    frame.src = url;
  }

  /** Moves the frame, showing `page`, to the fragment `hash` in place (a
   * fragment navigation keeps the document, so the gate stays open). No
   * fragment is `#`, since dropping it would reload the page. */
  private moveFragment(page: string, hash: string): void {
    const win = this.frameWin();
    this.frameHash = hash;
    try { win?.location.replace(pageSrc(this.id, this.shown(), this.s.origin ?? null, page) + (hash || "#")); } catch { /* the frame is gone */ }
  }

  /** Shows `target`, an HTML page of this version, as one history entry: the
   * shell URL is pushed and the frame is moved without an entry of its own, so
   * back and forward (through `popstate`) move between pages, also after the
   * shell document was reloaded. When the browser refuses the push, the frame
   * still moves, with an entry of its own (the page's greeting then updates
   * the address bar in place). */
  private openPage(target: string, hash = ""): void {
    const pushed = setUrl(shellPath(this.id, this.pinnedVersion, target, this.shown()) + hash, true);
    this.navigateFrame(target, pushed, hash);
  }

  // ---- lifecycle ----

  start(): void {
    if (this.live || this.disposed) return;
    this.live = true;
    const boot = this.init.boot ?? null;
    // This viewer, once known (the bootstrap, a lookup, or a rename).
    this.offs.push(onViewer(v => this.set({ me: v })));
    if (boot) {
      seedViewer(boot.viewer);
      this.set({ threads: boot.threads });
      this.loaded(boot.artifact, boot.attention ?? null);
    } else {
      getArtifact(this.id).then(d => this.loaded(d), e => this.set({ error: e instanceof ApiError && e.status === 404 ? "Artifact not found" : String(e) }));
    }
    this.decideOrigin(boot);
    this.offs.push(onShieldPress(() => this.showHint(this.s.commenting ? MOVE_TO_PICK : MOVE_TO_CLICK)));
    this.listen();
    // The stream's first `ready` reloads the threads either way.
    if (!boot) this.loadThreads();
    this.openStream();
  }

  private loaded(d: Loaded, attention: Attention | null = d.attention ?? null): void {
    if (this.disposed) return;
    this.latestKnown = Math.max(this.latestKnown, d.artifact.current_version);
    this.set(s => ({ data: d, working: d.artifact.working ?? [], attention, looked: { ...s.looked, ...attention?.looked }, newer: s.newer !== null && s.newer <= d.artifact.current_version ? null : s.newer }));
    this.agentsChanged(d.artifact.participants?.agents ?? []);
    this.viewChanged();
    this.decideChangelog();
    this.writeSeen();
    if (!this.presenceAsked) {
      this.presenceAsked = true;
      this.offs.push(afterPaint(() => this.startPresence()));
    }
  }

  // ---- presence (spec §10, "Presence") ----

  private presenceAsked = false;
  /** The reporter, once its code loaded after the first paint. */
  private reporter: PresenceReporter | null = null;
  private startPresence(): void {
    void import("./presence-reporter").then(m => {
      if (this.disposed) return;
      const r = new m.PresenceReporter(this.id, () => this.s, s => !!s.me && viewReady(s) && !s.deleted, people => this.set({ presence: people }));
      this.reporter = r;
      this.offs.push(() => r.dispose());
      r.fetch();
      r.report(true);
    }, () => {});
  }

  /** The "Share where I'm looking" switch: stored per browser, then reported
   * at once (turning it off withdraws the location without waiting). */
  setShareWhere(on: boolean): void {
    try { localStorage.setItem(SHARE_WHERE_KEY, on ? "1" : "0"); } catch { /* kept for this view only */ }
    this.set({ shareWhere: on });
    this.reporter?.reset();
    this.reporter?.report(true);
  }

  /** The changelog for this load (spec §8): decided once the view is ready,
   * and again when a newer latest version loads; never on the path to first
   * paint. The decision stays frozen until then (decided: Q4). */
  private decideFor = 0;
  private decideChangelog(): void {
    const s = this.s;
    if (!viewReady(s)) return;
    const latest = s.data.artifact.current_version;
    if (this.decideFor === latest) return;
    this.decideFor = latest;
    const pinned = this.pinnedVersion !== null || this.shown(s) !== latest;
    this.set({ rally: rallyOnce(this.id, this.shown(s)) });
    // Its code loads here, off the artifact entry.
    void import("./changelog-model").then(m => {
      if (this.decideFor === latest) this.set({ decided: m.decide(s.data.versions, latest, s.attention, pinned) });
    }, () => {});
  }

  /** Writes the version seen once this viewer is known and views the
   * unpinned latest. Separate from the decision: on a first visit there is
   * no cookie yet, so `me` arrives after the decision, from `getViewer()`. */
  private seenFor = 0;
  private writeSeen(): void {
    const s = this.s;
    if (!viewReady(s) || !s.me || s.deleted) return;
    const latest = s.data.artifact.current_version;
    if (this.pinnedVersion !== null || this.shown(s) !== latest || this.seenFor === latest) return;
    this.seenFor = latest;
    void putSeen(this.id, latest);
  }

  /** The frame mode: this tab's cached probe; else the daemon's guess (from
   * the `clax_frame` cookie, when it served a frame), confirmed or corrected
   * by a probe; else a probe. The decision is remembered in the cookie for
   * the next load's HTML. */
  private decideOrigin(boot: Boot | null): void {
    const o = artifactOrigin(this.id);
    const decided = (origin: string | null) => {
      if (this.disposed) return;
      rememberFrameMode(origin);
      this.set({ origin });
      this.viewChanged();
    };
    if (!o) { decided(null); return; }
    const cached = cachedOriginOk();
    if (cached !== null) { decided(cached ? o : null); return; }
    if (boot?.frame) {
      const guess = boot.frame.mode === "subdomain" ? o : null;
      // An unsandboxed frame shown on the cookie's word alone is heard only
      // once this tab's probe agrees: until then what it posts, and its
      // loads, wait in order, and the gate stays as it was reset.
      if (guess) this.held = [];
      decided(guess);
      void probeOrigin(o).then(ok => {
        const held = this.held ?? [];
        this.held = null;
        if ((ok ? o : null) !== guess) { decided(ok ? o : null); return; }
        for (const e of held) {
          if (this.disposed) return;
          if (e === LOADED) this.frameLoaded();
          else this.onMessage(e);
        }
      });
      return;
    }
    void probeOrigin(o).then(ok => decided(ok ? o : null));
  }

  /** What the page's inline listener kept before the shell listened (see
   * `takeEarly`), in order: a message goes through the same checks as one
   * heard now; a load counts only when it is the adopted frame's. */
  replay(e: Event): void {
    if (this.disposed) return;
    if (e.type === "message") this.hear(e as MessageEvent);
    else if (e.type === "load" && this.frame?.el && e.target === this.frame.el) this.frameLoaded();
  }

  /** A message to the shell: judged now, or kept in order while `held`. */
  private hear(e: MessageEvent): void {
    if (this.held) this.held.push(e);
    else this.onMessage(e);
  }

  /** Shows other props for the same artifact as a re-render of the view with
   * them did: the state is kept, and another version or page is shown
   * through `viewChanged` (for another version: the gate reset, a new
   * capability host, the replaced one disposed, and a new frame). */
  update({ id, pinnedVersion, file = INDEX_FILE }: ArtifactProps): void {
    if (id !== this.id) throw new Error("ArtifactController.update: another artifact needs another controller");
    if (this.disposed) return;
    this.set({ pinnedVersion, startFile: file });
    this.viewChanged();
  }

  /** Stops every listener, timer and stream, and disposes the capability
   * host; the state no longer changes. */
  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    this.live = false;
    for (const off of this.offs.splice(0)) off();
    this.cancelFlush();
    this.stream?.();
    this.stream = null;
    // A deferred publish belongs to this artifact: dropped before the host is
    // disposed, so the dispose's `settled` applies nothing.
    this.deferredPublish = null;
    this.host?.dispose();
    this.host = null;
    this.clearPending();
    clearTimeout(this.lookTimer);
    clearTimeout(this.hintTimer);
    clearTimeout(this.captureTimer);
    if (this.hashFrame) cancelAnimationFrame(this.hashFrame);
    this.hashFrame = 0;
  }

  private listen(): void {
    const onMessage = (e: MessageEvent) => this.hear(e);
    addEventListener("message", onMessage);
    // Whether the pointer is over the content frame (the shell sees a
    // mouseover on the iframe element as it enters, and on another element
    // as it leaves).
    let overFrame = false;
    const onOver = (e: MouseEvent) => { overFrame = e.target === this.frame?.el; };
    // Option widening works with focus in the shell: while comment mode is
    // on and the pointer is over the frame, Option and, with it held, Up and
    // Down are forwarded to the page (not from a text field), and so is
    // Escape, which drops a drag in progress or else answers clax:cancel
    // (ending comment mode). They are the page's keys, not input to the shell
    // (`setForwardedKeys`).
    const forwards = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      const typing = !!t && (t.localName === "input" || t.localName === "textarea" || t.isContentEditable);
      if (!this.s.commenting || !overFrame || typing) return false;
      return e.key === "Alt" || e.key === "Escape" || ((e.key === "ArrowUp" || e.key === "ArrowDown") && e.altKey);
    };
    const onKey = (e: KeyboardEvent) => {
      if (forwards(e)) {
        this.send({ type: "clax:key", key: e.key as "Alt" | "ArrowUp" | "ArrowDown" | "Escape", down: e.type === "keydown" });
        if (e.key === "ArrowUp" || e.key === "ArrowDown") e.preventDefault();
      } else if (e.type === "keydown" && e.key !== "Escape") {
        const a = keyAction(e);
        if (a && this.shortcut(a)) e.preventDefault();
      } else if (e.key === "Escape" && e.type === "keydown") {
        // Escape anywhere else closes the menu, else the sheet, or else ends comment mode here.
        if (this.s.menu) this.set({ menu: null }); else if (this.s.sheet) this.set({ sheet: null }); else this.set({ commenting: false });
      }
    };
    // Who the keys belong to (`keysOwned`).
    // The page pushing focus out of its frame (`parent.focus()`): the window's
    // blur saw the frame active, and focus is now on <body>. A give-back to
    // that frame is then pending until the viewer acts in the shell: a
    // trusted press, focus landing on a shell control, or their own keys in
    // the composer. While it is pending and focus is on <body>, a trusted key
    // is the viewer's typing for the page: it is swallowed (a Tab moves
    // nothing, a Space presses nothing) and focus goes back to the frame at
    // once. A page that keeps the main thread busy after `parent.focus()` so
    // the viewer's keys arrive first gains nothing. With no key, focus goes
    // back a task after the window's focus, once the focus move has settled
    // and only if it is still on <body>. A frame removed or replaced since
    // ends the give-back: with no frame to return to, keys pass. After a load
    // the page caused, a give-back to the new frame starts pending.
    let giveBackTo: HTMLIFrameElement | "any" | null = this.heldAtLoad ? "any" : null;
    const giveBackFrame = (): HTMLIFrameElement | null => {
      const el = this.frame?.el;
      if (!el || !el.isConnected || this.disposed) return null;
      return giveBackTo === "any" || giveBackTo === el ? el : null;
    };
    const handBack = () => {
      if (document.activeElement !== document.body) return;
      const el = giveBackFrame();
      if (el) el.focus(); else giveBackTo = null;
    };
    // The keyboard trail (`keyboardTrail`): focus that enters the shell from
    // the frame or from <body>, with no press of the viewer's, taints it, so
    // consequential actions take only a pointer's click. Only a trusted press
    // that puts focus on the shell control it targets clears it: not a press
    // on the gesture shield's bands or the prompt's backdrop, and not one that
    // leaves focus where it was (a band keeps it). No key clears it. Focus the
    // browser puts back where it was when the window regains focus changes
    // nothing. Every load starts tainted: the shell cannot tell a load the
    // page caused (a top navigation, or `window.open` of the shell's URL)
    // from any other.
    keyboardTrail.taint();
    let pressing = false;
    let restoreTo: Element | null = null;
    const isControl = (t: EventTarget | null): t is Element => t instanceof Element && t !== document.body && t.localName !== "iframe";
    const onPress = (e: PointerEvent) => {
      if (!e.isTrusted) return;
      this.keysOwned = true;
      giveBackTo = null;
      restoreTo = null;
      const target = e.target instanceof Element ? e.target : null;
      const before = document.activeElement;
      const counts = !!target && !target.closest(".frame-shield, .prompt-backdrop");
      // The focus this press gives a control comes in the same task.
      pressing = true;
      setTimeout(() => {
        pressing = false;
        const now = document.activeElement;
        if (counts && now !== before && isControl(now) && now.contains(target)) keyboardTrail.clear();
      }, 0);
    };
    const onFocusIn = (e: FocusEvent) => {
      const t = e.target;
      if (!e.isTrusted) return;
      const restored = t === restoreTo;
      restoreTo = null;
      if (!isControl(t)) return;
      if (!restored && !pressing && (e.relatedTarget === null || (e.relatedTarget instanceof Element && e.relatedTarget.localName === "iframe"))) keyboardTrail.taint();
      if (!t.closest("[role=dialog], [aria-modal=true], .composer")) {
        this.keysOwned = true;
        giveBackTo = null;
      }
    };
    // Only the window's own blur and focus reach these listeners: an element's do not bubble.
    const onBlur = () => {
      this.keysOwned = false;
      const a = document.activeElement;
      const el = this.frame?.el;
      giveBackTo = el && a === el ? el : null;
      restoreTo = isControl(a) ? a : null;
    };
    const onFocus = (e: FocusEvent) => {
      if (!e.isTrusted) return;
      if (giveBackTo) setTimeout(handBack, 0);
      // The browser's restore of focus comes with the window's focus; after
      // it, focus on <body> or the frame leaves nothing to restore.
      setTimeout(() => { if (!isControl(document.activeElement)) restoreTo = null; }, 0);
    };
    const onShellKey = (e: KeyboardEvent) => {
      if (!e.isTrusted) return;
      const a = document.activeElement;
      // The viewer's own keys in the composer: their typing is the shell's.
      if (a instanceof Element && a.closest(".composer")) giveBackTo = null;
      if (!giveBackTo || a !== document.body) return;
      const el = giveBackFrame();
      if (!el) { giveBackTo = null; return; }
      e.preventDefault();
      e.stopImmediatePropagation();
      el.focus();
    };
    addEventListener("pointerdown", onPress, true);
    addEventListener("keydown", onShellKey, true);
    addEventListener("focusin", onFocusIn, true);
    addEventListener("blur", onBlur);
    addEventListener("focus", onFocus);
    const unforward = setForwardedKeys(forwards);
    addEventListener("mouseover", onOver);
    addEventListener("keydown", onKey);
    addEventListener("keyup", onKey);
    // Back and forward move the shell URL between pages; the frame follows when
    // it shows another page.
    const onPop = () => {
      this.clearPending();
      const r = parseShellPath(location.pathname);
      if (r.kind !== "artifact" || r.id !== this.id) return;
      if (r.file !== this.s.file) this.navigateFrame(r.file, true, location.hash);
      else if (location.hash !== this.frameHash) this.moveFragment(r.file, location.hash);
    };
    addEventListener("popstate", onPop);
    const mq = typeof matchMedia === "function" ? matchMedia("(max-width: 480px)") : null;
    const onNarrow = () => this.set({ narrow: !!mq?.matches });
    mq?.addEventListener?.("change", onNarrow);
    this.offs.push(() => {
      unforward();
      removeEventListener("message", onMessage);
      removeEventListener("mouseover", onOver);
      removeEventListener("keydown", onKey);
      removeEventListener("keyup", onKey);
      removeEventListener("pointerdown", onPress, true);
      removeEventListener("keydown", onShellKey, true);
      removeEventListener("focusin", onFocusIn, true);
      removeEventListener("blur", onBlur);
      removeEventListener("focus", onFocus);
      removeEventListener("popstate", onPop);
      mq?.removeEventListener?.("change", onNarrow);
    });
  }

  /** The frame's document said `clax:bye`: it is going away, perhaps to
   * another site's document. Nothing more is sent in sandbox mode, nothing
   * the frame asks is answered, and the page and its pins are forgotten,
   * until the next document greets. A page that posts this itself only cuts
   * itself off, as `clax:cancel` only turns its own comment mode off. */
  private frameLeft(): void {
    this.gate.bye();
    this.leaveDocument();
    this.failedParts.clear();
    this.set({ file: null, resolved: {} });
  }

  /** The frame's document is gone, or is not the page: what serves it live
   * (the room socket, sample streams) ends now, not at the next hello. */
  private leaveDocument(): void {
    this.host?.leave();
  }

  private onMessage(e: MessageEvent): void {
    if (this.disposed) return;
    if (acceptByeFromFrame(e, this.frameWin(), this.s.origin ?? null)) { this.frameLeft(); return; }
    const m = acceptFromFrame(e, this.frameWin(), this.s.origin ?? null);
    if (!m) return;
    switch (m.type) {
      case "clax:hello": {
        // A stale or foreign document in the frame, or one naming a page
        // this version does not hold, gets no welcome, no anchors, and no pins.
        const greeted = typeof m.file === "string" && m.file ? m.file : INDEX_FILE;
        this.gate.hello(helloMatches(m, this.id, this.shown()) && this.holds(greeted));
        this.pendingPick = null;
        this.failedParts.clear();
        this.set({ resolved: {} });
        this.anchorIds.forget();
        if (!this.gate.open) { this.leaveDocument(); this.set({ file: null }); break; }
        this.host?.reset();
        this.set({ file: greeted });
        // The address bar follows the frame to another page. A link the page
        // handed over already pushed its URL; any other navigation (a script,
        // a form) made the frame's own history entry, so the URL is replaced.
        const r = parseShellPath(location.pathname);
        if (r.kind !== "artifact" || r.file !== greeted) setUrl(this.here(this.pinnedVersion) + location.hash);
        this.send({ type: "clax:welcome", mode: this.s.commenting ? "comment" : "view" });
        this.resolveAll();
        this.sendFocus();
        const p = this.pendingScroll;
        if (p) {
          // The jump's page greeted: scroll there. Another page greeted: the
          // viewer moved on, so the jump is dropped without a notice.
          this.clearPending();
          if (p.thread.anchor.file === greeted) this.send({ type: "clax:scroll-to", anchor: p.thread.anchor, sameVersion: p.thread.version_n === this.shown() });
        }
        break;
      }
      case "clax:pick-start": {
        // The viewer's pick itself: taken only in comment mode and while the
        // viewer's latest input went to the frame (`frameGesture`, the
        // composer tier). A refused start shows the viewer the hint only when
        // it can be their own press (`pickHintAllowed`), so a page posting
        // starts cannot show it while the viewer uses the shell. So a page can
        // post a pick of its own only while no pick of the bridge's is pending
        // and within the user-activation window (about five seconds) after
        // the viewer's latest input, once the viewer has clicked or pressed a
        // key in the page, moved the pointer onto or over it (by a real move,
        // not a layout change), turned the wheel or touched it there since
        // their latest input to the shell, or Tabbed into it. The page can
        // move focus into itself, so after a click on the shell's Comment
        // button, or on Cancel or Post in a composer that brings comment mode
        // back, it can forge a pick once the pointer moves, never while it
        // rests where that input left it. The composer then shows the pick's
        // quote or area label and screenshot, not where it anchors, and
        // nothing is posted without the viewer.
        //
        // A taken start opens the composer at once, focused, with its
        // screenshot to come (`clax:pick`), and turns comment mode off. The
        // bridge never has two picks in flight, so a start arriving with the
        // viewer's gesture while comment mode is still off for a taken one
        // whose screenshot is pending means one was forged: both are refused,
        // and the first one's composer closes and comment mode comes back,
        // unless the viewer has typed in it (then it stays, with its pick).
        // Once the viewer turns comment mode back on, a pending pick no longer
        // stands in the way of their next one.
        //
        // Every start refused is answered `clax:pick-refused`, so the bridge
        // renders no clip for it; a taken one's composer, once its textarea
        // has focus, sends `clax:composer-ready`, and only then does the
        // bridge render the clip, so its work never delays that focus.
        if (!this.gate.open || typeof m.pickId !== "string" || m.pickId.length > 64) break;
        const refuse = () => this.send({ type: "clax:pick-refused", pickId: m.pickId });
        const pending = this.pendingPick;
        if (pending && !this.s.commenting && Date.now() - pending.at <= PICK_WAIT_MS) {
          refuse();
          if (!frameGesture()) break;
          if (this.s.draft?.pickId === pending.pickId && this.composerText.trim()) break;
          this.pendingPick = null;
          this.send({ type: "clax:pick-refused", pickId: pending.pickId });
          this.set(s => ({ draft: s.draft?.pickId === pending.pickId ? null : s.draft }));
          if (this.resumeAfter === pending.pickId) {
            this.resumeAfter = null;
            if (!this.s.deleted) this.set({ commenting: true });
          }
          break;
        }
        if (!this.s.commenting) { refuse(); break; }
        this.pendingPick = null;
        if (!frameGesture()) { refuse(); if (pickHintAllowed()) this.showHint(MOVE_TO_PICK); break; }
        if (!m.anchor || typeof m.anchor !== "object" || typeof m.version !== "number") { refuse(); break; }
        const pickId = m.pickId;
        this.pendingPick = { pickId, at: Date.now() };
        this.set({ commenting: false });
        this.resumeAfter = pickId;
        this.set({ draft: { pickId, anchor: m.anchor, version: m.version, clip: null, capturing: true, clipToken: pickId } });
        break;
      }
      case "clax:pick": {
        // The screenshot for the composer the pick's start opened, taken once;
        // a clip the daemon would not keep is dropped here, with the reason
        // shown. The anchor is the start's.
        if (typeof m.pickId !== "string" || this.pendingPick?.pickId !== m.pickId) break;
        this.pendingPick = null;
        const png = m.clipPng instanceof ArrayBuffer && m.clipPng.byteLength > 0 ? m.clipPng : null;
        const tooBig = !!png && png.byteLength > MAX_CLIP_BYTES;
        const clipError = tooBig ? "the screenshot was too large to keep" : typeof m.clipError === "string" ? m.clipError : undefined;
        this.set(s => ({ draft: withClip(s.draft, m.pickId, png && !tooBig ? new Blob([png], { type: "image/png" }) : null, clipError) }));
        break;
      }
      case "clax:anchors": {
        if (this.customLive) break;
        // Mapped back now, through the handles of the page that is greeted
        // at this message: a later greeting forgets them.
        const found: AnchorResult[] = [];
        for (const r of m.results) {
          const tid = this.anchorIds.thread(r.id);
          if (tid) found.push({ ...r, id: tid });
        }
        this.set(s => {
          const next = m.requestId ? {} as Record<string, AnchorResult> : { ...s.resolved };
          for (const r of found) next[r.id] = r;
          return { resolved: next };
        });
        break;
      }
      case "clax:cancel": this.set({ commenting: false }); break;
      case "clax:hash":
        // The page's fragment moved (a link, a script): the address bar
        // follows in place, once per animation frame with the latest
        // fragment; the frame's own history entry carries the move.
        if (!this.gate.open || !validHash(m.hash)) break;
        this.frameHash = m.hash;
        if (!this.hashFrame) {
          this.hashFrame = requestAnimationFrame(() => {
            this.hashFrame = 0;
            if (location.hash !== this.frameHash) setUrl(location.pathname + location.search + this.frameHash);
          });
        }
        break;
      case "clax:navigate": {
        // A link the page handed over, from a document that greeted and is
        // not already leaving (one entry per greeting page). An HTML page of
        // this version is one history entry; another file of the version
        // loads in the frame as a plain link would; anything else is ignored.
        if (!this.gate.open || typeof m.file !== "string" || !this.holds(m.file)) break;
        const hash = validHash(m.hash) ? m.hash : "";
        this.clearPending();
        if (this.isPage(m.file)) this.openPage(m.file, hash);
        else this.navigateFrame(m.file, false, hash);
        break;
      }
      case "clax:degraded": {
        // A lazy part of the bridge did not load in the greeted page (its
        // CSP, say): the viewer is told, and comment mode goes off if it was
        // the comment part. The page could post this itself, so the notice
        // is the shell's own words only, never the message's text.
        if (!this.gate.open || typeof m.part !== "string" || !Object.hasOwn(PART_FAILED, m.part)) break;
        this.failedParts.add(m.part);
        this.showPartFailed(m.part);
        if (m.part === "comment") this.set({ commenting: false });
        break;
      }
      case "clax:hover": break;
      case "clax:use": case "clax:call": if (this.gate.open) void this.host?.handle(m); break;
    }
  }

  private loadThreads(): void {
    const done = this.threadLoad.begin();
    void report(listThreads(this.id), LOAD_FAILED, this.noticeFor(LOAD_FAILED)).then(done);
  }

  private openStream(): void {
    const open = async (resync: boolean) => {
      // The owner shell passes its token (null on a LAN view) so the daemon
      // counts its stream as the owner shell's.
      const token = await getToken();
      if (!this.live) return;
      this.stream?.();
      this.stream = subscribe(this.id, e => this.onEvent(e), token);
      // Events between the old and the new stream are lost: refetch as on a resync.
      if (resync) this.onEvent({ type: "resync", dropped: 0 });
    };
    // The daemon reads the viewer cookie when the stream opens (its level for
    // `doc` events is fixed then), so open it once the lookup has set the
    // cookie, and reopen when a later lookup or a rename changes the viewer.
    const first = () => { if (this.live && !this.stream) void open(false); };
    void getViewer().then(first, first);
    this.offs.push(onViewer(() => { if (this.live && this.stream) void open(true); }));
  }

  private onEvent(e: ArtifactEvent): void {
    if (this.disposed) return;
    this.host?.onEvent(e);
    if (e.type === "version" && e.by_page && e.n > this.shown()) {
      // The page republished itself (artifact.publish): every unpinned view
      // follows at once, on the page it shows (the new version carries every
      // file forward); a pinned view is offered Reload in the top bar. The publishing view
      // reloads itself after its call result is posted; while one of its own
      // publishes is in flight, another view's publish waits for it to settle.
      if (this.ownPublish.active > 0) { this.deferredPublish = Math.max(this.deferredPublish ?? 0, e.n); return; }
      if (this.pinnedVersion === null) {
        this.latestKnown = Math.max(this.latestKnown, e.n);
        holdKeysAcrossLoad();
        nav.assign(this.here(null));
        return;
      }
    }
    if (e.type === "version" && e.n > this.latestKnown) { this.latestKnown = e.n; this.set({ newer: e.n }); }
    if (e.type === "artifact_deleted") this.set({ deleted: true });
    if (e.type === "working") this.set({ working: e.working });
    if (e.type === "presence") this.set({ presence: e.people });
    // An agent may have started or ended: the Send target follows.
    if (e.type === "working" || e.type === "version") this.refreshAgents();
    if (e.type === "thread") this.changeThreads(ts => upsert(ts, e.thread));
    if (e.type === "thread_deleted") { this.changeThreads(ts => ts.filter(t => t.id !== e.thread_id)); this.set(s => ({ selected: s.selected === e.thread_id ? null : s.selected })); }
    if (e.type === "feedback_state") this.changeThreads(ts => ts.map(t => t.id === e.thread_id ? { ...t, feedback_state: { thread_id: e.thread_id, state: e.state, tier: e.tier, since: e.since, resends: e.resends, exhausted: e.exhausted } } : t));
    // A (re)connect may follow a daemon restart that dropped events without a
    // resync; reload like a resync. The first one also covers anything
    // published between the initial load and the stream opening.
    if (e.type === "resync" || e.type === "ready") {
      this.loadThreads();
      this.reporter?.fetch();
      getArtifact(this.id).then(d => {
        const n = d.artifact.current_version;
        this.set(s => ({ working: d.artifact.working ?? [], attention: d.attention ?? s.attention, looked: { ...s.looked, ...d.attention?.looked } }));
        this.agentsChanged(d.artifact.participants?.agents ?? []);
        if (n > this.latestKnown) { this.latestKnown = n; this.set({ newer: n }); }
      }, err => { if (err instanceof ApiError && err.status === 404) this.set({ deleted: true }); });
    }
  }

  /** Takes the artifact's agents as listed now: the Send target stays while
   * it names a live agent, else becomes the default (`defaultTarget`). */
  private agentsChanged(agents: AgentView[]): void {
    this.set(s => ({ agents, sendTo: s.sendTo !== null && agents.some(a => a.live && a.handle === s.sendTo) ? s.sendTo : defaultTarget(this.id, agents) }));
  }

  private agentsFetch = false;
  /** Refetches the artifact's agents; one request at a time. */
  private refreshAgents(): void {
    if (this.agentsFetch || this.disposed) return;
    this.agentsFetch = true;
    getArtifact(this.id).then(d => this.agentsChanged(d.artifact.participants?.agents ?? []), () => {}).finally(() => { this.agentsFetch = false; });
  }

  /** A send to `to` (or without one): remembered as this viewer's target on
   * success. A target that ended since the last refetch (`unknown_agent`)
   * refetches the agents, so the next Send names a live one; the failure
   * shows and the send is never retried without `to`. */
  private sendTracked<T>(p: Promise<T>, to: string | null): Promise<T> {
    return p.then(v => { if (to !== null) rememberTarget(this.id, to); return v; }, (e: unknown) => {
      if (e instanceof ApiError && e.code === "unknown_agent") this.refreshAgents();
      throw e;
    });
  }

  /** Sends `ids` as one batch with the selection bar's note to `sendTo`;
   * on success the selection and note clear. One batch at a time. */
  private async sendIds(ids: string[]): Promise<void> {
    if (!ids.length || this.s.batchBusy) return;
    const to = this.s.sendTo;
    this.set({ batchBusy: true });
    const r = await report(this.whileBusy(this.sendTracked(sendBatch(this.id, ids, this.s.batchNote.trim() || null, to), to)), SEND_FAILED, this.noticeFor(SEND_FAILED));
    if (r) {
      this.changeThreads(ts => r.threads.reduce((all, t) => upsert(all, t), ts));
      this.set({ selection: EMPTY_SELECTION, batchNote: "" });
    }
    this.set({ batchBusy: false });
  }

  private saveThread(p: Promise<Thread>, prefix: string): void {
    void report(this.whileBusy(p), prefix, this.noticeFor(prefix)).then(t => { if (t) this.changeThreads(ts => upsert(ts, t)); });
  }

  // ---- intents ----

  /** The Comment button. */
  toggleComment(): void { this.resumeAfter = null; this.set(s => ({ commenting: !s.commenting })); }
  togglePanel(): void { this.set(s => ({ panel: !s.panel })); }

  closeSheet(): void { this.set({ sheet: null }); }

  /** The sheet's code did not load: close it, so the keys act again, and say so. */
  sheetFailed(): void { this.set({ sheet: null, notice: `${SHEET_FAILED}.` }); }

  /** A shell key (spec §8, "Keys"); `keyAction` decided it applies. Returns
   * whether it acted, so a key that does nothing is left to the browser.
   * Nothing acts while the sheet, a page's prompt or the composer is open,
   * or while the keys are held (`keysOwned`): Escape (handled in `listen`)
   * is their only shell key. */
  shortcut(a: KeyAction): boolean {
    const s = this.s;
    if (!viewReady(s) || s.deleted || s.sheet || s.ask || s.draft || !this.keysOwned) return false;
    if (a === "help") this.set({ sheet: "keys" }); else this.toggleComment();
    return true;
  }
  private readonly pendingLook = new Set<string>();
  private lookTimer: ReturnType<typeof setTimeout> | undefined;
  /** This viewer looked at `t` (spec §10, "Participants and attention";
   * decided: Q4); marks go out at most once a second. */
  look(t: Thread): void {
    const mark = this.s.looked[t.id];
    if (mark && mark >= (t.comments.at(-1)?.created_at ?? "")) return;
    this.pendingLook.add(t.id);
    this.lookTimer ??= setTimeout(() => {
      this.lookTimer = undefined;
      const ids = [...this.pendingLook];
      this.pendingLook.clear();
      void putLooked(this.id, ids).then(m => { if (m && !this.disposed) this.set(s => ({ looked: { ...s.looked, ...m } })); });
    }, 1000);
  }

  /** Opens menu `m` from the top bar, or closes it when open. */
  openMenu(m: "versions" | "people"): void { this.set(s => ({ menu: s.menu === m ? null : m })); }
  closeMenu(): void { this.set({ menu: null }); }

  /** The version menu: the latest is the unpinned URL. */
  chooseVersion(n: number): void { nav.assign(this.here(n === this.latest() ? null : n)); }
  copyLink(): void { navigator.clipboard.writeText(location.origin + this.here(this.pinnedVersion) + location.hash).catch(() => {}); }
  reloadLatest(): void { nav.assign(this.here(null)); }
  dismissNotice(): void { this.set({ notice: null }); }
  setMe(v: Viewer): void { this.set({ me: v }); }
  hover(t: Thread | null): void { this.set({ hovered: t?.id ?? null }); }
  /** A pin's click: the sidebar opens on its thread. */
  openPin(t: Thread): void { this.set({ panel: true }); this.selectThread(t); }

  /** Selects `t` and brings it into view: in this page, or on the page it is on
   * (given up with a notice when that page does not greet in `pageWait.ms`). */
  selectThread(t: Thread): void {
    this.set({ selected: t.id });
    this.look(t);
    this.clearPending();
    if (t.anchor.file === this.s.file) {
      // A custom-anchors page brings its own threads into view; the shell
      // never scrolls it.
      if (!this.host?.reveal(t.id)) this.send({ type: "clax:scroll-to", anchor: t.anchor, sameVersion: t.version_n === this.shown() });
      return;
    }
    // A thread on a page this version does not hold is detached: nothing to open.
    if (!this.holds(t.anchor.file)) return;
    const timer = setTimeout(() => {
      if (this.pendingScroll?.thread !== t) return;
      this.pendingScroll = null;
      this.noticeFor(OPEN_FAILED)(`${OPEN_FAILED} ${t.anchor.file}: the page did not load`);
    }, pageWait.ms);
    this.pendingScroll = { thread: t, timer };
    this.openPage(t.anchor.file);
  }

  sendThread(t: Thread): void { const to = this.s.sendTo; this.saveThread(this.sendTracked(sendToAgent(this.id, t.id, to), to), SEND_FAILED); }
  /** Ticks or unticks `t`; with `shift`, every card between the last one
   * ticked and `t` in `order`, the sidebar's order of its cards (the
   * threads' order without it). */
  toggleSelect(t: Thread, shift: boolean, order: string[] = this.s.threads.map(x => x.id)): void { this.set(s => ({ selection: toggle(s.selection, t.id, shift, order) })); }
  clearSelection(): void { this.set({ selection: EMPTY_SELECTION }); }
  setBatchNote(v: string): void { this.set({ batchNote: v }); }
  /** Sends the ticked threads as one batch. */
  sendSelection(): Promise<void> { return this.sendIds(this.s.selection.ids); }
  /** Sends every open thread not yet sent as one batch. */
  sendUnsent(): Promise<void> { return this.sendIds(unsent(this.s.threads).map(t => t.id)); }
  /** The agent picker: every Send goes to `handle` from now on, remembered for this artifact. */
  chooseTarget(handle: string): void { rememberTarget(this.id, handle); this.set({ sendTo: handle }); }
  resolveThread(t: Thread): void { this.saveThread(resolveThread(this.id, t.id), RESOLVE_FAILED); }
  reply(t: Thread, body: string): void { this.saveThread(addComment(this.id, t.id, body), POST_FAILED); }
  composerInput(text: string): void { this.composerText = text; }
  /** The composer for `pickId` has focus: the bridge may render its clip now. */
  composerFocused(pickId: string): void { if (this.pendingPick?.pickId === pickId) this.send({ type: "clax:composer-ready", pickId }); }
  cancelDraft(): void { this.set({ draft: null }); }

  /** Posts `draft`'s comment (the open composer's); a failure shows in the
   * notice and is rethrown so the composer stays. */
  async submitDraft(body: string, draft: Draft | null = this.s.draft): Promise<void> {
    if (!draft) return;
    try {
      const { thread, clip_error: clipError } = await this.whileBusy(createThread(this.id, { anchor: draft.anchor, body, version: draft.version, clip: draft.clip }));
      this.noticeFor(POST_FAILED)(null);
      this.noticeFor(CLIP_DROPPED)(clipError ? `${CLIP_DROPPED}: ${clipError}` : null);
      this.changeThreads(ts => upsert(ts, thread));
      this.set({ selected: thread.id, draft: null, panel: true });
    } catch (e) {
      void report(Promise.reject(e), POST_FAILED, this.noticeFor(POST_FAILED));
      throw e;
    }
  }
}
