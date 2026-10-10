// The page's client of the browser's one event stream (`GET /api/stream`,
// held by `stream-hub.ts`). Views watch topics (`gallery`, `artifact:<id>`,
// `presence:<id>`, `working:<id>`, `docs:<id>`) as they mount and unwatch
// them as they unmount; the page tells the hub the union, and no connection
// opens or closes on a view change. The hub lives in a shared worker, so
// every Clax tab of this origin shares one connection; where shared workers
// are missing, the tabs elect a leader that holds it for them (Web Locks and
// a BroadcastChannel), and without those each tab holds its own.
//
// A watcher hears `ready` when its topics go live (refetch, then apply the
// deltas that follow), `resync` when a topic's events were dropped (refetch
// it), `stream_down` and `stream_up` around an outage, during which the
// page shows a quiet notice. A page hidden for `HIDDEN_MS` releases its
// topics other than background ones; holding none, it leaves the hub (a
// leader tab hands the connection to another, as a hidden tab may be
// frozen). It takes them again (with a `ready`) when it shows; so does a page
// hidden for good or in the back/forward cache. A hub that is lost again
// before saying anything is joined again after a backoff, and a shared
// worker that fails before saying anything is not asked for again. The
// token never goes in a URL: the hub's requests carry the events cookie
// that `GET /api/token` sets for the shell.
//
// A background watcher (`questions` and `inbox`) keeps its topics while the
// page is hidden: a hidden page releases only the other topics, and stays
// with the hub. The page tells the hub whether it has focus (on `focus`,
// `blur` and `visibilitychange`, and once it joins), so the hub can ask the
// most recently focused tab to notify when no tab has focus (`onNotify`).
import { getToken } from "./api";
import { after } from "./clock";
import { STREAM_DOWN, connTrouble } from "./conn-notice";
import { backoff } from "./lifecycle";
import type { HubMsg, TabMsg } from "./stream-hub";

/** One message on the stream, as a watcher hears it: `ready`, `resync`
 * (with its `topic`), `refused` (with its `topic` and `code`),
 * `stream_down`, `stream_up`, or a stream event by name, carrying its
 * `topic` and the fields of the daemon's delta. */
export type StreamEvent = { type: string; topic?: string; [field: string]: unknown };

/** How long the stream may be down before the notice shows. */
export const NOTICE_MS = 1500;
/** How long a hidden page keeps its topics. */
export const HIDDEN_MS = 30_000;
/** How long a shared worker's hub may stay silent (it pings every 10 s)
 * before the page counts it dead and connects to a new one. */
export const DEAD_MS = 35_000;
/** How long the page waits for the token request (which sets the events
 * cookie) before connecting without it. */
export const TOKEN_WAIT_MS = 3000;

/** The page's line to the hub; `pinged` when the hub's pings show it alive;
 * `leader` when it is a leader tab's (which may be this tab's own hub). */
export type Link = { send(m: TabMsg): void; close(): void; pinged?: boolean; leader?: boolean };
/** Makes a link that hands the hub's messages to `on`, and calls `lost`
 * when it can tell the hub is gone. */
export type LinkMaker = (on: (m: HubMsg) => void, lost: () => void) => Promise<Link>;

type Timer = ReturnType<typeof setTimeout>;
type Watcher = { topics: Set<string>; on: (e: StreamEvent) => void; background: boolean };

/** Holds Web Lock `name` until the returned function runs; null without Web Locks. */
export async function holdLock(name: string): Promise<(() => void) | null> {
  const locks = typeof navigator === "undefined" ? undefined : navigator.locks;
  if (!locks) return null;
  return new Promise(granted => {
    void locks.request(name, () => new Promise<void>(release => granted(release))).catch(() => granted(null));
  });
}

const newId = () => (typeof crypto !== "undefined" && "randomUUID" in crypto ? crypto.randomUUID() : `${Date.now()}-${Math.random()}`);

/** The shared worker failed before it said anything (its script would not
 * load, say): this page uses the fallbacks from then on. */
let workerBroken = false;

/** A link to the shared worker's hub. */
export const workerLink: LinkMaker = async (on, lost) => {
  const w = new SharedWorker(new URL("./stream-worker.ts", import.meta.url), { name: "clax-stream" });
  const port = w.port;
  let closed = false;
  let heard = false;
  port.onmessage = e => { heard = true; if (!closed) on(e.data as HubMsg); };
  // Where the browser says so, the worker's end closing (it crashed).
  port.addEventListener("close", () => { if (!closed) lost(); });
  w.addEventListener("error", () => {
    if (!heard) workerBroken = true;
    if (!closed) lost();
  });
  port.start();
  const lock = `clax-tab:${newId()}`;
  const release = await holdLock(lock);
  port.postMessage({ t: "hello", lock: release ? lock : undefined } satisfies TabMsg);
  return {
    pinged: true,
    send: m => { if (!closed) port.postMessage(m); },
    close: () => {
      if (closed) return;
      port.postMessage({ t: "bye" } satisfies TabMsg);
      closed = true;
      port.close();
      release?.();
    },
  };
};

/** The name of the leader's Web Lock and BroadcastChannel. */
export const LEADER = "clax-stream";

/** A link to the hub of the leader tab, which this tab may be: the tab
 * holding Web Lock `clax-stream` runs the hub and reaches the others over
 * BroadcastChannel `clax-stream`. When the leader leaves, the next tab in
 * line takes the lock and announces itself, and every tab sends it its
 * state again. */
export const leaderLink: LinkMaker = async on => {
  const { Hub } = await import("./stream-hub");
  const id = newId();
  const lock = `clax-tab:${id}`;
  const releaseTab = await holdLock(lock);
  const bc = new BroadcastChannel(LEADER);
  let hub: InstanceType<typeof Hub> | null = null;
  let resign: (() => void) | null = null;
  let closed = false;
  // What a new leader needs to hear again: the hello and the latest topics.
  const hello: TabMsg = { t: "hello", lock: releaseTab ? lock : undefined };
  let topics: TabMsg | null = null;
  let focus: TabMsg | null = null;
  const post = (m: TabMsg) => {
    if (hub) hub.receive(id, m);
    else bc.postMessage({ k: "tab", from: id, msg: m });
  };
  const resend = () => { post(hello); if (topics) post(topics); if (focus) post(focus); };
  bc.onmessage = (e: MessageEvent) => {
    const m = e.data;
    if (closed || !m || typeof m !== "object") return;
    if (m.k === "hub" && Array.isArray(m.to) && m.to.includes(id)) on(m.msg as HubMsg);
    else if (m.k === "tab" && hub && typeof m.from === "string") hub.receive(m.from, m.msg as TabMsg);
    else if (m.k === "leader" && !hub) resend();
  };
  void navigator.locks.request(LEADER, () => {
    if (closed) return;
    hub = new Hub({
      notify: true,
      send(ids, msg) {
        const others = ids.filter(x => x !== id);
        if (others.length) bc.postMessage({ k: "hub", to: others, msg });
        if (others.length !== ids.length) on(msg);
      },
      watchLock: (name, gone) => { void navigator.locks.request(name, () => gone()).catch(() => {}); },
    });
    resend();
    bc.postMessage({ k: "leader" });
    return new Promise<void>(r => { resign = r; });
  }).catch(() => {});
  post(hello);
  return {
    // Whether this tab holds the connection for the others.
    get leader() { return hub !== null; },
    send: m => { if (closed) return; if (m.t === "topics") topics = m; if (m.t === "focus") focus = m; post(m); },
    close: () => {
      if (closed) return;
      post({ t: "bye" });
      closed = true;
      hub?.close();
      hub = null;
      resign?.();
      bc.close();
      releaseTab?.();
    },
  };
};

/** A hub of this tab's own: one connection per tab. */
export const localLink: LinkMaker = async on => {
  const { Hub } = await import("./stream-hub");
  const hub = new Hub({ notify: true, send: (_ids, msg) => on(msg) });
  return { send: m => hub.receive("self", m), close: () => hub.close() };
};

/** The best link this browser supports. */
export const bestLink: LinkMaker = async (on, lost) => {
  if (typeof SharedWorker === "function" && !workerBroken) {
    try { return await workerLink(on, lost); } catch { /* blocked: fall back */ }
  }
  if (typeof BroadcastChannel === "function" && typeof navigator !== "undefined" && navigator.locks) return leaderLink(on, lost);
  return localLink(on, lost);
};

/** In tests only: every stream that had watchers, across the module copies
 * a test file loads, so the setup can fail a test that leaves one open
 * (linked, or with a timer pending, which would fire after the environment
 * is torn down). */
const watched: Set<EventStream> | null = import.meta.env.MODE === "test"
  ? ((globalThis as { claxWatchedStreams?: Set<EventStream> }).claxWatchedStreams ??= new Set())
  : null;

export class EventStream {
  private watchers: Watcher[] = [];
  private link: Link | null = null;
  /** The link being made; replaced or cleared when the page leaves meanwhile. */
  private linking: object | null = null;
  /** The topics last sent to the hub, as a key. */
  private sent = "";
  private queued = false;
  /** The page has been hidden for `HIDDEN_MS`: its topics other than background ones are released. */
  private released = false;
  /** The page is leaving (or in the back/forward cache). */
  private gone = false;
  /** Watchers were told `stream_down`, and not yet `stream_up`. */
  private down = false;
  private heard = Date.now();
  private hiddenTimer: (() => void) | undefined;
  private noticeTimer: Timer | undefined;
  private watchdog: ReturnType<typeof setInterval> | undefined;
  /** Hubs lost in a row without a word from any; the next join waits on it. */
  private losses = 0;
  private rejoin: Timer | undefined;
  /** The viewer changed while this page had no link. */
  private wantReconnect = false;
  /** The caller last asked for, for a reconnect asked before a link. */
  private caller: string | undefined;
  private hooked = false;
  private notifyHandlers = new Set<(data: Record<string, unknown>) => void>();
  /** What this link's hub was last told about focus; null until it is told (each new link). */
  private focusSent: boolean | null = null;

  constructor(private readonly win: Window = window, private readonly makeLink: LinkMaker = bestLink) {}

  /** The topics its watchers hold (tests). */
  get watching(): string[] { return [...new Set(this.watchers.flatMap(w => [...w.topics]))].sort(); }

  /** Whether it holds anything open: watchers, or a timer (tests). An idle
   * link, holding no topic, keeps no timer and counts as closed. */
  get open(): boolean {
    return this.watchers.length > 0 || this.rejoin !== undefined || this.noticeTimer !== undefined || this.watchdog !== undefined;
  }

  /** The topics this page holds now (tests). */
  get topics(): string[] { return this.sent ? this.sent.split("\n") : []; }

  /** Hands `on` the events of `topics` until the returned function runs.
   * A `background` watcher keeps its topics while the page is hidden. */
  watch(topics: readonly string[], on: (e: StreamEvent) => void, opts: { background?: boolean } = {}): () => void {
    const w: Watcher = { topics: new Set(topics), on, background: opts.background === true };
    this.watchers.push(w);
    watched?.add(this);
    this.hook();
    this.schedule();
    return () => {
      const i = this.watchers.indexOf(w);
      if (i < 0) return;
      this.watchers.splice(i, 1);
      this.schedule();
    };
  }

  /** Hands `f` what the hub asks this page to notify about (an unread
   * inbox item's event data) until the returned function runs. */
  onNotify(f: (data: Record<string, unknown>) => void): () => void {
    this.notifyHandlers.add(f);
    return () => { this.notifyHandlers.delete(f); };
  }

  /** Opens a new stream for every tab: the caller changed to `caller` (a
   * key), and the daemon fixes a stream's caller when it opens. The hub
   * reopens once per caller, however many tabs ask. Every watcher hears
   * `ready`. Without a link yet, the next link asks for it: the hub it joins
   * may hold a stream opened for the old caller. */
  reconnect(caller?: string): void {
    if (caller !== undefined) this.caller = caller;
    if (this.link) this.link.send({ t: "reconnect", caller });
    else this.wantReconnect = true;
  }

  /** Leaves the hub and forgets every watcher. */
  close(): void {
    this.watchers = [];
    watched?.delete(this);
    this.leave();
    this.hiddenTimer?.();
    this.hiddenTimer = undefined;
    if (this.hooked) {
      this.win.removeEventListener("pagehide", this.onHide);
      this.win.removeEventListener("pageshow", this.onShow);
      this.win.document.removeEventListener("visibilitychange", this.onVisibility);
      this.win.removeEventListener("focus", this.onFocus);
      this.win.removeEventListener("blur", this.onFocus);
      this.hooked = false;
    }
  }

  private hook(): void {
    if (this.hooked) return;
    this.hooked = true;
    this.win.addEventListener("pagehide", this.onHide);
    this.win.addEventListener("pageshow", this.onShow);
    this.win.document.addEventListener("visibilitychange", this.onVisibility);
    this.win.addEventListener("focus", this.onFocus);
    this.win.addEventListener("blur", this.onFocus);
    if (this.win.document.visibilityState === "hidden") this.onVisibility();
  }

  /** Tells the hub whether this page has focus, when that changed. */
  private onFocus = () => {
    if (!this.link) return;
    const d = this.win.document;
    const focused = d.visibilityState !== "hidden" && d.hasFocus();
    if (focused === this.focusSent) return;
    this.focusSent = focused;
    this.link.send({ t: "focus", focused });
  };

  private onHide = () => {
    this.gone = true;
    this.leave();
  };

  private onShow = (e: Event) => {
    if (!(e as PageTransitionEvent).persisted) return;
    this.gone = false;
    this.schedule();
  };

  private onVisibility = () => {
    this.onFocus();
    this.hiddenTimer?.();
    this.hiddenTimer = undefined;
    if (this.win.document.visibilityState === "hidden") {
      this.hiddenTimer = after(HIDDEN_MS, () => {
        this.hiddenTimer = undefined;
        this.released = true;
        // Background topics stay: the page keeps only them. A leader link is
        // left and joined again, so a shown tab takes over the connection
        // (a hidden tab may be frozen) and this one follows it.
        if (!this.watchers.some(w => w.background)) return this.leave();
        if (this.link?.leader) this.leave();
        this.schedule();
      });
    } else if (this.released) {
      this.released = false;
      this.schedule();
    }
  };

  /** Sends the page's topics once the current task is done, so a view
   * mounting several watchers sends one set. */
  private schedule(): void {
    if (this.queued) return;
    this.queued = true;
    queueMicrotask(() => { this.queued = false; this.sync(); });
  }

  private union(): string[] {
    if (this.gone) return [];
    const out = new Set<string>();
    for (const w of this.watchers) if (!this.released || w.background) for (const t of w.topics) out.add(t);
    return [...out].sort();
  }

  private sync(): void {
    // Nothing watches it any more: no hub to join again, and no stream-down
    // notice for no one (its timer would outlive the page's views).
    if (!this.watchers.length) this.idle();
    const topics = this.union();
    const key = topics.join("\n");
    if (!this.link) {
      if (topics.length && this.rejoin === undefined) this.connect();
      return;
    }
    if (key === this.sent) return;
    this.sent = key;
    this.link.send({ t: "topics", topics });
    // The hub is watched only while the page holds topics.
    if (!topics.length) { clearInterval(this.watchdog); this.watchdog = undefined; }
    else if (this.link.pinged && !this.watchdog) { this.heard = Date.now(); this.watchdog = setInterval(() => this.check(), DEAD_MS / 3); }
  }

  private connect(): void {
    if (this.linking) return;
    const ticket = {};
    this.linking = ticket;
    const box: { link: Link | null } = { link: null };
    const on = (m: HubMsg) => { if (box.link && this.link === box.link) this.onMessage(m); };
    const lost = () => { if (box.link) this.lost(box.link); };
    void (async () => {
      // The token request sets the events cookie the hub's requests carry.
      await Promise.race([getToken(), new Promise(r => setTimeout(r, TOKEN_WAIT_MS))]);
      let link: Link;
      try { link = await this.makeLink(on, lost); } catch { link = await localLink(on, lost); }
      box.link = link;
      if (this.linking !== ticket) { link.close(); return; }
      this.linking = null;
      this.link = link;
      this.sent = "";
      this.focusSent = null;
      if (this.wantReconnect) { this.wantReconnect = false; link.send({ t: "reconnect", caller: this.caller }); }
      this.sync();
      this.onFocus();
    })();
  }

  /** No watcher is left: the timers that serve watchers stop; an idle link stays for the next. */
  private idle(): void {
    clearTimeout(this.rejoin);
    this.rejoin = undefined;
    clearTimeout(this.noticeTimer);
    this.noticeTimer = undefined;
    if (this.down) { this.down = false; connTrouble("stream", false, STREAM_DOWN, this.win.document); }
  }

  /** Leaves the hub: its topics go, and the notice with them. */
  private leave(): void {
    clearTimeout(this.rejoin);
    this.rejoin = undefined;
    this.linking = null;
    this.link?.close();
    this.link = null;
    this.sent = "";
    clearInterval(this.watchdog);
    this.watchdog = undefined;
    clearTimeout(this.noticeTimer);
    this.noticeTimer = undefined;
    this.down = false;
    connTrouble("stream", false, STREAM_DOWN, this.win.document);
  }

  /** The hub is gone (its worker crashed, or went silent): the page says
   * the stream is down and connects to a new hub, whose `live` refetches. */
  private lost(link: Link): void {
    if (this.link !== link) return;
    this.link = null;
    link.close();
    this.sent = "";
    clearInterval(this.watchdog);
    this.watchdog = undefined;
    this.markDown();
    // At once the first time; after a backoff while hubs keep failing.
    const wait = this.losses === 0 ? 0 : backoff(this.losses - 1);
    this.losses++;
    clearTimeout(this.rejoin);
    this.rejoin = setTimeout(() => { this.rejoin = undefined; this.sync(); }, wait);
    if (!this.watchers.length) this.idle();
  }

  private check(): void {
    if (!this.link || !this.sent || this.win.document.visibilityState === "hidden") return;
    if (Date.now() - this.heard > DEAD_MS) this.lost(this.link);
  }

  private markDown(): void {
    if (this.down) return;
    this.down = true;
    this.tell(() => true, { type: "stream_down" });
    this.noticeTimer ??= setTimeout(() => {
      this.noticeTimer = undefined;
      if (this.down && !this.gone) connTrouble("stream", true, STREAM_DOWN, this.win.document);
    }, NOTICE_MS);
  }

  private markUp(): void {
    clearTimeout(this.noticeTimer);
    this.noticeTimer = undefined;
    connTrouble("stream", false, STREAM_DOWN, this.win.document);
    if (!this.down) return;
    this.down = false;
    this.tell(() => true, { type: "stream_up" });
  }

  private tell(pick: (w: Watcher) => boolean, e: StreamEvent): void {
    for (const w of [...this.watchers]) if (this.watchers.includes(w) && pick(w)) w.on(e);
  }

  private onMessage(m: HubMsg): void {
    this.heard = Date.now();
    this.losses = 0;
    switch (m.t) {
      case "event": {
        // Counts browser tests read: events this page has been handed, and `live` messages.
        const d = this.win as unknown as { claxStreamEvents?: number };
        d.claxStreamEvents = (d.claxStreamEvents ?? 0) + 1;
        this.tell(w => w.topics.has(m.topic), { ...m.data, type: m.name, topic: m.topic });
        break;
      }
      case "live": {
        // A topic went live, so the stream is up (a new hub says so this way).
        const d = this.win as unknown as { claxStreamLive?: number };
        d.claxStreamLive = (d.claxStreamLive ?? 0) + 1;
        this.markUp();
        const live = new Set(m.topics);
        this.tell(w => [...w.topics].some(t => live.has(t)), { type: "ready" });
        break;
      }
      case "resync": this.tell(w => w.topics.has(m.topic), { type: "resync", topic: m.topic }); break;
      case "refused": this.tell(w => w.topics.has(m.topic), { type: "refused", topic: m.topic, code: m.code }); break;
      case "status": if (m.up) this.markUp(); else this.markDown(); break;
      case "notify": for (const f of [...this.notifyHandlers]) f(m.data); break;
      // Answered, so a hub that cannot watch this tab's Web Lock keeps it.
      case "ping": this.link?.send({ t: "ping" }); break;
    }
  }
}

let page: EventStream | null = null;
/** This page's stream. */
export function pageStream(): EventStream {
  return (page ??= new EventStream());
}
