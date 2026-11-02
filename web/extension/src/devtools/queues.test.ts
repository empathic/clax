// The DevTools panel's Queues view: the worker-backed TriageApi checks what
// comes back; "Show on the page" evaluates a JSON-literal selector; the
// view picks the inspected site's queue and opens a report in place. The
// worker's side: the queues topic held while a panel watches, the pings
// told, and the queue messages' validators.
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Queue, Report } from "../../../overlay/src/triage/triage";
import { isFromPanel, isToPanel } from "../messages";
import { FakeEvent } from "../../test/fake-chrome";
import { QUEUES_HUB_ID, WorkerQueues } from "../sw/queues";
import { isReport, panelTriage, showExpr } from "./queues";
import QueuesView from "./QueuesView.svelte";

const QID = "01M66XHDKTMTCVB95FE9BQN04V";
const TID = "01J9AAAAAAAAAAAAAAAAAAAAAA";
const queue = (over: Partial<Queue> = {}): Queue => ({ id: QID, name: "Changeling feedback", origins: ["http://127.0.0.1:7700"], reporters_see: "own", created_at: "2026-11-01T00:00:00Z", role: "admin", ...over });
const report = (over: Partial<Report> = {}): Report => ({
  id: TID, queue_id: QID, status: "open", state: "open", reporter_id: null, metadata: null, metadata_version: 0, has_clip: false, claim: null,
  artifact: { id: "a1", external_id: "/x", title: "Dashboard", url: "http://127.0.0.1:7700/x", metadata: null, metadata_version: 0, revision: null },
  anchor: { kind: "element", selector: "#save", label: "Save" },
  comments: [{ id: "c1", author_kind: "reporter", author_name: "Alex", body: "Save does nothing", created_at: "2026-11-02T10:00:00Z" }],
  created_at: "2026-11-02T10:00:00Z", ...over,
});
/** A link whose worker answers `queue-*` requests from `answers`, and whose `ping` tells each Queues listener. */
function link(answers: Record<string, unknown>) {
  const pings = new Set<() => void>();
  const asked: unknown[] = [];
  return {
    asked, ping: () => pings.forEach(f => f()),
    queue: vi.fn(async (m: { t: string }) => { asked.push(m); if (!(m.t in answers)) throw new Error("No."); return answers[m.t]; }),
    onQueuePing: (f: () => void) => { pings.add(f); return () => pings.delete(f); },
  };
}
afterEach(() => cleanup());

describe("panelTriage", () => {
  it("reads the owner's queues and the daemon's origin, dropping what is not a queue", async () => {
    const api = panelTriage(link({ "queue-list": { queues: [queue(), { id: 1 }], base: "http://127.0.0.1:7480/" } }) as never);
    expect((await api.queues()).map(q => q.id)).toEqual([QID]);
    expect(api.base).toBe("http://127.0.0.1:7480");
    const bad = panelTriage(link({ "queue-list": { queues: [], base: "javascript:alert(1)" } }) as never);
    await bad.queues();
    expect(bad.base).toBeNull();
  });

  it("takes a report only whole, a clip only as a PNG data URL", async () => {
    expect(isReport(report())).toBe(true);
    expect(isReport({ ...report(), comments: [{ body: 1 }] })).toBe(false);
    const api = panelTriage(link({ "queue-report": { report: { ...report(), state: "gone" } }, "queue-clip": "https://evil.example/x.png" }) as never);
    await expect(api.report(QID, TID)).rejects.toThrow(/something else/);
    expect(await api.clip(QID, TID)).toBeNull();
  });

  it("replies and resolves through the worker, and hears its pings", async () => {
    const l = link({ "queue-reply": { comment: {}, report: report() }, "queue-resolve": { report: report({ status: "resolved", state: "resolved" }) } });
    const api = panelTriage(l as never);
    await api.reply(QID, TID, "On it");
    expect((await api.resolve(QID, TID)).state).toBe("resolved");
    expect(l.asked).toEqual([{ t: "queue-reply", qid: QID, tid: TID, body: "On it" }, { t: "queue-resolve", qid: QID, tid: TID }]);
    const heard = vi.fn();
    const off = api.watch(heard);
    l.ping();
    off();
    l.ping();
    expect(heard).toHaveBeenCalledOnce();
  });
});

describe("showExpr", () => {
  /** Evaluates the expression in a document holding `html`, with DevTools' `inspect` recorded. */
  function run(html: string, selector: string) {
    document.body.innerHTML = html;
    const inspected: Element[] = [];
    Element.prototype.scrollIntoView = () => {};
    const found = new Function("inspect", `return ${showExpr(selector)};`)((el: Element) => inspected.push(el));
    return { found, inspected };
  }
  it("scrolls to the element, outlines it and selects it in Elements", () => {
    const { found, inspected } = run(`<button id="save">Save</button>`, "#save");
    expect(found).toBe(true);
    expect(inspected.map(e => e.id)).toEqual(["save"]);
    expect((document.getElementById("save") as HTMLElement).style.outline).toContain("3px solid");
  });
  it("says false for a missing element or a selector that does not parse, and carries a hostile one as a literal", () => {
    expect(run("<p>", "#gone").found).toBe(false);
    expect(run("<p>", "[[[").found).toBe(false);
    expect(run("<p>", `"); window.pwned = 1; ("`).found).toBe(false);
    expect((window as unknown as Record<string, unknown>).pwned).toBeUndefined();
  });
});

describe("QueuesView", () => {
  const views = (answers: Record<string, unknown>, props: Partial<{ origin: string | null; page: string | null }> = {}) => {
    const l = link(answers);
    const said: string[] = [];
    const show = vi.fn(async () => true);
    render(QueuesView, { props: { api: panelTriage(l as never), origin: "http://127.0.0.1:7700", page: "http://127.0.0.1:7700/x", show, say: (t: string) => said.push(t), ...props } });
    return { l, said, show };
  };

  it("shows the inspected site's queue, this page's reports first, a report opened in place with Show on the page", async () => {
    const { said, show } = views({
      "queue-list": { queues: [queue(), queue({ id: "01M66XHDKTMTCVB95FE9BQN0ZZ", name: "Other", origins: ["https://other.example"] })], base: "http://127.0.0.1:7480" },
      "queue-reports": { reports: [report()], next: null }, "queue-report": { report: report() },
    });
    expect(await screen.findByRole("heading", { name: "Changeling feedback" })).toBeTruthy();
    expect(screen.queryByRole("radio")).toBeNull();
    expect(screen.getByRole("link", { name: "Open in Clax" }).getAttribute("href")).toBe(`http://127.0.0.1:7480/queues/${QID}`);
    const card = await screen.findByRole("link", { name: /^Save does nothing/ });
    expect(card.getAttribute("href")).toBe(`http://127.0.0.1:7480/queues/${QID}/${TID}`);
    expect(screen.getByRole("heading", { name: "On this page" })).toBeTruthy();
    await fireEvent.click(card);
    await fireEvent.click(await screen.findByRole("button", { name: "Show on the page" }));
    expect(show).toHaveBeenCalledWith("#save");
    await waitFor(() => expect(said).toEqual(["Shown on the page, and selected in Elements."]));
    expect(screen.getByRole("link", { name: "Open in Clax" }).getAttribute("href")).toBe(`http://127.0.0.1:7480/queues/${QID}/${TID}`);
    await fireEvent.click(screen.getByRole("link", { name: "← Changeling feedback" }));
    expect(await screen.findByRole("link", { name: /^Save does nothing/ })).toBeTruthy();
  });

  it("offers every queue of the owner's when none takes reports from the inspected site", async () => {
    views({ "queue-list": { queues: [queue({ origins: ["https://app.example"] })], base: null }, "queue-reports": { reports: [], next: null } });
    const pick = await screen.findByRole("group", { name: /No queue of yours takes reports from http:\/\/127\.0\.0\.1:7700/ });
    expect(screen.queryByRole("heading", { name: "Changeling feedback" })).toBeNull();
    await fireEvent.click(pick.querySelector("input")!);
    expect(await screen.findByRole("heading", { name: "Changeling feedback" })).toBeTruthy();
    expect(screen.queryByRole("link", { name: "Open in Clax" })).toBeNull();
  });

  it("says why when the queues cannot be read", async () => {
    views({});
    expect(await screen.findByRole("alert")).toHaveProperty("textContent", "Could not read your queues: No.");
  });

  it("says a daemon older than the extension needs updating, when its gateway refuses the queue routes", async () => {
    const l = link({});
    l.queue.mockRejectedValue(new Error("the extension may not use this route"));
    render(QueuesView, { props: { api: panelTriage(l as never), origin: null, page: null, show: async () => true, say: () => {} } });
    expect((await screen.findByRole("alert")).textContent).toBe("Could not read your queues: This Clax is older than the extension and does not show queues here yet: update Clax.");
  });

  it("offers Show on the page only for a report about the inspected page; another page's opens there", async () => {
    const elsewhere = report({ artifact: { ...report().artifact, url: "http://127.0.0.1:7700/y" } });
    views({ "queue-list": { queues: [queue()], base: null }, "queue-reports": { reports: [elsewhere], next: null }, "queue-report": { report: elsewhere } });
    await fireEvent.click(await screen.findByRole("link", { name: /^Save does nothing/ }));
    expect(await screen.findByRole("link", { name: "Open on the page" })).toHaveProperty("href", "http://127.0.0.1:7700/y");
    expect(screen.queryByRole("button", { name: "Show on the page" })).toBeNull();
  });
});

describe("the worker's queues", () => {
  const port = () => ({ postMessage: vi.fn(), onDisconnect: new FakeEvent<[]>() });
  it("holds the queues topic while a panel watches, tells each watcher at a ping, and lets go with the last", () => {
    const hub = { receive: vi.fn(), detach: vi.fn() };
    const q = new WorkerQueues({ hub });
    const a = port(), b = port();
    q.watch(a as never, true);
    q.watch(b as never, true);
    expect(hub.receive).toHaveBeenCalledTimes(1);
    expect(hub.receive).toHaveBeenCalledWith(QUEUES_HUB_ID, { t: "topics", topics: ["queues"] });
    q.fromHub({ t: "event", topic: "queues", name: "queues", data: {} });
    q.fromHub({ t: "resync", topic: "queues" });
    expect(a.postMessage).toHaveBeenCalledTimes(2);
    expect(a.postMessage).toHaveBeenCalledWith({ t: "queue-ping" });
    q.watch(a as never, false);
    expect(hub.detach).not.toHaveBeenCalled();
    b.onDisconnect.fire();
    expect(hub.detach).toHaveBeenCalledWith(QUEUES_HUB_ID);
  });

  it("validates the queue messages both ways", () => {
    expect(isFromPanel({ t: "queue-watch", on: true })).toBe(true);
    expect(isFromPanel({ t: "queue-list", req: 1 })).toBe(true);
    expect(isFromPanel({ t: "queue-reports", req: 1, qid: QID, cursor: null })).toBe(true);
    expect(isFromPanel({ t: "queue-report", req: 1, qid: QID, tid: TID })).toBe(true);
    expect(isFromPanel({ t: "queue-reply", req: 1, qid: QID, tid: TID, body: "Hi" })).toBe(true);
    expect(isFromPanel({ t: "queue-report", req: 1, qid: "../admin", tid: TID })).toBe(false);
    expect(isFromPanel({ t: "queue-reports", req: 1, qid: QID, cursor: "a b" })).toBe(false);
    expect(isFromPanel({ t: "queue-claim", req: 1, qid: QID, tid: TID })).toBe(false);
    expect(isToPanel({ t: "queue-data", req: 1, data: { queues: [] } })).toBe(true);
    expect(isToPanel({ t: "queue-ping" })).toBe(true);
    expect(isToPanel({ t: "queue-ping", extra: 1 })).toBe(false);
  });
});
