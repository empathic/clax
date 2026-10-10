// The browser's one connection to `GET /api/stream`, shared by every Clax
// tab of this origin. The hub runs where the connection lives: in the shared
// worker, in the elected leader tab when shared workers are missing, or in
// the tab itself as a last resort. Each tab tells it the whole set of topics
// it wants (`topics`); the hub subscribes the stream to their union over
// `POST /api/stream/<id>`, drops a topic once no tab wants it, and hands
// each event only to the tabs that want its topic. A tab hears `live` for a
// topic once the stream carries it for that tab: its views refetch then, and
// apply the stream's deltas after.
//
// The connection resumes after a drop (`Last-Event-ID`) with backoff; the
// tabs hear `status` so they can show the reconnect notice. It closes a
// moment after no tab wants any topic. Every request has a timeout: the
// stream must say `ready` within `CONNECT_MS` and must not go silent for
// `IDLE_MS` (the daemon sends a keep-alive every 15 s); a subscription that
// does not answer within `STUCK_MS` fails the connection, which reconnects.
//
// Notifications (a hub made with `notify`): tabs say whether they have focus.
// For each new unread inbox item (an `inbox_item` event whose `seq` is above
// every item the hub has seen, so an older item marked unread again or
// changed is not announced), when no tab has focus, the hub asks the most
// recently focused tab holding `inbox` to notify (`notify`). Items that
// arrive while the stream does not carry `inbox` yet (between a tab first
// wanting it and the subscription) or again (after a new stream) send no
// event: once the subscription answers, the hub fetches the unread items
// and announces those newer than the newest item when a tab first wanted
// `inbox` (fetched then) and not announced already, oldest first, at most
// the newest `CATCH_UP_SHOWN` of them.
import { STUCK_MS, backoff } from "./lifecycle";
import { parseBlock } from "./sse";

/** What a tab sends the hub. `hello` names the tab's Web Lock (held while
 * the tab lives), when it has one; `topics` is the tab's whole set;
 * `focus` whether the tab has focus; `reconnect` opens a new stream, the
 * caller having changed to `caller` (a key: see `callerKey`), unless the
 * stream is already opened as that caller. */
export type TabMsg =
  | { t: "hello"; lock?: string }
  | { t: "topics"; topics: string[] }
  | { t: "focus"; focused: boolean }
  | { t: "reconnect"; caller?: string }
  | { t: "ping" }
  | { t: "bye" };

/** What the hub sends a tab. `event` carries one stream event of a topic
 * the tab wants; `live` names topics the stream now carries for the tab
 * (refetch them); `resync` a topic whose events were dropped (refetch it);
 * `refused` a topic the daemon would not subscribe (`code` says why);
 * `status` whether the connection is up; `notify` asks the tab to show a
 * notification for an `inbox_item` event's data. */
export type HubMsg =
  | { t: "event"; topic: string; name: string; data: Record<string, unknown> }
  | { t: "notify"; data: Record<string, unknown> }
  | { t: "live"; topics: string[] }
  | { t: "resync"; topic: string }
  | { t: "refused"; topic: string; code: string }
  | { t: "status"; up: boolean }
  | { t: "ping" };

/** How long the stream may take to say `ready`. */
export const CONNECT_MS = 5000;
/** How long the stream may stay silent; the daemon's keep-alive is every 15 s. */
export const IDLE_MS = 40_000;
/** How long the connection stays after the last topic is dropped, so a tab
 * moving between views (or reloading) does not reopen it. */
export const LINGER_MS = 3000;
/** How often the hub pings each tab, so a tab can tell a dead hub. */
export const PING_MS = 10_000;
/** A tab without a Web Lock that has not been heard from in this long is gone. */
export const CLIENT_TTL_MS = 180_000;
/** Most unread items one catch-up fetches (newest first). */
export const CATCH_UP = 20;
/** Most of those one catch-up announces: the newest, so a long outage
 * ends in a few notifications, not a burst (the count says the rest). */
export const CATCH_UP_SHOWN = 3;

type Timer = ReturnType<typeof setTimeout>;

/** The key of the caller a stream's `ready` names (`{level, viewer}`), as
 * the tabs compute it (view/artifact-stream.ts): `admin` for the owner's
 * browser, named or not; else the viewer's public ID and whether it is
 * named (`interact`); null when `ready` names none. */
export function callerKey(c: unknown): string | null {
  if (!c || typeof c !== "object") return null;
  const { level, viewer } = c as { level?: unknown; viewer?: unknown };
  if (typeof level !== "string") return null;
  return level === "admin" || level === "owner" ? level : `${viewer}${level === "interact"}`;
}

/** `focusedAt` orders the tabs by when they last gained focus (0: never). */
type Client = { topics: Set<string>; live: Set<string>; heard: number; locked: boolean; focused: boolean; focusedAt: number };

/** Where the hub runs: how it reaches tabs, makes requests, and watches a
 * tab's Web Lock. */
export type HubEnv = {
  /** Sends `msg` to each tab in `ids`. */
  send(ids: string[], msg: HubMsg): void;
  fetch?: typeof fetch;
  /** Calls `gone` once the lock `name` is free (its tab ended); absent
   * without Web Locks. */
  watchLock?(name: string, gone: () => void): void;
  /** The daemon's origin, for requests (default: relative URLs). */
  base?: string;
  /** Asks tabs to notify about unread inbox items (the shell's hubs only). */
  notify?: boolean;
};

export class Hub {
  private clients = new Map<string, Client>();
  /** Topics the open stream carries, as the daemon last said. */
  private server = new Set<string>();
  /** Topics a subscription request in flight removes: not live for a tab
   * that asks for one meanwhile, since the stream is about to drop it. */
  private removing = new Set<string>();
  /** Topics the daemon refused, with its code; retried once no tab wants them. */
  private refused = new Map<string, string>();
  private streamId: string | null = null;
  /** The caller the open stream was opened for, as its `ready` said. */
  private caller: string | null = null;
  /** The caller a tab asked for while no stream was up: the next `ready`
   * says whether its stream is opened as it. */
  private asked: string | null = null;
  private seq = 0;
  private conn: AbortController | null = null;
  private up = false;
  /** The tabs were told the connection is down, and not yet that it is up. */
  private toldDown = false;
  private failures = 0;
  private retry: Timer | undefined;
  private linger: Timer | undefined;
  private watchdog: Timer | undefined;
  private pinger: ReturnType<typeof setInterval> | undefined;
  private syncing = false;
  private dirty = false;
  private readonly fetch: typeof fetch;
  /** The highest inbox item `seq` seen, or older than when a tab first wanted `inbox`. */
  private seenSeq = -1;
  /** Some tab wants `inbox` (hubs made with `notify`). */
  private inboxWanted = false;
  /** Since a tab first wanted `inbox`: the request that set `seenSeq` to the
   * newest item's `seq`, resolving to whether it did. */
  private inboxBase: Promise<boolean> | null = null;
  /** While a catch-up runs: the `seq`s announced from events meanwhile,
   * which it skips (`seenSeq` stays its baseline until it ends). */
  private windowSeen: Set<number> | null = null;
  private focusSeq = 0;

  constructor(private readonly env: HubEnv) {
    this.fetch = env.fetch ?? ((...a) => fetch(...a));
  }

  /** Counts for tests and the page's debug hook. */
  stats(): { clients: number; topics: string[]; connected: boolean; up: boolean; stream: string | null } {
    return { clients: this.clients.size, topics: [...this.wanted()].sort(), connected: this.conn !== null, up: this.up, stream: this.streamId };
  }

  /** Handles `msg` from tab `id`; an unknown tab is added. */
  receive(id: string, msg: TabMsg): void {
    if (msg.t === "bye") { this.detach(id); return; }
    let c = this.clients.get(id);
    if (!c) {
      c = { topics: new Set(), live: new Set(), heard: Date.now(), locked: false, focused: false, focusedAt: 0 };
      this.clients.set(id, c);
      // Tells a tab that joins during an outage.
      if (this.toldDown) this.env.send([id], { t: "status", up: false });
    }
    c.heard = Date.now();
    switch (msg.t) {
      case "hello":
        if (msg.lock && this.env.watchLock && !c.locked) {
          c.locked = true;
          const client = c;
          this.env.watchLock(msg.lock, () => { if (this.clients.get(id) === client) this.detach(id); });
        }
        break;
      case "topics": {
        const next = new Set(msg.topics);
        for (const t of c.live) if (!next.has(t)) c.live.delete(t);
        c.topics = next;
        // Topics the stream carries already are live for this tab at once.
        if (this.up) this.tellLive([id], [...next].filter(t => this.server.has(t) && !this.removing.has(t) && !c.live.has(t)));
        for (const t of next) {
          const code = this.refused.get(t);
          if (code) this.env.send([id], { t: "refused", topic: t, code });
        }
        this.changed();
        break;
      }
      case "focus":
        c.focused = msg.focused;
        if (msg.focused) c.focusedAt = ++this.focusSeq;
        break;
      case "reconnect":
        // Every tab asks when the caller changes (each hears the change), and
        // the hub may have reopened already (the daemon refused the old stream
        // to the new caller): the stream's own `ready` says what it is opened
        // as, so the first ask reopens it and the others find it done.
        if (msg.caller !== undefined) {
          if (!this.up) { this.asked = msg.caller; break; }
          if (msg.caller === this.caller) break;
        }
        this.streamId = null;
        if (this.conn) this.connect();
        break;
      case "ping": break;
    }
  }

  /** Tab `id` left: its topics go with it. */
  detach(id: string): void {
    if (!this.clients.delete(id)) return;
    this.changed();
  }

  /** Ends everything: the connection, timers, tabs. */
  close(): void {
    this.clients.clear();
    clearInterval(this.pinger);
    this.pinger = undefined;
    this.closeIdle();
  }

  private ping(): void {
    const now = Date.now();
    for (const [id, c] of [...this.clients]) {
      if (!c.locked && now - c.heard > CLIENT_TTL_MS) this.detach(id);
    }
    if (this.clients.size) this.env.send([...this.clients.keys()], { t: "ping" });
  }

  /** The union of every tab's topics, less those the daemon refused. */
  private wanted(): Set<string> {
    const out = new Set<string>();
    for (const c of this.clients.values()) for (const t of c.topics) out.add(t);
    for (const t of [...this.refused.keys()]) if (!out.has(t)) this.refused.delete(t);
    for (const t of this.refused.keys()) out.delete(t);
    return out;
  }

  private changed(): void {
    const want = this.wanted();
    if (this.env.notify && want.has("inbox") !== this.inboxWanted) {
      this.inboxWanted = !this.inboxWanted;
      this.inboxBase = this.inboxWanted ? this.baseline() : null;
    }
    // Tabs are pinged (and tabs without a lock expire) only while some tab
    // holds topics, refused ones included: such a tab still watches the hub.
    if ([...this.clients.values()].some(c => c.topics.size)) this.pinger ??= setInterval(() => this.ping(), PING_MS);
    else { clearInterval(this.pinger); this.pinger = undefined; }
    if (!want.size) {
      if (this.conn && this.linger === undefined) this.linger = setTimeout(() => { this.linger = undefined; if (!this.wanted().size) this.closeIdle(); }, LINGER_MS);
      if (!this.conn) { clearTimeout(this.retry); this.retry = undefined; }
      else if (this.up) void this.flush();
      return;
    }
    clearTimeout(this.linger);
    this.linger = undefined;
    if (!this.conn && this.retry === undefined) this.connect();
    else if (this.up) void this.flush();
  }

  private tellLive(ids: string[], topics: string[]): void {
    if (!topics.length) return;
    for (const id of ids) {
      const c = this.clients.get(id);
      if (!c) continue;
      const fresh = topics.filter(t => c.topics.has(t) && !c.live.has(t));
      if (!fresh.length) continue;
      for (const t of fresh) c.live.add(t);
      this.env.send([id], { t: "live", topics: fresh });
    }
  }

  private wanting(topic: string): string[] {
    const out: string[] = [];
    for (const [id, c] of this.clients) if (c.topics.has(topic)) out.push(id);
    return out;
  }

  // ---- the connection ----

  private url(path: string): string { return (this.env.base ?? "") + path; }

  private connect(): void {
    this.drop();
    clearTimeout(this.retry);
    this.retry = undefined;
    const ac = new AbortController();
    this.conn = ac;
    this.arm(ac, CONNECT_MS);
    void this.run(ac).catch(() => { if (this.conn === ac) this.fail(); });
  }

  private async run(ac: AbortController): Promise<void> {
    // After a failure the daemon may have restarted with a new token: the
    // token request renews the events cookie (a LAN view gets a 403 and none).
    if (this.failures > 0) await this.fetch(this.url("/api/token"), { signal: ac.signal, cache: "no-store" }).catch(() => null);
    const headers: Record<string, string> = {};
    if (this.streamId) headers["Last-Event-ID"] = `${this.streamId}:${this.seq}`;
    const res = await this.fetch(this.url("/api/stream"), { headers, signal: ac.signal, cache: "no-store", credentials: "same-origin" });
    if (!res.ok || !res.body) throw new Error(`stream answered ${res.status}`);
    const reader = res.body.getReader();
    const dec = new TextDecoder();
    let buf = "";
    try {
      for (;;) {
        const { value, done } = await reader.read();
        if (this.conn !== ac) return;
        if (done) throw new Error("the stream ended");
        if (this.up) this.arm(ac, IDLE_MS);
        buf = (buf + dec.decode(value, { stream: true })).replace(/\r\n/g, "\n");
        let i: number;
        while ((i = buf.indexOf("\n\n")) >= 0) {
          const m = parseBlock(buf.slice(0, i));
          buf = buf.slice(i + 2);
          if (m && this.conn === ac) this.frame(ac, m.event, m.data, m.id);
        }
      }
    } finally {
      reader.cancel().catch(() => {});
    }
  }

  /** Fails connection `ac` unless it is heard from within `ms`. */
  private arm(ac: AbortController, ms: number): void {
    clearTimeout(this.watchdog);
    this.watchdog = setTimeout(() => { if (this.conn === ac) this.fail(); }, ms);
  }

  private frame(ac: AbortController, name: string, raw: string, id: string | undefined): void {
    let data: Record<string, unknown>;
    try { data = JSON.parse(raw); } catch { return; }
    if (!data || typeof data !== "object") return;
    if (name === "ready") {
      this.arm(ac, IDLE_MS);
      this.caller = callerKey(data.caller);
      const asked = this.asked;
      this.asked = null;
      if (asked !== null && asked !== this.caller) {
        // A tab asked for another caller while this stream was being opened.
        this.streamId = null;
        this.connect();
        return;
      }
      const resumed = data.resumed === true && data.stream === this.streamId;
      this.streamId = String(data.stream);
      if (resumed) {
        this.server = new Set(Array.isArray(data.topics) ? data.topics.map(String) : []);
        for (const c of this.clients.values()) for (const t of [...c.live]) if (!this.server.has(t)) c.live.delete(t);
      } else {
        this.seq = Number(data.seq) || 0;
        this.server = new Set();
        for (const c of this.clients.values()) c.live.clear();
      }
      this.up = true;
      this.failures = 0;
      if (this.toldDown) {
        this.toldDown = false;
        if (this.clients.size) this.env.send([...this.clients.keys()], { t: "status", up: true });
      }
      // Topics the stream resumed with are live again for the tabs that still want them.
      if (resumed) for (const [cid, c] of this.clients) this.tellLive([cid], [...c.topics].filter(t => this.server.has(t) && !c.live.has(t)));
      void this.flush();
      return;
    }
    if (id) {
      const n = Number(id.slice(id.lastIndexOf(":") + 1));
      if (Number.isFinite(n) && n > this.seq) this.seq = n;
    }
    const topic = typeof data.topic === "string" ? data.topic : null;
    if (!topic) return;
    const to = this.wanting(topic);
    if (!to.length) return;
    if (name === "resync") this.env.send(to, { t: "resync", topic });
    else this.env.send(to, { t: "event", topic, name, data });
    if (this.env.notify && topic === "inbox" && name === "inbox_item") this.announce(data, to);
  }

  /** Announces an `inbox_item` event's item, once per item: one above
   * every item seen (and, during a catch-up, not announced in it). */
  private announce(data: Record<string, unknown>, to: string[]): void {
    const item = data.item as { seq?: unknown; read?: unknown } | null;
    if (!item || typeof item !== "object" || typeof item.seq !== "number" || item.seq <= this.seenSeq) return;
    if (this.windowSeen) {
      if (this.windowSeen.has(item.seq)) return;
      this.windowSeen.add(item.seq);
    } else this.seenSeq = item.seq;
    this.pick(data, item, to);
  }

  /** Asks one tab of `to` to notify about `item`, when it is unread and no
   * tab has focus: the one that gained focus last (ties: the first). */
  private pick(data: Record<string, unknown>, item: { read?: unknown }, to: string[]): void {
    if (item.read !== false) return;
    if ([...this.clients.values()].some(c => c.focused)) return;
    let best: string | null = null;
    let at = -1;
    for (const id of to) {
      const c = this.clients.get(id);
      if (c && c.focusedAt > at) { best = id; at = c.focusedAt; }
    }
    if (best) this.env.send([best], { t: "notify", data });
  }

  /** A GET of the daemon's JSON, bounded by `STUCK_MS`: "forbidden" on a
   * 403, null when it failed otherwise. */
  private async getJson(path: string): Promise<Record<string, unknown> | "forbidden" | null> {
    const req = new AbortController();
    const stuck = setTimeout(() => req.abort(), STUCK_MS);
    try {
      const res = await this.fetch(this.url(path), { signal: req.signal, cache: "no-store", credentials: "same-origin" });
      if (res.status === 403) return "forbidden";
      if (!res.ok) return null;
      const body: unknown = await res.json();
      return body && typeof body === "object" ? body as Record<string, unknown> : null;
    } catch {
      return null;
    } finally {
      clearTimeout(stuck);
    }
  }

  /** A tab first wants `inbox`: items up to the newest now are not new to
   * it. Whether `seenSeq` was set: not for a caller other than the owner
   * (403, kept while `inbox` is wanted); any other failure is forgotten, so
   * the next subscription asks again. */
  private async baseline(): Promise<boolean> {
    const body = await this.getJson("/api/inbox?limit=1");
    if (body === "forbidden") return false;
    if (!body || !Array.isArray(body.items)) {
      this.inboxBase = null;
      return false;
    }
    const seq = (body.items[0] as { seq?: unknown } | undefined)?.seq;
    this.seenSeq = Math.max(this.seenSeq, typeof seq === "number" ? seq : 0);
    return true;
  }

  /** The stream carries `inbox` again: announces the unread items that came
   * while it did not, oldest first, and ends the window `update` opened. */
  private async catchUp(): Promise<void> {
    const body = await this.getJson(`/api/inbox?read=unread&limit=${CATCH_UP}`);
    const seen = this.windowSeen ?? new Set<number>();
    this.windowSeen = null;
    const items = (body && body !== "forbidden" && Array.isArray(body.items) ? body.items : []) as { seq?: unknown; read?: unknown }[];
    const fresh = items
      .filter((i): i is { seq: number; read?: unknown } => !!i && typeof i.seq === "number" && i.seq > this.seenSeq && !seen.has(i.seq))
      .sort((a, b) => a.seq - b.seq);
    this.seenSeq = Math.max(this.seenSeq, ...seen, ...fresh.map(i => i.seq));
    for (const item of fresh.slice(-CATCH_UP_SHOWN)) this.pick({ topic: "inbox", item }, item, this.wanting("inbox"));
  }

  /** Ends a catch-up window whose subscription did not go through. */
  private endWindow(): void {
    if (!this.windowSeen) return;
    this.seenSeq = Math.max(this.seenSeq, ...this.windowSeen);
    this.windowSeen = null;
  }

  /** Ends the open connection without telling the tabs. */
  private drop(): void {
    clearTimeout(this.watchdog);
    this.watchdog = undefined;
    this.conn?.abort();
    this.conn = null;
    this.up = false;
  }

  /** The connection failed or is stuck: retried after a backoff; the tabs hear it is down. */
  private fail(): void {
    this.drop();
    if (!this.toldDown) {
      this.toldDown = true;
      if (this.clients.size) this.env.send([...this.clients.keys()], { t: "status", up: false });
    }
    if (!this.wanted().size) return;
    clearTimeout(this.retry);
    this.retry = setTimeout(() => { this.retry = undefined; if (this.wanted().size) this.connect(); }, backoff(this.failures++));
  }

  /** No tab wants anything: the connection closes, and the next starts afresh. */
  private closeIdle(): void {
    clearTimeout(this.linger);
    this.linger = undefined;
    clearTimeout(this.retry);
    this.retry = undefined;
    this.drop();
    this.streamId = null;
    this.server.clear();
    this.failures = 0;
    for (const c of this.clients.values()) c.live.clear();
    if (this.toldDown) {
      this.toldDown = false;
      if (this.clients.size) this.env.send([...this.clients.keys()], { t: "status", up: true });
    }
  }

  // ---- subscriptions ----

  /** Brings the stream's topics to the tabs' union, one request at a time. */
  private async flush(): Promise<void> {
    if (this.syncing) { this.dirty = true; return; }
    this.syncing = true;
    try {
      // Bounded, so a daemon that answers without the topics asked for
      // cannot keep the hub asking.
      for (let round = 0; round < 8; round++) {
        this.dirty = false;
        if (!this.up || !this.streamId) return;
        const want = this.wanted();
        const add = [...want].filter(t => !this.server.has(t));
        const remove = [...this.server].filter(t => !want.has(t));
        if (!add.length && !remove.length) return;
        if (!(await this.update(add, remove))) return;
      }
    } finally {
      this.syncing = false;
      if (this.dirty) void this.flush();
    }
  }

  /** One subscription request; false when the connection was failed or
   * replaced meanwhile. A request the daemon refuses for one topic is
   * split, so the other topics still go through. */
  private async update(add: string[], remove: string[]): Promise<boolean> {
    // Subscribing `inbox`: once its baseline is known, events are kept aside
    // from the request on, so the catch-up after it neither repeats nor skips one.
    // Only the call that opens the window catches up: a request split
    // because the daemon refused another topic subscribes `inbox` again inside it.
    if (this.env.notify && add.includes("inbox") && this.inboxWanted) this.inboxBase ??= this.baseline();
    const catching = this.env.notify && add.includes("inbox") && this.inboxBase !== null && await this.inboxBase;
    const ac = this.conn;
    const id = this.streamId;
    if (!ac || !id) return false;
    const opened = catching && !this.windowSeen;
    if (opened) this.windowSeen = new Set();
    const caught = await this.subscribe(ac, id, add, remove);
    if (opened) {
      if (caught && this.server.has("inbox")) await this.catchUp();
      else this.endWindow();
    }
    return caught;
  }

  /** [`update`]'s request on connection `ac`, stream `id`. */
  private async subscribe(ac: AbortController, id: string, add: string[], remove: string[]): Promise<boolean> {
    const req = new AbortController();
    const stuck = setTimeout(() => req.abort(), STUCK_MS);
    const abort = () => req.abort();
    ac.signal.addEventListener("abort", abort);
    for (const t of remove) this.removing.add(t);
    let res: Response;
    try {
      res = await this.fetch(this.url(`/api/stream/${id}`), {
        method: "POST", signal: req.signal, credentials: "same-origin",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ subscribe: add, unsubscribe: remove }),
      });
    } catch {
      if (this.conn === ac) this.fail();
      return false;
    } finally {
      clearTimeout(stuck);
      ac.signal.removeEventListener("abort", abort);
      for (const t of remove) this.removing.delete(t);
    }
    if (this.conn !== ac || this.streamId !== id) return false;
    let body: { topics?: unknown; error?: { code?: unknown } } = {};
    try { body = await res.json(); } catch { /* no body */ }
    if (res.ok) {
      this.server = new Set(Array.isArray(body.topics) ? body.topics.map(String) : []);
      // A topic the stream no longer carries is no longer live for any tab:
      // its next subscription tells them again, and they refetch then.
      for (const c of this.clients.values()) for (const t of [...c.live]) if (!this.server.has(t)) c.live.delete(t);
      const live = add.filter(t => this.server.has(t));
      this.tellLive([...this.clients.keys()], live);
      return true;
    }
    const code = typeof body.error?.code === "string" ? body.error.code : `http_${res.status}`;
    if (code === "unknown_stream") {
      // The daemon no longer holds this stream for this caller (the viewer
      // changed, or it restarted): a new one, and every tab refetches.
      this.streamId = null;
      this.connect();
      return false;
    }
    if (res.status >= 500) { this.fail(); return false; }
    if (add.length > 1) {
      for (const t of add) {
        if (!(await this.update([t], []))) return false;
      }
      if (remove.length) return this.update([], remove);
      return true;
    }
    if (add.length === 1) {
      this.refused.set(add[0], code);
      this.env.send(this.wanting(add[0]), { t: "refused", topic: add[0], code });
      return true;
    }
    // Only removals, refused: start over on a new stream.
    this.streamId = null;
    this.connect();
    return false;
  }
}
