import { describe, expect, it } from "vitest";
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

function harness() {
  const calls: string[] = [];
  const hubIn: { id: string; msg: TabMsg }[] = [];
  const detached: string[] = [];
  const overlay: { tabId: number; m: WorkerToOverlay }[] = [];
  const injected: number[] = [];
  const pages = new Map<string, { page: PageView | null; route: string | null }>();
  const threads = new Map<string, unknown[]>();
  const gates = new Map<string, Promise<void>>();
  const api: TabsApi = {
    lookup: async (url: string) => { calls.push(`lookup ${url}`); await gates.get(url); const r = pages.get(url); if (!r) throw Object.assign(new Error("down"), { code: "daemon_unreachable" }); return r; },
    threads: async (aid: string) => { calls.push(`threads ${aid}`); return (threads.get(aid) ?? []) as never; },
    thread: async (aid: string, tid: string) => { calls.push(`thread ${aid} ${tid}`); return { thread: full(tid, { comments: [] }) as never }; },
    working: async (aid: string) => { calls.push(`working ${aid}`); return []; },
    artifact: async (aid: string) => { calls.push(`artifact ${aid}`); return { artifact: { id: aid, participants: { people: [], agents: [{ handle: "a_1", harness: "claude", live: true }] } }, versions: [{ artifact_id: aid, n: 1 }] } as never; },
    me: async () => { calls.push("me"); return { viewer: { public_id: "u_owner", display_name: "Alex", created_at: "t" } }; },
  };
  const tabs = new Tabs({
    api,
    hub: { receive: (id, msg) => hubIn.push({ id, msg }), detach: id => detached.push(id) },
    toOverlay: (tabId, m) => overlay.push({ tabId, m }),
    inject: async tabId => { injected.push(tabId); },
  });
  return { tabs, calls, hubIn, detached, overlay, injected, pages, threads, gates };
}

function port(name = "panel:1") {
  const sent: WorkerToPanel[] = [];
  const onMessage = new FakeEvent<[unknown]>();
  const onDisconnect = new FakeEvent<[]>();
  return { name, sent, onMessage, onDisconnect, postMessage: (m: WorkerToPanel) => sent.push(m) };
}
const settle = async () => { for (let i = 0; i < 20; i++) await Promise.resolve(); };

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
});
