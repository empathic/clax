import { afterEach, describe, expect, it, vi } from "vitest";
import type { HubMsg, TabMsg } from "../../../shell/src/stream-hub";
import type { PageView, WorkerToOverlay, WorkerToPanel } from "../messages";
import { FakeEvent } from "../../test/fake-chrome";
import { type TabState, Tabs, applyEvent, emptyTab, type TabsApi } from "./tabs";

const AID = "7q3k9mzx2b4t";
const AID2 = "8r4m0nzy3c5v";
const T1 = "01J9AAAAAAAAAAAAAAAAAAAAAA";
const thread = (id: string, body: string) => ({ id, artifact_id: AID, status: "open", comment_count: 1, last_comment: { id: `c${id}`, thread_id: id, author_kind: "viewer", author_name: "A", via_harness: null, body, created_at: "t" }, anchor: { kind: "element", selector: "body", file: "index.html" } });
const full = (id: string, extra: object = {}) => ({ id, artifact_id: AID, status: "open", comments: [{ id: `c${id}`, thread_id: id, author_kind: "viewer", author_name: "A", via_harness: null, body: "b", created_at: "t" }], feedback_state: null, ...extra });
const page = (aid = AID, path = "/"): PageView => ({ artifact_id: aid, origin: "http://localhost:5173", path, page_url: `http://localhost:5173${path}`, title: "T", current_version: 1, url: `http://localhost:5173${path}` });

describe("applyEvent", () => {
  it("applies thread deltas, deletions, feedback states and working lists", () => {
    let s = emptyTab(1, "http://localhost:5173/");
    s = applyEvent(s, "thread", { thread: thread(T1, "one") });
    expect(s.threads).toHaveLength(1);
    s = applyEvent(s, "feedback_state", { thread_id: T1, state: "delivered", tier: "wait", since: "t", resends: 0, exhausted: false });
    expect(s.threads[0].feedback_state?.state).toBe("delivered");
    s = applyEvent(s, "working", { working: [{ key: "k", harness: "claude", message: null, thread_ids: [], started_at: "t", last_heartbeat: "t" }] });
    expect(s.working).toHaveLength(1);
    s = applyEvent(s, "thread_deleted", { thread_id: T1 });
    expect(s.threads).toHaveLength(0);
  });

  it("marks the tab pending while an open thread waits for a snapshot", () => {
    let s = emptyTab(1, "http://localhost:5173/");
    s = applyEvent(s, "thread", { thread: { ...thread(T1, "x"), addressed_pending: { harness: "claude", at: "t" } } });
    expect(s.pending).toBe(true);
  });

  it("keeps only the feedback state's own fields, and follows the page's version", () => {
    let s: TabState = { ...emptyTab(1, "http://localhost:5173/"), page: page() };
    s = applyEvent(s, "thread", { thread: thread(T1, "one") });
    s = applyEvent(s, "feedback_state", { topic: `artifact:${AID}`, artifact_id: AID, thread_id: T1, state: "sent", tier: null, since: "t", resends: 1, exhausted: false });
    expect(s.threads[0].feedback_state).toEqual({ thread_id: T1, state: "sent", tier: null, since: "t", resends: 1, exhausted: false });
    s = applyEvent(s, "version", { artifact_id: AID, n: 3 });
    expect(s.page?.current_version).toBe(3);
    s = applyEvent(s, "version", { artifact_id: AID, n: 2 });
    expect(s.page?.current_version).toBe(3);
  });
});

function memory() {
  const data: Record<string, unknown> = {};
  return {
    data,
    get: async (k: string) => (k in data ? { [k]: structuredClone(data[k]) } : {}),
    set: async (v: Record<string, unknown>) => { Object.assign(data, structuredClone(v)); },
    remove: async (k: string) => { delete data[k]; },
  };
}

/** The documents in the tabs: whether each tab's current document has the overlay. */
function documents() { return new Set<number>(); }

function harness(store = memory(), docs = documents()) {
  const calls: string[] = [];
  const hubIn: { id: string; msg: TabMsg }[] = [];
  const detached: string[] = [];
  const overlay: { tabId: number; m: WorkerToOverlay }[] = [];
  const injected: number[] = [];
  const pages = new Map<string, { page: PageView | null; route: string | null }>();
  const threads = new Map<string, unknown[]>();
  const gates = new Map<string, Promise<void>>();
  const gate: { threads?: Promise<void>; thread?: Promise<void>; working?: Promise<void> } = {};
  const working = new Map<string, unknown[]>();
  const api: TabsApi = {
    lookup: async (url: string) => { calls.push(`lookup ${url}`); await gates.get(url); const r = pages.get(url); if (!r) throw Object.assign(new Error("down"), { code: "daemon_unreachable" }); return r; },
    threads: async (aid: string) => { calls.push(`threads ${aid}`); const out = structuredClone(threads.get(aid) ?? []); await gate.threads; return out as never; },
    thread: async (aid: string, tid: string) => { calls.push(`thread ${aid} ${tid}`); await gate.thread; return { thread: full(tid, { comments: [] }) as never }; },
    working: async (aid: string) => { calls.push(`working ${aid}`); const out = working.get(aid) ?? []; await gate.working; return out as never; },
    artifact: async (aid: string) => { calls.push(`artifact ${aid}`); return { artifact: { id: aid, participants: { people: [], agents: [{ handle: "a_1", harness: "claude", live: true }] } }, versions: [{ artifact_id: aid, n: 1 }] } as never; },
    me: async () => { calls.push("me"); return { viewer: { public_id: "u_owner", display_name: "Alex", created_at: "t" } }; },
    presence: async (aid: string, windowId: number) => { calls.push(`presence ${aid} ${windowId}`); return { people: [here("u_owner")] }; },
    presenceOf: async (aid: string) => { calls.push(`presenceOf ${aid}`); return { people: [here("u_mia")] }; },
  };
  const tabs = new Tabs({
    api,
    hub: { receive: (id, msg) => hubIn.push({ id, msg }), detach: id => detached.push(id) },
    toOverlay: (tabId, m) => overlay.push({ tabId, m }),
    inject: async tabId => { if (docs.has(tabId)) return false; docs.add(tabId); injected.push(tabId); return true; },
    present: async tabId => docs.has(tabId),
    store,
  });
  /** The tab loads a new document: whatever was in the old one is gone. */
  const reload = (tabId: number) => docs.delete(tabId);
  return { tabs, calls, hubIn, detached, overlay, injected, pages, threads, gates, gate, working, store, docs, reload };
}

function port(name = "panel:1") {
  const sent: WorkerToPanel[] = [];
  const onMessage = new FakeEvent<[unknown]>();
  const onDisconnect = new FakeEvent<[]>();
  return { name, sent, onMessage, onDisconnect, postMessage: (m: WorkerToPanel) => sent.push(m) };
}
const settle = async () => { for (let i = 0; i < 20; i++) await Promise.resolve(); };
const URL1 = "http://localhost:5173/";
function hold() { let release!: () => void; const p = new Promise<void>(r => { release = r; }); return { p, release }; }
const here = (id: string) => ({ public_id: id, display_name: id, state: "here" as const, where: null, since: "t" });
const resolvedDelta = (id: string) => ({ ...thread(id, "one"), status: "resolved" });

describe("Tabs", () => {
  it("looks the page up, loads its threads and working list, follows its topics and tells the overlay", async () => {
    const h = harness();
    h.pages.set("http://localhost:5173/", { page: page(), route: null });
    h.threads.set(AID, [full(T1)]);
    const s = await h.tabs.route(4, "http://localhost:5173/");
    expect(s.threads.map(t => t.id)).toEqual([T1]);
    expect(h.calls).toEqual(["lookup http://localhost:5173/", `threads ${AID}`, `working ${AID}`]);
    expect(h.hubIn).toEqual([{ id: "tab:4", msg: { t: "topics", topics: [`artifact:${AID}`, `working:${AID}`] } }]);
    expect(h.overlay.at(-1)).toMatchObject({ tabId: 4, m: { t: "state", page: page(), threads: [expect.objectContaining({ id: T1 })], commentMode: false, pending: false } });
  });

  it("follows the live page a thread posted from the tab created, and only then", async () => {
    const h = harness();
    h.pages.set(URL1, { page: null, route: null });
    await h.tabs.route(4, URL1);
    h.pages.set(URL1, { page: page(), route: null });
    h.threads.set(AID, [full(T1)]);
    h.calls.length = 0;
    h.tabs.posted(4, page());
    await settle();
    expect(h.calls).toEqual([`lookup ${URL1}`, `threads ${AID}`, `working ${AID}`]);
    expect(h.hubIn.at(-1)).toEqual({ id: "tab:4", msg: { t: "topics", topics: [`artifact:${AID}`, `working:${AID}`] } });
    // The tab already shows that page: its stream brings the thread.
    h.calls.length = 0;
    h.tabs.posted(4, page());
    h.tabs.posted(9, page());
    await settle();
    expect(h.calls).toEqual([]);
  });

  it("follows nothing on a page with no live page", async () => {
    const h = harness();
    h.pages.set("http://localhost:5173/new", { page: null, route: null });
    const s = await h.tabs.route(4, "http://localhost:5173/new");
    expect(s.page).toBeNull();
    expect(h.hubIn.at(-1)).toEqual({ id: "tab:4", msg: { t: "topics", topics: [] } });
  });

  it("keeps the newest navigation when an older lookup answers last", async () => {
    const h = harness();
    let release!: () => void;
    h.gates.set("http://localhost:5173/a", new Promise<void>(r => { release = r; }));
    h.pages.set("http://localhost:5173/a", { page: page(AID, "/a"), route: null });
    h.pages.set("http://localhost:5173/b", { page: page(AID2, "/b"), route: null });
    const slow = h.tabs.route(4, "http://localhost:5173/a");
    await h.tabs.route(4, "http://localhost:5173/b");
    release();
    await slow;
    expect(h.tabs.state(4)?.page?.artifact_id).toBe(AID2);
    expect(h.tabs.state(4)?.url).toBe("http://localhost:5173/b");
  });

  it("records a failed lookup on the tab", async () => {
    const h = harness();
    const s = await h.tabs.route(4, "http://localhost:5173/x");
    expect(s.error).toEqual({ code: "daemon_unreachable", message: "down" });
  });

  it("brings the overlay at a page load only when the page has open threads", async () => {
    const h = harness();
    h.pages.set("http://localhost:5173/", { page: page(), route: null });
    await h.tabs.hello(4, "http://localhost:5173/");
    expect(h.injected).toEqual([]);
    h.threads.set(AID, [full(T1)]);
    await h.tabs.hello(5, "http://localhost:5173/");
    expect(h.injected).toEqual([5]);
  });

  it("injects the overlay once and flips comment mode on each toggle", async () => {
    const h = harness();
    h.pages.set("http://localhost:5173/", { page: page(), route: null });
    await h.tabs.toggle(4, "http://localhost:5173/");
    expect(h.tabs.state(4)?.commentMode).toBe(true);
    await h.tabs.toggle(4, "http://localhost:5173/");
    expect(h.tabs.state(4)?.commentMode).toBe(false);
    expect(h.injected).toEqual([4]);
  });

  it("applies the stream's events to the tabs that follow them, and refetches when they go live", async () => {
    const h = harness();
    h.pages.set("http://localhost:5173/", { page: page(), route: null });
    await h.tabs.route(4, "http://localhost:5173/");
    h.calls.length = 0;
    h.tabs.fromHub(["tab:4"], { t: "event", topic: `artifact:${AID}`, name: "thread", data: { thread: thread(T1, "hi") } } satisfies HubMsg);
    expect(h.tabs.state(4)?.threads.map(t => t.id)).toEqual([T1]);
    expect(h.overlay.at(-1)?.m).toMatchObject({ t: "state", threads: [expect.objectContaining({ id: T1 })] });
    h.threads.set(AID, [full(T1), full("01J9BBBBBBBBBBBBBBBBBBBBBB")]);
    h.tabs.fromHub(["tab:4"], { t: "live", topics: [`artifact:${AID}`, `working:${AID}`] });
    await settle();
    expect(h.calls).toEqual(["lookup http://localhost:5173/", `threads ${AID}`, `working ${AID}`]);
    expect(h.tabs.state(4)?.threads).toHaveLength(2);
  });

  it("fetches a thread whole when a delta does not add up", async () => {
    const h = harness();
    h.pages.set("http://localhost:5173/", { page: page(), route: null });
    await h.tabs.route(4, "http://localhost:5173/");
    h.calls.length = 0;
    h.tabs.fromHub(["tab:4"], { t: "event", topic: `artifact:${AID}`, name: "thread", data: { thread: { ...thread(T1, "hi"), comment_count: 3 } } });
    await settle();
    expect(h.calls).toEqual([`thread ${AID} ${T1}`]);
  });

  it("lists the open threads waiting for a snapshot", async () => {
    const h = harness();
    h.pages.set("http://localhost:5173/", { page: page(), route: null });
    h.threads.set(AID, [full(T1, { addressed_pending: { harness: "claude", at: "t" } }), full("01J9BBBBBBBBBBBBBBBBBBBBBB"), full("01J9CCCCCCCCCCCCCCCCCCCCCC", { status: "resolved", addressed_pending: { harness: "claude", at: "t" } })]);
    await h.tabs.route(4, "http://localhost:5173/");
    expect(h.tabs.pendingIds(4)).toEqual([T1]);
    expect(h.tabs.state(4)?.pending).toBe(true);
  });

  it("answers a panel with its tab's state, the owner and the page's versions, and pushes changes", async () => {
    const h = harness();
    h.pages.set("http://localhost:5173/", { page: page(), route: null });
    await h.tabs.route(4, "http://localhost:5173/");
    const p = port();
    const heard: unknown[] = [];
    h.tabs.attachPanel(p as unknown as chrome.runtime.Port, (tabId, m) => heard.push([tabId, m]));
    p.onMessage.fire({ t: "watch-tab", tabId: 4 });
    await settle();
    const last = p.sent.at(-1)!;
    expect(last.t).toBe("tab");
    if (last.t !== "tab") return;
    expect(last.state).toMatchObject({ tabId: 4, page: page(), viewer: { public_id: "u_owner", display_name: "Alex" }, versions: [{ n: 1 }], participants: { agents: [{ handle: "a_1" }] } });
    expect(JSON.stringify(last.state)).not.toContain("cxe_");
    expect(heard).toEqual([[4, { t: "watch-tab", tabId: 4 }]]);
    const before = p.sent.length;
    h.tabs.fromHub(["tab:4"], { t: "event", topic: `artifact:${AID}`, name: "thread", data: { thread: thread(T1, "hi") } });
    expect(p.sent.length).toBe(before + 1);
    p.onDisconnect.fire();
    h.tabs.fromHub(["tab:4"], { t: "event", topic: `artifact:${AID}`, name: "thread_deleted", data: { thread_id: T1 } });
    expect(p.sent.length).toBe(before + 1);
  });

  it("drops a closed tab and its topics", async () => {
    const h = harness();
    h.pages.set("http://localhost:5173/", { page: page(), route: null });
    await h.tabs.route(4, "http://localhost:5173/");
    h.tabs.close(4);
    expect(h.tabs.state(4)).toBeUndefined();
    expect(h.detached).toEqual(["tab:4"]);
  });

  it("opens a new stream after a new pairing", async () => {
    const h = harness();
    h.pages.set("http://localhost:5173/", { page: page(), route: null });
    await h.tabs.route(4, "http://localhost:5173/");
    h.tabs.repaired();
    expect(h.hubIn.at(-1)).toEqual({ id: "tab:4", msg: { t: "reconnect" } });
  });

  it("treats a page load as a new document: comment mode off, and the overlay injected again when the page has open threads", async () => {
    const h = harness();
    h.pages.set(URL1, { page: page(), route: null });
    h.threads.set(AID, [full(T1)]);
    await h.tabs.hello(4, URL1);
    await h.tabs.toggle(4, URL1);
    expect(h.tabs.state(4)?.commentMode).toBe(true);
    h.reload(4);
    await h.tabs.hello(4, URL1);
    expect(h.injected).toEqual([4, 4]);
    expect(h.tabs.state(4)).toMatchObject({ overlay: true, commentMode: false, selected: null, resolved: {} });
  });

  it("after a real load, forgets the overlay and comment mode but keeps the click's grant", async () => {
    const h = harness();
    h.pages.set(URL1, { page: page(), route: null });
    h.tabs.activate(4, URL1);
    await h.tabs.toggle(4, URL1);
    h.reload(4);
    await h.tabs.navigated(4);
    expect(h.tabs.state(4)).toMatchObject({ overlay: false, commentMode: false, active: true });
    await h.tabs.toggle(4, URL1);
    expect(h.injected).toEqual([4, 4]);
    expect(h.tabs.state(4)?.commentMode).toBe(true);
  });

  it("keeps comment mode, the overlay and the click's grant through an in-page route change", async () => {
    const h = harness();
    h.pages.set(URL1, { page: page(), route: null });
    h.pages.set("http://localhost:5173/settings", { page: page(AID, "/settings"), route: null });
    h.tabs.activate(4, URL1);
    await h.tabs.toggle(4, URL1);
    // Chrome reports the in-page navigation as loading; the document (and its overlay) stays.
    await h.tabs.navigated(4);
    expect(h.tabs.state(4)).toMatchObject({ overlay: true, commentMode: true, active: true });
    // On a site without the permanent permission the overlay is still heard.
    expect(h.tabs.admits(4)).toBe(true);
    await h.tabs.fromOverlay(4, 1, { t: "route", url: "http://localhost:5173/settings" }, "http://localhost:5173/settings");
    expect(h.tabs.state(4)).toMatchObject({ url: "http://localhost:5173/settings", commentMode: true, overlay: true });
  });

  it("turns comment mode on when a toggle had to bring the overlay back, whatever the record said", async () => {
    const h = harness();
    h.pages.set(URL1, { page: page(), route: null });
    await h.tabs.toggle(4, URL1);
    // A reload Chrome reported as nothing the worker noticed: the record still says on.
    h.reload(4);
    await h.tabs.toggle(4, URL1);
    expect(h.injected).toEqual([4, 4]);
    expect(h.tabs.state(4)).toMatchObject({ overlay: true, commentMode: true });
  });

  it("keeps a delta that arrives while a same-page lookup is in flight", async () => {
    const h = harness();
    h.pages.set(URL1, { page: page(), route: null });
    await h.tabs.route(4, URL1);
    const g = hold();
    h.gates.set(URL1, g.p);
    const r = h.tabs.route(4, URL1);
    await settle();
    h.tabs.fromHub(["tab:4"], { t: "event", topic: `artifact:${AID}`, name: "thread", data: { thread: thread(T1, "hi") } });
    g.release();
    await r;
    expect(h.tabs.state(4)?.threads.map(t => t.id)).toEqual([T1]);
  });

  it("keeps a delta newer than a refetch's answer", async () => {
    const h = harness();
    h.pages.set(URL1, { page: page(), route: null });
    h.threads.set(AID, [full(T1)]);
    await h.tabs.route(4, URL1);
    const g = hold();
    h.gate.threads = g.p;
    h.gate.working = g.p;
    h.tabs.fromHub(["tab:4"], { t: "live", topics: [`artifact:${AID}`] });
    await settle();
    expect(h.calls.filter(c => c.startsWith("threads"))).toHaveLength(2);
    // Committed after the daemon answered the list, heard before the answer is applied.
    h.tabs.fromHub(["tab:4"], { t: "event", topic: `artifact:${AID}`, name: "thread", data: { thread: resolvedDelta(T1) } });
    h.tabs.fromHub(["tab:4"], { t: "event", topic: `working:${AID}`, name: "working", data: { working: [{ key: "k", agent: "a_1", harness: "claude", message: null, thread_ids: [T1], started_at: "t", last_heartbeat: "t" }] } });
    g.release();
    await settle();
    expect(h.tabs.state(4)?.threads[0].status).toBe("resolved");
    expect(h.tabs.state(4)?.working).toHaveLength(1);
  });

  it("keeps a delta newer than a thread's own refetch", async () => {
    const h = harness();
    h.pages.set(URL1, { page: page(), route: null });
    h.threads.set(AID, [full(T1)]);
    await h.tabs.route(4, URL1);
    const g = hold();
    h.gate.thread = g.p;
    h.tabs.fromHub(["tab:4"], { t: "event", topic: `artifact:${AID}`, name: "thread", data: { thread: { ...thread(T1, "hi"), comment_count: 3 } } });
    await settle();
    h.tabs.fromHub(["tab:4"], { t: "event", topic: `artifact:${AID}`, name: "feedback_state", data: { thread_id: T1, state: "delivered", tier: "wait", since: "t", resends: 0, exhausted: false } });
    g.release();
    await settle();
    expect(h.tabs.state(4)?.threads[0].feedback_state?.state).toBe("delivered");
  });

  it("refetches once for notices that arrive together", async () => {
    const h = harness();
    h.pages.set(URL1, { page: page(), route: null });
    await h.tabs.route(4, URL1);
    h.calls.length = 0;
    h.tabs.fromHub(["tab:4"], { t: "live", topics: [`artifact:${AID}`] });
    h.tabs.fromHub(["tab:4"], { t: "resync", topic: `working:${AID}` });
    await settle();
    expect(h.calls.filter(c => c.startsWith("lookup"))).toHaveLength(1);
  });

  it("tells the panels and the overlays whether the stream is up", async () => {
    const h = harness();
    h.pages.set(URL1, { page: page(), route: null });
    await h.tabs.route(4, URL1);
    const p = port();
    h.tabs.attachPanel(p as unknown as chrome.runtime.Port, () => {});
    p.onMessage.fire({ t: "watch-tab", tabId: 4 });
    expect(p.sent).toContainEqual({ t: "stream-status", up: true });
    h.tabs.fromHub(["tab:4"], { t: "status", up: false });
    expect(p.sent.at(-1)).toEqual({ t: "stream-status", up: false });
    expect(h.overlay.at(-1)).toEqual({ tabId: 4, m: { t: "stream-status", up: false } });
    h.tabs.fromHub(["tab:4"], { t: "status", up: true });
    expect(p.sent.at(-1)).toEqual({ t: "stream-status", up: true });
  });

  it("keeps each tab's hub client alive by answering the hub's pings", async () => {
    const h = harness();
    h.pages.set(URL1, { page: page(), route: null });
    await h.tabs.route(4, URL1);
    h.tabs.fromHub(["tab:4", "tab:9"], { t: "ping" });
    expect(h.hubIn.filter(x => x.msg.t === "ping")).toEqual([{ id: "tab:4", msg: { t: "ping" } }]);
  });

  it("asks the overlay to send its results again when the tab's record has none for shown threads, once", async () => {
    const store = memory();
    const first = harness(store);
    first.pages.set(URL1, { page: page(), route: null });
    first.threads.set(AID, [full(T1)]);
    await first.tabs.toggle(4, URL1);
    await settle();
    const h = harness(store, first.docs);
    h.pages.set(URL1, { page: page(), route: null });
    h.threads.set(AID, [full(T1)]);
    await h.tabs.ready();
    await h.tabs.fromOverlay(4, 1, { t: "ping" }, URL1);
    const resends = () => h.overlay.filter(o => o.tabId === 4 && o.m.t === "resend").length;
    expect(resends()).toBe(1);
    const last = h.overlay.filter(o => o.tabId === 4).at(-1)!.m;
    expect(last.t).toBe("resend");
    await h.tabs.fromOverlay(4, 1, { t: "resolved", results: [{ id: T1, found: true, method: "selector", rect: null }] }, URL1);
    await h.tabs.fromOverlay(4, 1, { t: "route", url: URL1 }, URL1);
    expect(resends()).toBe(1);
  });

  it("does not ask for results when the page shows no open thread or has no overlay", async () => {
    const h = harness();
    h.pages.set(URL1, { page: page(), route: null });
    await h.tabs.toggle(4, URL1);
    await h.tabs.fromOverlay(4, 1, { t: "route", url: URL1 }, URL1);
    h.threads.set(AID, [full(T1)]);
    await h.tabs.fromOverlay(5, 1, { t: "route", url: URL1 }, URL1);
    expect(h.overlay.some(o => o.m.t === "resend")).toBe(false);
  });

  it("rebuilds a tab after the worker restarts, from session storage, at the tab's next message", async () => {
    const store = memory();
    const first = harness(store);
    first.pages.set(URL1, { page: page(), route: null });
    await first.tabs.toggle(4, URL1);
    first.tabs.activate(4);
    await settle();
    const h = harness(store, first.docs);
    h.pages.set(URL1, { page: page(), route: null });
    h.threads.set(AID, [full(T1)]);
    await h.tabs.ready();
    expect(h.tabs.admits(4)).toBe(true);
    await h.tabs.fromOverlay(4, 1, { t: "ping" }, URL1);
    expect(h.tabs.state(4)).toMatchObject({ url: URL1, overlay: true, commentMode: true, page: page() });
    expect(h.tabs.state(4)?.threads.map(t => t.id)).toEqual([T1]);
    expect(h.hubIn).toContainEqual({ id: "tab:4", msg: { t: "topics", topics: [`artifact:${AID}`, `working:${AID}`] } });
    await h.tabs.toggle(4, URL1);
    expect(h.injected).toEqual([]);
    expect(h.tabs.state(4)?.commentMode).toBe(false);
    h.tabs.close(4);
    await settle();
    const after = harness(store);
    await after.tabs.ready();
    expect(after.tabs.admits(4)).toBe(false);
  });
});

describe("Tabs and side panels", () => {
  afterEach(() => vi.useRealTimers());
  const tabMsgs = (p: ReturnType<typeof port>) => p.sent.filter(m => m.t === "tab").map(m => (m as Extract<WorkerToPanel, { t: "tab" }>).state);

  it("follows the presence of the page a panel shows, through a hub client of its own", async () => {
    const h = harness();
    h.pages.set(URL1, { page: page(), route: null });
    await h.tabs.route(4, URL1);
    const p = port("panel:2");
    h.tabs.attachPanel(p as unknown as chrome.runtime.Port, () => {});
    p.onMessage.fire({ t: "watch-tab", tabId: 4 });
    await settle();
    const client = h.hubIn.find(x => x.id.startsWith("panel:"))!;
    expect(client.msg).toEqual({ t: "topics", topics: [`presence:${AID}`] });
    h.tabs.fromHub([client.id], { t: "live", topics: [`presence:${AID}`] });
    await settle();
    expect(h.calls).toContain(`presenceOf ${AID}`);
    expect(tabMsgs(p).at(-1)?.presence).toEqual([here("u_mia")]);
    h.tabs.fromHub([client.id], { t: "event", topic: `presence:${AID}`, name: "presence", data: { artifact_id: AID, people: [here("u_ana")], gone: ["u_mia"] } });
    expect(tabMsgs(p).at(-1)?.presence).toEqual([here("u_ana")]);
    h.tabs.fromHub([client.id], { t: "ping" });
    expect(h.hubIn.at(-1)).toEqual({ id: client.id, msg: { t: "ping" } });
    p.onDisconnect.fire();
    expect(h.detached).toContain(client.id);
  });

  it("reports the owner here every 30 s while a visible panel shows a live page, and never away", async () => {
    vi.useFakeTimers();
    const h = harness();
    h.pages.set(URL1, { page: page(), route: null });
    await h.tabs.route(4, URL1);
    const p = port("panel:2");
    h.tabs.attachPanel(p as unknown as chrome.runtime.Port, () => {});
    p.onMessage.fire({ t: "watch-tab", tabId: 4 });
    await vi.advanceTimersByTimeAsync(0);
    expect(h.calls.filter(c => c.startsWith("presence "))).toEqual([]);
    p.onMessage.fire({ t: "visible", on: true });
    await vi.advanceTimersByTimeAsync(0);
    expect(h.calls.filter(c => c.startsWith("presence "))).toEqual([`presence ${AID} 2`]);
    await vi.advanceTimersByTimeAsync(30_000);
    expect(h.calls.filter(c => c.startsWith("presence "))).toHaveLength(2);
    // Hidden: the report lapses; nothing says away.
    p.onMessage.fire({ t: "visible", on: false });
    await vi.advanceTimersByTimeAsync(90_000);
    expect(h.calls.filter(c => c.startsWith("presence "))).toHaveLength(2);
    p.onMessage.fire({ t: "visible", on: true });
    await vi.advanceTimersByTimeAsync(0);
    expect(h.calls.filter(c => c.startsWith("presence "))).toHaveLength(3);
    p.onDisconnect.fire();
    await vi.advanceTimersByTimeAsync(90_000);
    expect(h.calls.filter(c => c.startsWith("presence "))).toHaveLength(3);
  });

  it("reports nothing for a tab with no live page", async () => {
    vi.useFakeTimers();
    const h = harness();
    h.pages.set(URL1, { page: null, route: null });
    await h.tabs.route(4, URL1);
    const p = port("panel:2");
    h.tabs.attachPanel(p as unknown as chrome.runtime.Port, () => {});
    p.onMessage.fire({ t: "watch-tab", tabId: 4 });
    p.onMessage.fire({ t: "visible", on: true });
    await vi.advanceTimersByTimeAsync(60_000);
    expect(h.calls.filter(c => c.startsWith("presence"))).toEqual([]);
    expect(h.hubIn.some(x => x.id.startsWith("panel:"))).toBe(false);
  });

  it("selects a thread, pinning it in the overlay, and sets comment mode from the panel", async () => {
    const h = harness();
    h.pages.set(URL1, { page: page(), route: null });
    await h.tabs.toggle(4, URL1);
    await settle();
    h.tabs.select(4, T1);
    expect(h.tabs.state(4)?.selected).toBe(T1);
    expect(h.overlay.at(-1)).toEqual({ tabId: 4, m: { t: "scroll-to", threadId: T1 } });
    h.tabs.select(4, null);
    expect(h.overlay.at(-1)).toEqual({ tabId: 4, m: { t: "focus", threadId: null } });
    h.tabs.setCommentMode(4, false);
    expect(h.tabs.state(4)?.commentMode).toBe(false);
  });

  it("applies a thread an action answered with, and the owner's new name, to every panel", async () => {
    const h = harness();
    h.pages.set(URL1, { page: page(), route: null });
    h.threads.set(AID, [full(T1)]);
    await h.tabs.route(4, URL1);
    const p = port("panel:2");
    h.tabs.attachPanel(p as unknown as chrome.runtime.Port, () => {});
    p.onMessage.fire({ t: "watch-tab", tabId: 4 });
    await settle();
    h.tabs.applied(4, full(T1, { status: "resolved" }) as never);
    expect(h.tabs.state(4)?.threads[0].status).toBe("resolved");
    h.tabs.removed(4, T1);
    expect(h.tabs.state(4)?.threads).toEqual([]);
    h.tabs.setViewer({ public_id: "u_owner", display_name: "Mia", created_at: "t" });
    expect(tabMsgs(p).at(-1)?.viewer).toEqual({ public_id: "u_owner", display_name: "Mia" });
  });
});
