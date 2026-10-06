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

function setup(over: { page?: typeof page | null; admits?: boolean; allUrls?: boolean; fail?: string } = {}) {
  const calls: string[] = [];
  const out: WorkerToPanel[] = [];
  const pg = over.page === undefined ? page : over.page;
  const api = (name: string, ret: unknown) => async (...a: unknown[]) => {
    calls.push(`${name} ${a.map(x => JSON.stringify(x)).join(" ")}`);
    if (over.fail === name) throw new ApiFailure("not_found", "No such thread.", 404);
    return ret;
  };
  const d: PanelDeps = {
    api: {
      sendThread: api("sendThread", { thread: thread(T1) }), sendBatch: api("sendBatch", { threads: [thread(T1), thread(T2)] }),
      comment: api("comment", { thread: thread(T1) }), resolve: api("resolve", { thread: thread(T1, "resolved") }),
      reopen: api("reopen", { thread: thread(T1) }), remove: api("remove", {}), looked: api("looked", {}),
      setName: api("setName", { viewer: { public_id: "u_o", display_name: "Mia", created_at: "t" } }),
    } as never,
    tabs: {
      state: () => (pg === null ? undefined : { url: URL1, page: pg, overlay: true } as never),
      admits: () => over.admits ?? false,
      route: async (tabId: number, url: string, fresh?: boolean) => { calls.push(`route ${tabId} ${url} ${!!fresh}`); return {} as never; },
      applied: (tabId: number, t: { id: string }) => calls.push(`applied ${tabId} ${t.id}`),
      removed: (tabId: number, id: string) => calls.push(`removed ${tabId} ${id}`),
      setViewer: (v: { display_name: string }) => calls.push(`viewer ${v.display_name}`),
      select: (tabId: number, id: string | null) => calls.push(`select ${tabId} ${id}`),
      setCommentMode: (tabId: number, on: boolean) => calls.push(`comment-mode ${tabId} ${on}`),
    } as never,
    pairer: { forget: async () => { calls.push("forget"); } },
    allUrls: async () => over.allUrls ?? false,
    navigate: async (tabId, url) => { calls.push(`navigate ${tabId} ${url}`); },
    turnOff: async (tabId, origin) => { calls.push(`turn-off ${tabId} ${origin}`); },
  };
  const run = (m: PanelToWorker, tabId: number | null = 4) => panelAction(d, tabId, m, r => out.push(r));
  return { calls, out, run };
}

describe("panelAction", () => {
  it("sends, replies, resolves, reopens and deletes on the tab's live page, applying the answer", async () => {
    const s = setup();
    await s.run({ t: "send", threadId: T1, to: null });
    await s.run({ t: "send-batch", threadIds: [T1, T2], note: "both", to: "a_0123456789abcdef012345" });
    await s.run({ t: "reply", threadId: T1, body: "ok" });
    await s.run({ t: "resolve", threadId: T1 });
    await s.run({ t: "reopen", threadId: T1 });
    await s.run({ t: "delete", threadId: T1 });
    await s.run({ t: "looked", threadIds: [T1] });
    expect(s.calls).toEqual([
      `sendThread "${AID}" "${T1}" null`, `applied 4 ${T1}`,
      `sendBatch "${AID}" ["${T1}","${T2}"] "both" "a_0123456789abcdef012345"`, `applied 4 ${T1}`, `applied 4 ${T2}`,
      `comment "${AID}" "${T1}" "ok"`, `applied 4 ${T1}`,
      `resolve "${AID}" "${T1}"`, `applied 4 ${T1}`,
      `reopen "${AID}" "${T1}"`, `applied 4 ${T1}`,
      `remove "${AID}" "${T1}"`, `removed 4 ${T1}`,
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
    await s.run({ t: "navigate", route: "?tab=billing" });
    await s.run({ t: "navigate", route: null });
    await s.run({ t: "navigate", route: "//evil.example/" });
    expect(s.calls).toEqual([`navigate 4 ${URL1}?tab=billing`, `navigate 4 ${URL1}`]);
    expect(s.out).toEqual([{ t: "failed", code: "invalid_route", message: "Not a route of this page." }]);
  });

  it("selects, refreshes on watch, retries with a new pairing, and turns Clax off on the origin", async () => {
    const s = setup();
    await s.run({ t: "select", threadId: T1 });
    await s.run({ t: "watch-tab", tabId: 4 });
    await s.run({ t: "retry" });
    await s.run({ t: "turn-off" });
    await s.run({ t: "ping" });
    await s.run({ t: "visible", on: true });
    expect(s.calls).toEqual([`select 4 ${T1}`, `route 4 ${URL1} false`, "forget", `route 4 ${URL1} true`, "turn-off 4 http://localhost:5173"]);
  });
});
