import { describe, expect, it } from "vitest";
import type { InboxItem, QuestionView } from "../../../shell/src/api";
import type { TabMsg } from "../../../shell/src/stream-hub";
import type { WorkerToPanel } from "../messages";
import { ApiFailure } from "./api";
import { HUB_ID, RETRY_LIMIT, RETRY_MAX_MS, WorkerInbox } from "./inbox";

const A = "7q3k9mzx2b4t";
const B = "8r4m0nzy3c5v";
const Q1 = "01J9AAAAAAAAAAAAAAAAAAAAAA";
const Q2 = "01J9BBBBBBBBBBBBBBBBBBBBBB";
const Q3 = "01J9CCCCCCCCCCCCCCCCCCCCCC";
const question = (id: string, aid: string | null, created: string, status: QuestionView["status"] = "open"): QuestionView => ({
  id, agent: { handle: `a_${"1".repeat(22)}`, harness: "claude", project: "clax" }, artifact: aid ? { id: aid, title: aid, kind: "live" } : null,
  source: "ask", status, questions: [{ question: "Which?", header: "Pick", options: [], multi_select: false, other: false }], answers: null,
  answered_via: null, created_at: created, closed_at: null,
});
const item = (id: string, seq: number, read = false) => ({ id, seq, kind: "reply", read, created_at: "t", url: `/a/${A}` }) as unknown as InboxItem;

class FakePort {
  sent: WorkerToPanel[] = [];
  private gone: (() => void)[] = [];
  onDisconnect = { addListener: (f: () => void) => { this.gone.push(f); } };
  postMessage(m: WorkerToPanel) { this.sent.push(structuredClone(m)); }
  close() { for (const f of this.gone) f(); }
  last<T extends WorkerToPanel["t"]>(t: T) { return this.sent.filter(m => m.t === t).at(-1) as Extract<WorkerToPanel, { t: T }> | undefined; }
}

function harness(answer: () => unknown = () => ({ unread: 2, questions: [question(Q2, B, "2026-10-07T10:02:00Z"), question(Q1, A, "2026-10-07T10:01:00Z")], latest: [] })) {
  const hub: { id: string; msg: TabMsg | "detach" }[] = [];
  const calls: string[] = [];
  const timers: { fn: () => void; ms: number; live: boolean }[] = [];
  let reply = answer;
  const inbox = new WorkerInbox({
    api: {
      // The worker's credentialed API: each call is the request it would make.
      inboxSummary: async join => { calls.push(`GET /api/inbox/summary${join ? "" : " (may pair)"}`); const r = reply(); if (r instanceof Error) throw r; return r as never; },
      inboxPage: async (f, before) => { calls.push(`GET /api/inbox ${JSON.stringify(f)} ${before}`); return { items: [item(Q3, 9)], next_cursor: "9", unread: 5 }; },
    },
    hub: { receive: (id, msg) => hub.push({ id, msg }), detach: id => hub.push({ id, msg: "detach" }) },
    after: (fn, ms) => { const t = { fn, ms, live: true }; timers.push(t); return t; },
    cancel: h => { (h as { live: boolean }).live = false; },
  });
  const settle = () => new Promise(r => setTimeout(r, 0));
  return { inbox, hub, calls, timers, settle, answer: (f: () => unknown) => { reply = f; } };
}

describe("WorkerInbox", () => {
  it("checks the owner through the credentialed API before it subscribes, then fetches again when the topics go live", async () => {
    const h = harness();
    const port = new FakePort();
    h.inbox.attach(port as never);
    // Told at once (nothing known yet), and nothing subscribed before the daemon answered.
    expect(port.sent[0]).toEqual({ t: "inbox", owner: null, unread: 0, questions: [] });
    expect(h.hub).toEqual([]);
    await h.settle();
    expect(h.calls).toEqual(["GET /api/inbox/summary"]);
    expect(h.hub).toEqual([{ id: HUB_ID, msg: { t: "topics", topics: ["questions", "inbox"] } }]);
    expect(h.inbox.owner).toBe(true);
    // Open questions, oldest first.
    expect(port.last("inbox")).toMatchObject({ owner: true, unread: 2, questions: [{ id: Q1 }, { id: Q2 }] });
    // Subscribed first, then fetched (`live`), so no change falls between.
    h.answer(() => ({ unread: 4, questions: [question(Q1, A, "2026-10-07T10:01:00Z")], latest: [] }));
    h.inbox.fromHub({ t: "live", topics: ["questions", "inbox"] });
    await h.settle();
    expect(h.calls).toEqual(["GET /api/inbox/summary", "GET /api/inbox/summary"]);
    expect(port.last("inbox")).toMatchObject({ unread: 4, questions: [{ id: Q1 }] });
    // The hub's ping is answered, so it keeps the client.
    h.inbox.fromHub({ t: "ping" });
    expect(h.hub.at(-1)).toEqual({ id: HUB_ID, msg: { t: "ping" } });
  });

  it("subscribes nothing when the daemon does not take the extension as the owner", async () => {
    const h = harness(() => new ApiFailure("forbidden", "Only the owner may do this", 403));
    const port = new FakePort();
    h.inbox.attach(port as never);
    await h.settle();
    expect(h.hub).toEqual([]);
    expect(h.inbox.owner).toBe(false);
    expect(port.last("inbox")).toEqual({ t: "inbox", owner: false, unread: 0, questions: [] });
    expect(h.timers).toEqual([]);
  });

  it("checks again a few times after a backoff when the daemon fails, then waits for a kick", async () => {
    const h = harness(() => new ApiFailure("http_500", "boom", 500));
    const port = new FakePort();
    h.inbox.attach(port as never);
    await h.settle();
    expect(h.timers.map(t => t.ms)).toEqual([1000]);
    h.timers[0].fn();
    await h.settle();
    expect(h.timers.map(t => t.ms)).toEqual([1000, 2000]);
    for (let i = 0; i < 8; i++) { const t = h.timers.at(-1)!; if (t.live) { t.live = false; t.fn(); } await h.settle(); }
    // RETRY_LIMIT retries, then none until a kick.
    expect(h.timers.map(t => t.ms)).toEqual([1000, 2000, 4000, 8000, 16000, RETRY_MAX_MS].slice(0, RETRY_LIMIT));
    expect(h.calls).toHaveLength(RETRY_LIMIT + 1);
    h.answer(() => ({ unread: 1, questions: [], latest: [] }));
    h.inbox.kick();
    await h.settle();
    expect(h.hub.map(x => x.msg)).toEqual([{ t: "topics", topics: ["questions", "inbox"] }]);
    // The last panel closing stops the retries and the subscription.
    port.close();
    expect(h.hub.at(-1)).toEqual({ id: HUB_ID, msg: "detach" });
  });

  it("never starts a pairing itself: with none stored or under way, it waits for one", async () => {
    for (const code of ["not_paired", "host_missing", "daemon_unreachable", "daemon_unavailable"]) {
      const h = harness(() => new ApiFailure(code, "no"));
      const port = new FakePort();
      h.inbox.attach(port as never);
      await h.settle();
      // Joined only (it asks with `join`), and no timer: nothing runs again by itself.
      expect(h.calls).toEqual(["GET /api/inbox/summary"]);
      expect(h.timers).toEqual([]);
      expect(h.hub).toEqual([]);
      // A pairing landed (the panel watched its tab): the check runs again.
      h.answer(() => ({ unread: 0, questions: [], latest: [] }));
      h.inbox.kick();
      await h.settle();
      expect(h.calls).toHaveLength(2);
      expect(h.hub).toEqual([{ id: HUB_ID, msg: { t: "topics", topics: ["questions", "inbox"] } }]);
      // Subscribed: a kick does nothing more.
      h.inbox.kick();
      await h.settle();
      expect(h.calls).toHaveLength(2);
    }
  });

  it("runs a check again once when a kick came while it ran and it found no pairing", async () => {
    let fail!: (e: unknown) => void;
    let first = true;
    const h = harness(() => (first ? (first = false, new Promise((_, r) => { fail = r; })) : { unread: 1, questions: [], latest: [] }));
    const port = new FakePort();
    h.inbox.attach(port as never);
    await h.settle();
    // A pairing landed while the check waited.
    h.inbox.kick();
    h.inbox.kick();
    fail(new ApiFailure("not_paired", "Clax is not paired yet."));
    await h.settle();
    expect(h.calls).toHaveLength(2);
    expect(h.hub).toEqual([{ id: HUB_ID, msg: { t: "topics", topics: ["questions", "inbox"] } }]);
    expect(h.inbox.owner).toBe(true);
  });

  it("keeps nothing a check answered after the last panel closed", async () => {
    let answer!: (v: unknown) => void;
    const h = harness(() => new Promise(r => { answer = r; }));
    const port = new FakePort();
    h.inbox.attach(port as never);
    await h.settle();
    port.close();
    answer({ unread: 2, questions: [question(Q1, A, "2026-10-07T10:01:00Z")], latest: [] });
    await h.settle();
    expect(h.inbox.questions).toEqual([]);
    expect(h.hub).toEqual([]);
  });

  it("upserts questions from their events and takes the count from item events; every panel hears each", async () => {
    const h = harness();
    const p1 = new FakePort();
    const p2 = new FakePort();
    h.inbox.attach(p1 as never);
    h.inbox.attach(p2 as never);
    await h.settle();
    const q3 = question(Q3, A, "2026-10-07T10:00:00Z");
    h.inbox.apply("question", { topic: "questions", question: q3 });
    expect(h.inbox.questions.map(q => q.id)).toEqual([Q3, Q1, Q2]);
    expect(p2.last("q-event")).toEqual({ t: "q-event", question: q3 });
    h.inbox.apply("question", { topic: "questions", question: { ...question(Q1, A, "2026-10-07T10:01:00Z"), status: "answered" } });
    expect(h.inbox.questions.map(q => q.id)).toEqual([Q3, Q2]);
    // A question about A only, for a panel showing A.
    expect(h.inbox.forPage(A).map(q => q.id)).toEqual([Q3]);
    expect(h.inbox.forPage(B).map(q => q.id)).toEqual([Q2]);
    expect(h.inbox.forPage("9s5n1pza4d6w")).toEqual([]);
    h.inbox.apply("inbox_item", { topic: "inbox", item: item(Q3, 12), unread: 7 });
    expect(h.inbox.unread).toBe(7);
    expect(p1.last("inbox-event")).toEqual({ t: "inbox-event", item: item(Q3, 12), unread: 7 });
    // A bulk mark: ids null, so what the panels show is fetched again.
    h.inbox.apply("inbox_read", { topic: "inbox", ids: null, read: true, unread: 0 });
    expect(h.inbox.unread).toBe(0);
    expect(p2.last("inbox-event")).toEqual({ t: "inbox-event", item: null, unread: 0 });
  });

  it("passes a page's filter and cursor to the daemon, and its count to the panels", async () => {
    const h = harness();
    const port = new FakePort();
    h.inbox.attach(port as never);
    await h.settle();
    const r = await h.inbox.page({ q: "layout", read: "unread" }, "42");
    expect(h.calls.at(-1)).toBe(`GET /api/inbox {"q":"layout","read":"unread"} 42`);
    expect(r.items.map(i => i.id)).toEqual([Q3]);
    expect(port.last("inbox")).toMatchObject({ unread: 5 });
  });

  it("drops its client when the daemon refuses the topics, and when the last panel closes", async () => {
    const h = harness();
    const p1 = new FakePort();
    const p2 = new FakePort();
    h.inbox.attach(p1 as never);
    h.inbox.attach(p2 as never);
    await h.settle();
    p1.close();
    expect(h.hub.filter(x => x.msg === "detach")).toEqual([]);
    p2.close();
    expect(h.hub.at(-1)).toEqual({ id: HUB_ID, msg: "detach" });
    expect(h.inbox.questions).toEqual([]);
    // A panel again: checked and subscribed again.
    const p3 = new FakePort();
    h.inbox.attach(p3 as never);
    await h.settle();
    expect(h.hub.at(-1)).toEqual({ id: HUB_ID, msg: { t: "topics", topics: ["questions", "inbox"] } });
    h.inbox.fromHub({ t: "refused", topic: "inbox", code: "forbidden" });
    expect(h.hub.at(-1)).toEqual({ id: HUB_ID, msg: "detach" });
    expect(p3.last("inbox")).toEqual({ t: "inbox", owner: false, unread: 2, questions: [] });
  });
});
