import { afterEach, describe, expect, it, vi } from "vitest";
import { FakeEvent } from "../../test/fake-chrome";
import { PanelLink, REQUEST_MS } from "./link.svelte";

function fakes() {
  const ports: { name: string; sent: unknown[]; onMessage: FakeEvent<[unknown]>; onDisconnect: FakeEvent<[]> }[] = [];
  const runtime = {
    connect: ({ name }: { name: string }) => {
      const p = { name, sent: [] as unknown[], onMessage: new FakeEvent<[unknown]>(), onDisconnect: new FakeEvent<[]>(), postMessage(m: unknown) { p.sent.push(m); }, disconnect() {} };
      ports.push(p);
      return p;
    },
  };
  return { ports, runtime };
}
const settle = async () => { for (let i = 0; i < 5; i++) await Promise.resolve(); };
let link: PanelLink | null = null;
afterEach(() => { link?.close(); link = null; vi.useRealTimers(); });

const ENV = (f: ReturnType<typeof fakes>, search = "?tab=5") => ({ runtime: f.runtime as never, search, doc: { visibilityState: "visible", addEventListener() {}, removeEventListener() {} } as never });

describe("PanelLink", () => {
  it("asks the worker who approves first, and takes its answer, or none on a failure or a lost worker", async () => {
    const f = fakes();
    link = new PanelLink(2, ENV(f));
    await settle();
    const p = f.ports[0];
    const T = "01J9AAAAAAAAAAAAAAAAAAAAAA", SAM = `u_${"a".repeat(22)}`;
    const asked = link.firsts([T]);
    const sent = p.sent.at(-1) as { t: string; req: number };
    expect(sent).toEqual({ t: "firsts", req: sent.req, threadIds: [T] });
    p.onMessage.fire({ t: "firsts", req: sent.req, firsts: [null, SAM] });
    expect(await asked).toEqual([null, SAM]);
    const failed = link.firsts([T]);
    p.onMessage.fire({ t: "failed", code: "no_tab", message: "m", req: (p.sent.at(-1) as { req: number }).req });
    expect(await failed).toBeNull();
    const lost = link.firsts([T]);
    p.onDisconnect.fire();
    expect(await lost).toBeNull();
  });

  it("keeps the inbox as the worker tells it, and the open questions in its question feed", () => {
    const f = fakes();
    link = new PanelLink(2, ENV(f));
    const p = f.ports[0];
    const q = { id: "01J9Q0000000000000000000AA", status: "open", artifact: { id: "7q3k9mzx2b4t" }, created_at: "2026-10-07T09:00:00Z" };
    p.onMessage.fire({ t: "inbox", owner: true, unread: 3, questions: [q] });
    expect(link.inbox).toEqual({ owner: true, unread: 3 });
    expect(link.questions.byArtifact("7q3k9mzx2b4t").map(x => x.id)).toEqual([q.id]);
    const heard: unknown[] = [];
    const off = link.onItem(i => heard.push(i));
    p.onMessage.fire({ t: "inbox-event", item: { id: "x" }, unread: 4 });
    p.onMessage.fire({ t: "inbox-event", item: null, unread: 0 });
    off();
    p.onMessage.fire({ t: "inbox-event", item: null, unread: 1 });
    expect(heard).toEqual([{ id: "x" }, null]);
    expect(link.inbox.unread).toBe(1);
    // A closed question leaves the open list (its card says what closed it for a moment).
    p.onMessage.fire({ t: "q-event", question: { ...q, status: "answered" } });
    expect(link.questions.open).toEqual([]);
    expect(link.questions.recent.map(x => x.status)).toEqual(["answered"]);
    p.onMessage.fire({ t: "inbox", owner: "yes", unread: 3, questions: [] });
    expect(link.inbox.owner).toBe(true);
  });

  it("answers a question through the worker and takes the question it answers with", async () => {
    const f = fakes();
    link = new PanelLink(2, ENV(f));
    await settle();
    const p = f.ports[0];
    const q = { id: "01J9Q0000000000000000000AA", status: "open", artifact: null, created_at: "2026-10-07T09:00:00Z" };
    p.onMessage.fire({ t: "inbox", owner: true, unread: 1, questions: [q] });
    const done = link.questions.answer(q.id, { answers: [{ selected: [], text: "Yes" }] });
    const sent = p.sent.at(-1) as { t: string; req: number };
    expect(sent).toEqual({ t: "q-answer", req: sent.req, questionId: q.id, body: { answers: [{ selected: [], text: "Yes" }] } });
    p.onMessage.fire({ t: "q-done", req: sent.req, question: { ...q, status: "answered" }, closed: false });
    await done;
    expect(link.questions.get(q.id)?.status).toBe("answered");
    // A refusal is the daemon's words; a lost worker is that Clax did not answer.
    const skip = link.questions.decline(q.id);
    const s2 = p.sent.at(-1) as { req: number };
    p.onMessage.fire({ t: "failed", code: "question_closed", message: "That question is closed.", req: s2.req });
    await expect(skip).rejects.toMatchObject({ status: 400, code: "question_closed" });
    const move = link.questions.release(q.id);
    p.onDisconnect.fire();
    await expect(move).rejects.toMatchObject({ status: 0, code: "worker_restarted" });
  });

  it("watches its own tab (`?tab=`), saying whether the panel is visible", async () => {
    const f = fakes();
    link = new PanelLink(2, ENV(f));
    await settle();
    expect(f.ports[0].name).toBe("panel:2");
    expect(f.ports[0].sent).toEqual([{ t: "watch-tab", tabId: 5 }, { t: "visible", on: true }]);
  });

  it("watches no tab when its page names none", async () => {
    vi.useFakeTimers();
    for (const search of ["", "?tab=0", "?tab=x"]) {
      const f = fakes();
      link = new PanelLink(2, ENV(f, search));
      await vi.advanceTimersByTimeAsync(0);
      f.ports[0].onDisconnect.fire();
      await vi.advanceTimersByTimeAsync(1000);
      expect(f.ports.flatMap(p => p.sent)).toEqual([]);
      link.close();
    }
  });

  it("takes only the worker's well-formed messages", async () => {
    const f = fakes();
    link = new PanelLink(2, ENV(f));
    const p = f.ports[0];
    p.onMessage.fire({ t: "tab", state: { tabId: 5, error: null } });
    expect(link.state).toEqual({ tabId: 5, error: null });
    expect(link.failures).toBe(0);
    p.onMessage.fire({ t: "failed", code: "no_page", message: "No live page." });
    expect(link.state?.error).toEqual({ code: "no_page", message: "No live page." });
    // Each failure of an action counts, the same one again too: the panel shows it again after a dismissal.
    expect(link.failures).toBe(1);
    p.onMessage.fire({ t: "failed", code: "no_page", message: "No live page." });
    expect(link.failures).toBe(2);
    p.onMessage.fire({ t: "stream-status", up: false });
    expect(link.up).toBe(false);
    p.onMessage.fire({ t: "tab", state: "<script>" });
    p.onMessage.fire({ t: "stream-status", up: "no" });
    expect(link.up).toBe(false);
    expect(link.state?.tabId).toBe(5);
  });

  it("asks the worker for a thread's clip, takes only a PNG data URL, and answers null on a failure or a lost worker", async () => {
    vi.useFakeTimers();
    const f = fakes();
    link = new PanelLink(2, ENV(f));
    const p = f.ports[0];
    p.onMessage.fire({ t: "tab", state: { tabId: 5, error: null } });
    p.onMessage.fire({ t: "failed", code: "no_page", message: "No live page." });
    const T = "01J9CCCCCCCCCCCCCCCCCCCCCC";
    const a = link.clip(T);
    expect(p.sent.at(-1)).toEqual({ t: "clip", req: 1, threadId: T });
    // Asking for a clip is not an action: the failure shown stays.
    expect(link.state?.error).toEqual({ code: "no_page", message: "No live page." });
    p.onMessage.fire({ t: "clip", req: 1, url: "javascript:alert(1)" });
    p.onMessage.fire({ t: "clip", req: 1, url: "data:image/png;base64,iVBORw0KGgo=" });
    expect(await a).toBe("data:image/png;base64,iVBORw0KGgo=");
    const b = link.clip(T);
    p.onMessage.fire({ t: "failed", code: "no_tab", message: "The panel shows no tab.", req: 2 });
    expect(await b).toBeNull();
    expect(link.failures).toBe(1);
    const c = link.clip(T);
    await vi.advanceTimersByTimeAsync(REQUEST_MS);
    expect(await c).toBeNull();
    const e = link.farPage(T);
    expect(p.sent.at(-1)).toEqual({ t: "far-page", req: 4, threadId: T });
    p.onMessage.fire({ t: "far-page", req: 4, page: null });
    expect(await e).toBeNull();
    const d = link.clip(T);
    p.onDisconnect.fire();
    expect(await d).toBeNull();
  });

  it("keeps the tab's site as the worker last told it", () => {
    const f = fakes();
    link = new PanelLink(2, ENV(f));
    const site = { origin: "http://localhost:5173", rules: [], pages: [] };
    f.ports[0].onMessage.fire({ t: "site", site });
    expect(link.site).toEqual(site);
    f.ports[0].onMessage.fire({ t: "site", site: "x" });
    expect(link.site).toEqual(site);
    f.ports[0].onMessage.fire({ t: "site", site: null });
    expect(link.site).toBeNull();
  });

  it("answers a request with its step, or its failure, without showing that as the tab's error", async () => {
    const f = fakes();
    link = new PanelLink(2, ENV(f));
    await settle();
    const p = f.ports[0];
    p.onMessage.fire({ t: "tab", state: { tabId: 5, error: null } });
    const one = link.request({ t: "rule", origin: "http://localhost:5173", pattern: "/users/:id" });
    const sent = p.sent.at(-1) as { req: number };
    expect(sent).toEqual({ t: "rule", origin: "http://localhost:5173", pattern: "/users/:id", req: expect.any(Number) });
    p.onMessage.fire({ t: "step", req: sent.req + 1, moved: 0, remaining: 0 });
    p.onMessage.fire({ t: "step", req: sent.req, moved: 200, remaining: 4 });
    expect(await one).toEqual({ moved: 200, remaining: 4 });
    const two = link.request({ t: "unrule", origin: "http://localhost:5173", ruleId: "01J9AAAAAAAAAAAAAAAAAAAAAA" });
    const req = (p.sent.at(-1) as { req: number }).req;
    expect(req).not.toBe(sent.req);
    p.onMessage.fire({ t: "failed", code: "invalid_pattern", message: "Bad", req });
    await expect(two).rejects.toMatchObject({ code: "invalid_pattern", message: "Bad" });
    expect(link.state?.error).toBeNull();
    // The worker went away: what was asked of it will not be answered.
    const three = link.request({ t: "rule", origin: "http://localhost:5173", pattern: "/a/:b" });
    p.onDisconnect.fire();
    await expect(three).rejects.toMatchObject({ code: "worker_restarted" });
  });

  it("gives up on a request the worker never answers", async () => {
    vi.useFakeTimers();
    const f = fakes();
    link = new PanelLink(2, ENV(f));
    const r = link.request({ t: "rule", origin: "http://localhost:5173", pattern: "/users/:id" });
    const done = expect(r).rejects.toMatchObject({ code: "timeout" });
    await vi.advanceTimersByTimeAsync(REQUEST_MS);
    await done;
  });

  it("keeps an action's failure shown through later pushes until the next action", async () => {
    const f = fakes();
    link = new PanelLink(2, ENV(f));
    await settle();
    const p = f.ports[0];
    p.onMessage.fire({ t: "tab", state: { tabId: 5, error: null } });
    link.post({ t: "send", threadId: "01J9AAAAAAAAAAAAAAAAAAAAAA", to: null });
    p.onMessage.fire({ t: "failed", code: "not_found", message: "No such thread." });
    p.onMessage.fire({ t: "tab", state: { tabId: 5, error: null } });
    expect(link.state?.error).toEqual({ code: "not_found", message: "No such thread." });
    // The tab's own error wins.
    p.onMessage.fire({ t: "tab", state: { tabId: 5, error: { code: "daemon_unreachable", message: "down" } } });
    expect(link.state?.error?.code).toBe("daemon_unreachable");
    // A ping keeps it; the next action clears it.
    link.post({ t: "ping" });
    p.onMessage.fire({ t: "tab", state: { tabId: 5, error: null } });
    expect(link.state?.error?.code).toBe("not_found");
    link.post({ t: "retry" });
    p.onMessage.fire({ t: "tab", state: { tabId: 5, error: null } });
    expect(link.state?.error).toBeNull();
  });

  it("connects again when the worker goes away, and pings while open", async () => {
    vi.useFakeTimers();
    const f = fakes();
    link = new PanelLink(2, { runtime: f.runtime as never, search: "?tab=7", doc: { visibilityState: "hidden", addEventListener() {}, removeEventListener() {} } as never });
    await vi.advanceTimersByTimeAsync(0);
    expect(f.ports[0].sent).toEqual([{ t: "watch-tab", tabId: 7 }, { t: "visible", on: false }]);
    await vi.advanceTimersByTimeAsync(20_000);
    expect(f.ports[0].sent.at(-1)).toEqual({ t: "ping" });
    f.ports[0].onDisconnect.fire();
    await vi.advanceTimersByTimeAsync(1000);
    expect(f.ports).toHaveLength(2);
    expect(f.ports[1].sent).toEqual([{ t: "watch-tab", tabId: 7 }, { t: "visible", on: false }]);
  });
});
