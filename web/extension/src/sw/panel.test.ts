import { describe, expect, it } from "vitest";
import type { PanelToWorker, WorkerToPanel } from "../messages";
import { ApiFailure } from "./api";
import { type PanelDeps, panelAction } from "./panel";

const AID = "7q3k9mzx2b4t";
const T1 = "01J9AAAAAAAAAAAAAAAAAAAAAA";
const T2 = "01J9BBBBBBBBBBBBBBBBBBBBBB";
const URL1 = "http://localhost:5173/app";
const page = { artifact_id: AID, origin: "http://localhost:5173", path: "/app", page_url: URL1, title: "T", current_version: 1, url: `http://localhost:7480/a/${AID}` };
const thread = (id: string, status = "open") => ({ id, artifact_id: AID, status });
const FAR_AID = "9x8w7v6t5s4r";
const T3 = "01J9CCCCCCCCCCCCCCCCCCCCCC";

const Q = "01J9QQQQQQQQQQQQQQQQQQQQQQ";
const SITE = { key: "http://localhost:5173", name: "http://localhost:5174", joined: true, origins: [{ origin: "http://localhost:5174", joined_at: "t", last_used_at: "t2" }, { origin: "http://localhost:5173", joined_at: "t", last_used_at: "t1" }] };
function setup(over: { closed?: boolean; remaining?: number; page?: typeof page | null; admits?: boolean; allUrls?: boolean; fail?: string; failCode?: string; error?: string; site?: string[]; down?: string[]; denied?: string[]; loading?: boolean; restoring?: Promise<void>;
  /** The threads of another page of the tab's site, in its listing. */
  far?: { id: string; artifact_id: string; clip_url: string | null }[] } = {}) {
  const calls: string[] = [];
  const out: WorkerToPanel[] = [];
  let waited = 0;
  const pg = over.page === undefined ? page : over.page;
  const api = (name: string, ret: unknown) => async (...a: unknown[]) => {
    calls.push(`${name} ${a.map(x => JSON.stringify(x)).join(" ")}`);
    if (over.fail === name) throw new ApiFailure(over.failCode ?? "not_found", "No such thread.", 404);
    // A thread's answer is the thread asked about.
    const r = ret as { thread?: { id: string } };
    return r?.thread && typeof a[1] === "string" ? { ...r, thread: { ...r.thread, id: a[1] } } : ret;
  };
  const d: PanelDeps = {
    api: {
      sendThread: api("sendThread", { thread: thread(T1) }), sendBatch: api("sendBatch", { threads: [thread(T1), thread(T2)] }),
      comment: api("comment", { thread: thread(T1) }), resolve: api("resolve", { thread: thread(T1, "resolved") }),
      reopen: api("reopen", { thread: thread(T1) }), looked: api("looked", {}),
      setName: api("setName", { viewer: { public_id: "u_o", display_name: "Mia", created_at: "t" } }),
      move: api("move", { moved: true }),
      addRule: api("addRule", { rule: {}, moved: [T1, T2], remaining: over.remaining ?? 0 }),
      deleteRule: api("deleteRule", { moved: [T1], remaining: over.remaining ?? 0 }),
      join: api("join", { site: SITE, moved: [T1], remaining: over.remaining ?? 0 }),
      split: api("split", { split: true, site: SITE }),
      answer: api("answer", {}),
      suggest: api("suggest", { origin: "http://localhost:5173", site: SITE, suggestions: [{ origin: "http://localhost:7702", site: { ...SITE, origins: [{ origin: "http://localhost:7702", joined_at: null, last_used_at: null }] }, reason: "path", path: "/app" }] }),
      sites: api("sites", { sites: [{ site: SITE }] }),
      clip: api("clip", "data:image/png;base64,iVBORw0KGgo="),
      artifact: api("artifact", { artifact: { participants: { people: [], agents: [{ handle: `a_${"1".repeat(22)}`, harness: "claude", live: true }] } }, versions: [{ artifact_id: "x", n: 1, created_at: "t", label: "l", note: "n", files: { "index.html": {} }, agent_harness: "claude" }] }),
      closeQuestion: api("closeQuestion", { question: { id: Q }, closed: over.closed ?? false }),
      markItem: api("markItem", { item: { id: T1, read: true }, unread: 3 }),
      markItems: api("markItems", { marked: 2, unread: 1 }),
    } as never,
    inbox: {
      page: async (f: unknown, before: string | null) => { calls.push(`page ${JSON.stringify(f)} ${before}`); return { items: [], next_cursor: null, unread: 4 }; },
      count: (n: number) => { calls.push(`count ${n}`); },
    },
    open: {
      daemon: async () => "http://127.0.0.1:7480",
      create: async (url: string) => { calls.push(`create ${url}`); },
      focus: async (tabId: number) => { calls.push(`focus ${tabId}`); },
      find: async (url: string) => { calls.push(`find ${url}`); return url === "http://localhost:5173/other" ? 12 : null; },
    },
    tabs: {
      ready: async () => { waited++; await over.restoring; },
      commentOn: async (tabId: number) => {
        if (over.loading) throw Object.assign(new Error("The page was still loading. Try again."), { code: "page_loading" });
        calls.push(`comment-on ${tabId}`);
      },
      state: (tabId: number) => (pg === null || tabId === 9 ? undefined : { url: URL1, page: pg, threads: [thread(T1)], overlay: true, on: "http://localhost:5173", error: over.error ? { code: over.error, message: "m" } : null } as never),
      admits: () => over.admits ?? false,
      route: async (tabId: number, url: string, fresh?: boolean) => { calls.push(`route ${tabId} ${url} ${!!fresh}`); return {} as never; },
      applied: (tabId: number, t: { id: string }) => calls.push(`applied ${tabId} ${t.id}`),
      fail: (tabId: number, e: { code: string }) => calls.push(`fail ${tabId} ${e.code}`),
      setViewer: (v: { display_name: string }) => calls.push(`viewer ${v.display_name}`),
      select: (tabId: number, id: string | null) => calls.push(`select ${tabId} ${id}`),
      setCommentMode: (tabId: number, on: boolean) => calls.push(`comment-mode ${tabId} ${on}`),
      onTabs: () => [9, 4],
      openThread: (tabId: number, id: string) => { calls.push(`open ${tabId} ${id}`); return id === T1 ? { path: "/users/7?x#/y", origins: over.site ?? ["http://localhost:5173"] } : null; },
      opening: (tabId: number, id: string, url: string) => { calls.push(`opening ${tabId} ${id} ${url}`); },
    } as never,
    sites: {
      load: async (o: string) => { calls.push(`load ${o}`); }, origins: (o: string) => over.site ?? [o],
      view: (o: string | null) => (o === "http://localhost:5173" && over.far ? { origin: o, rules: [], pages: [{ page: { ...page, artifact_id: FAR_AID, path: "/users/7" }, threads: over.far }] } as never : null),
      applied: (o: string, t: { id: string; status: string }) => calls.push(`site-applied ${o} ${t.id} ${t.status}`),
    },
    probe: async (url: string) => { calls.push(`probe ${url}`); return !(over.down ?? []).some(x => url.startsWith(x)); },
    title: async () => "My App",
    allowed: async (o: string) => !(over.denied ?? []).includes(o),
    pairer: { pair: async (retry?: boolean) => { calls.push(`pair ${!!retry}`); return {} as never; } },
    allUrls: async () => over.allUrls ?? false,
    navigate: async (tabId, url) => { calls.push(`navigate ${tabId} ${url}`); },
    turnOff: async tabId => { calls.push(`turn-off ${tabId}`); },
  };
  const run = (m: PanelToWorker, tabId: number | null = 4) => panelAction(d, tabId, m, r => out.push(r));
  return { calls, out, run, d, waited: () => waited };
}

describe("panelAction on the tab's site", () => {
  it("acts on a thread of another page of the site at its own page, and applies the answer to the site's listing", async () => {
    const s = setup({ far: [{ id: T3, artifact_id: FAR_AID, clip_url: null }] });
    await s.run({ t: "reply", threadId: T3, body: "@agent look" });
    await s.run({ t: "resolve", threadId: T3 });
    await s.run({ t: "reopen", threadId: T3 });
    await s.run({ t: "send", threadId: T3, to: null });
    expect(s.calls).toEqual([
      `comment "${FAR_AID}" "${T3}" "@agent look"`, `site-applied http://localhost:5173 ${T3} open`,
      `resolve "${FAR_AID}" "${T3}"`, `site-applied http://localhost:5173 ${T3} resolved`,
      `reopen "${FAR_AID}" "${T3}"`, `site-applied http://localhost:5173 ${T3} open`,
      `sendThread "${FAR_AID}" "${T3}" null`, `site-applied http://localhost:5173 ${T3} open`,
    ]);
    expect(s.out).toEqual([]);
    // A thread of the tab's page is still acted on there.
    const here = setup({ far: [{ id: T3, artifact_id: FAR_AID, clip_url: null }] });
    await here.run({ t: "resolve", threadId: T1 });
    expect(here.calls).toEqual([`resolve "${AID}" "${T1}"`, `applied 4 ${T1}`]);
  });

  it("marks another page's threads looked at their own page, and the tab's page's at the tab's", async () => {
    const s = setup({ far: [{ id: T3, artifact_id: FAR_AID, clip_url: null }] });
    await s.run({ t: "looked", threadIds: [T3] });
    await s.run({ t: "looked", threadIds: [T1] });
    expect(s.calls).toEqual([`looked "${FAR_AID}" ["${T3}"]`, `looked "${AID}" ["${T1}"]`]);
  });

  it("tells the panel the live agents and versions of another page of the site, and null for a thread it does not list", async () => {
    const s = setup({ far: [{ id: T3, artifact_id: FAR_AID, clip_url: null }] });
    await s.run({ t: "far-page", req: 7, threadId: T3 });
    await s.run({ t: "far-page", req: 8, threadId: T1 });
    expect(s.calls).toEqual([`artifact "${FAR_AID}"`]);
    expect(s.out).toEqual([
      { t: "far-page", req: 7, page: { artifactId: FAR_AID, agents: [{ handle: `a_${"1".repeat(22)}`, harness: "claude", live: true }], versions: [{ n: 1, created_at: "t", agent_harness: "claude" }] } },
      { t: "far-page", req: 8, page: null },
    ]);
  });

  it("fetches a thread's clip for the panel, of the tab's page or another page of its site, and answers null for one without", async () => {
    const s = setup({ far: [{ id: T3, artifact_id: FAR_AID, clip_url: `/api/artifacts/${FAR_AID}/threads/${T3}/clip` }, { id: T2, artifact_id: FAR_AID, clip_url: null }] });
    await s.run({ t: "clip", req: 1, threadId: T3 });
    await s.run({ t: "clip", req: 2, threadId: T2 });
    await s.run({ t: "clip", req: 3, threadId: "01J9DDDDDDDDDDDDDDDDDDDDDD" });
    expect(s.calls).toEqual([`clip "${FAR_AID}" "${T3}"`]);
    expect(s.out).toEqual([
      { t: "clip", req: 1, url: "data:image/png;base64,iVBORw0KGgo=" }, { t: "clip", req: 2, url: null }, { t: "clip", req: 3, url: null },
    ]);
    const failing = setup({ fail: "clip", far: [{ id: T3, artifact_id: FAR_AID, clip_url: "/x" }] });
    await failing.run({ t: "clip", req: 4, threadId: T3 });
    expect(failing.out).toEqual([{ t: "clip", req: 4, url: null }]);
  });

  it("opens a thread of another page in the tab, and refuses one the site does not have", async () => {
    const s = setup();
    await s.run({ t: "open-thread", threadId: T1 });
    expect(s.calls).toEqual([`open 4 ${T1}`, "probe http://localhost:5173/users/7?x#/y", `opening 4 ${T1} http://localhost:5173/users/7?x#/y`, "navigate 4 http://localhost:5173/users/7?x#/y"]);
    await s.run({ t: "open-thread", threadId: T2 });
    expect(s.out).toEqual([{ t: "failed", code: "not_found", message: "That thread is not on this site any more." }]);
  });

  it("opens a thread of a joined site on its most recently used address that answers, and says so when none does", async () => {
    const both = ["http://localhost:5174", "http://localhost:5173"];
    const s = setup({ site: both, down: ["http://localhost:5174"] });
    await s.run({ t: "open-thread", threadId: T1 });
    expect(s.calls).toEqual(["load http://localhost:5173", `open 4 ${T1}`, "probe http://localhost:5174/users/7?x#/y", "probe http://localhost:5173/users/7?x#/y", `opening 4 ${T1} http://localhost:5173/users/7?x#/y`, "navigate 4 http://localhost:5173/users/7?x#/y"]);
    const none = setup({ site: both, down: both });
    await none.run({ t: "open-thread", threadId: T1 });
    expect(none.calls.filter(c => c.startsWith("navigate"))).toEqual([]);
    expect(none.out).toEqual([{ t: "failed", code: "site_unreachable", message: "No address of this site answers (localhost:5174, localhost:5173). Start its server, then try again." }]);
  });

  it("opens a joined site's thread only on an address Chrome lets Clax into, besides the tab's own", async () => {
    const s = setup({ site: ["http://localhost:5174", "http://localhost:5173"], denied: ["http://localhost:5174"] });
    await s.run({ t: "open-thread", threadId: T1 });
    expect(s.calls.filter(c => c.startsWith("probe") || c.startsWith("navigate"))).toEqual(["probe http://localhost:5173/users/7?x#/y", "navigate 4 http://localhost:5173/users/7?x#/y"]);
  });

  it("moves a thread to a page of another origin of the tab's joined site", async () => {
    const s = setup({ site: ["http://localhost:5174", "http://localhost:5173"] });
    await s.run({ t: "move", threadId: T1, pageUrl: "http://localhost:5174/users/7" });
    expect(s.calls).toEqual([`move "${T1}" "http://localhost:5174/users/7"`]);
  });

  it("joins the tab's origin to another site a batch at a time, and refuses one for another tab's origin", async () => {
    const s = setup({ remaining: 2 });
    await s.run({ t: "join", req: 3, origin: "http://localhost:5173", with: "http://localhost:7702" });
    expect(s.calls).toEqual([`join "http://localhost:5173" "http://localhost:7702"`, "load http://localhost:5173"]);
    expect(s.out).toEqual([{ t: "step", req: 3, moved: 1, remaining: 2 }]);
    const done = setup();
    await done.run({ t: "join", req: 4, origin: "http://localhost:5173", with: "http://localhost:7702" });
    expect(done.calls).toContain(`route 4 ${URL1} true`);
    const other = setup();
    await other.run({ t: "join", req: 5, origin: "http://localhost:9", with: "http://localhost:7702" });
    expect(other.calls).toEqual([]);
    expect(other.out[0]).toMatchObject({ t: "failed", code: "page_changed", req: 5 });
  });

  it("splits an origin of the tab's site off it, and no other", async () => {
    const s = setup({ site: ["http://localhost:5174", "http://localhost:5173"] });
    await s.run({ t: "split", req: 6, origin: "http://localhost:5174" });
    expect(s.calls).toEqual([`split "http://localhost:5174"`, "load http://localhost:5173", `route 4 ${URL1} true`]);
    expect(s.out).toEqual([{ t: "step", req: 6, moved: 0, remaining: 0 }]);
    const other = setup();
    await other.run({ t: "split", req: 7, origin: "http://localhost:9" });
    expect(other.calls).toEqual([]);
  });

  it("asks for a suggestion with the tab's URL and title, answers it, and lists the sites", async () => {
    const s = setup();
    await s.run({ t: "suggest" });
    await s.run({ t: "answer", with: "http://localhost:7702", answer: "never" });
    await s.run({ t: "list-sites" });
    expect(s.calls).toEqual([`suggest "${URL1}" "My App"`, `answer "http://localhost:5173" "http://localhost:7702" "never"`, "sites "]);
    expect(s.out).toEqual([
      { t: "suggestion", origin: "http://localhost:5173", suggestion: { origin: "http://localhost:7702", origins: ["http://localhost:7702"], reason: "path", path: "/app" } },
      { t: "suggestion", origin: "http://localhost:5173", suggestion: null },
      { t: "sites", sites: [{ key: SITE.key, name: SITE.name, origins: ["http://localhost:5174", "http://localhost:5173"] }] },
    ]);
    // A tab Clax is off in asks nothing.
    const off = setup({ page: null });
    await off.run({ t: "suggest" }, null);
    expect(off.calls).toEqual([]);
  });

  it("moves a thread to a page of the tab's origin only", async () => {
    const s = setup();
    await s.run({ t: "move", threadId: T1, pageUrl: "http://localhost:5173/users/7" });
    expect(s.calls).toEqual([`move "${T1}" "http://localhost:5173/users/7"`]);
    await s.run({ t: "move", threadId: T1, pageUrl: "http://evil.test/users/7" });
    expect(s.out).toEqual([{ t: "failed", code: "cross_origin", message: "That page is on another site." }]);
    expect(s.calls).toHaveLength(1);
  });

  it("adds and deletes a rule one batch at a time for the tab's origin, telling the panel what is left", async () => {
    const s = setup({ remaining: 3 });
    await s.run({ t: "rule", req: 7, origin: "http://localhost:5173", pattern: "/users/:id" });
    await s.run({ t: "unrule", req: 8, origin: "http://localhost:5173", ruleId: T2 });
    expect(s.calls).toEqual([`addRule "http://localhost:5173" "/users/:id"`, "load http://localhost:5173", `deleteRule "${T2}"`, "load http://localhost:5173"]);
    expect(s.out).toEqual([{ t: "step", req: 7, moved: 2, remaining: 3 }, { t: "step", req: 8, moved: 1, remaining: 3 }]);
    // The last batch looks the tab's page up again: a merge may have changed which page its URL names.
    const done = setup();
    await done.run({ t: "rule", req: 9, origin: "http://localhost:5173", pattern: "/users/:id" });
    expect(done.calls).toContain("route 4 http://localhost:5173/app true");
  });

  it("names the request a rule's failure answers", async () => {
    const s = setup({ fail: "addRule", failCode: "invalid_pattern" });
    await s.run({ t: "rule", req: 5, origin: "http://localhost:5173", pattern: "/:a/:b" });
    expect(s.out).toEqual([{ t: "failed", code: "invalid_pattern", message: "No such thread.", req: 5 }]);
  });

  it("refuses a batch for another site than the tab's (the panel now follows another tab), writing nothing", async () => {
    const s = setup({ remaining: 3 });
    await s.run({ t: "rule", req: 3, origin: "http://other.test:8080", pattern: "/users/:id" });
    await s.run({ t: "unrule", req: 4, origin: "http://other.test:8080", ruleId: T2 });
    expect(s.calls).toEqual([]);
    expect(s.out).toEqual([
      { t: "failed", code: "page_changed", message: "The tab shows another page now.", req: 3 },
      { t: "failed", code: "page_changed", message: "The tab shows another page now.", req: 4 },
    ]);
  });

  it("answers every request, even with no tab or a tab Clax is off in", async () => {
    const s = setup({ page: null });
    await s.run({ t: "rule", req: 1, origin: "http://localhost:5173", pattern: "/users/:id" }, null);
    await s.run({ t: "rule", req: 2, origin: "http://localhost:5173", pattern: "/users/:id" });
    expect(s.out.map(o => [o.t, (o as { code?: string }).code, (o as { req?: number }).req])).toEqual([["failed", "no_tab", 1], ["failed", "page_changed", 2]]);
  });
});

describe("panelAction", () => {
  it("sends, replies, resolves, and reopens on the tab's live page, applying the answer", async () => {
    const s = setup();
    await s.run({ t: "send", threadId: T1, to: null });
    await s.run({ t: "send-batch", threadIds: [T1, T2], note: "both", to: "a_0123456789abcdef012345" });
    await s.run({ t: "reply", threadId: T1, body: "ok" });
    await s.run({ t: "resolve", threadId: T1 });
    await s.run({ t: "reopen", threadId: T1 });
    await s.run({ t: "looked", threadIds: [T1] });
    expect(s.calls).toEqual([
      `sendThread "${AID}" "${T1}" null`, `applied 4 ${T1}`,
      `sendBatch "${AID}" ["${T1}","${T2}"] "both" "a_0123456789abcdef012345"`, `applied 4 ${T1}`, `applied 4 ${T2}`,
      `comment "${AID}" "${T1}" "ok"`, `applied 4 ${T1}`,
      `resolve "${AID}" "${T1}"`, `applied 4 ${T1}`,
      `reopen "${AID}" "${T1}"`, `applied 4 ${T1}`,
      `looked "${AID}" ["${T1}"]`,
    ]);
    expect(s.out).toEqual([]);
  });

  it("tells the panel why an action failed", async () => {
    const s = setup({ fail: "resolve" });
    await s.run({ t: "resolve", threadId: T1 });
    expect(s.out).toEqual([{ t: "failed", code: "not_found", message: "No such thread." }]);
    const none = setup({ page: null });
    await none.run({ t: "send", threadId: T1, to: null });
    expect(none.out).toEqual([{ t: "failed", code: "no_page", message: "This tab shows no live page." }]);
    await none.run({ t: "send", threadId: T1, to: null }, null);
    expect(none.calls).toEqual([]);
  });

  it("keeps a failure a new pairing can fix as the tab's error, so Retry pairs again", async () => {
    const s = setup({ fail: "sendThread", failCode: "daemon_unreachable" });
    await s.run({ t: "send", threadId: T1, to: null });
    expect(s.calls).toEqual([`sendThread "${AID}" "${T1}" null`, "fail 4 daemon_unreachable"]);
    const other = setup({ fail: "sendThread" });
    await other.run({ t: "send", threadId: T1, to: null });
    expect(other.calls).toEqual([`sendThread "${AID}" "${T1}" null`]);
  });

  it("sets the owner's name, shared by every panel", async () => {
    const s = setup({ page: null });
    await s.run({ t: "set-name", name: "Mia" });
    expect(s.calls).toEqual([`setName "Mia"`, "viewer Mia"]);
  });

  it("refuses comment mode without a way to take the screenshot", async () => {
    const s = setup();
    await s.run({ t: "comment-mode", on: true });
    expect(s.out).toEqual([{ t: "failed", code: "no_capture_permission", message: "Clax needs the keyboard command on the page to take screenshots on this tab." }]);
    await s.run({ t: "comment-mode", on: false });
    expect(s.calls).toEqual(["comment-mode 4 false"]);
    // On only through `commentOn`, which makes sure of the page's overlay first.
    const clicked = setup({ admits: true });
    await clicked.run({ t: "comment-mode", on: true });
    const all = setup({ allUrls: true });
    await all.run({ t: "comment-mode", on: true });
    expect([...clicked.calls, ...all.calls]).toEqual(["comment-on 4", "comment-on 4"]);
  });

  it("tells the panel when the overlay could not be put in the page, rather than turning comment mode on with nothing listening", async () => {
    const s = setup({ admits: true, loading: true });
    await s.run({ t: "comment-mode", on: true });
    expect(s.out).toEqual([{ t: "failed", code: "page_loading", message: "The page was still loading. Try again." }]);
  });

  it("waits for a restarted worker to read its tabs back before acting", async () => {
    let restored!: () => void;
    const s = setup({ admits: true, restoring: new Promise<void>(r => { restored = r; }) });
    const acting = s.run({ t: "comment-mode", on: true });
    for (let i = 0; i < 5; i++) await Promise.resolve();
    expect(s.waited()).toBe(1);
    expect(s.calls).toEqual([]);
    restored();
    await acting;
    expect(s.calls).toEqual(["comment-on 4"]);
  });

  it("goes to a route of the page, and only to one", async () => {
    const s = setup();
    await s.run({ t: "navigate", route: "?tab=billing", artifactId: AID });
    await s.run({ t: "navigate", route: null, artifactId: AID });
    await s.run({ t: "navigate", route: "//evil.example/", artifactId: AID });
    expect(s.calls).toEqual([`navigate 4 ${URL1}?tab=billing`, `navigate 4 ${URL1}`]);
    expect(s.out).toEqual([{ t: "failed", code: "invalid_route", message: "Not a route of this page." }]);
  });

  it("acts only on the page the panel showed, and turns off only a tab Clax is on", async () => {
    const s = setup();
    await s.run({ t: "navigate", route: "?tab=billing", artifactId: "8r4m0nzy3c5v" });
    expect(s.out).toEqual([{ t: "failed", code: "page_changed", message: "The tab shows another page now." }]);
    const off = setup({ page: null });
    await off.run({ t: "turn-off", tabId: 4 });
    expect(off.calls).toEqual([]);
    expect(off.out).toEqual([]);
    // A panel that names another tab than the one it watches turns nothing off.
    const other = setup();
    await other.run({ t: "turn-off", tabId: 5 });
    expect(other.calls).toEqual([]);
    expect(other.out).toEqual([{ t: "failed", code: "page_changed", message: "The tab shows another page now." }]);
  });

  it("selects, refreshes on watch, retries with a new pairing, and turns Clax off in the tab", async () => {
    const s = setup();
    await s.run({ t: "select", threadId: T1 });
    await s.run({ t: "watch-tab", tabId: 4 });
    await s.run({ t: "retry" });
    await s.run({ t: "turn-off", tabId: 4 });
    await s.run({ t: "ping" });
    await s.run({ t: "visible", on: true });
    expect(s.calls).toEqual([`select 4 ${T1}`, `route 4 ${URL1} false`, `route 4 ${URL1} true`, "turn-off 4"]);
  });

  it("pairs again on Retry only after a pairing, credential or reachability failure", async () => {
    // An unreachable daemon is a pairing that names a daemon that is gone
    // (restarted on another port within the 10 s a pairing waits, or
    // stopped): the native host answers the running one, starting it if need be.
    for (const code of ["host_missing", "unknown_credential", "http_401", "daemon_unreachable"]) {
      const s = setup({ error: code });
      await s.run({ t: "retry" });
      expect(s.calls).toEqual(["pair true", `route 4 ${URL1} true`]);
    }
    const other = setup({ error: "http_500" });
    await other.run({ t: "retry" });
    expect(other.calls).toEqual([`route 4 ${URL1} true`]);
  });
});

describe("panelAction for the owner's questions and inbox", () => {
  it("answers, skips and moves questions through the owner routes, on no tab, and says when something closed one first", async () => {
    const s = setup();
    const body = { answers: [{ selected: ["Two"], text: null }] };
    await s.run({ t: "q-answer", req: 1, questionId: Q, body }, null);
    await s.run({ t: "q-decline", req: 2, questionId: Q }, null);
    await s.run({ t: "q-release", req: 3, questionId: Q }, 4);
    expect(s.calls).toEqual([`closeQuestion "${Q}" "answer" ${JSON.stringify(body)}`, `closeQuestion "${Q}" "decline" `, `closeQuestion "${Q}" "release" `]);
    expect(s.out).toEqual([1, 2, 3].map(req => ({ t: "q-done", req, question: { id: Q }, closed: false })));
    const c = setup({ closed: true });
    await c.run({ t: "q-answer", req: 7, questionId: Q, body }, null);
    expect(c.out).toEqual([{ t: "q-done", req: 7, question: { id: Q }, closed: true }]);
  });

  it("calls the API's methods on the API, as the real one needs its pairing", async () => {
    const s = setup();
    const out: WorkerToPanel[] = [];
    const real = { pairing: "p", closeQuestion(this: { pairing: string }, qid: string) { return Promise.resolve({ question: { id: qid, via: this.pairing }, closed: false }); },
      markItem(this: { pairing: string }) { return Promise.resolve({ item: { id: this.pairing }, unread: 0 }); } };
    await panelAction({ ...s.d, api: real as never }, null, { t: "q-decline", req: 1, questionId: Q }, r => out.push(r));
    await panelAction({ ...s.d, api: real as never }, null, { t: "inbox-mark", req: 2, ids: [T1], read: true }, r => out.push(r));
    expect(out).toEqual([{ t: "q-done", req: 1, question: { id: Q, via: "p" }, closed: false }, { t: "marked", req: 2, item: { id: "p" }, marked: 1, unread: 0 }]);
  });

  it("answers a failed request with its failure and its request number", async () => {
    const s = setup({ fail: "closeQuestion", failCode: "invalid_answer" });
    await s.run({ t: "q-answer", req: 5, questionId: Q, body: { answers: [{ selected: [], text: "x" }] } }, null);
    expect(s.out).toEqual([{ t: "failed", code: "invalid_answer", message: "No such thread.", req: 5 }]);
  });

  it("pages the inbox with the panel's filter and cursor, and marks items, the count going to every panel", async () => {
    const s = setup();
    await s.run({ t: "inbox-page", req: 1, filter: { q: "x", read: "unread" }, before: "42" }, null);
    expect(s.calls).toEqual([`page {"q":"x","read":"unread"} 42`]);
    expect(s.out[0]).toEqual({ t: "inbox-page", req: 1, page: { items: [], next_cursor: null, unread: 4 } });
    await s.run({ t: "inbox-mark", req: 2, ids: [T1], read: false }, null);
    expect(s.calls.slice(1)).toEqual([`markItem "${T1}" false`, "count 3"]);
    expect(s.out[1]).toEqual({ t: "marked", req: 2, item: { id: T1, read: true }, marked: 1, unread: 3 });
    await s.run({ t: "inbox-mark", req: 3, ids: [T1, T2], read: true }, null);
    expect(s.calls.slice(3)).toEqual([`markItems {"ids":["${T1}","${T2}"]}`, "count 1"]);
    expect(s.out[2]).toEqual({ t: "marked", req: 3, item: null, marked: 2, unread: 1 });
    await s.run({ t: "inbox-mark-all", req: 4, filter: { q: "x" }, upto: 99 }, null);
    expect(s.calls.slice(5)).toEqual([`markItems {"all":true,"filter":{"q":"x"},"upto":99}`, "count 1"]);
  });

  it("opens a live page's item in the tab showing that page, its thread selected, and anything else on the daemon in a new tab", async () => {
    const s = setup();
    // Tab 4 shows the live page AID (tab 9 is unknown).
    await s.run({ t: "open-url", url: `/a/${AID}?thread=${T2}`, pageUrl: URL1 }, null);
    expect(s.calls).toEqual(["focus 4", `select 4 ${T2}`]);
    await s.run({ t: "open-url", url: `/a/${AID}/v/3`, pageUrl: null }, null);
    expect(s.calls.slice(2)).toEqual(["focus 4"]);
    await s.run({ t: "open-url", url: "/a/8r4m0nzy3c5v?thread=x", pageUrl: null }, null);
    await s.run({ t: "open-url", url: "/inbox?q=01J9QQQQQQQQQQQQQQQQQQQQQQ", pageUrl: null }, null);
    expect(s.calls.slice(3)).toEqual(["create http://127.0.0.1:7480/a/8r4m0nzy3c5v?thread=x", "create http://127.0.0.1:7480/inbox?q=01J9QQQQQQQQQQQQQQQQQQQQQQ"]);
  });

  it("brings a tab showing the live page with Clax off in it to the front, else opens the item on the daemon", async () => {
    const s = setup();
    await s.run({ t: "open-url", url: "/a/8r4m0nzy3c5v?thread=x", pageUrl: "http://localhost:5173/other" }, null);
    expect(s.calls).toEqual(["find http://localhost:5173/other", "focus 12"]);
    await s.run({ t: "open-url", url: "/a/8r4m0nzy3c5v", pageUrl: "http://localhost:5173/gone" }, null);
    expect(s.calls.slice(2)).toEqual(["find http://localhost:5173/gone", "create http://127.0.0.1:7480/a/8r4m0nzy3c5v"]);
  });

  it("leaves no error on the tab for a failed question or inbox request", async () => {
    const s = setup({ fail: "markItem", failCode: "daemon_unreachable" });
    await s.run({ t: "inbox-mark", req: 3, ids: [T1], read: true }, 4);
    expect(s.calls.filter(c => c.startsWith("fail"))).toEqual([]);
    expect(s.out).toEqual([{ t: "failed", code: "daemon_unreachable", message: "No such thread.", req: 3 }]);
    // A tab action's failure still does.
    const t = setup({ fail: "resolve", failCode: "daemon_unreachable" });
    await t.run({ t: "resolve", threadId: T1 }, 4);
    expect(t.calls).toContain("fail 4 daemon_unreachable");
  });
});
