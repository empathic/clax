// The `room` capability (contract 0.2.61 room.d.ts), page side, in the
// bridge's lazy `room` part (parts/room.ts). The shell owns
// the WebSocket (web/shell/src/caps/room.ts) and relays the daemon's frames as
// `clax:event`s of ns "room"; this module keeps each room's peers as frozen
// snapshots, applies and coalesces this page's presence, batches onPeers per
// animation frame, checks every argument, and turns terminal errors into one
// onError per listener.
import { CapabilityError, type Rpc } from "../rpc";

export type WirePeer = {
  peer: string; by: string | null; isMe: boolean; sameTab: boolean;
  kind: "viewer" | "agent"; guest: boolean; presence: Record<string, unknown>;
};
export type Peer = Readonly<Omit<WirePeer, "presence"> & { presence: Readonly<Record<string, unknown>>; updatedAt: number }>;
export type PeersChange = Readonly<{ peers: readonly Peer[]; joined: readonly Peer[]; left: readonly Peer[]; updated: readonly Peer[] }>;
type WireMsg = Omit<WirePeer, "presence"> & { topic: string; data?: unknown };
type ErrorInfo = { code: string; message: string };
type OnError = (e: ErrorInfo) => void;

export const TOPIC = /^[a-z][a-z0-9_.-]{0,47}$/;
export const ROOM_NAME = /^[a-z0-9][a-z0-9_.-]{0,47}$/;
const KEY = /^[A-Za-z_][A-Za-z0-9_-]{0,63}$/;
export const MAX_JSON_BYTES = 4096;
export const MAX_DEPTH = 8;
export const MAX_JOINED = 16;
/** Presence is sent at most about 30 times a second. */
export const PRESENCE_INTERVAL_MS = 33;
/** onPeers waits for the next animation frame, or this long where frames do not run. */
export const FLUSH_FALLBACK_MS = 50;
export const JOIN_TIMEOUT_MS = 10_000;

const EMPTY: readonly Peer[] = Object.freeze([]);
const reject = (code: string, message: string) => Promise.reject(new CapabilityError(code, message));
const safe = (fn: () => void) => { try { fn(); } catch (e) { reportError(e); } };

function isPlainObject(v: unknown): v is Record<string, unknown> {
  if (v === null || typeof v !== "object" || Array.isArray(v)) return false;
  const p = Object.getPrototypeOf(v);
  return p === Object.prototype || p === null;
}

function plain(v: unknown, depth: number): boolean {
  if (depth > MAX_DEPTH) return false;
  if (v === null || typeof v === "string" || typeof v === "boolean") return true;
  if (typeof v === "number") return Number.isFinite(v);
  if (Array.isArray(v)) return v.every(x => plain(x, depth + 1));
  return isPlainObject(v) && Object.values(v).every(x => plain(x, depth + 1));
}

/** Why `v` cannot travel as room data (named `what`), or null when it can. */
function jsonProblem(what: string, v: unknown): string | null {
  if (!plain(v, 1)) return `${what} is not plain JSON, or nests deeper than ${MAX_DEPTH} levels`;
  const bytes = new TextEncoder().encode(JSON.stringify(v)).length;
  return bytes > MAX_JSON_BYTES ? `${what} is ${bytes} bytes of JSON; the limit is ${MAX_JSON_BYTES}` : null;
}

function freezePeer(w: WirePeer, updatedAt: number): Peer {
  const presence = Object.freeze(JSON.parse(JSON.stringify(w.presence ?? {})) as Record<string, unknown>);
  return Object.freeze({ peer: w.peer, by: w.by ?? null, isMe: !!w.isMe, sameTab: !!w.sameTab, kind: w.kind === "agent" ? "agent" : "viewer", guest: !!w.guest, presence, updatedAt });
}

type PeerSub = { fn: (c: PeersChange) => void; onError?: OnError; primed: boolean };
type TopicSub = { topic: string; fn: (m: Readonly<WireMsg>) => void; onError?: OnError };
type ConnSub = { fn: (c: boolean) => void; onError?: OnError };

/** Shared by the lobby and every named room. */
interface Runtime {
  rpc: Pick<Rpc, "call" | "on">;
  connected: boolean;
  me: string | null;
  reportDrop(): void;
}

/** One room (the lobby, or a named room) as this page sees it. */
class Scope {
  private readonly peers = new Map<string, Peer>();
  private snapshot: readonly Peer[] = EMPTY;
  private answered = false;
  private mine: Record<string, unknown> = {};
  private readonly topicSubs = new Set<TopicSub>();
  private readonly peerSubs = new Set<PeerSub>();
  readonly connSubs = new Set<ConnSub>();
  private readonly changes = new Map<string, "joined" | "updated" | "left">();
  private readonly gone = new Map<string, Peer>();
  private flushQueued = false;
  /** The peers changed since `snapshot` was built. */
  private dirty = false;
  private sendTimer: ReturnType<typeof setTimeout> | null = null;
  private lastSent = 0;
  dead: ErrorInfo | null = null;

  constructor(readonly name: string | null, private readonly rt: Runtime) {}

  // ---- frames from the shell ----

  onPeersFrame(list: WirePeer[]): void {
    const seen = new Set<string>();
    for (const w of list) { seen.add(w.peer); this.upsert(w); }
    for (const label of [...this.peers.keys()]) if (!seen.has(label)) this.remove(label);
    this.answered = true;
    this.commit();
  }

  onPeerFrame(w: WirePeer): void { this.upsert(w); this.commit(); }

  onLeftFrame(label: string): void { this.remove(label); this.commit(); }

  onMsgFrame(m: WireMsg): void {
    if (this.dead) return;
    const msg = Object.freeze({ peer: m.peer, by: m.by ?? null, isMe: !!m.isMe, sameTab: !!m.sameTab, kind: m.kind === "agent" ? "agent" : "viewer", guest: !!m.guest, topic: m.topic, ...(m.data === undefined ? {} : { data: m.data }) }) as Readonly<WireMsg>;
    for (const sub of [...this.topicSubs]) if (sub.topic === m.topic && this.topicSubs.has(sub)) safe(() => sub.fn(msg));
  }

  private upsert(w: WirePeer): void {
    if (w.sameTab) w = { ...w, presence: this.mine };
    const prev = this.peers.get(w.peer);
    const samePresence = !!prev && JSON.stringify(prev.presence) === JSON.stringify(w.presence ?? {});
    if (prev && samePresence && prev.by === (w.by ?? null) && prev.isMe === !!w.isMe) return;
    this.peers.set(w.peer, freezePeer(w, prev && samePresence ? prev.updatedAt : Date.now()));
    this.dirty = true;
    this.note(w.peer, prev ? "updated" : "joined");
  }

  private remove(label: string): void {
    const prev = this.peers.get(label);
    if (!prev) return;
    this.peers.delete(label);
    this.dirty = true;
    this.note(label, "left", prev);
  }

  private note(label: string, kind: "joined" | "updated" | "left", prev?: Peer): void {
    const before = this.changes.get(label);
    if (kind === "left") {
      if (before === "joined") { this.changes.delete(label); return; }
      this.changes.set(label, "left");
      if (prev && !this.gone.has(label)) this.gone.set(label, prev);
      return;
    }
    if (before === "left") { this.changes.set(label, "updated"); this.gone.delete(label); return; }
    if (before === "joined") return;
    this.changes.set(label, kind);
  }

  private commit(): void {
    if (this.dirty) {
      this.snapshot = Object.freeze([...this.peers.values()]);
      this.dirty = false;
    }
    this.schedule();
  }

  private schedule(): void {
    if (this.flushQueued || !this.peerSubs.size) return;
    this.flushQueued = true;
    const run = () => { if (!this.flushQueued) return; this.flushQueued = false; this.flush(); };
    if (typeof requestAnimationFrame === "function") requestAnimationFrame(run);
    setTimeout(run, FLUSH_FALLBACK_MS);
  }

  private flush(): void {
    const joined: Peer[] = [], updated: Peer[] = [], left: Peer[] = [];
    for (const [label, kind] of this.changes) {
      if (kind === "left") { const p = this.gone.get(label); if (p) left.push(p); continue; }
      const p = this.peers.get(label);
      if (p) (kind === "joined" ? joined : updated).push(p);
    }
    this.changes.clear();
    this.gone.clear();
    const none: readonly Peer[] = EMPTY;
    for (const sub of [...this.peerSubs]) {
      if (!this.peerSubs.has(sub) || this.dead) continue;
      if (!sub.primed) {
        if (!this.answered) continue;
        sub.primed = true;
        safe(() => sub.fn(Object.freeze({ peers: this.snapshot, joined: this.snapshot, left: none, updated: none })));
      } else if (joined.length || updated.length || left.length) {
        safe(() => sub.fn(Object.freeze({ peers: this.snapshot, joined: Object.freeze(joined), left: Object.freeze(left), updated: Object.freeze(updated) })));
      }
    }
  }

  // ---- the page's calls ----

  peersNow(): readonly Peer[] {
    if (!this.dead) return this.snapshot;
    return Object.freeze(this.snapshot.filter(p => p.sameTab));
  }

  onPeers(fn: unknown, onError?: OnError): () => void {
    if (typeof fn !== "function") throw new TypeError("onPeers takes a function");
    if (this.dead) { const d = this.dead; queueMicrotask(() => onError?.(d)); return () => {}; }
    const sub: PeerSub = { fn: fn as PeerSub["fn"], onError, primed: false };
    this.peerSubs.add(sub);
    this.schedule();
    return () => { this.peerSubs.delete(sub); };
  }

  on(topic: unknown, fn: unknown, onError?: OnError): () => void {
    if (typeof fn !== "function") throw new TypeError("on takes a handler function");
    if (typeof topic !== "string" || !TOPIC.test(topic)) {
      const e = { code: "invalid_argument", message: `'${String(topic)}' is not a topic (^[a-z][a-z0-9_.-]{0,47}$)` };
      queueMicrotask(() => onError?.(e));
      return () => {};
    }
    if (this.dead) { const d = this.dead; queueMicrotask(() => onError?.(d)); return () => {}; }
    const sub: TopicSub = { topic, fn: fn as TopicSub["fn"], onError };
    this.topicSubs.add(sub);
    return () => { this.topicSubs.delete(sub); };
  }

  onConnection(fn: unknown, onError?: OnError): () => void {
    if (typeof fn !== "function") throw new TypeError("onConnection takes a function");
    if (this.dead) { const d = this.dead; queueMicrotask(() => onError?.(d)); return () => {}; }
    const sub: ConnSub = { fn: fn as ConnSub["fn"], onError };
    this.connSubs.add(sub);
    queueMicrotask(() => { if (this.connSubs.has(sub)) safe(() => sub.fn(this.rt.connected && !this.dead)); });
    return () => { this.connSubs.delete(sub); };
  }

  connectionChanged(c: boolean): void {
    for (const sub of [...this.connSubs]) safe(() => sub.fn(c));
  }

  presence(patch: unknown): Promise<void> {
    if (this.dead) return reject(this.dead.code, this.dead.message);
    if (!isPlainObject(patch)) return reject("invalid_argument", "presence takes a plain object of fields");
    const merged: Record<string, unknown> = { ...this.mine };
    for (const [k, v] of Object.entries(patch)) {
      if (!KEY.test(k) || k in Object.prototype || k === "prototype") return reject("invalid_argument", `presence key '${k}' is not an identifier`);
      if (v === null) delete merged[k]; else merged[k] = v;
    }
    const problem = jsonProblem("presence", merged);
    if (problem) return reject("invalid_argument", problem);
    this.mine = JSON.parse(JSON.stringify(merged)) as Record<string, unknown>;
    const me = this.rt.me && this.peers.get(this.rt.me);
    if (me) {
      this.peers.set(me.peer, freezePeer({ ...me, presence: this.mine }, Date.now()));
      this.dirty = true;
      this.note(me.peer, "updated");
      this.commit();
    }
    this.queueSend();
    return Promise.resolve();
  }

  private queueSend(): void {
    if (this.sendTimer) return;
    const wait = Math.max(0, this.lastSent + PRESENCE_INTERVAL_MS - Date.now());
    this.sendTimer = setTimeout(() => {
      this.sendTimer = null;
      if (this.dead) return;
      this.lastSent = Date.now();
      void this.rt.rpc.call("room", "presence", [this.name, this.mine]).catch(() => {});
    }, wait);
  }

  emit(topic: unknown, data?: unknown): Promise<void> {
    if (this.dead) return reject(this.dead.code, this.dead.message);
    if (typeof topic !== "string" || !TOPIC.test(topic)) return reject("invalid_argument", `'${String(topic)}' is not a topic (^[a-z][a-z0-9_.-]{0,47}$)`);
    if (data !== undefined) { const p = jsonProblem("data", data); if (p) return reject("invalid_argument", p); }
    if (!this.rt.connected) return Promise.resolve();
    const args = data === undefined ? [this.name, topic] : [this.name, topic, data];
    return this.rt.rpc.call("room", "emit", args).then(r => {
      if ((r as { dropped?: boolean } | null)?.dropped) this.rt.reportDrop();
    });
  }

  /** Ends this scope: listeners hear `e` once (unless `silent`), calls reject with it. */
  die(e: ErrorInfo, silent = false): void {
    if (this.dead) return;
    this.dead = e;
    if (this.sendTimer) { clearTimeout(this.sendTimer); this.sendTimer = null; }
    const subs = [...this.topicSubs, ...this.peerSubs, ...this.connSubs];
    this.topicSubs.clear(); this.peerSubs.clear(); this.connSubs.clear();
    if (!silent) for (const s of subs) if (s.onError) safe(() => s.onError!(e));
  }
}

type Named = { scope: Scope; api: Readonly<Record<string, unknown>>; ready: Promise<Readonly<Record<string, unknown>>> };

/** The `room` namespace members; the shell is asked to connect at once. */
export function makeRoom(rpc: Pick<Rpc, "call" | "on">): Record<string, (...args: never[]) => unknown> {
  let dropReported = false;
  const rt: Runtime = {
    rpc, connected: false, me: null,
    reportDrop() {
      if (dropReported) return;
      dropReported = true;
      reportError(new Error("room: emits past about 40 a second were dropped"));
    },
  };
  const lobby = new Scope(null, rt);
  const named = new Map<string, Named>();
  const scope = (room: unknown): Scope | undefined => (room === null || room === undefined ? lobby : named.get(String(room))?.scope);

  rpc.on("room", "connection", d => {
    const c = !!(d as { connected: boolean }).connected;
    if (c === rt.connected || lobby.dead) return;
    rt.connected = c;
    lobby.connectionChanged(c);
    for (const n of named.values()) if (!n.scope.dead) n.scope.connectionChanged(c);
  });
  rpc.on("room", "welcome", d => { rt.me = String((d as { peer: string }).peer); });
  rpc.on("room", "peers", d => { const f = d as { room: string | null; peers: WirePeer[] }; scope(f.room)?.onPeersFrame(f.peers); });
  rpc.on("room", "peer", d => { const f = d as { room: string | null; peer: WirePeer }; scope(f.room)?.onPeerFrame(f.peer); });
  rpc.on("room", "left", d => { const f = d as { room: string | null; peer: string }; scope(f.room)?.onLeftFrame(f.peer); });
  rpc.on("room", "msg", d => { const f = d as { room: string | null; msg: WireMsg }; scope(f.room)?.onMsgFrame(f.msg); });
  rpc.on("room", "error", d => {
    const f = d as { room: string | null; code: string; message: string };
    const e = { code: String(f.code), message: String(f.message) };
    if (f.room === null) {
      rt.connected = false;
      lobby.die(e);
      for (const n of named.values()) n.scope.die(e);
      named.clear();
      return;
    }
    const n = named.get(f.room);
    if (n) { named.delete(f.room); n.scope.die(e); }
  });
  void rpc.call("room", "connect", []).catch(() => {});

  function makeNamed(s: Scope, name: string): Readonly<Record<string, unknown>> {
    let left = false;
    return Object.freeze({
      name,
      emit: (topic: unknown, data?: unknown) => s.emit(topic, data),
      on: (topic: unknown, fn: unknown, onError?: OnError) => s.on(topic, fn, onError),
      presence: (patch: unknown) => s.presence(patch),
      peers: () => s.peersNow(),
      onPeers: (fn: unknown, onError?: OnError) => s.onPeers(fn, onError),
      connected: () => rt.connected && !s.dead,
      onConnection: (fn: unknown, onError?: OnError) => s.onConnection(fn, onError),
      leave: () => {
        if (left) return Promise.resolve();
        left = true;
        if (named.get(name)?.scope === s) named.delete(name);
        s.die({ code: "invalid_argument", message: `this page left room '${name}'` }, true);
        void rpc.call("room", "leave", [name]).catch(() => {});
        return Promise.resolve();
      },
    });
  }

  function join(name: unknown): Promise<Readonly<Record<string, unknown>>> {
    if (lobby.dead) return reject(lobby.dead.code, lobby.dead.message);
    if (typeof name !== "string" || !ROOM_NAME.test(name)) return reject("invalid_argument", `'${String(name)}' is not a room name (^[a-z0-9][a-z0-9_.-]{0,47}$)`);
    const have = named.get(name);
    if (have) return have.ready;
    if (named.size >= MAX_JOINED) return reject("limit_reached", `a page may be in at most ${MAX_JOINED} named rooms; leave one first`);
    const s = new Scope(name, rt);
    const api = makeNamed(s, name);
    let timer: ReturnType<typeof setTimeout> | undefined;
    const timeout = new Promise<never>((_, no) => {
      timer = setTimeout(() => no(new CapabilityError("upstream_error", `joining '${name}' got no answer within 10 s`)), JOIN_TIMEOUT_MS);
    });
    const answered = Promise.race([rpc.call("room", "join", [name]), timeout]).finally(() => clearTimeout(timer));
    const ready = answered.then(() => api, e => {
      if (named.get(name)?.scope === s) named.delete(name);
      s.die({ code: "upstream_error", message: "join failed" }, true);
      throw e;
    });
    named.set(name, { scope: s, api, ready });
    return ready;
  }

  return {
    emit: (topic: unknown, data?: unknown) => lobby.emit(topic, data),
    on: (topic: unknown, fn: unknown, onError?: OnError) => lobby.on(topic, fn, onError),
    presence: (patch: unknown) => lobby.presence(patch),
    peers: () => lobby.peersNow(),
    onPeers: (fn: unknown, onError?: OnError) => lobby.onPeers(fn, onError),
    connected: () => rt.connected && !lobby.dead,
    onConnection: (fn: unknown, onError?: OnError) => lobby.onConnection(fn, onError),
    join,
    canSendToClaudeSession: () => Promise.resolve("off"),
    sendToClaudeSession: () => reject("claude_unavailable", "there is no Claude conversation beside this page in Clax"),
  } as unknown as Record<string, (...args: never[]) => unknown>;
}

/** The members of the `room` namespace (checked against room.d.ts in
 * bridge/test/capabilities.test.ts). */
export const ROOM_METHODS = ["emit", "on", "presence", "peers", "onPeers", "sendToClaudeSession", "canSendToClaudeSession", "join", "connected", "onConnection"] as const;

/** The frozen `room` namespace `claude.use("room")` resolves. */
export function roomNamespace(rpc: Pick<Rpc, "call" | "on">): Readonly<Record<string, unknown>> {
  const m = makeRoom(rpc);
  return Object.freeze(Object.fromEntries(ROOM_METHODS.map(k => [k, m[k]])));
}
