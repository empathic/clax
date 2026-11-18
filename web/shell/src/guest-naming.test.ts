import { afterEach, describe, expect, it, vi } from "vitest";
import type { Anchor } from "../../bridge/src/protocol";
import { freshModules } from "./test/modules";

// A hosted daemon's visitor has no guest session until PUT /api/viewers/me
// answers; every write that needs one waits for a name save in flight.

const anchor: Anchor = { kind: "artifact", selector: null, quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "" };
const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });
const viewer = (name: string) => ({ viewer: { public_id: "u_1", display_name: name, created_at: "x" } });
/** Lets every pending promise callback and fetch stub run. */
const settle = () => new Promise(r => setTimeout(r, 0));

/** A fetch whose name saves wait for `answer`; every other request answers at once. */
function server() {
  const sent: string[] = [];
  const answers: ((r: Response) => void)[] = [];
  vi.stubGlobal("fetch", vi.fn((url: string, init?: RequestInit) => {
    sent.push(`${init?.method ?? "GET"} ${url}`);
    if (url === "/api/viewers/me") return new Promise<Response>(r => { answers.push(r); });
    return Promise.resolve(json({ thread: { id: "t_1", comments: [] }, threads: [], sent: [], unchanged: [] }));
  }));
  return { sent, answer: (r: Response) => answers.shift()!(r) };
}

type Threads = typeof import("./threads");
const writes: [string, (t: Threads) => Promise<unknown>, string][] = [
  ["createThread", t => t.createThread("a_1", { anchor, body: "hi", version: 1, clip: null }), "POST /api/artifacts/a_1/threads"],
  ["addComment", t => t.addComment("a_1", "t_1", "hi"), "POST /api/artifacts/a_1/threads/t_1/comments"],
  ["addComment through a page", t => t.addComment("a_1", "t_1", "hi", true), "POST /api/artifacts/a_1/threads/t_1/comments"],
  ["sendToAgent", t => t.sendToAgent("a_1", "t_1"), "POST /api/artifacts/a_1/threads/t_1/send"],
  ["sendToAgent through a page", t => t.sendToAgent("a_1", "t_1", "h_1", true), "POST /api/artifacts/a_1/threads/t_1/send"],
  ["sendBatch", t => t.sendBatch("a_1", ["t_1"], null, null), "POST /api/artifacts/a_1/threads:send"],
  ["resolveThread", t => t.resolveThread("a_1", "t_1"), "POST /api/artifacts/a_1/threads/t_1/resolve"],
  ["reopenThread", t => t.reopenThread("a_1", "t_1"), "POST /api/artifacts/a_1/threads/t_1/reopen"],
];

describe("a guest's write while their name is being saved", () => {
  afterEach(async () => { vi.unstubAllGlobals(); await freshModules(); });

  it.each(writes)("%s is sent only once the save answered", async (_, write, request) => {
    const s = server();
    const t: Threads = await import("./threads");
    const saved = t.setViewerName("Mia");
    const done = write(t);
    await settle();
    expect(s.sent).toEqual(["PUT /api/viewers/me"]);
    s.answer(json(viewer("Mia")));
    await expect(saved).resolves.toMatchObject({ display_name: "Mia" });
    await done;
    expect(s.sent).toEqual(["PUT /api/viewers/me", request]);
  });

  it.each(writes)("%s is still sent, once, when the save fails", async (_, write, request) => {
    const s = server();
    const t: Threads = await import("./threads");
    const saved = t.setViewerName("Mia");
    const done = write(t);
    await settle();
    expect(s.sent).toEqual(["PUT /api/viewers/me"]);
    s.answer(json({ error: { code: "name_taken", message: "the name 'Mia' is taken" } }, 409));
    await expect(saved).rejects.toMatchObject({ status: 409 });
    await done;
    expect(s.sent).toEqual(["PUT /api/viewers/me", request]);
  });

  it("a write with no save in flight goes at once", async () => {
    const s = server();
    const t: Threads = await import("./threads");
    void t.addComment("a_1", "t_1", "hi");
    expect(s.sent).toEqual(["POST /api/artifacts/a_1/threads/t_1/comments"]);
  });
});
