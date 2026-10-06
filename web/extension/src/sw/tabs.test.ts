import { afterEach, describe, expect, it, vi } from "vitest";
import type { HubMsg, TabMsg } from "../../../shell/src/stream-hub";
import type { PageView, SiteView, WorkerToOverlay, WorkerToPanel } from "../messages";
import { FakeEvent } from "../../test/fake-chrome";
import { type TabState, Tabs, applyEvent, emptyTab, type TabsApi, CLOSED_MS } from "./tabs";

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

type FakeSites = { view(o: string | null): SiteView | null; follow(os: Iterable<string>): void; fromHub(id: string, msg: HubMsg): void };
function harness(store = memory(), docs = documents(), sites?: FakeSites) {
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
    sites,
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

  it("tells the overlay each thread's ID, status, anchor and pending flag, and none of its text", async () => {
    const h = harness();
    h.pages.set(URL1, { page: page(), route: null });
    h.threads.set(AID, [full(T1, { anchor: { kind: "element", selector: "body", quote: "Hi", file: "index.html" }, addressed_pending: { harness: "claude", at: "t" }, last_comment: { body: "secret" } })]);
    await h.tabs.route(4, URL1);
    const m = h.overlay.at(-1)!.m as Extract<WorkerToOverlay, { t: "state" }>;
    expect(m.threads).toEqual([{ id: T1, status: "open", anchor: { kind: "element", selector: "body", quote: "Hi", file: "index.html" }, addressed_pending: true }]);
    const text = JSON.stringify(h.overlay);
    for (const word of ["secret", "comments", "author_name", "harness", "feedback_state", "last_comment"]) expect(text).not.toContain(word);
    // The panel still gets the whole thread.
    expect(h.tabs.panelState(4).threads[0]).toMatchObject({ comments: [expect.objectContaining({ body: "b" })] });
  });

  it("tells the overlay only of changes it shows, and the whole state again to a new overlay", async () => {
    const h = harness();
    h.pages.set(URL1, { page: page(), route: null });
    h.threads.set(AID, [full(T1)]);
    await h.tabs.route(4, URL1);
    const states = () => h.overlay.filter(o => o.m.t === "state").length;
    const before = states();
    h.tabs.fromHub(["tab:4"], { t: "event", topic: `working:${AID}`, name: "working", data: { artifact_id: AID, working: [{ key: "k", harness: "claude", message: null, thread_ids: [], started_at: "t", last_heartbeat: "t" }] } });
    h.tabs.select(4, null);
    expect(h.tabs.state(4)?.working).toHaveLength(1);
    expect(states()).toBe(before);
    h.tabs.setCommentMode(4, true);
    expect(states()).toBe(before + 1);
    // An overlay's start (its first route) hears the state even when nothing changed.
    await h.tabs.fromOverlay(4, 1, { t: "route", url: URL1 }, URL1);
    expect(states()).toBe(before + 2);
  });

  it("looks up no address over MAX_URL, and says why on the tab", async () => {
    const h = harness();
    h.pages.set(URL1, { page: page(), route: null });
    h.threads.set(AID, [full(T1)]);
    await h.tabs.route(4, URL1);
    const long = `${URL1}#${"x".repeat(4096)}`;
    for (const m of [{ t: "route", url: null }] as const) {
      h.calls.length = 0;
      await h.tabs.fromOverlay(4, 1, m, long);
      expect(h.calls).toEqual([]);
      expect(h.tabs.state(4)).toMatchObject({ url: "", page: null, threads: [], error: { code: "url_too_long", message: "This page's address is too long for Clax." } });
      expect(h.hubIn.at(-1)).toEqual({ id: "tab:4", msg: { t: "topics", topics: [] } });
    }
    h.calls.length = 0;
    await h.tabs.route(4, long);
    expect(h.calls).toEqual([]);
    expect(h.tabs.state(4)?.error?.code).toBe("url_too_long");
  });

  it("drops a presence probe answered after the overlay was injected again", async () => {
    const h = harness();
    h.pages.set(URL1, { page: page(), route: null });
    h.threads.set(AID, [full(T1)]);
    h.tabs.turnOn(4, URL1, "http://localhost:5173");
    await h.tabs.toggle(4, URL1);
    // A new document whose overlay came (a click) while a slow probe was out.
    h.reload(4);
    const probe = hold();
    const tabs = h.tabs as unknown as { d: { present(tabId: number): Promise<boolean> } };
    const present = tabs.d.present;
    tabs.d.present = async tabId => { const was = await present(tabId); await probe.p; return was; };
    const navigated = h.tabs.navigated(4, false);
    await settle();
    await h.tabs.toggle(4, URL1);
    expect(h.tabs.state(4)?.overlay).toBe(true);
    probe.release();
    await navigated;
    expect(h.tabs.state(4)?.overlay).toBe(true);
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

  it("injects the overlay again at a new document's load only in a tab Clax is on", async () => {
    const h = harness();
    h.pages.set(URL1, { page: page(), route: null });
    h.tabs.turnOn(4, URL1, "http://localhost:5173");
    await h.tabs.toggle(4, URL1);
    // Another tab of the same origin, and a new tab: Clax is not on there.
    await h.tabs.navigated(5, true);
    await h.tabs.navigated(6, true);
    expect(h.injected).toEqual([4]);
    expect(h.tabs.onOrigin(5)).toBeNull();
    expect(h.tabs.panelState(5).enabled).toBe(false);
    expect(h.tabs.onTabs()).toEqual([4]);
    // A reload of the tab Clax is on: the new document gets the overlay once loaded.
    h.reload(4);
    await h.tabs.navigated(4, false);
    expect(h.injected).toEqual([4]);
    await h.tabs.navigated(4, true);
    expect(h.injected).toEqual([4, 4]);
    expect(h.tabs.state(4)).toMatchObject({ overlay: true, commentMode: false, on: "http://localhost:5173" });
  });

  it("forgets a closed tab's ID after a while, and a turned-off tab's only when it is turned on again", async () => {
    let now = 0;
    const h = harness();
    (h.tabs as unknown as { d: { now(): number } }).d.now = () => now;
    const closed = () => [...(h.tabs as unknown as { closed: Map<number, number | null> }).closed.keys()].sort();
    h.tabs.close(4, true);
    h.tabs.close(5);
    expect(closed()).toEqual([4, 5]);
    now = CLOSED_MS + 1;
    h.tabs.close(6, true);
    expect(closed()).toEqual([5, 6]);
    h.tabs.turnOn(5, URL1, "http://localhost:5173");
    expect(closed()).toEqual([6]);
  });

  it("marks nothing injected, and leaves comment mode, when the tab's document is of another origin", async () => {
    const h = harness();
    h.pages.set(URL1, { page: page(), route: null });
    h.tabs.turnOn(4, URL1, "http://localhost:5173");
    (h.tabs as unknown as { d: { inject(): Promise<null> } }).d.inject = async () => null;
    await h.tabs.toggle(4, URL1);
    await h.tabs.navigated(4, true);
    expect(h.tabs.state(4)).toMatchObject({ overlay: false, commentMode: false });
  });

  it("writes no record for a tab turned off while an answer for it was in flight, until it is turned on again", async () => {
    const h = harness();
    const slow = hold();
    h.gates.set(URL1, slow.p);
    h.pages.set(URL1, { page: page(), route: null });
    h.tabs.turnOn(4, URL1, "http://localhost:5173");
    const routed = h.tabs.route(4, URL1);
    h.tabs.close(4);
    slow.release();
    await routed;
    expect(h.tabs.state(4)).toBeUndefined();
    expect(h.tabs.onOrigin(4)).toBeNull();
    h.tabs.turnOn(4, URL1, "http://localhost:5173");
    expect(h.tabs.onOrigin(4)).toBe("http://localhost:5173");
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

  it("treats a page load as a new document: comment mode off, and the overlay injected again", async () => {
    const h = harness();
    h.pages.set(URL1, { page: page(), route: null });
    h.threads.set(AID, [full(T1)]);
    h.tabs.turnOn(4, URL1, "http://localhost:5173");
    await h.tabs.toggle(4, URL1);
    expect(h.tabs.state(4)?.commentMode).toBe(true);
    h.reload(4);
    await h.tabs.navigated(4, true);
    expect(h.injected).toEqual([4, 4]);
    expect(h.tabs.state(4)).toMatchObject({ overlay: true, commentMode: false, selected: null, resolved: {} });
  });

  it("after a real load, forgets the overlay, comment mode and the click's grant", async () => {
    const h = harness();
    h.pages.set(URL1, { page: page(), route: null });
    h.tabs.turnOn(4, URL1, "http://localhost:5173");
    await h.tabs.toggle(4, URL1);
    h.reload(4);
    await h.tabs.navigated(4, false);
    expect(h.tabs.state(4)).toMatchObject({ overlay: false, commentMode: false, active: false });
    expect(h.tabs.admits(4)).toBe(false);
    h.tabs.activate(4, URL1);
    await h.tabs.toggle(4, URL1);
    expect(h.injected).toEqual([4, 4]);
    expect(h.tabs.state(4)?.commentMode).toBe(true);
  });

  it("keeps comment mode, the overlay and the click's grant through an in-page route change", async () => {
    const h = harness();
    h.pages.set(URL1, { page: page(), route: null });
    h.pages.set("http://localhost:5173/settings", { page: page(AID, "/settings"), route: null });
    h.tabs.turnOn(4, URL1, "http://localhost:5173");
    await h.tabs.toggle(4, URL1);
    // Chrome reports the in-page navigation as loading; the document (and its overlay) stays.
    await h.tabs.navigated(4, false);
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

  it("tells the panels whether the stream is up, and the overlays nothing of it", async () => {
    const h = harness();
    h.pages.set(URL1, { page: page(), route: null });
    await h.tabs.route(4, URL1);
    const p = port();
    h.tabs.attachPanel(p as unknown as chrome.runtime.Port, () => {});
    p.onMessage.fire({ t: "watch-tab", tabId: 4 });
    expect(p.sent).toContainEqual({ t: "stream-status", up: true });
    h.tabs.fromHub(["tab:4"], { t: "status", up: false });
    expect(p.sent.at(-1)).toEqual({ t: "stream-status", up: false });
    expect(h.overlay.map(o => o.m.t)).not.toContain("stream-status");
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
    first.tabs.turnOn(4, URL1, "http://localhost:5173");
    await first.tabs.toggle(4, URL1);
    await settle();
    const h = harness(store, first.docs);
    h.pages.set(URL1, { page: page(), route: null });
    h.threads.set(AID, [full(T1)]);
    await h.tabs.ready();
    expect(h.tabs.admits(4)).toBe(true);
    expect(h.tabs.onOrigin(4)).toBe("http://localhost:5173");
    expect(h.tabs.onTabs()).toEqual([4]);
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
    expect(after.tabs.onTabs()).toEqual([]);
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

  it("reports the owner here in the window the panel's tab is in now, after the tab moved to another", async () => {
    const h = harness();
    h.pages.set(URL1, { page: page(), route: null });
    await h.tabs.route(4, URL1);
    (h.tabs as unknown as { d: { windowOf(tabId: number): Promise<number> } }).d.windowOf = async tabId => (tabId === 4 ? 9 : 1);
    const p = port("panel:2");
    h.tabs.attachPanel(p as unknown as chrome.runtime.Port, () => {});
    p.onMessage.fire({ t: "watch-tab", tabId: 4 });
    p.onMessage.fire({ t: "visible", on: true });
    await settle();
    expect(h.calls.filter(c => c.startsWith("presence "))).toEqual([`presence ${AID} 9`]);
    p.onDisconnect.fire();
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
    h.tabs.setViewer({ public_id: "u_owner", display_name: "Mia", created_at: "t" });
    expect(tabMsgs(p).at(-1)?.viewer).toEqual({ public_id: "u_owner", display_name: "Mia" });
  });

  it("scrolls to a thread on another route once the overlay finds it there, across a new document", async () => {
    const h = harness();
    const URL2 = "http://localhost:5173/?tab=billing";
    h.pages.set(URL1, { page: page(), route: null });
    h.pages.set(URL2, { page: page(), route: "?tab=billing" });
    h.threads.set(AID, [full(T1, { anchor: { kind: "element", selector: "body", file: "index.html", route: "?tab=billing" } })]);
    h.tabs.turnOn(4, URL1, "http://localhost:5173");
    await h.tabs.toggle(4, URL1);
    await settle();
    h.tabs.select(4, T1);
    expect(h.overlay.some(o => o.m.t === "scroll-to")).toBe(false);
    // The navigation loads a new document, whose overlay re-resolves on the thread's route.
    h.reload(4);
    await h.tabs.navigated(4, true);
    await h.tabs.fromOverlay(4, 1, { t: "route", url: URL2 }, URL2);
    await h.tabs.fromOverlay(4, 1, { t: "resolved", results: [{ id: T1, found: false, method: null, rect: null }] }, URL2);
    expect(h.overlay.some(o => o.m.t === "scroll-to")).toBe(false);
    await h.tabs.fromOverlay(4, 1, { t: "resolved", results: [{ id: T1, found: true, method: "exact", rect: null }] }, URL2);
    expect(h.overlay.at(-1)).toEqual({ tabId: 4, m: { t: "scroll-to", threadId: T1 } });
    expect(h.tabs.state(4)?.selected).toBe(T1);
    await h.tabs.fromOverlay(4, 1, { t: "resolved", results: [{ id: T1, found: true, method: "exact", rect: null }] }, URL2);
    expect(h.overlay.filter(o => o.m.t === "scroll-to")).toHaveLength(1);
  });

  it("refuses a panel port whose name names no window", () => {
    const h = harness();
    const p = { ...port("panel:x"), disconnect: vi.fn() };
    h.tabs.attachPanel(p as unknown as chrome.runtime.Port, () => {});
    expect(p.disconnect).toHaveBeenCalled();
    expect(p.onMessage.listeners).toHaveLength(0);
  });
});


describe("Tabs and the site's threads", () => {
  const O = "http://localhost:5173";
  const T2 = "01J9BBBBBBBBBBBBBBBBBBBBBB";
  const T3 = "01J9CCCCCCCCCCCCCCCCCCCCCC";
  const other = (id: string, extra: object = {}) => ({ ...full(id, { artifact_id: AID2, anchor: { kind: "element", selector: "h1", file: "index.html", route: "?x" }, page_path: "/users/7", page_url: `${O}/users/7?x`, addressed_pending: { harness: "claude", at: "t" }, ...extra }) });
  function sites() {
    let view: SiteView | null = null;
    const followed: string[][] = [];
    const heard: [string, HubMsg][] = [];
    return {
      followed, heard,
      set(v: SiteView | null) { view = v; },
      view: (o: string | null) => (o === O ? view : null),
      follow: (os: Iterable<string>) => { followed.push([...new Set(os)]); },
      fromHub: (id: string, msg: HubMsg) => { heard.push([id, msg]); },
    };
  }
  const listing = (): SiteView => ({ origin: O, rules: [], pages: [
    { page: page(), threads: [full(T1) as never] },
    { page: page(AID2, "/users/7"), threads: [other(T2) as never, other(T3, { status: "resolved" }) as never] },
  ] });

  it("follows the sites of the origins it is on for, once the daemon answered, and hands their hub messages on", async () => {
    const f = sites();
    const h = harness(memory(), documents(), f);
    h.pages.set(URL1, { page: null, route: null });
    h.tabs.turnOn(4, URL1, O);
    h.tabs.turnOn(5, URL1, O);
    expect(f.followed.flat()).toEqual([]);
    await h.tabs.route(4, URL1);
    await h.tabs.route(5, URL1);
    expect(f.followed.at(-1)).toEqual([O]);
    h.tabs.fromHub([`site:${O}`], { t: "ping" });
    expect(f.heard).toEqual([[`site:${O}`, { t: "ping" }]]);
    h.tabs.close(4);
    expect(f.followed.at(-1)).toEqual([O]);
    h.tabs.close(5);
    expect(f.followed.at(-1)).toEqual([]);
  });

  it("pins the site's open threads of other pages, after the page's own, with the path they were left at and no text", async () => {
    const f = sites();
    const h = harness(memory(), documents(), f);
    h.pages.set(URL1, { page: page(), route: null });
    h.threads.set(AID, [full(T1)]);
    h.tabs.turnOn(4, URL1, O);
    await h.tabs.route(4, URL1);
    f.set(listing());
    h.tabs.siteChanged(O);
    const m = h.overlay.at(-1)!.m as Extract<WorkerToOverlay, { t: "state" }>;
    expect(m.threads.map(t => [t.id, t.from])).toEqual([[T1, undefined], [T2, "/users/7"]]);
    // Its pending address is its own page's to settle: this page's snapshot does not wait for it.
    expect(m.threads[1]).toEqual({ id: T2, status: "open", anchor: { kind: "element", selector: "h1", file: "index.html", route: "?x" }, addressed_pending: false, from: "/users/7" });
    expect(m.pending).toBe(false);
    expect(JSON.stringify(m.threads)).not.toMatch(/page_url|comments|body/);
  });

  it("tells each panel of the tab's site when it watches, and again when the site changes", async () => {
    const f = sites();
    const h = harness(memory(), documents(), f);
    f.set(listing());
    h.pages.set(URL1, { page: null, route: null });
    h.tabs.turnOn(4, URL1, O);
    await h.tabs.route(4, URL1);
    const p = port();
    h.tabs.attachPanel(p as unknown as chrome.runtime.Port, () => {});
    p.onMessage.fire({ t: "watch-tab", tabId: 4 });
    expect(p.sent.filter(m => m.t === "site")).toEqual([{ t: "site", site: listing() }]);
    f.set(null);
    h.tabs.siteChanged("http://elsewhere.test");
    h.tabs.siteChanged(O);
    expect(p.sent.filter(m => m.t === "site")).toHaveLength(2);
  });

  it("opens a thread of another page: the tab's URL for it, then the overlay scrolls to it once found", async () => {
    const f = sites();
    const h = harness(memory(), documents(), f);
    f.set(listing());
    h.pages.set(URL1, { page: page(), route: null });
    h.tabs.turnOn(4, URL1, O);
    await h.tabs.route(4, URL1);
    expect(h.tabs.openThread(4, T2)).toBe(`${O}/users/7?x`);
    expect(h.tabs.openThread(4, "01J9DDDDDDDDDDDDDDDDDDDDDD")).toBeNull();
    await h.tabs.fromOverlay(4, 1, { t: "resolved", results: [{ id: T2, found: true, method: "selector", rect: null }] });
    expect(h.overlay.at(-1)).toEqual({ tabId: 4, m: { t: "scroll-to", threadId: T2 } });
    expect(h.tabs.state(4)?.selected).toBe(T2);
  });
});
