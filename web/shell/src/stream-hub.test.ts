import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CONNECT_MS, Hub, type HubMsg, LINGER_MS, PING_MS } from "./stream-hub";
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
});
