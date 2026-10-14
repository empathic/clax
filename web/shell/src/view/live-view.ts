// The live layers of one artifact view (spec 2026-10-11-clax-dev-hosted-design
// §16.3): the controller's go-between for the live socket, the content
// frame and the view's state. Loaded after the first paint (as presence is),
// so none of it is on the artifact entry.
//
// Out to the socket: the pointer and selection the frame reports
// (`clax:cursor`), or, while a pick's composer is open, the pick itself as
// the bridge described it; the thread the viewer has open; typing and
// drafts (`DraftSender`). In from the socket: other people's state, kept in
// `ViewState.live`. To the frame: the peers drawn there (`clax:peers`), with
// keys that start over for each document and colours, never names or IDs;
// from the frame: where it drew their pointers, which become the name tags
// the stage draws over it.

import { type BridgeToShell, type FramePeer, LIVE_CURSOR_MS, type LivePoint, type LiveSelection, type ShellToBridge } from "../../../bridge/src/protocol";
import { after, wall } from "../clock";
import type { ViewState } from "./artifact-controller";
import { PeerTags } from "./peer-tags";
import { type LayerState, DraftSender, FrameKeys, LiveSocket, type PeerTag, type SocketLike, anyTyping, applyBatch, emptyLayer, framePeers, liveUrl, peerTags } from "./live-layers";

/** The live layers as the view shows them. */
export type LiveUi = {
  /** Other people's state. */
  layer: LayerState;
  /** Name tags for the pointers the frame drew, in its viewport's pixels. */
  tags: PeerTag[];
  /** The clock "is typing…" is judged by (ms); it moves while anyone types. */
  now: number;
  /** This viewer's own live colour, once the socket said. */
  color: number | null;
};
export const emptyLive = (): LiveUi => ({ layer: emptyLayer(), tags: [], now: wall(), color: null });
/** The view's live layers: none until the live view's first change. */
const ui = (s: ViewState): LiveUi => s.live ?? emptyLive();

/** What the live view needs of the controller: its artifact, the version
 * a state shows and its state changes (`ArtifactController`'s own), its
 * state, and a post to the frame's greeted document (the gate decides). */
export type LiveHost = {
  readonly id: string;
  shown(s: ViewState): number;
  set(f: (s: ViewState) => Partial<ViewState>): void;
};
type Host = LiveHost & { state(): ViewState; toFrame(m: ShellToBridge): void };

const MAX_TAGS = 30;
type Frame = { anchor: LivePoint | null; selection: LiveSelection | null; pick: LiveSelection | null };
const NO_FRAME: Frame = { anchor: null, selection: null, pick: null };
const shape = <T>(v: T | null | undefined): T | null => (v && typeof v === "object" ? v : null);
/** How often `live.now` moves while anyone types, in ms. */
const TICK_MS = 1000;

export class LiveView {
  private readonly keys = new FrameKeys();
  private readonly drafts: DraftSender;
  private readonly socket: LiveSocket;
  /** The frame's latest pointer, selection and comment-mode pick. */
  private frame: Frame = NO_FRAME;
  private sentCursor = "";
  private sentPeers = "";
  private sentOpen: string | null | undefined = undefined;
  private at: { key: string; x: number; y: number }[] = [];
  /** Cancels the pending tick of `live.now`, while anyone types. */
  private tick: (() => void) | null = null;
  /** The name tags over the stage. */
  private readonly tagBox: PeerTags;
  /** Cancels the wait before the next cursor may go, and whether one is due then. */
  private cursorWait: (() => void) | null = null;
  private cursorDue = false;

  private readonly host: Host;

  constructor(ctl: LiveHost, state: () => ViewState, toFrame: (m: ShellToBridge) => void, make?: (url: string) => SocketLike) {
    const host: Host = this.host = { id: ctl.id, shown: s => ctl.shown(s), set: f => ctl.set(f), state, toFrame };
    this.socket = new LiveSocket(liveUrl(host.id), {
      welcome: w => host.set(s => ({ live: { ...ui(s), color: w.color } })),
      batch: b => {
        host.set(s => ({ live: { ...ui(s), layer: applyBatch(ui(s).layer, b, wall()), now: wall() } }));
        this.afterChange();
      },
      opened: () => {
        // A reopened socket's daemon holds none of this page's state.
        this.sentCursor = "";
        this.sentOpen = undefined;
        this.drafts.resend();
        this.sendCursor();
        this.sendOpen();
      },
      // A close for want of a seat (4429) reopens too: the stream's answer
      // alone makes the view a spectator, and pauses the socket.
      closed: () => this.clear(),
    }, make);
    this.drafts = new DraftSender(m => this.socket.send(m));
    // The stage the frame is in (artifact.html's `.stage`), where the name tags go over it.
    this.tagBox = new PeerTags(() => document.querySelector<HTMLElement>(".stage"));
  }

  start(): void {
    this.socket.start();
  }

  /** A spectator: the socket closes, and others' layers go, until `resume`. */
  pause(): void {
    this.socket.pause();
    this.clear();
  }

  /** Seated again: the socket opens. */
  resume(): void {
    this.socket.resume();
  }

  /** Their pointers and drafts are stale once the socket is gone. */
  private clear(): void {
    this.host.set(s => ({ live: { ...ui(s), layer: emptyLayer(), tags: [] } }));
    this.afterChange();
  }


  dispose(): void {
    this.drafts.stop();
    this.socket.stop();
    this.tick?.();
    this.tick = null;
    this.cursorWait?.();
    this.cursorWait = null;
    this.tagBox.remove();
  }

  /** The box of thread `threadId` (null: a new comment) holds `text` now. */
  text(threadId: string | null, text: string): void {
    this.drafts.input(threadId, text);
  }

  /** A live-layer message from the greeted page: its pointer and selection
   * (`clax:cursor`; the daemon checks the shapes it relays) or where it drew
   * the peers' pointers (`clax:peer-at`). */
  fromFrame(m: Extract<BridgeToShell, { type: "clax:cursor" | "clax:peer-at" }>): void {
    if (m.type === "clax:cursor") {
      this.frameCursor(shape(m.anchor), shape(m.selection), shape(m.pick));
    } else if (Array.isArray(m.peers)) {
      this.peerAt(m.peers.filter(p => p && typeof p.key === "string" && Number.isFinite(p.x) && Number.isFinite(p.y)));
    }
  }

  /** The frame reported its pointer, selection and last comment-mode pick. */
  frameCursor(anchor: LivePoint | null, selection: LiveSelection | null, pick: LiveSelection | null = null): void {
    this.frame = { anchor, selection, pick };
    this.sendCursor();
  }

  /** The frame said where it drew the peers' pointers. */
  peerAt(at: { key: string; x: number; y: number }[]): void {
    this.at = at.slice(0, MAX_TAGS);
    this.retag();
  }

  /** A new document greeted: its keys start over and it gets the peers. */
  hello(): void {
    this.keys.reset();
    this.at = [];
    this.sentPeers = "";
    this.frame = NO_FRAME;
    this.postPeers();
    this.retag();
  }

  /** The frame's document went (or did not greet): nothing is drawn there now. */
  left(): void {
    this.at = [];
    this.frame = NO_FRAME;
    this.sendCursor();
    this.retag();
  }

  /** The view changed from `prev` to `next`. */
  react(prev: ViewState, next: ViewState): void {
    if (prev.draft !== next.draft || prev.file !== next.file || this.host.shown(prev) !== this.host.shown(next)) this.sendCursor();
    if (prev.selected !== next.selected) this.sendOpen();
    if (prev.live?.layer !== next.live?.layer || prev.file !== next.file || this.host.shown(prev) !== this.host.shown(next)) this.postPeers();
    if (prev.presence !== next.presence || prev.data !== next.data) this.retag();
  }

  private afterChange(): void {
    this.postPeers();
    this.retag();
    // "is typing…" goes stale on its own: the clock moves while anyone types.
    if (this.tick || !anyTyping(ui(this.host.state()).layer, wall())) return;
    const step = () => {
      const now = wall();
      this.host.set(s => ({ live: { ...ui(s), now } }));
      this.tick = anyTyping(ui(this.host.state()).layer, now) ? after(TICK_MS, step) : null;
    };
    this.tick = after(TICK_MS, step);
  }

  /** The cursor as others should see it: while the composer of the
   * viewer's own pick is open, that pick (the frame's last), else the
   * frame's selection. At most one every
   * LIVE_CURSOR_MS, the latest always, however often the frame reports
   * (the page's own scripts can post `clax:cursor` too), so this page stays
   * within the socket's rate. */
  private sendCursor(): void {
    if (this.cursorWait) { this.cursorDue = true; return; }
    this.cursorNow();
    this.cursorWait = after(LIVE_CURSOR_MS, () => {
      this.cursorWait = null;
      if (this.cursorDue) { this.cursorDue = false; this.sendCursor(); }
    });
  }

  private cursorNow(): void {
    const s = this.host.state();
    const file = s.file;
    const version = this.host.shown(s);
    if (!file || version < 1) return;
    const selection = s.draft && !s.draft.byPage ? this.frame.pick : this.frame.selection;
    const m = { t: "cursor" as const, version, file, anchor: this.frame.anchor, selection };
    const k = JSON.stringify(m);
    if (k === this.sentCursor) return;
    this.sentCursor = k;
    this.socket.send(m);
  }

  private sendOpen(): void {
    const t = this.host.state().selected;
    if (t === this.sentOpen) return;
    this.sentOpen = t;
    this.socket.send({ t: "open", thread_id: t });
  }

  private postPeers(): void {
    const s = this.host.state();
    const peers: FramePeer[] = framePeers(ui(s).layer, this.host.shown(s), s.file, this.keys);
    // Whether anyone else is here, so the page reports this viewer's pointer.
    const here = ui(s).layer.present > 0;
    const k = JSON.stringify(peers) + here;
    if (k === this.sentPeers) return;
    this.sentPeers = k;
    this.host.toFrame({ type: "clax:peers", peers, here });
  }

  /** The name the view knows a person by: their presence, else the artifact's people. */
  private name(pid: string): string {
    const s = this.host.state();
    const p = s.presence.find(x => x.public_id === pid)?.display_name
      ?? s.data?.artifact.participants?.people.find(x => x.public_id === pid)?.display_name;
    return p?.trim() || "Viewer";
  }

  private retag(): void {
    const s = this.host.state();
    const tags = s.file ? peerTags(ui(s).layer, this.at, this.keys, pid => this.name(pid)) : [];
    if (JSON.stringify(tags) === JSON.stringify(ui(s).tags)) return;
    this.host.set(st => ({ live: { ...ui(st), tags } }));
    this.showTags(tags);
  }

  /** Draws `tags` over the frame. */
  private showTags(tags: PeerTag[]): void {
    this.tagBox.show(tags);
  }

  /** The name of person `pid`, for the sidebar's writers. */
  nameOf(pid: string): string {
    return this.name(pid);
  }
}
