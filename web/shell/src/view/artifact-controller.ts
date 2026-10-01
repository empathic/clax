// The artifact view's state and behaviour, framework-free: what artifact.tsx
// held in hooks. Components render `state` and call the intent methods; the
// frame is a FrameHost the mount gives it.
import { type AnchorResult, INDEX_FILE, type ShellToBridge } from "../../../bridge/src/protocol";
import { ApiError, type Artifact, type Version, getArtifact, getToken } from "../api";
import { acceptFromFrame, helloMatches, sendToFrame } from "../bridge-link";
import type { Declared } from "../caps/availability";
import { HINT_MS, frameGesture, onShieldPress, pickHintAllowed, setForwardedKeys } from "../caps/gesture";
import { CapabilityHost, type CommentsUi } from "../caps/host";
import { type ArtifactEvent, subscribe } from "../events";
import { LOAD_FAILED, OPEN_FAILED, POST_FAILED, RESOLVE_FAILED, SEND_FAILED, report, scopedNotice } from "../failure";
import { nav } from "../nav";
import { artifactOrigin, pageSrc, probeOrigin } from "../origin";
import { parseShellPath, shellPath } from "../route";
import { type Thread, type Viewer, addComment, createThread, currentViewer, getViewer, listThreads, onViewer, resolveThread, sendToAgent, upsert } from "../threads";
import { afterPaint } from "./after-paint";
import { AnchorHandles } from "./anchor-handles";
import { CAPTURE_LATE, type Draft, MAX_CLIP_BYTES, captureWait, nextDraft, withClip } from "./composer-model";
import { FrameGate } from "./frame-gate";
import type { FrameHost } from "./frame-host";
import { type Ask, promptQueue } from "./prompt-queue";
import { Store } from "./store";
import { type ThreadChange, ThreadSync } from "./thread-sync";
import { setUrl, validHash } from "./url";
import type { SetNotice } from "./viewer-name-model";

/** `file` is the page the frame opens on, from the shell URL (`index.html` when it names none). */
export type ArtifactProps = { id: string; pinnedVersion: number | null; file?: string };
export type Loaded = { artifact: Artifact; versions: Version[] };
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
};

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

const media = (q: string) => typeof matchMedia === "function" && matchMedia(q).matches;
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
  // without a reload: a reload to the latest, or the banner when pinned.
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
  private readonly prompt = promptQueue(ask => this.set({ ask }));

  constructor({ id, pinnedVersion, file: startFile = INDEX_FILE }: ArtifactProps) {
    this.id = id;
    this.startHash = location.hash;
    this.frameHash = this.startHash;
    this.state = new Store<ViewState>({
      pinnedVersion, startFile,
      data: null, error: null, origin: undefined, newer: null, deleted: false, commenting: false,
      panel: media("(min-width: 900px)"), narrow: media("(max-width: 480px)"),
      threads: [], resolved: {}, draft: null, selected: null, hovered: null, busy: 0,
      notice: null, hint: null, me: null, ask: null, file: startFile,
    });
    this.threadLoad = new ThreadSync(f => this.set(s => ({ threads: f(s.threads) })));
    this.ownPublish.settled = () => {
      const n = this.deferredPublish;
      this.deferredPublish = null;
      if (n === null) return;
      if (this.pinnedVersion === null) nav.assign(this.here(null));
      else if (n > this.latestKnown) { this.latestKnown = n; this.set({ newer: n }); }
    };
    // The comment UI as the `comments` capability drives it; it reads the
    // controller's current state, so the host always sees the latest.
    this.commentsUi = {
      openComposer: (d, opts) => {
        const next = nextDraft(this.s.draft, this.composerText, d, opts);
        if (!next) return false;
        this.set({ draft: next });
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

  // ---- state and reactions ----

  private set(patch: Partial<ViewState> | ((s: ViewState) => Partial<ViewState>)): void {
    if (this.disposed) return;
    const prev = this.s;
    this.state.set(patch);
    if (this.turnFrom === null && this.s !== prev) {
      this.turnFrom = prev;
      queueMicrotask(() => this.rendered());
    }
  }

  /** The end of a turn's changes, where Preact rendered them. As Preact does
   * with a component's effects, the reactions to the previous render still
   * pending run first, and this render's run after the next paint: with the
   * pass already scheduled, if any (Preact schedules one when its queue of
   * effects stops being empty, and keeps it until it ran). */
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

  /** What Preact ran as effects after the render that went from `prev` to
   * `next`: values the effects took from that render come from `next`, those
   * they read through refs from the current state, and `host` is the one
   * that render made or kept. */
  private react(prev: ViewState, next: ViewState, host: CapabilityHost | null): void {
    if (threadIds(prev.threads) !== threadIds(next.threads) || this.shown(prev) !== this.shown(next) || prev.origin !== next.origin) this.resolveAll();
    if (prev.commenting !== next.commenting) this.send({ type: "clax:comment-mode", on: next.commenting });
    if (prev.draft !== next.draft) this.draftChanged(next.draft, next.deleted);
    if (prev.hovered !== next.hovered || prev.selected !== next.selected || prev.threads !== next.threads) this.sendFocus();
    if (prev.draft?.clipToken !== next.draft?.clipToken || prev.draft?.capturing !== next.draft?.capturing) this.armCapture(next.draft);
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

  // A screenshot still being taken that never arrives: the composer says so
  // (Post stays disabled until then).
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

  private send(m: ShellToBridge): void {
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
    }
    // Made before the frame it serves is inserted: a hello and a capability
    // request can arrive as soon as it is.
    if (this.hostFor?.key !== key || this.hostFor.data !== s.data) {
      this.hostFor = { key, data: s.data };
      // A replaced host (another artifact, version, or view) and the host at
      // dispose are disposed, so their timers and late results never reach a frame.
      this.host?.dispose();
      const data = s.data;
      this.host = new CapabilityHost(getToken().then(token => ({
        aid: this.id,
        version: shown,
        pinned: this.pinnedVersion !== null,
        token,
        viewer: currentViewer,
        declared: (data.artifact.capabilities ?? {}) as Declared,
        prompt: this.prompt,
        post: m => { if (this.gate.open) this.send(m); },
        reload: () => nav.assign(this.here(null)),
        ownPublish: this.ownPublish,
        page: () => this.s.file,
        comments: this.commentsUi,
        files: data.versions.find(v => v.n === shown)?.files,
      })));
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
    if (this.gate.load()) this.set({ file: null, resolved: {} });
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
    getArtifact(this.id).then(d => {
      if (this.disposed) return;
      this.latestKnown = Math.max(this.latestKnown, d.artifact.current_version);
      this.set(s => ({ data: d, newer: s.newer !== null && s.newer <= d.artifact.current_version ? null : s.newer }));
      this.viewChanged();
    }, e => this.set({ error: e instanceof ApiError && e.status === 404 ? "Artifact not found" : String(e) }));
    const decided = (origin: string | null) => {
      if (this.disposed) return;
      this.set({ origin });
      this.viewChanged();
    };
    const o = artifactOrigin(this.id);
    if (!o) decided(null);
    else void probeOrigin(o).then(ok => decided(ok ? o : null));
    this.offs.push(onShieldPress(() => this.showHint(this.s.commenting ? MOVE_TO_PICK : MOVE_TO_CLICK)));
    this.listen();
    this.loadThreads();
    this.openStream();
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
    this.host?.dispose();
    this.host = null;
    this.clearPending();
    clearTimeout(this.hintTimer);
    clearTimeout(this.captureTimer);
    if (this.hashFrame) cancelAnimationFrame(this.hashFrame);
    this.hashFrame = 0;
  }

  private listen(): void {
    const onMessage = (e: MessageEvent) => this.onMessage(e);
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
      } else if (e.key === "Escape" && e.type === "keydown") {
        // Escape anywhere else ends comment mode here.
        this.set({ commenting: false });
      }
    };
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
      removeEventListener("popstate", onPop);
      mq?.removeEventListener?.("change", onNarrow);
    });
  }

  private onMessage(e: MessageEvent): void {
    if (this.disposed) return;
    const m = acceptFromFrame(e, this.frameWin(), this.s.origin ?? null);
    if (!m) return;
    switch (m.type) {
      case "clax:hello": {
        // A stale or foreign document in the frame, or one naming a page
        // this version does not hold, gets no welcome, no anchors, and no pins.
        const greeted = typeof m.file === "string" && m.file ? m.file : INDEX_FILE;
        this.gate.hello(helloMatches(m, this.id, this.shown()) && this.holds(greeted));
        this.pendingPick = null;
        this.set({ resolved: {} });
        this.anchorIds.forget();
        if (!this.gate.open) { this.set({ file: null }); break; }
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
      // file forward); a pinned view gets the banner. The publishing view
      // reloads itself after its call result is posted; while one of its own
      // publishes is in flight, another view's publish waits for it to settle.
      if (this.ownPublish.active > 0) { this.deferredPublish = Math.max(this.deferredPublish ?? 0, e.n); return; }
      if (this.pinnedVersion === null) {
        this.latestKnown = Math.max(this.latestKnown, e.n);
        nav.assign(this.here(null));
        return;
      }
    }
    if (e.type === "version" && e.n > this.latestKnown) { this.latestKnown = e.n; this.set({ newer: e.n }); }
    if (e.type === "artifact_deleted") this.set({ deleted: true });
    if (e.type === "thread") this.changeThreads(ts => upsert(ts, e.thread));
    if (e.type === "thread_deleted") { this.changeThreads(ts => ts.filter(t => t.id !== e.thread_id)); this.set(s => ({ selected: s.selected === e.thread_id ? null : s.selected })); }
    if (e.type === "feedback_state") this.changeThreads(ts => ts.map(t => t.id === e.thread_id ? { ...t, feedback_state: { thread_id: e.thread_id, state: e.state, tier: e.tier, since: e.since, resends: e.resends, exhausted: e.exhausted } } : t));
    // A (re)connect may follow a daemon restart that dropped events without a
    // resync; reload like a resync. The first one also covers anything
    // published between the initial load and the stream opening.
    if (e.type === "resync" || e.type === "ready") {
      this.loadThreads();
      getArtifact(this.id).then(d => {
        const n = d.artifact.current_version;
        if (n > this.latestKnown) { this.latestKnown = n; this.set({ newer: n }); }
      }, err => { if (err instanceof ApiError && err.status === 404) this.set({ deleted: true }); });
    }
  }

  private saveThread(p: Promise<Thread>, prefix: string): void {
    void report(this.whileBusy(p), prefix, this.noticeFor(prefix)).then(t => { if (t) this.changeThreads(ts => upsert(ts, t)); });
  }

  // ---- intents ----

  /** The Comment button. */
  toggleComment(): void { this.resumeAfter = null; this.set(s => ({ commenting: !s.commenting })); }
  togglePanel(): void { this.set(s => ({ panel: !s.panel })); }
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

  sendThread(t: Thread): void { this.saveThread(sendToAgent(this.id, t.id), SEND_FAILED); }
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
