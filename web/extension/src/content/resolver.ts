// Keeps the page's threads on their anchors in the live DOM (spec 2026-10-05
// §11 "Hot reload replaces the DOM"): after the DOM has been quiet for
// QUIET_MS, but at most MAX_WAIT_MS after the first change (a page that never
// goes quiet still re-resolves), in the next animation frame, every open
// thread of the current route (and of the site's other pages) is resolved again; one whose anchor is gone has no box and no number
// (Detached). Scrolls and resizes only measure the elements found last time.
// Mutations of Clax's own overlay hosts (every `clax-overlay` element, by
// default) never start a resolution.
import { OVERLAY_TAG, type Resolved, resolveAnchor, textIndex, type TextIndex } from "../../../bridge/src/anchor";
import { placeArea } from "../../../bridge/src/area";
import { type Box, INDEX_FILE, type ResolveMethod } from "../../../bridge/src/protocol";
import { rectOf } from "../../../bridge/src/target";
import { MAX_FAR, type OverlayThread } from "../messages";

export { MAX_FAR };

export const QUIET_MS = 150;
/** The longest a change waits for its resolution on a page that keeps changing. */
export const MAX_WAIT_MS = 1000;
/** A thread of the current route: its number among the found ones and its
 * box in viewport pixels, both null when its anchor is not in the page. */
export type Placed = { id: string; n: number | null; box: Box | null; method: ResolveMethod | null; from?: string };
/** `now` (default `performance.now()`) stamps `lastMutation`. */
export type Timers = { set(fn: () => void, ms: number): unknown; clear(h: unknown): void; frame(fn: () => void): void; now?(): number };
export const realTimers: Timers = {
  set: (fn, ms) => setTimeout(fn, ms),
  clear: h => clearTimeout(h as ReturnType<typeof setTimeout>),
  frame: fn => { requestAnimationFrame(() => fn()); },
};

/** Whether `n` is a `clax-overlay` element or inside one. */
export const inOverlay = (n: Node): boolean =>
  !!(n.nodeType === Node.ELEMENT_NODE ? (n as Element) : n.parentElement)?.closest(OVERLAY_TAG);

export class Resolver {
  threads: OverlayThread[] = [];
  /** When the last mutation that counts was observed (on `timers.now`). */
  lastMutation = 0;
  private route: string | null = null;
  private found = new Map<string, Resolved>();
  private timer: unknown = null;
  /** When the first change not yet resolved was observed. */
  private burstAt: number | null = null;
  private stopped = false;
  private readonly observer: MutationObserver;

  constructor(
    private readonly doc: Document,
    private readonly onPlaced: (p: Placed[]) => void,
    private readonly timers: Timers = realTimers,
    private readonly ignore: (n: Node) => boolean = inOverlay,
  ) {
    this.observer = new MutationObserver(records => {
      if (records.every(r => this.ignored(r))) return;
      const t = this.timers.now?.() ?? performance.now();
      this.lastMutation = t;
      this.burstAt ??= t;
      this.schedule(Math.max(0, Math.min(QUIET_MS, this.burstAt + MAX_WAIT_MS - t)));
    });
    // The document, not its root element, so a replaced <html> is heard too.
    this.observer.observe(doc, { subtree: true, childList: true, characterData: true, attributes: true });
  }

  /** A record about an ignored node, or one that only added, removed or
   * moved ignored nodes. */
  private ignored(r: MutationRecord): boolean {
    if (this.ignore(r.target)) return true;
    if (r.type !== "childList") return false;
    const nodes = [...r.addedNodes, ...r.removedNodes];
    return nodes.length > 0 && nodes.every(this.ignore);
  }

  /** The tab's threads or route changed: resolve on the next frame. */
  set(threads: OverlayThread[], route: string | null): void {
    this.threads = threads;
    this.route = route;
    this.schedule(0);
  }

  private schedule(ms: number): void {
    if (this.stopped) return;
    if (this.timer !== null) this.timers.clear(this.timer);
    this.timer = this.timers.set(() => {
      this.timer = null;
      this.burstAt = null;
      this.timers.frame(() => { if (!this.stopped) this.run(); });
    }, ms);
  }

  /** The open threads of the current route, then those of the site's other pages (`from`), on any route. */
  private here(): OverlayThread[] {
    const open = this.threads.filter(t => t.status === "open");
    return [...open.filter(t => t.from === undefined && (t.anchor.route ?? null) === this.route), ...open.filter(t => t.from !== undefined).slice(0, MAX_FAR)];
  }

  /** Whether a resolution is due: the DOM changed, or the threads did, since the last one. */
  get busy(): boolean {
    return this.timer !== null;
  }

  /** Resolves every open thread of the current route against the DOM now. */
  run(): Placed[] {
    let idx: TextIndex | undefined;
    const index = () => (idx ??= textIndex(this.doc.body));
    this.found.clear();
    for (const t of this.here()) {
      const r = resolveAnchor(this.doc, t.anchor, new Map(), INDEX_FILE, index, false);
      if (r) this.found.set(t.id, r);
    }
    return this.measure();
  }

  /** The boxes of the threads found last time, measured now. */
  measure(): Placed[] {
    let n = 0;
    const placed = this.here().map((t): Placed => {
      const r = this.found.get(t.id);
      if (!r) return { id: t.id, n: null, box: null, method: null, from: t.from };
      let box: Box;
      if (t.anchor.kind === "area") box = placeArea(t.anchor, r.element);
      else {
        const b = rectOf(r.range ?? r.element);
        box = { x: b.left, y: b.top, w: b.width, h: b.height };
      }
      return { id: t.id, n: ++n, box, method: r.method, from: t.from };
    });
    this.onPlaced(placed);
    return placed;
  }

  /** The element or range found for `threadId` last time, if any. */
  target(threadId: string): Element | Range | null {
    const r = this.found.get(threadId);
    return r ? r.range ?? r.element : null;
  }

  stop(): void {
    this.stopped = true;
    this.observer.disconnect();
    if (this.timer !== null) this.timers.clear(this.timer);
    this.timer = null;
  }
}
