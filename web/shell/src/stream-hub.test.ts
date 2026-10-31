import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CONNECT_MS, Hub, type HubMsg, LINGER_MS, PING_MS, RETRY_LABEL, SOCKET_PING_MS } from "./stream-hub";
import { Net } from "./test/fake-net";

/** Longer than any backoff wait (lifecycle.ts: 30 s, spread up by 20%). */
const BACKOFF_CAP = 36_000;

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

  it("waits out its backoff on the shell's clock, re-armed after each failure and cancelled once no tab wants a topic", async () => {
    const clock = (globalThis as unknown as { claxTestClock: { advance(ms: number): number; dueIn(label: string): number | null } }).claxTestClock;
    const due = () => clock.dueIn(RETRY_LABEL);
    hub.receive("t1", { t: "topics", topics: [`artifact:${A}`] });
    await tick();
    net.ready(net.conns[0], S1);
    await tick();
    expect(due()).toBeNull();
    // The daemon goes away: the hub waits the first backoff (500 ms, ±20%) on the clock.
    net.conns[0].end();
    await tick();
    const first = due();
    expect(first).not.toBeNull();
    expect(first!).toBeLessThanOrEqual(600);
    const opened = net.conns.length;
    // Advancing the clock past it reconnects now, with no timer advanced.
    clock.advance(first!);
    await tick();
    expect(net.conns.length).toBe(opened + 1);
    expect(due()).toBeNull();
    // That stream fails too: the wait is armed again, for the next backoff (1 s, ±20%).
    net.conns.at(-1)!.end();
    await tick();
    expect(due()).not.toBeNull();
    expect(due()!).toBeLessThanOrEqual(1200);
    // No tab wants a topic: the wait is cancelled, and nothing reconnects.
    hub.receive("t1", { t: "topics", topics: [] });
    await tick();
    expect(due()).toBeNull();
    clock.advance(BACKOFF_CAP);
    await tick();
    expect(net.conns.length).toBe(opened + 1);
  });

  it("tells a tab that joined during an outage that its topics are live once the stream resumes with them", async () => {
    hub.receive("t1", { t: "topics", topics: [`artifact:${A}`] });
    await tick();
    net.ready(net.conns[0], S1);
    await tick();
    net.conns[0].end();
    await vi.advanceTimersByTimeAsync(1000);
    hub.receive("t2", { t: "topics", topics: [`artifact:${A}`] });
    await tick();
    expect(of("t2", "live")).toEqual([]);
    net.ready(net.open.at(-1)!, S1, true, [`artifact:${A}`]);
    await tick();
    expect(of("t2", "live")).toEqual([{ t: "live", topics: [`artifact:${A}`] }]);
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

  it("reopens once per caller however many tabs ask: three tabs announcing one rename reopen the stream once", async () => {
    const unnamed = { level: "view", viewer: "u_1" };
    const named = { level: "interact", viewer: "u_1" };
    for (const t of ["t1", "t2", "t3"]) hub.receive(t, { t: "topics", topics: [`artifact:${A}`] });
    await tick();
    expect(net.conns).toHaveLength(1);
    net.ready(net.conns[0], S1, false, [], unnamed);
    await tick();
    // An announcement that leaves the caller as the stream was opened reopens nothing.
    hub.receive("t2", { t: "reconnect", caller: "u_1false" });
    await tick();
    expect(net.conns).toHaveLength(1);
    // The rename: every tab hears it and asks; the first ask reopens, the
    // others ask while that stream is opening, and its `ready` settles them.
    for (const t of ["t1", "t2", "t3"]) hub.receive(t, { t: "reconnect", caller: "u_1true" });
    await tick();
    expect(net.conns).toHaveLength(2);
    net.ready(net.conns[1], S2, false, [], named);
    await tick();
    expect(net.conns).toHaveLength(2);
    hub.receive("t3", { t: "reconnect", caller: "u_1true" });
    await tick();
    expect(net.conns).toHaveLength(2);
    // A drop and the hub's own reconnect resume the stream as it was opened.
    net.conns[1].end();
    await vi.advanceTimersByTimeAsync(1000);
    expect(net.conns).toHaveLength(3);
    net.ready(net.conns[2], S2, true, [`artifact:${A}`], named);
    await tick();
    hub.receive("t1", { t: "reconnect", caller: "u_1true" });
    await tick();
    expect(net.conns).toHaveLength(3);
  });

  it("does not reopen for a changed caller its own new stream is already opened as", async () => {
    hub.receive("t1", { t: "topics", topics: [`artifact:${A}`] });
    await tick();
    net.ready(net.conns[0], S1, false, [], { level: "view", viewer: "u_1" });
    await tick();
    // The token arrives (the browser is now the owner's): a subscription sent
    // with the new cookie finds the old stream refused, and the hub opens a
    // new one, before the tab's ask for the change reaches it.
    net.auto = false;
    hub.receive("t1", { t: "topics", topics: [`artifact:${A}`, "inbox"] });
    await tick();
    net.posts.at(-1)!.answer(404, { error: { code: "unknown_stream" } });
    await tick();
    expect(net.conns).toHaveLength(2);
    // Asked while that stream opens: its `ready` decides.
    hub.receive("t1", { t: "reconnect", caller: "admin" });
    await tick();
    expect(net.conns).toHaveLength(2);
    net.auto = true;
    net.ready(net.conns[1], S2, false, [], { level: "admin", viewer: "u_0" });
    await tick();
    expect(net.conns).toHaveLength(2);
    // Asked once it is up, for the caller it has: nothing.
    hub.receive("t1", { t: "reconnect", caller: "admin" });
    await tick();
    expect(net.conns).toHaveLength(2);
  });

  it("reopens when the stream that answers an ask is opened as another caller", async () => {
    hub.receive("t1", { t: "topics", topics: [`artifact:${A}`] });
    await tick();
    hub.receive("t1", { t: "reconnect", caller: "u_1true" });
    net.ready(net.conns[0], S1, false, [], { level: "view", viewer: "u_1" });
    await tick();
    expect(net.conns).toHaveLength(2);
    net.ready(net.conns[1], S2, false, [], { level: "interact", viewer: "u_1" });
    await tick();
    expect(net.conns).toHaveLength(2);
  });

  it("lets a stream request in flight answer before reopening for a reconnect that names no caller", async () => {
    hub.receive("t1", { t: "topics", topics: [`artifact:${A}`] });
    await tick();
    hub.receive("t1", { t: "reconnect" });
    await tick();
    // Not aborted before it answers: a browser would keep its socket open.
    expect(net.conns).toHaveLength(1);
    expect(net.conns[0].aborted).toBe(false);
    net.ready(net.conns[0], S1);
    await tick();
    expect(net.conns).toHaveLength(2);
    expect(net.conns[0].aborted).toBe(true);
    expect(net.conns[1].headers["Last-Event-ID"]).toBeUndefined();
    net.ready(net.conns[1], S2);
    await tick();
    expect(net.conns).toHaveLength(2);
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
  it("tells a tab joining a topic whose removal is in flight that it is live only once the stream carries it again", async () => {
    hub.receive("t1", { t: "topics", topics: [`artifact:${A}`] });
    await tick();
    net.ready(net.conns[0], S1);
    await tick();
    net.auto = false;
    hub.receive("t1", { t: "topics", topics: [] });
    await tick();
    const removal = net.posts.at(-1)!;
    expect(removal.body).toEqual({ subscribe: [], unsubscribe: [`artifact:${A}`] });
    // Another tab wants the topic while the removal is in flight.
    hub.receive("t2", { t: "topics", topics: [`artifact:${A}`] });
    await tick();
    removal.answer(200, { seq: 5, topics: [] });
    await tick();
    // Events between the removal and the next subscription never reach the
    // stream: a `live` before then would have the tab refetch too early.
    expect(of("t2", "live")).toEqual([]);
    const again = net.posts.at(-1)!;
    expect(again.body).toEqual({ subscribe: [`artifact:${A}`], unsubscribe: [] });
    again.answer(200, { seq: 7, topics: [`artifact:${A}`] });
    await tick();
    expect(of("t2", "live")).toEqual([{ t: "live", topics: [`artifact:${A}`] }]);
  });

  it("asks the most recently focused tab to notify, once per new item, only when no tab has focus", async () => {
    const sent: { ids: string[]; msg: HubMsg }[] = [];
    const h = new Hub({ notify: true, fetch: net.fetch, send: (ids, msg) => { sent.push({ ids, msg }); } });
    h.receive("t1", { t: "topics", topics: ["inbox"] });
    h.receive("t2", { t: "topics", topics: ["inbox"] });
    h.receive("t3", { t: "topics", topics: ["gallery"] });
    await tick();
    net.ready(net.conns[0], S1);
    await tick();
    let n = 0;
    const deliver = (data: Record<string, unknown>) => net.event(net.conns[0], S1, ++n, "inbox", "inbox_item", data);
    h.receive("t1", { t: "focus", focused: true });
    h.receive("t1", { t: "focus", focused: false });
    h.receive("t2", { t: "focus", focused: true });
    deliver({ item: { id: "I1", seq: 1, read: false }, unread: 1 });
    await tick();
    expect(sent.filter(m => m.msg.t === "notify")).toEqual([]);
    h.receive("t2", { t: "focus", focused: false });
    // The tab focused last, though not focused now, and only one holding `inbox`.
    h.receive("t3", { t: "focus", focused: true });
    h.receive("t3", { t: "focus", focused: false });
    deliver({ item: { id: "I2", seq: 2, read: false }, unread: 2 });
    deliver({ item: { id: "I2", seq: 2, read: false }, unread: 2 });
    deliver({ item: { id: "I3", seq: 3, read: true }, unread: 2 });
    // I1 came while a tab had focus: never announced. An older item marked
    // unread again or changed (a withdrawn question) is not new either.
    deliver({ item: { id: "I1", seq: 1, read: false }, unread: 2 });
    await tick();
    expect(sent.filter(m => m.msg.t === "notify")).toEqual([{ ids: ["t2"], msg: { t: "notify", data: { topic: "inbox", item: { id: "I2", seq: 2, read: false }, unread: 2 } } }]);
    h.close();
  });

  it("announces the unread items that came before the stream carried inbox, once each, oldest first", async () => {
    const sent: { ids: string[]; msg: HubMsg }[] = [];
    const h = new Hub({ notify: true, fetch: net.fetch, send: (ids, msg) => { sent.push({ ids, msg }); } });
    const notified = () => sent.filter(m => m.msg.t === "notify").map(m => m.msg.t === "notify" && (m.msg.data.item as { id: string }).id);
    // Unread before any tab wanted the inbox: not new.
    net.inbox.push({ id: "I4", seq: 4, read: false });
    h.receive("t1", { t: "topics", topics: ["inbox"] });
    await tick();
    expect(net.inboxGets).toEqual(["/api/inbox?limit=1"]);
    net.auto = false;
    net.ready(net.conns[0], S1);
    await tick();
    expect(net.posts).toHaveLength(1);
    // Before the subscription holds: no event, only the store has them.
    net.inbox.push({ id: "I5", seq: 5, read: false }, { id: "I6", seq: 6, read: true });
    // Just after it holds, before its answer: an event, and in the store.
    net.inbox.push({ id: "I7", seq: 7, read: false });
    net.event(net.conns[0], S1, 1, "inbox", "inbox_item", { item: { id: "I7", seq: 7, read: false }, unread: 3 });
    await tick();
    expect(notified()).toEqual(["I7"]);
    net.posts[0].answer(200, { seq: 0, topics: ["inbox"] });
    await tick();
    expect(net.inboxGets.at(-1)).toBe("/api/inbox?read=unread&limit=20");
    expect(notified()).toEqual(["I7", "I5"]);
    expect(sent.find(m => m.msg.t === "notify" && (m.msg.data.item as { id: string }).id === "I5")).toEqual({ ids: ["t1"], msg: { t: "notify", data: { topic: "inbox", item: { id: "I5", seq: 5, read: false } } } });
    // Later events: one already announced is not again; a new one is.
    net.event(net.conns[0], S1, 2, "inbox", "inbox_item", { item: { id: "I5", seq: 5, read: false }, unread: 3 });
    net.event(net.conns[0], S1, 3, "inbox", "inbox_item", { item: { id: "I8", seq: 8, read: false }, unread: 4 });
    await tick();
    expect(notified()).toEqual(["I7", "I5", "I8"]);
    h.close();
  });

  it("catches up on the items that came while a new stream was being subscribed", async () => {
    const sent: HubMsg[] = [];
    const h = new Hub({ notify: true, fetch: net.fetch, send: (_ids, msg) => { sent.push(msg); } });
    h.receive("t1", { t: "topics", topics: ["inbox"] });
    await tick();
    net.ready(net.conns[0], S1);
    await tick();
    net.inbox.push({ id: "I1", seq: 1, read: false });
    net.event(net.conns[0], S1, 1, "inbox", "inbox_item", { item: { id: "I1", seq: 1, read: false }, unread: 1 });
    await tick();
    // The daemon restarts: the new stream holds nothing until subscribed.
    h.receive("t1", { t: "reconnect" });
    await tick();
    net.inbox.push({ id: "I2", seq: 2, read: false });
    net.ready(net.open.at(-1)!, S2);
    await tick();
    expect(sent.filter(m => m.t === "notify").map(m => m.t === "notify" && (m.data.item as { id: string }).id)).toEqual(["I1", "I2"]);
    // The baseline was fetched once, when the tab first wanted the inbox.
    expect(net.inboxGets.filter(u => u.includes("limit=1&") || u.endsWith("limit=1"))).toHaveLength(1);
    h.close();
  });

  it("catches up once when a request holding inbox is split, and announces only the newest few", async () => {
    const sent: HubMsg[] = [];
    const h = new Hub({ notify: true, fetch: net.fetch, send: (_ids, msg) => { sent.push(msg); } });
    net.refuse.set(`docs:${A}`, [403, "not_declared"]);
    h.receive("t1", { t: "topics", topics: ["inbox", `docs:${A}`] });
    await tick();
    net.auto = false;
    net.ready(net.conns[0], S1);
    await tick();
    for (let s = 1; s <= 5; s++) net.inbox.push({ id: `I${s}`, seq: s, read: false });
    net.auto = true;
    net.posts[0].answer(403, { error: { code: "not_declared" } });
    await vi.advanceTimersByTimeAsync(10);
    expect(net.inboxGets.filter(u => u.includes("read=unread"))).toHaveLength(1);
    expect(sent.filter(m => m.t === "notify").map(m => m.t === "notify" && (m.data.item as { id: string }).id)).toEqual(["I3", "I4", "I5"]);
    h.close();
  });

  it("asks for the baseline again after it failed, and then catches up", async () => {
    const sent: HubMsg[] = [];
    const h = new Hub({ notify: true, fetch: net.fetch, send: (_ids, msg) => { sent.push(msg); } });
    net.inboxStatus = 500;
    h.receive("t1", { t: "topics", topics: ["inbox"] });
    await tick();
    net.ready(net.conns[0], S1);
    await tick();
    // Failed when the tab first wanted the inbox, and again at the subscription.
    expect(net.inboxGets).toEqual(["/api/inbox?limit=1", "/api/inbox?limit=1"]);
    net.inboxStatus = 200;
    h.receive("t1", { t: "reconnect" });
    await tick();
    net.inbox.push({ id: "I1", seq: 1, read: false });
    net.ready(net.open.at(-1)!, S2);
    await vi.advanceTimersByTimeAsync(10);
    expect(net.inboxGets.slice(2)).toEqual(["/api/inbox?limit=1", "/api/inbox?read=unread&limit=20"]);
    // I1 came before the baseline that worked: not new.
    expect(sent.filter(m => m.t === "notify")).toEqual([]);
    h.close();
  });

  it("does not catch up for a caller the inbox refuses", async () => {
    const sent: HubMsg[] = [];
    const h = new Hub({ notify: true, fetch: net.fetch, send: (_ids, msg) => { sent.push(msg); } });
    net.inboxStatus = 403;
    net.inbox.push({ id: "I1", seq: 1, read: false });
    h.receive("t1", { t: "topics", topics: ["inbox"] });
    await tick();
    net.ready(net.conns[0], S1);
    await tick();
    expect(net.inboxGets).toEqual(["/api/inbox?limit=1"]);
    expect(sent.filter(m => m.t === "notify")).toEqual([]);
    h.close();
  });

  it("never asks a tab to notify unless made to", async () => {
    hub.receive("t1", { t: "topics", topics: ["inbox"] });
    await tick();
    net.ready(net.conns[0], S1);
    await tick();
    net.event(net.conns[0], S1, 1, "inbox", "inbox_item", { item: { id: "I1", seq: 1, read: false }, unread: 1 });
    await tick();
    expect(of("t1", "notify")).toEqual([]);
    expect(of("t1", "event")).toHaveLength(1);
  });

  it("keeps pinging a tab whose topics were all refused, so it does not count the hub dead", async () => {
    net.refuse.set(`docs:${A}`, [403, "not_declared"]);
    hub.receive("t1", { t: "topics", topics: [`docs:${A}`] });
    await tick();
    net.ready(net.conns[0], S1);
    await tick();
    expect(of("t1", "refused")).toHaveLength(1);
    // The tab says its topics again (a view mounted): still all refused.
    hub.receive("t1", { t: "topics", topics: [`docs:${A}`] });
    await vi.advanceTimersByTimeAsync(PING_MS);
    expect(of("t1", "ping")).toHaveLength(1);
  });

  it("asks again for live topics refused a seat when a tab retries (Join live)", async () => {
    const live = [`artifact:${A}`, `presence:${A}`];
    for (const t of live) net.refuse.set(t, [429, "no_live_seat"]);
    hub.receive("t1", { t: "topics", topics: ["gallery", ...live] });
    await tick();
    net.ready(net.conns[0], S1);
    await tick();
    expect(of("t1", "refused").map(m => m.t === "refused" && m.code)).toEqual(["no_live_seat", "no_live_seat"]);
    expect(net.held.get(S1)).toEqual(new Set(["gallery"]));
    const asked = net.posts.length;
    // The tab saying its topics again asks nothing: they stay refused.
    hub.receive("t1", { t: "topics", topics: ["gallery", ...live] });
    await tick();
    expect(net.posts).toHaveLength(asked);
    // A seat came free; the tab retries and the topics go live.
    net.refuse.clear();
    hub.receive("t1", { t: "retry", topics: live });
    await tick();
    expect(net.posts.length).toBeGreaterThan(asked);
    expect(of("t1", "live").at(-1)).toEqual({ t: "live", topics: expect.arrayContaining(live) });
    expect(net.held.get(S1)).toEqual(new Set(["gallery", ...live]));
  });

  it("splits a request refused a seat by artifact, so an artifact's live topics are seated or refused together", async () => {
    const live = [`artifact:${A}`, `presence:${A}`, `working:${A}`];
    hub.receive("t1", { t: "topics", topics: ["gallery", ...live] });
    await tick();
    net.ready(net.conns[0], S1);
    net.auto = false;
    // The daemon: a request adding any of A's live topics is refused while
    // another viewer holds the one seat; that viewer leaves once two
    // requests were refused.
    let refusals = 0;
    const held = new Set<string>();
    for (let answered = 0, i = 0; i < 20; i++) {
      await tick();
      for (; answered < net.posts.length; answered++) {
        const p = net.posts[answered];
        if (refusals < 2 && p.body.subscribe.some(t => live.includes(t))) {
          refusals++;
          p.answer(429, { error: { code: "no_live_seat" } });
          continue;
        }
        for (const t of p.body.unsubscribe) held.delete(t);
        for (const t of p.body.subscribe) held.add(t);
        p.answer(200, { seq: 0, topics: [...held] });
      }
    }
    // Either all of A's live topics are live, or all are refused: never a
    // seat held for a view that was told it has none.
    const seated = live.filter(t => held.has(t));
    expect(seated.length === 0 || seated.length === live.length).toBe(true);
    expect(held.has("gallery")).toBe(true);
    const refused = of("t1", "refused").map(m => m.t === "refused" && m.topic);
    expect(new Set(refused)).toEqual(new Set(seated.length ? [] : live));
  });

  it("takes a topic the daemon unseated off the stream, refused until a retry", async () => {
    hub.receive("t1", { t: "topics", topics: [`artifact:${A}`] });
    await tick();
    net.ready(net.conns[0], S1);
    await tick();
    net.conns[0].push(`event: unseated\ndata: ${JSON.stringify({ topic: `artifact:${A}`, code: "seat_idle" })}\nid: ${S1}:3\n\n`);
    await tick();
    expect(of("t1", "refused")).toEqual([{ t: "refused", topic: `artifact:${A}`, code: "seat_idle" }]);
    expect(of("t1", "event")).toEqual([]);
    expect(hub.stats().topics).toEqual([]);
    net.held.set(S1, new Set());
    hub.receive("t1", { t: "retry", topics: [`artifact:${A}`] });
    await tick();
    expect(net.posts.at(-1)!.body.subscribe).toEqual([`artifact:${A}`]);
  });

  it("carries the stream over a WebSocket once the server answers the plain request 426, resuming by its parameter", async () => {
    type Sock = { url: string; sent: string[]; closed: boolean; onmessage: ((m: MessageEvent) => void) | null; onclose: (() => void) | null; onerror: (() => void) | null; send(t: string): void; close(): void };
    const socks: Sock[] = [];
    const plain: string[] = [];
    const fetch426 = (async (input: RequestInfo | URL, init?: RequestInit) => {
      if (String(input) !== "/api/stream") return net.fetch(input, init);
      plain.push(String(input));
      return new Response(JSON.stringify({ error: { code: "websocket_required" } }), { status: 426 });
    }) as typeof fetch;
    hub.close();
    hub = new Hub({
      fetch: fetch426,
      send: (ids, msg) => { for (const id of ids) got.set(id, [...(got.get(id) ?? []), msg]); },
      socket: url => {
        const s: Sock = { url, sent: [], closed: false, onmessage: null, onclose: null, onerror: null, send(t) { this.sent.push(t); }, close() { this.closed = true; } };
        socks.push(s);
        return s as unknown as WebSocket;
      },
    });
    const say = (s: Sock, text: string) => s.onmessage?.({ data: text } as MessageEvent);
    hub.receive("t1", { t: "hello", proof: "cxp_s" });
    hub.receive("t1", { t: "topics", topics: [`artifact:${A}`] });
    await tick();
    expect(plain).toHaveLength(1);
    expect(socks).toHaveLength(1);
    // A WebSocket sends no header: the proof rides its address.
    expect(socks[0].url).toMatch(/^(ws:\/\/[^/]+)?\/api\/stream\?owner_proof=cxp_s$/);
    say(socks[0], `event: ready\ndata: ${JSON.stringify({ stream: S1, seq: 0, resumed: false, topics: [] })}\n\n`);
    await tick();
    expect(net.posts[0].url).toBe(`/api/stream/${S1}`);
    expect(of("t1", "live")).toEqual([{ t: "live", topics: [`artifact:${A}`] }]);
    say(socks[0], `event: version\ndata: ${JSON.stringify({ topic: `artifact:${A}`, artifact_id: A, n: 2 })}\nid: ${S1}:4\n\n`);
    expect(of("t1", "event")).toEqual([{ t: "event", topic: `artifact:${A}`, name: "version", data: { topic: `artifact:${A}`, artifact_id: A, n: 2 } }]);
    // Pings stand in for the keep-alive; the answer keeps the stream up.
    await vi.advanceTimersByTimeAsync(SOCKET_PING_MS);
    expect(socks[0].sent).toEqual(["ping"]);
    say(socks[0], "pong");
    // The socket closes: the next one resumes the stream, and no plain request is made again.
    socks[0].onclose?.();
    await vi.advanceTimersByTimeAsync(1000);
    expect(of("t1", "status")).toEqual([{ t: "status", up: false }]);
    expect(socks).toHaveLength(2);
    expect(plain).toHaveLength(1);
    expect(socks[1].url).toMatch(new RegExp(`/api/stream\\?last_event_id=${S1}%3A4&owner_proof=cxp_s$`));
  });

  it("sends the owner browser session proof its tabs say on every request, and opens the stream again with a new one", async () => {
    hub.receive("t1", { t: "hello", proof: "cxp_one" });
    hub.receive("t1", { t: "topics", topics: [`artifact:${A}`] });
    await tick();
    expect(net.conns[0].headers["x-clax-owner-proof"]).toBe("cxp_one");
    net.ready(net.conns[0], S1);
    await tick();
    expect(net.posts[0].headers["x-clax-owner-proof"]).toBe("cxp_one");
    // The same proof again (another tab's hello) changes nothing.
    hub.receive("t2", { t: "hello", proof: "cxp_one" });
    await tick();
    expect(net.conns).toHaveLength(1);
    // A login in another tab: its new proof reopens the stream with it.
    hub.receive("t2", { t: "hello", proof: "cxp_two" });
    await tick();
    expect(net.conns[0].aborted).toBe(true);
    expect(net.conns.at(-1)!.headers["x-clax-owner-proof"]).toBe("cxp_two");
  });

  it("sends no proof header when no tab has one", async () => {
    hub.receive("t1", { t: "hello" });
    hub.receive("t1", { t: "topics", topics: [`artifact:${A}`] });
    await tick();
    expect(net.conns[0].headers["x-clax-owner-proof"]).toBeUndefined();
  });
});

describe("the stream hub on the app's host (`host: true`)", () => {
  let host: Hub;
  /** A hub on the host whose tabs answer its pings, as `EventStream` does. */
  const hostHub = (f: typeof fetch = net.fetch) => {
    const h: Hub = new Hub({
      host: true, fetch: f,
      send: (ids, msg) => {
        for (const id of ids) got.set(id, [...(got.get(id) ?? []), msg]);
        if (msg.t === "ping") queueMicrotask(() => { for (const id of ids) h.receive(id, { t: "ping" }); });
      },
    });
    return h;
  };
  beforeEach(() => { host = hostHub(); });
  afterEach(() => { host.close(); });
  const TEN_MINUTES = 10 * 60_000;

  it("opens no WebSocket, even on a stream answered 426", async () => {
    host.close();
    const socks: string[] = [];
    const f = (async (input: RequestInfo | URL, init?: RequestInit) => String(input) === "/api/stream"
      ? new Response(JSON.stringify({ error: { code: "websocket_required" } }), { status: 426 })
      : net.fetch(input, init)) as typeof fetch;
    host = new Hub({ host: true, fetch: f, send: () => {}, socket: url => { socks.push(url); return { send() {}, close() {} } as unknown as WebSocket; } });
    host.receive("t1", { t: "topics", topics: [`artifact:${A}`] });
    await tick();
    await vi.advanceTimersByTimeAsync(60_000);
    expect(socks).toEqual([]);
  });

  it("waits on a stream that says nothing, with no status and no reconnect, where a browser's hub gives up", async () => {
    host.receive("t1", { t: "topics", topics: [`artifact:${A}`] });
    await tick();
    await vi.advanceTimersByTimeAsync(TEN_MINUTES);
    expect(net.conns).toHaveLength(1);
    expect(net.conns[0].aborted).toBe(false);
    expect(of("t1", "status")).toEqual([]);
    // A browser's hub, on the same silence, says it is down and connects again.
    hub.receive("t2", { t: "topics", topics: [`artifact:${A}`] });
    await tick();
    await vi.advanceTimersByTimeAsync(TEN_MINUTES);
    expect(of("t2", "status")).toContainEqual({ t: "status", up: false });
    expect(net.conns.length).toBeGreaterThan(2);
  });

  it("keeps a live stream that goes silent", async () => {
    host.receive("t1", { t: "topics", topics: [`artifact:${A}`] });
    await tick();
    net.ready(net.conns[0], S1);
    await tick();
    await vi.advanceTimersByTimeAsync(TEN_MINUTES);
    expect(net.conns).toHaveLength(1);
    expect(net.conns[0].aborted).toBe(false);
    expect(of("t1", "status")).toEqual([]);
  });

  it("waits on a subscription that does not answer", async () => {
    host.receive("t1", { t: "topics", topics: ["gallery"] });
    await tick();
    net.ready(net.conns[0], S1);
    await tick();
    net.auto = false;
    host.receive("t1", { t: "topics", topics: ["gallery", `artifact:${A}`] });
    await tick();
    await vi.advanceTimersByTimeAsync(TEN_MINUTES);
    expect(net.conns).toHaveLength(1);
    expect(net.conns[0].aborted).toBe(false);
    net.posts.at(-1)!.answer(200, { seq: 0, topics: ["gallery", `artifact:${A}`] });
    await tick();
    expect(of("t1", "live")).toEqual([{ t: "live", topics: ["gallery"] }, { t: "live", topics: [`artifact:${A}`] }]);
    expect(of("t1", "status")).toEqual([]);
  });

  it("waits on an inbox request that does not answer", async () => {
    let aborted = false;
    host.close();
    host = new Hub({
      host: true, notify: true,
      fetch: ((i, init) => {
        if (!String(i).startsWith("/api/inbox")) return net.fetch(i, init);
        init?.signal?.addEventListener("abort", () => { aborted = true; });
        return new Promise<Response>(() => {});
      }) as typeof fetch,
      send: () => {},
    });
    host.receive("t1", { t: "topics", topics: ["inbox"] });
    await tick();
    await vi.advanceTimersByTimeAsync(TEN_MINUTES);
    expect(aborted).toBe(false);
  });

  /** A host hub whose requests are recorded in `asked`. */
  const asking = () => {
    const asked: string[] = [];
    const f = net.fetch;
    host.close();
    host = hostHub(((i, init) => { asked.push(String(i)); return f(i, init); }) as typeof fetch);
    return asked;
  };
  /** The core's closing block of a subscription. */
  const endBlock = (data: object) => `event: end\ndata: ${JSON.stringify(data)}\n\n`;

  it("opens a stream that ends with no ready again after growing waits, with no status and no token request", async () => {
    const asked = asking();
    host.receive("t1", { t: "topics", topics: [`artifact:${A}`] });
    await tick();
    // backoff(n) waits 500 * 2^n ms, give or take a fifth.
    for (const [n, floor] of [[1, 400], [2, 800], [3, 1600]]) {
      net.conns.at(-1)!.end();
      await vi.advanceTimersByTimeAsync(floor - 1);
      expect(net.conns).toHaveLength(n);
      await vi.advanceTimersByTimeAsync(floor * 1.5 + 1);
      expect(net.conns).toHaveLength(n + 1);
    }
    expect(of("t1", "status")).toEqual([]);
    expect(asked.filter(u => u === "/api/token")).toEqual([]);
  });

  it("opens a stream that ended after its ready again after one short wait, from where it stopped", async () => {
    const asked = asking();
    host.receive("t1", { t: "topics", topics: [`artifact:${A}`] });
    await tick();
    for (let round = 1; round <= 3; round++) {
      net.ready(net.conns.at(-1)!, S1, round > 1, [`artifact:${A}`]);
      await tick();
      net.event(net.conns.at(-1)!, S1, 6 + round, `artifact:${A}`, "version", { artifact_id: A, n: 2 });
      await tick();
      net.conns.at(-1)!.push(endBlock({ reason: "core_closed" }));
      net.conns.at(-1)!.end();
      await vi.advanceTimersByTimeAsync(399);
      expect(net.conns).toHaveLength(round);
      await vi.advanceTimersByTimeAsync(202);
      expect(net.conns).toHaveLength(round + 1);
      expect(net.conns.at(-1)!.headers["Last-Event-ID"]).toBe(`${S1}:${6 + round}`);
    }
    expect(of("t1", "status")).toEqual([]);
    expect(of("t1", "live")).toHaveLength(1);
    expect(asked.filter(u => u === "/api/token")).toEqual([]);
  });

  for (const status of [400, 403, 404]) {
    it(`opens nothing after the host refuses the stream ${status}, until a tab's topics change`, async () => {
      host.receive("t1", { t: "topics", topics: [`artifact:${A}`] });
      await tick();
      net.conns[0].push(endBlock({ reason: "stream_refused", status }));
      net.conns[0].end();
      await vi.advanceTimersByTimeAsync(TEN_MINUTES);
      expect(net.conns).toHaveLength(1);
      expect(of("t1", "status")).toEqual([]);
      host.receive("t1", { t: "topics", topics: [`artifact:${A}`, "gallery"] });
      await tick();
      expect(net.conns).toHaveLength(2);
      await vi.advanceTimersByTimeAsync(TEN_MINUTES);
      expect(net.conns).toHaveLength(2);
    });
  }

  it("opens a stream the host refused 503 again after a backoff", async () => {
    host.receive("t1", { t: "topics", topics: [`artifact:${A}`] });
    await tick();
    net.conns[0].push(endBlock({ reason: "stream_refused", status: 503 }));
    net.conns[0].end();
    await vi.advanceTimersByTimeAsync(399);
    expect(net.conns).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(202);
    expect(net.conns).toHaveLength(2);
  });

  it("opens the stream again after growing waits when a subscription answers 503 every time", async () => {
    const asked = asking();
    net.refuse.set("gallery", [503, "internal"]);
    host.receive("t1", { t: "topics", topics: ["gallery"] });
    await tick();
    // Each new stream says ready, but its subscription fails: ready does not
    // reset the backoff, so each wait is longer than the one before.
    for (const [n, floor] of [[1, 400], [2, 800], [3, 1600]]) {
      net.ready(net.conns.at(-1)!, S1);
      await tick();
      expect(net.posts).toHaveLength(n);
      await vi.advanceTimersByTimeAsync(floor - 1);
      expect(net.conns).toHaveLength(n);
      await vi.advanceTimersByTimeAsync(floor * 1.5 + 1);
      expect(net.conns).toHaveLength(n + 1);
    }
    expect(of("t1", "status")).toEqual([]);
    expect(asked.filter(u => u === "/api/token")).toEqual([]);
  });

  it("starts the backoff over once a stream's subscriptions go through", async () => {
    net.refuse.set("gallery", [503, "internal"]);
    host.receive("t1", { t: "topics", topics: ["gallery"] });
    await tick();
    for (const floor of [400, 800]) {
      net.ready(net.conns.at(-1)!, S1);
      await tick();
      await vi.advanceTimersByTimeAsync(floor * 1.5 + 1);
    }
    net.refuse.delete("gallery");
    net.ready(net.conns.at(-1)!, S1);
    await tick();
    expect(of("t1", "live")).toEqual([{ t: "live", topics: ["gallery"] }]);
    // The stream ends after it carried every topic: one short wait.
    const n = net.conns.length;
    net.conns.at(-1)!.end();
    await vi.advanceTimersByTimeAsync(601);
    expect(net.conns).toHaveLength(n + 1);
  });

  it("opens a stream the core keeps answering unknown_stream for again after growing waits", async () => {
    host.receive("t1", { t: "topics", topics: ["gallery"] });
    await tick();
    net.refuse.set("gallery", [404, "unknown_stream"]);
    net.ready(net.conns[0], S1);
    await tick();
    // The first time at once.
    expect(net.conns).toHaveLength(2);
    for (const [n, floor] of [[2, 800], [3, 1600]]) {
      net.ready(net.conns.at(-1)!, S1);
      await tick();
      await vi.advanceTimersByTimeAsync(floor - 1);
      expect(net.conns).toHaveLength(n);
      await vi.advanceTimersByTimeAsync(floor * 1.5 + 1);
      expect(net.conns).toHaveLength(n + 1);
    }
  });
});
