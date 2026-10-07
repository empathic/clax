import { afterEach, describe, expect, it, vi } from "vitest";
import type { StreamEvent } from "../stream";
import { InboxFeed, LEAVE_MS, QuestionFeed, SUMMARY_MS } from "./feed.svelte";
import { item, view } from "./fixtures";

/** An event stream the test drives: what was watched, and its events handed back. */
function fakeStream() {
  const watched: { topics: readonly string[]; background?: boolean; off: boolean }[] = [];
  let on: ((e: StreamEvent) => void) | null = null;
  return {
    watched,
    emit: (type: string, d: Record<string, unknown> = {}) => on?.({ ...d, type }),
    watch(topics: readonly string[], f: (e: StreamEvent) => void, opts: { background?: boolean } = {}) {
      const w = { topics, background: opts.background, off: false };
      watched.push(w);
      on = f;
      return () => { w.off = true; on = null; };
    },
  };
}

/** The daemon: `answer` maps a request to its status and body; every request is kept. */
function stubFetch(answer: (url: string, init?: RequestInit) => [number, unknown]) {
  const calls: { url: string; method: string; body: unknown }[] = [];
  vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
    calls.push({ url, method: init?.method ?? "GET", body: init?.body ? JSON.parse(String(init.body)) : undefined });
    const [status, body] = answer(url, init);
    return new Response(JSON.stringify(body), { status });
  }));
  return calls;
}
const settle = () => new Promise(r => setTimeout(r, 0));

/** A clock the test moves by hand. */
function fakeClock() {
  let t = 0;
  const due: { at: number; fn: () => void }[] = [];
  return {
    after: (ms: number, fn: () => void) => { const d = { at: t + ms, fn }; due.push(d); return () => { due.splice(due.indexOf(d), 1); }; },
    advance(ms: number) { t += ms; for (const d of due.filter(x => x.at <= t)) { due.splice(due.indexOf(d), 1); d.fn(); } },
  };
}

afterEach(() => { vi.unstubAllGlobals(); });

describe("QuestionFeed", () => {
  const A = view({ id: "Q1", created_at: "2026-10-07T09:00:00.000Z" });
  const B = view({ id: "Q2", created_at: "2026-10-07T09:05:00.000Z", artifact: { id: "other", title: "Other", kind: "html" } });

  it("fetches, then follows `questions` in the background, fetching again when it goes live or resyncs", async () => {
    let list = [B];
    const calls = stubFetch(() => [200, { questions: list, open: list.length }]);
    const stream = fakeStream();
    const feed = new QuestionFeed();
    expect(await feed.start(stream)).toBe(true);
    expect(feed.owner).toBe(true);
    expect(stream.watched).toEqual([{ topics: ["questions"], background: true, off: false }]);
    expect(calls.map(c => c.url)).toEqual(["/api/questions?status=open&limit=200"]);
    list = [B, A];
    stream.emit("ready");
    await settle();
    // Oldest first.
    expect(feed.open.map(q => q.id)).toEqual(["Q1", "Q2"]);
    expect(feed.byArtifact("7q3k9mzx2b4t").map(q => q.id)).toEqual(["Q1"]);
    list = [B];
    stream.emit("resync", { topic: "questions" });
    await settle();
    expect(feed.open.map(q => q.id)).toEqual(["Q2"]);
    expect(calls).toHaveLength(3);
  });

  it("upserts each question event, and keeps a closed one for 4 s before it leaves", async () => {
    stubFetch(() => [200, { questions: [A], open: 1 }]);
    const stream = fakeStream();
    const clock = fakeClock();
    const feed = new QuestionFeed({ after: clock.after });
    await feed.start(stream);
    stream.emit("question", { question: B });
    expect(feed.cards.map(q => q.id)).toEqual(["Q1", "Q2"]);
    const answered = { ...A, status: "answered" as const, answers: [] };
    stream.emit("question", { question: answered });
    expect(feed.open.map(q => q.id)).toEqual(["Q2"]);
    expect(feed.recent.map(q => q.status)).toEqual(["answered"]);
    // Still a card, in its place, saying what closed it.
    expect(feed.cards.map(q => q.id)).toEqual(["Q1", "Q2"]);
    // A late open view of it changes nothing.
    stream.emit("question", { question: A });
    expect(feed.open.map(q => q.id)).toEqual(["Q2"]);
    clock.advance(LEAVE_MS - 1);
    expect(feed.recent).toHaveLength(1);
    clock.advance(1);
    expect(feed.recent).toEqual([]);
    expect(feed.cards.map(q => q.id)).toEqual(["Q2"]);
  });

  it("answers through the owner route, taking a 409's question as what closed it", async () => {
    const calls = stubFetch((url, init) => {
      if (init?.method !== "POST") return [200, { questions: [A, B], open: 2 }];
      if (url.includes("Q1")) return [200, { question: { ...A, status: "answered" } }];
      return [409, { error: { code: "question_closed", message: "closed" }, question: { ...B, status: "withdrawn" } }];
    });
    const feed = new QuestionFeed({ after: fakeClock().after });
    await feed.start(fakeStream());
    await feed.answer("Q1", { answers: [] });
    await feed.decline("Q2");
    expect(calls.filter(c => c.method === "POST").map(c => c.url)).toEqual(["/api/questions/Q1/answer", "/api/questions/Q2/decline"]);
    expect(feed.recent.map(q => q.status)).toEqual(["answered", "withdrawn"]);
  });

  it("stays empty and quiet for anyone but the owner", async () => {
    const calls = stubFetch(() => [403, { error: { code: "forbidden" } }]);
    const stream = fakeStream();
    const feed = new QuestionFeed();
    expect(await feed.start(stream)).toBe(false);
    expect(feed.owner).toBe(false);
    expect(feed.open).toEqual([]);
    expect(stream.watched).toEqual([]);
    expect(calls).toHaveLength(1);
  });

  it("does not subscribe when stopped while its first fetch was out", async () => {
    stubFetch(() => [200, { questions: [], open: 0 }]);
    const stream = fakeStream();
    const feed = new QuestionFeed();
    const started = feed.start(stream);
    feed.stop();
    expect(await started).toBe(false);
    expect(stream.watched).toEqual([]);
  });

  it("tells its listeners each change", async () => {
    stubFetch(() => [200, { questions: [], open: 0 }]);
    const stream = fakeStream();
    const feed = new QuestionFeed();
    await feed.start(stream);
    const heard = vi.fn();
    feed.listen(heard);
    stream.emit("question", { question: A });
    expect(heard).toHaveBeenCalledTimes(1);
  });
});

describe("InboxFeed", () => {
  const SUMMARY = { unread: 2, questions: [], latest: [item("reply")] };

  it("takes the unread count from every event, and fetches the summary again after a bulk change", async () => {
    const calls = stubFetch(() => [200, SUMMARY]);
    const stream = fakeStream();
    const feed = new InboxFeed({ after: fakeClock().after });
    expect(await feed.start(stream)).toBe(true);
    expect(stream.watched).toEqual([{ topics: ["inbox"], background: true, off: false }]);
    expect([feed.unread, feed.latest.length, feed.owner]).toEqual([2, 1, true]);
    const heard: unknown[] = [];
    feed.listen(c => heard.push(c));
    const counts: number[] = [];
    feed.onCount(n => counts.push(n));
    stream.emit("inbox_item", { item: item("question", { id: "I9", seq: 9 }), unread: 3 });
    expect(feed.unread).toBe(3);
    stream.emit("inbox_read", { ids: null, read: true, unread: 0 });
    expect(feed.unread).toBe(0);
    await settle();
    expect(calls.map(c => c.url)).toEqual(["/api/inbox/summary", "/api/inbox/summary"]);
    // The summary's count is the daemon's now.
    expect(feed.unread).toBe(2);
    expect(heard).toEqual([{ item: expect.objectContaining({ id: "I9" }) }, { refetch: true }]);
    expect(counts).toEqual([3, 0, 2]);
  });

  it("fetches `latest` once after a burst of item changes", async () => {
    const calls = stubFetch(() => [200, SUMMARY]);
    const stream = fakeStream();
    const clock = fakeClock();
    const feed = new InboxFeed({ after: clock.after });
    await feed.start(stream);
    for (let n = 0; n < 5; n++) stream.emit("inbox_item", { item: item("reply", { id: `R${n}`, seq: 20 + n }), unread: 3 + n });
    // A read item `latest` does not show, and questions, change nothing there.
    stream.emit("inbox_item", { item: item("version", { id: "V", read: true }), unread: 7 });
    clock.advance(SUMMARY_MS);
    await settle();
    expect(calls.map(c => c.url)).toEqual(["/api/inbox/summary", "/api/inbox/summary"]);
  });

  it("marks one item, and every item a search matches up to the newest shown", async () => {
    const calls = stubFetch(url => [200, url.endsWith("/read") && !url.endsWith("inbox/read") ? { item: item("reply", { read: true }), unread: 1 } : url.endsWith("inbox/read") ? { marked: 4, unread: 0 } : SUMMARY]);
    const feed = new InboxFeed({ after: fakeClock().after });
    await feed.start(fakeStream());
    expect((await feed.mark("01JA00000000000000000000AA", true)).read).toBe(true);
    expect(feed.unread).toBe(1);
    expect(await feed.markAll({ q: "dash", kind: ["reply"] }, 41)).toBe(4);
    expect(feed.unread).toBe(0);
    expect(calls.slice(1).map(c => [c.method, c.url, c.body])).toEqual([
      ["POST", "/api/inbox/01JA00000000000000000000AA/read", {}],
      ["POST", "/api/inbox/read", { all: true, filter: { q: "dash", kind: ["reply"] }, upto: 41 }],
    ]);
  });

  it("stays quiet for anyone but the owner, and its pages refuse", async () => {
    stubFetch(() => [403, { error: { code: "forbidden" } }]);
    const stream = fakeStream();
    const feed = new InboxFeed();
    expect(await feed.start(stream)).toBe(false);
    expect([feed.owner, feed.unread, stream.watched.length]).toEqual([false, 0, 0]);
    await expect(feed.page({})).rejects.toMatchObject({ status: 403 });
  });
});
