// The worker's parts together (pairing, the API, the stream hub and the
// tabs) against a fake daemon that checks credentials the way the gateway
// does: a stream belongs to the credential that opened it, and only that
// credential can change or resume it.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PageView } from "../messages";
import { REPAIR_MS } from "./pairing";
import { createWorker, type Worker } from "./worker";

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
const tick = () => vi.advanceTimersByTimeAsync(0);
const settle = async () => { for (let i = 0; i < 5; i++) await tick(); };
const subscribes = (cred: string) => d.log.filter(l => l.method === "POST" && l.path.startsWith("/api/stream/") && l.cred === cred);
const lookups = (cred: string) => d.log.filter(l => l.path.startsWith("/api/live/pages") && l.cred === cred);

beforeEach(() => {
  vi.useFakeTimers();
  d = new Daemon();
  const session: Record<string, unknown> = {};
  const local: Record<string, unknown> = {};
  const area = (m: Record<string, unknown>) => ({
    get: async (k: string) => (k in m ? { [k]: m[k] } : {}),
    set: async (v: Record<string, unknown>) => { Object.assign(m, v); },
    remove: async (k: string) => { delete m[k]; },
  });
  w = createWorker({
    pair: { sendNative: async () => d.pair(), session: area(session), local: area(local), manifestVersion: "0.9.0", reload: () => {}, now: () => Date.now() },
    fetch: d.fetch,
    toOverlay: () => {},
    inject: async () => {},
  });
});
afterEach(() => { w.hub.close(); vi.useRealTimers(); });

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
});
