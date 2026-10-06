// One pick per tab, from the screenshot to the posted thread (spec
// 2026-10-05 §3.2, §8.1, §9.4). The overlay names the pick (128 random
// bits) and its anchor in `capture`; the worker takes it as the tab's pick,
// captures, and tells the overlay to open the composer. A composer port is
// taken only from the pick's tab, for the tab's current pick, within
// COMPOSER_WAIT_MS of its capture, and only one; once it is, the overlay is
// told (`composer-ready`) and only then shows the frame, so a document the
// page put in the frame is never shown or focused. A pick whose composer
// does not connect in time is cancelled. The draft goes out as soon as the
// composer asks; the thread is posted once both the body (from the
// composer) and the snapshot (from the overlay) are in, naming the threads
// that were pending when the overlay was told to serialize the page (spec
// L11). The composer never sees the credential: posting is the worker's.
// Every attempt to post a pick names it (`pick_id`), so the daemon makes one
// thread of a post retried after its answer was lost. A post fails, saying
// why, rather than wait: for a page whose address is over MAX_URL, or whose
// snapshot has not come SNAPSHOT_WAIT_MS after the person pressed Post.
import type { Anchor } from "../../../bridge/src/protocol";
import { bytesDataUrl } from "../data-url";
import { type ComposerNote, type OverlayToWorker, type PageView, type Rect, type SnapshotError, URL_TOO_LONG, type WorkerToComposer, type WorkerToOverlay, isFromComposer } from "../messages";
import type { Api } from "./api";

/** How long a captured pick waits for its composer page to connect. */
export const COMPOSER_WAIT_MS = 5000;
/** How long a pressed Post waits for the page's snapshot before it fails. */
export const SNAPSHOT_WAIT_MS = 30_000;
export const NO_SNAPSHOT = "The page did not send its snapshot. Post again.";

/** The snapshot posted for a pick that came without one, saying why. */
export function noSnapshotPage(error: SnapshotError | null): string {
  const why = error === "failed" ? "the page could not be read" : "the page was over Clax's snapshot limits";
  return `<!doctype html><meta charset="utf-8"><title>No snapshot</title><p>Clax took no snapshot of this page: ${why}.</p>`;
}

type Capture = Extract<OverlayToWorker, { t: "capture" }>;
type PickMsg = Extract<OverlayToWorker, { t: "pick" }>;
type Quiet = Extract<OverlayToWorker, { t: "quiet" }>;
type Captured = Extract<WorkerToOverlay, { t: "captured" }>;

type PickState = {
  pickId: string; tabId: number;
  /** When the overlay was told to open the composer: its page's wait starts then. */
  opened: number; anchor: Anchor; clip: Blob | null; clipError: string | null;
  /** The tab's pending threads when the overlay was told to serialize the page. */
  pending: string[];
  url: string | null; title: string; snapshot: string | null;
  /** The page's address is over MAX_URL: the daemon would refuse the post. */
  tooLong: boolean;
  /** Counts the person's presses of Post, so a wait times out only the press it began with. */
  posts: number;
  body: string | null; port: chrome.runtime.Port | null; connected: boolean; posting: boolean;
};

export type PicksDeps = {
  api: Pick<Api, "postThread" | "postSnapshot">;
  capture(windowId: number, rect: Rect, dpr: number): Promise<{ png: Blob } | { error: string }>;
  toOverlay(tabId: number, m: WorkerToOverlay): void;
  /** The tab's open threads waiting for a snapshot, as the worker knows them now. */
  pendingIds(tabId: number): string[];
  /** A thread was posted on `page` from the tab. */
  posted?(tabId: number, page: PageView): void;
  /** Whether the tab is still its window's active tab (what `captureVisibleTab` captures). */
  tabActive?(tabId: number): Promise<boolean>;
  now(): number;
  /** Runs `fn` after `ms`. */
  after?(ms: number, fn: () => void): void;
};

/** A `data:image/png` URL of `b` (a service worker cannot make object URLs). */
const dataUrl = async (b: Blob) => bytesDataUrl(new Uint8Array(await b.arrayBuffer()), "image/png");

const message = (e: unknown) => (e instanceof Error && e.message ? e.message : String(e));
/** How the composer words a failed post: the side panel's words for a failure to pair, else the failure's own. */
const WORDS: Record<string, string> = {
  host_missing: "Clax is not set up for Chrome yet. Run clax init (or /clax:extension in Claude Code), then post again.",
  host_failed: "Clax could not reach its helper. Post again; if it fails again, run clax init.",
  daemon_unavailable: "Clax could not start. See ~/.clax/logs/daemon.log, then post again.",
  daemon_unreachable: "Clax is not answering. Post again in a moment.",
};
export function postFailure(e: unknown): string {
  const code = (e as { code?: unknown } | null)?.code;
  return (typeof code === "string" && Object.hasOwn(WORDS, code) ? WORDS[code] : undefined) ?? message(e);
}

/** Tells the composer `m`; a port Chrome already closed (its frame went) hears nothing. */
function tell(port: chrome.runtime.Port | null, m: WorkerToComposer): void {
  try { port?.postMessage(m); } catch { /* the composer is gone */ }
}
function cut(port: chrome.runtime.Port | null): void {
  try { port?.disconnect(); } catch { /* already gone */ }
}

export class Picks {
  private byTab = new Map<number, PickState>();
  /** Tabs with a quiet snapshot in flight. */
  private quieting = new Set<number>();
  private readonly after: (ms: number, fn: () => void) => void;
  /** What runs once no pick is open (`whenIdle`). */
  private idlers: (() => void)[] = [];

  constructor(private readonly d: PicksDeps) {
    this.after = d.after ?? ((ms, fn) => { setTimeout(fn, ms); });
  }

  /** Runs `fn` once no tab has a pick open (at once when none has): the
   * extension's reload for a new daemon version (spec L13) waits for it, so
   * a composer and its text are not torn down mid-draft. */
  whenIdle(fn: () => void): void {
    this.idlers.push(fn);
    this.drain();
  }
  private drain(): void {
    if (this.byTab.size) return;
    for (const fn of this.idlers.splice(0)) fn();
  }
  /** The tab's pick goes. */
  private drop(tabId: number): void {
    this.byTab.delete(tabId);
    this.drain();
  }

  /** The tab's pick `pickId`, while it is current. */
  private current(tabId: number, pickId: string): PickState | null {
    const p = this.byTab.get(tabId);
    return p && p.pickId === pickId ? p : null;
  }

  /** Takes the overlay's pick for the tab, replacing any earlier one, and
   * captures its screenshot (nothing when the tab is no longer its window's
   * active tab); then tells the overlay to open its composer. Answers
   * `captured`, or null for a pick ID the tab already has. */
  async capture(tabId: number, windowId: number, m: Capture): Promise<Captured | null> {
    const prev = this.byTab.get(tabId);
    if (prev?.pickId === m.pickId) return null;
    cut(prev?.port ?? null);
    const p: PickState = {
      pickId: m.pickId, tabId, opened: Number.POSITIVE_INFINITY, anchor: m.anchor, clip: null, clipError: null, pending: [],
      url: null, title: "", snapshot: null, tooLong: false, posts: 0, body: null, port: null, connected: false, posting: false,
    };
    this.byTab.set(tabId, p);
    const active = this.d.tabActive ? await this.d.tabActive(tabId).catch(() => false) : true;
    const shot = active ? await this.d.capture(windowId, m.rect, m.dpr) : { error: "capture_failed" };
    if (this.byTab.get(tabId) !== p) return null;
    if ("png" in shot) p.clip = shot.png;
    else p.clipError = shot.error;
    // The overlay serializes the page once its composer is shown: what is pending now is what that snapshot covers.
    p.pending = this.d.pendingIds(tabId);
    p.opened = this.d.now();
    this.d.toOverlay(tabId, { t: "open-composer", pickId: p.pickId, rect: m.rect });
    this.after(COMPOSER_WAIT_MS, () => { if (this.byTab.get(tabId) === p && !p.connected) this.cancel(tabId, p.pickId, "timeout"); });
    return "png" in shot ? { t: "captured", pickId: p.pickId, ok: true } : { t: "captured", pickId: p.pickId, ok: false, error: shot.error };
  }

  /** The overlay's snapshot for the tab's pick (taken once). */
  async attach(tabId: number, m: PickMsg): Promise<void> {
    const p = this.current(tabId, m.pickId);
    if (!p || p.snapshot !== null) return;
    Object.assign(p, { url: m.url, tooLong: m.url === null, title: m.title, snapshot: m.snapshot ?? noSnapshotPage(m.snapshotError) });
    await this.maybePost(p);
  }

  /** A composer page's port (named `composer:<pickId>`) from tab `tabId`. */
  attachComposer(port: chrome.runtime.Port, tabId: number): void {
    const pickId = port.name.startsWith("composer:") ? port.name.slice("composer:".length) : "";
    const p = this.current(tabId, pickId);
    if (!p || p.connected || !(this.d.now() - p.opened < COMPOSER_WAIT_MS)) { cut(port); return; }
    p.port = port;
    p.connected = true;
    port.onMessage.addListener((m: unknown) => {
      if (!isFromComposer(m) || this.byTab.get(tabId) !== p) return;
      if (m.t === "ready") void this.sendDraft(p);
      else if (m.t === "post") {
        if (p.posting) return;
        p.body = m.body;
        const press = ++p.posts;
        void this.maybePost(p);
        this.after(SNAPSHOT_WAIT_MS, () => {
          if (this.byTab.get(tabId) !== p || p.posts !== press || p.posting || p.snapshot !== null || p.body === null) return;
          p.body = null;
          tell(p.port, { t: "failed", message: NO_SNAPSHOT });
        });
      } else this.cancel(tabId, pickId);
    });
    port.onDisconnect.addListener(() => {
      if (p.port !== port) return;
      p.port = null;
      // The composer went away (its frame was removed or navigated): a post in flight still lands.
      if (!p.posting && this.byTab.get(tabId) === p) this.cancel(tabId, pickId);
    });
    this.d.toOverlay(tabId, { t: "composer-ready", pickId });
  }

  /** Cancels the tab's pick (`pickId`, or whichever when null) and closes
   * its composer; `reason` tells the overlay why, when it is not the person's doing. */
  cancel(tabId: number, pickId: string | null, reason?: "timeout"): void {
    const p = this.byTab.get(tabId);
    if (!p || (pickId !== null && p.pickId !== pickId)) return;
    this.drop(tabId);
    const port = p.port;
    p.port = null;
    cut(port);
    this.d.toOverlay(tabId, reason ? { t: "close-composer", pickId: p.pickId, posted: false, reason } : { t: "close-composer", pickId: p.pickId, posted: false });
  }

  /** The tab closed: its pick goes. */
  close(tabId: number): void {
    const p = this.byTab.get(tabId);
    this.drop(tabId);
    if (p?.port) { const port = p.port; p.port = null; cut(port); }
  }

  /** A composer frame of the tab whose port is gone (`ComposerNote`). The
   * worker's own pick has its own flow (its port's end, `close-composer`);
   * a pick the worker does not hold (it was stopped and started again) is
   * the overlay's to let go: `lost` gives comment mode back, with the
   * composer and its text still shown, and `dismiss` closes the frame. */
  note(tabId: number, m: ComposerNote): void {
    const p = this.byTab.get(tabId);
    if (p?.pickId === m.pickId) {
      if (m.t === "dismiss") this.cancel(tabId, m.pickId);
      return;
    }
    this.d.toOverlay(tabId, m.t === "lost" ? { t: "pick-lost", pickId: m.pickId } : { t: "close-composer", pickId: m.pickId, posted: false });
  }

  /** An automatic snapshot (spec L11) for the threads the overlay saw
   * pending when it serialized the page that are pending still. Nothing is
   * posted when none is, and the daemon's `nothing_pending` is not a failure:
   * the next quiet snapshot tries again. One per tab at a time. */
  async quiet(tabId: number, m: Quiet): Promise<void> {
    const now = new Set(this.d.pendingIds(tabId));
    const pending = m.pending.filter(id => now.has(id));
    if (pending.length === 0 || this.quieting.has(tabId)) return;
    this.quieting.add(tabId);
    const f = new FormData();
    f.set("url", m.url);
    f.set("title", m.title);
    f.set("snapshot", new Blob([m.snapshot], { type: "text/html" }), "index.html");
    try {
      await this.d.api.postSnapshot(f, pending);
    } catch {
      // A 409 or a failure: the next quiet snapshot tries again.
    } finally {
      this.quieting.delete(tabId);
    }
  }

  private async sendDraft(p: PickState): Promise<void> {
    if (!p.port) return;
    const clipUrl = p.clip ? await dataUrl(p.clip) : null;
    // Read after the await: a composer that posted or went meanwhile hears no draft.
    tell(p.port, { t: "draft", anchor: p.anchor, clipUrl, clipError: p.clipError, capturing: false });
  }

  private async maybePost(p: PickState): Promise<void> {
    if (p.posting || p.body === null) return;
    if (p.tooLong) {
      p.body = null;
      tell(p.port, { t: "failed", message: URL_TOO_LONG });
      return;
    }
    if (!p.url || p.snapshot === null) return;
    p.posting = true;
    const f = new FormData();
    f.set("pick_id", p.pickId);
    f.set("url", p.url);
    f.set("title", p.title);
    f.set("anchor", JSON.stringify(p.anchor));
    f.set("body", p.body);
    f.set("snapshot", new Blob([p.snapshot], { type: "text/html" }), "index.html");
    if (p.clip) f.set("clip", p.clip, "clip.png");
    let r: Awaited<ReturnType<Api["postThread"]>>;
    try {
      r = await this.d.api.postThread(f, p.pending);
    } catch (e) {
      p.posting = false;
      p.body = null;
      tell(p.port, { t: "failed", message: postFailure(e) });
      return;
    }
    // The thread exists: the pick is done whether or not its composer hears it.
    if (this.byTab.get(p.tabId) === p) this.drop(p.tabId);
    const port = p.port;
    p.port = null;
    tell(port, { t: "posted", threadId: r.thread.id });
    cut(port);
    this.d.toOverlay(p.tabId, { t: "close-composer", pickId: p.pickId, posted: true });
    this.d.posted?.(p.tabId, r.page);
  }
}
