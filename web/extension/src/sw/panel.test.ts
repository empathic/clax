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

const SITE = { key: "http://localhost:5173", name: "http://localhost:5174", joined: true, origins: [{ origin: "http://localhost:5174", joined_at: "t", last_used_at: "t2" }, { origin: "http://localhost:5173", joined_at: "t", last_used_at: "t1" }] };
function setup(over: { remaining?: number; page?: typeof page | null; admits?: boolean; allUrls?: boolean; fail?: string; failCode?: string; error?: string; site?: string[]; down?: string[] } = {}) {
  const calls: string[] = [];
  const out: WorkerToPanel[] = [];
  const pg = over.page === undefined ? page : over.page;
  const api = (name: string, ret: unknown) => async (...a: unknown[]) => {
    calls.push(`${name} ${a.map(x => JSON.stringify(x)).join(" ")}`);
    if (over.fail === name) throw new ApiFailure(over.failCode ?? "not_found", "No such thread.", 404);
    return ret;
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
    } as never,
    tabs: {
      state: () => (pg === null ? undefined : { url: URL1, page: pg, overlay: true, on: "http://localhost:5173", error: over.error ? { code: over.error, message: "m" } : null } as never),
      admits: () => over.admits ?? false,
      route: async (tabId: number, url: string, fresh?: boolean) => { calls.push(`route ${tabId} ${url} ${!!fresh}`); return {} as never; },
      applied: (tabId: number, t: { id: string }) => calls.push(`applied ${tabId} ${t.id}`),
      fail: (tabId: number, e: { code: string }) => calls.push(`fail ${tabId} ${e.code}`),
      setViewer: (v: { display_name: string }) => calls.push(`viewer ${v.display_name}`),
      select: (tabId: number, id: string | null) => calls.push(`select ${tabId} ${id}`),
      setCommentMode: (tabId: number, on: boolean) => calls.push(`comment-mode ${tabId} ${on}`),
      openThread: (tabId: number, id: string) => { calls.push(`open ${tabId} ${id}`); return id === T1 ? { path: "/users/7?x#/y", origins: over.site ?? ["http://localhost:5173"] } : null; },
    } as never,
    sites: { load: async (o: string) => { calls.push(`load ${o}`); }, origins: (o: string) => over.site ?? [o] },
    probe: async (url: string) => { calls.push(`probe ${url}`); return !(over.down ?? []).some(x => url.startsWith(x)); },
    title: async () => "My App",
    pairer: { pair: async (retry?: boolean) => { calls.push(`pair ${!!retry}`); return {} as never; } },
    allUrls: async () => over.allUrls ?? false,
    navigate: async (tabId, url) => { calls.push(`navigate ${tabId} ${url}`); },
    turnOff: async tabId => { calls.push(`turn-off ${tabId}`); },
  };
  const run = (m: PanelToWorker, tabId: number | null = 4) => panelAction(d, tabId, m, r => out.push(r));
  return { calls, out, run };
}

describe("panelAction on the tab's site", () => {
  it("opens a thread of another page in the tab, and refuses one the site does not have", async () => {
    const s = setup();
    await s.run({ t: "open-thread", threadId: T1 });
    expect(s.calls).toEqual([`open 4 ${T1}`, "probe http://localhost:5173/users/7?x#/y", "navigate 4 http://localhost:5173/users/7?x#/y"]);
    await s.run({ t: "open-thread", threadId: T2 });
    expect(s.out).toEqual([{ t: "failed", code: "not_found", message: "That thread is not on this site any more." }]);
  });

  it("opens a thread of a joined site on its most recently used address that answers, and says so when none does", async () => {
    const both = ["http://localhost:5174", "http://localhost:5173"];
    const s = setup({ site: both, down: ["http://localhost:5174"] });
    await s.run({ t: "open-thread", threadId: T1 });
    expect(s.calls).toEqual([`open 4 ${T1}`, "probe http://localhost:5174/users/7?x#/y", "probe http://localhost:5173/users/7?x#/y", "navigate 4 http://localhost:5173/users/7?x#/y"]);
    const none = setup({ site: both, down: both });
    await none.run({ t: "open-thread", threadId: T1 });
    expect(none.calls.filter(c => c.startsWith("navigate"))).toEqual([]);
    expect(none.out).toEqual([{ t: "failed", code: "site_unreachable", message: "No address of this site answers (localhost:5174, localhost:5173). Start its server, then try again." }]);
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
    expect(s.out).toEqual([{ t: "failed", code: "no_capture_permission", message: "Clax needs a click on its button to take screenshots on this tab." }]);
    await s.run({ t: "comment-mode", on: false });
    expect(s.calls).toEqual(["comment-mode 4 false"]);
    const clicked = setup({ admits: true });
    await clicked.run({ t: "comment-mode", on: true });
    const all = setup({ allUrls: true });
    await all.run({ t: "comment-mode", on: true });
    expect([...clicked.calls, ...all.calls]).toEqual(["comment-mode 4 true", "comment-mode 4 true"]);
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
