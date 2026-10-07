import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "../api";
import { answerQuestion, declineQuestion, inboxQuery, inboxSummary, listInbox, listQuestions, markInbox, markInboxMany, releaseQuestion } from "./api";
import { view } from "./fixtures";

type Call = { url: string; init?: RequestInit };
/** Stubs `fetch` with one response per call, recording each request. */
function stub(...responses: Response[]): Call[] {
  const calls: Call[] = [];
  vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
    calls.push({ url, init });
    const r = responses.shift();
    if (!r) throw new Error(`no response for ${url}`);
    return r;
  }));
  return calls;
}
const json = (status: number, body: unknown) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
const body = (c: Call) => JSON.parse(String(c.init?.body));

afterEach(() => { vi.unstubAllGlobals(); });

describe("question and inbox fetchers", () => {
  it("answers, skips and moves with the right requests", async () => {
    const q = view();
    const calls = stub(json(200, { question: q }), json(200, { question: q }), json(200, { question: q }));
    const answers = { answers: [{ selected: ["Two"], text: null }] };
    expect(await answerQuestion("01J9", answers)).toEqual({ question: q });
    expect(await declineQuestion("01J9")).toEqual({ question: q });
    expect(await releaseQuestion("01J9")).toEqual({ question: q });
    expect(calls.map(c => [c.url, c.init?.method])).toEqual([
      ["/api/questions/01J9/answer", "POST"], ["/api/questions/01J9/decline", "POST"], ["/api/questions/01J9/release", "POST"],
    ]);
    expect(body(calls[0])).toEqual(answers);
    expect(new Headers(calls[0].init!.headers).get("content-type")).toBe("application/json");
  });

  it("resolves a 409 with the question to it as closed, for each closing verb", async () => {
    const closed = view({ status: "withdrawn" });
    const conflict = () => json(409, { error: { code: "question_closed", message: "closed" }, question: closed });
    stub(conflict(), conflict(), conflict());
    expect(await answerQuestion("q", { answers: [] })).toEqual({ closed });
    expect(await declineQuestion("q")).toEqual({ closed });
    expect(await releaseQuestion("q")).toEqual({ closed });
  });

  it("throws a 409 without a question, and other failures, as ApiError with the daemon's code", async () => {
    stub(json(409, { error: { code: "question_closed", message: "closed" } }), json(400, { error: { code: "invalid_answer", message: "\"Notes\": it has no answer" } }),
      new Response("oops", { status: 500, statusText: "Internal Server Error" }), json(409, { error: { code: "conflict", message: "x" }, question: view() }));
    await expect(answerQuestion("q", { answers: [] })).rejects.toMatchObject({ status: 409, code: "question_closed" });
    const e = await answerQuestion("q", { answers: [] }).catch(x => x);
    expect(e).toBeInstanceOf(ApiError);
    expect(e).toMatchObject({ status: 400, code: "invalid_answer", message: "400 \"Notes\": it has no answer" });
    await expect(declineQuestion("q")).rejects.toMatchObject({ status: 500, code: null, message: "500 Internal Server Error" });
    // Outside the closing verbs a 409 is an error like any other.
    await expect(markInbox("i", true)).rejects.toMatchObject({ status: 409 });
  });

  it("resolves a 403 to forbidden on every route", async () => {
    const no = () => json(403, { error: { code: "forbidden", message: "owner only" } });
    stub(...Array.from({ length: 9 }, no));
    for (const r of [listQuestions(), answerQuestion("q", { answers: [] }), declineQuestion("q"), releaseQuestion("q"), listInbox({}), inboxSummary(), markInbox("i", false), markInboxMany({ ids: ["i"] }), markInboxMany({ all: true })]) {
      expect(await r).toBe("forbidden");
    }
  });

  it("asks for questions and inbox pages with the API's query parameters", async () => {
    const calls = stub(json(200, { questions: [], open: 0 }), json(200, { items: [], next_cursor: null, unread: 0 }), json(200, { unread: 0, questions: [], latest: [] }), json(200, { item: {}, unread: 1 }), json(200, { item: {}, unread: 0 }));
    await listQuestions("all", 200);
    await listInbox({ q: " dash board ", kind: ["reply", "question"], agent: "claude", read: "unread" }, "4211", 50);
    await inboxSummary();
    await markInbox("b0123456789abcdef01234567", false);
    await markInbox("01JA", true);
    expect(calls.map(c => c.url)).toEqual([
      "/api/questions?status=all&limit=200",
      "/api/inbox?q=dash+board&kind=reply%2Cquestion&agent=claude&read=unread&before=4211&limit=50",
      "/api/inbox/summary",
      "/api/inbox/b0123456789abcdef01234567/unread",
      "/api/inbox/01JA/read",
    ]);
    expect(inboxQuery({})).toBe("");
    expect(inboxQuery({ q: "   ", kind: [], since: "2026-10-01", until: "2026-10-07", artifact: "7q3k9mzx2b4t" })).toBe("?artifact=7q3k9mzx2b4t&since=2026-10-01&until=2026-10-07");
  });

  it("marks many with the body the daemon takes: IDs, or all with a filter (never read state) and upto", async () => {
    const calls = stub(...Array.from({ length: 4 }, () => json(200, { marked: 2, unread: 0 })));
    expect(await markInboxMany({ ids: ["a", "b"] })).toEqual({ marked: 2, unread: 0 });
    await markInboxMany({ all: true });
    await markInboxMany({ all: true, filter: { q: " ", kind: [], read: "unread" } });
    await markInboxMany({ all: true, filter: { q: "dash", kind: ["reply"], artifact: "7q3k9mzx2b4t", agent: "a_1f3a00", since: "2026-10-01", until: "2026-10-07", read: "unread" }, upto: 4211 });
    expect(calls.every(c => c.url === "/api/inbox/read" && c.init?.method === "POST")).toBe(true);
    expect(calls.map(body)).toEqual([
      { ids: ["a", "b"] },
      { all: true },
      { all: true },
      { all: true, filter: { q: "dash", kind: ["reply"], artifact: "7q3k9mzx2b4t", agent: "a_1f3a00", since: "2026-10-01", until: "2026-10-07" }, upto: 4211 },
    ]);
  });
});
