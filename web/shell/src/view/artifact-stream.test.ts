import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { StreamEvent } from "../stream";
import type { Thread } from "../threads";
import { ThreadSync } from "./thread-sync";

let on: ((e: StreamEvent) => void) | null = null;
/** The callers the module asked the page's stream to reopen as. */
const reopens: (string | undefined)[] = [];
/** The topics the module asked the page's stream to ask for again. */
const retried: string[][] = [];
vi.mock("../stream", () => ({
  pageStream: () => ({ watch: (_t: string[], f: (e: StreamEvent) => void) => { on = f; return () => { on = null; }; }, reconnect: (c?: string) => { reopens.push(c); }, retry: (t: string[]) => { retried.push([...t]); } }),
}));
const { ArtifactStream, keepApprover } = await import("./artifact-stream");
const { getToken } = await import("../api");

const A = "7q3k9mzx2b4t";
const comment = (id: string) => ({ id, thread_id: "t1", author_kind: "viewer", author_name: "V", via_harness: null, body: id, created_at: "x" });
const delta = (status: "open" | "resolved", count: number, last: string) =>
  ({ type: "thread", topic: `artifact:${A}`, artifact_id: A, thread: { id: "t1", status, comment_count: count, last_comment: comment(last) } }) as unknown as StreamEvent;

let answers: ((t: unknown) => void)[] = [];
beforeEach(() => {
  answers = [];
  // The token and viewer lookups answer at once; each thread fetch waits for the test.
  vi.stubGlobal("fetch", vi.fn((url: string) => url === "/api/token" ? Promise.resolve(new Response(JSON.stringify({ token: "tk" })))
    : url === "/api/viewers/me" ? Promise.resolve(new Response(JSON.stringify({ viewer: { public_id: "u_1", display_name: null, created_at: "x" } })))
    : new Promise(resolve => {
      answers.push(thread => resolve(new Response(JSON.stringify({ thread }), { status: 200 })));
    })));
});
afterEach(() => vi.unstubAllGlobals());

describe("the artifact stream", () => {
  it("asks for the owner's stream when a token request succeeds after a failed one", async () => {
    reopens.length = 0;
    let fail = true;
    vi.stubGlobal("fetch", vi.fn(async (url: string) => {
      if (url !== "/api/token") throw new Error(`unexpected ${url}`);
      return fail ? new Response("{}", { status: 500 }) : new Response(JSON.stringify({ token: "tk" }));
    }));
    expect(await getToken()).toBeNull();
    expect(reopens).toEqual([]);
    fail = false;
    expect(await getToken()).toBe("tk");
    expect(reopens).toEqual(["admin"]);
  });


  it("a thread fetched whole never undoes a delta that arrived while it was in flight", async () => {
    let threads: Thread[] = [];
    const sync = new ThreadSync(f => { threads = f(threads); });
    const s = new ArtifactStream(A, {
      threads: () => threads,
      presence: () => [],
      changeThreads: f => sync.change(f),
      beginThread: tid => sync.beginThread(tid),
      event: () => {},
      page: () => {},
      spectate: () => {},
      seated: () => {},
      disposed: () => false,
    });
    // Two comments counted, one known: the thread is fetched whole.
    on!(delta("open", 2, "c2"));
    expect(answers).toHaveLength(1);
    // Meanwhile it is resolved: still a comment short, so fetched again.
    on!(delta("resolved", 2, "c2"));
    expect(answers).toHaveLength(2);
    // The second fetch answers first; the first, from before the resolve, last.
    answers[1]({ id: "t1", status: "resolved", comments: [comment("c1"), comment("c2")] });
    await vi.waitFor(() => expect(threads[0].comments).toHaveLength(2));
    answers[0]({ id: "t1", status: "open", comments: [comment("c1"), comment("c2")] });
    await new Promise(r => setTimeout(r, 10));
    expect(threads).toHaveLength(1);
    expect(threads[0].status).toBe("resolved");
    s.stop();
  });
  it("refetches when the daemon refuses the artifact's topic (it was deleted before the view subscribed)", () => {
    const heard: unknown[] = [];
    const s = new ArtifactStream(A, {
      threads: () => [], presence: () => [], changeThreads: () => {}, beginThread: () => () => {},
      event: e => heard.push(e), page: () => {}, spectate: () => {}, seated: () => {}, disposed: () => false,
    });
    on!({ type: "refused", topic: `presence:${A}`, code: "not_found" });
    expect(heard).toEqual([]);
    on!({ type: "refused", topic: `artifact:${A}`, code: "not_found" });
    expect(heard).toEqual([{ type: "resync", topic: `artifact:${A}` }]);
    s.stop();
  });
  it("turns spectator when the daemon has no live seat, and asks again on Join live", () => {
    const heard: string[] = [];
    const s = new ArtifactStream(A, {
      threads: () => [], presence: () => [], changeThreads: () => {}, beginThread: () => () => {},
      event: e => heard.push(`event:${e.type}`), page: () => {}, spectate: c => heard.push(`spectate:${c}`), seated: () => heard.push("seated"), disposed: () => false,
    });
    on!({ type: "refused", topic: `artifact:${A}`, code: "no_live_seat" });
    on!({ type: "refused", topic: `working:${A}`, code: "seat_idle" });
    // A seat code on another artifact's topic is not this view's.
    on!({ type: "refused", topic: `artifact:other`, code: "no_live_seat" });
    expect(heard).toEqual(["spectate:no_live_seat", "spectate:seat_idle"]);
    retried.length = 0;
    s.rejoin();
    expect(retried).toEqual([[`artifact:${A}`, `presence:${A}`, `working:${A}`]]);
    on!({ type: "ready" });
    expect(heard.slice(2)).toEqual(["seated", "event:ready"]);
    s.stop();
  });
});

describe("keepApprover", () => {
  const s = (o: Record<string, unknown>) => ({ thread_id: "t", state: "withheld" as const, tier: null, since: "x", resends: 0, exhausted: false, ...o });
  it("keeps the approver a fetched view named when an event, which never names one, still waits", () => {
    expect(keepApprover(s({ approver: "Sam" }), s({})) ).toEqual(s({ approver: "Sam" }));
    expect(keepApprover(s({ approver: "Sam" }), s({ state: "sent", tier: "piggyback" }))).toEqual(s({ state: "sent", tier: "piggyback" }));
    expect(keepApprover(null, s({}))).toEqual(s({}));
    expect(keepApprover(s({ approver: "Sam" }), null)).toBeNull();
  });
});
