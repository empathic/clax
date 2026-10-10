import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { StreamEvent } from "../stream";
import type { Thread } from "../threads";
import { ThreadSync } from "./thread-sync";

let on: ((e: StreamEvent) => void) | null = null;
vi.mock("../stream", () => ({
  pageStream: () => ({ watch: (_t: string[], f: (e: StreamEvent) => void) => { on = f; return () => { on = null; }; }, reconnect: () => {} }),
}));
const { ArtifactStream } = await import("./artifact-stream");

const A = "7q3k9mzx2b4t";
const comment = (id: string) => ({ id, thread_id: "t1", author_kind: "viewer", author_name: "V", via_harness: null, body: id, created_at: "x" });
const delta = (status: "open" | "resolved", count: number, last: string) =>
  ({ type: "thread", topic: `artifact:${A}`, artifact_id: A, thread: { id: "t1", status, comment_count: count, last_comment: comment(last) } }) as unknown as StreamEvent;

let answers: ((t: unknown) => void)[] = [];
beforeEach(() => {
  answers = [];
  // The viewer lookup (the stream notes the caller it opens as) answers at
  // once; each thread fetch waits for the test.
  vi.stubGlobal("fetch", vi.fn((url: string) => url === "/api/token" ? Promise.resolve(new Response(JSON.stringify({ token: "tk" })))
    : url === "/api/viewers/me" ? Promise.resolve(new Response(JSON.stringify({ viewer: { public_id: "u_1", display_name: null, created_at: "x" } })))
    : new Promise(resolve => {
      answers.push(thread => resolve(new Response(JSON.stringify({ thread }), { status: 200 })));
    })));
});
afterEach(() => vi.unstubAllGlobals());

describe("the artifact stream", () => {
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
      event: e => heard.push(e), page: () => {}, disposed: () => false,
    });
    on!({ type: "refused", topic: `presence:${A}`, code: "not_found" });
    expect(heard).toEqual([]);
    on!({ type: "refused", topic: `artifact:${A}`, code: "not_found" });
    expect(heard).toEqual([{ type: "resync", topic: `artifact:${A}` }]);
    s.stop();
  });
});
