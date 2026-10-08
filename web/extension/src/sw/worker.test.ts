// The worker's parts together (pairing, the API, the stream hub and the
// tabs) against a fake daemon that checks credentials the way the gateway
// does: a stream belongs to the credential that opened it, and only that
// credential can change or resume it.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PageView } from "../messages";
import { PAIR_SLOW_MS, REPAIR_MS } from "./pairing";
import { composerTab, createWorker, type Worker } from "./worker";

const AID = "7q3k9mzx2b4t";
const AID2 = "8r4m0nzy3c5v";
const T1 = "01J9AAAAAAAAAAAAAAAAAAAAAA";
const T2 = "01J9BBBBBBBBBBBBBBBBBBBBBB";
const URL1 = "http://localhost:5173/";
const URL2 = "http://localhost:5173/settings";
const enc = new TextEncoder();
const page = (aid: string, path: string): PageView => ({ artifact_id: aid, origin: "http://localhost:5173", path, page_url: `http://localhost:5173${path}`, title: "T", current_version: 1, url: `http://localhost:5173${path}` });
const full = (id: string) => ({ id, artifact_id: AID, status: "open", comments: [], feedback_state: null });
const json = (status: number, body: unknown) => new Response(JSON.stringify(body), { status });

type Stream = { id: string; cred: string; port: number; lastEventId: string | null; ended: boolean; push(t: string): void; end(): void };

class Daemon {
  port = 7480;
  live = new Set<string>();
  minted = 0;
  streams: Stream[] = [];
  held = new Map<string, Set<string>>();
  log: { method: string; path: string; cred: string | null; port: number }[] = [];
  threads: unknown[] = [];
  pages = new Map<string, PageView>([[URL1, page(AID, "/")], [URL2, page(AID2, "/settings")]]);
  /** Whether the daemon takes the extension as the owner. */
  owner = true;
  /** The native host answers once this settles; `hostMissing`: with an error. */
  hostGate: Promise<void> = Promise.resolve();
  hostMissing = false;

  pair() {
    const credential = `cxe_${String.fromCharCode(65 + this.minted++).repeat(43)}`;
    this.live.add(credential);
    return { type: "paired", v: 1, daemon: `http://localhost:${this.port}`, credential, clax_version: "0.9.0", viewer: { public_id: "u_owner", display_name: null } };
  }

  /** The credential is revoked; its streams end with it. */
  revoke(cred: string) {
    this.live.delete(cred);
    for (const s of this.streams) if (s.cred === cred && !s.ended) s.end();
  }

  /** The daemon restarted on another port: nothing it held survives. */
  restart(port: number) {
    for (const s of this.streams) if (!s.ended) s.end();
    this.port = port;
    this.live.clear();
    this.held.clear();
  }

  open(): Stream[] { return this.streams.filter(s => !s.ended); }

  fetch = (async (input: string, init: RequestInit = {}) => {
    if (init.signal?.aborted) throw new DOMException("aborted", "AbortError");
    const u = new URL(input);
    if (Number(u.port) !== this.port) throw new TypeError("Failed to fetch");
    const headers = new Headers(init.headers);
    const auth = headers.get("authorization");
    const cred = auth?.startsWith("Clax-Extension ") ? auth.slice(15) : null;
    const method = init.method ?? "GET";
    this.log.push({ method, path: u.pathname + u.search, cred, port: this.port });
    if (init.credentials !== "omit") throw new Error("a request that may carry cookies");
    if (!cred || !this.live.has(cred)) return json(401, { error: { code: "unknown_credential", message: "pair again" } });
    if (u.pathname === "/api/live/pages") {
      const p = this.pages.get(u.searchParams.get("url") ?? "") ?? null;
      return json(200, { page: p, route: null });
    }
    if (u.pathname.endsWith("/threads")) return json(200, { threads: this.threads });
    if (u.pathname.endsWith("/working")) return json(200, { working: [] });
    if (u.pathname === "/api/inbox/summary") return this.owner ? json(200, { unread: 1, questions: [], latest: [] }) : json(403, { error: { code: "forbidden", message: "Only the owner may do this" } });
    if (u.pathname === "/api/stream") {
      let ctrl!: ReadableStreamDefaultController<Uint8Array>;
      const body = new ReadableStream<Uint8Array>({ start(c) { ctrl = c; } });
      const lastEventId = headers.get("last-event-id");
      const prior = lastEventId ? this.streams.find(s => s.id === lastEventId.split(":")[0]) : undefined;
      // Only the opening credential resumes a stream.
      const resumed = !!prior && prior.cred === cred && prior.port === this.port;
      const s: Stream = {
        id: resumed ? prior!.id : `s${this.streams.length + 1}`, cred, port: this.port, lastEventId, ended: false,
        push: t => ctrl.enqueue(enc.encode(t)),
        end: () => { s.ended = true; try { ctrl.close(); } catch { /* closed */ } },
      };
      init.signal?.addEventListener("abort", () => { s.ended = true; try { ctrl.error(new DOMException("aborted", "AbortError")); } catch { /* closed */ } });
      this.streams.push(s);
      if (!resumed) this.held.set(s.id, new Set());
      queueMicrotask(() => s.push(`event: ready\ndata: ${JSON.stringify({ stream: s.id, seq: 0, resumed, topics: [...(this.held.get(s.id) ?? [])] })}\n\n`));
      return new Response(body, { status: 200, headers: { "content-type": "text/event-stream" } });
    }
    const sub = /^\/api\/stream\/(\w+)$/.exec(u.pathname);
    if (sub && method === "POST") {
      const owner = this.streams.find(s => s.id === sub[1] && !s.ended && s.port === this.port);
      if (!owner || owner.cred !== cred || !this.held.has(owner.id)) return json(404, { error: { code: "unknown_stream", message: "no such stream" } });
      const b = JSON.parse(String(init.body)) as { subscribe: string[]; unsubscribe: string[] };
      const held = this.held.get(owner.id)!;
      for (const t of b.unsubscribe) held.delete(t);
      for (const t of b.subscribe) held.add(t);
      return json(200, { seq: 0, topics: [...held] });
    }
    return json(404, { error: { code: "not_found", message: u.pathname } });
  }) as unknown as typeof fetch;
}

let d: Daemon;
let w: Worker;
let session: Record<string, unknown>;
let local: Record<string, unknown>;
const tick = () => vi.advanceTimersByTimeAsync(0);
const settle = async () => { for (let i = 0; i < 5; i++) await tick(); };
const subscribes = (cred: string) => d.log.filter(l => l.method === "POST" && l.path.startsWith("/api/stream/") && l.cred === cred);
const lookups = (cred: string) => d.log.filter(l => l.path.startsWith("/api/live/pages") && l.cred === cred);

beforeEach(() => {
  vi.useFakeTimers();
  d = new Daemon();
  session = {};
  local = {};
  w = boot();
});

const area = (m: Record<string, unknown>) => ({
  get: async (k: string) => (k in m ? { [k]: structuredClone(m[k]) } : {}),
  set: async (v: Record<string, unknown>) => { Object.assign(m, structuredClone(v)); },
  remove: async (k: string) => { delete m[k]; },
});
let pairings = 0;
const toOverlay: { tabId: number; m: unknown }[] = [];
const captures: unknown[][] = [];
const injected: number[] = [];
/** The tabs whose current document has the overlay. */
const docs = new Set<number>();
/** A worker as Chrome starts it, over the session and local storage that outlive it. */
const booted: Worker[] = [];
function boot(): Worker {
  const worker = createWorker({
    pair: { sendNative: async () => { pairings++; await d.hostGate; return d.hostMissing ? { type: "error", v: 1, code: "host_missing", message: "not found" } : d.pair(); }, session: area(session), local: area(local), manifestVersion: "0.9.0", reload: () => {}, now: () => Date.now() },
    fetch: d.fetch,
    toOverlay: (tabId, m) => toOverlay.push({ tabId, m }),
    capture: async (...a) => { captures.push(a); return { error: "no_capture_permission" }; },
    inject: async tabId => { if (docs.has(tabId)) return false; docs.add(tabId); injected.push(tabId); return true; },
    present: async tabId => docs.has(tabId),
    store: area(session),
  });
  booted.push(worker);
  return worker;
}
afterEach(async () => {
  // Requests still in flight may subscribe again once answered: let them settle, then close.
  await settle();
  for (const b of booted.splice(0)) b.hub.close();
  injected.length = 0;
  toOverlay.length = 0;
  captures.length = 0;
  docs.clear();
  vi.useRealTimers();
});

/** Tab 4 on URL1, its stream up and subscribed with the first credential. */
async function up(): Promise<string> {
  await w.tabs.route(4, URL1);
  await settle();
  const a = d.open()[0];
  expect(a).toBeDefined();
  expect(subscribes(a.cred)).toHaveLength(1);
  expect(d.held.get(a.id)).toEqual(new Set([`artifact:${AID}`, `working:${AID}`]));
  // Going live refetched the tab's state.
  expect(lookups(a.cred)).toHaveLength(2);
  return a.cred;
}

describe("the worker", () => {
  it("tells the tab when the native host is slow to pair, and goes on once its late answer comes", async () => {
    const pair = d.pair.bind(d);
    // The fake host answers late (sendNative awaits what it returns).
    d.pair = (() => new Promise(r => { setTimeout(() => r(pair()), PAIR_SLOW_MS + 5000); })) as unknown as typeof d.pair;
    try {
      w.tabs.turnOn(4, URL1, "http://localhost:5173");
      const routed = w.tabs.route(4, URL1);
      await vi.advanceTimersByTimeAsync(PAIR_SLOW_MS);
      expect(w.tabs.state(4)?.error).toMatchObject({ code: "host_slow" });
      expect(pairings).toBe(1);
      await vi.advanceTimersByTimeAsync(5000);
      await routed;
      await settle();
      expect(pairings).toBe(1);
      expect(w.tabs.state(4)).toMatchObject({ error: null, page: { artifact_id: AID } });
    } finally {
      d.pair = pair;
    }
  });

  it("checks the owner with the credential before the panels' inbox topics join the tabs' stream", async () => {
    const cred = await up();
    const port = { postMessage: () => {}, onDisconnect: { addListener: () => {} } };
    w.inbox.attach(port as never);
    await settle();
    const summary = d.log.filter(l => l.path === "/api/inbox/summary");
    expect(summary).toHaveLength(2);
    expect(summary.every(l => l.cred === cred)).toBe(true);
    expect(d.held.get(d.open()[0].id)).toEqual(new Set([`artifact:${AID}`, `working:${AID}`, "questions", "inbox"]));
    // Checked, then subscribed, then fetched again once the topics went live.
    const at = (l: (typeof d.log)[number]) => d.log.indexOf(l);
    const subscribe = d.log.filter(l => l.method === "POST" && l.path.startsWith("/api/stream/")).at(-1)!;
    expect(at(summary[0])).toBeLessThan(at(subscribe));
    expect(at(subscribe)).toBeLessThan(at(summary[1]));
  });

  it("joins a pairing under way when a panel opens mid-pairing: one native host, then the page, the summary and the topics", async () => {
    let open!: () => void;
    d.hostGate = new Promise(r => { open = r; });
    const before = pairings;
    void w.tabs.route(4, URL1);
    await settle();
    const port = { postMessage: () => {}, onDisconnect: { addListener: () => {} } };
    w.inbox.attach(port as never);
    await settle();
    expect(pairings - before).toBe(1);
    open();
    await settle();
    expect(pairings - before).toBe(1);
    expect(w.tabs.state(4)?.page?.artifact_id).toBe(AID);
    expect(d.log.some(l => l.path === "/api/inbox/summary")).toBe(true);
    expect(d.held.get(d.open()[0].id)).toEqual(new Set([`artifact:${AID}`, `working:${AID}`, "questions", "inbox"]));
  });

  it("starts no native host for the inbox while the host is missing, however long a panel stays open", async () => {
    d.hostMissing = true;
    const before = pairings;
    await w.tabs.route(4, URL1).catch(() => {});
    await settle();
    expect(pairings - before).toBe(1);
    const port = { postMessage: () => {}, onDisconnect: { addListener: () => {} } };
    w.inbox.attach(port as never);
    await vi.advanceTimersByTimeAsync(5 * 60_000);
    expect(pairings - before).toBe(1);
    expect(w.inbox.owner).toBeNull();
  });

  it("subscribes no inbox topic when the daemon does not take the extension as the owner, so the tabs' stream is untouched", async () => {
    d.owner = false;
    await up();
    const port = { postMessage: () => {}, onDisconnect: { addListener: () => {} } };
    w.inbox.attach(port as never);
    await settle();
    expect(w.inbox.owner).toBe(false);
    expect(d.held.get(d.open()[0].id)).toEqual(new Set([`artifact:${AID}`, `working:${AID}`]));
  });

  it("opens a new stream and refetches after a request pairs again, leaving the old stream", async () => {
    const A = await up();
    const oldStream = d.open()[0];
    await vi.advanceTimersByTimeAsync(REPAIR_MS);
    // The credential lapses for requests (the old stream stays open on the daemon's side).
    d.live.delete(A);
    d.threads = [full(T1)];
    await w.tabs.route(4, URL1);
    await settle();
    const B = d.open().at(-1)!.cred;
    expect(B).not.toBe(A);
    expect(oldStream.ended).toBe(true);
    expect(d.open()).toHaveLength(1);
    // The new stream is fresh: no Last-Event-ID of the old one, which it could not resume.
    expect(d.open()[0].lastEventId).toBeNull();
    expect(subscribes(B)).toHaveLength(1);
    expect(subscribes(B)[0].path).toBe(`/api/stream/${d.open()[0].id}`);
    expect(d.held.get(d.open()[0].id)).toEqual(new Set([`artifact:${AID}`, `working:${AID}`]));
    // Its topics going live refetched the tab's state with the new credential.
    expect(lookups(B).length).toBeGreaterThanOrEqual(2);
    expect(w.tabs.state(4)?.threads.map(t => t.id)).toEqual([T1]);
  });

  it("pairs again when the daemon ends a revoked credential's stream, and refetches on the new stream", async () => {
    const A = await up();
    await vi.advanceTimersByTimeAsync(REPAIR_MS);
    d.threads = [full(T2)];
    d.revoke(A);
    // The hub retries after its backoff; the resume is refused (401), the worker pairs again.
    await vi.advanceTimersByTimeAsync(1000);
    await settle();
    const open = d.open();
    expect(open).toHaveLength(1);
    const B = open[0].cred;
    expect(B).not.toBe(A);
    expect(subscribes(B)).toHaveLength(1);
    expect(subscribes(B)[0].path).toBe(`/api/stream/${open[0].id}`);
    expect(w.tabs.state(4)?.threads.map(t => t.id)).toEqual([T2]);
  });

  it("follows the daemon to its new port after a restart", async () => {
    await up();
    await vi.advanceTimersByTimeAsync(REPAIR_MS);
    d.threads = [full(T1)];
    d.restart(7490);
    await vi.advanceTimersByTimeAsync(1000);
    await settle();
    const open = d.open();
    expect(open).toHaveLength(1);
    expect(open[0].port).toBe(7490);
    expect(subscribes(open[0].cred)).toHaveLength(1);
    expect(w.tabs.state(4)?.threads.map(t => t.id)).toEqual([T1]);
    expect((await w.pairer.current()).daemon).toBe("http://localhost:7490");
  });

  it("replaces a stream the daemon answers 404 unknown_stream for, and refetches", async () => {
    const A = await up();
    const first = d.open()[0];
    // The daemon no longer holds the stream for this caller.
    d.held.delete(first.id);
    await w.tabs.route(4, URL2);
    await settle();
    const now = d.open();
    expect(now).toHaveLength(1);
    expect(now[0].id).not.toBe(first.id);
    expect(now[0].cred).toBe(A);
    expect(d.held.get(now[0].id)).toEqual(new Set([`artifact:${AID2}`, `working:${AID2}`]));
    expect(w.tabs.state(4)?.page?.artifact_id).toBe(AID2);
    expect(lookups(A).filter(l => l.path.includes("settings")).length).toBeGreaterThanOrEqual(2);
  });

  it("keeps following a tab's topics past the hub's client TTL", async () => {
    await up();
    const stream = d.open()[0];
    // The daemon's keep-alive every 15 s, for well past CLIENT_TTL_MS (180 s).
    for (let t = 0; t < 240_000; t += 15_000) {
      stream.push(": keep-alive\n\n");
      await vi.advanceTimersByTimeAsync(15_000);
    }
    expect(w.hub.stats()).toMatchObject({ clients: 1, topics: [`artifact:${AID}`, `working:${AID}`], up: true });
    expect(d.held.get(stream.id)).toEqual(new Set([`artifact:${AID}`, `working:${AID}`]));
    stream.push(`event: thread\ndata: ${JSON.stringify({ topic: `artifact:${AID}`, artifact_id: AID, thread: { ...full(T1), comment_count: 0, last_comment: null } })}\nid: ${stream.id}:1\n\n`);
    await settle();
    expect(w.tabs.state(4)?.threads.map(t => t.id)).toEqual([T1]);
  });

  it("rebuilds after Chrome restarts the worker: the stored pairing, the tab, a new stream and its state", async () => {
    await w.tabs.toggle(4, URL1);
    await settle();
    await settle();
    expect(injected).toEqual([4]);
    expect(d.open()).toHaveLength(1);
    const before = pairings;
    // Chrome stops the worker: its memory goes, storage stays; the stream drops.
    w.hub.close();
    for (const s of d.open()) s.end();
    d.threads = [full(T1)];
    w = boot();
    await w.tabs.ready();
    // The overlay's next ping starts it again.
    await w.tabs.fromOverlay(4, 1, { t: "ping" }, URL1);
    await settle();
    expect(pairings).toBe(before);
    expect(w.tabs.state(4)).toMatchObject({ overlay: true, commentMode: true, page: { artifact_id: AID } });
    expect(w.tabs.state(4)?.threads.map(t => t.id)).toEqual([T1]);
    const open = d.open();
    expect(open).toHaveLength(1);
    expect(d.held.get(open[0].id)).toEqual(new Set([`artifact:${AID}`, `working:${AID}`]));
    // The overlay already in the page is not injected again.
    await w.tabs.toggle(4, URL1);
    expect(injected).toEqual([4]);
  });
});

describe("the pick flow in the worker", () => {
  const PICK = "a".repeat(32);
  const ANCHOR = { kind: "element", selector: "#save", quote: "Save", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" } as const;
  const sender = (url: string, tab: number | undefined, frameId = 3, id = "ext") =>
    ({ name: `composer:${PICK}`, sender: { id, url, frameId, tab: tab === undefined ? undefined : { id: tab } } }) as unknown as chrome.runtime.Port;

  it("takes a composer port only from the extension's composer page framed in a tab", () => {
    expect(composerTab(sender("chrome-extension://ext/composer.html#" + PICK, 5), "ext")).toBe(5);
    // A dynamic URL (use_dynamic_url) has another host; the sender's ID is still the extension's.
    expect(composerTab(sender("chrome-extension://0f1e2d3c/composer.html#" + PICK, 5), "ext")).toBe(5);
    expect(composerTab(sender("chrome-extension://ext/composer.html", 5, 3, "other"), "ext")).toBeNull();
    expect(composerTab(sender("chrome-extension://ext/sidepanel.html", 5), "ext")).toBeNull();
    expect(composerTab(sender("http://localhost:5173/composer.html", 5), "ext")).toBeNull();
    expect(composerTab(sender("chrome-extension://ext/composer.html", undefined), "ext")).toBeNull();
    expect(composerTab(sender("chrome-extension://ext/composer.html", 5, 0), "ext")).toBeNull();
    expect(composerTab({ ...sender("chrome-extension://ext/composer.html", 5), name: "panel:1" } as chrome.runtime.Port, "ext")).toBeNull();
  });

  it("sends capture, pick, quiet and cancel to the picks, and the rest to the tabs", async () => {
    const r = await w.fromOverlay(4, 9, { t: "capture", pickId: PICK, anchor: ANCHOR, rect: { x: 1, y: 2, w: 3, h: 4 }, dpr: 2 }, URL1);
    expect(r).toEqual({ t: "captured", pickId: PICK, ok: false, error: "no_capture_permission" });
    expect(captures).toEqual([[9, { x: 1, y: 2, w: 3, h: 4 }, 2]]);
    expect(toOverlay.at(-1)).toEqual({ tabId: 4, m: { t: "open-composer", pickId: PICK, rect: { x: 1, y: 2, w: 3, h: 4 } } });
    await w.fromOverlay(4, 9, { t: "cancel", pickId: PICK }, URL1);
    expect(toOverlay.at(-1)).toEqual({ tabId: 4, m: { t: "close-composer", pickId: PICK, posted: false } });
    await w.fromOverlay(4, 9, { t: "comment-mode", on: true }, URL1);
    expect(w.tabs.state(4)?.commentMode).toBe(true);
  });
});
