import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CONNECT_MS, Hub, type HubMsg, LINGER_MS } from "./stream-hub";
import { Net } from "./test/fake-net";

const A = "7q3k9mzx2b4t";
const B = "8r4m0nzy3c5v";
const S1 = "0123456789abcdef0123456789abcdef";
const S2 = "fedcba9876543210fedcba9876543210";
const tick = () => vi.advanceTimersByTimeAsync(0);

let net: Net;
let got: Map<string, HubMsg[]>;
let hub: Hub;
const of = (id: string, t?: HubMsg["t"]) => (got.get(id) ?? []).filter(m => !t || m.t === t);

beforeEach(() => {
  vi.useFakeTimers();
  net = new Net();
  got = new Map();
  hub = new Hub({ fetch: net.fetch, send: (ids, msg) => { for (const id of ids) got.set(id, [...(got.get(id) ?? []), msg]); } });
});
afterEach(() => { hub.close(); vi.useRealTimers(); });

describe("the stream hub", () => {
  it("holds one connection for every tab and subscribes the union of their topics", async () => {
    hub.receive("t1", { t: "topics", topics: ["gallery", `artifact:${A}`] });
    hub.receive("t2", { t: "topics", topics: [`artifact:${A}`] });
    hub.receive("t3", { t: "topics", topics: [`artifact:${B}`, `presence:${B}`] });
    await tick();
    expect(net.conns).toHaveLength(1);
    net.ready(net.conns[0], S1);
    await tick();
    expect(net.posts).toHaveLength(1);
    expect(net.posts[0].url).toBe(`/api/stream/${S1}`);
    expect(new Set(net.posts[0].body.subscribe)).toEqual(new Set(["gallery", `artifact:${A}`, `artifact:${B}`, `presence:${B}`]));
    // Each tab hears its own topics go live.
    expect(of("t1", "live")).toEqual([{ t: "live", topics: expect.arrayContaining(["gallery", `artifact:${A}`]) }]);
    expect(of("t2", "live")).toEqual([{ t: "live", topics: [`artifact:${A}`] }]);
    expect(of("t3", "live")[0]).toMatchObject({ topics: expect.arrayContaining([`artifact:${B}`, `presence:${B}`]) });
  });

  it("routes each event to the tabs that want its topic, and no other", async () => {
    hub.receive("t1", { t: "topics", topics: ["gallery"] });
    hub.receive("t2", { t: "topics", topics: [`artifact:${A}`] });
    await tick();
    net.ready(net.conns[0], S1);
    await tick();
    net.event(net.conns[0], S1, 1, `artifact:${A}`, "version", { artifact_id: A, n: 2 });
    net.event(net.conns[0], S1, 2, "gallery", "version", { artifact_id: A, n: 2 });
    net.conns[0].push(`event: resync\ndata: ${JSON.stringify({ topic: "gallery", reason: "behind" })}\n\n`);
    await tick();
    expect(of("t1", "event").map(m => m.t === "event" && m.topic)).toEqual(["gallery"]);
    expect(of("t2", "event")).toEqual([{ t: "event", topic: `artifact:${A}`, name: "version", data: { topic: `artifact:${A}`, artifact_id: A, n: 2 } }]);
    expect(of("t1", "resync")).toEqual([{ t: "resync", topic: "gallery" }]);
    expect(of("t2", "resync")).toEqual([]);
  });

  it("counts references: a topic goes when its last tab leaves, and the connection a moment after the last topic", async () => {
    hub.receive("t1", { t: "topics", topics: [`artifact:${A}`, "gallery"] });
    hub.receive("t2", { t: "topics", topics: [`artifact:${A}`] });
    await tick();
    net.ready(net.conns[0], S1);
    await tick();
    hub.receive("t2", { t: "topics", topics: [] });
    await tick();
    expect(net.posts).toHaveLength(1);
    hub.receive("t1", { t: "topics", topics: ["gallery"] });
    await tick();
    expect(net.posts.at(-1)!.body).toEqual({ subscribe: [], unsubscribe: [`artifact:${A}`] });
    // A tab joining a topic the stream carries is live at once, with no request.
    hub.receive("t3", { t: "topics", topics: ["gallery"] });
    await tick();
    expect(net.posts).toHaveLength(2);
    expect(of("t3", "live")).toEqual([{ t: "live", topics: ["gallery"] }]);
    hub.detach("t1");
    hub.receive("t3", { t: "bye" });
    await tick();
    expect(net.open).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(LINGER_MS);
    expect(net.open).toHaveLength(0);
    // The tab with no topics left is still there, holding nothing.
    expect(hub.stats()).toMatchObject({ clients: 1, topics: [], connected: false });
  });

  it("resumes after a drop with Last-Event-ID, telling the tabs it was down, without refetching", async () => {
    hub.receive("t1", { t: "topics", topics: [`artifact:${A}`] });
    await tick();
    net.ready(net.conns[0], S1);
    await tick();
    net.event(net.conns[0], S1, 7, `artifact:${A}`, "version", { artifact_id: A, n: 2 });
    await tick();
    // The daemon goes away: the body ends.
    net.conns[0].end();
    await vi.advanceTimersByTimeAsync(1000);
    expect(of("t1", "status")).toEqual([{ t: "status", up: false }]);
    const again = net.open.at(-1)!;
    expect(again.headers["Last-Event-ID"]).toBe(`${S1}:7`);
    net.ready(again, S1, true, [`artifact:${A}`]);
    await tick();
    expect(of("t1", "status")).toEqual([{ t: "status", up: false }, { t: "status", up: true }]);
    expect(of("t1", "live")).toHaveLength(1);
  });

  it("refetches everywhere when the daemon could not resume", async () => {
    hub.receive("t1", { t: "topics", topics: [`artifact:${A}`] });
    await tick();
    net.ready(net.conns[0], S1);
    await tick();
    hub.receive("t1", { t: "reconnect" });
    await tick();
    const again = net.open.at(-1)!;
    expect(again.headers["Last-Event-ID"]).toBeUndefined();
    net.ready(again, S2);
    await tick();
    expect(of("t1", "live")).toHaveLength(2);
    expect(net.posts.at(-1)!.url).toBe(`/api/stream/${S2}`);
  });

  it("fails a stream that does not say ready in time, and retries with backoff", async () => {
    hub.receive("t1", { t: "topics", topics: ["gallery"] });
    await tick();
    await vi.advanceTimersByTimeAsync(CONNECT_MS);
    expect(net.conns[0].aborted).toBe(true);
    expect(of("t1", "status")).toEqual([{ t: "status", up: false }]);
    await vi.advanceTimersByTimeAsync(1000);
    expect(net.conns).toHaveLength(2);
  });

  it("splits a refused request, so one topic's refusal keeps the others", async () => {
    net.refuse.set(`docs:${A}`, [403, "not_declared"]);
    hub.receive("t1", { t: "topics", topics: [`artifact:${A}`, `docs:${A}`] });
    await tick();
    net.ready(net.conns[0], S1);
    await tick();
    expect(of("t1", "refused")).toEqual([{ t: "refused", topic: `docs:${A}`, code: "not_declared" }]);
    expect(of("t1", "live")).toEqual([{ t: "live", topics: [`artifact:${A}`] }]);
    expect(net.held.get(S1)).toEqual(new Set([`artifact:${A}`]));
  });

  it("opens a new stream when the daemon no longer holds this one", async () => {
    hub.receive("t1", { t: "topics", topics: ["gallery"] });
    await tick();
    net.ready(net.conns[0], S1);
    await tick();
    net.auto = false;
    hub.receive("t1", { t: "topics", topics: ["gallery", `artifact:${A}`] });
    await tick();
    net.posts.at(-1)!.answer(404, { error: { code: "unknown_stream" } });
    await tick();
    expect(net.conns[0].aborted).toBe(true);
    expect(net.open).toHaveLength(1);
    expect(net.open[0].headers["Last-Event-ID"]).toBeUndefined();
  });

  it("drops a tab whose Web Lock frees", async () => {
    let free = () => {};
    const h = new Hub({ fetch: net.fetch, send: () => {}, watchLock: (_name, gone) => { free = gone; } });
    h.receive("t1", { t: "hello", lock: "clax-tab:x" });
    h.receive("t1", { t: "topics", topics: ["gallery"] });
    expect(h.stats().clients).toBe(1);
    free();
    expect(h.stats()).toMatchObject({ clients: 0, topics: [] });
    h.close();
  });
});
